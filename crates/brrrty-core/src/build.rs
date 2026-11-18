//! The build pipeline: discover, parse, resolve, render, write, copy.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use brrrty_js::Value;
use brrrty_liquid::{Engine, EngineOptions, Tz};
use brrrty_markdown::MarkdownRenderer;
use brrrty_nunjucks::EngineOptions as NunjucksOptions;
use indexmap::IndexMap;
use rayon::prelude::*;
use serde_json::Value as Json;

use crate::collections::{self, Entry, Membership, Page};
use crate::content::{self, ContentPage};
use crate::data::{
    BUILD_KEYS, ComputedPlan, DirDataIndex, GlobalData, Local, OVERRIDE_PREFIX, clean_tags,
    config_global, frozen_keys, frozen_problem, is_js, js_help, js_object, key_line, load_global,
    merge, page_data, reserved_keys, reserved_problem,
};
use crate::discover::{PLUGIN_ONLY_EXTS, discover};
use crate::frontmatter::{Data, split_front_matter};
use crate::layout::{ChainError, LayoutFile, Layouts};
use crate::loader::FsLoader;
use crate::options::{BuildOptions, TemplateEngines};
use crate::order::{OrderLayout, OrderTemplate};
use crate::page::{file_path_stem, file_slug, resolve_date};
use crate::paginate::{Links, Spec};
use crate::passthrough::copy_passthrough_avoiding;
use crate::permalink::{Located, Locator, Permalink};
use crate::plugins::rss::{FeedOptions, FeedTemplate};
use crate::problem::{Problem, Severity, codes};
use crate::render::{DataEngine, Engines, Job, Pipeline, body_line, engines_for};
use crate::survey::{Log, Mode};
use crate::templates::Langs;
use brrrty_js::traps::{self, Outcome};

/// What a build did.
#[derive(Debug)]
pub struct BuildReport {
    pub written: usize,
    pub copied: usize,
    pub unchanged: usize,
    pub problems: Vec<Problem>,
    pub elapsed: Duration,
    /// The pages written, in no particular order.
    pub pages: Vec<WrittenPage>,
    /// Each passthrough file now in place: its source relative to the project folder, and
    /// its destination relative to the folder written to.
    pub passthrough: Vec<(PathBuf, PathBuf)>,
    /// The build got as far as writing pages. False when it stopped before any page could
    /// be written: the folders or the settings are wrong, discovery failed, or `strict` met
    /// an error early.
    pub complete: bool,
    /// The templates, relative to the project folder, that had a page left unwritten because
    /// of a problem, in no particular order.
    pub refused: Vec<PathBuf>,
    /// What a [`survey`] build recorded; `None` for a plain build.
    pub survey: Option<crate::survey::Survey>,
    /// The files and folders the config and data scripts read, listed or checked, relative
    /// to the project folder and sorted: what the output depends on besides the sources.
    pub reads: Vec<PathBuf>,
}

/// A page a build wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenPage {
    /// The template, relative to the project folder.
    pub source: PathBuf,
    /// Its `page.url`.
    pub url: String,
    /// Where it was written, relative to the folder written to.
    pub output: PathBuf,
    /// A hash of the bytes written, to tell whether another copy holds the same bytes
    /// without reading it. Only meaningful within one run of the program.
    pub hash: u64,
}

/// The hash [`WrittenPage::hash`] holds for `bytes`.
pub fn content_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

impl BuildReport {
    pub fn has_errors(&self) -> bool {
        self.problems
            .iter()
            .any(|p| matches!(p.severity, Severity::Error))
    }
}

/// True when `path` is lexically inside `base` and is not `base` itself.
pub(crate) fn is_inside(base: &Path, path: &Path) -> bool {
    let normalized = crate::options::normalize(path);
    normalized != base && normalized.starts_with(base)
}

/// Join `rel` onto `out`, refusing anything that is not a plain relative path.
fn contained_output_path(out: &Path, rel: &Path) -> Option<PathBuf> {
    if rel.as_os_str().is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let full = out.join(rel);
    is_inside(out, &full).then_some(full)
}

/// What every page of one template shares.
struct Source {
    /// Relative to the input folder.
    input_path: PathBuf,
    /// The template's path relative to the project root.
    shown: PathBuf,
    /// The canonical file; `None` for a virtual template.
    real: Option<PathBuf>,
    body: String,
    body_line: usize,
    engines: Engines,
    layouts: Vec<Arc<LayoutFile>>,
    /// Keys left out of the data because Brrrty can't supply their value.
    withheld: Vec<(String, Withheld)>,
    /// A feed `feedPlugin` adds: Brrrty renders its Nunjucks template itself.
    feed: Option<Arc<FeedTemplate>>,
}

/// One page to write: a template, or one page pagination made from it.
struct Prepared {
    source: Arc<Source>,
    /// Where it is written; `None` for `permalink: false`, which is still a
    /// page of the collections.
    output_rel: Option<PathBuf>,
    data: IndexMap<String, Value>,
    /// `pagination.pageNumber` of a generated page.
    page_number: Option<usize>,
}

impl Prepared {
    /// The page as problems name it.
    fn label(&self) -> String {
        match self.page_number {
            Some(n) => format!("{} (pagination page {n})", self.source.shown.display()),
            None => self.source.shown.display().to_string(),
        }
    }
}

/// What loading one template gave.
enum Loaded {
    /// A template that loaded.
    Template(Box<Templ>),
    /// A problem with this page.
    Failed(Problem),
    /// A problem in a layout file, the same for every page that uses it.
    LayoutFailed(Problem),
    /// A problem in a data file, the same for every page it applies to.
    DataFailed(Problem),
}

/// A template that loaded.
struct Templ {
    /// Relative to the input folder.
    rel: PathBuf,
    membership: Membership,
    /// Computed data sets `tags` or `eleventyExcludeFromCollections`, which
    /// Brrrty can't follow into the collections yet.
    computed_membership: bool,
    /// What decides when 11ty adds the template to its map (see
    /// [`crate::order`]).
    order: OrderTemplate,
    /// The layouts its data names, by the names they are named with.
    order_layouts: Vec<(String, OrderLayout)>,
    /// A JavaScript data file applies to it.
    js_data: bool,
    pages: Pages,
    /// Why Brrrty can't date its pages. It has none, and the collections it
    /// is in are refused.
    date_problem: Option<Problem>,
}

/// The pages of a template: made, or waiting for the collections they
/// paginate.
enum Pages {
    /// Every page of the template, including ones with no output. Empty for
    /// empty pagination.
    Ready(Vec<Prepared>),
    Deferred(Box<Deferred>),
}

/// A template that paginates a collection: it can only be expanded once the
/// collections it reads exist, in 11ty's order.
struct Deferred {
    source: Arc<Source>,
    values: Data,
    plan: Option<ComputedPlan>,
    spec: Spec,
    rel: PathBuf,
    shown: PathBuf,
    data_engine: DataEngine,
    /// Where `permalink` and `pagination` come from, when not the page.
    permalink_from: Option<String>,
    permalink_line: Option<usize>,
    pagination_from: Option<String>,
    pagination_line: Option<usize>,
}

/// What every page shares while loading.
struct Shared<'a> {
    langs: &'a Langs<'a>,
    /// `markdownTemplateEngine` and `htmlTemplateEngine`.
    template_engines: TemplateEngines,
    input: &'a Path,
    input_rel: &'a Path,
    layouts: &'a Layouts<'a>,
    /// The output folder as 11ty spells it in `page.outputPath`.
    output_shown: String,
    /// Global data: `_data`, then `eleventy` and `pkg`.
    global: Data,
    /// Top-level global data keys Brrrty can't supply, and why.
    withheld: BTreeMap<String, Withheld>,
    /// Directory and template data files.
    dir_data: &'a DirDataIndex,
    data_dir: PathBuf,
    /// Virtual templates (the feeds), by their path in the input folder.
    virtuals: &'a HashMap<PathBuf, FeedOptions>,
    /// The survey being recorded, in a survey build.
    survey: Option<&'a Log>,
}

/// Why a data key has no value.
#[derive(Clone, PartialEq, Eq)]
enum Withheld {
    /// This JavaScript data file sets it.
    Js(PathBuf),
    /// A global data file under it failed to load, and was reported.
    Failed,
}

impl Shared<'_> {
    /// Refuse every page when a global key Brrrty can't supply changes how
    /// pages are built.
    fn check_withheld(&self) -> Result<(), Problem> {
        for (key, why) in &self.withheld {
            let clean = key.strip_prefix(OVERRIDE_PREFIX).unwrap_or(key);
            if !BUILD_KEYS.contains(&clean) {
                continue;
            }
            return Err(match why {
                Withheld::Js(file) => Problem::error(
                    codes::DATA_JS_UNSUPPORTED,
                    format!(
                        "this JavaScript data file sets `{key}`, which changes how every page is built"
                    ),
                )
                .with_path(file)
                .with_help(js_help(file)),
                Withheld::Failed => Problem::error(
                    codes::DATA_UNSUPPORTED,
                    format!(
                        "global data key `{key}` comes from a data file that failed to load, which changes how every page is built"
                    ),
                )
                .with_path(&self.data_dir)
                .following("Fix the data file errors reported above."),
            });
        }
        Ok(())
    }
}

/// 11ty's spelling of a folder: `./src/`, `../x/`, or an absolute path, with
/// a trailing slash.
fn dot_dir(p: &Path) -> String {
    let joined = p
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    if p.is_absolute() {
        let shown = p.display().to_string();
        return format!("{}/", shown.trim_end_matches('/'));
    }
    if joined.is_empty() {
        "./".to_string()
    } else if joined == ".." || joined.starts_with("../") {
        format!("{joined}/")
    } else {
        format!("./{joined}/")
    }
}

fn slash_path(p: &Path) -> String {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether 11ty finds a page's date without reading the disk
/// (`Template.getMappedDate`). It reads the disk (a `stat`, or `git`) for
/// `Last Modified`, `Created` and the `git` forms, and for a page with no
/// truthy `date` whose input path has no `YYYY-MM-DD` in it. A date in the
/// data, or the first such date in the input path (`./posts/2020-03-04-x.md`,
/// `./2020-03-04/x.md`), needs no disk.
fn reads_no_file_date(date: Option<&Value>, input_path: &str) -> bool {
    match date {
        Some(Value::String(s)) if !s.is_empty() => {
            let s = s.to_lowercase();
            ![
                "last modified",
                "created",
                "git last modified",
                "git created",
            ]
            .contains(&s.as_str())
        }
        Some(v) if v.js_truthy() => true,
        _ => has_path_date(input_path),
    }
}

/// `/(\d{4}-\d{2}-\d{2})/` matches somewhere in `path`.
fn has_path_date(path: &str) -> bool {
    let b = path.as_bytes();
    let shape = b"dddd-dd-dd";
    b.windows(shape.len()).any(|w| {
        w.iter().zip(shape).all(|(c, s)| match s {
            b'd' => c.is_ascii_digit(),
            _ => c == s,
        })
    })
}

/// The data 11ty gives every page: `eleventy`, then `pkg` (the project's
/// `package.json`, or `{}`).
fn globals(opts: &BuildOptions, root: &Path, input_rel: &Path) -> (Data, String) {
    let input = dot_dir(input_rel);
    let output = crate::options::normalize(&opts.output);
    let output_shown = dot_dir(&output);
    let eleventy = serde_json::json!({
        "version": ELEVENTY_VERSION,
        "generator": format!("Eleventy v{ELEVENTY_VERSION}"),
        "env": {
            "source": "cli",
            "runMode": opts.run_mode.as_str(),
            "root": root.display().to_string(),
        },
        "directories": {
            "input": input,
            "data": format!("{input}_data/"),
            "includes": format!("{input}_includes/"),
            "output": output_shown,
        },
    });
    let pkg = fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Json>(&t).ok())
        .filter(Json::is_object)
        .unwrap_or_else(|| Json::Object(Default::default()));
    let mut g = Data::new();
    g.insert("eleventy".to_string(), Value::from_json(&eleventy));
    g.insert("pkg".to_string(), Value::from_json(&pkg));
    (g, output_shown)
}

