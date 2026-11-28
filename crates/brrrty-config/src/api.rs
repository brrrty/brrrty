//! The Rhai engine and the `config` object a script sees.
//!
//! `config` mirrors 11ty's `eleventyConfig` in snake_case. Each call records
//! what it set in [`State`]; nothing runs until the whole script has.

use std::io::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use brrrty_core::frontmatter::Data;
use brrrty_js::{Tz, Value, tz};
use indexmap::{IndexMap, IndexSet};
use rhai::{Dynamic, Engine, EvalAltResult, FnPtr, INT, ImmutableString, Map, Position};

use crate::Clock;
use crate::convert::{deep_copy, to_value};
use crate::filters::FilterDef;
use crate::regex;

/// Most operations a config script may run: a few seconds of work. A loop
/// that never ends stops here.
pub const MAX_OPERATIONS: u64 = 50_000_000;
/// Most nested function calls.
pub const MAX_CALL_LEVELS: usize = 64;
/// Most nested expressions, at the top level and inside functions.
pub const MAX_EXPR_DEPTH: usize = 64;
pub const MAX_FUNCTION_EXPR_DEPTH: usize = 32;
/// Longest a script may run. Operations aren't equal (copying a large
/// value is one), so time is bounded too.
pub const MAX_SECONDS: u64 = 10;
/// Largest string, in bytes.
pub const MAX_STRING_SIZE: usize = 16 * 1024 * 1024;
/// Most items in all arrays nested in one value.
pub const MAX_ARRAY_SIZE: usize = 100_000;
/// Most keys in all maps nested in one value.
pub const MAX_MAP_SIZE: usize = 100_000;

thread_local! {
    /// When the script, or the filter call, running on this thread began.
    static RUN_START: std::cell::Cell<std::time::Instant> =
        std::cell::Cell::new(std::time::Instant::now());
}

/// Start the clock that stops a run after [`MAX_SECONDS`], and the count of what its objects
/// hold (see [`crate::object::MAX_OBJECTS_SIZE`]). Call it where a
/// run begins on this thread.
pub fn start_clock() {
    RUN_START.with(|s| s.set(std::time::Instant::now()));
    crate::object::reset_objects_size();
}

/// A JavaScript `Date`: milliseconds since 1970, `NaN` when invalid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JsDate(pub f64);

/// A function from JavaScript data, passed through a Rhai script as it is:
/// Rhai can't call it, and it is the same function when it comes back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JsFunction(pub brrrty_js::FnHandle);

/// A global data value, or the function 11ty calls for it.
#[derive(Debug, Clone)]
pub enum Pending {
    Value(Value),
    Fn(FnPtr, Position),
}

/// What the script set, in call order.
#[derive(Debug, Default, Clone)]
pub struct State {
    /// Passthrough targets by source as written; `None` copies in place.
    pub passthrough: IndexMap<String, Option<String>>,
    pub ignores: IndexSet<String>,
    pub global_data: IndexMap<String, Pending>,
    pub filters: IndexMap<String, FilterDef>,
    /// `add_collection` functions by name, with where each was added.
    pub collections: IndexMap<String, (FnPtr, Position)>,
    /// `add_plugin("syntaxhighlight", ...)`.
    pub syntax_highlight: Option<brrrty_core::plugins::syntaxhighlight::SyntaxHighlight>,
    /// `add_plugin("rss", ...)` feeds, in order.
    pub feeds: Vec<brrrty_core::plugins::rss::FeedOptions>,
    /// `add_plugin("rss", #{ feed: false })`: the plugin's filters, without a feed.
    pub rss_filters: bool,
    /// `add_plugin("htmlbase")`.
    pub html_base: bool,
    /// `add_plugin("idattribute")`.
    pub id_attribute: bool,
    /// `add_plugin("inputpathtourl")`.
    pub input_path_to_url: bool,
    /// `add_plugin("render")`.
    pub render_plugin: bool,
    /// `add_layout_alias(from, to)`, by alias; a later one replaces an earlier one.
    pub layout_aliases: IndexMap<String, String>,
    /// `set_template_formats(...)`: the last call's list.
    pub template_formats: Option<Vec<String>>,
    /// `add_template_formats(...)`: the formats added, in order.
    pub added_template_formats: Vec<String>,
    /// 11ty's markdown-it, once `amend_library("md", ...)` has been called.
    pub markdown_lib: Option<crate::markdown_rules::MdLib>,
    /// Where a `config` call made once the script has run (from a filter, a collection or a
    /// global data function) is reported. The state is then a copy of what the script set,
    /// which such a call changes to no effect, as 11ty's late calls do.
    pub closed: Option<brrrty_core::LateProblems>,
    /// `set_js_data(...)`: the last call's mode and line.
    pub js_data: Option<(brrrty_core::JsData, Option<usize>)>,
    /// `set_js_config(...)`: the last call's mode and line.
    pub js_config: Option<(brrrty_core::JsData, Option<usize>)>,
    /// `set_order_check(...)`: the last call's answer.
    pub order_check: Option<bool>,
}

