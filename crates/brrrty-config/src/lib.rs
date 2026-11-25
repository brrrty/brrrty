//! Reads `brrrty.config.rhai`, Brrrty's config file.
//!
//! An 11ty site is configured in JavaScript (`eleventy.config.js`), which
//! Brrrty can't run. `brrrty.config.rhai` is the same config written in
//! [Rhai](https://rhai.rs): a script that calls methods on a `config`
//! object shaped like `eleventyConfig`, in snake_case, and whose last
//! expression is the object 11ty's config function returns:
//!
//! ```rhai
//! config.add_passthrough_copy("src/images/");
//! config.add_passthrough_copy(#{ "src/favicon.svg": "favicon.svg" });
//! config.ignores.add("src/drafts/**");
//! config.add_global_data("build", #{ year: year(now()) });
//!
//! #{ dir: #{ input: "src" } }
//! ```
//!
//! ## Filters
//!
//! `config.add_filter("name", |value, arg| ...)` adds a filter, the way
//! `addFilter` does. It gets the value, then the arguments as the template
//! wrote them (a keyword argument arrives in place as `[name, value]`); a
//! missing argument is `()`, so `n ?? 40` is a default. A filter that
//! fails refuses its page, with the template's position and the script's.
//! `config.add_random_filter("shuffle", |arr| ...)` adds a filter that may
//! call `random()` where its JavaScript calls `Math.random`: 11ty's page
//! differs on every build then, so the page builds, with a note.
//! `config.add_nondeterministic_filter("shuffle")` stands in for such a
//! filter without porting it: pages that use it are refused.
//!
//! ## Collections
//!
//! `config.add_collection("name", |api| ...)` adds a collection, the way
//! `addCollection` does, computed where 11ty computes it (after the tag
//! collections, before `all`, and before any template that paginates or
//! imports it). `api` has `get_all()`, `get_all_sorted()`,
//! `get_filtered_by_tag(tag)`, `get_filtered_by_tags(a, b, ...)` and
//! `get_filtered_by_glob(glob)` (a glob or an array of them, read from the
//! project root like `./src/posts/**/*.md`). They give the items
//! themselves: read-only, so returning one, or an array or `object()`
//! holding them, gives back that same page, its content included. A
//! collection that fails, reads an item's content (11ty hasn't rendered it
//! yet) or changes an item refuses only the pages that read it. See the
//! `collections` and `glob` modules.
//!
//! Arrays get JavaScript's `reverse()` (it returns the array),
//! `slice(start, end)`, `sort()` (by `String(item)`) and `sort(|a, b| ..)`
//! (any number or bool result; a comparator that fails is an error), in
//! place of Rhai's own `sort`, which skips a failing comparator.
//!
//! Scripts also get `regex_replace(s, pattern, replacement, flags)`,
//! `regex_split(s, pattern)` and `regex_test(s, pattern)` with JavaScript
//! pattern semantics, on the `regex` crate: no lookaround and no
//! backreferences (see the `regex` module for the other differences), and
//! `url_hostname(s)`, `date(ms)`, `to_locale_date_string(d)`.
//!
//! Scripts read files in the project with `read_text`, `read_json`,
//! `read_dir`, `file_exists` and `parse_csv` (csv-parse 7.0.3), confined to
//! the project folder; see the `files` and `csv` modules.
//!
//! ## Plugins
//!
//! `config.add_plugin("name", #{ ... })` adds a plugin Brrrty has its own
//! version of. `rss` is `@11ty/eleventy-plugin-rss`'s `feedPlugin`, with
//! its options in snake_case (`output_path`, `collection`, `metadata`):
//! each call adds a feed. `syntaxhighlight` is
//! `@11ty/eleventy-plugin-syntaxhighlight`, with its options in snake_case
//! (`pre_attributes`, `line_separator`): it highlights Markdown fences and
//! the `{% highlight %}` tag with Prism. Any other name is an error. Brrrty can't see a plugin
//! the 11ty config adds and the port leaves out (see `PORTING.md`).
//!
//! ## Values
//!
//! Every JavaScript number arrives as a float, so `n / 2` is `3.5` for
//! `7` and prints as JavaScript prints it (`5`, not `5.0`); a literal like
//! `7` written in the script is still a Rhai integer, so write `7.0` to
//! divide as JavaScript does. `undefined` is `()`. `null` is
//! `null_value()`: it equals `()` and is falsy, and it comes back out as
//! `null`, an object key included. `truthy(v)` is JavaScript's `if (v)`.
//! Functions and closures can't capture variables (Rhai is built with
//! `no_closure`), every argument is copied for the call, and a curried
//! value is too, so nothing carries state from one filter call to the next;
//! pages render in parallel and 11ty's page-order state couldn't be
//! reproduced. A script that tries to capture a variable is a syntax error
//! at that line.
//!
//! Rhai's operators aren't changed. Where they differ from JavaScript's,
//! `js_string`, `js_concat`, `js_sort`, `js_sort_by`, `js_div`,
//! `js_round` and `json_stringify` do what JavaScript does; see
//! `PORTING.md`.
//!
//! ## Key order
//!
//! A JavaScript object keeps its keys in the order they were added (integer
//! keys first). Rhai's `#{ ... }` maps are sorted by key instead, so
//! **use `object()` wherever key order can show in output**:
//!
//! ```rhai
//! let o = object();   // o.z = 1; o["a"] = 2;  o.keys() is ["z", "a"]
//! #{ z: 1, a: 2 }    // a plain map: its keys are sorted, ["a", "z"]
//! ```
//!
//! `o.keys()`, `o.values()`, `o.entries()`, `o.len()`, `o.contains(k)` and
//! `o.remove(k)` work, `for k in o { ... }` walks the keys (like `for...in`;
//! Rhai's own `for (k, v) in` gives a counter, not a value), and
//! `object(map)` converts a map. Objects that come in from Brrrty, such as
//! page data, are `object()`s, so returning them keeps their order.
//!
//! A script runs with limits on its operations, call depth and data sizes,
//! so one that never ends or grows without bound is stopped with a problem
//! that points at the file.

