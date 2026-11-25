use crate::frontmatter::Data;
use crate::problem::{Problem, codes};
use std::path::{Component, Path, PathBuf};

/// Options for one build.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    pub root: PathBuf,
    /// Relative to `root`.
    pub input: PathBuf,
    /// Relative to `root`, or absolute.
    pub output: PathBuf,
    /// Relative to `root`; copied after the config's passthrough entries.
    pub passthrough: Vec<PathBuf>,
    pub strict: bool,
    /// What the project's config file set.
    pub config: ConfigOptions,
    /// Write the site here instead of the output folder. The output folder
    /// still names the site everywhere 11ty shows it (`page.outputPath`,
    /// `eleventy.directories.output`) and is still left out of the input.
    /// `brrrty serve` builds into a fresh folder each time this way.
    pub write_to: Option<PathBuf>,
    /// How 11ty was started, as `eleventy.env.runMode` shows it.
    pub run_mode: RunMode,
    /// Whether JavaScript data files run, and in which Node.
    pub js: JsOptions,
}

/// Whether a site's JavaScript data files run (`config.set_js_data`,
/// `--js-data`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum JsData {
    /// Pages that need JavaScript data are refused, as before 0.0.22.
    #[default]
    Off,
    /// They run in the site's own Node.
    Node,
}

impl JsData {
    pub fn as_str(self) -> &'static str {
        match self {
            JsData::Off => "off",
            JsData::Node => "node",
        }
    }

    /// `"node"` or `"off"`.
    pub fn parse(s: &str) -> Option<JsData> {
        match s {
            "node" => Some(JsData::Node),
            "off" => Some(JsData::Off),
            _ => None,
        }
    }
}

/// Where the JavaScript data mode came from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum JsDataSource {
    /// Neither the config nor the command line said.
    #[default]
    Default,
    /// `config.set_js_data(...)` in the config file, at this line.
    Config { file: PathBuf, line: Option<usize> },
    /// `--js-data` on the command line.
    Flag,
    /// On because the JS config ran in Node: its Node runs the data files too.
    JsConfig,
}

/// The Node found to run JavaScript data, as doctor shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsNode {
    pub path: PathBuf,
    /// `v24.22.3`.
    pub version: String,
    /// `process.versions.tz`.
    pub tz: String,
    /// It can record every module the data loads (Node 22.15 and later).
    pub load_hooks: bool,
}

/// Why no Node runs the JavaScript data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsNodeMissing {
    /// Node was found, but it is older than 18.
    pub too_old: bool,
    /// What happened, in one line (`Node not found (tried node on PATH)`).
    pub message: String,
}

/// Starts a JavaScript runtime for one build.
pub type JsStarter = std::sync::Arc<
    dyn Fn(RunMode) -> Result<Box<dyn brrrty_js::runtime::JsRuntime>, String> + Send + Sync,
>;

/// How JavaScript data files are run in this build.
#[derive(Clone, Default)]
pub struct JsOptions {
    pub mode: JsData,
    pub source: JsDataSource,
    /// Whether the site's JavaScript is run again in other orders before
    /// anything is written (`--no-order-check` and
    /// `config.set_order_check(false)` turn it off).
    pub order_check: bool,
    /// The Node that runs them, or why there is none. Looked for only when
    /// the mode is `Node`; `None` otherwise.
    pub node: Option<Result<JsNode, JsNodeMissing>>,
    /// Starts the runtime; set when the mode is `Node` and Node was found.
    pub start: Option<JsStarter>,
}

impl std::fmt::Debug for JsOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsOptions")
            .field("mode", &self.mode)
            .field("source", &self.source)
            .field("node", &self.node)
            .field("start", &self.start.is_some())
            .finish()
    }
}

/// The folder `brrrty serve` builds into, inside the output folder. Brrrty owns the name:
/// `brrrty build` removes one a stopped session left behind, so it is never deployed.
pub const SERVE_STAGING_DIR: &str = ".brrrty-serve";