/// The `config` object.
#[derive(Debug, Clone)]
pub struct Config(Arc<Mutex<State>>);

/// `config.ignores`, a set of globs like 11ty's.
#[derive(Debug, Clone)]
pub struct Ignores(Arc<Mutex<State>>);

fn lock(state: &Arc<Mutex<State>>) -> MutexGuard<'_, State> {
    // Nothing panics while holding the lock, but don't make a panic worse.
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// The state, for a `config` call named `call` at `pos`. Once the config has loaded, 11ty
/// ignores such a call (its engines and file lists are set up already): Brrrty lets it change
/// a copy, so it behaves as in 11ty and changes no output, and warns once per place.
fn open<'a>(state: &'a Arc<Mutex<State>>, call: &str, pos: Position) -> MutexGuard<'a, State> {
    let guard = lock(state);
    if let Some(late) = &guard.closed {
        let p = brrrty_core::Problem::warning(
            brrrty_core::codes::CONFIG_LATE_CALL,
            format!(
                "{call} was called while pages render, so it was ignored: config can only change while it loads (11ty ignores it too)"
            ),
        )
        .with_path(crate::CONFIG_FILE)
        .with_help("call it at the top level of brrrty.config.rhai instead");
        late.add(crate::at(p, pos));
    }
    guard
}

/// A value the script gave that Brrrty can't use, thrown as a Rhai error.
#[derive(Debug, Clone)]
pub struct InvalidValue(pub String);

/// A `set_js_data` value other than `"node"` and `"off"`.
#[derive(Debug, Clone)]
pub struct InvalidJsData(pub String);

fn runtime(message: String) -> Box<EvalAltResult> {
    invalid(message, Position::NONE)
}

pub fn invalid(message: String, pos: Position) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(Dynamic::from(InvalidValue(message)), pos).into()
}

impl Config {
    pub fn new() -> (Config, Arc<Mutex<State>>) {
        let state = Arc::new(Mutex::new(State::default()));
        (Config(state.clone()), state)
    }
}

/// `new Date(ms)`'s TimeClip.
fn time_clip(ms: f64) -> f64 {
    if !ms.is_finite() || ms.abs() > tz::MAX_TIME {
        f64::NAN
    } else {
        ms.trunc() + 0.0
    }
}

