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
    Begun, Decoded, FilterCall, Filters, Finished, FnHandle, JsError, JsResult, JsRuntime,
    LoadedTemplate, Op, PageId, PageStep, PathPart, Pending, Refusal, RuntimeFailure, Settle,
    TemplateId, This, Waited,
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

/// The worker's source as Node is handed it: without its comment lines.
/// Linux takes no single argument longer than 128 KiB (`MAX_ARG_STRLEN`),
/// and the comments are a quarter of the file. Each one leaves its line
/// behind, empty, so a line number in a stack trace is the file's.
fn worker_source() -> &'static str {
    static SOURCE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let mut out = String::with_capacity(WORKER_SOURCE.len());
        for line in WORKER_SOURCE.split('\n') {
            if !line.trim_start().starts_with("//") {
                out.push_str(line);
            }
            out.push('\n');
        }
        out.pop();
        out
    });
    &SOURCE
}

/// The 11ty version the worker tells data files it is (`ELEVENTY_VERSION`).
const ELEVENTY_VERSION: &str = "3.1.8";

/// The stack of the thread that parses replies.
pub(crate) const REPLY_STACK: usize = 32 * 1024 * 1024;

/// How many of Node's last stderr lines a failure shows.
const STDERR_TAIL: usize = 20;

/// How long a stopped worker's stderr may take to reach its end: past this,
/// a process Node started still holds it, and Brrrty stops reading.
const STDERR_GRACE: Duration = Duration::from_millis(200);

/// How often a request is sent again when the worker lost the values it
/// refers to before giving up.
const AGAIN: usize = 16;

/// Whether a request is one the replay check runs again.
fn logged(op: &str) -> bool {
    !matches!(
        op,
        "reads" | "info" | "callsBegin" | "loadsChanged" | "exportsFinal" | "describe"
    )
}

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
    /// Its place among the frames Node sent. Node runs one request at a
    /// time and answers each as it ends, so this is the order it ran them.
    pub seq: u64,
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
    /// When Node ran it: its reply's place among Node's frames, and for a
    /// request with no reply, its place after the one before it. Threads
    /// take their replies in any order, so the log is put in this order
    /// before the replay check reads it.
    pub at: (u64, u32),
}

/// Where notes about a slow request go.
type Notes = Box<dyn Fn(&str) + Send>;

/// The requests of one operation, how its results are shaped, the page it
/// opens, and what to call it in a note.
type Planned = (Vec<Request<'static>>, Shape, Option<PageId>, &'static str);