// `Problem` is the crate-wide diagnostic type; boxing it would burden every caller.
#![allow(clippy::result_large_err)]

mod api;
mod builtins;
mod callbacks;
mod collections;
mod convert;
// Public for `tests/csv_probes.rs`, which checks it against csv-parse.
#[doc(hidden)]
pub mod csv;
mod data;
pub mod eleventy_cli;
mod eleventy_js;
mod files;
mod filters;
mod git;
mod glob;
mod highlight;
mod jshelpers;
mod markdown_rules;
pub mod markers;
mod object;
mod plugins;
pub mod rss_version;
use brrrty_jsregex::matcher as regex;
// Public for `tests/sort_probes.rs`, which checks it against V8 call by call.
#[doc(hidden)]
pub mod timsort;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use api::{
    MAX_ARRAY_SIZE, MAX_CALL_LEVELS, MAX_MAP_SIZE, MAX_OPERATIONS, MAX_SECONDS, MAX_STRING_SIZE,
    normalize_template_formats, utc_string,
};
pub use brrrty_core::PassthroughEntry;
use brrrty_core::frontmatter::Data;
use brrrty_core::{ConfigOptions, Problem, codes};
pub use convert::{date_value, deep_copy, to_dynamic, to_value};
pub use data::data_runner;
use rhai::{Dynamic, EvalAltResult, Map, ParseError, Position};

/// The config file's name, at the project root.
pub const CONFIG_FILE: &str = "brrrty.config.rhai";

/// The stack for running a script. Rhai recurses on Rust's stack, and a
/// debug build's frames are large, so 64 nested calls need room.
const STACK_SIZE: usize = 256 * 1024 * 1024;

