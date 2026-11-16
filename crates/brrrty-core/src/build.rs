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
use serde_json::{Map, Value as Json};

use crate::data::{
    global_data_names, js_object, key_line, local_data_files, merge, reserved_keys,
    reserved_problem,
};
use crate::discover::{PLUGIN_ONLY_EXTS, discover};
use crate::frontmatter::split_front_matter;
use crate::layout::{ChainError, LayoutFile, Layouts};
use crate::loader::FsLoader;
use crate::options::BuildOptions;
use crate::page::{Page, file_path_stem, file_slug, resolve_date};
use crate::passthrough::copy_passthrough_avoiding;
use crate::permalink::{Permalink, resolve_permalink};
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
}

/// What every page shares while loading.
struct Shared<'a> {
    input: &'a Path,
    input_rel: &'a Path,
    layouts: &'a Layouts<'a>,
    /// `eleventy` and `pkg`, the data 11ty supplies to every page.
    globals: Map<String, Json>,
    /// The output folder as 11ty spells it in `page.outputPath`.
    output_shown: String,
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
fn globals(opts: &BuildOptions, root: &Path, input_rel: &Path) -> (Map<String, Json>, String) {
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
        .unwrap_or_else(|| Json::Object(Map::new()));
    let mut g = Map::new();
    g.insert("eleventy".to_string(), eleventy);
    g.insert("pkg".to_string(), pkg);
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
    let (front, body) = split_front_matter(&text).map_err(|p| p.with_path(&shown))?;

    // Data files can set anything, a layout or permalink included, so check
    // them before reading anything else into the page.
    if let Some(file) = local_data_files(shared.input, shared.input_rel, rel).first() {
        return Err(Problem::error(
            codes::DATA_UNSUPPORTED,
            format!(
                "data file {} applies to this page, and Brrrty doesn't read data files yet",
                file.display()
            ),
        )
        .with_path(&shown)
        .with_help("Template and directory data files are in progress."));
    }

    let layouts = match front.get("layout") {
        None => Vec::new(),
        Some(v) if !crate::js::js_truthy(v) => Vec::new(),
        Some(Json::String(name)) => match shared.layouts.chain(name, &shown, &text) {
            Ok(chain) => chain,
            Err(ChainError::Page(p)) => return Err(p),
            Err(ChainError::Layout(p)) => return Ok(Loaded::LayoutFailed(p)),
        },
        Some(other) => {
            return Err(at(
                Problem::error(
                    codes::LAYOUT_INVALID,
                    format!(
                        "`layout` must be a layout name, found {}",
                        serde_json::to_string(other).unwrap_or_default()
                    ),
                )
                .with_path(&shown)
                .with_help("write the layout's name, like `layout: base`"),
                key_line(&text, "layout"),
            ));
        }
    };

    // 11ty refuses reserved names anywhere in the data.
    let reserved = reserved_keys(&front);
    if !reserved.is_empty() {
        let key = reserved[0].split('.').next().unwrap_or("");
        return Err(at(
            reserved_problem(&reserved).with_path(&shown),
            key_line(&text, key),
        ));
    }
    for layout in &layouts {
        let reserved = reserved_keys(&layout.data);
        if !reserved.is_empty() {
            return Ok(Loaded::LayoutFailed(
                reserved_problem(&reserved).with_path(&layout.shown),
            ));
        }
    }

    // The layout chain's data, farthest first and without `layout`, then the
    // page's own, over the data 11ty supplies.
    let mut layout_data = Map::new();
    for layout in layouts.iter().rev() {
        merge(&mut layout_data, &layout.data);
    }
    layout_data.shift_remove("layout");
    let mut data = shared.globals.clone();
    merge(&mut data, &layout_data);
    merge(&mut data, &front);

    reject_unsupported_keys(&data, &shown)?;
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
    let permalink = resolve_permalink(rel, &data).map_err(|p| p.with_path(&shown))?;
    let Permalink::Write { url, output_rel } = permalink else {
        return Ok(Loaded::Skip);
    };
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
        Some(Json::Object(own)) => js_object(own),
        _ => IndexMap::new(),
    };
    let string = |s: String| Value::String(s.into());
    let input_path = format!("{}{}", dot_dir(shared.input_rel), slash_path(rel));
    let output_path = format!("{}{}", shared.output_shown, slash_path(&output_rel));
    page.insert("inputPath".into(), string(input_path));
    page.insert("fileSlug".into(), string(file_slug(rel)));
    page.insert("filePathStem".into(), string(file_path_stem(rel)));
    page.insert("outputFileExtension".into(), string("html".into()));
    page.insert("templateSyntax".into(), string(syntax));
    page.insert("date".into(), Value::Date(date.fixed_offset()));
    page.insert("rawInput".into(), string(body.to_string()));
    page.insert("url".into(), string(url));
    page.insert("outputPath".into(), string(output_path));
    let mut values = js_object(&data);
    values.insert("page".into(), Value::Object(Arc::new(page)));

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
    })))
}

