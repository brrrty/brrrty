//! What crosses from a JavaScript runtime (the site's own Node, run by
//! `brrrty-node`) into a build: decoded values, function handles, errors,
//! and the trait a build drives the runtime through.

use std::path::{Path, PathBuf};

use crate::Value;
pub use crate::functions::{Settle, This};

pub use crate::value::FnHandle;

/// One step of a path inside a data file's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathPart {
    Key(String),
    Index(usize),
}

/// What the worker's encoding says of a plain object it refused for holding
/// properties templates could see but Brrrty can't list.
pub const UNLISTED_OBJECT: &str = "an object with properties Brrrty can't list";

/// Whether a refusal is of a plain object, which 11ty merges as data, as
/// opposed to a value 11ty ignores as a data file's whole result.
pub fn refused_plain_object(what: &str) -> bool {
    what.starts_with(UNLISTED_OBJECT)
}

/// A path inside a data file's value, shown as `data.posts[3].date`.
pub fn path_text(path: &[PathPart]) -> String {
    // A very deep path keeps its first and last parts.
    const KEEP: usize = 6;
    let mut out = String::from("data");
    for (n, part) in path.iter().enumerate() {
        if path.len() > 3 * KEEP && n >= KEEP && n < path.len() - KEEP {
            if n == KEEP {
                out.push_str(&format!("<{} levels>", path.len() - 2 * KEEP));
            }
            continue;
        }
        match part {
            PathPart::Index(i) => out.push_str(&format!("[{i}]")),
            PathPart::Key(k) => {
                let plain = k
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
                    && k.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
                if plain {
                    out.push('.');
                    out.push_str(k);
                } else {
                    out.push('[');
                    out.push_str(&serde_json::Value::String(k.clone()).to_string());
                    out.push(']');
                }
            }
        }
    }
    out
}

/// A value from the runtime. A function is a [`Value::Function`] in
/// `value`; a refused value is [`Value::Undefined`]. Both are listed by
/// path.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub value: Value,
    /// Every function value, by its path from the value's root.
    pub functions: Vec<(Vec<PathPart>, FnHandle)>,
    /// Every value that can't cross exactly, by path, with what it is
    /// ("a Map object").
    pub refused: Vec<(Vec<PathPart>, String)>,
}

impl Decoded {
    /// True when the whole value crossed: no functions and nothing refused.
    pub fn is_exact(&self) -> bool {
        self.functions.is_empty() && self.refused.is_empty()
    }
}

/// An exception thrown (or a promise rejected) by the site's code.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JsError {
    /// `TypeError`, or empty when a non-Error value was thrown.
    pub name: String,
    pub message: String,
    pub stack: String,
    /// Where the error is reported: the first stack frame inside the
    /// project and outside `node_modules`, else the first frame.
    pub file: Option<PathBuf>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// 11ty's own refusal of a JavaScript template ("No JavaScript template
    /// returned from ..."), not the site's code throwing.
    pub invalid: bool,
}

/// Why the runtime can't go on at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFailure {
    /// What happened, in one line (`Node exited with status 3`).
    pub message: String,
    /// The last lines Node wrote to stderr.
    pub stderr_tail: Vec<String>,
    /// Brrrty stopped the runtime itself (Ctrl-C, SIGTERM): not a failure
    /// of the data.
    pub stopped: bool,
    /// The runtime asks for the build to be run again: it took a shortcut
    /// it can't stand behind for this site, and the next runtime won't.
    pub again: bool,
}

/// The outcome of one request: a value, the site's own exception, a value
/// Brrrty refused to hand on, or a runtime that stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum JsResult<T> {
    Ok(T),
    Thrown(JsError),
    /// The request ran, but its outcome can't be used exactly.
    Refused(Refusal),
    Failed(RuntimeFailure),
}

/// Why a call's outcome can't be used exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The reason, naming the function: it reads what Brrrty can't hand
    /// it, changes its arguments, or returns something that can't cross.
    pub why: String,
    /// The function gave different results for the same data: it keeps
    /// state between calls, so any page's result from it may differ from
    /// 11ty's.
    pub impure: bool,
}

impl Refusal {
    pub fn new(why: impl Into<String>) -> Refusal {
        Refusal {
            why: why.into(),
            impure: false,
        }
    }
}

/// Runs one of Brrrty's filters for a JavaScript template's `this`:
/// `(name, arguments)`. `Err` refuses the render, with why.
pub type FilterCall<'a> = dyn FnMut(&str, &[Value]) -> Result<Value, String> + 'a;