/// The version of 11ty whose output Brrrty reproduces.
///
/// When bumping it: rerun `crates/brrrty-core/tools/order-calibration`
/// (`calibrate.py`, see its README) until it reports 0 mismatches, updating
/// `src/order.rs` if 11ty's loading steps changed; then rerun
/// `cargo xtask fixtures-refresh order`,
/// `cargo xtask fixtures-refresh order-data` and
/// `cargo xtask fixtures-refresh order-dates`.
const ELEVENTY_VERSION: &str = "3.1.8";

fn at(p: Problem, line: Option<usize>) -> Problem {
    match line {
        Some(l) => p.at(l, 1),
        None => p,
    }
}

/// Load one template: its front matter, layouts, data and output path.
/// Problems carry paths relative to the project root.
fn load(shared: &Shared<'_>, rel: &Path) -> Loaded {
    match load_page(shared, rel) {
        Ok(l) => l,
        Err(p) => Loaded::Failed(p),
    }
}

/// The layout chain `layout` names, for the page at `shown`.
fn layout_chain(
    shared: &Shared<'_>,
    layout: Option<&Value>,
    shown: &Path,
    text: &str,
) -> Result<Result<Vec<Arc<LayoutFile>>, Loaded>, Problem> {
    match layout {
        None => Ok(Ok(Vec::new())),
        Some(v) if !v.js_truthy() => Ok(Ok(Vec::new())),
        Some(Value::String(name)) => match shared.layouts.chain(name, shown, text) {
            Ok(chain) => Ok(Ok(chain)),
            Err(ChainError::Page(p)) => Err(p),
            Err(ChainError::Layout(p)) => Ok(Err(Loaded::LayoutFailed(p))),
        },
        Some(other) => Err(at(
            Problem::error(
                codes::LAYOUT_INVALID,
                format!(
                    "`layout` must be a layout name, found {}",
                    crate::data::json_text(other)
                ),
            )
            .with_path(shown)
            .with_help("write the layout's name, like `layout: base`"),
            key_line(text, "layout"),
        )),
    }
}

/// Every string in `value`, with its key path from `path`: the values 11ty renders as
/// templates under `permalink` and `eleventyComputed`.
fn template_strings(
    path: &str,
    value: &Value,
    line: Option<usize>,
    out: &mut Vec<(String, String, Option<usize>)>,
) {
    match value {
        Value::String(s) => out.push((path.to_string(), s.to_string(), line)),
        Value::Object(o) => {
            for (k, v) in o.iter() {
                template_strings(&format!("{path}.{k}"), v, line, out);
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                template_strings(&format!("{path}[{i}]"), v, line, out);
            }
        }
        _ => {}
    }
}

/// The problem for `tags` that are neither a string nor a list.
fn tags_problem(bad: &Value) -> Problem {
    Problem::error(
        codes::TAGS_INVALID,
        format!(
            "`tags` must be a string or a list, found {}",
            crate::data::json_text(bad)
        ),
    )
    .with_help("11ty stops the whole build here.")
}

/// Where a page's merged `key` comes from, for messages: `None` for front
/// matter, else the data file, layout or data folder.
fn source_of(
    key: &str,
    front: &Data,
    local: &Local,
    layouts: &[Arc<LayoutFile>],
    data_dir: &Path,
) -> Option<String> {
    if front.contains_key(key) {
        return None;
    }
    if let Some(file) = local.source.get(key) {
        return Some(file.display().to_string());
    }
    if let Some(l) = layouts.iter().find(|l| l.data.contains_key(key)) {
        return Some(format!("layout {}", l.shown.display()));
    }
    Some(data_dir.display().to_string())
}

/// Load one template. A page whose layout Brrrty can't render and that
/// fails for another reason too reports only the layout's problem, once for
/// all its pages, as it did before the layout became placeable.
fn load_page(shared: &Shared<'_>, rel: &Path) -> Result<Loaded, Problem> {
    let mut blocked = None;
    let loaded = load_page_inner(shared, rel, &mut blocked);
    if let Some(log) = shared.survey {
        let shown = shared.input_rel.join(rel);
        match (&loaded, &blocked) {
            (Err(p), _) | (Ok(Loaded::DataFailed(p)), _) | (Ok(Loaded::LayoutFailed(p)), None) => {
                log.stopped(&shown, p)
            }
            _ => {}
        }
    }
    match (loaded, blocked) {
        (Err(hidden), Some(layout)) => {
            // The layout is reported for the page; a survey keeps the page's own problem too.
            if let Some(log) = shared.survey {
                log.problem(&shared.input_rel.join(rel), &hidden);
            }
            Ok(Loaded::LayoutFailed(layout))
        }
        (other, _) => other,
    }
}

fn load_page_inner(
    shared: &Shared<'_>,
    rel: &Path,
    blocked: &mut Option<Problem>,
) -> Result<Loaded, Problem> {
    let shown = shared.input_rel.join(rel);
    let full = shared.input.join(rel);
    let feed = shared.virtuals.get(rel);
    let text = match feed {
        // A virtual template's text and data come from the config.
        Some(f) => f.content(),
        None => {
            let bytes = fs::read(&full).map_err(|e| {
                Problem::error(codes::TEMPLATE_UNREADABLE, format!("cannot read file: {e}"))
                    .with_path(&shown)
            })?;
            String::from_utf8(bytes).map_err(|_| {
                Problem::error(codes::TEMPLATE_NOT_UTF8, "file is not valid UTF-8")
                    .with_path(&shown)
                    .with_help("save the file as UTF-8")
            })?
        }
    };
    let (mut front, body) = split_front_matter(&text).map_err(|p| p.with_path(&shown))?;
    if let Some(f) = feed {
        // `Object.assign(frontMatterData, virtualTemplateData)`.
        for (k, v) in &f.data {
            front.insert(k.clone(), v.clone());
        }
    }

    // 11ty refuses reserved names and bad `tags` anywhere in the data, and
    // additions to its frozen `eleventy` and `pkg`.
    let reserved = reserved_keys(&front);
    if !reserved.is_empty() {
        let key = reserved[0].split('.').next().unwrap_or("");
        return Err(at(
            reserved_problem(&reserved).with_path(&shown),
            key_line(&text, key),
        ));
    }
    if let Err(bad) = clean_tags(&mut front) {
        return Err(at(
            tags_problem(&bad).with_path(&shown),
            key_line(&text, "tags"),
        ));
    }
    let frozen = frozen_keys(&front);
    if let Some(key) = frozen.first() {
        return Err(at(
            frozen_problem(&frozen).with_path(&shown),
            key_line(&text, key),
        ));
    }
    if let Err(p) = shared.check_withheld() {
        return Ok(Loaded::DataFailed(p));
    }
    let local = match shared.dir_data.local(rel) {
        Ok(l) => l,
        Err(p) => return Ok(Loaded::DataFailed(p.clone())),
    };

    // The layout whose front matter joins the data is the first truthy
    // `layout` of front matter, data files and global data; the one that
    // wraps the page is the merged `layout` (probed).
    let data_layout = [
        front.get("layout"),
        local.data.get("layout"),
        shared.global.get("layout"),
    ]
    .into_iter()
    .flatten()
    .find(|v| v.js_truthy());
    let data_layouts = match layout_chain(shared, data_layout, &shown, &text)? {
        Ok(chain) => chain,
        Err(loaded) => return Ok(loaded),
    };
    let mut layout_data = Data::new();
    for layout in data_layouts.iter().rev() {
        let mut d = layout.data.clone();
        let reserved = reserved_keys(&d);
        if !reserved.is_empty() {
            return Ok(Loaded::LayoutFailed(
                reserved_problem(&reserved).with_path(&layout.shown),
            ));
        }
        if let Err(bad) = clean_tags(&mut d) {
            return Ok(Loaded::LayoutFailed(
                tags_problem(&bad).with_path(&layout.shown),
            ));
        }
        let frozen = frozen_keys(&d);
        if !frozen.is_empty() {
            return Ok(Loaded::LayoutFailed(
                frozen_problem(&frozen).with_path(&layout.shown),
            ));
        }
        merge(&mut layout_data, &d);
    }
    layout_data.shift_remove("layout");

    let mut data = page_data(&shared.global, &layout_data, &local.data, &front);
    if let Some(log) = shared.survey
        && data.contains_key("pagination")
    {
        log.paginates(&shown);
    }

    let layouts = match data.get("layout") {
        v if v == data_layout => data_layouts.clone(),
        v => match layout_chain(shared, v, &shown, &text)? {
            Ok(chain) => chain,
            Err(loaded) => return Ok(loaded),
        },
    };
    *blocked = layouts.iter().find_map(|l| l.unrenderable()).cloned();
    if let Some(log) = shared.survey {
        log.layouts(&shown, layouts.iter().map(|l| l.shown.clone()).collect());
    }

    // Keys whose value Brrrty can't know are left out, so a page that reads
    // one is caught when it renders.
    let mut withheld: Vec<(String, Withheld)> = shared
        .withheld
        .iter()
        .map(|(k, w)| (k.clone(), w.clone()))
        .collect();
    for (key, file) in &local.js {
        withheld.retain(|(k, _)| k != key);
        withheld.push((key.clone(), Withheld::Js(file.clone())));
    }
    for (key, _) in &withheld {
        data.shift_remove(key);
    }

    let source = |key: &str| source_of(key, &front, &local, &data_layouts, &shared.data_dir);
    let meta = fs::metadata(&full).ok().filter(|_| feed.is_none());
    // A virtual template has no file: 11ty dates it `new Date()`.
    let now = || match brrrty_js::clock::source_date_epoch() {
        Ok(Some(ms)) => SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms.max(0) as u64),
        _ => SystemTime::now(),
    };
    let (created, changed) = match &meta {
        // With no creation time (Node's birthtimeMs 0), 11ty dates the page
        // by the build time, for `Last Modified` too.
        Some(m) => match crate::birth::created(&full, m) {
            Some(c) => (c, crate::birth::changed(m)),
            None => {
                let t = now();
                (t, t)
            }
        },
        None if feed.is_some() => {
            let t = now();
            (t, t)
        }
        None => (SystemTime::UNIX_EPOCH, SystemTime::UNIX_EPOCH),
    };
    // A page Brrrty can't date is refused on its own: it keeps its place in
    // the order, and only the collections it is in are refused.
    let (date, date_problem) = match resolve_date(&data, &shown, created, changed) {
        Ok(d) => (d, None),
        Err(p) => {
            let p = p.with_path(&shown);
            let p = if front.get("date").is_some_and(Value::js_truthy) {
                at(p, key_line(&text, "date"))
            } else if let Some(from) = data
                .get("date")
                .filter(|v| v.js_truthy())
                .and_then(|_| source("date"))
            {
                Problem {
                    message: format!("{} (`date` from {from})", p.message),
                    ..p
                }
            } else {
                p
            };
            (chrono::DateTime::<chrono::Utc>::UNIX_EPOCH, Some(p))
        }
    };
    let ext = rel
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let over = data.get("templateEngineOverride");
    let over_problem = |p: Problem| {
        at(
            p.with_path(&shown),
            key_line(&text, "templateEngineOverride"),
        )
    };
    let (engines, syntax) =
        engines_for(&ext, over, shared.template_engines).map_err(over_problem)?;
    if let Some(log) = shared.survey {
        log.lang(
            &shown,
            match engines {
                Engines::Raw => None,
                Engines::Template(l) => Some(l),
                Engines::Markdown { pre } => pre,
            },
        );
    }
    let data_engine = DataEngine::new(&ext, over, shared.template_engines).map_err(over_problem)?;
    if let Some(log) = shared.survey {
        let mut strings = Vec::new();
        for key in ["permalink", "eleventyComputed"] {
            if let Some(v) = data.get(key) {
                let line = if front.contains_key(key) {
                    key_line(&text, key)
                } else {
                    None
                };
                template_strings(key, v, line, &mut strings);
            }
        }
        log.data_strings(&shown, data_engine.lang, strings);
    }

    // `page`, in the order 11ty fills it in, after any `page` keys of the
    // page's own.
    let mut page = match data.get("page") {
        Some(Value::Object(own)) => js_object(own),
        _ => IndexMap::new(),
    };
    let string = |s: String| Value::String(s.into());
    let input_path = format!("{}{}", dot_dir(shared.input_rel), slash_path(rel));
    page.insert("inputPath".into(), string(input_path));
    page.insert("fileSlug".into(), string(file_slug(rel)));
    page.insert("filePathStem".into(), string(file_path_stem(rel)));
    page.insert("outputFileExtension".into(), string("html".into()));
    page.insert("templateSyntax".into(), string(syntax));
    page.insert("date".into(), Value::Date(date.fixed_offset()));
    page.insert("rawInput".into(), string(body.to_string()));
    let plan = computed_plan(&data, &shown, &text, data_engine)?;
    let mut values = js_object(&data);
    values.insert("page".into(), Value::Object(Arc::new(page)));
    // 11ty puts `collections` here, after the page's own data and before
    // computed values. It is filled in once every page is placed; until then
    // reading it counts as undefined.
    values.insert("collections".into(), Value::Undefined);
    let membership = Membership::of(&values);

    // The permalink renders with the page's data, before `page.url` and
    // `page.outputPath` exist. With computed data, 11ty runs it as the
    // entries that set them, in their place in the order.
    let from = |key: &str| {
        if data.contains_key(key) && !front.contains_key(key) {
            source(key)
        } else {
            None
        }
    };
    let locator = Locator {
        langs: shared.langs,
        data_engine,
        computed: plan.is_some(),
        rel,
        shown: &shown,
        output_shown: &shared.output_shown,
        from: from("permalink"),
        line: front
            .contains_key("permalink")
            .then(|| key_line(&text, "permalink"))
            .flatten(),
        subdir: None,
    };
    let source = Arc::new(Source {
        input_path: rel.to_path_buf(),
        shown: shown.clone(),
        real: feed.is_none().then(|| full.clone()),
        body: body.to_string(),
        body_line: body_line(&text, body),
        engines,
        layouts,
        withheld,
        feed: feed.map(|f| f.template.clone()),
    });
    let computed_membership = plan
        .as_ref()
        .is_some_and(|p| p.sets_root("tags") || p.sets_root("eleventyExcludeFromCollections"));
    let spec = Spec::new(&values).map_err(|p| {
        pagination_problem(p, from("pagination"), key_line(&text, "pagination"), &shown)
    })?;
    let order_files = shared.dir_data.files(rel);
    let js_data = order_files.iter().any(|f| is_js(&f.to_string_lossy()));
    let includes = shared.input_rel.join("_includes");
    let mut order_layouts: Vec<(String, OrderLayout)> = Vec::new();
    let mut name = match data_layout {
        Some(Value::String(s)) => Some(s.to_string()),
        _ => None,
    };
    for layout in &data_layouts {
        let Some(spelled) = name.take() else { break };
        let parent = match layout.data.get("layout") {
            Some(Value::String(s)) if !s.is_empty() => Some(s.to_string()),
            _ => None,
        };
        order_layouts.push((
            spelled,
            OrderLayout {
                normalized: slash_path(
                    layout
                        .shown
                        .strip_prefix(&includes)
                        .unwrap_or(&layout.shown),
                ),
                language: layout
                    .real
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default(),
                parent: parent.clone(),
            },
        ));
        name = parent;
    }
    let order_template = OrderTemplate {
        rel: rel.to_path_buf(),
        data_files: order_files.len(),
        layout: order_layouts.first().map(|(n, _)| n.clone()),
        // A virtual template never reads the disk for its date.
        dated: feed.is_some()
            || reads_no_file_date(
                data.get("date"),
                &format!("{}{}", dot_dir(shared.input_rel), slash_path(rel)),
            ),
        added_last: feed.is_some(),
    };
    let templ = |pages| {
        Ok(Loaded::Template(Box::new(Templ {
            rel: rel.to_path_buf(),
            membership: membership.clone(),
            computed_membership,
            order: order_template.clone(),
            order_layouts: order_layouts.clone(),
            js_data,
            pages,
            date_problem: date_problem.clone(),
        })))
    };
    if date_problem.is_some() {
        return templ(Pages::Ready(Vec::new()));
    }
    let Some(spec) = spec else {
        let place = Place {
            shared,
            plan: plan.as_ref(),
            locator: &locator,
            source: &source,
        };
        let mut undefined = BTreeSet::new();
        let (data, permalink) = place.locate(values, &locator, &mut undefined)?;
        place.check_undefined(&undefined)?;
        let output_rel = match permalink {
            Permalink::Write { output_rel, .. } => Some(output_rel),
            Permalink::Skip => None,
        };
        return templ(Pages::Ready(vec![Prepared {
            source: source.clone(),
            output_rel,
            data,
            page_number: None,
        }]));
    };
    let pagination_line = key_line(&text, "pagination");
    if membership.pagination.is_some()
        || spec.path.first().map(String::as_str) == Some("collections")
    {
        // Needs the collections: expanded once they are built.
        check_collection_target(
            &spec,
            &membership,
            &values,
            from("pagination"),
            pagination_line,
            &shown,
        )?;
        return templ(Pages::Deferred(Box::new(Deferred {
            source: source.clone(),
            values,
            plan,
            spec,
            rel: rel.to_path_buf(),
            shown: shown.clone(),
            data_engine,
            permalink_from: from("permalink"),
            permalink_line: locator.line,
            pagination_from: from("pagination"),
            pagination_line,
        })));
    }
    let place = Place {
        shared,
        plan: plan.as_ref(),
        locator: &locator,
        source: &source,
    };
    let pages = place.paginate(&spec, &values, None).map_err(|p| {
        if p.path.is_some() {
            p
        } else {
            pagination_problem(p, from("pagination"), pagination_line, &shown)
        }
    })?;
    templ(Pages::Ready(pages))
}

