//! One Node process running the worker, and the requests Brrrty makes of it.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use brrrty_js::Value;
use brrrty_js::runtime::{
    Decoded, FilterCall, Filters, FnHandle, JsError, JsResult, JsRuntime, LoadedTemplate, PageId,
    PathPart, Refusal, RuntimeFailure, Settle, TemplateId, This,
};
use serde_json::{Value as Json, json};

use crate::decode::decode;
use crate::find::NodeInfo;

/// The worker's source: the encoding, the computed data proxy, then the
/// protocol. Passed with `--eval`; nothing is written to disk.
const WORKER_SOURCE: &str = concat!(
    include_str!("worker/encode.mjs"),
    "\n",
    include_str!("worker/proxy.mjs"),
    "\n",
    include_str!("worker/main.mjs"),
);

/// The 11ty version the worker tells data files it is (`ELEVENTY_VERSION`).
const ELEVENTY_VERSION: &str = "3.1.8";

/// The stack of the thread that parses replies.
pub(crate) const REPLY_STACK: usize = 32 * 1024 * 1024;

/// How many of Node's last stderr lines a failure shows.
const STDERR_TAIL: usize = 20;

/// How long a stopped worker's stderr may take to reach its end: past this,
/// a process Node started still holds it, and Brrrty stops reading.
const STDERR_GRACE: Duration = Duration::from_millis(200);

/// How often a wait for a reply checks that Node still runs.
const POLL: Duration = Duration::from_millis(100);

/// The first note about a slow request, then the gap between notes.
const FIRST_NOTE: Duration = Duration::from_secs(10);
const NEXT_NOTE: Duration = Duration::from_secs(30);

/// How 11ty was started, as `ELEVENTY_RUN_MODE` shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Build,
    Serve,
}

impl RunMode {
    fn as_str(self) -> &'static str {
        match self {
            RunMode::Build => "build",
            RunMode::Serve => "serve",
        }
    }
}

/// Node couldn't be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerError(pub String);

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One reply frame, its value already decoded on the reader's thread (see
/// [`REPLY_STACK`]), or why there is none.
type Reply = Result<Frame, String>;

pub(crate) struct Frame {
    /// The reply without its value.
    pub head: Json,
    /// The decoded `ok` value, when the reply has one.
    ok: Option<Result<Decoded, String>>,
    /// The `ok` value as sent, kept when the worker records its requests
    /// for the replay check (see `JsRuntime::verify`).
    pub raw: Option<Json>,
    /// The whole reply as text, kept while the data loads: the replay
    /// check compares loads, and a deep value is safer kept as text than
    /// as a parsed value on a thread with a small stack.
    pub text: Option<Box<str>>,
}

/// What a reply's reader keeps besides the decoded value.
pub(crate) const KEEP_RAW: u8 = 1;
pub(crate) const KEEP_TEXT: u8 = 2;

impl Record {
    /// The request, parsed.
    pub fn req(&self) -> Json {
        parse_reply(self.req_text.as_bytes()).unwrap_or(Json::Null)
    }
}

/// One request that loaded data or a template, with its reply, for the
/// replay check.
#[derive(Clone)]
pub(crate) struct LoadRecord {
    pub req: Json,
    /// The whole reply, as text: parsed by the replay check, on its roomy
    /// stack.
    pub text: Box<str>,
}

/// One request made after the data files loaded, with its reply, for the
/// replay check.
#[derive(Clone)]
pub(crate) struct Record {
    /// The render that made it ([`brrrty_js::functions::unit`]); a request
    /// made outside one is a unit of its own.
    pub unit: u64,
    pub label: Arc<str>,
    /// The request as sent, as text: a parsed request takes ten times the
    /// memory, and a build keeps one per call (see [`Record::req`]).
    pub req_text: Box<str>,
    pub head: Json,
    pub raw: Option<Json>,
    /// The questions the request asked Brrrty, with the answers it got.
    pub answers: Vec<(Json, Json)>,
}

/// A running worker. Dropping it closes Node's stdin, waits up to 2 s for
/// it to exit, then kills it.
pub struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Where Brrrty answers the worker's questions (a filter a JavaScript
    /// template calls on `this`).
    answers: Option<Box<dyn Write + Send>>,
    /// The replay check's answers, as the first run gave them, in order.
    pub(crate) answers_feed: Option<VecDeque<(Json, Json)>>,
    /// The questions the request in flight asked, with their answers.
    asked: Vec<(Json, Json)>,
    /// Brrrty's filters, for the questions a request asks.
    filters: Option<Filters>,
    replies: Receiver<Reply>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    stderr_thread: Option<JoinHandle<()>>,
    root: PathBuf,
    next_id: u64,
    /// Set once the worker failed: every later request fails the same way.
    failed: Option<RuntimeFailure>,
    /// The value each load (by call mode and file) gave last.
    last_loaded: HashMap<(bool, PathBuf), Decoded>,
    /// The value a `same` reply to the request in flight stands for.
    earlier: Option<Decoded>,
    /// Values sent to the worker and kept there, by their data's address
    /// (the value is held, so the address can't be reused).
    kept: Kept,
    /// What the worker was started with, for the replay check.
    node: NodeInfo,
    run_mode: RunMode,
    /// Keep each reply's value as sent, as well as decoded.
    pub(crate) keep_raw: Arc<std::sync::atomic::AtomicU8>,
    /// The requests that loaded the data, in order, with their replies.
    load_log: Vec<LoadRecord>,
    /// How many of them loaded global data (the rest are template and
    /// directory data files and templates, which 11ty loads as their
    /// files finish loading).
    global_end: usize,
    /// The requests made since, with their replies.
    call_log: Vec<Record>,
    /// Set once the data files loaded: see [`JsRuntime::begin_calls`].
    calls_begun: bool,
    /// How many function handles the data files' values made.
    load_handles: u64,
    /// The longest a request took.
    slowest: Duration,
    /// How long a request may take before the worker is given up on (the
    /// replay check's workers only).
    pub(crate) deadline: Option<Duration>,
    /// The last reply's head, for a request whose reply says more than its
    /// value (`templateLoad`'s id).
    last_head: Json,
    /// Where slow-request notes go; stderr unless a test replaces it.
    notes: Box<dyn Fn(&str) + Send>,
    /// When the first note comes, and then how often.
    note_times: (Duration, Duration),
}

