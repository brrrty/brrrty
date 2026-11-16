//! Filters written in Rhai: `config.add_filter("name", |value, arg| ...)`.
//!
//! A filter gets the value it is applied to, then the arguments in the
//! order the template wrote them, as 11ty passes them to a JavaScript
//! filter. A keyword argument keeps its place and arrives as a two-item
//! array, `[name, value]`. An argument the template left out is `()`
//! (JavaScript's `undefined`), so `n ?? 40` gives a default, and extra
//! arguments are ignored, as they are in JavaScript.
//!
//! A filter that fails, returns something that isn't data, or runs too long
//! refuses the page that used it and nothing else. The error has two
//! positions: the template's, and the script's own.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use brrrty_js::Value;
use brrrty_liquid::{ArgsFilterFn, FilterCtx};
use rhai::{AST, Dynamic, Engine, FnPtr};

use crate::CONFIG_FILE;
use crate::convert::{to_dynamic, to_value};

/// What a filter is.
#[derive(Debug, Clone)]
pub enum FilterDef {
    Fn(FnPtr),
    /// A filter whose output can't match 11ty's: it uses randomness.
    Nondeterministic,
}

/// The script, kept so its filters can run while pages render.
pub struct Runtime {
    pub engine: Engine,
    pub ast: AST,
}

/// Stack for one filter call. Rhai recurses on Rust's stack, and render
/// threads are small. The memory is only used as deep as the call goes.
const CALL_STACK: usize = 256 * 1024 * 1024;

/// How many arguments (the value included) the script function takes, not
/// counting the ones a closure captured.
fn arity(rt: &Runtime, f: &FnPtr) -> Option<usize> {
    rt.ast
        .iter_functions()
        .find(|m| m.name == f.fn_name())
        .map(|m| m.params.len().saturating_sub(f.curry().len()))
}

/// The filter as the Liquid engine runs it.
pub fn make(rt: Arc<Runtime>, name: String, def: FilterDef) -> ArgsFilterFn {
    match def {
        FilterDef::Nondeterministic => Arc::new(move |_, _, _: &FilterCtx| {
            Err(format!(
                "the filter `{name}` is nondeterministic: it uses randomness, so its output can't be the same as 11ty's; Brrrty refuses pages that use it"
            ))
        }),
        FilterDef::Fn(f) => {
            let arity = arity(&rt, &f);
            Arc::new(move |input, args, _: &FilterCtx| call(&rt, &name, &f, arity, input, args))
        }
    }
}

fn call(
    rt: &Runtime,
    name: &str,
    f: &FnPtr,
    arity: Option<usize>,
    input: &Value,
    args: &[Value],
) -> Result<Value, String> {
    crate::api::start_clock();
    let mut argv: Vec<Dynamic> = std::iter::once(input).chain(args).map(to_dynamic).collect();
    if let Some(n) = arity {
        argv.resize(n, Dynamic::UNIT);
    }
    let run = || stacker::grow(CALL_STACK, || f.call::<Dynamic>(&rt.engine, &rt.ast, argv));
    match std::panic::catch_unwind(AssertUnwindSafe(run)) {
        Err(panic) => {
            let why = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "it panicked".into());
            Err(format!("filter `{name}` failed: {why}"))
        }
        Ok(Err(e)) => Err(describe(name, *e)),
        Ok(Ok(result)) => to_value(&result)
            .map_err(|why| format!("filter `{name}` returned a value that can't be data: {why}")),
    }
}

/// A Rhai error as one line: what went wrong, and where in the script.
fn describe(name: &str, e: rhai::EvalAltResult) -> String {
    let parts = crate::error_parts(e);
    let mut text = format!("filter `{name}` failed: {}", parts.message);
    if let (Some(line), col) = (parts.position.line(), parts.position.position()) {
        text.push_str(&format!(" (at {CONFIG_FILE}:{line}:{})", col.unwrap_or(1)));
    }
    text
}
