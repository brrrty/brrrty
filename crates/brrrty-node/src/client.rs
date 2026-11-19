//! One Node process running the worker, and the requests Brrrty makes of it.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use brrrty_js::Value;
use brrrty_js::runtime::{
    Decoded, FilterCall, Filters, FnHandle, JsError, JsResult, JsRuntime, LoadedTemplate, PageId,
    PageStep, PathPart, Refusal, RuntimeFailure, Settle, TemplateId, This,
};
use serde_json::{Value as Json, json};

use crate::decode::decode;
use crate::find::NodeInfo;
use crate::wire::{Added, Table};

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

/// How many bytes of requests may be written behind one Node hasn't
/// answered: less than any pipe holds.
const PIPE_SAFE: usize = 4000;

/// How long a wait for a reply looks for it before sleeping.
const SPIN: Duration = Duration::from_micros(60);

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

/// One frame from the worker, or why there is none.
type Reply = Result<Frame, String>;

pub(crate) struct Frame {
    /// A question of the worker's own (a filter a JavaScript template
    /// calls on `this`), not a reply.
    pub question: bool,
    /// The request it answers.
    pub id: u64,
    /// The reply without its value.
    pub head: Json,
    /// The decoded `ok` value, when the reply has one.
    ok: Option<Result<Decoded, String>>,
    /// The reply as sent, without its id and clock: what the replay check
    /// compares.
    pub body: Box<str>,
    /// The clock and random values the request read, as JSON text.
    pub clock: Option<Box<str>>,
    /// The reply mentions a function a call made.
    pub made: bool,
}

impl Frame {
    /// The decoded `ok` value, when it crossed exactly.
    pub fn value(&self) -> Option<&Value> {
        match &self.ok {
            Some(Ok(d)) => Some(&d.value),
            _ => None,
        }
    }
}

/// One request that loaded data or a template, with its reply, for the
/// replay check.
pub(crate) struct LoadRecord {
    /// The request as sent, without its id.
    pub text: Box<str>,
    /// Whether it loads a template (`templateLoad`), a data file (`load`)
    /// or neither.
    pub kind: LoadKind,
    pub path: String,
    pub body: Box<str>,
    pub clock: Option<Box<str>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadKind {
    Data,
    Template,
    Other,
}

/// One request made after the data files loaded, with its reply, for the
/// replay check.
pub(crate) struct Record {
    /// The render that made it ([`brrrty_js::functions::unit`]); a request
    /// made outside one is a unit of its own.
    pub unit: u64,
    pub label: Arc<str>,
    pub op: &'static str,
    /// The function called, when the request calls one.
    pub handle: Option<u32>,
    /// The request as sent, without its id. It refers to the values the
    /// workers keep by id, which are the same in every worker.
    pub text: Box<str>,
    pub body: Box<str>,
    pub clock: Option<Box<str>>,
    /// The reply mentions a function a call made.
    pub made: bool,
    /// The dynamic definitions the request needs (see [`crate::wire`]),
    /// and whether it names a function a call made itself.
    pub dynamic: Vec<u32>,
    pub names_made: bool,
    /// The questions the request asked Brrrty, with the answers it got,
    /// both as sent.
    pub answers: Vec<(Box<str>, Box<str>)>,
    /// Nothing came back from it, and nothing will.
    pub one_way: bool,
    /// It ran after a call sent with it changed a value the worker kept:
    /// what it gave isn't used, and a replay's won't be the same.
    pub unchecked: bool,
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
    pub(crate) answers_feed: Option<VecDeque<(Box<str>, Box<str>)>>,
    /// The questions the request in flight asked, with their answers.
    asked: Vec<(Box<str>, Box<str>)>,
    /// Brrrty's filters, for the questions a request asks.
    filters: Option<Filters>,
    replies: Receiver<Reply>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    stderr_thread: Option<JoinHandle<()>>,
    root: PathBuf,
    next_id: u64,
    next_page: u64,
    /// Set once the worker failed: every later request fails the same way.
    failed: Option<RuntimeFailure>,
    /// The value each load (by call mode and file) gave last.
    last_loaded: HashMap<(bool, PathBuf), Decoded>,
    /// The value a `same` reply to the request in flight stands for.
    earlier: Option<Decoded>,
    /// The lists and objects the workers keep (see [`crate::wire`]).
    table: Table,
    /// What the worker was started with, for the replay check.
    node: NodeInfo,
    run_mode: RunMode,
    /// Decode each reply's value. The replay check's workers don't: they
    /// compare replies as text.
    pub(crate) decode: Arc<AtomicBool>,
    /// The requests that loaded the data, in order, with their replies.
    load_log: Vec<LoadRecord>,
    /// How many of them loaded global data (the rest are template and
    /// directory data files and templates, which 11ty loads as their
    /// files finish loading).
    global_end: usize,
    /// The requests made since, with their replies.
    call_log: Vec<Record>,
    /// A call changed a value the worker kept.
    dropped: bool,
    /// Set once the data files loaded: see [`JsRuntime::begin_calls`].
    calls_begun: bool,
    /// How many function handles the data files' values made.
    load_handles: u64,
    /// The replay check's two workers, started as soon as there is
    /// something for them to check.
    replays: Option<[crate::replay::Handle; 2]>,
    /// The longest a request took.
    slowest: Duration,
    /// How long a request may take before the worker is given up on (the
    /// replay check's workers only).
    pub(crate) deadline: Option<Duration>,
    /// The template id of the last `templateLoad` reply.
    last_template: Option<u64>,
    /// What the requests cost, for `BRRRTY_JS_TRACE`.
    stats: Stats,
    /// The worker froze the data files' values when the calls began.
    frozen: bool,
    /// Why it walks them instead, when it does.
    walks: Option<String>,
    /// Where slow-request notes go; stderr unless a test replaces it.
    notes: Box<dyn Fn(&str) + Send>,
    /// When the first note comes, and then how often.
    note_times: (Duration, Duration),
}

/// What a worker's requests cost so far.
#[derive(Default)]
struct Stats {
    started: Option<Instant>,
    requests: u64,
    /// Bytes of requests, and of definitions.
    sent: u64,
    defined: u64,
    definitions: u64,
    /// Bytes of replies.
    received: u64,
    /// Time between a request going out and its reply coming in.
    waited: Duration,
}

/// One request of the first run: what to send, and what the replay check
/// needs to know of it.
struct Request<'a> {
    op: &'static str,
    /// The request as JSON text, without its id.
    text: String,
    /// The definitions it needs sent first.
    added: Added,
    /// The function it calls.
    handle: Option<u32>,
    /// The file it loads.
    path: Option<&'a Path>,
    /// Nothing comes back from it (`pageClose`).
    one_way: bool,
}