impl Worker {
    /// Start Node with the worker, in `root` (the project folder), with
    /// 11ty's environment variables added to Brrrty's own.
    pub fn start(node: &NodeInfo, root: &Path, run_mode: RunMode) -> Result<Worker, WorkerError> {
        let mut cmd = Command::new(&node.path);
        cmd.args(["--input-type=module", "--eval", WORKER_SOURCE])
            .current_dir(root)
            .env("ELEVENTY_ROOT", root)
            .env("ELEVENTY_SOURCE", "cli")
            .env("ELEVENTY_RUN_MODE", run_mode.as_str())
            .env("ELEVENTY_VERSION", ELEVENTY_VERSION)
            .stdin(Stdio::piped());
        let cannot =
            |e: std::io::Error| WorkerError(format!("cannot start {}: {e}", node.path.display()));
        let (mut child, frames, stderr, answers) = spawn(cmd).map_err(cannot)?;
        live(child.id(), true);
        let (tx, replies) = mpsc::channel();
        // Load replies are kept as text: the replay check compares them.
        let keep_raw = Arc::new(std::sync::atomic::AtomicU8::new(KEEP_TEXT));
        let keep = keep_raw.clone();
        // A reply nests up to three JSON levels per level of the value
        // (`encode.mjs` refuses values deeper than 1000), and parsing it
        // recurses once per level: a roomy stack, not serde's 128 limit.
        std::thread::Builder::new()
            .name("brrrty-node-replies".into())
            .stack_size(REPLY_STACK)
            .spawn(move || read_frames(frames, tx, keep))
            .map_err(|e| {
                let _ = child.kill();
                let _ = child.wait();
                WorkerError(format!("cannot read Node's replies: {e}"))
            })?;
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let tail = stderr_tail.clone();
        let stderr_thread = std::thread::spawn(move || drain_stderr(stderr, tail));
        Ok(Worker {
            stdin: child.stdin.take(),
            answers,
            answers_feed: None,
            asked: Vec::new(),
            filters: None,
            child,
            replies,
            stderr_tail,
            stderr_thread: Some(stderr_thread),
            root: root.to_path_buf(),
            next_id: 0,
            failed: None,
            last_loaded: HashMap::new(),
            earlier: None,
            kept: Kept::default(),
            node: node.clone(),
            run_mode,
            keep_raw,
            load_log: Vec::new(),
            global_end: 0,
            call_log: Vec::new(),
            calls_begun: false,
            load_handles: 0,
            slowest: Duration::ZERO,
            deadline: None,
            last_head: Json::Null,
            notes: Box::new(|s| eprintln!("{s}")),
            note_times: (FIRST_NOTE, NEXT_NOTE),
        })
    }

    /// Send slow-request notes to `notes` instead of stderr, the first
    /// after `first` and then every `next` (tests).
    pub fn set_notes(
        &mut self,
        notes: impl Fn(&str) + Send + 'static,
        first: Duration,
        next: Duration,
    ) {
        self.notes = Box::new(notes);
        self.note_times = (first, next);
    }

    /// Node's process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The project folder the worker runs in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Send one request and wait for its reply. `what` names it in the
    /// notes about a slow reply.
    fn request(&mut self, req: Json, what: &str) -> JsResult<Decoded> {
        let filters = self.filters.clone();
        self.request_with(
            req,
            what,
            &mut |name: &str, args: &[Value]| match &filters {
                Some(f) => f(name, args),
                None => Err(format!(
                    "calls `this.{name}` where Brrrty has no filters to run it"
                )),
            },
        )
    }

    /// [`Worker::request`], answering the worker's filter questions with
    /// `filters`.
    fn request_with(
        &mut self,
        req: Json,
        what: &str,
        filters: &mut FilterCall<'_>,
    ) -> JsResult<Decoded> {
        let op = req
            .get("op")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let logged = !matches!(
            op.as_str(),
            "reads"
                | "info"
                | "callsBegin"
                | "feedClock"
                | "feedCalls"
                | "loadsChanged"
                | "exportsFinal"
                | "describe"
        );
        self.asked.clear();
        let frame = match self.exchange_with(req.clone(), what, filters) {
            Ok(f) => f,
            Err(f) => return JsResult::Failed(f),
        };
        if logged && !self.calls_begun {
            self.load_log.push(LoadRecord {
                req: req.clone(),
                text: frame.text.clone().unwrap_or_default(),
            });
        }
        if logged && self.calls_begun {
            let (unit, label) = match brrrty_js::functions::unit() {
                Some(u) => u,
                None => (u64::MAX - self.call_log.len() as u64, Arc::from("")),
            };
            self.call_log.push(Record {
                unit,
                label,
                req_text: req.to_string().into_boxed_str(),
                head: frame.head.clone(),
                raw: frame.raw.clone(),
                answers: std::mem::take(&mut self.asked),
            });
        }
        self.last_head = frame.head.clone();
        self.interpret(frame, what)
    }

    /// Send one request and wait for its reply frame.
    pub(crate) fn exchange(&mut self, req: Json, what: &str) -> Result<Frame, RuntimeFailure> {
        self.exchange_with(req, what, &mut |name: &str, _: &[Value]| {
            Err(format!(
                "calls `this.{name}` where Brrrty has no filters to run it"
            ))
        })
    }

    /// Answer one of the worker's questions: run the filter it names, or,
    /// in the replay check, give the first run's answer.
    fn answer(&mut self, frame: &Frame, filters: &mut FilterCall<'_>) -> Result<(), String> {
        let name = frame
            .head
            .get("filter")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let question = json!({"filter": name, "args": frame.raw.clone().unwrap_or(Json::Null)});
        let reply = match self.answers_feed.as_mut() {
            Some(feed) => match feed.pop_front() {
                Some((q, a)) if q == question => a,
                _ => {
                    json!({"refused": "asks for a filter the first run didn't ask for at this point"})
                }
            },
            None => {
                let args = match &frame.ok {
                    Some(Ok(d)) if d.is_exact() => match &d.value {
                        Value::Array(a) => Ok(a.to_vec()),
                        _ => Err("passes its filter something Brrrty can't read".to_string()),
                    },
                    Some(Ok(_)) => Err(format!(
                        "passes `this.{name}` a function or a value that can't cross from JavaScript to Brrrty exactly"
                    )),
                    _ => Err("passes its filter something Brrrty can't read".to_string()),
                };
                match args.and_then(|a| filters(&name, &a)) {
                    Ok(v) => json!({"ok": encode(&v)}),
                    Err(why) => json!({"refused": why}),
                }
            }
        };
        self.asked.push((question, reply.clone()));
        let mut line = reply.to_string();
        line.push('\n');
        match self.answers.as_mut() {
            Some(w) => w
                .write_all(line.as_bytes())
                .and_then(|_| w.flush())
                .map_err(|e| format!("cannot answer Node: {e}")),
            None => Err("Node asked a question it can't be answered on".to_string()),
        }
    }

