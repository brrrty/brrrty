//! JavaScript data files run in the site's own Node (`config.set_js_data("node")`):
//! one runtime per build, started when the first file needs it, sent 11ty's
//! data argument once, then asked for each file in build order.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use brrrty_js::functions::{Caller, Settle, This};
use brrrty_js::runtime::{
    Decoded, JsError, JsResult, JsRuntime, LoadedTemplate, PageId, PathPart, Refusal,
    RuntimeFailure, TemplateId, path_text,
};
use brrrty_js::{FnHandle, Value};

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
    /// The names on a JavaScript `this` (11ty's filters and shortcodes),
    /// and Brrrty's filters that run them: given to the runtime when it
    /// starts.
    pub filters: Option<(Vec<String>, JsFilters)>,
    /// Set when the runtime asks for the build to be run again (see
    /// [`RuntimeFailure::again`]).
    pub again: Arc<std::sync::atomic::AtomicBool>,
}

/// What loading one JavaScript template or layout gave.
pub(crate) enum JsTemplateLoad {
    Loaded(LoadedTemplate),
    /// 11ty refuses it, or its code threw: the problem names the file.
    Failed(Problem),
    /// No runtime: JavaScript is off, Node is missing, or the runtime
    /// stopped.
    Unavailable,
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
            filters: None,
            again: Arc::default(),
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
                    JsResult::Ok(()) => {
                        if let Some((names, f)) = &self.filters {
                            rt.set_filters(f.clone());
                            match rt.template_setup(names) {
                                JsResult::Ok(()) => {}
                                JsResult::Failed(f) => {
                                    self.failed = Some(worker_failed_with(&f));
                                    return None;
                                }
                                JsResult::Thrown(JsError { message: why, .. })
                                | JsResult::Refused(Refusal { why, .. }) => {
                                    self.failed = Some(worker_failed(&why));
                                    return None;
                                }
                            }
                        }
                        self.runtime = Some(rt);
                    }
                    JsResult::Thrown(JsError { message: why, .. })
                    | JsResult::Refused(Refusal { why, .. }) => {
                        self.failed = Some(worker_failed(&format!(
                            "Node refused the data argument: {why}"
                        )));
                        return None;
                    }
                    JsResult::Failed(f) => {
                        self.failed = Some(worker_failed_with(&f));
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
            JsResult::Refused(r) => {
                JsLoad::Thrown(Problem::error(codes::JS_DATA_VALUE, r.why).with_path(shown))
            }
            JsResult::Failed(f) => {
                self.failed = Some(worker_failed_with(&f).with_path(shown));
                self.runtime = None;
                JsLoad::Unavailable
            }
        }
    }

    /// Load the JavaScript template or layout `full` (shown as `shown`):
    /// its instance stays in the runtime, its data comes back.
    pub fn load_template(&mut self, full: &Path, shown: &Path, layout: bool) -> JsTemplateLoad {
        let Some(rt) = self.runtime() else {
            return JsTemplateLoad::Unavailable;
        };
        let full = full.canonicalize().unwrap_or_else(|_| full.to_path_buf());
        let r = rt.load_template(&full, layout);
        self.loads += 1;
        match r {
            JsResult::Ok(t) => JsTemplateLoad::Loaded(t),
            JsResult::Thrown(e) if e.invalid => JsTemplateLoad::Failed(
                Problem::error(codes::JS_TEMPLATE_INVALID, e.message)
                    .with_path(shown)
                    .with_help("11ty stops the whole build here. Export a function, a class with `render`, or an object with `render` and `data`."),
            ),
            JsResult::Thrown(e) => JsTemplateLoad::Failed(
                thrown_problem(&e, shown, &self.root)
                    .with_help(format!(
                        "11ty stops the whole build on this template.{}",
                        stack_help(&e, &self.root)
                    )),
            ),
            JsResult::Refused(r) => JsTemplateLoad::Failed(
                Problem::error(codes::JS_DATA_VALUE, r.why).with_path(shown),
            ),
            JsResult::Failed(f) => {
                self.failed = Some(worker_failed_with(&f).with_path(shown));
                self.runtime = None;
                JsTemplateLoad::Unavailable
            }
        }
    }

    /// The global data files' values a later file changed, as they are now
    /// (absolute file, value): what 11ty's pages see.
    pub fn loads_changed(&mut self) -> Vec<(PathBuf, Decoded)> {
        let Some(rt) = self.runtime.as_mut() else {
            return Vec::new();
        };
        match rt.loads_changed() {
            JsResult::Ok(list) => list,
            JsResult::Failed(f) => {
                self.failed = Some(worker_failed_with(&f));
                self.runtime = None;
                Vec::new()
            }
            JsResult::Thrown(e) => {
                self.failed = Some(worker_failed(&e.message));
                Vec::new()
            }
            JsResult::Refused(r) => {
                self.failed = Some(worker_failed(&r.why));
                Vec::new()
            }
        }
    }

    /// Hand the runtime over for the pages to call the functions in the
    /// data: `None` when no file ran.
    pub fn take_calls(&mut self) -> Option<Arc<JsCalls>> {
        let mut runtime = self.runtime.take()?;
        match runtime.begin_calls() {
            JsResult::Ok(()) => {}
            JsResult::Failed(f) => {
                self.failed = Some(worker_failed_with(&f));
                return None;
            }
            JsResult::Refused(Refusal { why, .. }) => {
                self.failed = Some(
                    Problem::error(codes::JS_DATA_IMPURE, why).with_help(
                        "Change a copy instead: a data file that sorts, adds to or sets something on another file's value gives pages that depend on the order 11ty loads files. Nothing was written.",
                    ),
                );
                return None;
            }
            JsResult::Thrown(JsError { message: why, .. }) => {
                self.failed = Some(worker_failed(&why));
                return None;
            }
        }
        Some(Arc::new(JsCalls {
            runtime: Mutex::new(Some(runtime)),
            root: self.root.clone(),
            failed: Mutex::new(None),
            impure: Mutex::new(Vec::new()),
            filters: Mutex::new(None),
            again: self.again.clone(),
        }))
    }

    /// Every file the data's code read or loaded, relative to the project
    /// folder where it is inside it.
    pub fn reads(&mut self) -> Vec<PathBuf> {
        let hooks = matches!(&self.options.node, Some(Ok(n)) if n.load_hooks);
        let root = self.root.clone();
        let Some(rt) = self.runtime.as_mut() else {
            return Vec::new();
        };
        reads_of(rt.as_mut(), &root, hooks)
    }
}