impl<'a> Request<'a> {
    fn new(op: &'static str, text: String) -> Request<'a> {
        Request {
            op,
            text,
            added: Added::default(),
            handle: None,
            path: None,
            one_way: false,
        }
    }
}

/// A wait for one frame: when it began, and when the next note is due.
pub(crate) struct Wait {
    start: Instant,
    next_note: Instant,
    /// Node exiting ends the wait even when a process it started keeps
    /// the replies' pipe open.
    exited_at: Option<Instant>,
}

impl Worker {
    /// Start Node with the worker, in `root` (the project folder), with
    /// 11ty's environment variables added to Brrrty's own.
    pub fn start(node: &NodeInfo, root: &Path, run_mode: RunMode) -> Result<Worker, WorkerError> {
        Worker::start_as(node, root, run_mode, false)
    }

    /// [`Worker::start`]; `replay` for one of the replay check's workers.
    pub(crate) fn start_as(
        node: &NodeInfo,
        root: &Path,
        run_mode: RunMode,
        replay: bool,
    ) -> Result<Worker, WorkerError> {
        let mut cmd = Command::new(&node.path);
        cmd.args(["--input-type=module", "--eval", WORKER_SOURCE])
            .current_dir(root)
            .env("ELEVENTY_ROOT", root)
            .env("ELEVENTY_SOURCE", "cli")
            .env("ELEVENTY_RUN_MODE", run_mode.as_str())
            .env("ELEVENTY_VERSION", ELEVENTY_VERSION)
            .stdin(Stdio::piped());
        if walk_roots().contains(root) {
            cmd.env("BRRRTY_JS_EXPORTS", "walk");
        }
        if replay {
            cmd.env("BRRRTY_ROLE", "replay");
        }
        let cannot =
            |e: std::io::Error| WorkerError(format!("cannot start {}: {e}", node.path.display()));
        let (mut child, frames, stderr, answers) = spawn(cmd).map_err(cannot)?;
        live(child.id(), true);
        let (tx, replies) = mpsc::channel();
        let decode = Arc::new(AtomicBool::new(true));
        let decoding = decode.clone();
        // A reply nests up to three JSON levels per level of the value
        // (`encode.mjs` refuses values deeper than 1000), and parsing it
        // recurses once per level: a roomy stack, not serde's 128 limit.
        std::thread::Builder::new()
            .name("brrrty-node-replies".into())
            .stack_size(REPLY_STACK)
            .spawn(move || read_frames(frames, tx, decoding))
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
            next_page: 0,
            failed: None,
            last_loaded: HashMap::new(),
            earlier: None,
            table: Table::default(),
            node: node.clone(),
            run_mode,
            decode,
            load_log: Vec::new(),
            global_end: 0,
            call_log: Vec::new(),
            dropped: false,
            calls_begun: false,
            load_handles: 0,
            replays: None,
            slowest: Duration::ZERO,
            deadline: None,
            last_template: None,
            stats: Stats {
                started: Some(Instant::now()),
                ..Stats::default()
            },
            frozen: false,
            walks: None,
            notes: Box::new(|s| eprintln!("{s}")),
            note_times: (FIRST_NOTE, NEXT_NOTE),
        })
    }

    /// The request that opens a page with `data`, and the page's number.
    fn open_request(
        &mut self,
        data: &Value,
        traps: &[(String, String)],
        defaults: &[String],
    ) -> Result<(PageId, Request<'static>), String> {
        self.next_page += 1;
        let page = self.next_page;
        let mut added = Added::default();
        let mut text = String::with_capacity(1024);
        let _ = write!(text, "{{\"op\":\"pageOpen\",\"page\":{page},\"data\":");
        self.table.page_data(data, false, &mut text, &mut added)?;
        text.push_str(",\"traps\":");
        pairs(&mut text, traps);
        text.push_str(",\"defaults\":");
        crate::wire::strings(&mut text, defaults);
        text.push('}');
        Ok((
            PageId(page),
            Request {
                added,
                ..Request::new("pageOpen", text)
            },
        ))
    }

    /// Requests of a page's computed data, sent together, answered with
    /// the worker's own filters.
    fn run(&mut self, reqs: Vec<Request<'_>>) -> Vec<JsResult<Decoded>> {
        if reqs.is_empty() {
            return Vec::new();
        }
        let filters = self.filters.clone();
        self.requests(
            reqs,
            "a computed data function",
            &mut |name: &str, args: &[Value]| match &filters {
                Some(f) => f(name, args),
                None => Err(format!(
                    "calls `this.{name}` where Brrrty has no filters to run it"
                )),
            },
        )
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

    /// Whether the worker froze the data files' values when the pages'
    /// calls began (`Ok`), or walks them after every call, and why (`Err`).
    pub fn exports_frozen(&self) -> Result<(), String> {
        if self.frozen {
            Ok(())
        } else {
            Err(self.walks.clone().unwrap_or_default())
        }
    }

    /// One line of what the requests cost so far (`BRRRTY_JS_TRACE`).
    fn trace(&self, at: &str) {
        let s = &self.stats;
        eprintln!(
            "note: {at}, {:.3} s after Node started: {} requests ({:.1} MB) and {} definitions ({:.1} MB) sent, {:.1} MB of replies, {:.3} s waiting on Node",
            s.started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0),
            s.requests,
            s.sent as f64 / 1e6,
            s.definitions,
            s.defined as f64 / 1e6,
            s.received as f64 / 1e6,
            s.waited.as_secs_f64(),
        );
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
    fn request(&mut self, req: Request<'_>, what: &str) -> JsResult<Decoded> {
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
        req: Request<'_>,
        what: &str,
        filters: &mut FilterCall<'_>,
    ) -> JsResult<Decoded> {
        self.requests(vec![req], what, filters)
            .pop()
            .unwrap_or_else(|| JsResult::Failed(self.fail("a request with no reply")))
    }

    /// Several requests sent together, and their replies in order: Node
    /// runs them one after another without Brrrty in between, so they cost
    /// one round trip. The list ends with the first reply that isn't a
    /// value; the requests after it still ran.
    fn requests(
        &mut self,
        reqs: Vec<Request<'_>>,
        what: &str,
        filters: &mut FilterCall<'_>,
    ) -> Vec<JsResult<Decoded>> {
        let logged = |op: &str| {
            !matches!(
                op,
                "reads" | "info" | "callsBegin" | "loadsChanged" | "exportsFinal" | "describe"
            )
        };
        if self.calls_begun && self.replays.is_none() && reqs.iter().any(|r| logged(r.op)) {
            self.start_replays();
        }
        // The values the requests refer to for the first time, each in a
        // line of its own, to this worker and to the replay check's.
        for req in &reqs {
            for (line, dynamic) in &req.added.lines {
                self.stats.definitions += 1;
                self.stats.defined += line.len() as u64;
                if let Err(f) = self.write_line(line) {
                    return vec![JsResult::Failed(f)];
                }
                if !dynamic && let Some(replays) = &self.replays {
                    for r in replays {
                        r.line(line.clone());
                    }
                }
            }
        }
        let mut out = Vec::with_capacity(reqs.len());
        // A call in the group changed a value the worker kept: the ones
        // after it ran on what it left, which a replay doesn't have.
        let mut unchecked = false;
        let mut reqs = reqs.into_iter().peekable();
        while reqs.peek().is_some() {
            // As many as can go in one write. Node may stop reading while
            // it waits for an answer to a question of its own, so what is
            // written past the first request must fit in the pipe.
            let mut group: Vec<(Request<'_>, u64)> = Vec::new();
            let mut lines = String::new();
            let mut ahead = 0;
            while let Some(next) = reqs.peek() {
                if !group.is_empty() && ahead + next.text.len() + 32 > PIPE_SAFE {
                    break;
                }
                let Some(req) = reqs.next() else { break };
                let (id, line) = self.line_for(&req.text, None);
                if !group.is_empty() {
                    ahead += line.len() + 1;
                }
                lines.push_str(&line);
                lines.push('\n');
                group.push((req, id));
            }
            if let Err(f) = self.write_lines(&lines) {
                out.push(JsResult::Failed(f));
                return out;
            }
            for (req, id) in group {
                self.asked.clear();
                let mut frame = if req.one_way {
                    None
                } else {
                    match self.reply(id, req.text.len(), what, filters) {
                        Ok(f) => Some(f),
                        Err(f) => {
                            out.push(JsResult::Failed(f));
                            return out;
                        }
                    }
                };
                if req.op == "templateLoad" {
                    self.last_template = frame
                        .as_ref()
                        .and_then(|f| f.head.get("template"))
                        .and_then(Json::as_u64);
                }
                let body = frame
                    .as_mut()
                    .map(|f| std::mem::take(&mut f.body))
                    .unwrap_or_default();
                let clock = frame.as_mut().and_then(|f| f.clock.take());
                if logged(req.op) && !self.calls_begun {
                    self.load_log.push(LoadRecord {
                        text: req.text.into_boxed_str(),
                        kind: match req.op {
                            "load" => LoadKind::Data,
                            "templateLoad" => LoadKind::Template,
                            _ => LoadKind::Other,
                        },
                        path: req
                            .path
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        body,
                        clock,
                    });
                } else if logged(req.op) {
                    let (unit, label) = match brrrty_js::functions::unit() {
                        Some(u) => u,
                        None => (u64::MAX - self.call_log.len() as u64, Arc::from("")),
                    };
                    let dropped = frame
                        .as_ref()
                        .is_some_and(|f| f.head.get("dropShared").is_some());
                    self.dropped |= dropped;
                    self.call_log.push(Record {
                        unit,
                        label,
                        op: req.op,
                        handle: req.handle,
                        text: req.text.into_boxed_str(),
                        body,
                        clock,
                        made: frame.as_ref().is_some_and(|f| f.made),
                        names_made: req.added.made
                            || req.handle.is_some_and(|h| self.table.made_by_call(h)),
                        dynamic: req.added.dynamic,
                        answers: std::mem::take(&mut self.asked),
                        one_way: req.one_way,
                        unchecked,
                    });
                    unchecked |= dropped;
                }
                let Some(frame) = frame else {
                    // Nothing comes back from a one-way request.
                    out.push(JsResult::Ok(Decoded {
                        value: Value::Undefined,
                        functions: Vec::new(),
                        refused: Vec::new(),
                    }));
                    continue;
                };
                let r = self.interpret(frame, what);
                if matches!(r, JsResult::Failed(_)) {
                    out.push(r);
                    return out;
                }
                out.push(r);
            }
        }
        // The list ends with the first reply that isn't a value.
        if let Some(bad) = out.iter().position(|r| !matches!(r, JsResult::Ok(_))) {
            out.truncate(bad + 1);
        }
        out
    }

    /// Write whole lines (each ending in a newline) to Node's stdin.
    fn write_lines(&mut self, lines: &str) -> Result<(), RuntimeFailure> {
        if let Some(f) = &self.failed {
            return Err(f.clone());
        }
        let sent = self
            .stdin
            .as_mut()
            .map(|s| s.write_all(lines.as_bytes()).and_then(|_| s.flush()));
        if !matches!(sent, Some(Ok(()))) {
            return Err(self.fail("Node stopped reading requests"));
        }
        Ok(())
    }

    /// Wait for the reply to request `id`, answering the worker's filter
    /// questions with `filters` meanwhile.
    fn reply(
        &mut self,
        id: u64,
        sent: usize,
        what: &str,
        filters: &mut FilterCall<'_>,
    ) -> Result<Frame, RuntimeFailure> {
        let mut wait = self.wait();
        let frame = loop {
            match self.next_frame(what, &mut wait) {
                Ok(f) if f.question => {
                    if let Err(why) = self.answer(&f, filters) {
                        return Err(self.fail(&why));
                    }
                }
                Ok(f) => break f,
                Err(f) => return Err(f),
            }
        };
        let took = wait.start.elapsed();
        self.slowest = self.slowest.max(took);
        self.stats.requests += 1;
        self.stats.sent += sent as u64;
        self.stats.received += frame.body.len() as u64;
        self.stats.waited += took;
        if frame.id != id {
            let head = frame.head.clone();
            return Err(self.fail(&format!("a reply out of order: {head}")));
        }
        Ok(frame)
    }

    /// Write one line to Node's stdin.
    pub(crate) fn write_line(&mut self, line: &str) -> Result<(), RuntimeFailure> {
        if let Some(f) = &self.failed {
            return Err(f.clone());
        }
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        let sent = self
            .stdin
            .as_mut()
            .map(|s| s.write_all(&bytes).and_then(|_| s.flush()));
        if !matches!(sent, Some(Ok(()))) {
            return Err(self.fail("Node stopped reading requests"));
        }
        Ok(())
    }

    /// Node's stdin, for a thread that writes requests ahead of their
    /// replies.
    pub(crate) fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.stdin.take()
    }

    /// The next request id, and the line that sends `text` under it, with
    /// the clock and random values `feed` when the replay check hands the
    /// first run's back.
    pub(crate) fn line_for(&mut self, text: &str, feed: Option<&str>) -> (u64, String) {
        self.next_id += 1;
        let id = self.next_id;
        let mut line = String::with_capacity(text.len() + 32);
        let _ = write!(line, "{{\"id\":{id},");
        if let Some(feed) = feed {
            line.push_str("\"feed\":");
            line.push_str(feed);
            line.push(',');
        }
        line.push_str(text.strip_prefix('{').unwrap_or(text));
        (id, line)
    }

    /// Send one request and wait for its reply frame.
    pub(crate) fn exchange(
        &mut self,
        text: &str,
        feed: Option<&str>,
        what: &str,
    ) -> Result<Frame, RuntimeFailure> {
        self.exchange_with(text, feed, what, &mut |name: &str, _: &[Value]| {
            Err(format!(
                "calls `this.{name}` where Brrrty has no filters to run it"
            ))
        })
    }

    /// Answer one of the worker's questions: run the filter it names, or,
    /// in the replay check, give the first run's answer.
    pub(crate) fn answer(
        &mut self,
        frame: &Frame,
        filters: &mut FilterCall<'_>,
    ) -> Result<(), String> {
        let reply: Box<str> = match self.answers_feed.as_mut() {
            Some(feed) => match feed.pop_front() {
                Some((q, a)) if q == frame.body => a,
                _ => {
                    r#"{"refused":"asks for a filter the first run didn't ask for at this point"}"#
                        .into()
                }
            },
            None => {
                let name = frame
                    .head
                    .get("filter")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string();
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
                .to_string()
                .into()
            }
        };
        self.asked.push((frame.body.clone(), reply.clone()));
        match self.answers.as_mut() {
            Some(w) => w
                .write_all(reply.as_bytes())
                .and_then(|_| w.write_all(b"\n"))
                .and_then(|_| w.flush())
                .map_err(|e| format!("cannot answer Node: {e}")),
            None => Err("Node asked a question it can't be answered on".to_string()),
        }
    }

    /// [`Worker::exchange`], answering the worker's filter questions with
    /// `filters` while it waits.
    pub(crate) fn exchange_with(
        &mut self,
        text: &str,
        feed: Option<&str>,
        what: &str,
        filters: &mut FilterCall<'_>,
    ) -> Result<Frame, RuntimeFailure> {
        let (id, line) = self.line_for(text, feed);
        self.write_line(&line)?;
        self.reply(id, line.len(), what, filters)
    }

    /// Start waiting for a frame.
    pub(crate) fn wait(&self) -> Wait {
        let start = Instant::now();
        Wait {
            start,
            next_note: start + self.note_times.0,
            exited_at: None,
        }
    }

    /// The next frame from Node: a reply, or a question of its own.
    pub(crate) fn next_frame(
        &mut self,
        what: &str,
        wait: &mut Wait,
    ) -> Result<Frame, RuntimeFailure> {
        if let Some(f) = &self.failed {
            return Err(f.clone());
        }
        // Most replies come within microseconds: look for one a few times
        // before sleeping, which costs more than the wait.
        let spun = Instant::now();
        let early = loop {
            match self.replies.try_recv() {
                Ok(r) => break Some(r),
                Err(mpsc::TryRecvError::Disconnected) => break None,
                Err(mpsc::TryRecvError::Empty) if spun.elapsed() < SPIN => std::hint::spin_loop(),
                Err(mpsc::TryRecvError::Empty) => break None,
            }
        };
        if let Some(reply) = early {
            return reply.map_err(|why| self.fail(&why));
        }
        let reply = loop {
            match self.replies.recv_timeout(POLL) {
                Ok(r) => break r,
                Err(RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    if let Some(d) = self.deadline
                        && now.duration_since(wait.start) > d
                    {
                        break Err(format!(
                            "{what} took more than {} s, longer than the first run allows",
                            d.as_secs()
                        ));
                    }
                    if now >= wait.next_note {
                        (self.notes)(&format!(
                            "note: still waiting on {what}, {} s",
                            wait.start.elapsed().as_secs()
                        ));
                        wait.next_note = now + self.note_times.1;
                    }
                    match wait.exited_at {
                        Some(t) if now.duration_since(t) >= POLL => {
                            break Err("Node exited without replying".to_string());
                        }
                        Some(_) => {}
                        None => {
                            if matches!(self.child.try_wait(), Ok(Some(_))) {
                                wait.exited_at = Some(now);
                            }
                        }
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    break Err("Node closed its output".to_string());
                }
            }
        };
        reply.map_err(|why| self.fail(&why))
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
        if let Some(why) = reply.get("again") {
            // The worker froze the data files' values and can't vouch for
            // what it then saw: this folder's builds walk them from now on.
            walk_roots().insert(self.root.clone());
            let mut f = self.fail(&format!(
                "the JavaScript worker has to check the data files' values after every call ({})",
                why.as_str().unwrap_or("")
            ));
            f.again = true;
            self.failed = Some(f.clone());
            return JsResult::Failed(f);
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
            self.table.forget();
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

    /// Start the replay check's two workers: they load the data again, in
    /// their own orders, while this one goes on, and are sent every value
    /// this one defines as it defines it.
    fn start_replays(&mut self) {
        let loads = Arc::new(crate::replay::Loads {
            node: self.node.clone(),
            root: self.root.clone(),
            run_mode: self.run_mode,
            loads: std::mem::take(&mut self.load_log),
            global_end: self.global_end,
            load_handles: self.load_handles,
        });
        let replays = [
            crate::replay::Handle::start(loads.clone(), true),
            crate::replay::Handle::start(loads, false),
        ];
        for (line, dynamic) in &self.table.lines {
            if !dynamic {
                for r in &replays {
                    r.line(line.clone());
                }
            }
        }
        self.replays = Some(replays);
    }

    /// The worker can't go on: say why, with Node's exit status when it
    /// exited, and the last lines of its stderr.
    pub(crate) fn fail(&mut self, why: &str) -> RuntimeFailure {
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
            again: false,
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

    /// Stop Node at once (the replay check, at a difference).
    pub(crate) fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.stop_group();
        if self.failed.is_none() {
            self.failed = Some(RuntimeFailure {
                message: "the replay check stopped its Node".into(),
                stderr_tail: Vec::new(),
                stopped: false,
                again: false,
            });
        }
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

/// The filters for requests that run computed data: the worker's own.
fn no_filters() -> impl FnMut(&str, &[Value]) -> Result<Value, String> {
    |name: &str, _: &[Value]| {
        Err(format!(
            "calls `this.{name}` where Brrrty has no filters to run it"
        ))
    }
}

fn close_request(page: PageId) -> Request<'static> {
    let text = op_text("pageClose", |t| {
        let _ = write!(t, ",\"page\":{}", page.0);
    });
    Request {
        one_way: true,
        ..Request::new("pageClose", text)
    }
}

fn call_request(page: PageId, f: FnHandle, set: Option<&str>) -> Request<'static> {
    let text = op_text("pageCall", |t| {
        let _ = write!(t, ",\"page\":{},\"handle\":{},\"set\":", page.0, f.0);
        match set {
            Some(key) => crate::wire::string(t, key),
            None => t.push_str("null"),
        }
    });
    Request {
        handle: Some(f.0),
        ..Request::new("pageCall", text)
    }
}

fn vars_request(page: PageId, f: FnHandle, keys: &[String]) -> Request<'static> {
    let text = op_text("pageVars", |t| {
        let _ = write!(t, ",\"page\":{},\"handle\":{},\"keys\":", page.0, f.0);
        crate::wire::strings(t, keys);
    });
    Request {
        handle: Some(f.0),
        ..Request::new("pageVars", text)
    }
}

/// `{"op":"<op>"` and the fields `rest` writes, closed.
fn op_text(op: &str, rest: impl FnOnce(&mut String)) -> String {
    let mut t = String::with_capacity(96);
    let _ = write!(t, "{{\"op\":\"{op}\"");
    rest(&mut t);
    t.push('}');
    t
}

/// `[[key, why], ...]` as JSON text.
fn pairs(out: &mut String, traps: &[(String, String)]) {
    out.push('[');
    for (i, (k, why)) in traps.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('[');
        crate::wire::string(out, k);
        out.push(',');
        crate::wire::string(out, why);
        out.push(']');
    }
    out.push(']');
}

fn unit(r: JsResult<Decoded>) -> JsResult<()> {
    match r {
        JsResult::Ok(_) => JsResult::Ok(()),
        JsResult::Thrown(e) => JsResult::Thrown(e),
        JsResult::Refused(r) => JsResult::Refused(r),
        JsResult::Failed(f) => JsResult::Failed(f),
    }
}

impl JsRuntime for Worker {
    fn set_argument(&mut self, arg: &Value) -> JsResult<()> {
        let text = op_text("setArg", |t| {
            let _ = write!(t, ",\"value\":{}", encode(arg));
        });
        unit(self.request(Request::new("setArg", text), "the data argument"))
    }

    fn load(&mut self, file: &Path, call: bool) -> JsResult<Decoded> {
        let what = self.shown(file);
        let key = (call, file.to_path_buf());
        // The worker says `same` when the value is the one it sent last
        // time for this file, and the decoded value is used again: equal
        // data, and the same objects, which Brrrty compares cheaply.
        self.earlier = self.last_loaded.get(&key).cloned();
        let text = op_text("load", |t| {
            t.push_str(",\"path\":");
            crate::wire::string(t, &file.to_string_lossy());
            let _ = write!(t, ",\"call\":{call}");
        });
        let r = self.request(
            Request {
                path: Some(file),
                ..Request::new("load", text)
            },
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
        let mut added = Added::default();
        let mut text = String::with_capacity(128);
        let _ = write!(
            text,
            "{{\"op\":\"call\",\"handle\":{},\"await\":{}",
            f.0,
            settle == Settle::Await
        );
        let mut write = |text: &mut String| -> Result<(), String> {
            match this {
                This::Value(v) => {
                    text.push_str(",\"this\":");
                    self.table.argument(v, false, text, &mut added)?;
                }
                This::Opaque(what) => {
                    text.push_str(",\"opaque\":");
                    crate::wire::string(text, what);
                }
            }
            text.push_str(",\"args\":[");
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    text.push(',');
                }
                self.table.argument(a, false, text, &mut added)?;
            }
            text.push_str("]}");
            Ok(())
        };
        if let Err(why) = write(&mut text) {
            return JsResult::Refused(Refusal::new(why));
        }
        self.request(
            Request {
                added,
                handle: Some(f.0),
                ..Request::new("call", text)
            },
            "a data function",
        )
    }

    fn page_open(
        &mut self,
        data: &Value,
        traps: &[(String, String)],
        defaults: &[String],
    ) -> JsResult<PageId> {
        let (page, open) = match self.open_request(data, traps, defaults) {
            Ok(r) => r,
            Err(why) => return JsResult::Refused(Refusal::new(why)),
        };
        match self.request(open, "a page's data") {
            JsResult::Ok(_) => JsResult::Ok(PageId(page.0)),
            JsResult::Thrown(e) => JsResult::Thrown(e),
            JsResult::Refused(r) => JsResult::Refused(r),
            JsResult::Failed(f) => JsResult::Failed(f),
        }
    }

    fn page_vars(&mut self, page: PageId, f: FnHandle, keys: &[String]) -> JsResult<Vec<String>> {
        let r = self.request(vars_request(page, f, keys), "a computed data function");
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
        self.request(call_request(page, f, set), "a computed data function")
    }

    fn page_set(&mut self, page: PageId, key: &str, value: &Value) -> JsResult<()> {
        let mut added = Added::default();
        let mut text = String::with_capacity(128);
        let _ = write!(text, "{{\"op\":\"pageSet\",\"page\":{},\"key\":", page.0);
        crate::wire::string(&mut text, key);
        text.push_str(",\"value\":");
        if let Err(why) = self.table.argument(value, false, &mut text, &mut added) {
            return JsResult::Refused(Refusal::new(why));
        }
        text.push('}');
        unit(self.request(
            Request {
                added,
                ..Request::new("pageSet", text)
            },
            "a page's data",
        ))
    }

    fn page_close(&mut self, page: PageId) {
        let _ = self.request(close_request(page), "a page's data");
    }

    fn page_once(
        &mut self,
        data: &Value,
        traps: &[(String, String)],
        defaults: &[String],
        f: FnHandle,
    ) -> JsResult<Decoded> {
        let (page, open) = match self.open_request(data, traps, defaults) {
            Ok(r) => r,
            Err(why) => return JsResult::Refused(Refusal::new(why)),
        };
        let reqs = vec![open, call_request(page, f, None), close_request(page)];
        let mut out = self.requests(reqs, "a permalink function", &mut no_filters());
        match out.len() {
            // Opened, called, closed.
            3 => out.swap_remove(1),
            // The call, or the open, gave no value.
            _ => out
                .pop()
                .unwrap_or_else(|| JsResult::Failed(self.fail("a request with no reply"))),
        }
    }

    fn page_open_with_vars(
        &mut self,
        data: &Value,
        traps: &[(String, String)],
        defaults: &[String],
        functions: &[FnHandle],
        keys: &[String],
    ) -> JsResult<(PageId, Vec<Vec<String>>)> {
        let (page, open) = match self.open_request(data, traps, defaults) {
            Ok(r) => r,
            Err(why) => return JsResult::Refused(Refusal::new(why)),
        };
        let mut reqs = vec![open];
        reqs.extend(functions.iter().map(|f| vars_request(page, *f, keys)));
        let filters = self.filters.clone();
        let out = self.requests(
            reqs,
            "a computed data function",
            &mut |name: &str, args: &[Value]| match &filters {
                Some(f) => f(name, args),
                None => Err(format!(
                    "calls `this.{name}` where Brrrty has no filters to run it"
                )),
            },
        );
        let opened = out.first().is_some_and(|r| matches!(r, JsResult::Ok(_)));
        let mut found = Vec::with_capacity(functions.len());
        let mut bad = None;
        for r in out.into_iter().skip(usize::from(opened)) {
            match r {
                JsResult::Ok(d) => match strings(&d.value) {
                    Some(list) => found.push(list),
                    None => {
                        bad = Some(JsResult::Failed(
                            self.fail(&format!("pageVars gave {:?}", d.value)),
                        ));
                        break;
                    }
                },
                JsResult::Thrown(e) => bad = Some(JsResult::Thrown(e)),
                JsResult::Refused(r) => bad = Some(JsResult::Refused(r)),
                JsResult::Failed(f) => bad = Some(JsResult::Failed(f)),
            }
        }
        match bad {
            None if opened => JsResult::Ok((page, found)),
            None => JsResult::Failed(self.fail("pageOpen gave no reply")),
            Some(bad) => {
                if opened {
                    self.page_close(page);
                }
                bad
            }
        }
    }

    fn page_run(&mut self, page: PageId, steps: &[PageStep<'_>]) -> Vec<JsResult<Decoded>> {
        let mut reqs = Vec::with_capacity(steps.len());
        for step in steps {
            match step {
                PageStep::Call(f, set) => reqs.push(call_request(page, *f, *set)),
                PageStep::Set(key, value) => {
                    let mut added = Added::default();
                    let mut text = String::with_capacity(128);
                    let _ = write!(text, "{{\"op\":\"pageSet\",\"page\":{},\"key\":", page.0);
                    crate::wire::string(&mut text, key);
                    text.push_str(",\"value\":");
                    if let Err(why) = self.table.argument(value, false, &mut text, &mut added) {
                        // What comes before it still runs; the list ends here.
                        let mut out = self.run(reqs);
                        if out.iter().all(|r| matches!(r, JsResult::Ok(_))) {
                            out.push(JsResult::Refused(Refusal::new(why)));
                        }
                        return out;
                    }
                    text.push('}');
                    reqs.push(Request {
                        added,
                        ..Request::new("pageSet", text)
                    });
                }
            }
        }
        self.run(reqs)
    }

    fn loads_changed(&mut self) -> JsResult<Vec<(PathBuf, Decoded)>> {
        self.global_end = self.load_log.len();
        let d = match self.request(
            Request::new("loadsChanged", op_text("loadsChanged", |_| {})),
            "the data files",
        ) {
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
        let frame = match self.exchange(&op_text("callsBegin", |_| {}), None, "the data files") {
            Ok(f) => f,
            Err(f) => return JsResult::Failed(f),
        };
        let head = &frame.head;
        self.load_handles = head.get("handles").and_then(Json::as_u64).unwrap_or(0);
        self.frozen = head.get("exports").and_then(Json::as_str) == Some("frozen");
        self.walks = head.get("walks").and_then(Json::as_str).map(str::to_string);
        self.table.load_handles = Some(u32::try_from(self.load_handles).unwrap_or(u32::MAX));
        self.calls_begun = true;
        if trace() {
            self.trace("the data files loaded");
            match self.exports_frozen() {
                Ok(()) => {
                    eprintln!("note: the data files' values are frozen while pages call functions")
                }
                Err(why) => {
                    eprintln!("note: the data files' values are walked after every call ({why})")
                }
            }
        }
        let changed: Vec<&str> = head
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
        if head.get("protos").and_then(Json::as_bool) == Some(true) {
            return JsResult::Refused(Refusal {
                why: "a JavaScript data file changes one of JavaScript's built-in prototypes (`Array.prototype` and the like), which 11ty's templates use and Brrrty's don't".into(),
                impure: true,
            });
        }
        // Template and directory data files and templates load in an order
        // of 11ty's own: with any, the check runs even when no page calls
        // a function, so it starts now.
        if self.load_log.len() > self.global_end {
            self.start_replays();
        }
        JsResult::Ok(())
    }

    fn set_filters(&mut self, filters: Filters) {
        self.filters = Some(filters);
    }

    fn template_setup(&mut self, filters: &[String]) -> JsResult<()> {
        let text = op_text("templateSetup", |t| {
            t.push_str(",\"filters\":");
            crate::wire::strings(t, filters);
        });
        unit(self.request(
            Request::new("templateSetup", text),
            "the JavaScript templates' setup",
        ))
    }

    fn load_template(&mut self, file: &Path, layout: bool) -> JsResult<LoadedTemplate> {
        let what = self.shown(file);
        let text = op_text("templateLoad", |t| {
            t.push_str(",\"path\":");
            crate::wire::string(t, &file.to_string_lossy());
            let _ = write!(t, ",\"layout\":{layout}");
        });
        self.last_template = None;
        let r = self.request(
            Request {
                path: Some(file),
                ..Request::new("templateLoad", text)
            },
            &what,
        );
        match r {
            JsResult::Ok(data) => match self.last_template {
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
        let mut added = Added::default();
        let mut text = String::with_capacity(1024);
        let _ = write!(
            text,
            "{{\"op\":\"render\",\"template\":{},\"data\":",
            template.0
        );
        if let Err(why) = self.table.page_data(data, true, &mut text, &mut added) {
            return JsResult::Refused(Refusal::new(why));
        }
        text.push_str(",\"traps\":");
        pairs(&mut text, traps);
        if let Some(first) = first {
            text.push_str(",\"first\":");
            if let Err(why) = self.table.argument(first, true, &mut text, &mut added) {
                return JsResult::Refused(Refusal::new(why));
            }
        }
        text.push('}');
        self.request_with(
            Request {
                added,
                ..Request::new("render", text)
            },
            "a JavaScript template",
            filters,
        )
    }

    fn verify(&mut self) -> JsResult<()> {
        // What the calls changed of the data files' values comes first: it
        // explains whatever the replay would find.
        match self.request(
            Request::new("exportsFinal", op_text("exportsFinal", |_| {})),
            "the data files' values",
        ) {
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
        // Nothing to check: no page called a function, and no template or
        // template or directory data file loaded.
        let Some(replays) = self.replays.take() else {
            return JsResult::Ok(());
        };
        let calls = Arc::new(crate::replay::Calls {
            records: std::mem::take(&mut self.call_log),
            load_handles: self.load_handles,
            slowest: self.slowest,
            dynamic: std::mem::take(&mut self.table.dynamic),
            lines: self.table.lines.clone(),
            dropped: self.dropped,
        });
        if trace() {
            self.trace("the pages' calls ended");
        }
        let checking = Instant::now();
        let [a, b] = replays;
        a.run(calls.clone());
        b.run(calls);
        let (a, b) = (a.finish(), b.finish());
        if trace() {
            eprintln!(
                "note: the replay check took {:.3} s after the build's own calls",
                checking.elapsed().as_secs_f64()
            );
        }
        match a.and(b) {
            Ok(()) => JsResult::Ok(()),
            Err(crate::replay::Failure::Differs(why, handle)) => {
                let named = handle
                    .and_then(|h| {
                        let text = op_text("describe", |t| {
                            let _ = write!(t, ",\"handle\":{h}");
                        });
                        self.exchange(&text, None, "a function's name").ok()
                    })
                    .and_then(|f| match f.value() {
                        Some(Value::String(s)) => Some(s.to_string()),
                        _ => None,
                    })
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
        match self.request(
            Request::new("reads", op_text("reads", |_| {})),
            "the list of files read",
        ) {
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

/// The project folders whose builds found they must walk the data files'
/// values after each call rather than freeze them (see the worker's
/// "Freezing"): a build there starts that way.
fn walk_roots() -> MutexGuard<'static, std::collections::HashSet<PathBuf>> {
    static ROOTS: std::sync::LazyLock<Mutex<std::collections::HashSet<PathBuf>>> =
        std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));
    ROOTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether to say on stderr what the JavaScript worker's requests cost
/// (`BRRRTY_JS_TRACE`, for whoever looks into a slow build).
fn trace() -> bool {
    static ON: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var_os("BRRRTY_JS_TRACE").is_some());
    *ON
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

/// Kill the worker with this process id and what it started.
pub(crate) fn stop_pid(pid: u32) {
    #[cfg(unix)]
    kill_group(pid);
    #[cfg(not(unix))]
    let _ = pid;
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

/// Read `\x1eBRRRTY <kind> <id> <clock length> <body length>\n<clock><body>`
/// frames until Node closes its output. Anything else is a protocol error,
/// sent once.
fn read_frames(stdout: impl Read, tx: mpsc::Sender<Reply>, decoding: Arc<AtomicBool>) {
    let mut r = BufReader::with_capacity(1 << 16, stdout);
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
        let fields = header
            .strip_prefix(b"\x1eBRRRTY ")
            .and_then(|h| h.strip_suffix(b"\n"))
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| {
                let mut it = h.split(' ');
                let kind = it.next()?;
                let id = it.next()?.parse::<u64>().ok()?;
                let clock = it.next()?.parse::<usize>().ok()?;
                let body = it.next()?.parse::<usize>().ok()?;
                (it.next().is_none() && matches!(kind, "r" | "f" | "q"))
                    .then_some((kind, id, clock, body))
            });
        let Some((kind, id, clock_len, body_len)) = fields else {
            let shown = String::from_utf8_lossy(&header);
            let _ = tx.send(Err(format!(
                "something other than the worker wrote to its output: {:?}",
                shown.chars().take(80).collect::<String>()
            )));
            return;
        };
        let mut read = |len: usize| -> Result<String, String> {
            let mut bytes = vec![0; len];
            r.read_exact(&mut bytes)
                .map_err(|e| format!("a cut-off reply: {e}"))?;
            String::from_utf8(bytes).map_err(|_| "a reply that isn't UTF-8".to_string())
        };
        let parts = read(clock_len).and_then(|c| read(body_len).map(|b| (c, b)));
        let (clock, body) = match parts {
            Ok(p) => p,
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };
        let decode_ok = decoding.load(std::sync::atomic::Ordering::Relaxed);
        let parsed = parse_reply(body.as_bytes()).map(|mut head| {
            let value = head.as_object_mut().and_then(|o| o.remove("ok"));
            let ok = if decode_ok || kind == "q" {
                value.as_ref().map(decode)
            } else {
                None
            };
            Frame {
                question: kind == "q",
                id,
                head,
                ok,
                body: body.into_boxed_str(),
                clock: (!clock.is_empty()).then(|| clock.into_boxed_str()),
                made: kind == "f",
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