    /// [`Worker::exchange`], answering the worker's filter questions with
    /// `filters` while it waits.
    pub(crate) fn exchange_with(
        &mut self,
        mut req: Json,
        what: &str,
        filters: &mut FilterCall<'_>,
    ) -> Result<Frame, RuntimeFailure> {
        if let Some(f) = &self.failed {
            return Err(f.clone());
        }
        self.next_id += 1;
        let id = self.next_id;
        req["id"] = json!(id);
        let mut line = req.to_string();
        line.push('\n');
        let sent = self
            .stdin
            .as_mut()
            .map(|s| s.write_all(line.as_bytes()).and_then(|_| s.flush()));
        if !matches!(sent, Some(Ok(()))) {
            return Err(self.fail("Node stopped reading requests"));
        }
        let start = Instant::now();
        let mut next_note = start + self.note_times.0;
        // Node exiting ends the wait even when a process it started keeps
        // the replies' pipe open.
        let mut exited_at: Option<Instant> = None;
        let reply = loop {
            match self.replies.recv_timeout(POLL) {
                Ok(Ok(f)) if f.head.get("filter").is_some() => {
                    if let Err(why) = self.answer(&f, filters) {
                        break Err(why);
                    }
                }
                Ok(r) => break r,
                Err(RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    if let Some(d) = self.deadline
                        && now.duration_since(start) > d
                    {
                        break Err(format!(
                            "{what} took more than {} s, longer than the first run allows",
                            d.as_secs()
                        ));
                    }
                    if now >= next_note {
                        (self.notes)(&format!(
                            "note: still waiting on {what}, {} s",
                            start.elapsed().as_secs()
                        ));
                        next_note = now + self.note_times.1;
                    }
                    match exited_at {
                        Some(t) if now.duration_since(t) >= POLL => {
                            break Err("Node exited without replying".to_string());
                        }
                        Some(_) => {}
                        None => {
                            if matches!(self.child.try_wait(), Ok(Some(_))) {
                                exited_at = Some(now);
                            }
                        }
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    break Err("Node closed its output".to_string());
                }
            }
        };
        self.slowest = self.slowest.max(start.elapsed());
        let frame = match reply {
            Ok(r) => r,
            Err(why) => return Err(self.fail(&why)),
        };
        if frame.head.get("id") != Some(&json!(id)) {
            let head = frame.head.clone();
            return Err(self.fail(&format!("a reply out of order: {head}")));
        }
        Ok(frame)
    }

    /// What a reply means.
    fn interpret(&mut self, frame: Frame, what: &str) -> JsResult<Decoded> {
        let Frame {
            head: reply, ok, ..
        } = frame;
        if reply.get("changedArgument").is_some() {
            return JsResult::Failed(self.fail(&format!(
                "{what} changes the data 11ty hands it (the config's global data and `eleventy`), which 11ty then merges into every page's data; Brrrty can't follow that, so return the data instead"
            )));
        }
        if let Some(p) = reply.get("protocol") {
            return JsResult::Failed(self.fail(&format!("the worker refused a request: {p}")));
        }
        if let Some(t) = reply.get("thrown") {
            return JsResult::Thrown(js_error(t, &self.root));
        }
        if let Some(m) = reply.get("invalid") {
            return JsResult::Thrown(JsError {
                message: m.as_str().unwrap_or("").to_string(),
                invalid: true,
                ..JsError::default()
            });
        }
        if reply.get("dropShared").is_some() {
            // The worker forgot them; ids go on rising, so none is reused.
            self.kept.lists.clear();
        }
        if let Some(r) = reply.get("refused") {
            return JsResult::Refused(Refusal {
                why: r.as_str().unwrap_or("").to_string(),
                impure: reply.get("impure").and_then(Json::as_bool) == Some(true),
            });
        }
        let earlier = self.earlier.take();
        if reply.get("same").is_some() {
            return match earlier {
                Some(d) => JsResult::Ok(d),
                None => JsResult::Failed(self.fail("a reply that repeats a value never sent")),
            };
        }
        match ok {
            Some(Ok(d)) => JsResult::Ok(d),
            Some(Err(e)) => JsResult::Failed(self.fail(&format!("a value Brrrty can't read: {e}"))),
            None => JsResult::Failed(self.fail(&format!("a reply with no value: {reply}"))),
        }
    }

    /// The worker can't go on: say why, with Node's exit status when it
    /// exited, and the last lines of its stderr.
    fn fail(&mut self, why: &str) -> RuntimeFailure {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => {
                    let _ = self.child.kill();
                    break self.child.wait().ok();
                }
            }
        };
        self.stop_group();
        self.finish_stderr();
        let message = match status {
            Some(s) => match (s.code(), signal_of(&s)) {
                (Some(code), _) => format!("Node exited with status {code} ({why})"),
                (None, Some(sig)) => format!("Node was killed by signal {sig} ({why})"),
                _ => why.to_string(),
            },
            None => why.to_string(),
        };
        let stopped = STOPPING.load(std::sync::atomic::Ordering::SeqCst);
        let message = if stopped {
            "Brrrty was stopped, and stopped Node with it".to_string()
        } else {
            message
        };
        let failure = RuntimeFailure {
            stopped,
            message,
            stderr_tail: self
                .stderr_tail
                .lock()
                .map(|t| t.iter().cloned().collect())
                .unwrap_or_default(),
        };
        self.failed = Some(failure.clone());
        failure
    }

    /// Kill what is left of Node's process group: processes the site's
    /// code started and left running. One started in a session of its
    /// own (`detached`) is out of reach and keeps running, as under 11ty.
    fn stop_group(&mut self) {
        #[cfg(unix)]
        kill_group(self.child.id());
        live(self.child.id(), false);
    }

    /// Wait briefly for the rest of Node's stderr, then stop waiting: a
    /// process Node started may hold it open for as long as it runs.
    fn finish_stderr(&mut self) {
        if let Some(t) = self.stderr_thread.take() {
            let deadline = Instant::now() + STDERR_GRACE;
            while !t.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }

    fn shown(&self, file: &Path) -> String {
        file.strip_prefix(&self.root)
            .unwrap_or(file)
            .display()
            .to_string()
    }
}

