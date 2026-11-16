//! The Rhai engine and the `config` object a script sees.
//!
//! `config` mirrors 11ty's `eleventyConfig` in snake_case. Each call records
//! what it set in [`State`]; nothing runs until the whole script has.

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

/// Start the clock that stops a run after [`MAX_SECONDS`]. Call it where a
/// run begins on this thread.
pub fn start_clock() {
    RUN_START.with(|s| s.set(std::time::Instant::now()));
}

/// A JavaScript `Date`: milliseconds since 1970, `NaN` when invalid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JsDate(pub f64);

/// A global data value, or the function 11ty calls for it.
#[derive(Debug, Clone)]
pub enum Pending {
    Value(Value),
    Fn(FnPtr, Position),
}

/// What the script set, in call order.
#[derive(Debug, Default)]
pub struct State {
    /// Passthrough targets by source as written; `None` copies in place.
    pub passthrough: IndexMap<String, Option<String>>,
    pub ignores: IndexSet<String>,
    pub global_data: IndexMap<String, Pending>,
    pub filters: IndexMap<String, FilterDef>,
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

/// A value the script gave that Brrrty can't use, thrown as a Rhai error.
#[derive(Debug, Clone)]
pub struct InvalidValue(pub String);

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
    if n.is_finite() {
        Dynamic::from_int(n as INT)
    } else {
        Dynamic::from_float(n)
    }
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

/// An engine with Brrrty's limits and the config API registered.
pub fn engine(clock: Clock) -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(MAX_OPERATIONS)
        .set_max_call_levels(MAX_CALL_LEVELS)
        .set_max_expr_depths(MAX_EXPR_DEPTH, MAX_FUNCTION_EXPR_DEPTH)
        .set_max_string_size(MAX_STRING_SIZE)
        .set_max_array_size(MAX_ARRAY_SIZE)
        .set_max_map_size(MAX_MAP_SIZE)
        // Config scripts are one file: no imports, and no `eval`.
        .set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new())
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
    e.on_print(|s| println!("{s}"));
    e.on_debug(|s, _src, pos| eprintln!("{} {pos}: {s}", crate::CONFIG_FILE));

    e.register_type_with_name::<Config>("Config")
        .register_type_with_name::<Ignores>("Ignores")
        .register_type_with_name::<JsDate>("Date");

    e.register_fn(
        "add_passthrough_copy",
        |c: &mut Config, from: ImmutableString| {
            lock(&c.0).passthrough.insert(from.to_string(), None);
        },
    );
    e.register_fn(
        "add_passthrough_copy",
        |c: &mut Config, map: Map| -> Result<(), Box<EvalAltResult>> {
            let mut state = lock(&c.0);
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
        |c: &mut Config, name: ImmutableString, f: FnPtr| {
            lock(&c.0)
                .filters
                .insert(name.to_string(), FilterDef::Fn(f));
        },
    );
    e.register_fn(
        "add_nondeterministic_filter",
        |c: &mut Config, name: ImmutableString| {
            lock(&c.0)
                .filters
                .insert(name.to_string(), FilterDef::Nondeterministic);
        },
    );
    crate::object::register(&mut e);
    register_text(&mut e);
    e.register_get("ignores", |c: &mut Config| Ignores(c.0.clone()));
    e.register_fn("add", |i: &mut Ignores, glob: ImmutableString| {
        lock(&i.0).ignores.insert(glob.to_string());
    });
    e.register_fn("delete", |i: &mut Ignores, glob: ImmutableString| {
        lock(&i.0).ignores.shift_remove(glob.as_str())
    });
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
            add_global_data(&mut lock(&c.0).global_data, name.to_string(), pending);
            Ok(())
        },
    );

    let now = move || JsDate(clock.now_ms());
    e.register_fn("now", now);
    e.register_fn("date", |ms: INT| JsDate(time_clip(ms as f64)));
    e.register_fn("date", |_: ()| JsDate(f64::NAN));
    e.register_fn("date", |d: JsDate| d);
    e.register_fn("date", |ms: rhai::FLOAT| JsDate(time_clip(ms)));
    e.register_fn("date_to_utc_string", |d: &mut JsDate| utc_string(d.0));
    e.register_fn("get_time", |d: &mut JsDate| number(d.0));
    e.register_fn("year", |d: &mut JsDate| match tz::fields(Tz::Local, d.0) {
        Some(f) => Dynamic::from_int(f.year as INT),
        None => Dynamic::from_float(f64::NAN),
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
        move |s: &str, p: &str, r: &str, f: &str| -> Res<String> { Ok(re(p, f)?.replace(s, r)) },
    );
    e.register_fn(
        "regex_replace",
        move |s: &str, p: &str, r: &str| -> Res<String> { Ok(re(p, "")?.replace(s, r)) },
    );
    let split = move |s: &str, p: &str, f: &str| -> Res<rhai::Array> {
        Ok(re(p, f)?
            .split(s)
            .into_iter()
            .map(|x| x.map_or(Dynamic::UNIT, Dynamic::from))
            .collect())
    };
    e.register_fn("regex_split", move |s: &str, p: &str| split(s, p, ""));
    e.register_fn("regex_split", move |s: &str, p: &str, f: &str| {
        split(s, p, f)
    });
    e.register_fn("regex_test", move |s: &str, p: &str| -> Res<bool> {
        Ok(re(p, "")?.is_match(s))
    });
    e.register_fn(
        "regex_test",
        move |s: &str, p: &str, f: &str| -> Res<bool> { Ok(re(p, f)?.is_match(s)) },
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
    e.register_fn("to_locale_date_string", |d: &mut JsDate| {
        locale_date_string(Tz::Local, d.0)
    });
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
