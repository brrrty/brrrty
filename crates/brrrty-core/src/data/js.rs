//! JavaScript data files run in the site's own Node (`config.set_js_data("node")`):
//! one runtime per build, started when the first file needs it, sent 11ty's
//! data argument once, then asked for each file in build order.

use std::path::{Path, PathBuf};

use brrrty_js::Value;
use brrrty_js::runtime::{Decoded, JsError, JsResult, JsRuntime, PathPart, path_text};

use crate::options::{JsData, JsNodeMissing, JsOptions, RunMode};
use crate::problem::{Problem, codes};

/// What happened to one JavaScript data file in a build, for doctor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsFileState {
    /// It ran in Node.
    Ran,
    /// It ran and threw.
    Threw,
    /// It didn't run: JavaScript data is off, or there is no Node.
    NotRun,
    /// A `.rhai` port stands in for it; not compared.
    Ported,
    /// A `.rhai` port stands in for it, and gives the same value.
    PortedIdentical,
    /// A `.rhai` port stands in for it, and differs first at this path.
    PortedDiffers(String),
}

/// One build's JavaScript runtime.
pub(crate) struct JsSession<'a> {
    options: &'a JsOptions,
    run_mode: RunMode,
    root: PathBuf,
    /// What 11ty hands an exported function.
    argument: Value,
    runtime: Option<Box<dyn JsRuntime>>,
    /// The runtime stopped (or never started): the build stops with this.
    pub failed: Option<Problem>,
    /// How many files were loaded, for the report.
    pub loads: usize,
    /// Each JavaScript data file seen, and what happened to it.
    pub files: Vec<(PathBuf, JsFileState)>,
    /// Run a ported file too and compare it with its port (doctor).
    pub check_ports: bool,
}

/// What loading one file gave.
pub(crate) enum JsLoad {
    /// Its value, maybe with parts that can't cross.
    Value(Decoded),
    /// It threw: the problem names the file and where.
    Thrown(Problem),
    /// No runtime: JavaScript data is off, Node is missing, or the runtime
    /// stopped. The file is treated as before 0.0.22.
    Unavailable,
}

impl<'a> JsSession<'a> {
    pub fn new(options: &'a JsOptions, run_mode: RunMode, root: &Path, argument: Value) -> Self {
        JsSession {
            options,
            run_mode,
            root: root.to_path_buf(),
            argument,
            runtime: None,
            failed: None,
            loads: 0,
            files: Vec::new(),
            check_ports: false,
        }
    }

    /// Whether files can run: on, with a Node, and not stopped.
    pub fn on(&self) -> bool {
        self.options.mode == JsData::Node && self.options.start.is_some() && self.failed.is_none()
    }

    /// Why files don't run when JavaScript data is on: no Node, or too old.
    pub fn missing(&self) -> Option<&JsNodeMissing> {
        match (&self.options.mode, &self.options.node) {
            (JsData::Node, Some(Err(m))) => Some(m),
            _ => None,
        }
    }

    fn runtime(&mut self) -> Option<&mut Box<dyn JsRuntime>> {
        if self.runtime.is_none() {
            if !self.on() {
                return None;
            }
            let start = self.options.start.as_ref()?;
            match start(self.run_mode) {
                Ok(mut rt) => match rt.set_argument(&self.argument) {
                    JsResult::Ok(()) => self.runtime = Some(rt),
                    JsResult::Thrown(e) => {
                        self.failed = Some(worker_failed(&format!(
                            "Node refused the data argument: {}",
                            e.message
                        )));
                        return None;
                    }
                    JsResult::Failed(f) => {
                        self.failed = Some(worker_failed_with(&f.message, &f.stderr_tail));
                        return None;
                    }
                },
                Err(why) => {
                    self.failed = Some(worker_failed(&why));
                    return None;
                }
            }
        }
        self.runtime.as_mut()
    }

    /// Record what happened to `shown`: a later state replaces an earlier
    /// one only when it says more (a throw over a run, a comparison over a
    /// bare port).
    pub fn record(&mut self, shown: &Path, state: JsFileState) {
        match self.files.iter_mut().find(|(f, _)| f == shown) {
            Some((_, s)) => {
                let better = matches!(
                    (&*s, &state),
                    (JsFileState::Ran, JsFileState::Threw)
                        | (
                            JsFileState::Ported,
                            JsFileState::PortedIdentical | JsFileState::PortedDiffers(_),
                        )
                );
                if better {
                    *s = state;
                }
            }
            None => self.files.push((shown.to_path_buf(), state)),
        }
    }

    /// Import `full` (shown as `shown`); call its export when it is a
    /// function and `call` is set.
    pub fn load(&mut self, full: &Path, shown: &Path, call: bool) -> JsLoad {
        let r = self.load_inner(full, shown, call);
        let state = match &r {
            JsLoad::Value(_) => JsFileState::Ran,
            JsLoad::Thrown(_) => JsFileState::Threw,
            JsLoad::Unavailable => JsFileState::NotRun,
        };
        self.record(shown, state);
        r
    }

