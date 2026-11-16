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
//! `config.add_nondeterministic_filter("shuffle")` stands in for a filter
//! that uses `Math.random`: its output can never match 11ty's, so pages
//! that use it are refused.
//!
//! Scripts also get `regex_replace(s, pattern, replacement, flags)`,
//! `regex_split(s, pattern)` and `regex_test(s, pattern)` with JavaScript
//! pattern semantics, on the `regex` crate: no lookaround and no
//! backreferences (see the `regex` module for the other differences), and
//! `url_hostname(s)`, `date(ms)`, `to_locale_date_string(d)`.
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
mod convert;
mod filters;
mod object;
mod regex;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use api::{
    MAX_ARRAY_SIZE, MAX_CALL_LEVELS, MAX_MAP_SIZE, MAX_OPERATIONS, MAX_SECONDS, MAX_STRING_SIZE,
    utc_string,
};
pub use brrrty_core::PassthroughEntry;
use brrrty_core::frontmatter::Data;
use brrrty_core::{ConfigOptions, Problem, codes};
pub use convert::{date_value, deep_copy, to_dynamic, to_value};
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
    let run = std::thread::Builder::new()
        .name("brrrty-config".into())
        .stack_size(STACK_SIZE)
        .spawn(move || evaluate(&src, clock));
    let result = match run {
        Ok(handle) => match handle.join() {
            Ok(r) => r,
            Err(panic) => std::panic::resume_unwind(panic),
        },
        Err(e) => Err(vec![Problem::error(
            codes::BUILD_THREADS_FAILED,
            format!("cannot start a thread to run {CONFIG_FILE}: {e}"),
        )]),
    };
    result
        .map(Some)
        .map_err(|ps| ps.into_iter().map(|p| p.with_path(CONFIG_FILE)).collect())
}

/// Run a script and read what it set. Problems have no path yet.
fn evaluate(src: &str, clock: Clock) -> Result<SiteConfig, Vec<Problem>> {
    api::start_clock();
    let engine = api::engine(clock);
    let ast = engine.compile(src).map_err(|e| vec![syntax_problem(&e)])?;
    let (config, state) = api::Config::new();
    let mut scope = rhai::Scope::new();
    scope.push("config", config);
    let returned: Dynamic = engine
        .eval_ast_with_scope(&mut scope, &ast)
        .map_err(|e| vec![runtime_problem(*e)])?;
    // Drop the scope before taking the state: nothing else holds it then.
    drop(scope);
    let state = std::mem::take(&mut *state.lock().unwrap_or_else(|e| e.into_inner()));
    let global_data = api::resolve_global_data(&engine, &ast, state.global_data)
        .map_err(|e| vec![runtime_problem(*e)])?;
    let mut problems = Vec::new();
    let dir = read_returned(returned, &mut problems)?;
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
    Ok(SiteConfig {
        file: PathBuf::from(CONFIG_FILE),
        dir,
        passthrough: state
            .passthrough
            .into_iter()
            .map(|(from, to)| PassthroughEntry { from, to })
            .collect(),
        ignores: state.ignores.into_iter().collect(),
        global_data,
        problems,
        filters,
    })
}

fn at(p: Problem, pos: Position) -> Problem {
    match (pos.line(), pos.position()) {
        (Some(line), Some(col)) => p.at(line, col),
        (Some(line), None) => p.at(line, 1),
        _ => p,
    }
}

fn syntax_problem(e: &ParseError) -> Problem {
    at(
        Problem::error(codes::CONFIG_SYNTAX, e.0.to_string()).with_help(
            "fix the script; Rhai's syntax is close to JavaScript's, see https://rhai.rs/book/",
        ),
        e.1,
    )
}

/// A runtime error taken apart: the innermost error's text and position,
/// and the function it happened in.
pub(crate) struct ErrorParts {
    pub message: String,
    pub position: Position,
    pub code: &'static str,
    pub limit: bool,
}

pub(crate) fn error_parts(e: EvalAltResult) -> ErrorParts {
    let outer = e.position();
    let mut inner = e;
    let mut function = None;
    while let EvalAltResult::ErrorInFunctionCall(name, _, err, _) = inner {
        function.get_or_insert(name);
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
        EvalAltResult::ErrorTerminated(token, _) => token.to_string(),
        other => other.to_string(),
    };
    if let Some(f) = function {
        message.push_str(&format!(" (in function `{f}`)"));
    }
    ErrorParts {
        message,
        position,
        code,
        limit,
    }
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
fn read_returned(v: Dynamic, problems: &mut Vec<Problem>) -> Result<DirOptions, Vec<Problem>> {
    let invalid = |m: String| vec![Problem::error(codes::CONFIG_INVALID, m)];
    if v.is_unit() {
        return Ok(DirOptions::default());
    }
    let type_name = v.type_name();
    let Some(map) = v.try_cast::<Map>() else {
        return Err(invalid(format!(
            "{CONFIG_FILE} ends with a {type_name}; it should end with the object 11ty's config returns, like #{{ dir: #{{ input: \"src\" }} }}, or with a statement"
        )));
    };
    let mut dir = DirOptions::default();
    for (key, value) in map {
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
    // Brrrty reads `_includes` and `_data` only.
    for (name, value, default) in [
        ("includes", &dir.includes, "_includes"),
        ("data", &dir.data, "_data"),
        ("layouts", &dir.layouts, "_includes"),
    ] {
        if let Some(v) = value
            && v.trim_start_matches("./").trim_end_matches('/') != default
        {
            return Err(vec![
                Problem::error(
                    codes::CONFIG_UNSUPPORTED,
                    format!(
                        "`dir.{name}` is `{v}`, and Brrrty only reads the default `{default}` folder so far"
                    ),
                )
                .with_help(format!("rename the folder to `{default}` inside the input folder")),
            ]);
        }
    }
    Ok(dir)
}