fn reads_of(rt: &mut dyn JsRuntime, root: &Path, hooks: bool) -> Vec<PathBuf> {
    {
        let root_dir = root.to_path_buf();
        let root = root_dir.canonicalize().unwrap_or_else(|_| root_dir.clone());
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
            out.extend(coarse_reads(&root_dir));
        }
        out
    }
}

/// The runtime once the data files have run: the pages call the functions
/// in the data through it, from any render thread, one request at a time.
pub(crate) struct JsCalls {
    runtime: Mutex<Option<Box<dyn JsRuntime>>>,
    root: PathBuf,
    /// The runtime stopped: the build stops with this.
    failed: Mutex<Option<Problem>>,
    /// Functions that gave different results for the same data: every
    /// page's result from them may differ from 11ty's, so nothing is
    /// written.
    impure: Mutex<Vec<Problem>>,
    /// The filters a JavaScript template calls on `this`, by name.
    filters: Mutex<Option<JsFilters>>,
    /// The runtime asked for the build to be run again (see
    /// [`RuntimeFailure::again`]).
    again: Arc<std::sync::atomic::AtomicBool>,
}

/// Runs one of Brrrty's filters for a JavaScript template: `(name,
/// arguments)`, the first argument the filter's input. `Err` is why the
/// render is refused.
pub(crate) type JsFilters = brrrty_js::runtime::Filters;