/// What 11ty refuses or Brrrty can't follow in a `pagination.data` that
/// reads collections.
fn check_collection_target(
    spec: &Spec,
    membership: &Membership,
    values: &Data,
    from: Option<String>,
    line: Option<usize>,
    shown: &Path,
) -> Result<(), Problem> {
    let key = spec.key();
    let fail = |p: Problem| pagination_problem(p, from.clone(), line, shown);
    if membership.pagination.as_deref() == Some("[keys]") {
        return Err(fail(
            Problem::error(
                codes::PAGINATION_UNSUPPORTED,
                "pagination over the whole `collections` object isn't supported yet",
            )
            .with_help("Paginate one collection, like `collections.posts`."),
        ));
    }
    if membership.pagination.is_none() {
        return Err(fail(
            Problem::error(
                codes::PAGINATION_UNSUPPORTED,
                format!("Brrrty can't tell which collection `{key}` reads"),
            )
            .with_help("Write it as `collections.name` or `collections[\"name\"]`."),
        ));
    }
    // 11ty's own check, which only knows the dotted spelling.
    for tag in collections::included_tags(values) {
        if key == format!("collections.{tag}") {
            return Err(fail(Problem::error(
                codes::PAGINATION_INVALID,
                format!(
                    "pagination circular reference: `{key}` iterates over both the `{tag}` collection and also supplies pages to that collection"
                ),
            )
            .with_help("11ty stops the whole build here.")));
        }
    }
    Ok(())
}

/// A pagination problem, placed at `pagination:` in the front matter, or
/// naming the data file or layout it comes from.
fn pagination_problem(
    mut p: Problem,
    from: Option<String>,
    line: Option<usize>,
    shown: &Path,
) -> Problem {
    match from {
        Some(from) => p.message = format!("{} (`pagination` from {from})", p.message),
        None => p = at(p, line),
    }
    p.with_path(shown)
}

/// How one template's pages are placed: their permalinks, `page.url` and
/// `page.outputPath`, and computed data.
struct Place<'a, 'b> {
    shared: &'a Shared<'b>,
    plan: Option<&'a ComputedPlan>,
    locator: &'a Locator<'a>,
    source: &'a Arc<Source>,
}

/// `data` with `page.url` and `page.outputPath` set from `found`, at the
/// end of `page`, as 11ty sets them.
fn with_location(mut data: Data, found: &Located) -> Data {
    if let Some(Value::Object(page)) = data.get_mut("page") {
        let page = Arc::make_mut(page);
        page.insert("url".into(), found.url.clone());
        page.insert("outputPath".into(), found.output_path.clone());
    }
    data
}

impl Place<'_, '_> {
    /// Locate the page with `locator` and set `page.url` and
    /// `page.outputPath`: computed data does it as its first entries, in
    /// their place in the order.
    fn locate(
        &self,
        values: Data,
        locator: &Locator<'_>,
        undefined: &mut BTreeSet<String>,
    ) -> Result<(Data, Permalink), Problem> {
        match self.plan {
            Some(plan) => {
                let applied = plan.apply(
                    self.shared.langs,
                    locator.shown,
                    &values,
                    undefined,
                    locator,
                )?;
                Ok((applied.data, applied.permalink))
            }
            None => {
                let found = locator.locate(&values, undefined)?;
                Ok((with_location(values, &found), found.permalink))
            }
        }
    }

    /// A permalink or computed value that read data Brrrty can't supply
    /// would put the page in the wrong place.
    fn check_undefined(&self, undefined: &BTreeSet<String>) -> Result<(), Problem> {
        match missing_data_problem(
            undefined,
            &self.source.withheld,
            &self.shared.data_dir,
            CollectionsState::default(),
        ) {
            Some(problem) => Err(problem.with_path(&self.source.shown)),
            None => Ok(()),
        }
    }

    /// The pages pagination makes, as `Pagination.getPageTemplates` does:
    /// every page is located first, then each gets `pagination.hrefs` and
    /// `pagination.links` and runs its computed data. An error on any page
    /// refuses them all, since every page lists every other.
    fn paginate(
        &self,
        spec: &Spec,
        values: &Data,
        expand_from: Option<&Data>,
    ) -> Result<Vec<Prepared>, Problem> {
        let root = spec.path.first().map(String::as_str).unwrap_or("");
        if self.source.withheld.iter().any(|(k, _)| k == root) {
            self.check_undefined(&BTreeSet::from([root.to_string()]))?;
        }
        // 11ty crashes on `permalink: {}` here (`templateRender has not yet
        // initialized`).
        let permalink = match values.get("permalink") {
            None | Some(Value::Null) => match values.get("eleventyComputed") {
                Some(Value::Object(c)) => c.get("permalink"),
                _ => None,
            },
            v => v,
        };
        if matches!(permalink, Some(Value::Object(o)) if o.is_empty()) {
            return Err(Problem::error(
                codes::PERMALINK_INVALID,
                "`permalink: {}` with pagination stops 11ty",
            )
            .with_path(&self.source.shown)
            .with_help(
                "11ty stops the whole build here. Remove `permalink`, or give each page its own.",
            ));
        }
        // Without a permalink to place them, pages after the first get a
        // folder named by their number.
        let has_permalink = values.get("permalink").is_some_and(Value::js_truthy)
            || matches!(values.get("eleventyComputed"), Some(Value::Object(c))
                if c.get("permalink").is_some_and(Value::js_truthy));

        let paging = spec.expand(expand_from.unwrap_or(values))?;
        let base = paging.base(values);
        let on_page = |n: usize| {
            move |mut p: Problem| {
                p.message = format!("{} (pagination page {n})", p.message);
                p.with_path(&self.source.shown)
            }
        };

        // Every page's first location, for `hrefs` and `links`.
        type First = (Located, BTreeSet<String>);
        // A page (none for `permalink: false`) and what it read undefined.
        type Generated = (Option<Prepared>, BTreeSet<String>);
        let first: Vec<Result<First, Problem>> = (0..paging.len())
            .into_par_iter()
            .map(|n| {
                let data = paging.page_data(&base, n, None).map_err(on_page(n))?;
                let locator = Locator {
                    subdir: (n > 0 && !has_permalink).then_some(n),
                    ..self.locator.clone()
                };
                let mut undefined = BTreeSet::new();
                let found = locator.locate(&data, &mut undefined).map_err(on_page(n))?;
                Ok((found, undefined))
            })
            .collect();
        let first: Vec<First> = first.into_iter().collect::<Result<_, _>>()?;
        let mut undefined = BTreeSet::new();
        for (_, u) in &first {
            undefined.extend(u.iter().cloned());
        }
        self.check_undefined(&undefined)?;
        let links = Links::new(
            first.iter().map(|(f, _)| f.url.clone()).collect(),
            first
                .iter()
                .filter_map(|(f, _)| f.link.clone())
                .map(|l| Value::String(l.into()))
                .collect(),
        );

        let pages: Vec<Result<Generated, Problem>> = first
            .into_par_iter()
            .enumerate()
            .map(|(n, (found, _))| {
                let data = paging
                    .page_data(&base, n, Some(&links))
                    .map_err(on_page(n))?;
                // 11ty sets `page.url` and `page.outputPath` from the first
                // location; computed data runs them again in its order.
                let data = with_location(data, &found);
                let mut undefined = BTreeSet::new();
                let (data, permalink) = match self.plan {
                    None => (data, found.permalink),
                    Some(_) => {
                        let locator = Locator {
                            subdir: (n > 0 && !has_permalink).then_some(n),
                            ..self.locator.clone()
                        };
                        self.locate(data, &locator, &mut undefined)
                            .map_err(on_page(n))?
                    }
                };
                let output_rel = match permalink {
                    Permalink::Write { output_rel, .. } => Some(output_rel),
                    Permalink::Skip => None,
                };
                let page = Some(Prepared {
                    source: self.source.clone(),
                    output_rel,
                    data,
                    page_number: Some(n),
                });
                Ok((page, undefined))
            })
            .collect();
        let mut out = Vec::with_capacity(pages.len());
        let mut undefined = BTreeSet::new();
        for page in pages {
            let (page, u) = page?;
            undefined.extend(u);
            out.extend(page);
        }
        self.check_undefined(&undefined)?;
        Ok(out)
    }
}