/// How 11ty was started (`eleventy.env.runMode`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RunMode {
    /// `eleventy`, `brrrty build`.
    #[default]
    Build,
    /// `eleventy --serve`, `brrrty serve`.
    Serve,
}

impl RunMode {
    /// The name 11ty gives it.
    pub fn as_str(self) -> &'static str {
        match self {
            RunMode::Build => "build",
            RunMode::Serve => "serve",
        }
    }
}

/// What a config file (`brrrty.config.rhai`) set, besides the folders,
/// which go in [`BuildOptions`] itself.
#[derive(Debug, Clone, Default)]
pub struct ConfigOptions {
    /// The config file, relative to the root; `None` when there is none.
    pub file: Option<PathBuf>,
    /// `addPassthroughCopy` entries, in 11ty's order.
    pub passthrough: Vec<PassthroughEntry>,
    /// `ignores.add` globs, relative to the root as 11ty reads them.
    pub ignores: Vec<String>,
    /// `addGlobalData` values by name as given: `a.b` sets `b` inside `a`.
    pub global_data: Data,
    /// Warnings from reading the config, reported with the build's.
    pub problems: Vec<Problem>,
    /// `add_filter` filters, in the order they were added. A later one
    /// replaces an earlier one of the same name, and any standard filter.
    pub filters: Vec<ConfigFilter>,
    /// `add_collection` collections, in the order they were added.
    pub collections: Vec<ConfigCollection>,
    /// Runs `.rhai` data files. A site needs no config file to have them,
    /// so the command line sets this whether or not there is a config.
    pub data: Option<DataRunner>,
    /// 11ty's syntax highlighting plugin
    /// (`@11ty/eleventy-plugin-syntaxhighlight`), when the config added it.
    pub syntax_highlight: Option<crate::plugins::syntaxhighlight::SyntaxHighlight>,
    /// Renderer rules on 11ty's own markdown-it (`amendLibrary("md", ...)`), when the config
    /// set any.
    pub markdown_rules: Option<MarkdownRules>,
    /// The feeds `feedPlugin` (`@11ty/eleventy-plugin-rss`) adds, in the
    /// order they were added: each is a virtual template.
    pub feeds: Vec<crate::plugins::rss::FeedOptions>,
    /// The RSS plugin's filters without a feed (its default export,
    /// `rssPlugin`). A feed adds them too.
    pub rss_filters: bool,
    /// `dir.includes`, relative to the input folder; `None` is `_includes`.
    pub includes: Option<String>,
    /// `dir.layouts`, relative to the input folder; `None` is the includes folder.
    pub layouts: Option<String>,
    /// `dir.data`, relative to the input folder; `None` is `_data`.
    pub data_folder: Option<String>,
    /// The template formats to build; `None` is 11ty's defaults (`liquid`,
    /// `md`, `njk`, `html`, `11ty.js`).
    pub template_formats: Option<Vec<String>>,
    /// `template_formats` was set, not just added to.
    pub template_formats_set: bool,
    /// `add_template_formats` names, in order.
    pub added_template_formats: Vec<String>,
    /// `addLayoutAlias` aliases: a layout name and the file it stands for,
    /// relative to the layouts folder.
    pub layout_aliases: indexmap::IndexMap<String, String>,
    /// `markdownTemplateEngine` and `htmlTemplateEngine`.
    pub template_engines: TemplateEngines,
    /// What the config's and the data files' scripts read from the project folder.
    pub reads: FileReads,
    /// Warnings found while pages render (a `config` call made too late), reported with the
    /// build's.
    pub late_problems: LateProblems,
    /// `config.set_js_data(...)`: the mode and the line of the call.
    pub js_data: Option<(JsData, Option<usize>)>,
    /// `config.set_js_config(...)`: the mode and the line of the call.
    pub js_config: Option<(JsData, Option<usize>)>,
    /// Filters from the JS config, in the order added.
    pub js_filters: Vec<JsConfigFilter>,
    /// The Node that loaded the JS config, when it did.
    pub js_started: Option<StartedJs>,
    /// `config.set_order_check(...)`, when the config calls it.
    pub order_check: Option<bool>,
    /// The config still has `brrrty init`'s TODO markers: this error refuses every page, and
    /// nothing is written or copied.
    pub unported: Option<Problem>,
}