thread_local! {
    static CALLS: RefCell<Option<Arc<JsCalls>>> = const { RefCell::new(None) };
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The runtime the pages on this thread call functions through, if any.
pub(crate) fn calls() -> Option<Arc<JsCalls>> {
    CALLS.with(|c| c.borrow().clone())
}

impl JsCalls {
    /// Make `calls` the runtime for this thread's pages (and its template
    /// engines), or none.
    pub fn install_here(calls: Option<Arc<JsCalls>>) {
        brrrty_js::functions::set(Some(match &calls {
            Some(c) => c.clone() as Arc<dyn Caller>,
            None => Arc::new(NoCalls),
        }));
        let before = CALLS.with(|c| std::mem::replace(&mut *c.borrow_mut(), calls));
        drop(before);
    }

    /// Why the build must stop: the runtime stopped, or a function kept
    /// state between calls.
    pub fn stop_problems(&self) -> Vec<Problem> {
        let mut out: Vec<Problem> = lock(&self.failed).iter().cloned().collect();
        out.extend(lock(&self.impure).iter().cloned());
        out
    }

    /// Run the replay check (see [`JsRuntime::verify`]): when a call's
    /// result depended on the calls before it, the build stops.
    pub fn verify(&self) {
        let r = self.with(|rt| rt.verify());
        if let JsResult::Refused(r) = r {
            let r = Refusal { impure: true, ..r };
            self.note_refusal(&r);
        }
    }

    /// Every file the data's code read so far.
    pub fn reads(&self, hooks: bool) -> Vec<PathBuf> {
        match lock(&self.runtime).as_mut() {
            Some(rt) => reads_of(rt.as_mut(), &self.root, hooks),
            None => Vec::new(),
        }
    }

    /// One request, or the problem for a runtime that already stopped.
    fn with<T>(&self, f: impl FnOnce(&mut dyn JsRuntime) -> JsResult<T>) -> JsResult<T> {
        let mut rt = lock(&self.runtime);
        let Some(r) = rt.as_mut() else {
            return JsResult::Failed(RuntimeFailure {
                message: "JavaScript data stopped earlier in this build".into(),
                stderr_tail: Vec::new(),
                stopped: false,
                again: false,
            });
        };
        let out = f(r.as_mut());
        if let JsResult::Failed(failure) = &out {
            if failure.again {
                self.again.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            *rt = None;
            let mut failed = lock(&self.failed);
            if failed.is_none() {
                *failed = Some(worker_failed_with(failure));
            }
        }
        out
    }

    fn note_refusal(&self, r: &Refusal) -> Problem {
        let code = if r.impure {
            codes::JS_DATA_IMPURE
        } else {
            codes::JS_DATA_FUNCTION
        };
        let p = Problem::error(code, r.why.clone());
        if r.impure {
            let p = p.clone().with_help(IMPURE_HELP);
            let mut list = lock(&self.impure);
            if !list.iter().any(|q| q.message == p.message) {
                list.push(p);
            }
        }
        p
    }

    fn problem<T>(&self, r: JsResult<T>, shown: &Path) -> Result<T, Problem> {
        match r {
            JsResult::Ok(v) => Ok(v),
            JsResult::Thrown(e) => Err(thrown_problem(&e, shown, &self.root).with_help(format!(
                "11ty stops the whole build here. Fix the function in your data.{}",
                stack_help(&e, &self.root)
            ))),
            JsResult::Refused(r) => Err(self.note_refusal(&r).with_path(shown)),
            JsResult::Failed(f) => Err(worker_failed_with(&f).with_path(shown)),
        }
    }

    /// Keep `data` in the runtime as the page's data object (see
    /// [`JsRuntime::page_open`]).
    pub fn page<'a>(
        &'a self,
        data: &Value,
        traps: &[(String, String)],
        defaults: &[String],
        shown: &'a Path,
    ) -> Result<JsPage<'a>, Problem> {
        let id = self.problem(self.with(|rt| rt.page_open(data, traps, defaults)), shown)?;
        Ok(JsPage {
            calls: self,
            id,
            shown,
        })
    }
}

impl JsCalls {
    /// The filters JavaScript templates call on `this`.
    pub fn set_filters(&self, filters: JsFilters) {
        *lock(&self.filters) = Some(filters);
    }