impl Deferred {
    /// Expand the template with the collections made so far.
    fn run(
        &self,
        shared: &Shared<'_>,
        so_far: &Arc<IndexMap<String, Value>>,
    ) -> Result<Vec<Prepared>, Problem> {
        let locator = Locator {
            langs: shared.langs,
            data_engine: self.data_engine,
            computed: self.plan.is_some(),
            rel: &self.rel,
            shown: &self.shown,
            output_shown: &shared.output_shown,
            from: self.permalink_from.clone(),
            line: self.permalink_line,
            subdir: None,
        };
        let place = Place {
            shared,
            plan: self.plan.as_ref(),
            locator: &locator,
            source: &self.source,
        };
        let mut target = self.values.clone();
        target.insert("collections".into(), Value::Object(so_far.clone()));
        place
            .paginate(&self.spec, &self.values, Some(&target))
            .map_err(|p| {
                if p.path.is_some() {
                    p
                } else {
                    pagination_problem(
                        p,
                        self.pagination_from.clone(),
                        self.pagination_line,
                        &self.shown,
                    )
                }
            })
    }
}

/// The page's computed data, if it has any: refused when 11ty would abort,
/// or when it sets something Brrrty can't follow yet.
fn computed_plan(
    data: &Data,
    shown: &Path,
    text: &str,
    engine: DataEngine,
) -> Result<Option<ComputedPlan>, Problem> {
    let line = key_line(text, "eleventyComputed");
    if let Some(Value::Object(computed)) = data.get("eleventyComputed") {
        let reserved = reserved_keys(computed);
        if !reserved.is_empty() {
            return Err(at(reserved_problem(&reserved).with_path(shown), line));
        }
    }
    ComputedPlan::new(data, engine).map_err(|p| at(p.with_path(shown), line))
}

/// What a page's reads of `collections` are checked against.
#[derive(Clone, Copy, Default)]
struct CollectionsState<'a> {
    /// Why the collections can't be built, when they can't.
    gap: Option<&'a str>,
    /// The site has an 11ty config file, which may add collections.
    has_config: bool,
    /// The collections come from Brrrty's own config file: it names every
    /// collection the config adds, so listing them is fine.
    rhai_config: bool,
    /// Collections left out because a page Brrrty can't date is in them,
    /// with that page's input path.
    poisoned: Option<&'a BTreeMap<String, String>>,
    /// The config's collections that failed, with why.
    broken: Option<&'a BTreeMap<String, Problem>>,
}

/// The problem for a read of a config collection that failed.
fn broken_problem(what: String, problem: &Problem) -> Problem {
    Problem::error(
        codes::DATA_UNSUPPORTED,
        format!("{what}, which failed: {}", problem.message),
    )
    .with_help("Fix the problem reported for that collection in the config file.")
}

/// The problem for a read of a collection that holds `by`, a page Brrrty
/// can't date.
fn poisoned_problem(what: String, by: &str) -> Problem {
    let by = by.strip_prefix("./").unwrap_or(by);
    Problem::error(
        codes::DATA_UNSUPPORTED,
        format!("{what}, which holds {by}, a page whose date Brrrty can't read, so it can't tell the order"),
    )
    .with_help("Fix the date problem reported for that page.")
}

/// The problem for a page that read collection data Brrrty can't give.
fn collections_problem(
    undefined: &BTreeSet<String>,
    state: CollectionsState<'_>,
) -> Option<Problem> {
    let gap = state.gap;
    use brrrty_js::traps::{ITEM_PREFIX, OPEN_PREFIX};
    let item = |k: &str| undefined.contains(&format!("{ITEM_PREFIX}{k}"));
    if item("templateContent") || item("content") {
        return Some(
            Problem::error(
                codes::CONTENT_UNAVAILABLE,
                "a permalink or computed value reads `templateContent` or `content` of a collection item, before 11ty renders any content",
            )
            .with_help(
                "11ty places every page before it renders content, and stops the whole build here (\"Tried to use templateContent too early\").",
            ),
        );
    }
    if item("template") {
        return Some(
            Problem::error(
                codes::DATA_UNSUPPORTED,
                "this page reads `template` of a collection item, 11ty's internal template object",
            )
            .with_help("Brrrty has no equivalent; read the item's `data`, `url` or `inputPath`."),
        );
    }
    if item("collections") {
        return Some(
            Problem::error(
                codes::DATA_UNSUPPORTED,
                "this page reads `data.collections` of a collection item, which Brrrty doesn't build yet",
            )
            .with_help("Read `collections` directly instead."),
        );
    }
    let empty = BTreeMap::new();
    let poisoned = state.poisoned.unwrap_or(&empty);
    let no_broken = BTreeMap::new();
    let broken = state.broken.unwrap_or(&no_broken);
    for name in undefined.iter().filter_map(|n| n.strip_prefix(OPEN_PREFIX)) {
        if name == "*" {
            if let Some((tag, by)) = poisoned.iter().next() {
                return Some(poisoned_problem(
                    format!("this page lists every collection, including `collections.{tag}`"),
                    by,
                ));
            }
            if let Some((tag, problem)) = broken.iter().next() {
                return Some(broken_problem(
                    format!("this page lists every collection, including `collections.{tag}`"),
                    problem,
                ));
            }
        } else if let Some(problem) = broken.get(name) {
            return Some(broken_problem(
                format!("this page reads `collections.{name}`"),
                problem,
            ));
        } else if let Some(by) = poisoned.get(name) {
            return Some(poisoned_problem(
                format!("this page reads `collections.{name}`"),
                by,
            ));
        }
    }
    // Without a config file, a name that isn't a collection is undefined,
    // as it is in 11ty.
    if let Some(name) = undefined
        .iter()
        .filter_map(|n| n.strip_prefix(OPEN_PREFIX))
        .filter(|n| !(state.rhai_config && *n == "*"))
        .find(|_| state.has_config)
    {
        let what = if name == "*" {
            "this page lists every collection".to_string()
        } else {
            format!("this page reads `collections.{name}`, which is not a tag collection")
        };
        let help = if state.rhai_config {
            format!(
                "If your 11ty config adds `{name}` with `addCollection`, port it to brrrty.config.rhai with `config.add_collection`."
            )
        } else {
            "Brrrty doesn't read your 11ty config; port it to brrrty.config.rhai, where `config.add_collection` adds a collection.".to_string()
        };
        return Some(
            Problem::error(
                codes::DATA_UNSUPPORTED,
                format!("{what}, and this site has an 11ty config file that may add collections"),
            )
            .with_help(help),
        );
    }
    if undefined.contains("collections") {
        return Some(match gap {
            Some(why) => Problem::error(
                codes::DATA_UNSUPPORTED,
                format!("this page uses `collections`, but {why}"),
            )
            .following("Fix the problems reported above."),
            None => Problem::error(
                codes::DATA_UNSUPPORTED,
                "a permalink, `pagination` or computed value reads `collections`, which Brrrty doesn't support there yet",
            )
            .with_help("Read `collections` in the page itself, or set the value in front matter."),
        });
    }
    None
}