/// The folders the script returned in `dir`, as written. `None` is 11ty's
/// default, or the command line's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirOptions {
    pub input: Option<String>,
    pub output: Option<String>,
    pub includes: Option<String>,
    pub data: Option<String>,
    pub layouts: Option<String>,
}

/// Everything a config script set.
#[derive(Debug, Clone)]
pub struct SiteConfig {
    /// The config file, relative to the root.
    pub file: PathBuf,
    pub dir: DirOptions,
    /// `markdownTemplateEngine` and `htmlTemplateEngine`, as returned.
    pub template_engines: brrrty_core::TemplateEngines,
    /// The template formats to build: `templateFormats` as returned, or
    /// `set_template_formats`, plus `add_template_formats`; `None` builds
    /// 11ty's defaults.
    pub template_formats: Option<Vec<String>>,
    /// `template_formats` was set (by `set_template_formats` or a returned
    /// `templateFormats`), not just added to.
    pub template_formats_set: bool,
    /// `add_template_formats` names, in order.
    pub added_template_formats: Vec<String>,
    /// `add_passthrough_copy` entries, in 11ty's order: one per source as
    /// written, the last target winning.
    pub passthrough: Vec<PassthroughEntry>,
    /// `ignores.add` globs, in order, each once.
    pub ignores: Vec<String>,
    /// `add_global_data` values by name as given.
    pub global_data: Data,
    /// Warnings about what the script set.
    pub problems: Vec<Problem>,
    /// `add_filter` filters, in order; a later one replaces an earlier one
    /// of the same name.
    pub filters: Vec<brrrty_core::ConfigFilter>,
    /// `add_collection` collections, in order.
    pub collections: Vec<brrrty_core::ConfigCollection>,
    /// `add_plugin("syntaxhighlight", ...)`.
    pub syntax_highlight: Option<brrrty_core::plugins::syntaxhighlight::SyntaxHighlight>,
    /// `add_plugin("rss", ...)` feeds, in order.
    pub feeds: Vec<brrrty_core::plugins::rss::FeedOptions>,
    /// `add_plugin("rss", #{ feed: false })`: the RSS plugin's filters without a feed.
    pub rss_filters: bool,
    /// `add_layout_alias` aliases: a layout name and the file it stands for.
    pub layout_aliases: indexmap::IndexMap<String, String>,
    /// Renderer rules `amend_library("md", ...)` set on 11ty's library.
    pub markdown_rules: Option<brrrty_core::MarkdownRules>,
    /// What the script read from the project, and its filters and collections read later.
    pub reads: brrrty_core::FileReads,
    /// Warnings its filters and collections raise while pages render.
    pub late_problems: brrrty_core::LateProblems,
    /// The error that refuses every page while `brrrty init`'s TODO markers
    /// are left in the file (see [`markers`]).
    pub unported: Option<Problem>,
    /// `config.set_js_data(...)`: the last call's mode and line.
    pub js_data: Option<(brrrty_core::JsData, Option<usize>)>,
    /// `config.set_js_config(...)`: the last call's mode and line.
    pub js_config: Option<(brrrty_core::JsData, Option<usize>)>,
    /// `config.set_order_check(...)`, when the config calls it.
    pub order_check: Option<bool>,
}

impl SiteConfig {
    /// The parts of the config a build reads, besides the folders.
    pub fn build_options(&self) -> ConfigOptions {
        ConfigOptions {
            file: Some(self.file.clone()),
            passthrough: self.passthrough.clone(),
            ignores: self.ignores.clone(),
            global_data: self.global_data.clone(),
            problems: self.problems.clone(),
            filters: self.filters.clone(),
            collections: self.collections.clone(),
            data: None,
            syntax_highlight: self.syntax_highlight.clone(),
            markdown_rules: self.markdown_rules.clone(),
            feeds: self.feeds.clone(),
            rss_filters: self.rss_filters,
            includes: self.dir.includes.clone(),
            layouts: self.dir.layouts.clone(),
            data_folder: self.dir.data.clone(),
            layout_aliases: self.layout_aliases.clone(),
            template_engines: self.template_engines,
            template_formats: self.template_formats.clone(),
            template_formats_set: self.template_formats_set,
            added_template_formats: self.added_template_formats.clone(),
            reads: self.reads.clone(),
            late_problems: self.late_problems.clone(),
            unported: self.unported.clone(),
            js_data: self.js_data,
            js_config: self.js_config,
            js_filters: Vec::new(),
            js_shortcodes: Vec::new(),
            js_liquid_shortcodes: Vec::new(),
            js_started: None,
            order_check: self.order_check,
        }
    }
}