    /// Compare a ported file with the value of its port (doctor only).
    pub fn check_port(&mut self, full: &Path, shown: &Path, port: Option<&Value>) {
        if !self.check_ports || !self.on() {
            self.record(shown, JsFileState::Ported);
            return;
        }
        let state = match (self.load_inner(full, shown, true), port) {
            (JsLoad::Value(d), Some(port)) => match first_inexact(&d) {
                Some((at, _)) => JsFileState::PortedDiffers(path_text(&at)),
                None => match first_difference(&d.value, port, &mut Vec::new()) {
                    None => JsFileState::PortedIdentical,
                    Some((at, how)) => {
                        JsFileState::PortedDiffers(format!("{} ({how})", path_text(&at)))
                    }
                },
            },
            _ => JsFileState::Ported,
        };
        self.record(shown, state);
    }

    fn load_inner(&mut self, full: &Path, shown: &Path, call: bool) -> JsLoad {
        let Some(rt) = self.runtime() else {
            return JsLoad::Unavailable;
        };
        let full = full.canonicalize().unwrap_or_else(|_| full.to_path_buf());
        let r = rt.load(&full, call);
        self.loads += 1;
        match r {
            JsResult::Ok(d) => JsLoad::Value(d),
            JsResult::Thrown(e) => JsLoad::Thrown(thrown_problem(&e, shown, &self.root)),
            JsResult::Failed(f) => {
                self.failed = Some(worker_failed_with(&f.message, &f.stderr_tail).with_path(shown));
                self.runtime = None;
                JsLoad::Unavailable
            }
        }
    }

    /// Every file the data's code read or loaded, relative to the project
    /// folder where it is inside it.
    pub fn reads(&mut self) -> Vec<PathBuf> {
        let root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        let hooks = matches!(&self.options.node, Some(Ok(n)) if n.load_hooks);
        let Some(rt) = self.runtime.as_mut() else {
            return Vec::new();
        };
        let mut out: Vec<PathBuf> = match rt.reads() {
            JsResult::Ok(paths) => paths
                .into_iter()
                .map(|p| p.strip_prefix(&root).map(Path::to_path_buf).unwrap_or(p))
                .collect(),
            _ => Vec::new(),
        };
        // A Node without module load hooks (before 22.15) can't say which ES
        // modules loaded: then `node_modules` and every project script count.
        if !hooks {
            out.extend(coarse_reads(&self.root));
        }
        out
    }
}

/// The first path where two values differ, and how: `key order`, `keys`,
/// `length` or `value` (type, number with `-0` and `NaN`, string, date to the
/// millisecond).
pub(crate) fn first_difference(
    a: &Value,
    b: &Value,
    at: &mut Vec<PathPart>,
) -> Option<(Vec<PathPart>, &'static str)> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x.iter() {
                at.push(PathPart::Key(k.clone()));
                let found = match y.get(k) {
                    None => Some((at.clone(), "keys")),
                    Some(w) => first_difference(v, w, at),
                };
                at.pop();
                if found.is_some() {
                    return found;
                }
            }
            if x.len() != y.len() {
                return Some((at.clone(), "keys"));
            }
            (!x.keys().eq(y.keys())).then(|| (at.clone(), "key order"))
        }
        (Value::Array(x), Value::Array(y)) => {
            for (i, (v, w)) in x.iter().zip(y.iter()).enumerate() {
                at.push(PathPart::Index(i));
                let found = first_difference(v, w, at);
                at.pop();
                if found.is_some() {
                    return found;
                }
            }
            (x.len() != y.len()).then(|| (at.clone(), "length"))
        }
        (Value::Number(x), Value::Number(y)) => {
            let same = (x.is_nan() && y.is_nan())
                || (x == y && x.is_sign_negative() == y.is_sign_negative());
            (!same).then(|| (at.clone(), "value"))
        }
        (Value::Date(x), Value::Date(y)) => {
            (x.timestamp_millis() != y.timestamp_millis()).then(|| (at.clone(), "value"))
        }
        (x, y) => (x != y).then(|| (at.clone(), "value")),
    }
}

/// `node_modules` and every `.js`, `.cjs` and `.mjs` file in the project
/// outside it and outside dot folders, relative to `root`.
fn coarse_reads(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("node_modules")];
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(root.join(&rel)) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            let path = rel.join(&*name);
            match e.file_type() {
                Ok(t) if t.is_dir() => {
                    if name != "node_modules" && !name.starts_with('.') {
                        stack.push(path);
                    }
                }
                Ok(t) if t.is_file() && super::rhai::is_js(&name) => out.push(path),
                _ => {}
            }
        }
    }
    if !root.join("node_modules").exists() {
        out.remove(0);
    }
    out
}

/// The problem for a runtime that stopped: the build can't go on.
pub(crate) fn worker_failed(why: &str) -> Problem {
    Problem::error(codes::JS_WORKER_FAILED, format!("JavaScript data stopped: {why}"))
        .with_help("Brrrty stopped the build. This is a bug in Brrrty when your data files did nothing unusual; please report it.")
}