impl ConfigOptions {
    /// The includes folder, relative to the input folder.
    pub fn includes_dir(&self) -> &str {
        self.includes.as_deref().unwrap_or("_includes")
    }

    /// Whether `format` (`md`, `njk`, `11ty.js`...) is one of the template formats.
    pub fn has_format(&self, format: &str) -> bool {
        match &self.template_formats {
            None => matches!(format, "liquid" | "md" | "njk" | "html" | "11ty.js"),
            Some(f) => f.iter().any(|x| x == format),
        }
    }

    /// The extensions an extensionless layout name is tried with, in order:
    /// the template formats as listed, with `11ty.js` standing for the three
    /// JavaScript spellings.
    pub fn layout_extensions(&self) -> Vec<String> {
        let default = ["liquid", "md", "njk", "html", "11ty.js"];
        let list: Vec<&str> = match &self.template_formats {
            None => default.to_vec(),
            Some(f) => f.iter().map(String::as_str).collect(),
        };
        let mut out = Vec::new();
        for f in list {
            if f == "11ty.js" {
                out.extend(["11ty.js", "11ty.cjs", "11ty.mjs"].map(String::from));
            } else {
                out.push(f.to_string());
            }
        }
        out
    }

    /// The global data folder, relative to the input folder.
    pub fn data_dir(&self) -> &str {
        self.data_folder.as_deref().unwrap_or("_data")
    }

    /// The layouts folder, relative to the input folder: `dir.layouts`, or
    /// the includes folder.
    pub fn layouts_dir(&self) -> &str {
        self.layouts.as_deref().unwrap_or(self.includes_dir())
    }
}

/// Warnings a script raises while pages render, each kept once. Shared by the config's engine
/// and the build, which adds them to its report.
#[derive(Debug, Clone, Default)]
pub struct LateProblems(std::sync::Arc<std::sync::Mutex<Vec<Problem>>>);

impl LateProblems {
    /// Note `p`, unless the same problem (message and place) is noted already.
    pub fn add(&self, p: Problem) {
        let mut v = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !v
            .iter()
            .any(|q| q.message == p.message && q.line == p.line && q.column == p.column)
        {
            v.push(p);
        }
    }

    /// Everything noted so far.
    pub fn all(&self) -> Vec<Problem> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The files and folders a build's scripts read, listed or checked (found or not), as
/// paths relative to the project folder. Shared by the config's engine and every data
/// file's, so one build's log holds them all; `brrrty serve` rebuilds when one changes.
#[derive(Debug, Clone, Default)]
pub struct FileReads(std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<PathBuf>>>);

impl FileReads {
    /// Note that `path`, relative to the project folder, was read.
    pub fn add(&self, path: PathBuf) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path);
    }

    /// Everything noted so far, sorted.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

/// A template language that runs on a page's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TemplateLanguage {
    Liquid,
    Nunjucks,
}

impl TemplateLanguage {
    /// 11ty's name for it, as `page.templateSyntax` lists it.
    pub fn name(self) -> &'static str {
        match self {
            TemplateLanguage::Liquid => "liquid",
            TemplateLanguage::Nunjucks => "njk",
        }
    }

    /// The language 11ty names `name` (`liquid` or `njk`), if Brrrty runs
    /// it.
    pub fn from_name(name: &str) -> Option<TemplateLanguage> {
        match name {
            "liquid" => Some(TemplateLanguage::Liquid),
            "njk" => Some(TemplateLanguage::Nunjucks),
            _ => None,
        }
    }
}