    /// Render the JavaScript template `t` with `data` (see
    /// [`JsRuntime::render`]) for the page `shown`: what its `render` gave,
    /// or why the page is refused.
    pub fn render(
        &self,
        t: TemplateId,
        data: &Value,
        first: Option<&Value>,
        traps: &[(String, String)],
        shown: &Path,
    ) -> Result<Value, Problem> {
        let filters = lock(&self.filters).clone();
        let mut run = |name: &str, args: &[Value]| match &filters {
            Some(f) => f(name, args),
            None => Err(format!("calls `this.{name}`, which Brrrty can't run here")),
        };
        let r = self.with(|rt| rt.render(t, data, first, traps, &mut run));
        let d = match r {
            JsResult::Ok(d) => d,
            JsResult::Thrown(e) => {
                return Err(thrown_problem(&e, shown, &self.root).with_help(format!(
                    "11ty stops the whole build here. Fix the template.{}",
                    stack_help(&e, &self.root)
                )));
            }
            JsResult::Refused(r) => {
                let p = self.note_refusal(&r).with_path(shown);
                return Err(if r.impure {
                    p
                } else if r.why.contains(crate::render::GAP_TRAP) {
                    // The collections can't be built: the page waits on the
                    // problem that says why.
                    p.following("Fix the problems reported above.")
                } else {
                    p.with_help(RENDER_REFUSED_HELP)
                });
            }
            JsResult::Failed(f) => return Err(worker_failed_with(&f).with_path(shown)),
        };
        if let Some(why) = returned_problem(&d) {
            return Err(Problem::error(
                codes::JS_TEMPLATE_VALUE,
                why.replacen(
                    "a function from JavaScript data",
                    "this template's `render`",
                    1,
                ),
            )
            .with_path(shown)
            .with_help("Return a string."));
        }
        if let Some((at, _)) = d.functions.first() {
            let place = if at.is_empty() {
                String::new()
            } else {
                format!(" at {}", path_text(at).replacen("data", "its result", 1))
            };
            return Err(Problem::error(
                codes::JS_TEMPLATE_VALUE,
                format!("this template's `render` returns a function{place}, which 11ty would print as JavaScript's own text for it"),
            )
            .with_path(shown)
            .with_help("Return a string."));
        }
        Ok(d.value)
    }
}

const RENDER_REFUSED_HELP: &str = "11ty runs this in its own JavaScript, where Brrrty can't follow it; the rest of the site still builds.";

const IMPURE_HELP: &str = "Brrrty loads files and builds pages in a different order than 11ty, so code whose result depends on what ran before it gives different pages. Nothing was written. Make each function, render and module return the same thing for the same input: move counters, registries and random values out of code that pages share (a helper several templates or data files import), and out of functions pages call.";

/// The stack frames of a thrown error worth showing, as a help line.
fn stack_help(e: &JsError, root: &Path) -> String {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
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
    if frames.is_empty() {
        String::new()
    } else {
        format!("\nStack:\n  {}", frames.join("\n  "))
    }
}

/// A runtime for threads with no JavaScript data: a call can't happen.
struct NoCalls;

impl Caller for NoCalls {
    fn call(&self, _: FnHandle, _: This<'_>, _: &[Value], _: Settle) -> Result<Value, String> {
        Err("this page calls a function from JavaScript data, which isn't running".into())
    }
}

/// What a page's template can't use of a value a function returned.
fn returned_problem(d: &Decoded) -> Option<String> {
    let (at, what) = d.refused.first()?;
    let place = if at.is_empty() {
        String::new()
    } else {
        format!(" at {}", path_text(at).replacen("data", "its result", 1))
    };
    let promise = if at.is_empty() && what == "a Promise object" {
        " (Nunjucks doesn't wait for a promise, so 11ty prints `[object Promise]`)"
    } else {
        ""
    };
    Some(format!(
        "a function from JavaScript data returned {what}{place}, which can't cross from JavaScript to Brrrty exactly{promise}"
    ))
}

impl Caller for JsCalls {
    fn call(
        &self,
        f: FnHandle,
        this: This<'_>,
        args: &[Value],
        settle: Settle,
    ) -> Result<Value, String> {
        match self.with(|rt| rt.call(f, this, args, settle)) {
            JsResult::Ok(d) => match returned_problem(&d) {
                Some(why) => Err(why),
                None => Ok(d.value),
            },
            JsResult::Thrown(e) => {
                let place = match (&e.file, e.line) {
                    (Some(file), Some(line)) => {
                        let root = self
                            .root
                            .canonicalize()
                            .unwrap_or_else(|_| self.root.clone());
                        let file = file.canonicalize().unwrap_or_else(|_| file.clone());
                        let shown = file.strip_prefix(&root).unwrap_or(&file);
                        format!(" ({}:{line}:{})", shown.display(), e.column.unwrap_or(1))
                    }
                    _ => String::new(),
                };
                let head = if e.name.is_empty() {
                    e.message.clone()
                } else {
                    format!("{}: {}", e.name, e.message)
                };
                Err(format!(
                    "a function from JavaScript data threw {head}{place}; 11ty stops the whole build here"
                ))
            }
            JsResult::Refused(r) => {
                self.note_refusal(&r);
                Err(r.why)
            }
            JsResult::Failed(f) => Err(format!("JavaScript data stopped: {}", f.message)),
        }
    }
}

