//! One Node process running the worker, and the requests Brrrty makes of it.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use brrrty_js::Value;
use brrrty_js::runtime::{Decoded, FnHandle, JsError, JsResult, JsRuntime, RuntimeFailure};
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

/// How many of Node's last stderr lines a failure shows.
const STDERR_TAIL: usize = 20;

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

/// One reply frame, or why there is none.
type Reply = Result<Json, String>;

/// A running worker. Dropping it closes Node's stdin, waits up to 2 s for
/// it to exit, then kills it.
pub struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<Reply>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    stderr_thread: Option<JoinHandle<()>>,
    root: PathBuf,
    next_id: u64,
    /// Set once the worker failed: every later request fails the same way.
    failed: Option<RuntimeFailure>,
    /// Where slow-request notes go; stderr unless a test replaces it.
    notes: Box<dyn Fn(&str) + Send>,
    /// When the first note comes, and then how often.
    note_times: (Duration, Duration),
}

impl Worker {
    /// Start Node with the worker, in `root` (the project folder), with
    /// 11ty's environment variables added to Brrrty's own.
    pub fn start(node: &NodeInfo, root: &Path, run_mode: RunMode) -> Result<Worker, WorkerError> {
        let mut child = Command::new(&node.path)
            .args(["--input-type=module", "--eval", WORKER_SOURCE])
            .current_dir(root)
            .env("ELEVENTY_ROOT", root)
            .env("ELEVENTY_SOURCE", "cli")
            .env("ELEVENTY_RUN_MODE", run_mode.as_str())
            .env("ELEVENTY_VERSION", ELEVENTY_VERSION)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| WorkerError(format!("cannot start {}: {e}", node.path.display())))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (tx, replies) = mpsc::channel();
        std::thread::spawn(move || read_frames(stdout, tx));
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let tail = stderr_tail.clone();
        let stderr_thread = std::thread::spawn(move || drain_stderr(stderr, tail));
        Ok(Worker {
            stdin: child.stdin.take(),
            child,
            replies,
            stderr_tail,
            stderr_thread: Some(stderr_thread),
            root: root.to_path_buf(),
            next_id: 0,
            failed: None,
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
    fn request(&mut self, mut req: Json, what: &str) -> JsResult<Json> {
        if let Some(f) = &self.failed {
            return JsResult::Failed(f.clone());
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
            return JsResult::Failed(self.fail("Node stopped reading requests"));
        }
        let start = Instant::now();
        let mut wait = self.note_times.0;
        let reply = loop {
            match self.replies.recv_timeout(wait) {
                Ok(r) => break r,
                Err(RecvTimeoutError::Timeout) => {
                    (self.notes)(&format!(
                        "note: still waiting on {what}, {} s",
                        start.elapsed().as_secs()
                    ));
                    wait = self.note_times.1;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    break Err("Node closed its output".to_string());
                }
            }
        };
        let reply = match reply {
            Ok(r) => r,
            Err(why) => return JsResult::Failed(self.fail(&why)),
        };
        if reply.get("id") != Some(&json!(id)) {
            return JsResult::Failed(self.fail(&format!("a reply out of order: {reply}")));
        }
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
        match reply.get("ok") {
            Some(v) => JsResult::Ok(v.clone()),
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
        if let Some(t) = self.stderr_thread.take() {
            let _ = t.join();
        }
        let message = match status {
            Some(s) => match (s.code(), signal_of(&s)) {
                (Some(code), _) => format!("Node exited with status {code} ({why})"),
                (None, Some(sig)) => format!("Node was killed by signal {sig} ({why})"),
                _ => why.to_string(),
            },
            None => why.to_string(),
        };
        let failure = RuntimeFailure {
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

    fn decoded(&mut self, r: JsResult<Json>) -> JsResult<Decoded> {
        match r {
            JsResult::Ok(v) => match decode(&v) {
                Ok(d) => JsResult::Ok(d),
                Err(e) => JsResult::Failed(self.fail(&format!("a value Brrrty can't read: {e}"))),
            },
            JsResult::Thrown(e) => JsResult::Thrown(e),
            JsResult::Failed(f) => JsResult::Failed(f),
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
            JsResult::Failed(f) => JsResult::Failed(f),
        }
    }

    fn load(&mut self, file: &Path, call: bool) -> JsResult<Decoded> {
        let what = self.shown(file);
        let r = self.request(
            json!({"op": "load", "path": file.to_string_lossy(), "call": call}),
            &what,
        );
        self.decoded(r)
    }

    fn call(&mut self, f: FnHandle, args: &[Value]) -> JsResult<Decoded> {
        let args: Vec<Json> = args.iter().map(encode).collect();
        let r = self.request(
            json!({"op": "call", "handle": f.0, "args": args}),
            "a data function",
        );
        self.decoded(r)
    }

    fn vars_used(&mut self, f: FnHandle, data: &Value) -> JsResult<Vec<String>> {
        let r = self.request(
            json!({"op": "varsUsed", "handle": f.0, "data": encode(data)}),
            "a computed data function",
        );
        match r {
            JsResult::Ok(Json::Array(a)) if a.iter().all(Json::is_string) => JsResult::Ok(
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect(),
            ),
            JsResult::Ok(other) => JsResult::Failed(self.fail(&format!("varsUsed gave {other}"))),
            JsResult::Thrown(e) => JsResult::Thrown(e),
            JsResult::Failed(f) => JsResult::Failed(f),
        }
    }

    fn reads(&mut self) -> JsResult<Vec<PathBuf>> {
        match self.request(json!({"op": "reads"}), "the list of files read") {
            JsResult::Ok(Json::Array(a)) => JsResult::Ok(
                a.iter()
                    .filter_map(|s| s.as_str().map(PathBuf::from))
                    .collect(),
            ),
            JsResult::Ok(other) => JsResult::Failed(self.fail(&format!("reads gave {other}"))),
            JsResult::Thrown(e) => JsResult::Thrown(e),
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
        if let Some(t) = self.stderr_thread.take() {
            let _ = t.join();
        }
    }
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
fn read_frames(stdout: impl Read, tx: mpsc::Sender<Reply>) {
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
        let parsed =
            serde_json::from_slice(&body).map_err(|e| format!("a reply that isn't JSON: {e}"));
        if tx.send(parsed).is_err() {
            return;
        }
    }
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
    }
}