/// Brrrty's filters by name, shared: see [`JsRuntime::set_filters`].
pub type Filters = std::sync::Arc<dyn Fn(&str, &[Value]) -> Result<Value, String> + Send + Sync>;

/// A JavaScript template or layout (`.11ty.js`) loaded in the runtime: its
/// instance stays there, by this id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TemplateId(pub u64);

/// What loading a JavaScript template gave: its id, and its data (what
/// 11ty takes as its front matter).
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedTemplate {
    pub id: TemplateId,
    pub data: Decoded,
}

/// An operation the pages make while they build, for [`JsRuntime::begin`].
#[derive(Debug, Clone, Copy)]
pub enum Op<'a> {
    /// [`JsRuntime::call`].
    Call {
        f: FnHandle,
        this: This<'a>,
        args: &'a [Value],
        settle: Settle,
    },
    /// [`JsRuntime::page_once`].
    PageOnce {
        data: &'a Value,
        traps: &'a [(String, String)],
        defaults: &'a [String],
        f: FnHandle,
    },
    /// [`JsRuntime::page_open_with_vars`]: the results are the open's and
    /// each function's list, or the one that failed.
    PageOpenWithVars {
        data: &'a Value,
        traps: &'a [(String, String)],
        defaults: &'a [String],
        functions: &'a [FnHandle],
        keys: &'a [String],
    },
    /// [`JsRuntime::page_run`].
    PageRun {
        page: PageId,
        steps: &'a [PageStep<'a>],
    },
    /// [`JsRuntime::render`].
    Render {
        template: TemplateId,
        data: &'a Value,
        first: Option<&'a Value>,
        traps: &'a [(String, String)],
    },
}

/// An operation begun (see [`JsRuntime::begin`]).
pub enum Begun {
    /// It is over already.
    Done(Finished),
    /// It was sent: wait for it without holding the runtime, then hand
    /// what came to [`JsRuntime::finish`].
    Sent(Box<dyn Pending>),
}