/// Where `now()` gets the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    System,
    /// Milliseconds since 1970, from `SOURCE_DATE_EPOCH`.
    Fixed(i64),
}

impl Clock {
    /// The clock `SOURCE_DATE_EPOCH` asks for.
    pub fn from_env() -> Result<Clock, Problem> {
        match brrrty_js::clock::source_date_epoch() {
            Ok(Some(ms)) => Ok(Clock::Fixed(ms)),
            Ok(None) => Ok(Clock::System),
            Err(message) => Err(epoch_problem(message)),
        }
    }

    /// The clock for a `SOURCE_DATE_EPOCH` value, `None` when it is unset.
    pub fn parse(epoch: Option<&str>) -> Result<Clock, Problem> {
        match epoch {
            None => Ok(Clock::System),
            Some(text) => brrrty_js::clock::parse_epoch(text)
                .map(Clock::Fixed)
                .map_err(epoch_problem),
        }
    }

    pub fn now_ms(self) -> f64 {
        match self {
            Clock::System => brrrty_js::clock::system_now_ms(),
            Clock::Fixed(ms) => ms as f64,
        }
    }
}

fn epoch_problem(message: String) -> Problem {
    Problem::error(codes::SOURCE_DATE_EPOCH_INVALID, message)
        .with_help("set it to whole seconds since 1970, or unset it to use the clock")
}

/// Read `<root>/brrrty.config.rhai`, with the clock `SOURCE_DATE_EPOCH`
/// asks for. `Ok(None)` when there is no config file.
pub fn load(root: &Path) -> Result<Option<SiteConfig>, Vec<Problem>> {
    let clock = Clock::from_env().map_err(|p| vec![p])?;
    brrrty_js::tz::check_local().map_err(|m| vec![brrrty_core::tz_problem(m)])?;
    load_with(root, clock)
}

/// [`load`] with the given clock.
pub fn load_with(root: &Path, clock: Clock) -> Result<Option<SiteConfig>, Vec<Problem>> {
    let path = root.join(CONFIG_FILE);
    let unreadable = |why: String| {
        vec![
            Problem::error(
                codes::CONFIG_UNREADABLE,
                format!("cannot read {CONFIG_FILE}: {why}"),
            )
            .with_path(CONFIG_FILE),
        ]
    };
    let src = match std::fs::read(&path) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| unreadable("it isn't UTF-8".into()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unreadable(e.to_string())),
    };
    let unported = markers::unported_problem(&src);
    let stale: Vec<Problem> = markers::stale_problem(root, &src)
        .or_else(|| markers::unverified_problem(root, &src))
        .into_iter()
        .chain(markers::flags_problem(root, &src))
        .chain(markers::modules_problems(root, &src))
        .collect();
    let root = root.to_path_buf();
    let run = std::thread::Builder::new()
        .name("brrrty-config".into())
        .stack_size(STACK_SIZE)
        .spawn(move || brrrty_core::contain::catch(|| evaluate(&src, clock, &root)));
    let crashed = |why: String| {
        Err(vec![
            Problem::error(
                codes::CONFIG_ERROR,
                format!("Brrrty crashed while running {CONFIG_FILE}: {why}"),
            )
            .with_help(BUG_HELP),
        ])
    };
    let result = match run {
        Ok(handle) => match handle.join() {
            Ok(Ok(r)) => r,
            Ok(Err(why)) => crashed(why),
            Err(_) => crashed("the thread running it stopped".into()),
        },
        Err(e) => Err(vec![Problem::error(
            codes::BUILD_THREADS_FAILED,
            format!("cannot start a thread to run {CONFIG_FILE}: {e}"),
        )]),
    };
    result
        .map(|mut c| {
            c.unported = unported;
            c.problems.extend(stale);
            Some(c)
        })
        .map_err(|ps| ps.into_iter().map(|p| p.with_path(CONFIG_FILE)).collect())
}