/// What runs on `.md` and `.html` files before Markdown (11ty's
/// `markdownTemplateEngine` and `htmlTemplateEngine`). `None` is `false`:
/// nothing does. Both are Liquid by default, as in 11ty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateEngines {
    pub markdown: Option<TemplateLanguage>,
    pub html: Option<TemplateLanguage>,
}

impl Default for TemplateEngines {
    fn default() -> Self {
        TemplateEngines {
            markdown: Some(TemplateLanguage::Liquid),
            html: Some(TemplateLanguage::Liquid),
        }
    }
}

/// One `.rhai` data file to run.
#[derive(Debug, Clone, Copy)]
pub struct DataRequest<'a> {
    /// The file, as a path that can be read.
    pub file: &'a Path,
    /// The file as reported, relative to the root.
    pub shown: &'a Path,
    /// The data 11ty hands an exported function: the config's
    /// `add_global_data` values and 11ty's own `eleventy` data.
    pub global: &'a Data,
}

/// Runs a `.rhai` data file and gives its value. Every file gets a fresh
/// engine, so nothing carries over from one to the next. A problem has the
/// file's line and column and no path; the build adds the path.
pub type DataFn =
    std::sync::Arc<dyn Fn(&DataRequest) -> Result<brrrty_js::Value, Problem> + Send + Sync>;

/// A [`DataFn`] that can be shown in a `Debug` print.
#[derive(Clone)]
pub struct DataRunner(pub DataFn);

impl std::fmt::Debug for DataRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DataRunner")
    }
}

/// markdown-it renderer rules for 11ty's own library: every `.md` template and Markdown layout
/// is rendered through them.
#[derive(Clone)]
pub struct MarkdownRules(pub std::sync::Arc<dyn brrrty_markdown::RenderRules + Send + Sync>);

impl std::fmt::Debug for MarkdownRules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MarkdownRules")
    }
}

/// A filter the config added.
#[derive(Clone)]
pub struct ConfigFilter {
    pub name: String,
    pub run: brrrty_liquid::ArgsFilterFn,
}

impl std::fmt::Debug for ConfigFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigFilter")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Which template engines a filter from the JS config is registered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterEngine {
    /// `addFilter`, `addAsyncFilter`: Nunjucks and Liquid.
    All,
    /// `addNunjucksFilter`, `addNunjucksAsyncFilter`.
    Nunjucks,
    /// `addLiquidFilter`.
    Liquid,
}

/// A filter the JS config added: it runs in Node. Rhai filters of the same
/// name replace it.
#[derive(Clone)]
pub struct JsConfigFilter {
    pub name: String,
    pub engine: FilterEngine,
    pub run: brrrty_liquid::ArgsFilterFn,
}

impl std::fmt::Debug for JsConfigFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsConfigFilter")
            .field("name", &self.name)
            .field("engine", &self.engine)
            .finish_non_exhaustive()
    }
}

/// The Node runtime that loaded the JS config, kept for the build: the
/// config's functions live in it. The first user takes it.
#[derive(Clone, Default)]
pub struct StartedJs(
    pub std::sync::Arc<std::sync::Mutex<Option<Box<dyn brrrty_js::runtime::JsRuntime>>>>,
);

impl std::fmt::Debug for StartedJs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StartedJs")
    }
}

/// A collection the config added (`addCollection`).
#[derive(Clone)]
pub struct ConfigCollection {
    pub name: String,
    pub run: CollectionFn,
}

impl std::fmt::Debug for ConfigCollection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigCollection")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Runs a config collection over the pages added so far, as 11ty calls
/// the function with its `TemplateCollection`.
pub type CollectionFn = std::sync::Arc<
    dyn Fn(&CollectionInput) -> Result<brrrty_js::Value, CollectionError> + Send + Sync,
>;