impl JsRuntime for Worker {
    fn set_argument(&mut self, arg: &Value) -> JsResult<()> {
        match self.request(
            json!({"op": "setArg", "value": encode(arg)}),
            "the data argument",
        ) {
            JsResult::Ok(_) => JsResult::Ok(()),
            JsResult::Thrown(e) => JsResult::Thrown(e),
            JsResult::Refused(r) => JsResult::Refused(r),
            JsResult::Failed(f) => JsResult::Failed(f),
        }
    }

    fn load(&mut self, file: &Path, call: bool) -> JsResult<Decoded> {
        let what = self.shown(file);
        let key = (call, file.to_path_buf());
        // The worker says `same` when the value is the one it sent last
        // time for this file, and the decoded value is used again: equal
        // data, and the same objects, which Brrrty compares cheaply.
        self.earlier = self.last_loaded.get(&key).cloned();
        let r = self.request(
            json!({"op": "load", "path": file.to_string_lossy(), "call": call}),
            &what,
        );
        if let JsResult::Ok(d) = &r {
            self.last_loaded.insert(key, d.clone());
        }
        r
    }

    fn call(
        &mut self,
        f: FnHandle,
        this: This<'_>,
        args: &[Value],
        settle: Settle,
    ) -> JsResult<Decoded> {
        let mut req = json!({"op": "call", "handle": f.0, "await": settle == Settle::Await});
        let encoded = match this {
            This::Value(v) => encode_all(std::iter::once(v).chain(args), Some(&mut self.kept)),
            This::Opaque(what) => {
                req["opaque"] = json!(what);
                encode_all(args.iter(), Some(&mut self.kept))
            }
        };
        let mut encoded = match encoded {
            Ok(e) => e,
            Err(why) => return JsResult::Refused(Refusal::new(why)),
        };
        if let This::Value(_) = this {
            req["this"] = encoded.remove(0);
        }
        req["args"] = Json::Array(encoded);
        self.request(req, "a data function")
    }

    fn page_open(
        &mut self,
        data: &Value,
        traps: &[(String, String)],
        defaults: &[String],
    ) -> JsResult<PageId> {
        let data = match encode_page(data, &mut self.kept) {
            Ok(e) => e,
            Err(why) => return JsResult::Refused(Refusal::new(why)),
        };
        let r = self.request(
            json!({"op": "pageOpen", "data": data, "traps": traps, "defaults": defaults}),
            "a page's data",
        );
        match r {
            JsResult::Ok(d) => match d.value {
                Value::Number(n) if n >= 1.0 && n.fract() == 0.0 => JsResult::Ok(PageId(n as u64)),
                other => JsResult::Failed(self.fail(&format!("pageOpen gave {other:?}"))),
            },
            JsResult::Thrown(e) => JsResult::Thrown(e),
            JsResult::Refused(r) => JsResult::Refused(r),
            JsResult::Failed(f) => JsResult::Failed(f),
        }
    }

    fn page_vars(&mut self, page: PageId, f: FnHandle, keys: &[String]) -> JsResult<Vec<String>> {
        let r = self.request(
            json!({"op": "pageVars", "page": page.0, "handle": f.0, "keys": keys}),
            "a computed data function",
        );
        match r {
            JsResult::Ok(d) => match strings(&d.value) {
                Some(list) => JsResult::Ok(list),
                None => JsResult::Failed(self.fail(&format!("pageVars gave {:?}", d.value))),
            },
            JsResult::Thrown(e) => JsResult::Thrown(e),
            JsResult::Refused(r) => JsResult::Refused(r),
            JsResult::Failed(f) => JsResult::Failed(f),
        }
    }

    fn page_call(&mut self, page: PageId, f: FnHandle, set: Option<&str>) -> JsResult<Decoded> {
        self.request(
            json!({"op": "pageCall", "page": page.0, "handle": f.0, "set": set}),
            "a computed data function",
        )
    }

    fn page_set(&mut self, page: PageId, key: &str, value: &Value) -> JsResult<()> {
        let value = match encode_all(std::iter::once(value), Some(&mut self.kept)) {
            Ok(mut e) => e.remove(0),
            Err(why) => return JsResult::Refused(Refusal::new(why)),
        };
        match self.request(
            json!({"op": "pageSet", "page": page.0, "key": key, "value": value}),
            "a page's data",
        ) {
            JsResult::Ok(_) => JsResult::Ok(()),
            JsResult::Thrown(e) => JsResult::Thrown(e),
            JsResult::Refused(r) => JsResult::Refused(r),
            JsResult::Failed(f) => JsResult::Failed(f),
        }
    }

    fn page_close(&mut self, page: PageId) {
        let _ = self.request(json!({"op": "pageClose", "page": page.0}), "a page's data");
    }

