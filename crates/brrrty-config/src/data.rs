//! `.rhai` data files: the Rhai port of a JavaScript data file.
//!
//! `_data/site.rhai` gives the key `site`, `x.11tydata.rhai` and
//! `dir/dir.11tydata.rhai` give template and directory data, exactly as
//! the `.json` and `.js` files of those names do. The script's last
//! expression is the value (a map or `object()`, an array, or a scalar):
//!
//! ```rhai
//! #{ name: "Mine", url: "https://example.com" }
//! ```
//!
//! A script can instead define `fn data()`, which Brrrty calls the way 11ty
//! calls a function a JavaScript data file exports. 11ty hands that
//! function the config's global data (`add_global_data` values, and its own
//! `eleventy` data), so `fn data(global)` receives the same. When `data`
//! is defined it is the whole value: the script's own statements don't run.
//!
//! Each file runs once per build, on a fresh engine, so nothing one file
//! defines (a function, a variable) is there in the next. The script
//! can't see `config`. It has the same limits as the config script, and
//! `now()` follows `SOURCE_DATE_EPOCH`. A script that fails gives a problem
//! with the file, line and column; the build refuses only the pages that
//! read its key.

use brrrty_core::frontmatter::Data;
use brrrty_core::{DataRequest, DataRunner, Problem, codes};
use brrrty_js::Value;
use rhai::{Dynamic, Scope};

use crate::api::{
    MAX_ARRAY_SIZE, MAX_CALL_LEVELS, MAX_MAP_SIZE, MAX_OPERATIONS, MAX_SECONDS, MAX_STRING_SIZE,
};
use crate::convert::{to_dynamic, to_value};
use crate::{Clock, STACK_SIZE, at, error_parts};

/// The runner a build calls for each `.rhai` data file, in the project at
/// `root`. What the scripts read from the project goes in `reads`.
pub fn data_runner(
    clock: Clock,
    root: std::path::PathBuf,
    reads: brrrty_core::FileReads,
) -> DataRunner {
    DataRunner(std::sync::Arc::new(move |req| {
        run_file(req, clock, &root, &reads)
    }))
}

fn run_file(
    req: &DataRequest,
    clock: Clock,
    root: &std::path::Path,
    reads: &brrrty_core::FileReads,
) -> Result<Value, Problem> {
    let bytes = std::fs::read(req.file).map_err(|e| {
        Problem::error(
            codes::DATA_FILE_INVALID,
            format!("cannot read this data file: {e}"),
        )
    })?;
    let src = String::from_utf8(bytes).map_err(|_| {
        Problem::error(codes::DATA_FILE_INVALID, "this data file isn't UTF-8")
            .with_help("save the file as UTF-8")
    })?;
    let global = req.global.clone();
    let label = req.shown.display().to_string();
    let root = root.to_path_buf();
    let reads = reads.clone();
    let run = std::thread::Builder::new()
        .name("brrrty-data".into())
        .stack_size(STACK_SIZE)
        .spawn(move || {
            brrrty_core::contain::catch(|| evaluate(&src, &global, clock, &label, &root, reads))
        })
        .map_err(|e| {
            Problem::error(
                codes::BUILD_THREADS_FAILED,
                format!("cannot start a thread to run a data file: {e}"),
            )
        })?;
    let crashed = |why: String| {
        Err(Problem::error(
            codes::DATA_RHAI_ERROR,
            format!("Brrrty crashed while running this data file: {why}"),
        )
        .with_help(crate::BUG_HELP))
    };
    match run.join() {
        Ok(Ok(r)) => r,
        Ok(Err(why)) => crashed(why),
        Err(_) => crashed("the thread running it stopped".into()),
    }
}

/// Run one script on its own engine and read its value.
pub(crate) fn evaluate(
    src: &str,
    global: &Data,
    clock: Clock,
    label: &str,
    root: &std::path::Path,
    reads: brrrty_core::FileReads,
) -> Result<Value, Problem> {
    crate::api::start_clock();
    let engine = crate::api::engine_for(clock, label.to_string(), root, reads);
    let ast = engine.compile(src).map_err(|e| {
        let help = match &*e.0 {
            rhai::ParseErrorType::VariableUndefined(name) if name == "config" => {
                "a data script has no `config`: that object is only in brrrty.config.rhai".to_string()
            }
            rhai::ParseErrorType::VariableUndefined(_) => "a function can't see variables outside it, and a closure `|x| ...` can't capture them. Pass the value in as an argument; see PORTING.md".to_string(),
            _ => "fix the script; Rhai's syntax is close to JavaScript's, see https://rhai.rs/book/".to_string(),
        };
        at(
            Problem::error(codes::DATA_RHAI_SYNTAX, e.0.to_string()).with_help(help),
            e.1,
        )
    })?;
    let exported = ast.iter_functions().find(|f| f.name == "data");
    let result: Result<Dynamic, _> = match exported {
        Some(f) => {
            let args: Vec<Dynamic> = match f.params.len() {
                0 => vec![],
                1 => vec![to_dynamic(&Value::Object(std::sync::Arc::new(
                    global.clone(),
                )))],
                n => {
                    return Err(Problem::error(
                        codes::DATA_RHAI_ERROR,
                        format!(
                            "`fn data` takes {n} parameters; 11ty's exported function gets one, the global data, so write `fn data()` or `fn data(global)`"
                        ),
                    ));
                }
            };
            // Only `data` runs: the script's own statements don't.
            engine.call_fn_with_options::<Dynamic>(
                rhai::CallFnOptions::new().eval_ast(false),
                &mut Scope::new(),
                &ast,
                "data",
                args,
            )
        }
        None => engine.eval_ast::<Dynamic>(&ast),
    };
    let value = result.map_err(|e| problem(*e))?;
    to_value(&value).map_err(|why| {
        Problem::error(
            codes::DATA_RHAI_ERROR,
            format!("this data file's value can't be data: {why}"),
        )
        .with_help("a data file gives maps, arrays, strings, numbers, booleans and dates; functions and closures can't be data")
    })
}

fn problem(e: rhai::EvalAltResult) -> Problem {
    let parts = error_parts(e);
    let p = if parts.limit {
        Problem::error(codes::DATA_RHAI_LIMIT, parts.message).with_help(format!(
            "a data script may run {MAX_OPERATIONS} operations for up to {MAX_SECONDS} seconds, nest {MAX_CALL_LEVELS} calls, and build strings up to {} MiB and up to {MAX_ARRAY_SIZE} array items or {MAX_MAP_SIZE} map keys; look for a loop or recursion that never stops",
            MAX_STRING_SIZE / 1024 / 1024
        ))
    } else {
        Problem::error(codes::DATA_RHAI_ERROR, parts.message)
    };
    at(p, parts.position)
}
