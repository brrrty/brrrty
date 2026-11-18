//! What crosses from a JavaScript runtime (the site's own Node, run by
//! `brrrty-node`) into a build: decoded values, function handles, errors,
//! and the trait a build drives the runtime through.

use std::path::{Path, PathBuf};

use crate::Value;

/// A function value that stayed in the runtime, to be called later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FnHandle(pub u32);

/// One step of a path inside a data file's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathPart {
    Key(String),
    Index(usize),
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

/// A value from the runtime. Functions and refused values are
/// [`Value::Undefined`] in `value`, and listed by path.
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
}

/// The outcome of one request: a value, the site's own exception, or a
/// runtime that stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum JsResult<T> {
    Ok(T),
    Thrown(JsError),
    Failed(RuntimeFailure),
}

/// A JavaScript runtime that loads data files and calls the functions in
/// them, one request at a time, in the order a build makes them.
pub trait JsRuntime: Send {
    /// The data 11ty hands an exported function (`addGlobalData` values and
    /// `eleventy`), sent once before any file loads.
    fn set_argument(&mut self, arg: &Value) -> JsResult<()>;
    /// Import `file` (absolute) as 11ty does; when `call` and the export is
    /// a function, call it with the argument and await it.
    fn load(&mut self, file: &Path, call: bool) -> JsResult<Decoded>;
    /// Call a function value with `args`, awaiting the result.
    fn call(&mut self, f: FnHandle, args: &[Value]) -> JsResult<Decoded>;
    /// The top-level data keys `f` reads, as 11ty's computed data proxy
    /// finds them.
    fn vars_used(&mut self, f: FnHandle, data: &Value) -> JsResult<Vec<String>>;
    /// Every file the site's code read or loaded so far, absolute.
    fn reads(&mut self) -> JsResult<Vec<PathBuf>>;
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