    fn loads_changed(&mut self) -> JsResult<Vec<(PathBuf, Decoded)>> {
        self.global_end = self.load_log.len();
        let d = match self.request(json!({"op": "loadsChanged"}), "the data files") {
            JsResult::Ok(d) => d,
            JsResult::Thrown(e) => return JsResult::Thrown(e),
            JsResult::Refused(r) => return JsResult::Refused(r),
            JsResult::Failed(f) => return JsResult::Failed(f),
        };
        let Value::Array(items) = &d.value else {
            return JsResult::Failed(self.fail("loadsChanged gave something other than a list"));
        };
        let mut out = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let (Value::String(p), value) = (item.get("path"), item.get("value")) else {
                return JsResult::Failed(self.fail("loadsChanged gave an item without a path"));
            };
            let under = |at: &[PathPart]| -> Option<Vec<PathPart>> {
                match at {
                    [PathPart::Index(j), PathPart::Key(k), rest @ ..]
                        if *j == i && k == "value" =>
                    {
                        Some(rest.to_vec())
                    }
                    _ => None,
                }
            };
            out.push((
                PathBuf::from(&*p),
                Decoded {
                    value,
                    functions: d
                        .functions
                        .iter()
                        .filter_map(|(at, h)| under(at).map(|a| (a, *h)))
                        .collect(),
                    refused: d
                        .refused
                        .iter()
                        .filter_map(|(at, w)| under(at).map(|a| (a, w.clone())))
                        .collect(),
                },
            ));
        }
        JsResult::Ok(out)
    }

    fn begin_calls(&mut self) -> JsResult<()> {
        self.keep_raw
            .store(KEEP_RAW, std::sync::atomic::Ordering::SeqCst);
        let frame = match self.exchange(json!({"op": "callsBegin"}), "the data files") {
            Ok(f) => f,
            Err(f) => return JsResult::Failed(f),
        };
        let raw = frame.raw.unwrap_or(Json::Null);
        self.load_handles = raw.get("handles").and_then(Json::as_u64).unwrap_or(0);
        self.calls_begun = true;
        let changed: Vec<&str> = raw
            .get("changed")
            .and_then(Json::as_array)
            .map(|a| a.iter().filter_map(Json::as_str).collect())
            .unwrap_or_default();
        if !changed.is_empty() {
            return JsResult::Refused(Refusal {
                why: format!(
                    "the data {} {} gave changed while template and directory data files loaded; 11ty's pages see it as it was when each page was placed, which depends on the order 11ty loads them",
                    if changed.len() == 1 { "file" } else { "files" },
                    changed.join(", ")
                ),
                impure: true,
            });
        }
        if raw.get("protos").and_then(Json::as_bool) == Some(true) {
            return JsResult::Refused(Refusal {
                why: "a JavaScript data file changes one of JavaScript's built-in prototypes (`Array.prototype` and the like), which 11ty's templates use and Brrrty's don't".into(),
                impure: true,
            });
        }
        JsResult::Ok(())
    }

    fn set_filters(&mut self, filters: Filters) {
        self.filters = Some(filters);
    }

    fn template_setup(&mut self, filters: &[String]) -> JsResult<()> {
        match self.request(
            json!({"op": "templateSetup", "filters": filters}),
            "the JavaScript templates' setup",
        ) {
            JsResult::Ok(_) => JsResult::Ok(()),
            JsResult::Thrown(e) => JsResult::Thrown(e),
            JsResult::Refused(r) => JsResult::Refused(r),
            JsResult::Failed(f) => JsResult::Failed(f),
        }
    }

    fn load_template(&mut self, file: &Path, layout: bool) -> JsResult<LoadedTemplate> {
        let what = self.shown(file);
        let r = self.request(
            json!({"op": "templateLoad", "path": file.to_string_lossy(), "layout": layout}),
            &what,
        );
        match r {
            JsResult::Ok(data) => match self.last_head.get("template").and_then(Json::as_u64) {
                Some(id) => JsResult::Ok(LoadedTemplate {
                    id: TemplateId(id),
                    data,
                }),
                None => JsResult::Failed(self.fail("templateLoad gave no template id")),
            },
            JsResult::Thrown(e) => JsResult::Thrown(e),
            JsResult::Refused(r) => JsResult::Refused(r),
            JsResult::Failed(f) => JsResult::Failed(f),
        }
    }

    fn render(
        &mut self,
        template: TemplateId,
        data: &Value,
        first: Option<&Value>,
        traps: &[(String, String)],
        filters: &mut FilterCall<'_>,
    ) -> JsResult<Decoded> {
        let (data, first) = match encode_render(data, first, &mut self.kept) {
            Ok(e) => e,
            Err(why) => return JsResult::Refused(Refusal::new(why)),
        };
        let mut req = json!({"op": "render", "template": template.0, "data": data, "traps": traps});
        if let Some(first) = first {
            req["first"] = first;
        }
        self.request_with(req, "a JavaScript template", filters)
    }

    fn verify(&mut self) -> JsResult<()> {
        // What the calls changed of the data files' values comes first: it
        // explains whatever the replay would find.
        match self.request(json!({"op": "exportsFinal"}), "the data files' values") {
            JsResult::Ok(_) => {}
            other @ (JsResult::Refused(_) | JsResult::Failed(_)) => {
                return match other {
                    JsResult::Refused(r) => JsResult::Refused(r),
                    JsResult::Failed(f) => JsResult::Failed(f),
                    _ => JsResult::Ok(()),
                };
            }
            JsResult::Thrown(e) => return JsResult::Thrown(e),
        }
        let log = crate::replay::Log {
            node: &self.node,
            root: &self.root,
            run_mode: self.run_mode,
            loads: &self.load_log,
            global_end: self.global_end,
            load_handles: self.load_handles,
            calls: &self.call_log,
            slowest: self.slowest,
        };
        match log.check() {
            Ok(()) => JsResult::Ok(()),
            Err(crate::replay::Failure::Differs(why, handle)) => {
                let named = handle
                    .and_then(|h| {
                        self.exchange(json!({"op": "describe", "handle": h}), "a function's name")
                            .ok()
                    })
                    .and_then(|f| f.raw)
                    .and_then(|r| r.as_str().map(str::to_string))
                    .unwrap_or_else(|| "a function from JavaScript data".to_string());
                JsResult::Refused(Refusal {
                    why: why.replace("{fn}", &named),
                    impure: true,
                })
            }
            Err(crate::replay::Failure::Runtime(f)) => JsResult::Failed(f),
        }
    }

    fn reads(&mut self) -> JsResult<Vec<PathBuf>> {
        match self.request(json!({"op": "reads"}), "the list of files read") {
            JsResult::Ok(d) => match strings(&d.value) {
                Some(list) => JsResult::Ok(list.into_iter().map(PathBuf::from).collect()),
                None => JsResult::Failed(self.fail(&format!("reads gave {:?}", d.value))),
            },
            JsResult::Thrown(e) => JsResult::Thrown(e),
            JsResult::Refused(r) => JsResult::Refused(r),
            JsResult::Failed(f) => JsResult::Failed(f),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"{\"op\":\"bye\"}\n");
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        self.stop_group();
        self.finish_stderr();
    }
}