fn worker_failed_with(why: &str, stderr: &[String]) -> Problem {
    let mut p = worker_failed(why);
    if !stderr.is_empty() {
        p.message.push_str("\nNode's last lines:\n");
        p.message.push_str(&stderr.join("\n"));
    }
    p.help = Some(
        "A data file that calls process.exit() or writes straight to file descriptor 1 stops the build; otherwise this is a bug in Brrrty, please report it."
            .into(),
    );
    p
}

/// The problem for a data file that threw, at the place the error names
/// (relative to the project) when it has one, else at the data file.
pub(crate) fn thrown_problem(e: &JsError, shown: &Path, root: &Path) -> Problem {
    let mut head = if e.name.is_empty() {
        e.message.clone()
    } else {
        format!("{}: {}", e.name, e.message)
    };
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let place = e.file.as_ref().map(|f| {
        let f = f.canonicalize().unwrap_or_else(|_| f.clone());
        f.strip_prefix(&root).map(Path::to_path_buf).unwrap_or(f)
    });
    let p = match (&place, e.line) {
        (Some(file), Some(line)) => {
            if file != shown {
                head.push_str(&format!(" (loading {})", shown.display()));
            }
            Problem::error(codes::JS_DATA_ERROR, head)
                .with_path(file)
                .at(line as usize, e.column.unwrap_or(1) as usize)
        }
        _ => Problem::error(codes::JS_DATA_ERROR, head).with_path(shown),
    };
    // Node's and the worker's own frames say nothing to the site's author;
    // the rest do, with paths inside the project shown relative to it.
    let prefix = format!("{}/", root.display());
    let url_prefix = format!("file://{prefix}");
    let frames: Vec<String> = e
        .stack
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("at ") && !l.contains("node:") && !l.contains("[eval"))
        .take(5)
        .map(|l| l.replace(&url_prefix, "").replace(&prefix, ""))
        .collect();
    let mut help = String::from("11ty stops the whole build on this file. Fix the data file.");
    if !frames.is_empty() {
        help.push_str("\nStack:\n  ");
        help.push_str(&frames.join("\n  "));
    }
    p.with_help(help)
}

/// The problem for a page that reads a key whose JavaScript value can't
/// cross exactly.
pub(crate) fn value_problem(keys: &str, file: &Path, at: &[PathPart], what: &str) -> Problem {
    Problem::error(
        codes::JS_DATA_VALUE,
        format!(
            "this page uses {keys} from {}, whose {} is {what}, which can't cross from JavaScript to Brrrty exactly",
            file.display(),
            path_text(at)
        ),
    )
    .with_help("Give the data file plain values there: objects, arrays, strings, numbers, booleans, null or Dates. For a luxon DateTime, use .toJSDate() or a formatted string.")
}

/// The first part of a decoded value that can't cross, and what it is: a
/// refused value, or a function read as a value.
pub(crate) fn first_inexact(d: &Decoded) -> Option<(Vec<PathPart>, String)> {
    let refused = d.refused.first().cloned();
    let function = d
        .functions
        .first()
        .map(|(p, _)| (p.clone(), "a function".to_string()));
    match (refused, function) {
        (Some(r), Some(f)) => Some(if r.0.len() <= f.0.len() { r } else { f }),
        (r, f) => r.or(f),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn differences_are_found_at_their_path() {
        let v = |j: serde_json::Value| Value::from_json(&j);
        let at = |a: &Value, b: &Value| {
            first_difference(a, b, &mut Vec::new())
                .map(|(p, how)| format!("{} {how}", path_text(&p)))
        };
        let a = v(serde_json::json!({"a": [1, {"b": 2}], "c": "x"}));
        assert_eq!(at(&a, &a.clone()), None);
        let b = v(serde_json::json!({"a": [1, {"b": 3}], "c": "x"}));
        assert_eq!(at(&a, &b), Some("data.a[1].b value".into()));
        let b = v(serde_json::json!({"c": "x", "a": [1, {"b": 2}]}));
        assert_eq!(at(&a, &b), Some("data key order".into()));
        let b = v(serde_json::json!({"a": [1], "c": "x"}));
        assert_eq!(at(&a, &b), Some("data.a length".into()));
        let b = v(serde_json::json!({"a": [1, {"b": 2}], "c": "x", "d": 1}));
        assert_eq!(at(&a, &b), Some("data keys".into()));
        let z = at(&Value::Number(-0.0), &Value::Number(0.0));
        assert_eq!(z, Some("data value".into()));
        assert_eq!(at(&Value::Number(f64::NAN), &Value::Number(f64::NAN)), None);
    }

    #[test]
    fn without_load_hooks_node_modules_and_every_project_script_count() {
        let t = tempfile::tempdir().unwrap();
        for f in [
            "a.js",
            "lib/b.mjs",
            "lib/c.cjs",
            "lib/d.json",
            "node_modules/x/index.js",
            ".git/hooks/e.js",
        ] {
            let p = t.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let mut got = coarse_reads(t.path());
        got.sort();
        assert_eq!(
            got,
            ["a.js", "lib/b.mjs", "lib/c.cjs", "node_modules"]
                .iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        );
    }
}