/// Help for a problem that comes from a panic in Brrrty while running a script.
pub(crate) const BUG_HELP: &str = "this is a bug in Brrrty, not in your script. Please report it at https://github.com/brrrty/brrrty/issues";

/// Run a script and read what it set. Problems have no path yet.
fn evaluate(src: &str, clock: Clock, root: &Path) -> Result<SiteConfig, Vec<Problem>> {
    api::start_clock();
    let reads = brrrty_core::FileReads::default();
    let engine = api::engine(clock, root, reads.clone());
    let (config, state) = api::Config::new();
    let mut scope = rhai::Scope::new();
    scope.push("config", config);
    // With strict variables, a variable that isn't in scope is a syntax
    // error at its line. That is what a closure that tries to capture one
    // is (see PORTING.md).
    let ast = engine
        .compile_with_scope(&scope, src)
        .map_err(|e| vec![syntax_problem(&e)])?;
    // Renderer rules can be set while the script runs, never once pages render.
    let returned: Dynamic =
        markdown_rules::loading(|| engine.eval_ast_with_scope(&mut scope, &ast))
            .map_err(|e| vec![runtime_problem(*e)])?;
    // Drop the scope before taking the state: nothing else holds it then.
    drop(scope);
    let late_problems = brrrty_core::LateProblems::default();
    let state = {
        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        let taken = std::mem::take(&mut *guard);
        // A `config` call from here on (a global data function, a filter, a collection)
        // changes a copy, as 11ty ignores it, and is reported.
        *guard = api::State {
            closed: Some(late_problems.clone()),
            ..taken.clone()
        };
        taken
    };
    let global_data = api::resolve_global_data(&engine, &ast, state.global_data)
        .map_err(|e| vec![runtime_problem(*e)])?;
    let mut problems = Vec::new();
    let (dir, template_engines, returned_formats) = read_returned(returned, &mut problems)?;
    // 11ty: the `setTemplateFormats` call wins over the returned `templateFormats`;
    // `addTemplateFormats` adds to either.
    let template_formats_set = state.template_formats.is_some() || returned_formats.is_some();
    let added_template_formats = state.added_template_formats.clone();
    let template_formats = match state.template_formats.or(returned_formats) {
        Some(f) => Some(f),
        // Added to the defaults.
        None if !state.added_template_formats.is_empty() => Some(
            ["liquid", "md", "njk", "html", "11ty.js"]
                .map(String::from)
                .to_vec(),
        ),
        None => None,
    }
    .map(|mut f| {
        for a in &state.added_template_formats {
            if !f.contains(a) {
                f.push(a.clone());
            }
        }
        f
    });
    // A feed's template can't live where 11ty keeps includes and layouts.
    let includes = dir.includes.as_deref().unwrap_or("_includes");
    let layouts = dir.layouts.as_deref().unwrap_or(includes);
    for feed in &state.feeds {
        let path = Path::new(&feed.input_path);
        if path.starts_with(includes) || path.starts_with(layouts) {
            return Err(vec![Problem::error(
                codes::CONFIG_INVALID,
                format!(
                    "add_plugin(\"rss\", ...): `input_path` is `{}`; Brrrty supports a `.njk` path inside the input folder, outside the includes, layouts and `_data` folders",
                    feed.input_path
                ),
            )]);
        }
    }
    // Brrrty's `rss` plugin is version 3's.
    if state.rss_filters || !state.feeds.is_empty() {
        match rss_version::check(root, src) {
            Ok(warning) => problems.extend(warning),
            Err(p) => return Err(vec![p]),
        }
    }
    // The script's filters run later, while pages render.
    let runtime = Arc::new(filters::Runtime { engine, ast });
    let filters = state
        .filters
        .into_iter()
        .map(|(name, def)| brrrty_core::ConfigFilter {
            run: filters::make(runtime.clone(), name.clone(), def),
            name,
        })
        .collect();
    let collections = state
        .collections
        .into_iter()
        .map(|(name, (f, at))| brrrty_core::ConfigCollection {
            run: collections::make(runtime.clone(), name.clone(), f, at),
            name,
        })
        .collect();
    let markdown_rules = state
        .markdown_lib
        .map(|md| md.rules())
        .filter(|rules| !rules.is_empty())
        .map(|rules| {
            brrrty_core::MarkdownRules(Arc::new(markdown_rules::ConfigRules {
                runtime: runtime.clone(),
                rules,
            }))
        });
    Ok(SiteConfig {
        file: PathBuf::from(CONFIG_FILE),
        dir,
        template_engines,
        template_formats,
        template_formats_set,
        added_template_formats,
        passthrough: state
            .passthrough
            .into_iter()
            .map(|(from, to)| PassthroughEntry { from, to })
            .collect(),
        ignores: state.ignores.into_iter().collect(),
        global_data,
        problems,
        filters,
        collections,
        syntax_highlight: state.syntax_highlight,
        markdown_rules,
        feeds: state.feeds,
        rss_filters: state.rss_filters,
        layout_aliases: state.layout_aliases,
        reads,
        late_problems,
        unported: None,
        js_data: state.js_data,
        js_config: state.js_config,
        order_check: state.order_check,
    })
}

