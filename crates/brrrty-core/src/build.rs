//! The build pipeline: discover, parse, resolve, render, write, copy.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use brrrty_js::Value;
use brrrty_liquid::{Engine, EngineOptions, Tz};
use brrrty_markdown::MarkdownRenderer;
use indexmap::IndexMap;
use rayon::prelude::*;
use serde_json::Value as Json;

use crate::collections::{self, Entry, Membership, Page};
use crate::content::{self, ContentPage};
use crate::data::{
    BUILD_KEYS, ComputedPlan, DirDataIndex, GlobalData, Local, OVERRIDE_PREFIX, clean_tags,
    frozen_keys, frozen_problem, js_help, js_object, key_line, load_global, merge, page_data,
    reserved_keys, reserved_problem,
};
use crate::discover::{PLUGIN_ONLY_EXTS, discover};
use crate::frontmatter::{Data, split_front_matter};
use crate::layout::{ChainError, LayoutFile, Layouts};
use crate::loader::FsLoader;
use crate::options::BuildOptions;
use crate::order::{OrderLayout, OrderTemplate};
use crate::page::{file_path_stem, file_slug, resolve_date};
use crate::paginate::{Links, Spec};
use crate::passthrough::copy_passthrough_avoiding;
use crate::permalink::{Located, Locator, Permalink};
use crate::problem::{Problem, Severity, codes};
use crate::render::{Engines, Job, Pipeline, body_line, engines_for};
use brrrty_js::traps::{self, Outcome};

/// What a build did.
#[derive(Debug)]
pub struct BuildReport {
    pub written: usize,
    pub copied: usize,
    pub unchanged: usize,
    pub problems: Vec<Problem>,
    pub elapsed: Duration,
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
    body: String,
    body_line: usize,
    engines: Engines,
    layouts: Vec<Arc<LayoutFile>>,
    /// Keys left out of the data because Brrrty can't supply their value.
    withheld: Vec<(String, Withheld)>,
    /// A template in a language Brrrty doesn't render yet (Nunjucks): its
    /// pages are collection items, with this problem as their content, and
    /// are never written.
    unsupported: Option<Problem>,
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
    /// A template Brrrty doesn't render (reported on its own) whose place in
    /// the collections it can't work out either, and why.
    Unplaced(Problem),
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
    raw: bool,
    /// Where `permalink` and `pagination` come from, when not the page.
    permalink_from: Option<String>,
    permalink_line: Option<usize>,
    pagination_from: Option<String>,
    pagination_line: Option<usize>,
}

/// What every page shares while loading.
struct Shared<'a> {
    engine: &'a Engine,
    loader: &'a FsLoader,
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
                .with_help("Fix the data file errors reported above."),
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
            "runMode": "build",
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
    let loaded = match load_page(shared, rel) {
        Ok(l) => l,
        Err(p) => Loaded::Failed(p),
    };
    if !is_nunjucks(rel) {
        return loaded;
    }
    // A Nunjucks template is already reported as unsupported; anything else
    // wrong with it only keeps it out of the collections.
    match loaded {
        Loaded::Failed(p) | Loaded::LayoutFailed(p) | Loaded::DataFailed(p) => Loaded::Unplaced(p),
        other => other,
    }
}

fn is_nunjucks(rel: &Path) -> bool {
    rel.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("njk"))
}

/// Nunjucks compiles a string that has `{%`, `{{` or `{#` in it
/// (`Nunjucks.needsCompilation`); anything else it leaves as written.
fn has_nunjucks(v: &Value) -> bool {
    match v {
        Value::String(s) => s.contains("{%") || s.contains("{{") || s.contains("{#"),
        Value::Array(a) => a.iter().any(has_nunjucks),
        Value::Object(o) => o.values().any(has_nunjucks),
        _ => false,
    }
}