/// A number of milliseconds, or `NaN`, as Rhai sees it.
fn number(n: f64) -> Dynamic {
    Dynamic::from_float(n)
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `Date.prototype.toUTCString()`.
pub fn utc_string(ms: f64) -> String {
    let Some(f) = tz::fields(Tz::Utc, ms) else {
        return "Invalid Date".into();
    };
    let year = if f.year < 0 {
        format!("-{:04}", -f.year)
    } else {
        format!("{:04}", f.year)
    };
    format!(
        "{}, {:02} {} {year} {:02}:{:02}:{:02} GMT",
        DAYS[f.weekday as usize], f.day, MONTHS[f.month0 as usize], f.hour, f.minute, f.second
    )
}

/// An engine with Brrrty's limits and the config API registered, for a
/// project at `root`.
pub fn engine(clock: Clock, root: &Path, reads: brrrty_core::FileReads) -> Engine {
    engine_for(clock, crate::CONFIG_FILE.to_string(), root, reads)
}

/// [`engine`] for a script called `label` in `debug` output. What the script reads from the
/// project goes in `reads`.
pub fn engine_for(
    clock: Clock,
    label: String,
    root: &Path,
    reads: brrrty_core::FileReads,
) -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(MAX_OPERATIONS)
        .set_max_call_levels(MAX_CALL_LEVELS)
        .set_max_expr_depths(MAX_EXPR_DEPTH, MAX_FUNCTION_EXPR_DEPTH)
        .set_max_string_size(MAX_STRING_SIZE)
        .set_max_array_size(MAX_ARRAY_SIZE)
        .set_max_map_size(MAX_MAP_SIZE)
        // Rhai's string interner sits behind a lock that sleeps 10 ms
        // when another thread holds it, and filters run on several threads
        // at once, so it is off.
        .set_max_strings_interned(0)
        // Config scripts are one file: no imports, and no `eval`.
        .set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new())
        .set_strict_variables(true)
        // The optimizer folds `${...}` of constants into a string with Rhai's own float
        // formatting (`5.0`, `1e21`), past the `to_string` registered below, so a constant
        // would print differently from a variable holding the same number.
        .set_optimization_level(rhai::OptimizationLevel::None)
        .disable_symbol("eval");
    e.on_progress(move |ops| {
        (ops % 1024 == 0 && RUN_START.with(|s| s.get().elapsed().as_secs() >= MAX_SECONDS)).then(
            || {
                Dynamic::from(format!(
                    "the script ran for more than {MAX_SECONDS} seconds and was stopped"
                ))
            },
        )
    });
    // A closed stdout or stderr (`| head -1`) doesn't stop the script:
    // a failed write is ignored rather than panicking mid-render.
    e.on_print(|s| {
        let _ = writeln!(std::io::stdout(), "{s}");
    });
    e.on_debug(move |s, _src, pos| {
        let _ = writeln!(std::io::stderr(), "{label} {pos}: {s}");
    });

    // Debug builds only: a panic on demand, so tests can check that a panic inside a script
    // run becomes a Problem through the real binary, panic hook and all.
    #[cfg(debug_assertions)]
    e.register_fn("__brrrty_test_panic", || -> () {
        panic!("deliberate test panic from a script")
    });

    // `fs.readFileSync` and friends, inside the project.
    crate::files::register(&mut e, root, reads);
    // `execSync("git rev-parse HEAD")`, read from the repository's files.
    let root = root.to_path_buf();
    e.register_fn("git_head", move || -> Result<String, Box<EvalAltResult>> {
        crate::git::head(&root).map_err(|why| runtime(format!("git_head(): {why}")))
    });

    e.register_type_with_name::<Config>("Config")
        .register_type_with_name::<Ignores>("Ignores")
        .register_type_with_name::<JsDate>("Date")
        .register_type_with_name::<JsFunction>("Function");

    e.register_fn(
        "add_passthrough_copy",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         from: ImmutableString|
         -> Result<(), Box<EvalAltResult>> {
            open(&c.0, "config.add_passthrough_copy", ctx.call_position())
                .passthrough
                .insert(from.to_string(), None);
            Ok(())
        },
    );
    e.register_fn(
        "add_passthrough_copy",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         map: Map|
         -> Result<(), Box<EvalAltResult>> {
            let mut state = open(&c.0, "config.add_passthrough_copy", ctx.call_position());
            for (from, to) in map {
                let to = to.into_immutable_string().map_err(|t| {
                    runtime(format!(
                        "add_passthrough_copy maps `{from}` to a {t}; the target must be a string"
                    ))
                })?;
                state
                    .passthrough
                    .insert(from.to_string(), Some(to.to_string()));
            }
            Ok(())
        },
    );
    e.register_fn(
        "add_filter",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         name: ImmutableString,
         f: FnPtr|
         -> Result<(), Box<EvalAltResult>> {
            open(&c.0, "config.add_filter", ctx.call_position())
                .filters
                .insert(name.to_string(), FilterDef::Fn(f));
            Ok(())
        },
    );
    // Brrrty's own: a filter that calls `random()` where the JavaScript
    // one calls `Math.random`.
    e.register_fn(
        "add_random_filter",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         name: ImmutableString,
         f: FnPtr|
         -> Result<(), Box<EvalAltResult>> {
            open(&c.0, "config.add_random_filter", ctx.call_position())
                .filters
                .insert(name.to_string(), FilterDef::Random(f));
            Ok(())
        },
    );
    e.register_fn(
        "add_nondeterministic_filter",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         name: ImmutableString|
         -> Result<(), Box<EvalAltResult>> {
            open(
                &c.0,
                "config.add_nondeterministic_filter",
                ctx.call_position(),
            )
            .filters
            .insert(name.to_string(), FilterDef::Nondeterministic);
            Ok(())
        },
    );
    e.register_fn(
        "add_collection",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         name: ImmutableString,
         f: FnPtr|
         -> Result<(), Box<EvalAltResult>> {
            let pos = ctx.call_position();
            // 11ty's `addCollection` refuses a name twice; `all`, a
            // bracketed name and an empty one are 11ty's own (`all`) or
            // can't be read (`collections[""]` isn't a config collection).
            if name.is_empty() || name == "all" || (name.starts_with('[') && name.ends_with(']')) {
                return Err(invalid(
                    format!(
                        "add_collection(\"{name}\", ...): 11ty uses this name itself, or never reads it as a collection; pick another name"
                    ),
                    pos,
                ));
            }
            let mut state = open(&c.0, "config.add_collection", pos);
            if state.collections.contains_key(name.as_str()) {
                return Err(invalid(
                    format!(
                        "config.addCollection({name}) already exists. Try a different name for your collection (11ty's error)"
                    ),
                    pos,
                ));
            }
            state.collections.insert(name.to_string(), (f, pos));
            Ok(())
        },
    );
    // Brrrty's own: whether JavaScript data files run, in the site's Node.
    e.register_fn(
        "set_js_data",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         mode: ImmutableString|
         -> Result<(), Box<EvalAltResult>> {
            let pos = ctx.call_position();
            let Some(m) = brrrty_core::JsData::parse(mode.as_str()) else {
                return Err(EvalAltResult::ErrorRuntime(
                    Dynamic::from(InvalidJsData(format!(
                        "config.set_js_data({}): use \"node\" (run JavaScript data files in the site's own Node) or \"off\"",
                        serde_json::Value::String(mode.to_string())
                    ))),
                    pos,
                )
                .into());
            };
            open(&c.0, "config.set_js_data", pos).js_data = Some((m, pos.line()));
            Ok(())
        },
    );
    // Brrrty's own: whether the site's 11ty config runs in Node and what it
    // sets is applied to the build.
    e.register_fn(
        "set_js_config",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         mode: ImmutableString|
         -> Result<(), Box<EvalAltResult>> {
            let pos = ctx.call_position();
            let Some(m) = brrrty_core::JsData::parse(mode.as_str()) else {
                return Err(EvalAltResult::ErrorRuntime(
                    Dynamic::from(InvalidJsData(format!(
                        "config.set_js_config({}): use \"node\" (run the site's 11ty config in its own Node) or \"off\"",
                        serde_json::Value::String(mode.to_string())
                    ))),
                    pos,
                )
                .into());
            };
            open(&c.0, "config.set_js_config", pos).js_config = Some((m, pos.line()));
            Ok(())
        },
    );
    // Brrrty's own: whether the site's JavaScript is run again in other
    // orders before anything is written.
    e.register_fn(
        "set_order_check",
        |ctx: rhai::NativeCallContext, c: &mut Config, on: bool| {
            open(&c.0, "config.set_order_check", ctx.call_position()).order_check = Some(on);
        },
    );
    // 11ty's `addLayoutAlias`: `layout: from` names the layout file `to`,
    // relative to the layouts folder.
    e.register_fn(
        "add_layout_alias",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         from: ImmutableString,
         to: ImmutableString|
         -> Result<(), Box<EvalAltResult>> {
            let mut state = open(&c.0, "config.add_layout_alias", ctx.call_position());
            state
                .layout_aliases
                .insert(from.to_string(), to.to_string());
            Ok(())
        },
    );
    // 11ty's `setTemplateFormats` and `addTemplateFormats`.
    e.register_fn(
        "set_template_formats",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         list: Dynamic|
         -> Result<(), Box<EvalAltResult>> {
            let formats = template_formats("set_template_formats", list).map_err(|m| {
                Box::new(EvalAltResult::ErrorRuntime(m.into(), ctx.call_position()))
            })?;
            // An empty string leaves the setting unset, as in 11ty.
            open(&c.0, "config.set_template_formats", ctx.call_position()).template_formats =
                formats;
            Ok(())
        },
    );
    e.register_fn(
        "add_template_formats",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         list: Dynamic|
         -> Result<(), Box<EvalAltResult>> {
            let formats = template_formats("add_template_formats", list).map_err(|m| {
                Box::new(EvalAltResult::ErrorRuntime(m.into(), ctx.call_position()))
            })?;
            open(&c.0, "config.add_template_formats", ctx.call_position())
                .added_template_formats
                .extend(formats.unwrap_or_default());
            Ok(())
        },
    );
    e.register_fn(
        "add_plugin",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         name: ImmutableString|
         -> Result<(), Box<EvalAltResult>> {
            add_plugin(
                &mut open(&c.0, "config.add_plugin", ctx.call_position()),
                &name,
                None,
                ctx.call_position(),
            )
        },
    );
    e.register_fn(
        "add_plugin",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         name: ImmutableString,
         options: Map|
         -> Result<(), Box<EvalAltResult>> {
            add_plugin(
                &mut open(&c.0, "config.add_plugin", ctx.call_position()),
                &name,
                Some(options),
                ctx.call_position(),
            )
        },
    );
    e.register_fn(
        "amend_library",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         name: ImmutableString,
         f: FnPtr|
         -> Result<(), Box<EvalAltResult>> {
            // 11ty lowercases the name.
            let name = name.to_lowercase();
            if name != "md" {
                return Err(runtime(format!(
                    "amend_library(\"{name}\", ...): Brrrty only has 11ty's Markdown library, \"md\""
                )));
            }
            let mut state = open(&c.0, "config.amend_library", ctx.call_position());
            if state.closed.is_some() {
                // 11ty's library is set up already: ignored, with the warning above.
                return Ok(());
            }
            let md = state
                .markdown_lib
                .get_or_insert_with(crate::markdown_rules::MdLib::eleventy)
                .clone();
            let _ = f.call_within_context::<Dynamic>(&ctx, (md,))?;
            Ok(())
        },
    );
    e.register_fn(
        "set_library",
        |_: &mut Config, name: ImmutableString, _: Dynamic| -> Result<(), Box<EvalAltResult>> {
            Err(runtime(format!(
                "set_library(\"{name}\", ...) isn't ported: Brrrty renders Markdown with its own markdown-it, set up as 11ty's; use amend_library(\"md\", |md| ...) for renderer rules"
            )))
        },
    );
    crate::markdown_rules::register(&mut e);
    crate::collections::register(&mut e);
    crate::object::register(&mut e);
    crate::jshelpers::register(&mut e);
    register_text(&mut e);
    crate::builtins::register(&mut e);
    e.register_get("ignores", |c: &mut Config| Ignores(c.0.clone()));
    e.register_fn(
        "add",
        |ctx: rhai::NativeCallContext,
         i: &mut Ignores,
         glob: ImmutableString|
         -> Result<(), Box<EvalAltResult>> {
            open(&i.0, "config.ignores.add", ctx.call_position())
                .ignores
                .insert(glob.to_string());
            Ok(())
        },
    );
    e.register_fn(
        "delete",
        |ctx: rhai::NativeCallContext,
         i: &mut Ignores,
         glob: ImmutableString|
         -> Result<bool, Box<EvalAltResult>> {
            Ok(open(&i.0, "config.ignores.delete", ctx.call_position())
                .ignores
                .shift_remove(glob.as_str()))
        },
    );
    e.register_fn(
        "add_global_data",
        |ctx: rhai::NativeCallContext,
         c: &mut Config,
         name: ImmutableString,
         value: Dynamic|
         -> Result<(), Box<EvalAltResult>> {
            let pending =
                if let Some(f) = value.clone().try_cast::<FnPtr>() {
                    Pending::Fn(f, ctx.call_position())
                } else {
                    Pending::Value(to_value(&value).map_err(|why| {
                        runtime(format!("add_global_data(\"{name}\", ...): {why}"))
                    })?)
                };
            add_global_data(
                &mut open(&c.0, "config.add_global_data", ctx.call_position()).global_data,
                name.to_string(),
                pending,
            );
            Ok(())
        },
    );

    let now = move || JsDate(clock.now_ms());
    e.register_fn("now", now);
    // Rhai's `timestamp()` reads the real clock, so it would give different output on every
    // build even with SOURCE_DATE_EPOCH set. Rhai's time package is left out (`no_time`), and
    // this says what to use instead of "function not found".
    e.register_fn("timestamp", || -> Result<(), Box<EvalAltResult>> {
        Err(runtime(
            "timestamp() is not available: it reads the real clock, so output would change from \
             one build to the next. Use now(), which follows SOURCE_DATE_EPOCH"
                .into(),
        ))
    });
    e.register_fn("date", |ms: INT| JsDate(time_clip(ms as f64)));
    e.register_fn("date", |_: ()| JsDate(f64::NAN));
    // `new Date(null)` is the epoch.
    e.register_fn("date", |_: crate::object::JsNull| JsDate(0.0));
    e.register_fn("date", |d: JsDate| d);
    e.register_fn("date", |ms: rhai::FLOAT| JsDate(time_clip(ms)));
    e.register_fn("date_to_utc_string", |d: &mut JsDate| utc_string(d.0));
    e.register_fn("get_time", |d: &mut JsDate| number(d.0));
    e.register_fn("year", |d: &mut JsDate| match tz::fields(Tz::Local, d.0) {
        Some(f) => Dynamic::from_float(f.year as f64),
        None => Dynamic::from_float(f64::NAN),
    });
    // Numbers print as JavaScript prints them: `5`, not `5.0`.
    e.register_fn("to_string", |n: rhai::FLOAT| {
        brrrty_js::js_number_to_string(n)
    });
    e.register_fn("to_debug", |n: rhai::FLOAT| {
        brrrty_js::js_number_to_string(n)
    });
    e.register_fn("+", |s: ImmutableString, n: rhai::FLOAT| {
        format!("{s}{}", brrrty_js::js_number_to_string(n))
    });
    e.register_fn("+", |n: rhai::FLOAT, s: ImmutableString| {
        format!("{}{s}", brrrty_js::js_number_to_string(n))
    });
    e.register_fn("to_string", |d: &mut JsDate| {
        tz::date_to_string(Tz::Local, d.0)
    });
    e.register_fn("to_debug", |d: &mut JsDate| {
        tz::date_to_string(Tz::Local, d.0)
    });
    e
}