pub(crate) fn at(p: Problem, pos: Position) -> Problem {
    match (pos.line(), pos.position()) {
        (Some(line), Some(col)) => p.at(line, col),
        (Some(line), None) => p.at(line, 1),
        _ => p,
    }
}

fn syntax_problem(e: &ParseError) -> Problem {
    let help = if matches!(*e.0, rhai::ParseErrorType::VariableUndefined(_)) {
        "a function can't see variables outside it, and a closure `|x| ...` can't capture them. Pass the value in as an argument, or curry it into a named function (`Fn(\"name\").curry(value)`); see PORTING.md"
    } else {
        "fix the script; Rhai's syntax is close to JavaScript's, see https://rhai.rs/book/"
    };
    at(
        Problem::error(codes::CONFIG_SYNTAX, e.0.to_string()).with_help(help),
        e.1,
    )
}

/// A runtime error taken apart: the innermost error's text and position,
/// and the innermost named function it happened in.
pub(crate) struct ErrorParts {
    pub message: String,
    pub position: Position,
    pub code: &'static str,
    pub limit: bool,
}

pub(crate) fn error_parts(e: EvalAltResult) -> ErrorParts {
    let outer = e.position();
    let mut inner = e;
    // The innermost function the script named: a closure's internal name
    // (`anon$...`) says nothing the position doesn't.
    let mut function = None;
    while let EvalAltResult::ErrorInFunctionCall(name, _, err, _) = inner {
        if !name.starts_with("anon$") {
            function = Some(name);
        }
        inner = *err;
    }
    let position = if inner.position().is_none() {
        outer
    } else {
        inner.position()
    };
    let limit = matches!(
        inner,
        EvalAltResult::ErrorTooManyOperations(..)
            | EvalAltResult::ErrorStackOverflow(..)
            | EvalAltResult::ErrorDataTooLarge(..)
            | EvalAltResult::ErrorTooManyModules(..)
            | EvalAltResult::ErrorTerminated(..)
    );
    inner.clear_position();
    let mut code = if limit {
        codes::CONFIG_LIMIT
    } else {
        codes::CONFIG_ERROR
    };
    let mut message = match &inner {
        EvalAltResult::ErrorRuntime(token, _) if token.is::<api::InvalidValue>() => {
            code = codes::CONFIG_INVALID;
            token.clone_cast::<api::InvalidValue>().0
        }
        EvalAltResult::ErrorRuntime(token, _) if token.is::<api::InvalidJsData>() => {
            code = codes::CONFIG_JS_DATA_INVALID;
            token.clone_cast::<api::InvalidJsData>().0
        }
        EvalAltResult::ErrorTerminated(token, _) => token.to_string(),
        other => other.to_string(),
    };
    if let Some(f) = function {
        message.push_str(&format!(" (in function `{f}`)"));
    }
    let message = readable(&message);
    ErrorParts {
        message,
        position,
        code,
        limit,
    }
}

