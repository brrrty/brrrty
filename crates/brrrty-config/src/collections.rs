//! Collections written in Rhai: `config.add_collection("name", |api| ...)`.
//!
//! 11ty computes a config collection at its place in its dependency graph
//! (after the tag collections, before `all` is set, and before any template
//! that paginates it), handing the function its `TemplateCollection`: the
//! pages added so far. Brrrty's core does the same and calls the function
//! here with those pages. `api` has 11ty's methods in snake_case:
//!
//! - `get_all()`: every page, in the order 11ty added them;
//! - `get_all_sorted()`: by date, then input path;
//! - `get_filtered_by_tag(tag)`, `get_filtered_by_tags(a, b, ...)`: sorted,
//!   the pages in that tag (in every one of those tags);
//! - `get_filtered_by_glob(glob)` (a glob, or an array of them): sorted, the
//!   pages whose input path matches (see `glob.rs`).
//!
//! Each gives a new array of the items themselves. An item is read-only,
//! and so is everything read through it (`object.rs`); whatever the
//! function returns (an array, an `object()`, a map, nested any way) holds
//! those same items, so their content and identity survive.

use std::sync::{Arc, Mutex};

use brrrty_core::{CollectionError, CollectionFn, CollectionInput, CollectionPage, Problem, codes};
use brrrty_js::Value;
use rhai::{Array, Dynamic, EvalAltResult, FnPtr, ImmutableString, Position};

use crate::CONFIG_FILE;
use crate::api::invalid;
use crate::filters::Runtime;
use crate::glob::Glob;
use crate::object::{JsObject, frozen_dynamic};

/// The `api` a collection's function gets.
#[derive(Clone)]
pub struct CollectionApi {
    input: Arc<CollectionInput>,
    /// The first page Brrrty can't date that a call would have returned.
    undated: Arc<Mutex<Option<String>>>,
    /// Set when `get_all()` was called while the order pages were added in
    /// isn't known: why.
    unordered: Arc<Mutex<Option<String>>>,
}

type Res<T> = Result<T, Box<EvalAltResult>>;

impl CollectionApi {
    /// The pages that pass `keep`, in `order`, as items. A page Brrrty
    /// can't date that would pass makes the result unknowable.
    fn pick(&self, sorted: bool, keep: impl Fn(&str, &[String]) -> bool) -> Res<Array> {
        if let Some(u) = self
            .input
            .undated
            .iter()
            .find(|u| keep(&u.input_path, &u.tags))
        {
            let path = u.input_path.clone();
            if let Ok(mut slot) = self.undated.lock() {
                slot.get_or_insert_with(|| path.clone());
            }
            return Err(invalid(
                format!(
                    "its result would hold {}, a page whose date Brrrty can't read, so it can't tell the order",
                    path.strip_prefix("./").unwrap_or(&path)
                ),
                Position::NONE,
            ));
        }
        if !sorted && let Some(why) = &self.input.unordered {
            if let Ok(mut slot) = self.unordered.lock() {
                slot.get_or_insert_with(|| why.clone());
            }
            return Err(invalid(
                format!("`get_all()` gives pages in the order 11ty added them, {why}"),
                Position::NONE,
            ));
        }
        let pages = &self.input.pages;
        let pass = |p: &CollectionPage| keep(&p.input_path, &p.tags);
        let items: Box<dyn Iterator<Item = &CollectionPage>> = if sorted {
            Box::new(self.input.sorted.iter().map(|&i| &pages[i]))
        } else {
            Box::new(pages.iter())
        };
        Ok(items
            .filter(|p| pass(p))
            .map(|p| frozen_dynamic(&p.item))
            .collect())
    }

    fn by_tags(&self, tags: &[ImmutableString]) -> Res<Array> {
        // `getFilteredByTags`: in every one of them.
        self.pick(true, |_, have| {
            tags.iter().all(|t| have.iter().any(|h| h == t.as_str()))
        })
    }

    fn by_glob(&self, globs: &[Glob]) -> Res<Array> {
        self.pick(true, |path, _| globs.iter().any(|g| g.matches(path)))
    }
}

fn globs_of(v: &Dynamic) -> Res<Vec<Glob>> {
    let compile = |s: &str| Glob::new(s).map_err(|why| invalid(why, Position::NONE));
    if let Some(s) = v.read_lock::<ImmutableString>() {
        return Ok(vec![compile(s.as_str())?]);
    }
    if let Some(a) = v.read_lock::<Array>() {
        return a
            .iter()
            .map(|x| match x.read_lock::<ImmutableString>() {
                Some(s) => compile(s.as_str()),
                None => Err(invalid(
                    format!(
                        "get_filtered_by_glob takes a glob or an array of globs, and this array holds a {}",
                        x.type_name()
                    ),
                    Position::NONE,
                )),
            })
            .collect();
    }
    Err(invalid(
        format!(
            "get_filtered_by_glob takes a glob or an array of globs, not a {}",
            v.type_name()
        ),
        Position::NONE,
    ))
}