/// A page's data object in the runtime, while its computed values and
/// permalink run. Forgotten when dropped.
pub(crate) struct JsPage<'a> {
    calls: &'a JsCalls,
    id: PageId,
    shown: &'a Path,
}

impl JsPage<'_> {
    /// The data keys `f` reads, as 11ty's computed data proxy finds them.
    pub fn vars(&mut self, f: FnHandle, keys: &[String]) -> Result<Vec<String>, Problem> {
        let id = self.id;
        self.calls
            .problem(self.calls.with(|rt| rt.page_vars(id, f, keys)), self.shown)
    }

    /// Call `f` with the page's data, storing the result at `set`.
    pub fn call(&mut self, f: FnHandle, set: Option<&str>) -> Result<Value, Problem> {
        let id = self.id;
        let d = self
            .calls
            .problem(self.calls.with(|rt| rt.page_call(id, f, set)), self.shown)?;
        match returned_problem(&d) {
            Some(why) => Err(Problem::error(codes::JS_DATA_VALUE, why)
                .with_path(self.shown)
                .with_help("Return plain values: objects, arrays, strings, numbers, booleans, null or Dates.")),
            None => Ok(d.value),
        }
    }

    /// Set a dotted key of the page's data.
    pub fn set(&mut self, key: &str, value: &Value) -> Result<(), Problem> {
        let id = self.id;
        self.calls.problem(
            self.calls.with(|rt| rt.page_set(id, key, value)),
            self.shown,
        )
    }
}

impl Drop for JsPage<'_> {
    fn drop(&mut self) {
        let id = self.id;
        let mut rt = lock(&self.calls.runtime);
        if let Some(r) = rt.as_mut() {
            r.page_close(id);
        }
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

fn worker_failed_with(f: &RuntimeFailure) -> Problem {
    let mut p = worker_failed(&f.message);
    if f.stopped {
        p.help = Some("Brrrty stopped the build on a signal (Ctrl-C or SIGTERM).".into());
        return p;
    }
    if !f.stderr_tail.is_empty() {
        p.message.push_str("\nNode's last lines:\n");
        p.message.push_str(&f.stderr_tail.join("\n"));
    }
    p.help = Some(
        "A data file that calls process.exit(), or closes descriptors it didn't open, stops the build; otherwise this is a bug in Brrrty, please report it."
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

/// The first part of a decoded data file's value a build can't use, and
/// what it is: a refused value, or a function where 11ty would read it as a
/// value. Functions are fine elsewhere: under `eleventyComputed`, as
/// `permalink`, and anywhere outside the keys that change how a page is
/// built, where templates call them.
pub(crate) fn first_unusable(d: &Decoded) -> Option<(Vec<PathPart>, String)> {
    let refused = d.refused.first().cloned();
    let function = d
        .functions
        .iter()
        .find(|(p, _)| !function_allowed(p))
        .map(|(p, _)| (p.clone(), "a function".to_string()));
    match (refused, function) {
        (Some(r), Some(f)) => Some(if r.0.len() <= f.0.len() { r } else { f }),
        (r, f) => r.or(f),
    }
}

/// Whether a function at `path` of a data file's value is one a build can
/// call: not at the root, and not inside a key that changes how a page is
/// built, except a `permalink` function and the functions of
/// `eleventyComputed`.
pub(crate) fn function_allowed(path: &[PathPart]) -> bool {
    let Some(PathPart::Key(first)) = path.first() else {
        return false;
    };
    let key = first.strip_prefix(super::OVERRIDE_PREFIX).unwrap_or(first);
    if !super::BUILD_KEYS.contains(&key) {
        return true;
    }
    key == "eleventyComputed" || (key == "permalink" && path.len() == 1)
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
    fn a_worker_brrrty_stopped_is_not_called_a_bug() {
        let mut f = RuntimeFailure {
            message: "Node exited with status 1".into(),
            stderr_tail: vec!["boom".into()],
            stopped: false,
            again: false,
        };
        let p = worker_failed_with(&f);
        let help = p.help.unwrap_or_default();
        assert!(help.contains("bug in Brrrty"), "{help}");
        assert!(!help.contains("descriptor 1"), "{help}");
        f.stopped = true;
        let p = worker_failed_with(&f);
        let help = p.help.unwrap_or_default();
        assert!(!help.contains("bug"), "{help}");
        assert!(help.contains("signal"), "{help}");
    }

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