/// Rhai's message, with names a script never wrote made readable: a
/// closure's internal name (`anon$1f2e...`) becomes "an anonymous
/// function", and a call that doesn't exist on an array read through an
/// item says how to copy it.
fn readable(message: &str) -> String {
    static ANON: std::sync::LazyLock<::regex::Regex> = std::sync::LazyLock::new(|| {
        ::regex::Regex::new(r"`?anon\$[0-9a-f]+`?").expect("valid pattern")
    });
    let mut out = ANON
        .replace_all(message, "an anonymous function")
        .into_owned();
    if out.contains("Function not found") && out.contains("frozen_array") {
        out.push_str(
            "; an array read through a collection item is read-only, so copy it first with `to_array()`",
        );
    }
    out
}

/// A runtime error as one problem.
fn runtime_problem(e: EvalAltResult) -> Problem {
    let parts = error_parts(e);
    let p = if parts.limit {
        Problem::error(parts.code, parts.message).with_help(format!(
            "a config script may run {MAX_OPERATIONS} operations for up to {MAX_SECONDS} seconds, nest {MAX_CALL_LEVELS} calls, and build strings up to {} MiB and up to {MAX_ARRAY_SIZE} array items or {MAX_MAP_SIZE} map keys; look for a loop or recursion that never stops",
            MAX_STRING_SIZE / 1024 / 1024
        ))
    } else {
        Problem::error(parts.code, parts.message)
    };
    at(p, parts.position)
}

/// The returned object: `()` or a map with `dir`.
/// A returned `markdownTemplateEngine` or `htmlTemplateEngine`: a
/// language's name, or `false` for none.
fn template_engine(
    key: &str,
    value: Dynamic,
) -> Result<Option<brrrty_core::TemplateLanguage>, Vec<Problem>> {
    if value.as_bool() == Ok(false) {
        return Ok(None);
    }
    let type_name = value.type_name();
    let Ok(name) = value.into_immutable_string() else {
        return Err(vec![Problem::error(
            codes::CONFIG_INVALID,
            format!(
                "`{key}` is a {type_name}; it should be a template language, like \"njk\", or false"
            ),
        )]);
    };
    match brrrty_core::TemplateLanguage::from_name(&name) {
        Some(l) => Ok(Some(l)),
        None => Err(vec![
            Problem::error(
                codes::CONFIG_UNSUPPORTED,
                format!("`{key}` is `{name}`, which Brrrty doesn't run there"),
            )
            .with_help("use \"liquid\", \"njk\" or false"),
        ]),
    }
}

/// What the returned object sets: the folders, the template engines and `templateFormats`.
type Returned = (
    DirOptions,
    brrrty_core::TemplateEngines,
    Option<Vec<String>>,
);