/// An operation on its way.
pub trait Pending: Send {
    /// Wait for it, answering the runtime's questions (a filter a
    /// JavaScript template calls on `this`) with `filters`.
    fn wait(self: Box<Self>, filters: &mut FilterCall<'_>) -> Waited;
}

/// What came of a [`Pending`] operation, for the runtime that sent it.
pub struct Waited(pub Box<dyn std::any::Any + Send>);

/// How an operation ended.
pub enum Finished {
    /// Its results, and the page it opened.
    Results {
        results: Vec<JsResult<Decoded>>,
        page: Option<PageId>,
    },
    /// The runtime lost the values it referred to before it ran (another
    /// page's call changed one): begin it again.
    Again,
}

/// One step of a page's computed data, for [`JsRuntime::page_run`].
#[derive(Debug, Clone, Copy)]
pub enum PageStep<'a> {
    /// Call the function with the page's data, and set its result at the
    /// dotted key.
    Call(FnHandle, Option<&'a str>),
    /// Set the dotted key to the value.
    Set(&'a str, &'a Value),
}

/// One page's data, kept in the runtime while its computed values run, as
/// 11ty keeps one data object per page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageId(pub u64);

/// A JavaScript runtime that loads data files and calls the functions in
/// them, one request at a time.
pub trait JsRuntime: Send {
    /// The data 11ty hands an exported function (`addGlobalData` values and
    /// `eleventy`), sent once before any file loads.
    fn set_argument(&mut self, arg: &Value) -> JsResult<()>;
    /// Import `file` (absolute) as 11ty does; when `call` and the export is
    /// a function, call it with the argument and await it.
    fn load(&mut self, file: &Path, call: bool) -> JsResult<Decoded>;
    /// Call a function value from a template with `this` and `args`.
    fn call(
        &mut self,
        f: FnHandle,
        this: This<'_>,
        args: &[Value],
        settle: Settle,
    ) -> JsResult<Decoded>;
    /// Keep `data` as a page's data object. Each of `traps` (a top-level
    /// key and why) is a key Brrrty can't supply: reading it refuses the
    /// call. Each of `defaults` (a dotted computed key) that is undefined
    /// is set to `""`, as 11ty's computed data proxy does.
    fn page_open(
        &mut self,
        data: &Value,
        traps: &[(String, String)],
        defaults: &[String],
    ) -> JsResult<PageId>;
    /// The keys `f` reads of the page's data, as 11ty's
    /// `ComputedDataProxy.findVarsUsed` finds them (`keys` are the page's
    /// computed keys).
    fn page_vars(&mut self, page: PageId, f: FnHandle, keys: &[String]) -> JsResult<Vec<String>>;
    /// Call `f` with the page's data, as 11ty calls a computed function or
    /// a permalink function, awaiting it; with `set`, store the result at
    /// that dotted key of the page's data.
    fn page_call(&mut self, page: PageId, f: FnHandle, set: Option<&str>) -> JsResult<Decoded>;
    /// Set a dotted key of the page's data, as 11ty's `lodash.set` does.
    fn page_set(&mut self, page: PageId, key: &str, value: &Value) -> JsResult<()>;
    /// Forget the page's data.
    fn page_close(&mut self, page: PageId);
    /// Begin an operation. A runtime that can run several at once sends it
    /// and gives back [`Begun::Sent`]: the render threads then wait for
    /// their results at the same time, and the runtime never waits on
    /// Brrrty between one request and the next. Any other runs it here.
    fn begin(&mut self, op: Op<'_>, filters: &mut FilterCall<'_>) -> Begun {
        let results = match op {
            Op::Call {
                f,
                this,
                args,
                settle,
            } => vec![self.call(f, this, args, settle)],
            Op::PageOnce {
                data,
                traps,
                defaults,
                f,
            } => vec![self.page_once(data, traps, defaults, f)],
            Op::PageOpenWithVars {
                data,
                traps,
                defaults,
                functions,
                keys,
            } => {
                return Begun::Done(
                    match self.page_open_with_vars(data, traps, defaults, functions, keys) {
                        JsResult::Ok((page, found)) => Finished::Results {
                            results: std::iter::once(Value::Null)
                                .chain(found.into_iter().map(|list| {
                                    Value::Array(std::sync::Arc::new(
                                        list.into_iter().map(|s| Value::String(s.into())).collect(),
                                    ))
                                }))
                                .map(|value| {
                                    JsResult::Ok(Decoded {
                                        value,
                                        functions: Vec::new(),
                                        refused: Vec::new(),
                                    })
                                })
                                .collect(),
                            page: Some(page),
                        },
                        JsResult::Thrown(e) => Finished::Results {
                            results: vec![JsResult::Thrown(e)],
                            page: None,
                        },
                        JsResult::Refused(r) => Finished::Results {
                            results: vec![JsResult::Refused(r)],
                            page: None,
                        },
                        JsResult::Failed(f) => Finished::Results {
                            results: vec![JsResult::Failed(f)],
                            page: None,
                        },
                    },
                );
            }
            Op::PageRun { page, steps } => self.page_run(page, steps),
            Op::Render {
                template,
                data,
                first,
                traps,
            } => vec![self.render(template, data, first, traps, filters)],
        };
        Begun::Done(Finished::Results {
            results,
            page: None,
        })
    }
    /// What came of an operation [`JsRuntime::begin`] sent.
    fn finish(&mut self, _waited: Waited) -> Finished {
        Finished::Results {
            results: vec![JsResult::Failed(RuntimeFailure {
                message: "this runtime sends nothing ahead".into(),
                stderr_tail: Vec::new(),
                stopped: false,
                again: false,
            })],
            page: None,
        }
    }
    /// [`JsRuntime::page_open`], one [`JsRuntime::page_call`] of `f` with
    /// nothing set, and [`JsRuntime::page_close`]: a permalink function
    /// called for a page with no computed data open.
    fn page_once(
        &mut self,
        data: &Value,
        traps: &[(String, String)],
        defaults: &[String],
        f: FnHandle,
    ) -> JsResult<Decoded> {
        let page = match self.page_open(data, traps, defaults) {
            JsResult::Ok(p) => p,
            JsResult::Thrown(e) => return JsResult::Thrown(e),
            JsResult::Refused(r) => return JsResult::Refused(r),
            JsResult::Failed(f) => return JsResult::Failed(f),
        };
        let r = self.page_call(page, f, None);
        self.page_close(page);
        r
    }
    /// [`JsRuntime::page_open`], then [`JsRuntime::page_vars`] of each of
    /// `functions` in order: the page, and what each reads. When one of
    /// them gives no list, the page is closed and that is the result.
    fn page_open_with_vars(
        &mut self,
        data: &Value,
        traps: &[(String, String)],
        defaults: &[String],
        functions: &[FnHandle],
        keys: &[String],
    ) -> JsResult<(PageId, Vec<Vec<String>>)> {
        let page = match self.page_open(data, traps, defaults) {
            JsResult::Ok(p) => p,
            JsResult::Thrown(e) => return JsResult::Thrown(e),
            JsResult::Refused(r) => return JsResult::Refused(r),
            JsResult::Failed(f) => return JsResult::Failed(f),
        };
        let mut found = Vec::with_capacity(functions.len());
        for f in functions {
            match self.page_vars(page, *f, keys) {
                JsResult::Ok(list) => found.push(list),
                other => {
                    self.page_close(page);
                    return match other {
                        JsResult::Thrown(e) => JsResult::Thrown(e),
                        JsResult::Refused(r) => JsResult::Refused(r),
                        JsResult::Failed(f) => JsResult::Failed(f),
                        JsResult::Ok(_) => unreachable!("handled above"),
                    };
                }
            }
        }
        JsResult::Ok((page, found))
    }
    /// The `steps` in order, each as [`JsRuntime::page_call`] or
    /// [`JsRuntime::page_set`] would run it. The list ends with the first
    /// result that isn't a value (a runtime may have run the steps after
    /// it all the same: nothing is made of them).
    fn page_run(&mut self, page: PageId, steps: &[PageStep<'_>]) -> Vec<JsResult<Decoded>> {
        let mut out = Vec::with_capacity(steps.len());
        for step in steps {
            let r = match step {
                PageStep::Call(f, set) => self.page_call(page, *f, *set),
                PageStep::Set(key, value) => match self.page_set(page, key, value) {
                    JsResult::Ok(()) => JsResult::Ok(Decoded {
                        value: Value::Undefined,
                        functions: Vec::new(),
                        refused: Vec::new(),
                    }),
                    JsResult::Thrown(e) => JsResult::Thrown(e),
                    JsResult::Refused(r) => JsResult::Refused(r),
                    JsResult::Failed(f) => JsResult::Failed(f),
                },
            };
            let stop = !matches!(r, JsResult::Ok(_));
            out.push(r);
            if stop {
                break;
            }
        }
        out
    }
    /// Every file the site's code read or loaded so far, absolute.
    fn reads(&mut self) -> JsResult<Vec<PathBuf>>;
    /// The global data files are loaded: each of their values that a later
    /// one changed (by its file), as it is now, which is what 11ty's pages
    /// see.
    fn loads_changed(&mut self) -> JsResult<Vec<(PathBuf, Decoded)>>;
    /// The data files are loaded; what follows are the pages' calls.
    /// `Refused` when a template or directory data file changed a value
    /// another file gave, or a built-in prototype.
    fn begin_calls(&mut self) -> JsResult<()>;
    /// Brrrty's filters, which JavaScript calls on `this` (a template, a
    /// computed or permalink function): `(name, arguments)`.
    fn set_filters(&mut self, filters: Filters);
    /// The names 11ty puts on a JavaScript template's `this` as functions
    /// (its filters and shortcodes), in its order. Sent before any template
    /// loads.
    fn template_setup(&mut self, filters: &[String]) -> JsResult<()>;
    /// Import the JavaScript template or layout `file` (absolute) as 11ty
    /// does, make its instance, and read its `data`.
    fn load_template(&mut self, file: &Path, layout: bool) -> JsResult<LoadedTemplate>;
    /// Render a loaded template with the page's `data` (for a layout, with
    /// `content`), as 11ty's JavaScript engine does: `first` is the `page`
    /// 11ty's first render of it set as `this.page`. Each of `traps` (a
    /// top-level key and why) is a key Brrrty can't supply. A filter the
    /// template calls on `this` runs as `filters(name, arguments)`; `Err`
    /// refuses the render with why.
    fn render(
        &mut self,
        template: TemplateId,
        data: &Value,
        first: Option<&Value>,
        traps: &[(String, String)],
        filters: &mut FilterCall<'_>,
    ) -> JsResult<Decoded>;
    /// Check that no call's result depended on the calls made before it:
    /// run the same calls again, in fresh runtimes, in other orders, and
    /// compare every result. `Refused` (marked impure) names the first
    /// difference.
    fn verify(&mut self) -> JsResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_read_like_javascript() {
        let p = [
            PathPart::Key("posts".into()),
            PathPart::Index(3),
            PathPart::Key("a b".into()),
            PathPart::Key("$ok_1".into()),
        ];
        assert_eq!(path_text(&p), r#"data.posts[3]["a b"].$ok_1"#);
        assert_eq!(path_text(&[]), "data");
    }

    #[test]
    fn a_very_deep_path_keeps_its_ends() {
        let p: Vec<PathPart> = (0..40).map(|_| PathPart::Key("c".into())).collect();
        assert_eq!(path_text(&p), "data.c.c.c.c.c.c<28 levels>.c.c.c.c.c.c");
    }
}