/// What a worker's threads share: Node itself, and where its frames go.
pub(crate) struct Mux {
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    /// Where Brrrty answers the worker's questions (a filter a JavaScript
    /// template calls on `this`).
    answers: Mutex<Option<Box<dyn Write + Send>>>,
    /// The requests whose replies a [`Ticket`] waits for, by id: where
    /// their frames go. Any other frame goes to the worker itself.
    routes: Mutex<HashMap<u64, mpsc::Sender<Reply>>>,
    /// Where slow-request notes go; stderr unless a test replaces it.
    notes: Mutex<Notes>,
    /// When the first note comes, and then how often.
    note_times: Mutex<(Duration, Duration)>,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Mux {
    /// The next frame on `rx`: a reply, or a question of Node's own. `Err`
    /// is why none will come.
    fn next(
        &self,
        rx: &Receiver<Reply>,
        what: &str,
        wait: &mut Wait,
        deadline: Option<Duration>,
    ) -> Result<Frame, String> {
        // Most replies come within microseconds: look for one a few times
        // before sleeping, which costs more than the wait.
        let spun = Instant::now();
        loop {
            match rx.try_recv() {
                Ok(r) => return r,
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) if spun.elapsed() < SPIN => std::hint::spin_loop(),
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        loop {
            match rx.recv_timeout(POLL) {
                Ok(r) => return r,
                Err(RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    if let Some(d) = deadline
                        && now.duration_since(wait.start) > d
                    {
                        return Err(format!(
                            "{what} took more than {} s, longer than the first run allows",
                            d.as_secs()
                        ));
                    }
                    if now >= wait.next_note {
                        // Node runs one request at a time: this one may be
                        // waiting its turn behind the others.
                        let waiting = locked(&self.routes).len();
                        let behind = if waiting > 1 {
                            format!(
                                " ({waiting} requests are waiting on Node, which runs one at a time)"
                            )
                        } else {
                            String::new()
                        };
                        (locked(&self.notes))(&format!(
                            "note: still waiting on {what}, {} s{behind}",
                            wait.start.elapsed().as_secs()
                        ));
                        wait.next_note = now + locked(&self.note_times).1;
                    }
                    match wait.exited_at {
                        Some(t) if now.duration_since(t) >= POLL => {
                            return Err("Node exited without replying".to_string());
                        }
                        Some(_) => {}
                        None => {
                            if matches!(locked(&self.child).try_wait(), Ok(Some(_))) {
                                wait.exited_at = Some(now);
                            }
                        }
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("Node closed its output".to_string());
                }
            }
        }
    }

    /// Start waiting for a frame.
    fn wait(&self) -> Wait {
        let start = Instant::now();
        Wait {
            start,
            next_note: start + locked(&self.note_times).0,
            exited_at: None,
        }
    }

    /// Run the filter a question names, and send Node the answer. The
    /// answer as sent, or why it couldn't be.
    fn answer(&self, frame: &Frame, filters: &mut FilterCall<'_>) -> Result<Box<str>, String> {
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
        let reply: Box<str> = match args.and_then(|a| filters(&name, &a)) {
            Ok(v) => json!({"ok": encode(&v)}),
            Err(why) => json!({"refused": why}),
        }
        .to_string()
        .into();
        self.send_answer(&reply)?;
        Ok(reply)
    }

    fn send_answer(&self, reply: &str) -> Result<(), String> {
        match locked(&self.answers).as_mut() {
            Some(w) => w
                .write_all(reply.as_bytes())
                .and_then(|_| w.write_all(b"\n"))
                .and_then(|_| w.flush())
                .map_err(|e| format!("cannot answer Node: {e}")),
            None => Err("Node asked a question it can't be answered on".to_string()),
        }
    }
}

/// How the results of requests sent together are handed back.
#[derive(Clone, Copy, PartialEq)]
enum Shape {
    /// As they are: the list ends with the first that isn't a value.
    List,
    /// A page opened, one function called, the page closed: the call's
    /// result.
    Once,
    /// A page opened, then what each function reads: all of them, or the
    /// first that failed (the page is then closed).
    OpenWithVars,
}

/// One request on its way.
struct Sent {
    id: u64,
    op: &'static str,
    handle: Option<u32>,
    text: String,
    path: Option<String>,
    one_way: bool,
    /// From [`Added`]: the request holds a function a call made, and the
    /// dynamic definitions it needs.
    made: bool,
    dynamic: Vec<u32>,
}

/// Requests sent together, their replies still to come. Waiting for them
/// needs no hold on the worker, so other requests go out meanwhile and
/// Node runs them back to back.
pub(crate) struct Ticket {
    mux: Arc<Mux>,
    rx: Receiver<Reply>,
    sent: Vec<Sent>,
    what: String,
    shape: Shape,
    page: Option<PageId>,
    deadline: Option<Duration>,
}

/// The replies to a [`Ticket`]'s requests, for [`Worker::settle`].
pub(crate) struct Done {
    sent: Vec<Sent>,
    /// One per request that has a reply, in order; shorter when Node
    /// stopped.
    frames: Vec<Frame>,
    /// The questions each request asked, with the answers it got.
    asked: Vec<Vec<(Box<str>, Box<str>)>>,
    failure: Option<String>,
    what: String,
    shape: Shape,
    page: Option<PageId>,
    waited: Duration,
}

impl Ticket {
    /// Wait for the replies, answering Node's questions with `filters`.
    fn collect(self, filters: &mut FilterCall<'_>) -> Done {
        let started = Instant::now();
        let mut frames = Vec::with_capacity(self.sent.len());
        let mut asked = Vec::with_capacity(self.sent.len());
        let mut failure = None;
        'all: for sent in self.sent.iter().filter(|s| !s.one_way) {
            let mut wait = self.mux.wait();
            let mut questions = Vec::new();
            loop {
                match self
                    .mux
                    .next(&self.rx, &self.what, &mut wait, self.deadline)
                {
                    Ok(f) if f.question => match self.mux.answer(&f, filters) {
                        Ok(reply) => questions.push((f.body.clone(), reply)),
                        Err(why) => {
                            failure = Some(why);
                            break 'all;
                        }
                    },
                    Ok(f) if f.id != sent.id => {
                        failure = Some(format!("a reply out of order: {}", f.head));
                        break 'all;
                    }
                    Ok(f) => {
                        frames.push(f);
                        asked.push(questions);
                        break;
                    }
                    Err(why) => {
                        failure = Some(why);
                        break 'all;
                    }
                }
            }
        }
        let mut routes = locked(&self.mux.routes);
        for sent in &self.sent {
            routes.remove(&sent.id);
        }
        drop(routes);
        Done {
            sent: self.sent,
            frames,
            asked,
            failure,
            what: self.what,
            shape: self.shape,
            page: self.page,
            waited: started.elapsed(),
        }
    }
}

impl Pending for Ticket {
    fn wait(self: Box<Self>, filters: &mut FilterCall<'_>) -> Waited {
        Waited(Box::new(self.collect(filters)))
    }
}

/// A running worker. Dropping it closes Node's stdin, waits up to 2 s for
/// it to exit, then kills it.
pub struct Worker {
    mux: Arc<Mux>,
    pid: u32,
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
    /// The loads whose value is the same every time, and whether the last
    /// load's reply said so.
    fixed: std::collections::HashSet<(bool, PathBuf)>,
    last_fixed: bool,
    /// The latest of Node's frames a request of this worker's was
    /// answered by (see [`Frame::seq`]).
    last_seq: u64,
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
    replays: Option<Vec<crate::replay::Handle>>,
    /// The other Nodes the build's calls run in (see [`Worker::node_for`]),
    /// and how many Nodes there are to be in all.
    helpers: Vec<crate::replay::Helper>,
    nodes: usize,
    /// Which Node each render's calls go to (0 is this worker's own), and
    /// each open page's.
    unit_node: HashMap<u64, usize>,
    page_node: HashMap<u64, usize>,
    next_node: usize,
    /// The Node the request being sent goes to, and the one every part
    /// of a request sent in parts goes to.
    target: usize,
    group_target: usize,
    /// A render's calls went to another Node.
    spread: bool,
    /// Where this worker's own Node's lines go once its calls begin in a
    /// build of several Nodes: a thread writes them, so that a Node busy
    /// with a long call doesn't hold up what Brrrty sends the others.
    writer: Option<mpsc::Sender<Arc<str>>>,
    /// The calls stay in this worker's own Node from here on.
    solo: bool,
    /// The longest a request took.
    slowest: Duration,
    /// How long a request may take before the worker is given up on (the
    /// replay check's workers only).
    pub(crate) deadline: Option<Duration>,
    /// The template id of the last `templateLoad` reply.
    last_template: Option<u64>,
    next_ticket: u64,
    /// What the requests cost, for `BRRRTY_JS_TRACE`.
    stats: Stats,
    /// The worker froze the data files' values when the calls began.
    frozen: bool,
    /// Why this worker was started walking, when Brrrty asked it to.
    walk: Option<String>,
    /// Why it walks them instead, when it does.
    walks: Option<String>,
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
        Worker::start_as(node, root, run_mode, None)
    }

    /// [`Worker::start`]; `replay` for one of the replay check's workers.
    pub(crate) fn start_as(
        node: &NodeInfo,
        root: &Path,
        run_mode: RunMode,
        replay: Option<&crate::replay::Loads>,
    ) -> Result<Worker, WorkerError> {
        // The build's own worker asks whether this folder's builds walk;
        // the replay check's workers do as it did.
        let walk = match replay {
            Some(loads) => loads.walk.clone(),
            None => walk_roots().starting(root),
        };
        let mut cmd = Command::new(&node.path);
        cmd.args(["--input-type=module", "--eval", worker_source()])
            .current_dir(root)
            .env("ELEVENTY_ROOT", root)
            .env("ELEVENTY_SOURCE", "cli")
            .env("ELEVENTY_RUN_MODE", run_mode.as_str())
            .env("ELEVENTY_VERSION", ELEVENTY_VERSION)
            .stdin(Stdio::piped());
        if let Some(why) = &walk {
            cmd.env("BRRRTY_JS_EXPORTS", "walk");
            cmd.env("BRRRTY_JS_WALK_WHY", why);
        } else {
            cmd.env_remove("BRRRTY_JS_WALK_WHY");
        }
        if replay.is_some() {
            cmd.env("BRRRTY_ROLE", "replay");
        }
        let cannot =
            |e: std::io::Error| WorkerError(format!("cannot start {}: {e}", node.path.display()));
        let (mut child, frames, stderr, answers) = spawn(cmd).map_err(cannot)?;
        let pid = child.id();
        live(pid, true);
        let (tx, replies) = mpsc::channel();
        let decode = Arc::new(AtomicBool::new(true));
        let decoding = decode.clone();
        let stdin = child.stdin.take();
        let mux = Arc::new(Mux {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            answers: Mutex::new(answers),
            routes: Mutex::new(HashMap::new()),
            notes: Mutex::new(Box::new(|s| eprintln!("{s}"))),
            note_times: Mutex::new((FIRST_NOTE, NEXT_NOTE)),
        });
        let routing = mux.clone();
        // A reply nests up to three JSON levels per level of the value
        // (`encode.mjs` refuses values deeper than 1000), and parsing it
        // recurses once per level: a roomy stack, not serde's 128 limit.
        std::thread::Builder::new()
            .name("brrrty-node-replies".into())
            .stack_size(REPLY_STACK)
            .spawn(move || read_frames(frames, tx, decoding, &routing))
            .map_err(|e| {
                let mut child = locked(&mux.child);
                let _ = child.kill();
                let _ = child.wait();
                WorkerError(format!("cannot read Node's replies: {e}"))
            })?;
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let tail = stderr_tail.clone();
        let stderr_thread = std::thread::spawn(move || drain_stderr(stderr, tail));
        // The other Nodes of the build start now, and load as this one does.
        let nodes = if replay.is_some() {
            1
        } else {
            build_nodes(root)
        };
        Ok(Worker {
            mux,
            pid,
            answers_feed: None,
            asked: Vec::new(),
            filters: None,
            replies,
            stderr_tail,
            stderr_thread: Some(stderr_thread),
            root: root.to_path_buf(),
            next_id: 0,
            next_page: 0,
            failed: None,
            last_loaded: HashMap::new(),
            earlier: None,
            fixed: std::collections::HashSet::new(),
            last_fixed: false,
            last_seq: 0,
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
            helpers: (1..nodes)
                .map(|_| crate::replay::Helper::start(node, root, run_mode, walk.clone()))
                .collect(),
            nodes,
            unit_node: HashMap::new(),
            page_node: HashMap::new(),
            next_node: 0,
            target: 0,
            group_target: 0,
            spread: false,
            writer: None,
            solo: false,
            slowest: Duration::ZERO,
            deadline: None,
            last_template: None,
            next_ticket: 0,
            stats: Stats {
                started: Some(Instant::now()),
                ..Stats::default()
            },
            frozen: false,
            walk,
            walks: None,
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

    /// Send slow-request notes to `notes` instead of stderr, the first
    /// after `first` and then every `next` (tests).
    pub fn set_notes(
        &mut self,
        notes: impl Fn(&str) + Send + 'static,
        first: Duration,
        next: Duration,
    ) {
        *locked(&self.mux.notes) = Box::new(notes);
        *locked(&self.mux.note_times) = (first, next);
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
        self.pid
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
        let mut out = Vec::with_capacity(reqs.len());
        let mut reqs = reqs.into_iter().peekable();
        let target = std::mem::take(&mut self.group_target);
        while reqs.peek().is_some() {
            // As many as can go in one write. Node may stop reading while
            // it waits for an answer to a question of its own, so what is
            // written past the first request must fit in the pipe.
            let mut group = Vec::new();
            let mut ahead = 0;
            while let Some(next) = reqs.peek() {
                if !group.is_empty() && ahead + next.text.len() + 48 > PIPE_SAFE {
                    break;
                }
                if !group.is_empty() {
                    ahead += next.text.len() + 48;
                }
                group.extend(reqs.next());
            }
            self.target = target;
            let ticket = match self.send(group, what, Shape::List, None) {
                Ok(t) => t,
                Err(f) => {
                    out.push(JsResult::Failed(f));
                    return out;
                }
            };
            let done = ticket.collect(filters);
            match self.settle(done) {
                Finished::Results { results, .. } => {
                    let stop = results.iter().any(|r| !matches!(r, JsResult::Ok(_)));
                    out.extend(results);
                    if stop {
                        return out;
                    }
                }
                Finished::Again => {
                    out.push(JsResult::Failed(
                        self.fail("the worker lost the values a request referred to"),
                    ));
                    return out;
                }
            }
        }
        out
    }

    /// Whether the requests can go in one write (see [`PIPE_SAFE`]).
    fn fits(reqs: &[Request<'_>]) -> bool {
        reqs.iter()
            .skip(1)
            .map(|r| r.text.len() + 48)
            .sum::<usize>()
            <= PIPE_SAFE
    }

    /// Send requests together, in one write. Their replies come to the
    /// ticket, which is waited on without the worker.
    fn send(
        &mut self,
        reqs: Vec<Request<'_>>,
        what: &str,
        shape: Shape,
        page: Option<PageId>,
    ) -> Result<Ticket, RuntimeFailure> {
        if let Some(f) = &self.failed {
            return Err(f.clone());
        }
        // Starting the replays hands them every definition so far, these
        // requests' own among them: they get each one once.
        let mut handed = false;
        if self.calls_begun && self.replays.is_none() && reqs.iter().any(|r| logged(r.op)) {
            self.start_replays();
            handed = true;
        }
        // The values the requests refer to for the first time, each in a
        // line of its own, to this worker and to the replay check's.
        for req in &reqs {
            for (line, dynamic) in &req.added.lines {
                self.stats.definitions += 1;
                self.stats.defined += line.len() as u64;
                self.write_line(line)?;
                if !handed && self.helpers.iter().any(|h| !h.useless) {
                    let own: Arc<str> = format!("{line}\n").into();
                    for h in self.helpers.iter().filter(|h| !h.useless) {
                        h.line(own.clone());
                    }
                }
                if !dynamic
                    && !handed
                    && let Some(replays) = &self.replays
                {
                    for r in replays {
                        r.line(line.clone());
                    }
                }
            }
        }
        let (tx, rx) = mpsc::channel();
        // The requests of one ticket say so: when the worker has lost the
        // values one of them refers to, it runs none of the rest.
        self.next_ticket += 1;
        let ticket = self.next_ticket;
        let mut lines = String::new();
        let mut sent = Vec::with_capacity(reqs.len());
        // The Node these go to: this worker's own, or one of the others.
        let target = std::mem::take(&mut self.target);
        let mux = match target {
            0 => self.mux.clone(),
            n => match self.helpers.get(n - 1).and_then(|h| h.mux.clone()) {
                Some(mux) => mux,
                None => return Err(self.fail("a Node of the build's is gone")),
            },
        };
        {
            let mut routes = locked(&mux.routes);
            for req in reqs {
                self.next_id += 1;
                let id = self.next_id;
                let _ = write!(lines, "{{\"id\":{id},\"t\":{ticket},");
                lines.push_str(req.text.strip_prefix('{').unwrap_or(&req.text));
                lines.push('\n');
                if !req.one_way {
                    routes.insert(id, tx.clone());
                }
                sent.push(Sent {
                    id,
                    op: req.op,
                    handle: req.handle,
                    path: req.path.map(|p| p.to_string_lossy().into_owned()),
                    one_way: req.one_way,
                    made: req.added.made,
                    dynamic: req.added.dynamic,
                    text: req.text,
                });
            }
        }
        self.stats.sent += lines.len() as u64;
        if target == 0
            && let Some(writer) = &self.writer
        {
            let _ = writer.send(lines.into());
        } else if target == 0 {
            let written = locked(&self.mux.stdin)
                .as_mut()
                .map(|s| s.write_all(lines.as_bytes()).and_then(|_| s.flush()));
            if !matches!(written, Some(Ok(()))) {
                return Err(self.fail("Node stopped reading requests"));
            }
        } else {
            self.helpers[target - 1].line(lines.into());
        }
        Ok(Ticket {
            mux,
            rx,
            sent,
            what: what.to_string(),
            shape,
            page,
            deadline: self.deadline,
        })
    }

    /// What the replies to a ticket's requests mean: each is logged for
    /// the replay check, and the results shaped as the ticket asked.
    fn settle(&mut self, done: Done) -> Finished {
        let Done {
            sent,
            frames,
            asked,
            failure,
            what,
            shape,
            page,
            waited,
        } = done;
        self.slowest = self.slowest.max(waited);
        self.stats.waited += waited;
        let mut results: Vec<JsResult<Decoded>> = Vec::with_capacity(frames.len());
        // A call in the group changed a value the worker kept: the ones
        // after it ran on what it left, which a replay doesn't have.
        let mut unchecked = false;
        // When Node ran the last request of these that it answered.
        let mut last_at = (self.last_seq, 0u32);
        let mut frames = frames.into_iter();
        let mut asked = asked.into_iter();
        for req in sent {
            let mut frame = if req.one_way {
                None
            } else {
                match frames.next() {
                    Some(f) => Some(f),
                    // Node stopped before this one.
                    None => break,
                }
            };
            let answers = if req.one_way {
                Vec::new()
            } else {
                asked.next().unwrap_or_default()
            };
            if let Some(f) = &frame {
                self.stats.requests += 1;
                self.stats.received += f.body.len() as u64;
                if f.head.get("stale").is_some() {
                    // The worker forgot the values this refers to (a call
                    // another page made changed one): it ran nothing of
                    // this request or the ones sent with it after it. They
                    // go again, the values with them.
                    if results.iter().all(|r| matches!(r, JsResult::Ok(_))) {
                        self.table.forget();
                        return Finished::Again;
                    }
                    break;
                }
                // A step of the page before this one failed: the worker
                // ran neither this nor the ones after it.
                if f.head.get("after").is_some() {
                    break;
                }
            }
            if req.op == "templateLoad" {
                self.last_template = frame
                    .as_ref()
                    .and_then(|f| f.head.get("template"))
                    .and_then(Json::as_u64);
            }
            if req.op == "load" {
                self.last_fixed = frame
                    .as_ref()
                    .is_some_and(|f| f.head.get("fixed").is_some());
            }
            let body = frame
                .as_mut()
                .map(|f| std::mem::take(&mut f.body))
                .unwrap_or_default();
            let clock = frame.as_mut().and_then(|f| f.clock.take());
            if logged(req.op) && !self.calls_begun {
                for h in self.helpers.iter().filter(|h| !h.useless) {
                    h.load(&req.text, clock.as_deref(), &body);
                }
                self.load_log.push(LoadRecord {
                    text: req.text.into_boxed_str(),
                    kind: match req.op {
                        "load" => LoadKind::Data,
                        "templateLoad" => LoadKind::Template,
                        _ => LoadKind::Other,
                    },
                    path: req.path.unwrap_or_default(),
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
                if (req.made
                    || frame.as_ref().is_some_and(|f| f.made)
                    || req.handle.is_some_and(|h| self.table.made_by_call(h)))
                    && let Some(f) = self.lone("a call made a function")
                {
                    return Finished::Results {
                        results: vec![JsResult::Failed(f)],
                        page: None,
                    };
                }
                let at = match &frame {
                    Some(f) => {
                        self.last_seq = self.last_seq.max(f.seq);
                        last_at = (f.seq, 0);
                        last_at
                    }
                    None => {
                        last_at.1 += 1;
                        last_at
                    }
                };
                self.call_log.push(Record {
                    at,
                    unit,
                    label,
                    op: req.op,
                    handle: req.handle,
                    text: req.text.into_boxed_str(),
                    body,
                    clock,
                    made: frame.as_ref().is_some_and(|f| f.made),
                    names_made: req.made || req.handle.is_some_and(|h| self.table.made_by_call(h)),
                    dynamic: req.dynamic,
                    answers,
                    one_way: req.one_way,
                    unchecked,
                });
                unchecked |= dropped;
            }
            let Some(frame) = frame else {
                continue;
            };
            let r = self.interpret(frame, &what);
            let failed = matches!(r, JsResult::Failed(_));
            results.push(r);
            if failed {
                return Finished::Results {
                    results: vec![results.pop().expect("just pushed")],
                    page: None,
                };
            }
        }
        if let Some(why) = failure {
            return Finished::Results {
                results: vec![JsResult::Failed(self.fail(&why))],
                page: None,
            };
        }
        self.shape(shape, page, results)
    }

    /// The results of an operation whose requests went one part at a time.
    fn shaped(&mut self, done: Done, results: Vec<JsResult<Decoded>>) -> Finished {
        self.shape(done.shape, done.page, results)
    }

    fn shape(
        &mut self,
        shape: Shape,
        page: Option<PageId>,
        mut results: Vec<JsResult<Decoded>>,
    ) -> Finished {
        // The list ends with the first reply that isn't a value.
        let bad = results.iter().position(|r| !matches!(r, JsResult::Ok(_)));
        match shape {
            Shape::List => {
                if let Some(bad) = bad {
                    results.truncate(bad + 1);
                }
                Finished::Results { results, page }
            }
            Shape::Once => {
                let result = match bad {
                    Some(bad) => results.swap_remove(bad),
                    // Opened, then called.
                    None if results.len() == 2 => results.swap_remove(1),
                    None => JsResult::Failed(self.fail("a page's call gave no reply")),
                };
                Finished::Results {
                    results: vec![result],
                    page,
                }
            }
            Shape::OpenWithVars => match bad {
                Some(bad) => {
                    if bad > 0
                        && let Some(page) = page
                    {
                        self.page_close(page);
                    }
                    Finished::Results {
                        results: vec![results.swap_remove(bad)],
                        page: None,
                    }
                }
                None => Finished::Results { results, page },
            },
        }
    }

    /// The requests an operation makes, how its results are shaped, and
    /// the page it opens. `Err` when a value can't be handed over.
    fn op_requests(&mut self, op: Op<'_>) -> Result<Planned, String> {
        Ok(match op {
            Op::Call {
                f,
                this,
                args,
                settle,
            } => {
                let mut added = Added::default();
                let mut text = String::with_capacity(128);
                let _ = write!(
                    text,
                    "{{\"op\":\"call\",\"handle\":{},\"await\":{}",
                    f.0,
                    settle == Settle::Await
                );
                match this {
                    This::Value(v) => {
                        text.push_str(",\"this\":");
                        self.table.argument(v, false, &mut text, &mut added)?;
                    }
                    This::Opaque(what) => {
                        text.push_str(",\"opaque\":");
                        crate::wire::string(&mut text, what);
                    }
                }
                text.push_str(",\"args\":[");
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        text.push(',');
                    }
                    self.table.argument(a, false, &mut text, &mut added)?;
                }
                text.push(']');
                // Whose template is calling: the worker finds that page's
                // own data among the collections' items by it.
                if let Some((page, number)) = brrrty_js::functions::own_page() {
                    text.push_str(",\"own\":");
                    self.table.argument(&page, false, &mut text, &mut added)?;
                    if let Some(n) = number {
                        text.push_str(",\"ownAt\":");
                        crate::wire::number(&mut text, n);
                    }
                }
                text.push('}');
                let req = Request {
                    added,
                    handle: Some(f.0),
                    ..Request::new("call", text)
                };
                (vec![req], Shape::List, None, "a data function")
            }
            Op::PageOnce {
                data,
                traps,
                defaults,
                f,
            } => {
                let (page, open) = self.open_request(data, traps, defaults)?;
                (
                    vec![open, call_request(page, f, None), close_request(page)],
                    Shape::Once,
                    None,
                    "a permalink function",
                )
            }
            Op::PageOpenWithVars {
                data,
                traps,
                defaults,
                functions,
                keys,
            } => {
                let (page, open) = self.open_request(data, traps, defaults)?;
                let mut reqs = vec![open];
                reqs.extend(functions.iter().map(|f| vars_request(page, *f, keys)));
                (
                    reqs,
                    Shape::OpenWithVars,
                    Some(page),
                    "a computed data function",
                )
            }
            Op::PageRun { page, steps } => {
                let mut reqs = Vec::with_capacity(steps.len());
                for step in steps {
                    match step {
                        PageStep::Call(f, set) => reqs.push(call_request(page, *f, *set)),
                        PageStep::Set(key, value) => {
                            let mut added = Added::default();
                            let mut text = String::with_capacity(128);
                            let _ =
                                write!(text, "{{\"op\":\"pageSet\",\"page\":{},\"key\":", page.0);
                            crate::wire::string(&mut text, key);
                            text.push_str(",\"value\":");
                            self.table.argument(value, false, &mut text, &mut added)?;
                            text.push('}');
                            reqs.push(Request {
                                added,
                                ..Request::new("pageSet", text)
                            });
                        }
                    }
                }
                (reqs, Shape::List, None, "a computed data function")
            }
            Op::Render {
                template,
                data,
                first,
                traps,
            } => {
                let mut added = Added::default();
                let mut text = String::with_capacity(1024);
                let _ = write!(
                    text,
                    "{{\"op\":\"render\",\"template\":{},\"data\":",
                    template.0
                );
                self.table.page_data(data, true, &mut text, &mut added)?;
                text.push_str(",\"traps\":");
                pairs(&mut text, traps);
                if let Some(first) = first {
                    text.push_str(",\"first\":");
                    self.table.argument(first, true, &mut text, &mut added)?;
                }
                text.push('}');
                let req = Request {
                    added,
                    ..Request::new("render", text)
                };
                (vec![req], Shape::List, None, "a JavaScript template")
            }
        })
    }

    /// [`Worker::now`], for an operation with one result.
    fn one(&mut self, op: Op<'_>, filters: &mut FilterCall<'_>) -> JsResult<Decoded> {
        self.now(op, filters)
            .pop()
            .unwrap_or_else(|| JsResult::Failed(self.fail("a request with no reply")))
    }

    /// Run an operation and wait for it here.
    fn now(&mut self, op: Op<'_>, filters: &mut FilterCall<'_>) -> Vec<JsResult<Decoded>> {
        for _ in 0..AGAIN {
            let finished = match self.begin(op, filters) {
                Begun::Done(f) => f,
                Begun::Sent(pending) => {
                    let waited = pending.wait(filters);
                    self.finish(waited)
                }
            };
            if let Finished::Results { results, .. } = finished {
                return results;
            }
        }
        vec![JsResult::Failed(self.fail(
            "the worker kept losing the values a request referred to",
        ))]
    }

    /// Write one line to Node's stdin.
    pub(crate) fn write_line(&mut self, line: &str) -> Result<(), RuntimeFailure> {
        if let Some(f) = &self.failed {
            return Err(f.clone());
        }
        if let Some(writer) = &self.writer {
            let _ = writer.send(format!("{line}\n").into());
            return Ok(());
        }
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        let sent = locked(&self.mux.stdin)
            .as_mut()
            .map(|s| s.write_all(&bytes).and_then(|_| s.flush()));
        if !matches!(sent, Some(Ok(()))) {
            return Err(self.fail("Node stopped reading requests"));
        }
        Ok(())
    }

    /// Where this worker's Node's frames go.
    pub(crate) fn mux(&self) -> Arc<Mux> {
        self.mux.clone()
    }

    /// Node's stdin, for a thread that writes requests ahead of their
    /// replies.
    pub(crate) fn take_stdin(&mut self) -> Option<ChildStdin> {
        locked(&self.mux.stdin).take()
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
            Some(feed) => {
                let reply: Box<str> = match feed.pop_front() {
                    Some((q, a)) if q == frame.body => a,
                    _ => r#"{"refused":"asks for a filter the first run didn't ask for at this point"}"#
                        .into(),
                };
                self.mux.send_answer(&reply)?;
                reply
            }
            None => self.mux.answer(frame, filters)?,
        };
        self.asked.push((frame.body.clone(), reply));
        Ok(())
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
        self.stats.sent += line.len() as u64;
        self.stats.received += frame.body.len() as u64;
        self.stats.waited += took;
        if frame.id != id {
            let head = frame.head.clone();
            return Err(self.fail(&format!("a reply out of order: {head}")));
        }
        Ok(frame)
    }

    /// Start waiting for a frame.
    pub(crate) fn wait(&self) -> Wait {
        self.mux.wait()
    }

    /// The next frame from Node that no ticket waits for: a reply, or a
    /// question of its own.
    pub(crate) fn next_frame(
        &mut self,
        what: &str,
        wait: &mut Wait,
    ) -> Result<Frame, RuntimeFailure> {
        if let Some(f) = &self.failed {
            return Err(f.clone());
        }
        let mux = self.mux.clone();
        mux.next(&self.replies, what, wait, self.deadline)
            .map_err(|why| self.fail(&why))
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
            walk_roots().sent_back(&self.root, why.as_str().unwrap_or(""));
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
    /// The Node the calls of the render on this thread go to.
    ///
    /// One Node runs one call at a time, and on a site with many pages it
    /// is what the build waits on. So once the pages' calls begin, more
    /// Nodes load the data as this one did (see `Helper`), and each
    /// render's calls, all of them, go to one Node: the renders are dealt
    /// between the Nodes that are ready. A site whose results don't
    /// depend on the order of its calls gives the same results however
    /// its renders are dealt, and whether it is such a site is what the
    /// replay check finds out, each replay in one Node, as before.
    ///
    /// What needs every call in one Node keeps to it: see [`Worker::lone`].
    fn node_for(&mut self) -> usize {
        let Some((unit, _)) = brrrty_js::functions::unit() else {
            return 0;
        };
        if let Some(&n) = self.unit_node.get(&unit) {
            return n;
        }
        let mut ready = vec![0];
        if !self.solo {
            let wait = build_waits();
            for (i, h) in self.helpers.iter_mut().enumerate() {
                if h.usable(wait) {
                    ready.push(i + 1);
                }
            }
        }
        let n = ready[self.next_node % ready.len()];
        self.next_node += 1;
        self.unit_node.insert(unit, n);
        if n != 0 {
            self.spread = true;
        }
        n
    }

    /// Something only one Node can do right has come up: a call made a
    /// function (which lives in the Node that made it), or a template or
    /// data file loads after the calls began. If every call so far ran in
    /// this worker's own Node, the rest do too. Otherwise the build runs
    /// again in one Node; `Some` is that request to run again.
    fn lone(&mut self, why: &str) -> Option<RuntimeFailure> {
        if self.nodes <= 1 {
            return None;
        }
        self.solo = true;
        for h in &mut self.helpers {
            h.give_up();
        }
        if !self.spread {
            return None;
        }
        solo_roots().insert(self.root.clone());
        let mut f = self.fail(&format!(
            "the JavaScript worker has to run every call in one Node ({why})"
        ));
        f.again = true;
        self.failed = Some(f.clone());
        Some(f)
    }

    fn start_replays(&mut self) {
        let loads = Arc::new(crate::replay::Loads {
            node: self.node.clone(),
            root: self.root.clone(),
            run_mode: self.run_mode,
            loads: std::mem::take(&mut self.load_log),
            global_end: self.global_end,
            load_handles: self.load_handles,
            walk: self.walk.clone(),
            // The replays load every file again, several in one request:
            // none of it should take longer than all of it did here.
            took: self.stats.started.map(|t| t.elapsed()).unwrap_or_default(),
            frozen: self.frozen,
        });
        use crate::replay::Order;
        let mut replays = vec![
            crate::replay::Handle::start(loads.clone(), Order::Interleaved),
            crate::replay::Handle::start(loads.clone(), Order::Named),
        ];
        if self.nodes > 1 {
            // In several Nodes the build's renders have no order between
            // them for the check to differ from, so a third replay runs
            // them whole in the reverse of name order: with the named one,
            // every two renders are seen both ways round.
            replays.push(crate::replay::Handle::start(
                loads.clone(),
                Order::NamedReversed,
            ));
        }
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
        self.writer = None;
        drop(locked(&self.mux.stdin).take());
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            let mut child = locked(&self.mux.child);
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if Instant::now() < deadline => {
                    drop(child);
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => {
                    let _ = child.kill();
                    break child.wait().ok();
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
        {
            let mut child = locked(&self.mux.child);
            let _ = child.kill();
            let _ = child.wait();
        }
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
        kill_group(self.pid);
        live(self.pid, false);
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
        if self.calls_begun
            && let Some(f) = self.lone("a data file loads after the pages' calls began")
        {
            return JsResult::Failed(f);
        }
        let what = self.shown(file);
        let key = (call, file.to_path_buf());
        // The worker says `same` when the value is the one it sent last
        // time for this file, and the decoded value is used again: equal
        // data, and the same objects, which Brrrty compares cheaply.
        self.earlier = self.last_loaded.get(&key).cloned();
        // What a module exports as it is (no function to call, no getter)
        // is the same every time: one request, however many templates the
        // file applies to.
        if self.fixed.contains(&key)
            && let Some(d) = self.earlier.take()
        {
            return JsResult::Ok(d);
        }
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
            if self.last_fixed {
                self.fixed.insert(key.clone());
            }
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
        let op = Op::Call {
            f,
            this,
            args,
            settle,
        };
        self.one(op, &mut no_filters())
    }

    fn begin(&mut self, op: Op<'_>, filters: &mut FilterCall<'_>) -> Begun {
        let target = match &op {
            Op::PageRun { page, .. } => self.page_node.get(&page.0).copied().unwrap_or(0),
            _ => self.node_for(),
        };
        let (reqs, shape, page, what) = match self.op_requests(op) {
            Ok(r) => r,
            Err(why) => {
                return Begun::Done(Finished::Results {
                    results: vec![JsResult::Refused(Refusal::new(why))],
                    page: None,
                });
            }
        };
        if let Some(page) = page
            && target != 0
        {
            self.page_node.insert(page.0, target);
        }
        self.group_target = target;
        self.target = target;
        if !Worker::fits(&reqs) {
            // Too much to send ahead of the first reply: one part at a
            // time, here.
            let results = self.requests(reqs, what, filters);
            let done = Done {
                sent: Vec::new(),
                frames: Vec::new(),
                asked: Vec::new(),
                failure: None,
                what: what.to_string(),
                shape,
                page,
                waited: Duration::ZERO,
            };
            return Begun::Done(self.shaped(done, results));
        }
        match self.send(reqs, what, shape, page) {
            Ok(ticket) => Begun::Sent(Box::new(ticket)),
            Err(f) => Begun::Done(Finished::Results {
                results: vec![JsResult::Failed(f)],
                page: None,
            }),
        }
    }

    fn finish(&mut self, waited: Waited) -> Finished {
        match waited.0.downcast::<Done>() {
            Ok(done) => self.settle(*done),
            Err(_) => Finished::Results {
                results: vec![JsResult::Failed(
                    self.fail("replies that aren't this worker's"),
                )],
                page: None,
            },
        }
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
        self.group_target = self.page_node.remove(&page.0).unwrap_or(0);
        // Nothing comes back from it.
        let _ = self.requests(
            vec![close_request(page)],
            "a page's data",
            &mut no_filters(),
        );
    }

    fn page_once(
        &mut self,
        data: &Value,
        traps: &[(String, String)],
        defaults: &[String],
        f: FnHandle,
    ) -> JsResult<Decoded> {
        let op = Op::PageOnce {
            data,
            traps,
            defaults,
            f,
        };
        self.one(op, &mut no_filters())
    }

    fn page_run(&mut self, page: PageId, steps: &[PageStep<'_>]) -> Vec<JsResult<Decoded>> {
        let filters = self.filters.clone();
        self.now(
            Op::PageRun { page, steps },
            &mut |name: &str, args: &[Value]| match &filters {
                Some(f) => f(name, args),
                None => Err(format!(
                    "calls `this.{name}` where Brrrty has no filters to run it"
                )),
            },
        )
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
        for h in &self.helpers {
            h.begin(self.load_handles, self.frozen);
        }
        if !self.helpers.is_empty()
            && let Some(mut stdin) = locked(&self.mux.stdin).take()
        {
            let (tx, rx) = mpsc::channel::<Arc<str>>();
            let spawned = std::thread::Builder::new()
                .name("brrrty-node-write".into())
                .spawn(move || {
                    for line in rx {
                        if stdin
                            .write_all(line.as_bytes())
                            .and_then(|_| stdin.flush())
                            .is_err()
                        {
                            break;
                        }
                    }
                });
            if spawned.is_ok() {
                self.writer = Some(tx);
            }
        }
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
        if self.calls_begun
            && let Some(f) = self.lone("the templates are set up after the pages' calls began")
        {
            return JsResult::Failed(f);
        }
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
        if self.calls_begun
            && let Some(f) = self.lone("a template loads after the pages' calls began")
        {
            return JsResult::Failed(f);
        }
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
        let op = Op::Render {
            template,
            data,
            first,
            traps,
        };
        self.one(op, filters)
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
        // The same of every other Node that ran calls.
        for i in 0..self.helpers.len() {
            if self.helpers[i].mux.is_none() {
                continue;
            }
            self.group_target = i + 1;
            let results = self.requests(
                vec![Request::new(
                    "exportsFinal",
                    op_text("exportsFinal", |_| {}),
                )],
                "the data files' values",
                &mut no_filters(),
            );
            match results.into_iter().next() {
                Some(JsResult::Ok(_)) => {}
                Some(JsResult::Refused(r)) => return JsResult::Refused(r),
                Some(JsResult::Thrown(e)) => return JsResult::Thrown(e),
                Some(JsResult::Failed(f)) => return JsResult::Failed(f),
                None => return JsResult::Failed(self.fail("a Node of the build's gave no reply")),
            }
        }
        if self.frozen {
            walk_roots().froze(&self.root);
        }
        let Some(replays) = self.replays.take() else {
            return JsResult::Ok(());
        };
        let calls = Arc::new(crate::replay::Calls {
            records: {
                // The order Node ran them in, not the order the render
                // threads took their replies: the replay check's orders are
                // made from this one, and must differ from it.
                let mut records = std::mem::take(&mut self.call_log);
                records.sort_by_key(|r| r.at);
                records
            },
            load_handles: self.load_handles,
            slowest: self.slowest,
            dynamic: std::mem::take(&mut self.table.dynamic),
            lines: self.table.lines.clone(),
            dropped: self.dropped,
            by_name: self.nodes > 1,
        });
        if trace() {
            self.trace("the pages' calls ended");
            if self.nodes > 1 {
                let mut each = vec![0usize; self.nodes];
                for n in self.unit_node.values() {
                    each[*n] += 1;
                }
                eprintln!(
                    "note: the build's renders ran in {} Nodes: {each:?}",
                    self.nodes
                );
            }
        }
        let checking = Instant::now();
        for r in &replays {
            r.run(calls.clone());
        }
        drop(calls);
        // The first that failed, in the replays' own order.
        let checked: Vec<_> = replays.into_iter().map(|r| r.finish()).collect();
        let checked = checked.into_iter().find(|r| r.is_err()).unwrap_or(Ok(()));
        if trace() {
            eprintln!(
                "note: the replay check took {:.3} s after the build's own calls",
                checking.elapsed().as_secs_f64()
            );
        }
        match checked {
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
            Err(crate::replay::Failure::Again(why)) => {
                walk_roots().sent_back(&self.root, &why);
                let mut f = self.fail(&format!(
                    "the JavaScript worker has to check the data files' values after every call ({why})"
                ));
                f.again = true;
                self.failed = Some(f.clone());
                JsResult::Failed(f)
            }
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
        if let Some(writer) = self.writer.take() {
            let _ = writer.send("{\"op\":\"bye\"}\n".into());
        }
        if let Some(mut stdin) = locked(&self.mux.stdin).take() {
            let _ = stdin.write_all(b"{\"op\":\"bye\"}\n");
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut child = locked(&self.mux.child);
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    drop(child);
                    std::thread::sleep(Duration::from_millis(5))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
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
/// The folders whose builds run every call in one Node: a build there
/// was sent back for it (see [`Worker::lone`]).
fn solo_roots() -> MutexGuard<'static, std::collections::HashSet<PathBuf>> {
    static ROOTS: std::sync::LazyLock<Mutex<std::collections::HashSet<PathBuf>>> =
        std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));
    ROOTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// How many Nodes a build's calls run in: half the cores, at most 4, and
/// one where there are only two cores (the replay check's Nodes want
/// them). `BRRRTY_JS_BUILD_NODES` says another number.
fn build_nodes(root: &Path) -> usize {
    static ASKED: std::sync::LazyLock<Option<usize>> = std::sync::LazyLock::new(|| {
        std::env::var("BRRRTY_JS_BUILD_NODES")
            .ok()
            .and_then(|v| v.parse().ok())
    });
    if solo_roots().contains(root) {
        return 1;
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    ASKED
        .unwrap_or(cores / 2)
        .clamp(1, 8)
        .min(if ASKED.is_some() { 8 } else { 4 })
}

/// Whether a render waits for the other Nodes to be ready before its
/// first call (`BRRRTY_JS_BUILD_WAIT`, for the tests: a small site is
/// built before another Node has loaded its data).
fn build_waits() -> bool {
    static ON: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var_os("BRRRTY_JS_BUILD_WAIT").is_some());
    *ON
}

fn walk_roots() -> MutexGuard<'static, WalkMemo> {
    static ROOTS: std::sync::LazyLock<Mutex<WalkMemo>> =
        std::sync::LazyLock::new(|| Mutex::new(WalkMemo::default()));
    ROOTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Which folders' builds walk, and for how long. A build that was sent
/// back runs again walking; the build after that tries the freeze again
/// (under `brrrty serve` the code may have been fixed). A folder sent
/// back again and again walks for twice as many builds each time, up to
/// 64, so a site that can't be frozen stops paying for the try.
#[derive(Default)]
pub(crate) struct WalkMemo {
    roots: HashMap<PathBuf, Walks>,
}

struct Walks {
    why: String,
    /// How many builds still start walking.
    left: u32,
    /// How many it was the last time the folder was sent back.
    last: u32,
}

impl WalkMemo {
    const MOST: u32 = 64;

    /// A build of `root` was sent back.
    pub(crate) fn sent_back(&mut self, root: &Path, why: &str) {
        let w = self.roots.entry(root.to_path_buf()).or_insert(Walks {
            why: String::new(),
            left: 0,
            last: 0,
        });
        // Several of one build's requests may say so.
        if w.left > 0 {
            return;
        }
        w.why = why.to_string();
        w.last = (w.last * 2).clamp(1, Self::MOST);
        w.left = w.last;
    }

    /// A build of `root` starts: why it walks, if it does.
    pub(crate) fn starting(&mut self, root: &Path) -> Option<String> {
        let w = self.roots.get_mut(root)?;
        if w.left == 0 {
            return None;
        }
        w.left -= 1;
        Some(w.why.clone())
    }

    /// A build of `root` froze and passed.
    pub(crate) fn froze(&mut self, root: &Path) {
        self.roots.remove(root);
    }
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
    // Elsewhere there is no group: Windows stops the tree by name.
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(any(unix, windows)))]
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
fn read_frames(stdout: impl Read, tx: mpsc::Sender<Reply>, decoding: Arc<AtomicBool>, mux: &Mux) {
    read_frames_to(stdout, &tx, decoding, mux);
    // No more frames: whoever waits for one learns so.
    locked(&mux.routes).clear();
}

fn read_frames_to(
    stdout: impl Read,
    tx: &mpsc::Sender<Reply>,
    decoding: Arc<AtomicBool>,
    mux: &Mux,
) {
    let mut r = BufReader::with_capacity(1 << 16, stdout);
    let mut seq = 0u64;
    loop {
        seq += 1;
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
                seq,
            }
        });
        // A frame a ticket waits for goes to it; any other to the worker.
        let route = match &parsed {
            Ok(f) => locked(&mux.routes).get(&f.id).cloned(),
            Err(_) => None,
        };
        let sent = match route {
            Some(to) => to.send(parsed).is_ok(),
            None => tx.send(parsed).is_ok(),
        };
        if !sent && locked(&mux.routes).is_empty() {
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

#[cfg(test)]
mod source {
    /// Linux refuses an argument of 128 KiB or more ("Argument list too
    /// long"), on any machine, whatever else is on the command line.
    /// macOS doesn't, so this is where a worker grown too long shows.
    #[test]
    fn the_worker_fits_in_one_argument_with_room_to_grow() {
        let source = super::worker_source();
        assert!(
            source.len() < 120 * 1024,
            "the worker's source is {} bytes without its comments",
            source.len()
        );
        // Line numbers are the file's.
        assert_eq!(source.lines().count(), super::WORKER_SOURCE.lines().count());
        assert!(!source.lines().any(|l| l.trim_start().starts_with("//")));
    }
}

#[cfg(test)]
mod walk_memo {
    use super::WalkMemo;
    use std::path::Path;

    /// A build sent back runs again walking, and the one after tries the
    /// freeze again; a folder sent back every time walks for longer and
    /// longer, and one that freezes is forgotten.
    #[test]
    fn a_folder_sent_back_walks_for_a_while_not_for_good() {
        let root = Path::new("/site");
        let mut m = WalkMemo::default();
        assert_eq!(m.starting(root), None);
        m.sent_back(root, "why");
        // Several requests of the one build may say so.
        m.sent_back(root, "another request");
        assert_eq!(m.starting(root).as_deref(), Some("why"));
        // The build after the rerun tries again.
        assert_eq!(m.starting(root), None);
        m.sent_back(root, "again");
        assert_eq!(m.starting(root).as_deref(), Some("again"));
        assert_eq!(m.starting(root).as_deref(), Some("again"));
        assert_eq!(m.starting(root), None);
        m.sent_back(root, "third");
        let walked = std::iter::from_fn(|| m.starting(root)).count();
        assert_eq!(walked, 4);
        for _ in 0..10 {
            m.sent_back(root, "more");
            while m.starting(root).is_some() {}
        }
        m.sent_back(root, "most");
        assert_eq!(std::iter::from_fn(|| m.starting(root)).count(), 64);
        // A build that froze and passed starts over.
        m.froze(root);
        m.sent_back(root, "new");
        assert_eq!(std::iter::from_fn(|| m.starting(root)).count(), 1);
    }
}