/// The pages 11ty's `TemplateCollection` holds when it computes a config
/// collection.
#[derive(Debug, Clone, Default)]
pub struct CollectionInput {
    /// In the order they were added (`getAll`).
    pub pages: Vec<CollectionPage>,
    /// Indexes into `pages` sorted by date, then input path (`getAllSorted`).
    pub sorted: Vec<usize>,
    /// Pages 11ty has added that Brrrty can't place (no date it can read):
    /// a result that holds one can't be given.
    pub undated: Vec<UndatedPage>,
    /// Why the order pages were added in (`getAll`) isn't known, when it
    /// isn't: then `get_all()` can't be given.
    pub unordered: Option<String>,
}

/// One page of a [`CollectionInput`].
#[derive(Debug, Clone)]
pub struct CollectionPage {
    /// The collection item itself: a result must hold this same value.
    pub item: brrrty_js::Value,
    /// `./src/page.md`, as 11ty names it.
    pub input_path: String,
    /// The tags the page is in (`getIncludedTagNames`).
    pub tags: Vec<String>,
}

/// A page in a [`CollectionInput`] that Brrrty can't date.
#[derive(Debug, Clone)]
pub struct UndatedPage {
    pub input_path: String,
    pub tags: Vec<String>,
}

/// Why a config collection has no value.
#[derive(Debug, Clone)]
pub enum CollectionError {
    /// It failed: the problem says where in the config.
    Failed(Problem),
    /// Its result would hold this page, which Brrrty can't date.
    Undated(String),
}

/// One `addPassthroughCopy` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassthroughEntry {
    /// The file or folder to copy, relative to the root.
    pub from: String,
    /// Where to copy it, relative to the output folder. `None` keeps its
    /// path, relative to the input folder when it is inside it.
    pub to: Option<String>,
}

/// Resolve `.` and `..` lexically, without touching the filesystem.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Make `path` absolute and resolve symlinks. The path may not exist yet: the longest
/// existing ancestor is canonicalized and the remainder is appended.
pub(crate) fn resolve(path: &Path) -> Result<PathBuf, Problem> {
    resolve_from(path, std::env::current_dir())
}

fn resolve_from(path: &Path, cwd: std::io::Result<PathBuf>) -> Result<PathBuf, Problem> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.map_err(|e| {
            Problem::error(
                codes::CWD_UNAVAILABLE,
                format!("cannot find the current folder: {e}"),
            )
            .with_help("run Brrrty from a folder that exists, or pass an absolute root")
        })?
        .join(path)
    };
    let abs = normalize(&abs);
    let mut existing = abs.clone();
    let mut rest = Vec::new();
    loop {
        if let Ok(c) = existing.canonicalize() {
            let mut out = c;
            out.extend(rest.iter().rev());
            return Ok(out);
        }
        match existing.file_name().map(|n| n.to_os_string()) {
            Some(name) if existing.pop() => rest.push(name),
            _ => return Ok(abs),
        }
    }
}

/// `path` relative to `base`, both absolute and normalized, using `..` where `path` is not
/// under `base`.
fn lexical_relative(base: &Path, path: &Path) -> PathBuf {
    let b: Vec<_> = base.components().collect();
    let p: Vec<_> = path.components().collect();
    let common = b.iter().zip(&p).take_while(|(x, y)| x == y).count();
    let mut out = PathBuf::new();
    for _ in common..b.len() {
        out.push("..");
    }
    for c in &p[common..] {
        out.push(c.as_os_str());
    }
    out
}