/// The problem for a page that read data Brrrty doesn't supply yet.
fn missing_data_problem(
    undefined: &BTreeSet<String>,
    withheld: &[(String, Withheld)],
    data_dir: &Path,
    state: CollectionsState<'_>,
) -> Option<Problem> {
    if let Some(p) = collections_problem(undefined, state) {
        return Some(p);
    }
    let used: Vec<&(String, Withheld)> = withheld
        .iter()
        .filter(|(k, _)| undefined.contains(k.as_str()))
        .collect();
    if used.is_empty() {
        return None;
    }
    let names = |list: &[&(String, Withheld)]| {
        list.iter()
            .map(|(k, _)| format!("`{k}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut files: Vec<&PathBuf> = Vec::new();
    for (_, w) in &used {
        if let Withheld::Js(f) = w
            && !files.contains(&f)
        {
            files.push(f);
        }
    }
    if let Some(first) = files.first() {
        let js: Vec<&(String, Withheld)> = used
            .iter()
            .copied()
            .filter(|(_, w)| matches!(w, Withheld::Js(_)))
            .collect();
        let list = files
            .iter()
            .map(|f| f.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let noun = if files.len() == 1 {
            "a JavaScript data file"
        } else {
            "JavaScript data files"
        };
        return Some(
            Problem::error(
                codes::DATA_JS_UNSUPPORTED,
                format!("this page uses {} from {list}, {noun}", names(&js)),
            )
            .with_help(js_help(first)),
        );
    }
    Some(
        Problem::error(
            codes::DATA_UNSUPPORTED,
            format!(
                "this page uses {} from {}, which has a data file that failed to load",
                names(&used),
                data_dir.display()
            ),
        )
        .following("Fix the data file errors reported above."),
    )
}

/// One problem per distinct layout or data file problem, saying how many
/// pages it stopped. `noun` names what the pages use.
fn shared_problems(found: Vec<Problem>, noun: &str) -> Vec<Problem> {
    let mut counted: Vec<(Problem, usize)> = Vec::new();
    for p in found {
        match counted.iter_mut().find(|(q, _)| *q == p) {
            Some((_, n)) => *n += 1,
            None => counted.push((p, 1)),
        }
    }
    counted
        .into_iter()
        .map(|(mut p, n)| {
            let pages = if n == 1 {
                format!("1 page uses this {noun} and was not written.")
            } else {
                format!("{n} pages use this {noun} and were not written.")
            };
            p.help = Some(match p.help.take() {
                Some(h) if h.ends_with('.') => format!("{h} {pages}"),
                Some(h) => format!("{h}. {pages}"),
                None => pages,
            });
            p
        })
        .collect()
}

/// Sort by path then code, and in strict mode keep only up to the first error.
fn finish(mut report: BuildReport, strict: bool, start: Instant) -> BuildReport {
    sort_problems(&mut report, strict);
    report.elapsed = start.elapsed();
    report
}

/// [`finish`]'s order, and in strict mode only up to the first error.
fn sort_problems(report: &mut BuildReport, strict: bool) {
    // A problem another causes comes after it, wherever their paths sort.
    report.problems.sort_by(|a, b| {
        a.follows
            .cmp(&b.follows)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.code.cmp(b.code))
            .then_with(|| a.message.cmp(&b.message))
    });
    if strict
        && let Some(i) = report
            .problems
            .iter()
            .position(|p| matches!(p.severity, Severity::Error))
    {
        report.problems.truncate(i + 1);
    }
}

/// Remove `<out>/.brrrty-serve`, and nothing else: only that exact name, only directly
/// inside the output folder. A symlink by that name is removed, never followed. A folder a
/// live `brrrty serve` holds is left alone, with a note.
fn remove_serve_staging(out: &Path) -> Option<Problem> {
    let staging = out.join(crate::options::SERVE_STAGING_DIR);
    let meta = fs::symlink_metadata(&staging).ok()?;
    if meta.is_dir()
        && let Some(pid) = crate::serve_lock::serve_lock_holder(&staging)
    {
        let by = if pid.is_empty() {
            String::new()
        } else {
            format!(" (process {pid})")
        };
        return Some(
            Problem::info(
                codes::SERVE_STAGING_IN_USE,
                format!(
                    "left {} alone: a running `brrrty serve`{by} is using it",
                    staging.display()
                ),
            )
            .with_help("it is removed when that session stops; don't deploy while it runs"),
        );
    }
    let removed = if meta.is_dir() {
        fs::remove_dir_all(&staging)
    } else {
        fs::remove_file(&staging)
    };
    let e = removed.err()?;
    Some(
        Problem::warning(
            codes::SERVE_STAGING_LEFT,
            format!(
                "could not remove {} that a stopped `brrrty serve` left in the output folder: {e}",
                staging.display()
            ),
        )
        .with_help("delete it by hand so it isn't deployed with the site"),
    )
}

/// Stack for each render thread. Deeply nested Markdown and Liquid recurse
/// deeply; the default 2 MiB is not enough for 10,000 nested blockquotes.
const RENDER_STACK: usize = 16 * 1024 * 1024;

/// Build the site described by `opts`.
/// The local time zone can't be used: `TZ` names no zone in the bundled
/// tz data, or `/etc/localtime` can't be read as one.
/// The first of `LC_ALL`, `LC_MESSAGES` and `LANG` that is set (an empty value counts)
/// and doesn't make Node's default locale exactly `en-US`, by
/// [`brrrty_js::locale::default_locale_check`]: `(name, value)`. Every set one is
/// checked, not only the first: which one wins differs between platforms (on macOS,
/// `LC_ALL=C LANG=fr_FR.UTF-8` gives `und-FR`). `LC_TIME` and `LANGUAGE` don't set
/// Node's default locale.
pub fn locale_mismatch(get: impl Fn(&str) -> Option<String>) -> Option<(String, String)> {
    ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|k| {
        let v = get(k)?;
        brrrty_js::locale::default_locale_check(Some((k, &v)))
            .is_err()
            .then(|| (k.to_string(), v))
    })
}

/// How 11ty formats a date with `%x` (`toLocaleDateString()`) in the locale `value`,
/// when Node has been checked for it: 15 March 2024.
fn locale_sample(value: &str) -> Option<&'static str> {
    if value.is_empty() {
        return Some("M03 for %B, as Node reads an empty value as `und`");
    }
    let base = value.split(['.', '@']).next().unwrap_or_default();
    match base {
        "en_GB" | "en_AU" => Some("15/03/2024"),
        "en_CA" => Some("2024-03-15"),
        "en_IN" | "en_IE" => Some("15/3/2024"),
        "de_DE" => Some("15.3.2024"),
        "fr_FR" => Some("15/03/2024"),
        _ => None,
    }
}

/// What a message says about the locale `name=value`.
pub fn locale_message(name: &str, value: &str) -> String {
    let tag = value
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .replace('_', "-");
    let shown = if value.is_empty() {
        format!("{name} is set but empty, so")
    } else {
        format!("{name}={value}, so")
    };
    match locale_sample(value) {
        Some(s) if value.is_empty() => format!(
            "{shown} 11ty formats dates for the locale `und` ({s}); Brrrty uses en-US (March, 3/15/2024)"
        ),
        Some(s) => format!(
            "{shown} 11ty formats dates like %x in {tag} ({s}); Brrrty uses en-US (3/15/2024)"
        ),
        None => format!(
            "{shown} 11ty formats dates like %c, %x and %X in {tag}; Brrrty uses en-US (3/15/2024)"
        ),
    }
}

/// The warning when 11ty, run in this environment, would format dates in a locale
/// other than en-US.
fn locale_problem() -> Option<Problem> {
    let (name, value) = locale_mismatch(|k| std::env::var(k).ok())?;
    Some(
        Problem::warning(
            codes::LOCALE_DIFFERS,
            format!("In this environment, {}", locale_message(&name, &value)),
        )
        .with_help(
            "Liquid's %c, %x and %X, and toLocaleString(), format with Node's default locale, so a page that uses them differs from 11ty's run in this environment. Build with LANG=en_US.UTF-8 (and LC_ALL, LC_MESSAGES unset or the same) to compare with 11ty.",
        ),
    )
}

pub fn tz_problem(message: String) -> Problem {
    Problem::error(codes::TZ_INVALID, message).with_help(
        "set TZ to a zone name such as America/Chicago, the zone your 11ty builds run in",
    )
}

pub fn build(opts: &BuildOptions, markdown: &dyn MarkdownRenderer) -> BuildReport {
    build_with(opts, markdown, Mode::default())
}

/// Build the site described by `opts` the way `brrrty doctor` looks at it:
/// the report is what [`build`] reports, plus a [`crate::Survey`] of every
/// refusal per template, including the ones a plain build stops before
/// reaching. With `dry_run`, nothing is written: no pages, no passthrough
/// copies, and nothing is removed from the output folder.
pub fn survey(opts: &BuildOptions, markdown: &dyn MarkdownRenderer, dry_run: bool) -> BuildReport {
    build_with(
        opts,
        markdown,
        Mode {
            survey: true,
            dry_run,
        },
    )
}

fn build_with(opts: &BuildOptions, markdown: &dyn MarkdownRenderer, mode: Mode) -> BuildReport {
    let start = Instant::now();
    match rayon::ThreadPoolBuilder::new()
        .stack_size(RENDER_STACK)
        .build()
    {
        Ok(pool) => {
            let mut report = pool.install(|| build_in_pool(opts, markdown, start, mode));
            report.reads = opts.config.reads.paths();
            // Warnings the config's scripts raised while pages rendered.
            let late = opts.config.late_problems.all();
            if !late.is_empty() {
                report.problems.extend(late);
                sort_problems(&mut report, opts.strict);
            }
            report
        }
        Err(e) => {
            let report = BuildReport {
                written: 0,
                copied: 0,
                unchanged: 0,
                problems: vec![Problem::error(
                    codes::BUILD_THREADS_FAILED,
                    format!("cannot start the render threads: {e}"),
                )],
                elapsed: Duration::ZERO,
                pages: Vec::new(),
                passthrough: Vec::new(),
                complete: false,
                refused: Vec::new(),
                survey: None,
                reads: Vec::new(),
            };
            finish(report, opts.strict, start)
        }
    }
}

/// [`build_once`] until no page meets a language that changes a grammar
/// the build didn't know would change. Each pass knows of more changed
/// grammars, and there are finitely many, so this ends.
fn build_in_pool(
    opts: &BuildOptions,
    markdown: &dyn MarkdownRenderer,
    start: Instant,
    mode: Mode,
) -> BuildReport {
    let mut found = Vec::new();
    loop {
        let mut again = Vec::new();
        let report = build_once(opts, markdown, start, mode, &found, &mut again);
        if again.is_empty() {
            return report;
        }
        found.extend(again);
    }
}

/// Builds with the plugin refusing what `found` changes, besides what the
/// scan finds. When rendering meets a language that changes grammars and
/// neither knew of, nothing is written: `again` gets the languages, and
/// the build has to be redone with them.
fn build_once(
    opts: &BuildOptions,
    markdown: &dyn MarkdownRenderer,
    start: Instant,
    mode: Mode,
    found: &[crate::plugins::syntaxhighlight::scan::Found],
    again: &mut Vec<crate::plugins::syntaxhighlight::scan::Found>,
) -> BuildReport {
    let mut report = BuildReport {
        written: 0,
        copied: 0,
        unchanged: 0,
        problems: Vec::new(),
        elapsed: Duration::ZERO,
        pages: Vec::new(),
        passthrough: Vec::new(),
        complete: false,
        refused: Vec::new(),
        survey: None,
        reads: Vec::new(),
    };
    let strict = opts.strict;
    let log_owned = mode.survey.then(Log::default);
    let log = log_owned.as_ref();
    let has_error = |r: &BuildReport| r.has_errors();

    if let Err(p) = opts.validate() {
        report.problems.push(p);
        return finish(report, strict, start);
    }
    if let Err(message) = brrrty_js::clock::source_date_epoch() {
        report.problems.push(
            Problem::error(codes::SOURCE_DATE_EPOCH_INVALID, message)
                .with_help("set it to whole seconds since 1970, or unset it to use the clock"),
        );
        return finish(report, strict, start);
    }
    if let Err(message) = brrrty_js::tz::check_local() {
        report.problems.push(tz_problem(message));
        return finish(report, strict, start);
    }
    report.problems.extend(locale_problem());
    report.problems.extend(opts.config.problems.iter().cloned());
    // `brrrty init` left parts of the 11ty config unported: any page could differ from
    // 11ty's, so every page is refused once rendered (to find its other problems too).
    let unported = opts.config.unported.as_ref();
    report.problems.extend(unported.cloned());
    // A config file may add collections. Brrrty's own config can't yet, and
    // when there is one, 11ty's config is not read, so it isn't reported.
    let mut has_config = opts.config.file.is_some();
    if let Ok(root) = opts.root_dir() {
        for name in [
            "eleventy.config.js",
            "eleventy.config.cjs",
            "eleventy.config.mjs",
            ".eleventy.js",
        ] {
            if root.join(name).is_file() {
                has_config = true;
                if opts.config.file.is_some() {
                    continue;
                }
                report.problems.push(
                    Problem::warning(
                        codes::ELEVENTY_CONFIG_IGNORED,
                        "Brrrty doesn't read your 11ty config yet",
                    )
                    .with_path(name)
                    .with_help(
                        "Pass your input folder and passthrough paths as flags, like --input src --passthrough src/images.",
                    ),
                );
            }
        }
    }
    let (root, input, out) = match (opts.root_dir(), opts.input_dir(), opts.write_dir()) {
        (Ok(r), Ok(i), Ok(o)) => (r, i, o),
        (Err(p), _, _) | (_, Err(p), _) | (_, _, Err(p)) => {
            report.problems.push(p);
            return finish(report, strict, start);
        }
    };
    // A plain build owns the whole output folder, so a serve session's staging folder left
    // there (by a session that was killed) goes. A serve build writes elsewhere and leaves it.
    if opts.write_to.is_none()
        && !mode.dry_run
        && let Some(p) = remove_serve_staging(&out)
    {
        report.problems.push(p);
    }
    // Pages are tracked relative to the input folder; problems report them relative to the
    // root, through the input folder as spelled (11ty finds path dates in that spelling too).
    let input_rel = match opts.input_rel() {
        Ok(r) => r,
        Err(p) => {
            report.problems.push(p);
            return finish(report, strict, start);
        }
    };

    let discovery = match discover(opts) {
        Ok(d) => d,
        Err(p) => {
            report.problems.push(p);
            return finish(report, strict, start);
        }
    };
    report.problems.extend(discovery.warnings.iter().cloned());
    for path in &discovery.unsupported {
        let shown = input_rel.join(path);
        let p = unsupported_template(&shown);
        // A JavaScript template can make any number of pages: a survey counts it as one,
        // and says there may be more.
        if let Some(log) = log {
            log.problem(&shown, &p);
            log.paginates(&shown);
            log.stopped(&shown, &p);
            log.refused_at_least_once(&shown);
        }
        report.problems.push(p);
    }
    // 11ty skips these silently unless a plugin adds them; say so once per extension.
    let mut by_ext: std::collections::BTreeMap<String, Vec<&PathBuf>> = Default::default();
    for path in &discovery.plugin_only {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        if let Some(ext) = PLUGIN_ONLY_EXTS.iter().find(|x| name.ends_with(*x)) {
            by_ext.entry((*ext).to_string()).or_default().push(path);
        }
    }
    for (ext, paths) in by_ext {
        report.problems.push(
            Problem::warning(
                codes::TEMPLATE_PLUGIN_ONLY,
                format!(
                    "{} `{ext}` {} skipped: in 11ty these are templates only when a plugin adds them",
                    paths.len(),
                    if paths.len() == 1 { "file" } else { "files" }
                ),
            )
            .with_path(input_rel.join(paths[0])),
        );
    }
    if strict && has_error(&report) {
        return finish(report, strict, start);
    }

    let mut engine = Engine::new(EngineOptions {
        strict_filters: true,
        timezone: Tz::Local,
    });
    engine.register_eleventy_filters();
    for f in &opts.config.filters {
        engine.register_args_filter(&f.name, f.run.clone());
    }
    // The syntax highlighting plugin, refusing the grammars a language some
    // file names could change for the pages 11ty renders after it.
    let highlight = opts.config.syntax_highlight.as_ref().map(|p| {
        let mut all = crate::plugins::syntaxhighlight::scan::scan(
            &input,
            &input_rel,
            &out,
            crate::plugins::syntaxhighlight::scan::Context {
                engines: opts.config.template_engines,
                liquid_tag: p.options.has_format("liquid"),
                nunjucks_tag: p.options.has_format("njk"),
                global_data: &opts.config.global_data,
            },
        );
        all.extend(found.iter().cloned());
        p.with_mutations(&all)
    });
    // `{% highlight %}`, from the syntax highlighting plugin.
    if let Some(plugin) = &highlight
        && plugin.options.has_format("liquid")
    {
        let plugin = Arc::new(plugin.clone());
        engine.register_raw_block(
            "highlight",
            Arc::new(move |args: &str, body: &str| {
                use crate::plugins::syntaxhighlight::Error;
                use brrrty_liquid::{ErrorKind, TagError};
                plugin.liquid_tag(args, body).map_err(|e| match e {
                    Error::Unsupported(why) => TagError {
                        kind: ErrorKind::Unsupported,
                        message: format!("this `{{% highlight %}}` can't be highlighted the way 11ty highlights it: {why}"),
                        help: Some(crate::render::HIGHLIGHT_HELP.to_string()),
                    },
                    Error::Thrown(why) => TagError {
                        kind: ErrorKind::TagFailed,
                        message: format!("the syntax highlighting plugin fails on this `{{% highlight %}}`: {why}"),
                        help: Some(crate::render::HIGHLIGHT_THROWN_HELP.to_string()),
                    },
                })
            }),
        );
    }
    // 11ty runs a config's plugins after the config, so a plugin's filters
    // win over the config's own of the same name.
    let rss = opts.config.rss_filters || !opts.config.feeds.is_empty();
    if rss {
        for (name, f) in crate::plugins::rss::universal_filters() {
            engine.register_args_filter(name, f);
        }
    }
    // 11ty gives Nunjucks the same filters, and the RSS plugin's
    // Nunjucks-only ones.
    let mut nunjucks = brrrty_nunjucks::Engine::new(NunjucksOptions {
        timezone: Tz::Local,
        autoescape: true,
    });
    crate::filters::register_nunjucks_filters(&mut nunjucks, &opts.config.filters, rss);
    let includes = input.join("_includes");
    let includes = includes.canonicalize().unwrap_or(includes);
    let loader = FsLoader::new(
        includes.clone(),
        input_rel.join("_includes"),
        input.clone(),
        input_rel.clone(),
    );
    let nunjucks_loader = FsLoader::nunjucks(includes, input_rel.join("_includes"), root.clone());
    let langs = Langs {
        liquid: &engine,
        liquid_loader: &loader,
        nunjucks: &nunjucks,
        nunjucks_loader: &nunjucks_loader,
        nunjucks_highlight: highlight
            .as_ref()
            .is_some_and(|p| p.options.has_format("njk")),
    };
    let template_engines = opts.config.template_engines;
    let layouts = Layouts::new(&langs, template_engines, input.clone(), input_rel.clone());
    let data_dir = input_rel.join("_data");
    let (globals, output_shown) = globals(opts, &root, &input_rel);
    // 11ty sets the config's `addGlobalData` values, adds its `eleventy`
    // data, and merges that over `_data` (deeply); then it replaces `pkg`
    // outright with the project's package.json.
    let mut initial = match config_global(&opts.config.global_data) {
        Ok(d) => d,
        Err(p) => {
            let file = opts.config.file.clone().unwrap_or_default();
            report.problems.push(p.with_path(file));
            Data::new()
        }
    };
    let reserved = reserved_keys(&initial);
    if !reserved.is_empty() {
        let file = opts.config.file.clone().unwrap_or_default();
        report
            .problems
            .push(reserved_problem(&reserved).with_path(file));
        for key in &reserved {
            initial.shift_remove(key.split('.').next().unwrap_or(key));
        }
    }
    if let Some(v) = globals.get("eleventy") {
        initial.insert("eleventy".to_string(), v.clone());
    }
    let initial = js_object(&initial);
    let GlobalData {
        value: user_value,
        js_keys,
        notes: data_notes,
        problems: data_problems,
        unavailable,
    } = load_global(&input, &root, opts.config.data.as_ref(), &initial);
    report.problems.extend(data_notes);
    report.problems.extend(data_problems);
    let user_data = match user_value {
        Value::Object(o) => Arc::unwrap_or_clone(o),
        _ => Data::new(),
    };
    let withheld: BTreeMap<String, Withheld> = unavailable
        .into_iter()
        .map(|key| {
            let js = js_keys.iter().find(|r| r.key_path.first() == Some(&key));
            let why = match js {
                Some(r) => Withheld::Js(r.file.clone()),
                None => Withheld::Failed,
            };
            (key, why)
        })
        .collect();
    let mut global = Data::new();
    merge(&mut global, &user_data);
    merge(&mut global, &initial);
    if let Some(v) = globals.get("pkg") {
        global.insert("pkg".to_string(), v.clone());
    }
    // JavaScript templates aren't loaded: their data is JavaScript.
    let javascript = &discovery.unsupported;
    let mut loadable: Vec<PathBuf> = discovery.templates.clone();
    // The feeds' virtual templates come after every file, as 11ty adds
    // them. One that has a file's path is refused, as 11ty refuses it.
    let mut virtuals: HashMap<PathBuf, FeedOptions> = HashMap::new();
    let on_disk: HashSet<PathBuf> = loadable
        .iter()
        .chain(&discovery.unsupported)
        .cloned()
        .collect();
    for feed in &opts.config.feeds {
        let rel = PathBuf::from(&feed.input_path);
        if on_disk.contains(&rel) {
            report.problems.push(
                Problem::error(
                    codes::FEED_UNSUPPORTED,
                    format!(
                        "a feed's virtual template has the same path as a file: `{}`",
                        feed.input_path
                    ),
                )
                .with_path(input_rel.join(&rel))
                .with_help("11ty stops the whole build here (\"A virtual template had the same path as a file on the file system\"). Give the feed another `input_path`, or rename the file."),
            );
            continue;
        }
        loadable.push(rel.clone());
        virtuals.insert(rel, feed.clone());
    }
    let dir_data = DirDataIndex::load(
        &input,
        &input_rel,
        &loadable,
        opts.config.data.as_ref(),
        &initial,
    );
    report.problems.extend(dir_data.notes.iter().cloned());
    let shared = Shared {
        langs: &langs,
        template_engines,
        input: &input,
        input_rel: &input_rel,
        layouts: &layouts,
        output_shown,
        global,
        withheld,
        dir_data: &dir_data,
        data_dir: data_dir.clone(),
        virtuals: &virtuals,
        survey: log,
    };

    let loaded: Vec<Loaded> = loadable.par_iter().map(|rel| load(&shared, rel)).collect();
    let mut templates: Vec<Templ> = Vec::new();
    let mut from_layouts = Vec::new();
    let mut from_data = Vec::new();
    let mut unbuilt = 0usize;
    for (rel, item) in loadable.iter().zip(loaded) {
        match item {
            Loaded::Template(t) => templates.push(*t),
            Loaded::Failed(p) => {
                unbuilt += 1;
                report.refused.push(input_rel.join(rel));
                if let Some(log) = log {
                    log.problem(&input_rel.join(rel), &p);
                }
                report.problems.push(p);
            }
            Loaded::LayoutFailed(p) => {
                unbuilt += 1;
                report.refused.push(input_rel.join(rel));
                if let Some(log) = log {
                    log.problem(&input_rel.join(rel), &p);
                }
                from_layouts.push(p);
            }
            Loaded::DataFailed(p) => {
                unbuilt += 1;
                report.refused.push(input_rel.join(rel));
                if let Some(log) = log {
                    log.problem(&input_rel.join(rel), &p);
                }
                from_data.push(p);
            }
        }
    }
    report
        .problems
        .extend(shared_problems(from_data, "data file"));
    // Layout problems are counted once, with the pages refused when they
    // would be written.
    let mut layout_refusals = from_layouts;
    if strict && (has_error(&report) || !layout_refusals.is_empty()) {
        report
            .problems
            .extend(shared_problems(layout_refusals, "layout"));
        return finish(report, strict, start);
    }

    // Collections: every page of every template, in the order 11ty adds
    // templates to its map.
    let mut order_layouts: HashMap<String, OrderLayout> = HashMap::new();
    for t in &templates {
        for (name, layout) in &t.order_layouts {
            order_layouts
                .entry(name.clone())
                .or_insert_with(|| layout.clone());
        }
    }
    let order_inputs: Vec<OrderTemplate> = templates.iter().map(|t| t.order.clone()).collect();
    let js_data = templates.iter().filter(|t| t.js_data).count();
    let mut by_index: Vec<Option<Templ>> = templates.into_iter().map(Some).collect();
    let templates: Vec<Templ> =
        crate::order::entry_order(&order_inputs, &order_layouts, &slash_path(&input_rel))
            .into_iter()
            .filter_map(|i| by_index[i].take())
            .collect();
    let config_names: HashSet<String> = opts
        .config
        .collections
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let mut gap = collections_gap(
        unbuilt,
        javascript.len(),
        templates.iter().any(|t| t.computed_membership),
        js_data,
    )
    .or_else(|| imports_gap(&templates, &input_rel, has_config, &config_names));
    let input_dot = dot_dir(&input_rel);
    let mut slots: Vec<Slot> = Vec::new();
    let mut entries: Vec<Entry> = Vec::new();
    // Each template's collection items, in page order: a page's content is
    // what its item's `templateContent` reads.
    let mut slot_items: Vec<Vec<Value>> = Vec::new();
    for t in templates {
        let input_path = format!("{input_dot}{}", slash_path(&t.rel));
        let (pages, slot) = match t.pages {
            Pages::Ready(p) => {
                let items: Vec<Page> = p
                    .iter()
                    .map(|x| Page::new(&x.data, x.page_number))
                    .collect();
                slot_items.push(items.iter().map(|x| x.item.clone()).collect());
                (Some(items), Slot::Ready(p))
            }
            Pages::Deferred(d) => {
                slot_items.push(Vec::new());
                (None, Slot::Deferred(d))
            }
        };
        let poison = t.date_problem.is_some();
        report.problems.extend(t.date_problem);
        entries.push(Entry {
            input_path,
            membership: t.membership,
            pages,
            poison,
        });
        slots.push(slot);
    }
    let mut collections_value: Option<Arc<IndexMap<String, Value>>> = None;
    // Collections left out because a page Brrrty can't date is in them.
    let mut poisoned: BTreeMap<String, String> = BTreeMap::new();
    // The config's collections that failed.
    let mut broken: BTreeMap<String, Problem> = BTreeMap::new();
    let mut made: Vec<Option<Vec<Prepared>>> = slots.iter().map(|_| None).collect();
    // Each template's place in 11ty's order, which is file order until the
    // collections give the real one.
    let mut template_order: Vec<usize> = (0..slots.len()).collect();
    if gap.is_none() {
        let built = collections::build(entries, &opts.config.collections, |i, so_far| {
            let Slot::Deferred(d) = &slots[i] else {
                return Ok(Vec::new());
            };
            let pages = d.run(&shared, so_far)?;
            let items: Vec<Page> = pages
                .iter()
                .map(|p| Page::new(&p.data, p.page_number))
                .collect();
            slot_items[i] = items.iter().map(|x| x.item.clone()).collect();
            made[i] = Some(pages);
            Ok(items)
        });
        for (place, &i) in built.order.iter().enumerate() {
            template_order[i] = place;
        }
        let clean = built.failed.is_empty() && built.skipped.is_empty();
        // A config collection that failed is reported once, where the
        // config says.
        report.problems.extend(built.broken.values().cloned());
        for (i, name) in built.refused {
            if let Slot::Deferred(d) = &slots[i] {
                let what = format!("this page paginates `collections.{name}`");
                let problem = match built.broken.get(&name) {
                    Some(p) => broken_problem(what, p),
                    None => {
                        let by = built.poisoned.get(&name).map_or("", |s| s.as_str());
                        poisoned_problem(what, by)
                    }
                };
                report.problems.push(pagination_problem(
                    problem,
                    d.pagination_from.clone(),
                    d.pagination_line,
                    &d.shown,
                ));
            }
        }
        poisoned = built.poisoned;
        broken = built.broken;
        for (i, problem) in built.failed {
            if let (Some(log), Slot::Deferred(d)) = (log, &slots[i]) {
                log.problem(&d.shown, &problem);
            }
            match blocking_layout(&slots[i]) {
                Some(layout) => {
                    note_layout(log, &slots[i], &layout);
                    layout_refusals.push(layout)
                }
                None => report.problems.push(problem),
            }
        }
        for i in built.skipped {
            if let Some(layout) = blocking_layout(&slots[i]) {
                note_layout(log, &slots[i], &layout);
                layout_refusals.push(layout);
            } else if let Slot::Deferred(d) = &slots[i] {
                report.problems.push(
                    Problem::error(
                        codes::PAGINATION_UNSUPPORTED,
                        "this page paginates a collection, but another template that paginates a collection failed first",
                    )
                    .with_path(&d.shown)
                    .following("Fix the problem reported above."),
                );
            }
        }
        if clean {
            collections_value = Some(built.collections);
        } else {
            gap = Some("a template that paginates a collection failed, so the collections would be missing its pages".to_string());
        }
    } else if let Some(why) = &gap {
        for slot in &slots {
            if let Some(layout) = blocking_layout(slot) {
                note_layout(log, slot, &layout);
                layout_refusals.push(layout);
            } else if let Slot::Deferred(d) = slot {
                report.problems.push(
                    Problem::error(
                        codes::PAGINATION_UNSUPPORTED,
                        format!("this page paginates a collection, but {why}"),
                    )
                    .with_path(&d.shown)
                    .following("Fix the problems reported above."),
                );
            }
        }
    }
    let mut prepared: Vec<Prepared> = Vec::new();
    // For each page: its template's place in 11ty's order, and its item.
    let mut page_template: Vec<usize> = Vec::new();
    let mut page_item: Vec<Option<usize>> = Vec::new();
    for (i, (slot, made)) in slots.into_iter().zip(made).enumerate() {
        let pages = match slot {
            Slot::Ready(p) => p,
            Slot::Deferred(_) => made.unwrap_or_default(),
        };
        for (n, _) in pages.iter().enumerate() {
            page_template.push(template_order[i]);
            // Registered even when the collections are refused: a page that
            // paginated one before that still holds its items.
            let item = match slot_items[i].get(n) {
                Some(Value::Object(o)) => Some(traps::object_id(o)),
                _ => None,
            };
            page_item.push(item);
        }
        prepared.extend(pages);
    }
    // The registered `collections` object makes a read of a name that isn't
    // a tag a trap when the site's config may add collections.
    let _open = collections_value
        .as_ref()
        .filter(|_| has_config || !poisoned.is_empty() || !broken.is_empty())
        .map(|c| OpenGuard::new(brrrty_js::traps::object_id(c)));
    for p in &mut prepared {
        match &collections_value {
            Some(c) => {
                p.data
                    .insert("collections".to_string(), Value::Object(c.clone()));
            }
            None => {
                p.data.shift_remove("collections");
            }
        }
    }
    if strict && (has_error(&report) || !layout_refusals.is_empty()) {
        report
            .problems
            .extend(shared_problems(layout_refusals, "layout"));
        return finish(report, strict, start);
    }

    // Detect outputs that two or more pages claim.
    let mut by_output: BTreeMap<&Path, Vec<&Prepared>> = BTreeMap::new();
    for p in &prepared {
        if let Some(output) = &p.output_rel {
            by_output.entry(output.as_path()).or_default().push(p);
        }
    }
    let mut colliding: BTreeSet<PathBuf> = BTreeSet::new();
    for (output, pages) in &by_output {
        if pages.len() > 1 {
            let list: Vec<String> = pages.iter().map(|p| p.label()).collect();
            report.problems.push(
                Problem::error(
                    codes::OUTPUT_COLLISION,
                    format!(
                        "{} pages write to {}: {}",
                        pages.len(),
                        output.display(),
                        list.join(", ")
                    ),
                )
                .with_path(&pages[0].source.shown)
                .with_help("give each page a unique permalink"),
            );
            colliding.insert(output.to_path_buf());
        }
    }
    // A page that shares its output with another is not written, but its
    // content is still rendered for the pages that read it.
    let writes = |p: &Prepared| {
        p.output_rel
            .as_ref()
            .is_some_and(|o| !colliding.contains(o))
    };
    // A page whose layout Brrrty can't render yet is refused when it would
    // be written; its content still serves the pages that read it.
    let unsupported_layout = |p: &Prepared| {
        p.source
            .layouts
            .iter()
            .find_map(|l| l.unrenderable())
            .cloned()
    };
    if strict && has_error(&report) {
        return finish(report, strict, start);
    }

    // Render every page's content in 11ty's dependency order, then the
    // layouts, which can read any page's content.
    let pipeline = Pipeline {
        langs: &langs,
        markdown,
        highlight: highlight.as_ref().filter(|p| p.options.has_format("md")),
        rules: opts.config.markdown_rules.as_ref().map(|r| r.0.as_ref()),
    };
    let jobs: Vec<ContentPage<'_>> = prepared
        .iter()
        .enumerate()
        .map(|(i, p)| ContentPage {
            job: Job {
                shown: &p.source.shown,
                real: p.source.real.as_deref(),
                body: &p.source.body,
                body_line: p.source.body_line,
                engines: p.source.engines,
                data: &p.data,
                layouts: &p.source.layouts,
                feed: p.source.feed.as_deref(),
            },
            template: page_template[i],
            page_number: p.page_number,
            item: page_item[i],
            renderable: content::renderable(&p.data),
        })
        .collect();
    let state = CollectionsState {
        gap: gap.as_deref(),
        has_config,
        rhai_config: opts.config.file.is_some(),
        poisoned: Some(&poisoned),
        broken: Some(&broken),
    };
    let check = |i: usize, undefined: &BTreeSet<String>| {
        let src = &prepared[i].source;
        missing_data_problem(undefined, &src.withheld, &data_dir, state)
            .map(|p| p.with_path(&src.shown))
    };
    let contents = content::render_all(&pipeline, &jobs, &check);
    let settled = contents.settled_source();

    // Each result names the page whose problem explains it, if another; a
    // layout Brrrty can't render is reported once for all its pages.
    enum Refused {
        Page(Problem, Option<usize>),
        Layout(Problem),
    }
    impl From<(Problem, Option<usize>)> for Refused {
        fn from((p, other): (Problem, Option<usize>)) -> Refused {
            Refused::Page(p, other)
        }
    }
    type Written = Result<(usize, u64), Refused>;
    // Every page is rendered before any is written: highlighting may only
    // now have met a language that changes grammars, which the scan didn't
    // see, and then the build is redone instead.
    type Ready<'p> = Result<(usize, PathBuf, String, &'p Path), Refused>;
    // A survey records a page refused for another page's missing content as needing that
    // page, not as a blocker of its own.
    let note = |log: &Log, shown: &Path, p: &Problem, other: Option<usize>| match other {
        Some(j) => log.needs(shown, p, &prepared[j].source.shown),
        None => log.problem(shown, p),
    };
    let ready: Vec<Ready<'_>> = (0..prepared.len())
        .into_par_iter()
        .filter(|&i| writes(&prepared[i]))
        .map(|i| -> Ready<'_> {
            let p = &prepared[i];
            if let Some(problem) = unsupported_layout(p) {
                // A survey also records what the page's content would have been refused for.
                if let Some(log) = log {
                    log.problem(&p.source.shown, &problem);
                    if let Err((hidden, other)) = contents.content(i) {
                        note(log, &p.source.shown, &hidden, other);
                    }
                }
                return Err(Refused::Layout(problem));
            }
            let src = &p.source;
            let shown = &src.shown;
            let output_rel = p.output_rel.as_deref().unwrap_or(Path::new(""));
            let Some(dest) = contained_output_path(&out, output_rel) else {
                return Err(Refused::Page(
                    Problem::error(
                        codes::OUTPUT_ESCAPES,
                        format!(
                            "{} would be written outside the output folder",
                            output_rel.display()
                        ),
                    )
                    .with_path(shown),
                    None,
                ));
            };
            let content = contents.content(i).inspect_err(|(p, other)| {
                if let Some(log) = log {
                    note(log, shown, p, *other);
                }
            })?;
            let guard = traps::install(settled.clone());
            let rendered = pipeline.layouts(&jobs[i].job, content);
            let reads = traps::take_item_reads();
            drop(guard);
            if let Some(j) = reads
                .iter()
                .find(|(_, o)| *o == Outcome::Refused)
                .and_then(|(id, _)| contents.page_of(*id))
            {
                let problem = contents.needs_problem(i, j);
                if let Some(log) = log {
                    note(log, shown, &problem, Some(j));
                }
                return Err(Refused::Page(problem, Some(j)));
            }
            let rendered = rendered.map_err(|p| {
                if let Some(log) = log {
                    log.problem(shown, &p);
                }
                (p, None)
            })?;
            if let Some(problem) =
                missing_data_problem(&rendered.undefined, &src.withheld, &data_dir, state)
            {
                let problem = problem.with_path(shown);
                if let Some(log) = log {
                    log.problem(shown, &problem);
                }
                return Err(Refused::Page(problem, None));
            }
            if let Some(problem) = unported {
                if let Some(log) = log {
                    log.problem(shown, problem);
                }
                return Err(Refused::Page(problem.clone(), None));
            }
            Ok((i, dest, rendered.html, shown.as_path()))
        })
        .collect();
    if let Some(plugin) = &highlight {
        let unforeseen = plugin.unforeseen();
        if !unforeseen.is_empty() {
            *again = unforeseen;
            return report;
        }
    }
    // Written on the writer threads: see `io_threads`.
    let results: Vec<Written> = crate::io_threads::install(|| {
        ready
            .into_par_iter()
            .map(|r| -> Written {
                let (i, dest, html, shown) = r?;
                if mode.dry_run {
                    return Ok((i, content_hash(html.as_bytes())));
                }
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(|e| {
                        (
                            Problem::error(
                                codes::WRITE_FAILED,
                                format!("cannot create folder: {e}"),
                            )
                            .with_path(shown),
                            None,
                        )
                    })?;
                }
                let hash = content_hash(html.as_bytes());
                fs::write(&dest, html).map_err(|e| {
                    (
                        Problem::error(codes::WRITE_FAILED, format!("cannot write output: {e}"))
                            .with_path(shown),
                        None,
                    )
                })?;
                Ok((i, hash))
            })
            .collect()
    });
    // The pages of one template often fail the same way: say it once. A page
    // that isn't written reports its own problem only when another page's
    // problem points to it.
    let problem_key = |p: &Problem| {
        format!(
            "{}|{:?}|{:?}|{:?}|{}",
            p.code, p.path, p.line, p.column, p.message
        )
    };
    // The unported config's error is in the report already.
    let mut seen: BTreeSet<String> = unported.map(problem_key).into_iter().collect();
    let mut report_problem = |report: &mut BuildReport, p: Problem| {
        if seen.insert(problem_key(&p)) {
            report.problems.push(p);
        }
    };
    let mut pointed: Vec<usize> = Vec::new();
    let mut from_layouts = layout_refusals;
    let mut written_pages: HashSet<usize> = HashSet::new();
    for r in results {
        match r {
            Ok((i, hash)) => {
                written_pages.insert(i);
                report.written += 1;
                let p = &prepared[i];
                let url = match p.data.get("page") {
                    Some(Value::Object(page)) => match page.get("url") {
                        Some(Value::String(u)) => u.to_string(),
                        _ => String::new(),
                    },
                    _ => String::new(),
                };
                report.pages.push(WrittenPage {
                    source: p.source.shown.clone(),
                    url,
                    output: p.output_rel.clone().unwrap_or_default(),
                    hash,
                });
            }
            Err(Refused::Page(p, other)) => {
                report_problem(&mut report, p);
                pointed.extend(other);
            }
            Err(Refused::Layout(p)) => from_layouts.push(p),
        }
    }
    report
        .problems
        .extend(shared_problems(from_layouts, "layout"));
    let mut explained: BTreeSet<usize> = BTreeSet::new();
    while let Some(j) = pointed.pop() {
        if writes(&prepared[j]) || !contents.renders(j) || !explained.insert(j) {
            continue;
        }
        if let Err((p, other)) = contents.content(j) {
            report_problem(&mut report, p);
            pointed.extend(other);
        }
    }
    if strict && has_error(&report) {
        return finish(report, strict, start);
    }

    // Every page with somewhere to go that didn't get there was refused.
    let mut refused: BTreeSet<&Path> = prepared
        .iter()
        .enumerate()
        .filter(|(i, p)| p.output_rel.is_some() && !written_pages.contains(i))
        .map(|(_, p)| p.source.shown.as_path())
        .collect();
    refused.extend(report.refused.iter().map(PathBuf::as_path));
    let refused: Vec<PathBuf> = refused.into_iter().map(Path::to_path_buf).collect();
    report.refused = refused;
    if let Some(log) = log {
        for (i, p) in prepared.iter().enumerate() {
            if p.output_rel.is_none() {
                continue;
            }
            if written_pages.contains(&i) {
                log.written(&p.source.shown);
            } else {
                log.refused(&p.source.shown);
            }
        }
        for t in &report.refused {
            log.refused_at_least_once(t);
        }
        // Problems the build reports at a template's own path refused it too.
        for p in &report.problems {
            if let Some(path) = &p.path
                && matches!(p.severity, Severity::Error)
                && report.refused.contains(path)
            {
                log.problem(path, p);
            }
        }
    }

    let pages: HashMap<PathBuf, PathBuf> = prepared
        .iter()
        .filter(|p| writes(p) && unsupported_layout(p).is_none())
        .filter_map(|p| {
            p.output_rel
                .as_ref()
                .map(|o| (out.join(o), input_rel.join(&p.source.input_path)))
        })
        .collect();
    // With the config unported, a passthrough copy may be a file 11ty processes: copy none.
    let pt = match unported {
        Some(_) => crate::passthrough::PassthroughResult {
            copied: 0,
            unchanged: 0,
            overridden: HashSet::new(),
            files: Vec::new(),
            problems: Vec::new(),
        },
        None => copy_passthrough_avoiding(opts, &pages, mode.dry_run),
    };
    // A passthrough file that replaced a page means that page was not really written.
    report.written = report.written.saturating_sub(pt.overridden.len());
    report
        .pages
        .retain(|p| !pt.overridden.contains(&out.join(&p.output)));
    report.passthrough = pt.files;
    report.copied = pt.copied;
    report.unchanged = pt.unchanged;
    report.problems.extend(pt.problems);
    report.complete = true;
    let js_keys: BTreeMap<String, PathBuf> = shared
        .withheld
        .iter()
        .filter_map(|(k, w)| match w {
            Withheld::Js(f) => Some((k.clone(), f.clone())),
            Withheld::Failed => None,
        })
        .collect();
    let mut report = finish(report, strict, start);
    if let Some(log) = log_owned {
        let mut templates = log.finish();
        // Every refused page waits on the unported config, whatever else refused it.
        if let Some(u) = unported {
            for t in templates.values_mut() {
                if t.refused > 0 && !t.problems.contains(u) {
                    t.problems.push(u.clone());
                }
            }
        }
        crate::scan::Scanner::new(&langs, template_engines, &root, &js_keys)
            .add_findings(&mut templates);
        report.survey = Some(crate::survey::Survey { templates, js_keys });
    }
    report
}

