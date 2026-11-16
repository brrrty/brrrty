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
use crate::page::{Page, file_path_stem, file_slug, resolve_date};
use crate::passthrough::copy_passthrough_avoiding;
use crate::permalink::{Locator, Permalink};
use crate::problem::{Problem, Severity, codes};
use crate::render::{Engines, Job, Pipeline, body_line, engines_for};

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

struct Prepared {
    page: Page,
    output_rel: PathBuf,
    /// The page's path relative to the project root.
    shown: PathBuf,
    body_line: usize,
    engines: Engines,
    data: IndexMap<String, Value>,
    layouts: Vec<Arc<LayoutFile>>,
    /// Keys left out of `data` because Brrrty can't supply their value.
    withheld: Vec<(String, Withheld)>,
}

/// What loading one template gave.
enum Loaded {
    Page(Box<Prepared>),
    /// `permalink: false`.
    Skip,
    /// A problem with this page.
    Failed(Problem),
    /// A problem in a layout file, the same for every page that uses it.
    LayoutFailed(Problem),
    /// A problem in a data file, the same for every page it applies to.
    DataFailed(Problem),
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

fn load_page(shared: &Shared<'_>, rel: &Path) -> Result<Loaded, Problem> {
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
    reject_unsupported_keys(&data, &shown, source)?;
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
    let (engines, syntax) = engines_for(&ext, data.get("templateEngineOverride")).map_err(|p| {
        at(
            p.with_path(&shown),
            key_line(&text, "templateEngineOverride"),
        )
    })?;

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

    // The permalink renders with the page's data, before `page.url` and
    // `page.outputPath` exist. With computed data, 11ty runs it as the
    // entries that set them, in their place in the order.
    let locator = Locator {
        engine: shared.engine,
        loader: shared.loader,
        rel,
        shown: &shown,
        output_shown: &shared.output_shown,
        raw: engines == Engines::Raw,
        from: if data.contains_key("permalink") && !front.contains_key("permalink") {
            source("permalink")
        } else {
            None
        },
        line: front
            .contains_key("permalink")
            .then(|| key_line(&text, "permalink"))
            .flatten(),
    };
    let mut undefined = BTreeSet::new();
    let (values, permalink) = match &plan {
        Some(plan) => {
            let applied = plan.apply(
                shared.engine,
                shared.loader,
                &shown,
                &values,
                &mut undefined,
                &locator,
            )?;
            (applied.data, applied.permalink)
        }
        None => {
            let found = locator.locate(&values, &mut undefined)?;
            let mut values = values;
            if let Some(Value::Object(page)) = values.get_mut("page") {
                let page = Arc::make_mut(page);
                page.insert("url".into(), found.url);
                page.insert("outputPath".into(), found.output_path);
            }
            (values, found.permalink)
        }
    };
    // A permalink or computed value that read data Brrrty can't supply
    // would put the page in the wrong place.
    if let Some(problem) = missing_data_problem(&undefined, &withheld, &shared.data_dir) {
        return Err(problem.with_path(&shown));
    }
    let Permalink::Write { output_rel, .. } = permalink else {
        return Ok(Loaded::Skip);
    };

    Ok(Loaded::Page(Box::new(Prepared {
        body_line: body_line(&text, body),
        page: Page {
            input_path: rel.to_path_buf(),
            file_slug: file_slug(rel),
            date,
            data,
            body: body.to_string(),
        },
        output_rel,
        shown,
        engines,
        data: values,
        layouts,
        withheld,
    })))
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

/// Keys that change a page's output in ways Brrrty can't reproduce yet.
const UNSUPPORTED_KEYS: &[&str] = &["pagination"];

/// Refuse pages that 11ty would process further than Brrrty can yet, instead of writing
/// output that silently differs. `source` says where a key comes from
/// (`None` for front matter).
fn reject_unsupported_keys(
    data: &Data,
    shown: &Path,
    source: impl Fn(&str) -> Option<String>,
) -> Result<(), Problem> {
    let found: Vec<(&str, Option<String>)> = UNSUPPORTED_KEYS
        .iter()
        .filter(|k| data.contains_key(**k))
        .map(|k| (*k, source(k)))
        .collect();
    if found.is_empty() {
        return Ok(());
    }
    let all_front = found.iter().all(|(_, s)| s.is_none());
    let keys: Vec<String> = found
        .iter()
        .map(|(k, s)| match s {
            Some(from) => format!("`{k}` (from {from})"),
            None => format!("`{k}`"),
        })
        .collect();
    let what = if all_front {
        "Front matter key"
    } else {
        "Data key"
    };
    let message = if keys.len() == 1 {
        format!("{what} {} isn't supported yet", keys[0])
    } else {
        format!("{what}s {} aren't supported yet", keys.join(", "))
    };
    Err(Problem::error(codes::FRONT_MATTER_UNSUPPORTED, message)
        .with_path(shown)
        .with_help("Pagination is in progress."))
}

/// The problem for a page that read data Brrrty doesn't supply yet.
fn missing_data_problem(
    undefined: &BTreeSet<String>,
    withheld: &[(String, Withheld)],
    data_dir: &Path,
) -> Option<Problem> {
    if undefined.contains("collections") {
        return Some(
            Problem::error(
                codes::DATA_UNSUPPORTED,
                "this page uses `collections`, which Brrrty doesn't build yet",
            )
            .with_help("Collections are in progress."),
        );
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
    if let Ok(root) = opts.root_dir() {
        for name in [
            "eleventy.config.js",
            "eleventy.config.cjs",
            "eleventy.config.mjs",
            ".eleventy.js",
        ] {
            if root.join(name).is_file() {
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
        report.problems.push(
            Problem::error(
                codes::TEMPLATE_UNSUPPORTED,
                "this template language is not supported yet",
            )
            .with_path(input_rel.join(path))
            .with_help("Nunjucks and JavaScript template support is in progress."),
        );
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
    let dir_data = DirDataIndex::load(&input, &input_rel, &discovery.templates);
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

    let loaded: Vec<Loaded> = discovery
        .templates
        .par_iter()
        .map(|rel| load(&shared, rel))
        .collect();
    let mut prepared = Vec::new();
    let mut from_layouts = Vec::new();
    let mut from_data = Vec::new();
    for item in loaded {
        match item {
            Loaded::Page(p) => prepared.push(*p),
            Loaded::Skip => {}
            Loaded::Failed(p) => report.problems.push(p),
            Loaded::LayoutFailed(p) => from_layouts.push(p),
            Loaded::DataFailed(p) => from_data.push(p),
        }
    }
    report
        .problems
        .extend(shared_problems(from_layouts, "layout"));
    report
        .problems
        .extend(shared_problems(from_data, "data file"));
    if strict && has_error(&report) {
        return finish(report, strict, start);
    }

    // Detect outputs that two or more pages claim.
    let mut by_output: BTreeMap<&Path, Vec<&Path>> = BTreeMap::new();
    for p in &prepared {
        by_output
            .entry(p.output_rel.as_path())
            .or_default()
            .push(p.page.input_path.as_path());
    }
    let mut colliding: Vec<PathBuf> = Vec::new();
    for (output, inputs) in &by_output {
        if inputs.len() > 1 {
            let list: Vec<String> = inputs
                .iter()
                .map(|i| input_rel.join(i).display().to_string())
                .collect();
            report.problems.push(
                Problem::error(
                    codes::OUTPUT_COLLISION,
                    format!(
                        "{} pages write to {}: {}",
                        inputs.len(),
                        output.display(),
                        list.join(", ")
                    ),
                )
                .with_path(input_rel.join(inputs[0]))
                .with_help("give each page a unique permalink"),
            );
            colliding.push(output.to_path_buf());
        }
    }
    prepared.retain(|p| !colliding.contains(&p.output_rel));
    if strict && has_error(&report) {
        return finish(report, strict, start);
    }

    // Render and write.
    let pipeline = Pipeline {
        engine: &engine,
        loader: &loader,
        markdown,
    };
    let results: Vec<Result<(), Problem>> = prepared
        .par_iter()
        .map(|p| {
            let shown = &p.shown;
            let Some(dest) = contained_output_path(&out, &p.output_rel) else {
                return Err(Problem::error(
                    codes::OUTPUT_ESCAPES,
                    format!(
                        "{} would be written outside the output folder",
                        p.output_rel.display()
                    ),
                )
                .with_path(shown));
            };
            let rendered = pipeline.render(&Job {
                shown,
                body: &p.page.body,
                body_line: p.body_line,
                engines: p.engines,
                data: &p.data,
                layouts: &p.layouts,
            })?;
            if let Some(problem) = missing_data_problem(&rendered.undefined, &p.withheld, &data_dir)
            {
                return Err(problem.with_path(shown));
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    Problem::error(codes::WRITE_FAILED, format!("cannot create folder: {e}"))
                        .with_path(shown)
                })?;
            }
            fs::write(&dest, rendered.html).map_err(|e| {
                Problem::error(codes::WRITE_FAILED, format!("cannot write output: {e}"))
                    .with_path(shown)
            })
        })
        .collect();
    for r in results {
        match r {
            Ok(()) => report.written += 1,
            Err(p) => report.problems.push(p),
        }
    }
    if strict && has_error(&report) {
        return finish(report, strict, start);
    }

    let pages: HashMap<PathBuf, PathBuf> = prepared
        .iter()
        .map(|p| (out.join(&p.output_rel), input_rel.join(&p.page.input_path)))
        .collect();
    let pt = copy_passthrough_avoiding(opts, &pages);
    // A passthrough file that replaced a page means that page was not really written.
    report.written = report.written.saturating_sub(pt.overridden.len());
    report.copied = pt.copied;
    report.unchanged = pt.unchanged;
    report.problems.extend(pt.problems);
    finish(report, strict, start)
}

#[cfg(test)]
mod tests {
    use super::*;

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