/// The process ids of the workers running now. Each leads its own process
/// group, so a terminal's Ctrl-C doesn't reach it: [`stop_all`] does.
static LIVE: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Set once [`stop_all`] ran: a worker failing after that was stopped.
static STOPPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn live(pid: u32, running: bool) {
    let mut l = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    l.retain(|p| *p != pid);
    if running {
        l.push(pid);
    }
}

/// Kill every running worker and what it started, at once: for Ctrl-C,
/// SIGTERM and a panic. Safe to call from a signal handler thread.
pub fn stop_all() {
    STOPPING.store(true, std::sync::atomic::Ordering::SeqCst);
    let l = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    #[cfg(unix)]
    for pid in l.iter() {
        kill_group(*pid);
    }
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    if let Ok(pid) = i32::try_from(pid) {
        // SAFETY: killpg only sends a signal, to the group Node leads.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
}

/// Start the worker. Its replies come on a pipe of their own, which Node
/// inherits under the descriptor number it has here, and its stdout joins
/// its stderr: a child process or logger writing to fd 1 is only noise, as
/// it is under 11ty.
#[cfg(unix)]
fn spawn(mut cmd: Command) -> std::io::Result<Spawned> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let (frames_r, frames_w) = std::io::pipe()?;
    // A copy is numbered 3 or above, clear of the standard three.
    let frames_w = frames_w.try_clone()?;
    let (answers_r, answers_w) = std::io::pipe()?;
    let answers_r = answers_r.try_clone()?;
    let (err_r, err_w) = std::io::pipe()?;
    let fd = frames_w.as_raw_fd();
    let answers_fd = answers_r.as_raw_fd();
    cmd.stdout(err_w.try_clone()?)
        .stderr(err_w)
        .env("BRRRTY_FRAMES_FD", fd.to_string())
        .env("BRRRTY_ANSWERS_FD", answers_fd.to_string())
        // A group of its own, so what Node starts can be stopped with it.
        .process_group(0);
    // SAFETY: fcntl is async-signal-safe, and the descriptor is this
    // process's own, open until after the spawn.
    unsafe {
        cmd.pre_exec(move || {
            // Keep it open across exec in Node only.
            if libc::fcntl(fd, libc::F_SETFD, 0) == -1
                || libc::fcntl(answers_fd, libc::F_SETFD, 0) == -1
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn();
    // The parent's write ends close here, so the reads end when Node's do.
    drop(cmd);
    drop(frames_w);
    drop(answers_r);
    Ok((
        child?,
        Box::new(frames_r),
        Box::new(err_r),
        Some(Box::new(answers_w)),
    ))
}

/// A started Node: the process, its replies, its stderr, and where
/// Brrrty answers its questions (none where that pipe can't be passed).
type Spawned = (
    Child,
    Box<dyn Read + Send>,
    Box<dyn Read + Send>,
    Option<Box<dyn Write + Send>>,
);

/// Start the worker, its replies on stdout.
#[cfg(not(unix))]
fn spawn(mut cmd: Command) -> std::io::Result<Spawned> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("BRRRTY_FRAMES_FD", "1")
        .spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    Ok((child, Box::new(stdout), Box::new(stderr), None))
}

#[cfg(unix)]
fn signal_of(s: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    s.signal()
}

#[cfg(not(unix))]
fn signal_of(_: &std::process::ExitStatus) -> Option<i32> {
    None
}

/// Read `\x1eBRRRTY <len>\n<json>` frames until Node closes stdout.
/// Anything else is a protocol error, sent once.
fn read_frames(
    stdout: impl Read,
    tx: mpsc::Sender<Reply>,
    keep_raw: Arc<std::sync::atomic::AtomicU8>,
) {
    let mut r = BufReader::new(stdout);
    loop {
        let mut header = Vec::new();
        match r.read_until(b'\n', &mut header) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) => {
                let _ = tx.send(Err(format!("cannot read Node's output: {e}")));
                return;
            }
        }
        let len = header
            .strip_prefix(b"\x1eBRRRTY ")
            .and_then(|h| h.strip_suffix(b"\n"))
            .and_then(|n| std::str::from_utf8(n).ok())
            .and_then(|n| n.parse::<usize>().ok());
        let Some(len) = len else {
            let shown = String::from_utf8_lossy(&header);
            let _ = tx.send(Err(format!(
                "something other than the worker wrote to its output: {:?}",
                shown.chars().take(80).collect::<String>()
            )));
            return;
        };
        let mut body = vec![0; len];
        if let Err(e) = r.read_exact(&mut body) {
            let _ = tx.send(Err(format!("a cut-off reply: {e}")));
            return;
        }
        let keep = keep_raw.load(std::sync::atomic::Ordering::SeqCst);
        let parsed = parse_reply(&body).map(|mut head| {
            let value = head.as_object_mut().and_then(|o| o.remove("ok"));
            let ok = value.as_ref().map(decode);
            let raw = if keep == KEEP_RAW { value } else { None };
            let text = (keep == KEEP_TEXT)
                .then(|| String::from_utf8_lossy(&body).into_owned().into_boxed_str());
            Frame {
                head,
                ok,
                raw,
                text,
            }
        });
        if tx.send(parsed).is_err() {
            return;
        }
    }
}