pub fn register(e: &mut rhai::Engine) {
    e.register_type_with_name::<CollectionApi>("CollectionApi")
        .register_fn("get_all", |a: &mut CollectionApi| {
            a.pick(false, |_, _| true)
        })
        .register_fn("get_all_sorted", |a: &mut CollectionApi| {
            a.pick(true, |_, _| true)
        })
        .register_fn(
            "get_filtered_by_tag",
            |a: &mut CollectionApi, tag: ImmutableString| {
                // `!tagName` gives every page.
                if tag.is_empty() {
                    a.pick(true, |_, _| true)
                } else {
                    a.pick(true, |_, have| have.iter().any(|h| h == tag.as_str()))
                }
            },
        )
        .register_fn("get_filtered_by_tag", |a: &mut CollectionApi, _: ()| {
            a.pick(true, |_, _| true)
        })
        .register_fn("get_filtered_by_tags", |a: &mut CollectionApi| {
            a.by_tags(&[])
        })
        .register_fn(
            "get_filtered_by_tags",
            |a: &mut CollectionApi, t1: ImmutableString| a.by_tags(&[t1]),
        )
        .register_fn(
            "get_filtered_by_tags",
            |a: &mut CollectionApi, t1: ImmutableString, t2: ImmutableString| a.by_tags(&[t1, t2]),
        )
        .register_fn(
            "get_filtered_by_tags",
            |a: &mut CollectionApi,
             t1: ImmutableString,
             t2: ImmutableString,
             t3: ImmutableString| a.by_tags(&[t1, t2, t3]),
        )
        .register_fn(
            "get_filtered_by_tags",
            |a: &mut CollectionApi,
             t1: ImmutableString,
             t2: ImmutableString,
             t3: ImmutableString,
             t4: ImmutableString| a.by_tags(&[t1, t2, t3, t4]),
        )
        .register_fn(
            "get_filtered_by_glob",
            |a: &mut CollectionApi, globs: Dynamic| a.by_glob(&globs_of(&globs)?),
        )
        .register_fn("to_string", |_: &mut CollectionApi| {
            "[object TemplateCollection]".to_string()
        })
        .register_fn("to_debug", |_: &mut CollectionApi| {
            "[object TemplateCollection]".to_string()
        });
}

/// A call that starts with less than this much stack left runs on a new
/// stack, as a filter's does.
const STACK_RED_ZONE: usize = 12 * 1024 * 1024;
const CALL_STACK: usize = 128 * 1024 * 1024;

/// The collection as the core runs it. `at` is where `add_collection` was
/// called, for problems that have no place of their own.
pub fn make(rt: Arc<Runtime>, name: String, f: FnPtr, at: Position) -> CollectionFn {
    Arc::new(move |input: &CollectionInput| run(&rt, &name, &f, at, input))
}

fn problem(name: &str, message: String, code: &'static str, pos: Position) -> Problem {
    let p = Problem::error(code, format!("collection `{name}` failed: {message}"))
        .with_path(CONFIG_FILE);
    let p = match (pos.line(), pos.position()) {
        (Some(line), col) => p.at(line, col.unwrap_or(1)),
        _ => p,
    };
    p.with_help("pages that read this collection were not written; 11ty stops the whole build here")
}

fn run(
    rt: &Runtime,
    name: &str,
    f: &FnPtr,
    at: Position,
    input: &CollectionInput,
) -> Result<Value, CollectionError> {
    crate::api::start_clock();
    crate::object::clear_premature();
    let api = CollectionApi {
        input: Arc::new(input.clone()),
        undated: Arc::new(Mutex::new(None)),
        unordered: Arc::new(Mutex::new(None)),
    };
    let undated = api.undated.clone();
    let unordered = api.unordered.clone();
    let call = || {
        stacker::maybe_grow(STACK_RED_ZONE, CALL_STACK, || {
            f.call::<Dynamic>(&rt.engine, &rt.ast, (api,))
        })
    };
    let result = brrrty_core::contain::catch(call);
    let premature = crate::object::take_premature();
    if let Some(path) = undated.lock().ok().and_then(|mut u| u.take()) {
        return Err(CollectionError::Undated(path));
    }
    // Even when the config catches it: its result would follow that order.
    if let Some(why) = unordered.lock().ok().and_then(|mut u| u.take()) {
        let mut p = problem(
            name,
            format!("`get_all()` gives pages in the order 11ty added them, {why}"),
            codes::DATA_UNSUPPORTED,
            at,
        );
        p.help = Some("Use `get_all_sorted()`, `get_filtered_by_tag()` or `get_filtered_by_glob()`, which are sorted by date. Pages that read this collection were not written.".into());
        return Err(CollectionError::Failed(p));
    }
    let failed = |message: String, code, pos| {
        Err(CollectionError::Failed(problem(name, message, code, pos)))
    };
    let value = match result {
        Err(why) => return failed(why, codes::CONFIG_ERROR, at),
        Ok(Err(e)) => {
            let parts = crate::error_parts(*e);
            let pos = if parts.position.is_none() {
                at
            } else {
                parts.position
            };
            let code = if parts.code == codes::CONFIG_INVALID {
                codes::CONFIG_ERROR
            } else {
                parts.code
            };
            // A caught premature read still stops it.
            let message = premature.unwrap_or(parts.message);
            return failed(message, code, pos);
        }
        Ok(Ok(v)) => v,
    };
    if let Some(why) = premature {
        return failed(why, codes::CONFIG_ERROR, at);
    }
    if let Some(why) = not_an_item(&value) {
        return failed(why, codes::CONFIG_ERROR, at);
    }
    crate::convert::to_value(&value).or_else(|why| {
        failed(
            format!("it returned a value that can't be data: {why}"),
            codes::CONFIG_ERROR,
            at,
        )
    })
}

/// 11ty gives every object in a returned array that has an `inputPath` the
/// content of that page (`populateCollectionsWithContent`). An item has
/// it already; an object the script made would get it too, which Brrrty
/// can't follow.
fn not_an_item(v: &Dynamic) -> Option<String> {
    let a = v.read_lock::<Array>()?;
    a.iter().find_map(|x| {
        let o = x.read_lock::<JsObject>()?;
        (!matches!(&*o, JsObject::Frozen(_)) && o.contains("inputPath")).then(|| {
            "it returned an array holding an object with an `inputPath` that isn't a collection item; 11ty would give that object the page's content, which Brrrty can't".to_string()
        })
    })
}