const MONTHS_LONG: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `d.toLocaleDateString('en-US', { year: 'numeric', month: 'long', day:
/// 'numeric' })` in `tz`. A year before 1 prints as ICU prints it, one
/// more than its absolute value (the era is not shown).
pub fn locale_date_string(tz: Tz, ms: f64) -> String {
    match tz::fields(tz, ms) {
        None => "Invalid Date".into(),
        Some(f) => {
            let year = if f.year >= 1 { f.year } else { 1 - f.year };
            format!("{} {}, {year}", MONTHS_LONG[f.month0 as usize], f.day)
        }
    }
}

/// Text helpers a filter needs: JavaScript regular expressions, and URLs.
fn register_text(e: &mut Engine) {
    type Res<T> = Result<T, Box<EvalAltResult>>;
    fn re(pattern: &str, flags: &str) -> Res<std::sync::Arc<regex::JsRegex>> {
        regex::compile(pattern, flags).map_err(runtime)
    }
    e.register_fn(
        "regex_replace",
        move |s: &str, p: &str, r: &str, f: &str| -> Res<String> {
            re(p, f)?.replace(s, r).map_err(runtime)
        },
    );
    e.register_fn(
        "regex_replace",
        move |s: &str, p: &str, r: &str| -> Res<String> {
            re(p, "")?.replace(s, r).map_err(runtime)
        },
    );
    let split = move |s: &str, p: &str, f: &str| -> Res<rhai::Array> {
        Ok(re(p, f)?
            .split(s)
            .map_err(runtime)?
            .into_iter()
            .map(|x| x.map_or(Dynamic::UNIT, Dynamic::from))
            .collect())
    };
    e.register_fn("regex_split", move |s: &str, p: &str| split(s, p, ""));
    e.register_fn("regex_split", move |s: &str, p: &str, f: &str| {
        split(s, p, f)
    });
    e.register_fn("regex_test", move |s: &str, p: &str| -> Res<bool> {
        re(p, "")?.is_match(s).map_err(runtime)
    });
    e.register_fn(
        "regex_test",
        move |s: &str, p: &str, f: &str| -> Res<bool> { re(p, f)?.is_match(s).map_err(runtime) },
    );
    // `new URL(s).hostname`, or `()` when `s` isn't an absolute URL.
    e.register_fn("url_hostname", |s: Dynamic| -> Dynamic {
        let Ok(text) = s.into_immutable_string() else {
            return Dynamic::UNIT;
        };
        match url::Url::parse(text.as_str()) {
            Ok(u) => Dynamic::from(u.host_str().unwrap_or("").to_string()),
            Err(_) => Dynamic::UNIT,
        }
    });
    // JavaScript truthiness: `if (x)` is `if truthy(x)`.
    e.register_fn("truthy", |v: Dynamic| -> bool {
        crate::convert::to_value(&v).map_or(true, |v| v.js_truthy())
    });
    e.register_fn("to_locale_date_string", |d: &mut JsDate| {
        locale_date_string(Tz::Local, d.0)
    });
}