/// The problem for a template in a language Brrrty doesn't render yet:
/// JavaScript.
fn unsupported_template(shown: &Path) -> Problem {
    Problem::error(
        codes::TEMPLATE_UNSUPPORTED,
        "this template language is not supported yet",
    )
    .with_path(shown)
    .with_help(
        "JavaScript templates are coming soon. Liquid, Nunjucks, Markdown and HTML templates work now.",
    )
}

/// The problem of a layout Brrrty can't render, when a template that
/// paginates a collection uses one: it is refused for that layout.
fn blocking_layout(slot: &Slot) -> Option<Problem> {
    match slot {
        Slot::Deferred(d) => d
            .source
            .layouts
            .iter()
            .find_map(|l| l.unrenderable())
            .cloned(),
        Slot::Ready(_) => None,
    }
}

/// Record in a survey that a layout refused the template in `slot`.
fn note_layout(log: Option<&Log>, slot: &Slot, layout: &Problem) {
    if let (Some(log), Slot::Deferred(d)) = (log, slot) {
        log.problem(&d.shown, layout);
    }
}

/// Which template page a slot holds while the collections are built.
enum Slot {
    Ready(Vec<Prepared>),
    Deferred(Box<Deferred>),
}

/// Why the collections can't be built when a template's
/// `eleventyImport.collections` can't be followed: 11ty names an entry that
/// is a date in the local time zone, and an entry that names a collection
/// the 11ty config may add moves the template to a later stage of 11ty's
/// graph.
fn imports_gap(
    templates: &[Templ],
    input_rel: &Path,
    has_config: bool,
    config_names: &HashSet<String>,
) -> Option<String> {
    let shown = |t: &Templ| input_rel.join(&t.rel).display().to_string();
    if let Some(t) = templates.iter().find(|t| t.membership.unknown_import) {
        return Some(format!(
            "{} lists a date in `eleventyImport.collections`, which 11ty names in the local time zone",
            shown(t)
        ));
    }
    if !has_config {
        return None;
    }
    let tags: HashSet<&str> = templates
        .iter()
        .flat_map(|t| t.membership.tags.iter().map(String::as_str))
        .collect();
    // Only a string names a config collection (11ty looks it up in a Set).
    let known = |name: &str, string: bool| {
        let stage = ["[basic]", "[userconfig]", "[keys]"].contains(&name);
        name == "all" || tags.contains(name) || (string && (stage || config_names.contains(name)))
    };
    let imports = templates.iter().find_map(|t| {
        let (name, _) = t
            .membership
            .imports
            .iter()
            .find(|(name, string)| !known(name, *string))?;
        Some(format!(
            "{} imports `{name}` with `eleventyImport.collections`, which is not a tag collection and may be one your 11ty config adds",
            shown(t)
        ))
    });
    // A page that paginates a name no page tags and the config doesn't
    // add would paginate an empty collection; 11ty's config may add it.
    // A path into a config collection (`meta.posts.tags`) reads that one.
    imports.or_else(|| {
        templates.iter().find_map(|t| {
            let name = t.membership.pagination.as_deref()?;
            let first = name.split('.').next().unwrap_or(name);
            if name == "[keys]" || known(name, true) || config_names.contains(first) {
                return None;
            }
            Some(format!(
                "{} paginates `collections.{name}`, which is not a tag collection and may be one your 11ty config adds",
                shown(t)
            ))
        })
    })
}