/// Why Brrrty can't place a Nunjucks template without running Nunjucks, if
/// it can't: its permalink or computed data is a Nunjucks template, or it
/// overrides its template language.
fn nunjucks_unplaced(data: &Data) -> Option<&'static str> {
    if data.contains_key("templateEngineOverride") {
        return Some("it sets `templateEngineOverride`");
    }
    let dynamic = !matches!(data.get("dynamicPermalink"), Some(Value::Bool(false)));
    if dynamic && data.get("permalink").is_some_and(has_nunjucks) {
        return Some("its `permalink` is a Nunjucks template");
    }
    if data.get("eleventyComputed").is_some_and(has_nunjucks) {
        return Some("its `eleventyComputed` values are Nunjucks templates");
    }
    None
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
    match (loaded, blocked) {
        (Err(_), Some(layout)) => Ok(Loaded::LayoutFailed(layout)),
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
    let bytes = fs::read(&full).map_err(|e| {
        Problem::error(codes::TEMPLATE_UNREADABLE, format!("cannot read file: {e}"))
            .with_path(&shown)
    })?;
    let text = String::from_utf8(bytes).map_err(|_| {
        Problem::error(codes::TEMPLATE_NOT_UTF8, "file is not valid UTF-8")
            .with_path(&shown)
            .with_help("save the file as UTF-8")
    })?;
    let (mut front, body) = split_front_matter(&text).map_err(|p| p.with_path(&shown))?;

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

    let layouts = match data.get("layout") {
        v if v == data_layout => data_layouts.clone(),
        v => match layout_chain(shared, v, &shown, &text)? {
            Ok(chain) => chain,
            Err(loaded) => return Ok(loaded),
        },
    };
    *blocked = layouts.iter().find_map(|l| l.unrenderable()).cloned();

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
    let meta = fs::metadata(&full).ok();
    let modified = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let created = meta
        .as_ref()
        .and_then(|m| m.created().ok())
        .unwrap_or(modified);
    let date = resolve_date(&data, &shown, created, modified).map_err(|p| p.with_path(&shown))?;
    let ext = rel
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let nunjucks = ext == "njk";
    let (engines, syntax) = if nunjucks {
        // Placed without rendering: everything that would run Nunjucks to
        // place the page keeps it out of the collections instead.
        if let Some(why) = nunjucks_unplaced(&data) {
            return Err(Problem::error(codes::TEMPLATE_UNSUPPORTED, why).with_path(&shown));
        }
        (Engines::Raw, "njk".to_string())
    } else {
        engines_for(&ext, data.get("templateEngineOverride")).map_err(|p| {
            at(
                p.with_path(&shown),
                key_line(&text, "templateEngineOverride"),
            )
        })?
    };

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
    let plan = computed_plan(&data, &shown, &text, engines == Engines::Raw)?;
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
        engine: shared.engine,
        loader: shared.loader,
        rel,
        shown: &shown,
        output_shown: &shared.output_shown,
        raw: engines == Engines::Raw,
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
        body: body.to_string(),
        body_line: body_line(&text, body),
        engines,
        layouts,
        withheld,
        unsupported: nunjucks.then(|| unsupported_template(&shown)),
    });
    let computed_membership = plan
        .as_ref()
        .is_some_and(|p| p.sets_root("tags") || p.sets_root("eleventyExcludeFromCollections"));
    let spec = Spec::new(&values).map_err(|p| {
        pagination_problem(p, from("pagination"), key_line(&text, "pagination"), &shown)
    })?;
    let order_files = shared.dir_data.files(rel);
    let js_data = order_files.iter().any(|f| {
        let name = f.to_string_lossy();
        [".js", ".cjs", ".mjs"].iter().any(|e| name.ends_with(e))
    });
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
        dated: reads_no_file_date(
            data.get("date"),
            &format!("{}{}", dot_dir(shared.input_rel), slash_path(rel)),
        ),
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
        })))
    };
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
    if membership.consumes.is_some() || spec.path.first().map(String::as_str) == Some("collections")
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
            raw: engines == Engines::Raw,
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
    if membership.consumes.as_deref() == Some("[keys]") {
        return Err(fail(
            Problem::error(
                codes::PAGINATION_UNSUPPORTED,
                "pagination over the whole `collections` object isn't supported yet",
            )
            .with_help("Paginate one collection, like `collections.posts`."),
        ));
    }
    if membership.consumes.is_none() {
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
                    self.shared.engine,
                    self.shared.loader,
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
            None,
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
            engine: shared.engine,
            loader: shared.loader,
            rel: &self.rel,
            shown: &self.shown,
            output_shown: &shared.output_shown,
            raw: self.raw,
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
    raw: bool,
) -> Result<Option<ComputedPlan>, Problem> {
    let line = key_line(text, "eleventyComputed");
    if let Some(Value::Object(computed)) = data.get("eleventyComputed") {
        let reserved = reserved_keys(computed);
        if !reserved.is_empty() {
            return Err(at(reserved_problem(&reserved).with_path(shown), line));
        }
    }
    ComputedPlan::new(data, raw).map_err(|p| at(p.with_path(shown), line))
}