/// A list of strings, as `reads` and `varsUsed` reply.
fn strings(v: &Value) -> Option<Vec<String>> {
    match v {
        Value::Array(a) => a
            .iter()
            .map(|s| match s {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

/// One reply body, without serde's nesting limit (see [`REPLY_STACK`]).
pub(crate) fn parse_reply(body: &[u8]) -> Result<Json, String> {
    let mut de = serde_json::Deserializer::from_slice(body);
    de.disable_recursion_limit();
    let v = serde::Deserialize::deserialize(&mut de)
        .and_then(|v: Json| de.end().map(|()| v))
        .map_err(|e| format!("a reply that isn't JSON: {e}"))?;
    Ok(v)
}

/// Pass Node's stderr through to Brrrty's, as it comes, keeping the last
/// lines for a failure report.
fn drain_stderr(stderr: impl Read, tail: Arc<Mutex<VecDeque<String>>>) {
    let mut r = BufReader::new(stderr);
    let mut line = Vec::new();
    let mut err = std::io::stderr();
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let _ = err.write_all(&line);
                if let Ok(mut t) = tail.lock() {
                    if t.len() == STDERR_TAIL {
                        t.pop_front();
                    }
                    t.push_back(String::from_utf8_lossy(&line).trim_end().to_string());
                }
            }
        }
    }
}

/// The site's error, placed: a SyntaxError at the header Node prints for
/// it, anything else at the first stack frame inside the project and
/// outside `node_modules`.
fn js_error(t: &Json, root: &Path) -> JsError {
    let s = |k: &str| t.get(k).and_then(Json::as_str).unwrap_or("").to_string();
    let stack = s("stack");
    let place = crate::stack::syntax_header(&s("inspect"))
        .or_else(|| crate::stack::pick(&crate::stack::frames(&stack), root));
    JsError {
        name: s("name"),
        message: s("message"),
        file: place.as_ref().map(|p| p.file.clone()),
        line: place.as_ref().map(|p| p.line),
        column: place.and_then(|p| p.column),
        stack,
        invalid: false,
    }
}

/// Values in the worker's wire forms, for a call: an object or array
/// that appears more than once is sent once (`{"$s": [id, value]}`, then
/// `{"$r": id}`), so data shared between pages, as 11ty shares it, stays
/// one object; and a property Brrrty doesn't have the value of (a
/// collection item's content, a key it withholds) is sent as a trap,
/// `{"$t": why}`, which refuses the call when the function reads it. `Err`
/// when a value can't be handed over at all.
///
/// With `kept`, a list of [`KEPT_LEN`] items or more is kept by the worker
/// between requests (`{"$S": [id, value]}`, then `{"$R": id}`): the lists
/// global data gives every page cross once, and stay one list, as in 11ty.
pub fn encode_all<'v>(
    values: impl Iterator<Item = &'v Value> + Clone,
    kept: Option<&mut Kept>,
) -> Result<Vec<Json>, String> {
    let mut counts: HashMap<usize, u32> = HashMap::new();
    for v in values.clone() {
        count(v, &mut counts, kept.as_deref());
    }
    let mut e = Encoder {
        counts,
        ids: HashMap::new(),
        kept,
        no_keep: None,
        in_kept: 0,
        guard: false,
    };
    values.map(|v| e.encode(v)).collect()
}

/// [`encode_all`] of a page's data, whose top object is the page's own:
/// what is inside it may be kept, the object itself isn't.
fn encode_page(data: &Value, kept: &mut Kept) -> Result<Json, String> {
    let mut counts: HashMap<usize, u32> = HashMap::new();
    count(data, &mut counts, Some(kept));
    let mut e = Encoder {
        counts,
        ids: HashMap::new(),
        kept: Some(kept),
        no_keep: ptr_of(data),
        in_kept: 0,
        guard: false,
    };
    e.encode(data)
}

/// A render's data and `first` page in the worker's wire forms: the data's
/// own top object isn't kept, `first` is page 0's `page` (the same object
/// as the data's `page` on page 0, sent once), and a `collections` object
/// Brrrty can't answer everything about crosses guarded (see
/// [`Encoder::guard`]).
fn encode_render(
    data: &Value,
    first: Option<&Value>,
    kept: &mut Kept,
) -> Result<(Json, Option<Json>), String> {
    let mut counts: HashMap<usize, u32> = HashMap::new();
    count(data, &mut counts, Some(kept));
    if let Some(f) = first {
        count(f, &mut counts, Some(kept));
    }
    let mut e = Encoder {
        counts,
        ids: HashMap::new(),
        kept: Some(kept),
        no_keep: ptr_of(data),
        in_kept: 0,
        guard: true,
    };
    let data = e.encode(data)?;
    let first = first.map(|f| e.encode(f)).transpose()?;
    Ok((data, first))
}

/// How many items or keys a list or object must have for the worker to
/// keep it between requests.
pub const KEPT_LEN: usize = 8;

/// The lists the worker keeps, by address, each held so the address stays
/// its own.
#[derive(Default)]
pub struct Kept {
    lists: HashMap<usize, (Value, u64)>,
    next: u64,
}

fn ptr_of(v: &Value) -> Option<usize> {
    match v {
        Value::Object(o) => Some(Arc::as_ptr(o) as usize),
        Value::Array(a) => Some(Arc::as_ptr(a) as usize),
        _ => None,
    }
}

/// How often each object and list appears in a request. A list the worker
/// keeps already is sent as a reference, so what is inside it doesn't count.
fn count(v: &Value, counts: &mut HashMap<usize, u32>, kept: Option<&Kept>) {
    let mut stack = vec![v];
    while let Some(v) = stack.pop() {
        let Some(p) = ptr_of(v) else { continue };
        let n = counts.entry(p).or_insert(0);
        *n += 1;
        if *n > 1 || kept.is_some_and(|k| k.lists.contains_key(&p)) {
            continue;
        }
        match v {
            Value::Object(o) => stack.extend(o.values()),
            Value::Array(a) => stack.extend(a.iter()),
            _ => {}
        }
    }
}

struct Encoder<'k> {
    counts: HashMap<usize, u32>,
    /// The id of each shared object or array sent so far.
    ids: HashMap<usize, usize>,
    kept: Option<&'k mut Kept>,
    /// The one object or list not to keep.
    no_keep: Option<usize>,
    /// How many kept values the encoder is inside.
    in_kept: usize,
    /// Send a `collections` object that traps (an open or order-unknown
    /// one) as a guarded object instead of refusing it: a render gets the
    /// whole page data, `collections` included.
    guard: bool,
}

/// Why a JavaScript template can't read a name an open `collections`
/// object doesn't have (`{key}` is the name).
const OPEN_MISSING: &str = "reads `collections.{key}`, which Brrrty doesn't have: the site's 11ty config may add it, or a page in it can't be built (see the other problems)";

/// Why a JavaScript template can't list an open `collections` object.
const OPEN_LIST: &str = "lists the names in `collections`, and the site's 11ty config may add collections Brrrty can't see";

/// Why a property Brrrty marks as unknown can't be handed to a function.
fn trap_reason(key: &str) -> String {
    match key {
        "templateContent" | "content" => format!(
            "reads `{key}` of a collection item, which 11ty fills in only as it renders pages"
        ),
        "template" => {
            "reads `template` of a collection item, 11ty's internal template object".to_string()
        }
        "rawInput" => {
            "reads `rawInput` of a JavaScript template's page, which in 11ty is its instance object, not text".to_string()
        }
        _ => format!("reads `{key}`, which Brrrty doesn't hand to JavaScript functions yet"),
    }
}