/// Why the collections can't be built, when they can't. A collection built
/// without a page would be wrong, so every read of `collections` is refused.
fn collections_gap(
    unbuilt: usize,
    javascript: usize,
    computed: bool,
    js_data: usize,
) -> Option<String> {
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            one.to_string()
        } else {
            many.to_string()
        }
    };
    if unbuilt > 0 {
        return Some(format!(
            "{unbuilt} {} could not be built, so the collections would be missing {}",
            plural(unbuilt, "template", "templates"),
            plural(unbuilt, "its page", "their pages")
        ));
    }
    if javascript > 0 {
        return Some(format!(
            "{javascript} JavaScript {}, whose data Brrrty can't read yet, so the collections would be missing {}",
            plural(
                javascript,
                "template (.11ty.js) is in the site",
                "templates (.11ty.js) are in the site"
            ),
            plural(javascript, "its pages", "their pages")
        ));
    }
    if js_data > 0 {
        return Some(format!(
            "{js_data} {} a JavaScript data file, which 11ty loads from the disk while it decides the order of templates, so Brrrty can't tell the order of the collections",
            plural(js_data, "template uses", "templates use")
        ));
    }
    computed.then(|| {
        "computed data sets `tags` or `eleventyExcludeFromCollections`, which Brrrty can't follow into the collections yet".to_string()
    })
}