/// The problem for a page that read collection data Brrrty can't give.
/// `gap` is why the collections can't be built, when they can't.
fn collections_problem(undefined: &BTreeSet<String>, gap: Option<&str>) -> Option<Problem> {
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
    if let Some(name) = undefined.iter().find_map(|n| n.strip_prefix(OPEN_PREFIX)) {
        let what = if name == "*" {
            "this page lists every collection".to_string()
        } else {
            format!("this page reads `collections.{name}`, which is not a tag collection")
        };
        return Some(
            Problem::error(
                codes::DATA_UNSUPPORTED,
                format!("{what}, and this site has an 11ty config file that may add collections"),
            )
            .with_help(
                "Brrrty doesn't read your 11ty config yet; collections defined there are coming in a later release.",
            ),
        );
    }
    if undefined.contains("collections") {
        return Some(match gap {
            Some(why) => Problem::error(
                codes::DATA_UNSUPPORTED,
                format!("this page uses `collections`, but {why}"),
            )
            .with_help("Fix the problems reported above."),
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
    gap: Option<&str>,
) -> Option<Problem> {
    if let Some(p) = collections_problem(undefined, gap) {
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
        .with_help("Fix the data file errors reported above."),
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
    report.problems.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
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
    report.elapsed = start.elapsed();
    report
}

/// Stack for each render thread. Deeply nested Markdown and Liquid recurse
/// deeply; the default 2 MiB is not enough for 10,000 nested blockquotes.
const RENDER_STACK: usize = 16 * 1024 * 1024;

/// Build the site described by `opts`.
pub fn build(opts: &BuildOptions, markdown: &dyn MarkdownRenderer) -> BuildReport {
    let start = Instant::now();
    match rayon::ThreadPoolBuilder::new()
        .stack_size(RENDER_STACK)
        .build()
    {
        Ok(pool) => pool.install(|| build_in_pool(opts, markdown, start)),
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
            };
            finish(report, opts.strict, start)
        }
    }
}

fn build_in_pool(
    opts: &BuildOptions,
    markdown: &dyn MarkdownRenderer,
    start: Instant,
) -> BuildReport {
    let mut report = BuildReport {
        written: 0,
        copied: 0,
        unchanged: 0,
        problems: Vec::new(),
        elapsed: Duration::ZERO,
    };
    let strict = opts.strict;
    let has_error = |r: &BuildReport| r.has_errors();

    if let Err(p) = opts.validate() {
        report.problems.push(p);
        return finish(report, strict, start);
    }
    let mut has_config = false;
    if let Ok(root) = opts.root_dir() {
        for name in [
            "eleventy.config.js",
            "eleventy.config.cjs",
            "eleventy.config.mjs",
            ".eleventy.js",
        ] {
            if root.join(name).is_file() {
                has_config = true;
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
    let (root, input, out) = match (opts.root_dir(), opts.input_dir(), opts.output_dir()) {
        (Ok(r), Ok(i), Ok(o)) => (r, i, o),
        (Err(p), _, _) | (_, Err(p), _) | (_, _, Err(p)) => {
            report.problems.push(p);
            return finish(report, strict, start);
        }
    };
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
        report
            .problems
            .push(unsupported_template(&input_rel.join(path)));
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
    let includes = input.join("_includes");
    let includes = includes.canonicalize().unwrap_or(includes);
    let loader = FsLoader::new(
        includes,
        input_rel.join("_includes"),
        input.clone(),
        input_rel.clone(),
    );
    let layouts = Layouts::new(&engine, input.clone(), input_rel.clone());
    let data_dir = input_rel.join("_data");
    let GlobalData {
        value: user_value,
        js_keys,
        problems: data_problems,
        unavailable,
    } = load_global(&input, &root);
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
    let (globals, output_shown) = globals(opts, &root, &input_rel);
    // 11ty merges its `eleventy` data over `_data` (deeply), then replaces
    // `pkg` outright with the project's package.json.
    let mut global = Data::new();
    merge(&mut global, &user_data);
    let mut eleventy = Data::new();
    if let Some(v) = globals.get("eleventy") {
        eleventy.insert("eleventy".to_string(), v.clone());
    }
    merge(&mut global, &eleventy);
    if let Some(v) = globals.get("pkg") {
        global.insert("pkg".to_string(), v.clone());
    }
    // Nunjucks templates aren't rendered, but they are loaded, with their
    // data files: they are collection items like any other page.
    let (nunjucks, javascript): (Vec<PathBuf>, Vec<PathBuf>) = discovery
        .unsupported
        .iter()
        .cloned()
        .partition(|rel| is_nunjucks(rel));
    let loadable: Vec<PathBuf> = discovery
        .templates
        .iter()
        .cloned()
        .chain(nunjucks)
        .collect();
    let dir_data = DirDataIndex::load(&input, &input_rel, &loadable);
    let shared = Shared {
        engine: &engine,
        loader: &loader,
        input: &input,
        input_rel: &input_rel,
        layouts: &layouts,
        output_shown,
        global,
        withheld,
        dir_data: &dir_data,
        data_dir: data_dir.clone(),
    };

    let loaded: Vec<Loaded> = loadable.par_iter().map(|rel| load(&shared, rel)).collect();
    let mut templates: Vec<Templ> = Vec::new();
    let mut from_layouts = Vec::new();
    let mut from_data = Vec::new();
    let mut unbuilt = 0usize;
    let mut unplaced: Vec<Problem> = Vec::new();
    for item in loaded {
        match item {
            Loaded::Template(t) => templates.push(*t),
            Loaded::Unplaced(p) => unplaced.push(p),
            Loaded::Failed(p) => {
                unbuilt += 1;
                report.problems.push(p);
            }
            Loaded::LayoutFailed(p) => {
                unbuilt += 1;
                from_layouts.push(p);
            }
            Loaded::DataFailed(p) => {
                unbuilt += 1;
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
    let mut gap = collections_gap(
        unbuilt,
        &unplaced,
        javascript.len(),
        templates.iter().any(|t| t.computed_membership),
        js_data,
    );
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
        entries.push(Entry {
            input_path,
            membership: t.membership,
            pages,
        });
        slots.push(slot);
    }
    let mut collections_value: Option<Arc<IndexMap<String, Value>>> = None;
    let mut made: Vec<Option<Vec<Prepared>>> = slots.iter().map(|_| None).collect();
    // Each template's place in 11ty's order, which is file order until the
    // collections give the real one.
    let mut template_order: Vec<usize> = (0..slots.len()).collect();
    if gap.is_none() {
        let built = collections::build(entries, |i, so_far| {
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
        for (i, problem) in built.failed {
            match blocking_layout(&slots[i]) {
                Some(layout) => layout_refusals.push(layout),
                None => report.problems.push(problem),
            }
        }
        for i in built.skipped {
            if let Some(layout) = blocking_layout(&slots[i]) {
                layout_refusals.push(layout);
            } else if let Slot::Deferred(d) = &slots[i] {
                report.problems.push(
                    Problem::error(
                        codes::PAGINATION_UNSUPPORTED,
                        "this page paginates a collection, but another template that paginates a collection failed first",
                    )
                    .with_path(&d.shown)
                    .with_help("Fix the problem reported above."),
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
                layout_refusals.push(layout);
            } else if let Slot::Deferred(d) = slot {
                report.problems.push(
                    Problem::error(
                        codes::PAGINATION_UNSUPPORTED,
                        format!("this page paginates a collection, but {why}"),
                    )
                    .with_path(&d.shown)
                    .with_help("Fix the problems reported above."),
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
        .filter(|_| has_config)
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
    // content is still rendered for the pages that read it. A Nunjucks
    // template is reported once, as unsupported, and never written.
    let writes = |p: &Prepared| {
        p.source.unsupported.is_none()
            && p.output_rel
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
        engine: &engine,
        loader: &loader,
        markdown,
    };
    let jobs: Vec<ContentPage<'_>> = prepared
        .iter()
        .enumerate()
        .map(|(i, p)| ContentPage {
            job: Job {
                shown: &p.source.shown,
                body: &p.source.body,
                body_line: p.source.body_line,
                engines: p.source.engines,
                data: &p.data,
                layouts: &p.source.layouts,
            },
            template: page_template[i],
            page_number: p.page_number,
            item: page_item[i],
            renderable: content::renderable(&p.data),
            unsupported: p.source.unsupported.as_ref(),
        })
        .collect();
    let check = |i: usize, undefined: &BTreeSet<String>| {
        let src = &prepared[i].source;
        missing_data_problem(undefined, &src.withheld, &data_dir, gap.as_deref())
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
    type Written = Result<(), Refused>;
    let results: Vec<Written> = (0..prepared.len())
        .into_par_iter()
        .filter(|&i| writes(&prepared[i]))
        .map(|i| -> Written {
            let p = &prepared[i];
            if let Some(problem) = unsupported_layout(p) {
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
            let content = contents.content(i)?;
            let guard = traps::install(settled.clone());
            let rendered = pipeline.layouts(&jobs[i].job, content);
            let reads = traps::take_item_reads();
            drop(guard);
            if let Some(j) = reads
                .iter()
                .find(|(_, o)| *o == Outcome::Refused)
                .and_then(|(id, _)| contents.page_of(*id))
            {
                return Err(Refused::Page(contents.needs_problem(i, j), Some(j)));
            }
            let rendered = rendered.map_err(|p| (p, None))?;
            if let Some(problem) = missing_data_problem(
                &rendered.undefined,
                &src.withheld,
                &data_dir,
                gap.as_deref(),
            ) {
                return Err(Refused::Page(problem.with_path(shown), None));
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    (
                        Problem::error(codes::WRITE_FAILED, format!("cannot create folder: {e}"))
                            .with_path(shown),
                        None,
                    )
                })?;
            }
            fs::write(&dest, rendered.html).map_err(|e| {
                (
                    Problem::error(codes::WRITE_FAILED, format!("cannot write output: {e}"))
                        .with_path(shown),
                    None,
                )
            })?;
            Ok(())
        })
        .collect();
    // The pages of one template often fail the same way: say it once. A page
    // that isn't written reports its own problem only when another page's
    // problem points to it.
    let mut seen = BTreeSet::new();
    let mut report_problem = |report: &mut BuildReport, p: Problem| {
        let key = format!(
            "{}|{:?}|{:?}|{:?}|{}",
            p.code, p.path, p.line, p.column, p.message
        );
        if seen.insert(key) {
            report.problems.push(p);
        }
    };
    let mut pointed: Vec<usize> = Vec::new();
    let mut from_layouts = layout_refusals;
    for r in results {
        match r {
            Ok(()) => report.written += 1,
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
        if writes(&prepared[j])
            || prepared[j].source.unsupported.is_some()
            || !contents.renders(j)
            || !explained.insert(j)
        {
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

    let pages: HashMap<PathBuf, PathBuf> = prepared
        .iter()
        .filter(|p| writes(p) && unsupported_layout(p).is_none())
        .filter_map(|p| {
            p.output_rel
                .as_ref()
                .map(|o| (out.join(o), input_rel.join(&p.source.input_path)))
        })
        .collect();
    let pt = copy_passthrough_avoiding(opts, &pages);
    // A passthrough file that replaced a page means that page was not really written.
    report.written = report.written.saturating_sub(pt.overridden.len());
    report.copied = pt.copied;
    report.unchanged = pt.unchanged;
    report.problems.extend(pt.problems);
    finish(report, strict, start)
}

/// The problem for a template in a language Brrrty doesn't render yet.
fn unsupported_template(shown: &Path) -> Problem {
    let njk = shown
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("njk"));
    let help = if njk {
        "Nunjucks support is coming soon. Liquid, Markdown and HTML templates work now."
    } else {
        "JavaScript templates are coming soon. Liquid, Markdown and HTML templates work now."
    };
    Problem::error(
        codes::TEMPLATE_UNSUPPORTED,
        "this template language is not supported yet",
    )
    .with_path(shown)
    .with_help(help)
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

/// Which template page a slot holds while the collections are built.
enum Slot {
    Ready(Vec<Prepared>),
    Deferred(Box<Deferred>),
}

/// Why the collections can't be built, when they can't. A collection built
/// without a page would be wrong, so every read of `collections` is refused.
fn collections_gap(
    unbuilt: usize,
    unplaced: &[Problem],
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
    if let Some(first) = unplaced.first() {
        let path = first
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let more = match unplaced.len() {
            1 => String::new(),
            2 => " (and 1 more template)".to_string(),
            n => format!(" (and {} more templates)", n - 1),
        };
        return Some(format!(
            "Brrrty can't tell where {path} goes in the collections: {}{more}",
            first.message
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