impl Encoder<'_> {
    fn encode(&mut self, v: &Value) -> Result<Json, String> {
        let Some(p) = ptr_of(v) else {
            return Ok(encode(v));
        };
        let big = match v {
            Value::Array(a) => a.len() >= KEPT_LEN,
            Value::Object(o) => o.len() >= KEPT_LEN && !brrrty_js::traps::is_guarded_object(o),
            _ => false,
        };
        if big
            && self.no_keep != Some(p)
            && let Some(kept) = self.kept.as_deref_mut()
        {
            if let Some((_, id)) = kept.lists.get(&p) {
                return Ok(json!({"$R": id}));
            }
            let id = kept.next;
            kept.next += 1;
            kept.lists.insert(p, (v.clone(), id));
            // A kept value stands on its own: it may be sent again, by
            // itself, in another request (the replay check does), so what
            // is inside it never refers to this request's shared values.
            self.in_kept += 1;
            let inner = self.inner(v);
            self.in_kept -= 1;
            return Ok(json!({"$S": [id, inner?]}));
        }
        if self.in_kept > 0 {
            return self.inner(v);
        }
        self.encode_inline(v, p)
    }

    /// `v` (at address `p`) in full, as a value shared within the request
    /// when it appears more than once in it.
    fn encode_inline(&mut self, v: &Value, p: usize) -> Result<Json, String> {
        if let Some(id) = self.ids.get(&p) {
            return Ok(json!({"$r": id}));
        }
        let shared = self.counts.get(&p).copied().unwrap_or(0) > 1;
        let id = self.ids.len();
        if shared {
            self.ids.insert(p, id);
        }
        let inner = self.inner(v)?;
        Ok(if shared {
            json!({"$s": [id, inner]})
        } else {
            inner
        })
    }

    /// A list or object's own wire form.
    fn inner(&mut self, v: &Value) -> Result<Json, String> {
        Ok(match v {
            Value::Array(a) => Json::Array(
                a.iter()
                    .map(|x| self.encode(x))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Value::Object(o) => {
                let open = brrrty_js::traps::is_open_object(o);
                let unordered = brrrty_js::traps::unordered_reason(o);
                if (open || unordered.is_some()) && !self.guard {
                    return Err(
                        "passes the whole `collections` object to a function, which Brrrty doesn't do yet; pass one collection, like `collections.post`".to_string(),
                    );
                }
                let mut entries = Vec::with_capacity(o.len());
                // A render's own `content` (a layout's, given `undefined` by
                // a template's render) is a value, not an item's marker.
                let top = self.guard && self.no_keep == ptr_of(v);
                for (k, x) in o.iter() {
                    let item = if matches!(x, Value::Undefined)
                        && brrrty_js::traps::TRAP_KEYS.contains(&k.as_str())
                        && !(top && brrrty_js::traps::CONTENT_KEYS.contains(&k.as_str()))
                    {
                        json!({"$t": trap_reason(k)})
                    } else {
                        self.encode(x)?
                    };
                    entries.push(json!([k, item]));
                }
                if let Some(keys) = brrrty_js::traps::withheld_keys(o) {
                    for k in keys.iter().filter(|k| !o.contains_key(k.as_str())) {
                        entries.push(json!([k, {"$t": format!("reads `{k}` of another page's `data`, which Brrrty can't supply")}]));
                    }
                }
                if open || unordered.is_some() {
                    let mut c = serde_json::Map::new();
                    if open {
                        c.insert("missing".into(), json!(OPEN_MISSING));
                    }
                    match (&unordered, open) {
                        (Some(why), _) => {
                            c.insert(
                                "list".into(),
                                json!(format!("lists the names in `collections`, {why}")),
                            );
                        }
                        (None, true) => {
                            c.insert("list".into(), json!(OPEN_LIST));
                        }
                        (None, false) => {}
                    }
                    return Ok(json!({"$o": entries, "$c": c}));
                }
                json!({"$o": entries})
            }
            _ => encode(v),
        })
    }
}

/// A Brrrty value in the worker's wire forms.
pub fn encode(v: &Value) -> Json {
    match v {
        Value::Undefined => json!({"$u": 1}),
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Number(n) => {
            if n.is_nan() {
                json!({"$n": "NaN"})
            } else if *n == f64::INFINITY {
                json!({"$n": "Infinity"})
            } else if *n == f64::NEG_INFINITY {
                json!({"$n": "-Infinity"})
            } else if *n == 0.0 && n.is_sign_negative() {
                json!({"$n": "-0"})
            } else {
                serde_json::Number::from_f64(*n).map_or(Json::Null, Json::Number)
            }
        }
        Value::String(s) => Json::String(s.to_string()),
        Value::Array(a) => Json::Array(a.iter().map(encode).collect()),
        Value::Object(o) => {
            json!({"$o": o.iter().map(|(k, v)| json!([k, encode(v)])).collect::<Vec<_>>()})
        }
        Value::Date(d) => json!({"$d": d.timestamp_millis()}),
        Value::Function(h) => json!({"$f": h.0}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(n: usize) -> Value {
        Value::Object(Arc::new(
            (0..n)
                .map(|i| (format!("k{i}"), Value::Number(i as f64)))
                .collect(),
        ))
    }

    #[test]
    fn big_objects_and_lists_cross_once_and_the_page_itself_never_is_kept() {
        let mut kept = Kept::default();
        let big = object(KEPT_LEN);
        let small = object(2);
        let first = encode_all([&big, &small].into_iter(), Some(&mut kept)).unwrap();
        assert!(first[0].get("$S").is_some(), "{}", first[0]);
        assert!(first[1].get("$o").is_some(), "{}", first[1]);
        let again = encode_all([&big].into_iter(), Some(&mut kept)).unwrap();
        assert_eq!(again[0], json!({"$R": 0}));
        // A page's own data object holds kept values but isn't one.
        let page = Value::Object(Arc::new(
            (0..KEPT_LEN)
                .map(|i| (format!("p{i}"), big.clone()))
                .collect(),
        ));
        let p = encode_page(&page, &mut kept).unwrap();
        assert!(p.get("$o").is_some(), "{p}");
        assert!(p.to_string().contains("\"$R\":0"), "{p}");
    }
}