/// Unregisters the open `collections` object when the build ends.
struct OpenGuard(usize);

impl OpenGuard {
    fn new(id: usize) -> OpenGuard {
        brrrty_js::traps::open(id);
        OpenGuard(id)
    }
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        brrrty_js::traps::close(self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// From `Template.getMappedDate`: which pages read their date from disk.
    #[test]
    fn the_locale_must_be_en_us_in_every_variable_node_may_take() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let m = |p| locale_mismatch(env(p)).map(|(n, v)| format!("{n}={v}"));
        assert_eq!(m(&[]), None);
        assert_eq!(m(&[("LANG", "en_US.UTF-8")]), None);
        assert_eq!(m(&[("LANG", "C.UTF-8")]), None);
        assert_eq!(m(&[("LC_ALL", "POSIX"), ("LANG", "C")]), None);
        assert_eq!(
            m(&[("LANG", "en_GB.UTF-8")]).as_deref(),
            Some("LANG=en_GB.UTF-8")
        );
        assert_eq!(
            m(&[("LANG", "en_CA.UTF-8")]).as_deref(),
            Some("LANG=en_CA.UTF-8")
        );
        assert_eq!(m(&[("LANG", "")]).as_deref(), Some("LANG="));
        assert_eq!(
            m(&[("LC_ALL", ""), ("LANG", "en_US.UTF-8")]).as_deref(),
            Some("LC_ALL=")
        );
        assert_eq!(
            m(&[("LC_ALL", "C"), ("LANG", "fr_FR.UTF-8")]).as_deref(),
            Some("LANG=fr_FR.UTF-8")
        );
        assert_eq!(
            m(&[("LC_MESSAGES", "de_DE.UTF-8"), ("LANG", "en_US.UTF-8")]).as_deref(),
            Some("LC_MESSAGES=de_DE.UTF-8")
        );
        // LC_TIME and LANGUAGE don't set Node's default locale.
        assert_eq!(m(&[("LC_TIME", "fr_FR.UTF-8"), ("LANGUAGE", "fr")]), None);
        assert_eq!(
            locale_message("LANG", "en_GB.UTF-8"),
            "LANG=en_GB.UTF-8, so 11ty formats dates like %x in en-GB (15/03/2024); Brrrty uses en-US (3/15/2024)"
        );
        assert!(locale_message("LANG", "").contains("`und` (M03"));
    }

    #[test]
    fn which_pages_read_their_date_from_disk() {
        let s = |t: &str| Value::String(t.into());
        let plain = "./src/a.md";
        let dated_path = "./src/posts/2020-03-04-a.md";
        assert!(reads_no_file_date(Some(&s("2020-01-01")), plain));
        assert!(reads_no_file_date(None, dated_path));
        assert!(reads_no_file_date(None, "./2021-05-06/c.html"));
        assert!(reads_no_file_date(Some(&s("")), dated_path));
        assert!(reads_no_file_date(Some(&Value::Bool(false)), dated_path));
        assert!(!reads_no_file_date(None, plain));
        assert!(!reads_no_file_date(None, "./src/2020-3-04-a.md"));
        assert!(!reads_no_file_date(Some(&s("")), plain));
        for special in [
            "Last Modified",
            "created",
            "git Last Modified",
            "GIT CREATED",
        ] {
            assert!(
                !reads_no_file_date(Some(&s(special)), dated_path),
                "{special}"
            );
        }
    }

    #[test]
    fn contained_output_path_accepts_plain_relative_paths() {
        let out = Path::new("/site/_site");
        assert_eq!(
            contained_output_path(out, Path::new("a/b/index.html")),
            Some(PathBuf::from("/site/_site/a/b/index.html"))
        );
    }

    #[test]
    fn contained_output_path_refuses_escapes() {
        let out = Path::new("/site/_site");
        for bad in ["../x.html", "a/../../x.html", "/etc/passwd", "", "."] {
            assert_eq!(contained_output_path(out, Path::new(bad)), None, "{bad}");
        }
    }
}