/// Front matter keys that change a page's output in ways Brrrty can't reproduce yet.
const UNSUPPORTED_KEYS: &[&str] = &["pagination", "eleventyComputed"];

/// Refuse pages that 11ty would process further than Brrrty can yet, instead of writing
/// output that silently differs.
fn reject_unsupported_keys(data: &Map<String, Json>, shown: &Path) -> Result<(), Problem> {
    let keys: Vec<String> = UNSUPPORTED_KEYS
        .iter()
        .filter(|k| data.contains_key(**k))
        .map(|k| format!("`{k}`"))
        .collect();
    if keys.is_empty() {
        return Ok(());
    }
    let message = if keys.len() == 1 {
        format!("Front matter key {} isn't supported yet", keys[0])
    } else {
        format!("Front matter keys {} aren't supported yet", keys.join(", "))
    };
    Err(Problem::error(codes::FRONT_MATTER_UNSUPPORTED, message)
        .with_path(shown)
        .with_help("Pagination and computed data are in progress."))
}

/// The problem for a page that read data Brrrty doesn't supply yet.
fn missing_data_problem(
    undefined: &BTreeSet<String>,
    data_names: &BTreeSet<String>,
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
    let used: Vec<String> = undefined
        .intersection(data_names)
        .map(|n| format!("`{n}`"))
        .collect();
    if used.is_empty() {
        return None;
    }
    Some(
        Problem::error(
            codes::DATA_UNSUPPORTED,
            format!(
                "this page uses {} from {}, which Brrrty doesn't read yet",
                used.join(", "),
                data_dir.display()
            ),
        )
        .with_help("Global data files are in progress."),
    )
}

/// One problem per distinct layout problem, saying how many pages it stopped.
fn layout_problems(found: Vec<Problem>) -> Vec<Problem> {
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
                "1 page uses this layout and was not written.".to_string()
            } else {
                format!("{n} pages use this layout and were not written.")
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
    let data_names = global_data_names(&input.join("_data"));
    let (globals, output_shown) = globals(opts, &root, &input_rel);
    let shared = Shared {
        input: &input,
        input_rel: &input_rel,
        layouts: &layouts,
        globals,
        output_shown,
    };

    let loaded: Vec<Loaded> = discovery
        .templates
        .par_iter()
        .map(|rel| load(&shared, rel))
        .collect();
    let mut prepared = Vec::new();
    let mut from_layouts = Vec::new();
    for item in loaded {
        match item {
            Loaded::Page(p) => prepared.push(*p),
            Loaded::Skip => {}
            Loaded::Failed(p) => report.problems.push(p),
            Loaded::LayoutFailed(p) => from_layouts.push(p),
        }
    }
    report.problems.extend(layout_problems(from_layouts));
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
            if let Some(problem) = missing_data_problem(&rendered.undefined, &data_names, &data_dir)
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