/// `path` relative to the project `root`, for reporting. A path outside the root is kept as given.
pub(crate) fn root_relative(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

impl BuildOptions {
    pub fn root_dir(&self) -> Result<PathBuf, Problem> {
        resolve(&self.root)
    }

    pub fn input_dir(&self) -> Result<PathBuf, Problem> {
        resolve(&self.root_dir()?.join(&self.input))
    }

    /// The input folder relative to the root, spelled the way the user wrote it: symlinks are
    /// not resolved, so `src -> ../real` stays `src`, and `--input ../x` stays `../x`. 11ty
    /// matches ignore lines and finds path dates against this spelling, not the real path.
    pub fn input_rel(&self) -> Result<PathBuf, Problem> {
        let given_root = normalize(&std::env::current_dir().unwrap_or_default().join(&self.root));
        let input = normalize(&given_root.join(&self.input));
        if let Ok(r) = input.strip_prefix(&given_root) {
            return Ok(r.to_path_buf());
        }
        // An absolute input may name the root by its real path.
        if self.input.is_absolute()
            && let Ok(r) = self.input_dir()?.strip_prefix(self.root_dir()?)
        {
            return Ok(r.to_path_buf());
        }
        Ok(lexical_relative(&given_root, &input))
    }

    pub fn output_dir(&self) -> Result<PathBuf, Problem> {
        resolve(&self.root_dir()?.join(&self.output))
    }

    /// Where files are written: [`BuildOptions::write_to`] when set, otherwise the output
    /// folder.
    pub fn write_dir(&self) -> Result<PathBuf, Problem> {
        match &self.write_to {
            Some(w) => resolve(&self.root_dir()?.join(w)),
            None => self.output_dir(),
        }
    }

    /// Refuse a missing input folder, and an output folder that is the root, the input dir,
    /// or an ancestor of the input dir.
    pub fn validate(&self) -> Result<(), Problem> {
        let out = self.output_dir()?;
        let input = self.input_dir()?;
        if !input.exists() {
            return Err(
                Problem::error(codes::INPUT_MISSING, "the input folder doesn't exist")
                    .with_path(&self.input)
                    .with_help("check the --input folder name, relative to where you ran Brrrty"),
            );
        }
        if !input.is_dir() {
            return Err(Problem::error(
                codes::INPUT_NOT_A_DIRECTORY,
                "the input path is a file, not a folder",
            )
            .with_path(&self.input)
            .with_help("pass the folder that holds your source files, like --input src"));
        }
        if out == self.root_dir()? || input.starts_with(&out) {
            return Err(Problem::error(
                codes::OUTPUT_OVERLAPS_SOURCE,
                format!(
                    "the output folder {} would overlap the source folder {}",
                    out.display(),
                    input.display()
                ),
            )
            .with_help(
                "choose an output folder that is separate from your source files, such as _site",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_makes_dot_absolute_and_nonempty() {
        let r = resolve(Path::new(".")).unwrap();
        assert!(r.is_absolute());
        assert_eq!(r, std::env::current_dir().unwrap().canonicalize().unwrap());
    }

    #[test]
    fn resolve_handles_missing_tail() {
        let r = resolve(Path::new("./nope/../_site")).unwrap();
        assert_eq!(r, resolve(Path::new(".")).unwrap().join("_site"));
    }

    #[test]
    fn input_rel_keeps_the_spelling() {
        let o = |input: &str| BuildOptions {
            root: PathBuf::from("."),
            input: PathBuf::from(input),
            output: PathBuf::from("_site"),
            passthrough: vec![],
            strict: false,
            config: ConfigOptions::default(),
            ..BuildOptions::default()
        };
        assert_eq!(o("./src/").input_rel().unwrap(), PathBuf::from("src"));
        assert_eq!(o(".").input_rel().unwrap(), PathBuf::new());
        assert_eq!(o("../x/./y").input_rel().unwrap(), PathBuf::from("../x/y"));
        assert_eq!(o("a/../../x").input_rel().unwrap(), PathBuf::from("../x"));
        assert_eq!(
            lexical_relative(Path::new("/a/b"), Path::new("/a/c/d")),
            PathBuf::from("../c/d")
        );
    }

    #[test]
    fn unavailable_cwd_is_an_error_for_relative_paths_only() {
        let broken = || Err(std::io::Error::other("gone"));
        let p = resolve_from(Path::new("src"), broken()).unwrap_err();
        assert_eq!(p.code, codes::CWD_UNAVAILABLE);
        assert!(resolve_from(&std::env::temp_dir(), broken()).is_ok());
    }
}