/// The plugins `add_plugin` knows, by the name a script gives.
pub const PLUGINS: [&str; 6] = [
    "rss",
    "syntaxhighlight",
    "htmlbase",
    "idattribute",
    "inputpathtourl",
    "render",
];

/// `eleventyConfig.addPlugin(plugin, options)` for a plugin Brrrty has its
/// own version of, named as in [`PLUGINS`].
fn add_plugin(
    state: &mut State,
    name: &str,
    options: Option<Map>,
    pos: Position,
) -> Result<(), Box<EvalAltResult>> {
    match name {
        // `@11ty/eleventy-plugin-syntaxhighlight`.
        "syntaxhighlight" => {
            if state.syntax_highlight.is_some() {
                return Err(invalid(
                    "add_plugin(\"syntaxhighlight\", ...) is called twice; Brrrty takes it once, with all its options".to_string(),
                    pos,
                ));
            }
            let plugin = crate::highlight::syntax_highlight(options.as_ref())
                .map_err(|why| invalid(why, pos))?;
            state.syntax_highlight = Some(plugin);
            Ok(())
        }
        // `@11ty/eleventy`'s `HtmlBasePlugin`. Brrrty's `pathPrefix` is
        // always `/`, which turns its page transform off, so it adds two
        // filters. Its options (`baseHref`, `extensions`) change the pages.
        "htmlbase" => {
            if let Some(key) = options.as_ref().and_then(|o| o.keys().next()) {
                return Err(invalid(
                    format!(
                        "add_plugin(\"htmlbase\", ...) takes no options yet: Brrrty doesn't support `{key}`, which changes the URLs the plugin writes"
                    ),
                    pos,
                ));
            }
            state.html_base = true;
            Ok(())
        }
        // `@11ty/eleventy`'s `IdAttributePlugin`. Its options (`selector`,
        // `slugify`, `decodeEntities`, `checkDuplicates`, `filter`) change the pages.
        "idattribute" => {
            if let Some(key) = options.as_ref().and_then(|o| o.keys().next()) {
                return Err(invalid(
                    format!(
                        "add_plugin(\"idattribute\", ...) takes no options yet: Brrrty doesn't support `{key}`, which changes the ids the plugin writes"
                    ),
                    pos,
                ));
            }
            state.id_attribute = true;
            Ok(())
        }
        // `@11ty/eleventy`'s `InputPathToUrlTransformPlugin`. Its option (`extensions`)
        // changes which pages it rewrites.
        "inputpathtourl" => {
            if let Some(key) = options.as_ref().and_then(|o| o.keys().next()) {
                return Err(invalid(
                    format!(
                        "add_plugin(\"inputpathtourl\", ...) takes no options yet: Brrrty doesn't support `{key}`, which changes the pages the plugin rewrites"
                    ),
                    pos,
                ));
            }
            state.input_path_to_url = true;
            Ok(())
        }
        // `@11ty/eleventy`'s `RenderPlugin`. Its options (`tagName`, `tagNameFile`, `filterName`,
        // `accessGlobalData`) change the tags it adds and the data they see.
        "render" => {
            if let Some(key) = options.as_ref().and_then(|o| o.keys().next()) {
                return Err(invalid(
                    format!(
                        "add_plugin(\"render\", ...) takes no options yet: Brrrty doesn't support `{key}`, which changes the tags the plugin adds or the data they see"
                    ),
                    pos,
                ));
            }
            state.render_plugin = true;
            Ok(())
        }
        // `@11ty/eleventy-plugin-rss`'s `feedPlugin`: one feed per call.
        "rss" => {
            let options = options.unwrap_or_default();
            // `addPlugin(pluginRss)`, the plugin's default export: its
            // filters (and the HTML <base> plugin's), and no feed.
            if let Some(feed) = options.get("feed") {
                if feed.as_bool().ok() != Some(false) {
                    return Err(invalid(
                        "add_plugin(\"rss\", ...): `feed` can only be `false`, which adds the plugin's filters without a feed; leave it out to add a feed".to_string(),
                        pos,
                    ));
                }
                if options.len() > 1 {
                    return Err(invalid(
                        "add_plugin(\"rss\", #{ feed: false }) takes no other options; Brrrty doesn't support `htmlBasePluginOptions` or `posthtmlRenderOptions` yet, and a feed's options go in a call of its own".to_string(),
                        pos,
                    ));
                }
                state.rss_filters = true;
                return Ok(());
            }
            let value = to_value(&Dynamic::from_map(options))
                .map_err(|why| invalid(format!("add_plugin(\"rss\", ...): {why}"), pos))?;
            let feed = crate::plugins::feed(&value).map_err(|why| invalid(why, pos))?;
            if state.feeds.iter().any(|f| f.input_path == feed.input_path) {
                return Err(invalid(
                    format!(
                        "Virtual template conflict: you can\u{2019}t add multiple virtual templates that have the same inputPath: {} (11ty's error; give one feed another `input_path`, or another `metadata.title` or `type`)",
                        feed.input_path
                    ),
                    pos,
                ));
            }
            state.feeds.push(feed);
            Ok(())
        }
        _ => Err(invalid(
            format!(
                "add_plugin(\"{name}\", ...): Brrrty doesn't have this plugin. It has {}",
                PLUGINS
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            pos,
        )),
    }
}

/// 11ty's `addGlobalData`: two plain objects merge deeply, anything else
/// replaces the old value where it was.
fn add_global_data(data: &mut IndexMap<String, Pending>, name: String, value: Pending) {
    let merged = match (data.get(&name), &value) {
        (Some(Pending::Value(old @ Value::Object(_))), Pending::Value(new @ Value::Object(_))) => {
            Pending::Value(deep_copy(Some(old), new))
        }
        _ => value,
    };
    data.insert(name, merged);
}

/// Resolve each pending value in order, calling functions as 11ty does.
pub fn resolve_global_data(
    engine: &Engine,
    ast: &rhai::AST,
    data: IndexMap<String, Pending>,
) -> Result<Data, Box<EvalAltResult>> {
    let mut out = Data::new();
    for (name, pending) in data {
        let value = match pending {
            Pending::Value(v) => v,
            Pending::Fn(f, pos) => {
                let made: Dynamic = f.call(engine, ast, ())?;
                to_value(&made).map_err(|why| {
                    invalid(
                        format!("add_global_data(\"{name}\", ...): its function returned {why}"),
                        pos,
                    )
                })?
            }
        };
        out.insert(name, value);
    }
    Ok(out)
}

/// The template formats Brrrty builds, in 11ty's default order.
const KNOWN_FORMATS: &[&str] = &[
    "liquid", "md", "njk", "html", "11ty.js", "11ty.cjs", "11ty.mjs",
];

/// A list of template formats: an array of names, or one string of names
/// separated by commas, as 11ty reads them. `what` names the setting in errors.
/// An empty string is `None`: 11ty treats it as not set, so the defaults build.
pub(crate) fn template_formats(what: &str, v: Dynamic) -> Result<Option<Vec<String>>, String> {
    let type_name = v.type_name();
    let names: Vec<String> = if let Some(s) = v.clone().try_cast::<ImmutableString>() {
        if s.is_empty() {
            return Ok(None);
        }
        s.split(',').map(str::to_string).collect()
    } else if let Some(a) = v.try_cast::<rhai::Array>() {
        let mut names = Vec::new();
        for item in a {
            let t = item.type_name();
            match item.try_cast::<ImmutableString>() {
                Some(s) => names.push(s.to_string()),
                None => {
                    return Err(format!(
                        "{what}: a list of template formats holds a {t}; it should hold names, like \"njk\""
                    ));
                }
            }
        }
        names
    } else {
        return Err(format!(
            "{what}: got a {type_name}; it should be a list of template formats, like [\"njk\", \"md\"]"
        ));
    };
    normalize_template_formats(what, names).map(Some)
}

/// Trim, lowercase and check template format names, as 11ty reads them.
pub fn normalize_template_formats(what: &str, names: Vec<String>) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for n in names {
        // 11ty trims and lowercases the names.
        let n = n.trim().to_lowercase();
        if n.is_empty() {
            continue;
        }
        if n.starts_with('.') {
            return Err(format!(
                "{what}: `{n}` matches no files; 11ty wants the bare name, like \"{}\"",
                n.trim_start_matches('.')
            ));
        }
        if !KNOWN_FORMATS.contains(&n.as_str()) {
            return Err(format!(
                "{what}: `{n}` is not a template format Brrrty builds; it builds liquid, md, njk, html and 11ty.js"
            ));
        }
        if !out.contains(&n) {
            out.push(n);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_string_matches_node() {
        // Probed with Node: `new Date(t).toUTCString()`.
        assert_eq!(utc_string(0.0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(
            utc_string(-62_198_755_200_000.0),
            "Fri, 01 Jan -0001 00:00:00 GMT"
        );
        assert_eq!(utc_string(8.64e15), "Sat, 13 Sep 275760 00:00:00 GMT");
        assert_eq!(utc_string(f64::NAN), "Invalid Date");
    }

    #[test]
    fn locale_dates_match_node() {
        // Node 26, TZ=UTC: `new Date(t).toLocaleDateString('en-US', ...)`.
        for (t, want) in [
            (0.0, "January 1, 1970"),
            (1_700_000_000_000.0, "November 14, 2023"),
            (-62_198_755_200_000.0, "January 1, 2"),
            (8.64e15, "September 13, 275760"),
            (253_402_300_800_000.0, "January 1, 10000"),
            (-1.0, "December 31, 1969"),
            (951_782_400_000.0, "February 29, 2000"),
            (f64::NAN, "Invalid Date"),
        ] {
            assert_eq!(locale_date_string(Tz::Utc, t), want, "{t}");
        }
    }

    #[test]
    fn time_clip_is_javascripts() {
        assert_eq!(time_clip(1.9), 1.0);
        assert_eq!(time_clip(-1.9), -1.0);
        assert!(time_clip(8.64e15 + 1.0).is_nan());
    }
}