/// The returned object: `()` or a map with `dir` and the template engines.
fn read_returned(v: Dynamic, problems: &mut Vec<Problem>) -> Result<Returned, Vec<Problem>> {
    let invalid = |m: String| vec![Problem::error(codes::CONFIG_INVALID, m)];
    let mut engines = brrrty_core::TemplateEngines::default();
    if v.is_unit() {
        return Ok((DirOptions::default(), engines, None));
    }
    let type_name = v.type_name();
    let Some(map) = v.try_cast::<Map>() else {
        return Err(invalid(format!(
            "{CONFIG_FILE} ends with a {type_name}; it should end with the object 11ty's config returns, like #{{ dir: #{{ input: \"src\" }} }}, or with a statement"
        )));
    };
    let mut dir = DirOptions::default();
    let mut formats = None;
    for (key, value) in map {
        match key.as_str() {
            "templateFormats" => {
                formats = api::template_formats("`templateFormats`", value)
                    .map_err(|m| vec![Problem::error(codes::CONFIG_INVALID, m)])?;
                continue;
            }
            "markdownTemplateEngine" => {
                engines.markdown = template_engine(&key, value)?;
                continue;
            }
            "htmlTemplateEngine" => {
                engines.html = template_engine(&key, value)?;
                continue;
            }
            // 11ty 3 no longer reads it (probed: data files are not
            // rendered).
            "dataTemplateEngine" => {
                problems.push(
                    Problem::warning(
                        codes::CONFIG_UNSUPPORTED,
                        "`dataTemplateEngine` was ignored: 11ty 3 doesn't read it",
                    )
                    .with_path(CONFIG_FILE),
                );
                continue;
            }
            _ => {}
        }
        if key != "dir" {
            return Err(vec![
                Problem::error(
                    codes::CONFIG_UNSUPPORTED,
                    format!("{CONFIG_FILE} returns `{key}`, which Brrrty doesn't support yet"),
                )
                .with_help("remove it, or build with 11ty until Brrrty supports it"),
            ]);
        }
        let type_name = value.type_name();
        let Some(folders) = value.try_cast::<Map>() else {
            return Err(invalid(format!(
                "`dir` is a {type_name}; it should be a map of folders, like #{{ input: \"src\" }}"
            )));
        };
        for (name, folder) in folders {
            let type_name = folder.type_name();
            let Ok(folder) = folder.into_immutable_string() else {
                return Err(invalid(format!(
                    "`dir.{name}` is a {type_name}; it should be a folder name"
                )));
            };
            let folder = Some(folder.to_string());
            match name.as_str() {
                "input" => dir.input = folder,
                "output" => dir.output = folder,
                "includes" => dir.includes = folder,
                "data" => dir.data = folder,
                "layouts" => dir.layouts = folder,
                _ => problems.push(
                    Problem::warning(
                        codes::CONFIG_UNSUPPORTED,
                        format!("`dir.{name}` is not a folder 11ty reads, so it was ignored"),
                    )
                    .with_path(CONFIG_FILE),
                ),
            }
        }
    }
    // Includes, layouts and data: a folder inside the input folder.
    for (name, value) in [
        ("data", &mut dir.data),
        ("includes", &mut dir.includes),
        ("layouts", &mut dir.layouts),
    ] {
        if let Some(v) = value.as_mut() {
            match inside_input(v) {
                Some(clean) => *v = clean,
                None => {
                    return Err(vec![
                        Problem::error(
                            codes::CONFIG_UNSUPPORTED,
                            format!(
                                "`dir.{name}` is `{v}`, and Brrrty only reads a folder inside the input folder for it so far"
                            ),
                        )
                        .with_help(format!(
                            "give `dir.{name}` a folder inside the input folder, like `_includes`, relative to it"
                        )),
                    ]);
                }
            }
        }
    }
    Ok((dir, engines, formats))
}

/// `folder`, relative to the input folder, written plainly (`./parts/` is
/// `parts`); `None` when it isn't a folder inside the input folder.
fn inside_input(folder: &str) -> Option<String> {
    if folder.starts_with('/') || folder.contains('\\') {
        return None;
    }
    let parts: Vec<&str> = folder
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.is_empty() || parts.contains(&"..") {
        return None;
    }
    Some(parts.join("/"))
}
