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
