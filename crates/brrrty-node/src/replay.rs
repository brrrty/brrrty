//! The replay check: whether any call a build made to a function in the
//! JavaScript data gave a result that depended on the calls made before it.
//!
//! 11ty makes the calls one page after another, in an order of its own;
//! Brrrty makes them as its render threads reach them. When a function
//! keeps state between calls (a registry, a counter, a "seen" set), the two
//! orders give different pages, and Brrrty's order isn't even the same from
//! one build to the next. So every request made since the data files
//! loaded is run again in two more workers: the renders (each a unit whose
//! own calls keep their order) in the reverse of the order they first
//! called in, and in the order of their names (page and kind), which is the
//! same every build. Every reply must be the same as the first time. A
//! cache that only remembers what it would compute anyway gives the same
//! replies in any order and passes; a function whose results follow the
//! order of calls gives a different reply in one of the orders: whichever
//! order puts a write before a read the first run didn't, or the reverse.
//!
//! The data files load again first, in their own order, handed the clock
//! and random values they read the first time, so state they set up from
//! the time is the same.
//!
//! The two workers start as soon as the first run has something for them
//! to check, and load the data while it goes on. They are sent each value
//! the first run defines as it defines it (see [`crate::wire`]), so when
//! the first run ends they hold what it held, and a request is replayed by
//! sending its text again: it refers to the same values by the same ids.
//! Requests go ahead of their replies, so a replay takes the time the
//! site's own code takes, not a round trip per call.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brrrty_js::runtime::RuntimeFailure;
use serde_json::{Value as Json, json};

use crate::client::{Frame, LoadKind, LoadRecord, Record, RunMode, Worker, parse_reply};
use crate::find::NodeInfo;
use crate::wire::DynamicDef;

/// How the first run loaded the data.
pub(crate) struct Loads {
    pub node: NodeInfo,
    pub root: PathBuf,
    pub run_mode: RunMode,
    pub loads: Vec<LoadRecord>,
    /// How many of `loads` loaded global data.
    pub global_end: usize,
    pub load_handles: u64,
    /// Why the build's own worker walks, when Brrrty asked it to: the
    /// replays' workers do the same.
    pub walk: Option<String>,
    /// How long the first run's loads took, all of them.
    pub took: Duration,
}

/// The requests the first run made once the data had loaded.
pub(crate) struct Calls {
    pub records: Vec<Record>,
    pub load_handles: u64,
    /// The longest any request took the first time.
    pub slowest: Duration,
    /// Definitions holding functions calls made, by root.
    pub dynamic: HashMap<u32, DynamicDef>,
    /// Every definition, in order, and whether it is dynamic.
    pub lines: Vec<(Arc<str>, bool)>,
    /// A call changed a value the worker kept.
    pub dropped: bool,
}

pub(crate) enum Failure {
    /// A reply differed: why, for the build's problem, and the first run's
    /// handle of the function called, when it was a call, to name it.
    Differs(String, Option<u64>),
    /// A replay's Node couldn't run.
    Runtime(RuntimeFailure),
    /// A replay's worker asks for the build to be run again, walking (see
    /// the worker's "Freezing"): why.
    Again(String),
}

/// The reply, unless the worker asks for the build again.
fn heard(frame: Frame) -> Result<Frame, Failure> {
    match frame.head.get("again") {
        Some(why) => Err(Failure::Again(why.as_str().unwrap_or("").to_string())),
        None => Ok(frame),
    }
}

enum Msg {
    /// A value the first run defined.
    Line(Arc<str>),
    /// The first run is over: replay its calls.
    Run(Arc<Calls>),
}

/// One of the replay check's workers, on a thread of its own.
pub(crate) struct Handle {
    tx: Sender<Msg>,
    result: Receiver<Result<(), Failure>>,
    /// Its Node's process id while it runs, so a build that ends without
    /// the check stops it at once.
    live: Arc<Mutex<Option<u32>>>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pid) = *live {
            crate::client::stop_pid(pid);
        }
    }
}

/// Which order a replay runs the build's renders in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Order {
    /// One request from each render in turn, the renders in the reverse
    /// of the order the build first called from them.
    Reversed,
    /// Each render whole, in the order 11ty reaches them (see
    /// [`like_11ty`]).
    Like11ty,
    /// Each render whole, in the order of their names.
    Named,
}

impl Handle {
    /// Start a worker that loads the data again with the template and
    /// directory data modules in the reverse of the build's order (for
    /// [`Order::Reversed`]) or rotated by one, then waits for the calls
    /// and runs them in `order`.
    pub fn start(loads: Arc<Loads>, order: Order) -> Handle {
        let (tx, rx) = mpsc::channel();
        let (done, result) = mpsc::channel();
        let live = Arc::new(Mutex::new(None));
        let cancelled = Arc::new(AtomicBool::new(false));
        // Requests are parsed again here, and data may nest deep: a roomy
        // stack, as for replies.
        let spawned = std::thread::Builder::new()
            .name("brrrty-node-replay".into())
            .stack_size(crate::client::REPLY_STACK)
            .spawn({
                let done = done.clone();
                let (live, cancelled) = (live.clone(), cancelled.clone());
                move || {
                    let _ = done.send(work(&loads, order, rx, &live, &cancelled));
                }
            });
        if let Err(e) = spawned {
            let _ = done.send(Err(runtime(format!(
                "the replay check couldn't start: {e}"
            ))));
        }
        Handle {
            tx,
            result,
            live,
            cancelled,
        }
    }

    pub fn line(&self, line: Arc<str>) {
        let _ = self.tx.send(Msg::Line(line));
    }

    pub fn run(&self, calls: Arc<Calls>) {
        let _ = self.tx.send(Msg::Run(calls));
    }

    pub fn finish(self) -> Result<(), Failure> {
        self.result.recv().unwrap_or_else(|_| {
            Err(Failure::Runtime(RuntimeFailure {
                message: "the replay check stopped".into(),
                stderr_tail: Vec::new(),
                stopped: false,
                again: false,
            }))
        })
    }
}

fn runtime(why: impl Into<String>) -> Failure {
    Failure::Runtime(RuntimeFailure {
        message: why.into(),
        stderr_tail: Vec::new(),
        stopped: false,
        again: false,
    })
}

/// A request of the replay check, and what a failure of it means.
fn send(w: &mut Worker, text: &str, feed: Option<&str>) -> Result<Frame, Failure> {
    w.exchange(text, feed, "a request of the replay check")
        .map_err(failed)
        .and_then(heard)
}

fn failed(f: RuntimeFailure) -> Failure {
    if f.message.contains("longer than the first run allows") {
        Failure::Differs(
            format!(
                "the replay check ran the JavaScript data again, and {}: the data or a function behaves differently when it runs again, so Brrrty can't check the pages' calls",
                f.message
            ),
            None,
        )
    } else {
        Failure::Runtime(f)
    }
}

/// A request may take a while again, but not without end: the first run
/// finished.
fn deadline(slowest: Duration) -> Duration {
    (slowest * 4).max(floor())
}

/// The least a replay's request is allowed: 10 s, or `BRRRTY_JS_REPLAY_FLOOR_MS`
/// (for the tests, which can't wait that long to see a slow load pass).
fn floor() -> Duration {
    static FLOOR: std::sync::LazyLock<Duration> = std::sync::LazyLock::new(|| {
        std::env::var("BRRRTY_JS_REPLAY_FLOOR_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(10))
    });
    *FLOOR
}

fn work(
    loads: &Loads,
    order: Order,
    rx: Receiver<Msg>,
    live: &Mutex<Option<u32>>,
    cancelled: &AtomicBool,
) -> Result<(), Failure> {
    if cancelled.load(Ordering::SeqCst) {
        return Ok(());
    }
    let mut w = Worker::start_as(&loads.node, &loads.root, loads.run_mode, Some(loads))
        .map_err(|e| runtime(format!("the replay check couldn't start Node: {e}")))?;
    *live.lock().unwrap_or_else(|e| e.into_inner()) = Some(w.pid());
    let out = work_with(&mut w, loads, order, rx, cancelled);
    // Its process id is no longer this worker's once it is dropped.
    *live.lock().unwrap_or_else(|e| e.into_inner()) = None;
    // Nothing of it is needed any more: no waiting for Node to wind down.
    w.stop();
    out
}

fn work_with(
    w: &mut Worker,
    loads: &Loads,
    order: Order,
    rx: Receiver<Msg>,
    cancelled: &AtomicBool,
) -> Result<(), Failure> {
    if cancelled.load(Ordering::SeqCst) {
        return Ok(());
    }
    // Replies are compared as text.
    w.decode.store(false, std::sync::atomic::Ordering::Relaxed);
    // A replay's loads run in another order, several modules in one
    // request, so each request is allowed what all of the first run's
    // loads took together, four times over.
    w.deadline = Some(deadline(loads.took));
    // The template and template/directory data modules run in the
    // reverse of the build's order in one worker, and rotated by one
    // (the first last) in the other: a rotation moves every module's
    // place, which the reverse leaves alone for the middle one.
    let mut early = loads_reordered(w, loads, order == Order::Reversed, cancelled).and_then(|()| {
        let begun = send(w, r#"{"op":"callsBegin"}"#, None)?;
        if begun.head.get("handles").and_then(Json::as_u64) != Some(loads.load_handles) {
            return Err(Failure::Differs(
                "the JavaScript data gave different functions when it loaded again, so Brrrty can't check that the functions' results don't depend on the order pages call them".into(),
                None,
            ));
        }
        Ok(())
    });
    loop {
        match rx.recv() {
            Ok(Msg::Line(line)) => {
                if early.is_ok()
                    && let Err(f) = w.write_line(&line)
                {
                    early = Err(Failure::Runtime(f));
                }
            }
            Ok(Msg::Run(calls)) => {
                early?;
                return replay(w, &calls, order);
            }
            // The build ended without the check.
            Err(_) => return Ok(()),
        }
    }
}

/// Load the data again with the template and directory data files and
/// the templates evaluated in another order (`reverse`, or the first
/// run's rotated by one), as 11ty evaluates each when its file finishes
/// loading, in an order of its own; then their loads in the first run's
/// order, each compared with the first run's reply. Each load gets the
/// clock and random values it read the first time.
///
/// Between the two orders every module's place changes (the rotation
/// moves each one), so a page that shows its own module's place (a
/// shared counter it read at load) differs in one of them. A
/// dependence that comes out the same in the build's order, its reverse
/// and its rotation is not seen.
fn loads_reordered(
    w: &mut Worker,
    log: &Loads,
    reverse: bool,
    cancelled: &AtomicBool,
) -> Result<(), Failure> {
    let feed = |r: &LoadRecord| r.clock.as_deref().unwrap_or("[]").to_string();
    // The build ended without the check (it failed, or was sent back):
    // nothing more is loaded for it.
    let send = |w: &mut Worker, text: &str, feed: Option<&str>| {
        if cancelled.load(Ordering::SeqCst) {
            return Err(runtime("the replay check was stopped".to_string()));
        }
        send(w, text, feed)
    };
    // A replay worker, handed no clock of its own: each load brings its own.
    send(w, r#"{"op":"feedClock","values":[]}"#, None)?;
    let split = log.global_end.min(log.loads.len());
    for r in &log.loads[..split] {
        send(w, &r.text, Some(&feed(r)))?;
    }
    let tail = &log.loads[split..];
    let mut items: Vec<(usize, Json)> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for (i, r) in tail.iter().enumerate() {
        let kind = match r.kind {
            LoadKind::Template => "template",
            // A data file's module runs once, on its first load.
            LoadKind::Data if seen.insert(r.path.as_str()) => "data",
            _ => continue,
        };
        let clock: Json = parse_reply(feed(r).as_bytes()).unwrap_or(json!([]));
        items.push((i, json!({"kind": kind, "path": r.path, "feed": clock})));
    }
    if reverse {
        items.reverse();
    } else if !items.is_empty() {
        items.rotate_left(1);
    }
    let got = send(
        w,
        &json!({"op": "preload", "items": items.iter().map(|(_, j)| j.clone()).collect::<Vec<_>>()})
            .to_string(),
        None,
    )?;
    let consumed: Vec<usize> = got
        .head
        .get("consumed")
        .and_then(Json::as_array)
        .map(|a| a.iter().map(|n| n.as_u64().unwrap_or(0) as usize).collect())
        .unwrap_or_default();
    let mut used: HashMap<usize, usize> = HashMap::new();
    for ((i, _), n) in items.iter().zip(consumed) {
        used.insert(*i, n);
    }
    for (i, r) in tail.iter().enumerate() {
        // What its module code read ran already.
        let skip = used.get(&i).copied().unwrap_or(0);
        let rest = if skip == 0 {
            feed(r)
        } else {
            let all: Json = parse_reply(feed(r).as_bytes()).unwrap_or(json!([]));
            Json::Array(
                all.as_array()
                    .map(|a| a.iter().skip(skip).cloned().collect())
                    .unwrap_or_default(),
            )
            .to_string()
        };
        let got = send(w, &r.text, Some(&rest))?;
        if got.body != r.body {
            let shown = Path::new(&r.path)
                .strip_prefix(&log.root)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| {
                    if r.path.is_empty() {
                        "a file".to_string()
                    } else {
                        r.path.clone()
                    }
                });
            return Err(Failure::Differs(
                format!(
                    "{shown} gives different data when the templates and template and directory data files load in another order: 11ty runs each one's module code as its file finishes loading, in an order of its own, so module state they share (a counter, a registry) follows that order, which Brrrty can't know"
                ),
                None,
            ));
        }
    }
    Ok(())
}

/// One render's requests, in order.
struct Unit {
    label: Arc<str>,
    records: Vec<usize>,
    /// Handles of functions made by calls that its requests use before its
    /// own replies made them: another unit must have made them.
    needs: HashSet<u64>,
}

/// Every function handle in `j`: `{"$f": n}` forms, and with `fields`, a
/// request's `handle`.
fn handles_in(j: &Json, out: &mut Vec<u64>, fields: bool) {
    match j {
        Json::Array(a) => a.iter().for_each(|x| handles_in(x, out, false)),
        Json::Object(o) => {
            if let Some(("$f", Json::Number(n))) = tagged(j) {
                out.extend(n.as_u64());
                return;
            }
            for (k, v) in o {
                if fields && k == "handle" {
                    out.extend(v.as_u64());
                } else {
                    handles_in(v, out, false);
                }
            }
        }
        _ => {}
    }
}

/// A wire form with one key: `{"$f": 3}` gives `("$f", 3)`.
fn tagged(j: &Json) -> Option<(&str, &Json)> {
    let o = j.as_object()?;
    if o.len() != 1 {
        return None;
    }
    o.iter().next().map(|(k, v)| (k.as_str(), v))
}

fn parse(text: &str) -> Json {
    parse_reply(text.as_bytes()).unwrap_or(Json::Null)
}

impl Calls {
    /// Whether no request or reply names a function a call made, and no
    /// call changed a kept value: every request can be sent again as it
    /// is, ahead of the replies.
    fn plain(&self) -> bool {
        !self.dropped
            && self.dynamic.is_empty()
            && self
                .records
                .iter()
                .all(|r| !r.made && !r.names_made && r.dynamic.is_empty())
    }

    /// The functions calls made that request `i` names: in itself, and in
    /// the dynamic definitions it needs.
    fn uses(&self, i: usize) -> Vec<u64> {
        let r = &self.records[i];
        let mut used = Vec::new();
        if r.names_made {
            handles_in(&parse(&r.text), &mut used, true);
        }
        for root in &r.dynamic {
            if let Some(d) = self.dynamic.get(root) {
                handles_in(&parse(&d.line), &mut used, false);
            }
        }
        used.retain(|h| *h >= self.load_handles);
        used
    }

    fn units(&self, with_needs: bool) -> Vec<Unit> {
        let mut index: HashMap<u64, usize> = HashMap::new();
        let mut units: Vec<Unit> = Vec::new();
        for (i, r) in self.records.iter().enumerate() {
            let u = *index.entry(r.unit).or_insert_with(|| {
                units.push(Unit {
                    label: r.label.clone(),
                    records: Vec::new(),
                    needs: HashSet::new(),
                });
                units.len() - 1
            });
            units[u].records.push(i);
        }
        if !with_needs {
            return units;
        }
        for unit in &mut units {
            let mut made: HashSet<u64> = HashSet::new();
            for &i in &unit.records {
                for h in self.uses(i) {
                    if !made.contains(&h) {
                        unit.needs.insert(h);
                    }
                }
                let r = &self.records[i];
                if r.made {
                    let mut out = Vec::new();
                    handles_in(&parse(&r.body), &mut out, false);
                    made.extend(out.into_iter().filter(|h| *h >= self.load_handles));
                }
            }
        }
        units
    }
}

/// The second word of the name of a content render whose page has a
/// layout ("5 laid-out src/a.njk"; "5 content src/b.njk" without one).
const LAID_OUT: &str = "laid-out";

/// The order of the renders' names as text, but for the word that says
/// whether a page has a layout: computed data and permalinks before any
/// content, content before any layout, each by file.
fn by_name(a: &str, b: &str) -> std::cmp::Ordering {
    fn plain(label: &str) -> (&str, &str) {
        let (stage, rest) = label.split_once(' ').unwrap_or((label, ""));
        (stage, rest.split_once(' ').map_or(rest, |(_, r)| r))
    }
    plain(a).cmp(&plain(b)).then_with(|| a.cmp(b))
}

/// The order 11ty runs the build's renders in, as near as their names
/// say it: computed data and permalinks before any content, content
/// before any layout, and the content in the order 11ty 3.1.8 reaches
/// each page's first call:
///
/// 1. pages without a layout, by template language: Liquid, Nunjucks,
///    HTML, Markdown;
/// 2. those languages' pages with a layout (11ty reads the layout file
///    first, and they fall behind);
/// 3. JavaScript templates without a layout, then with one.
///
/// Each group is in input order (a folder's files before its folders, by
/// name; a template's pages by number).
///
/// 11ty's true order has more to it (where exactly a page with a layout
/// lands follows when the layout file was read, a Markdown page without
/// a layout can come after a Liquid one with, and a page that reads
/// another's content renders twice), so this is the nearest fixed order,
/// not a promise of 11ty's.
fn like_11ty(a: &str, b: &str) -> std::cmp::Ordering {
    /// (stage, class, file, page number) of a render's name.
    fn key(label: &str) -> (&str, u8, &str, u64) {
        let (stage, rest) = label.split_once(' ').unwrap_or((label, ""));
        let (kind, rest) = rest.split_once(' ').unwrap_or(("", rest));
        let laid_out = kind == LAID_OUT;
        let (file, n) = match rest.rsplit_once(" (pagination page ") {
            Some((file, n)) => (file, n.trim_end_matches(')').parse().ok()),
            None => match rest.rsplit_once(' ') {
                Some((file, n)) => (file, n.parse().ok()),
                None => (rest, None),
            },
        };
        let (file, n) = match n {
            Some(n) => (file, n),
            None => (rest, 0),
        };
        let script = !(file.ends_with(".liquid")
            || file.ends_with(".njk")
            || file.ends_with(".html")
            || file.ends_with(".md"));
        let class = if stage != "5" {
            0
        } else if script {
            // After every other page, its own pages with a layout last.
            10 + u8::from(laid_out)
        } else if laid_out {
            5
        } else if file.ends_with(".liquid") {
            1
        } else if file.ends_with(".njk") {
            2
        } else if file.ends_with(".html") {
            3
        } else {
            4
        };
        (stage, class, file, n)
    }
    /// A folder's files before its folders, each by name.
    fn files_first(a: &str, b: &str) -> std::cmp::Ordering {
        let (mut a, mut b) = (a.split('/').peekable(), b.split('/').peekable());
        loop {
            match (a.next(), b.next()) {
                (Some(x), Some(y)) => {
                    let order = match (a.peek().is_some(), b.peek().is_some()) {
                        (false, true) => std::cmp::Ordering::Less,
                        (true, false) => std::cmp::Ordering::Greater,
                        _ => x.cmp(y),
                    };
                    if order.is_ne() {
                        return order;
                    }
                }
                (None, None) => return std::cmp::Ordering::Equal,
                (None, Some(_)) => return std::cmp::Ordering::Less,
                (Some(_), None) => return std::cmp::Ordering::Greater,
            }
        }
    }
    let (ka, kb) = (key(a), key(b));
    ka.0.cmp(kb.0)
        .then(ka.1.cmp(&kb.1))
        .then_with(|| files_first(ka.2, kb.2))
        .then(ka.3.cmp(&kb.3))
        .then_with(|| a.cmp(b))
}

fn replay(w: &mut Worker, calls: &Calls, how: Order) -> Result<(), Failure> {
    let interleave = how == Order::Reversed;
    w.deadline = Some(deadline(calls.slowest));
    // The clock and random values the first run's calls read, by kind,
    // in the order it read them.
    let mut by_kind = serde_json::Map::new();
    for r in &calls.records {
        let Some(clock) = &r.clock else { continue };
        if let Json::Array(reads) = parse(clock) {
            for read in reads {
                if let Some([Json::String(kind), v]) = read.as_array().map(Vec::as_slice)
                    && let Json::Array(list) = by_kind
                        .entry(kind.clone())
                        .or_insert_with(|| Json::Array(Vec::new()))
                {
                    list.push(v.clone());
                }
            }
        }
    }
    send(
        w,
        &json!({"op": "feedCalls", "values": Json::Object(by_kind)}).to_string(),
        None,
    )?;
    let plain = calls.plain();
    let units = calls.units(!plain);
    let sorted = |by: fn(&str, &str) -> std::cmp::Ordering| {
        let mut sorted: Vec<usize> = (0..units.len()).collect();
        sorted.sort_by(|a, b| by(&units[*a].label, &units[*b].label).then(a.cmp(b)));
        sorted
    };
    let order: Vec<usize> = match how {
        Order::Reversed => (0..units.len()).rev().collect(),
        Order::Like11ty => sorted(like_11ty),
        Order::Named => sorted(by_name),
    };
    if plain {
        let mut sequence = Vec::with_capacity(calls.records.len());
        if interleave {
            // One request from each render in turn: renders that call
            // functions at the same time, as Brrrty's threads may, and as
            // 11ty never does.
            let mut active: Vec<(usize, usize)> = order.iter().map(|&u| (u, 0)).collect();
            while !active.is_empty() {
                for slot in active.iter_mut() {
                    sequence.push(units[slot.0].records[slot.1]);
                    slot.1 += 1;
                }
                active.retain(|(u, at)| *at < units[*u].records.len());
            }
        } else {
            for &u in &order {
                sequence.extend(units[u].records.iter().copied());
            }
        }
        return ahead(w, calls, &sequence);
    }
    stepped(w, calls, &units, &order, interleave)
}

/// How many requests may be on their way before their replies are read.
const AHEAD: usize = 512;

/// Send the first run's requests again in the order of `sequence`, ahead
/// of their replies, and compare each reply as it comes.
fn ahead(w: &mut Worker, calls: &Calls, sequence: &[usize]) -> Result<(), Failure> {
    let Some(mut stdin) = w.take_stdin() else {
        return Err(runtime("the replay check's Node stopped reading requests"));
    };
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::scope(|s| {
        // The requests go from a thread of their own: Node may stop
        // reading them while it waits for an answer to a question of its
        // own, and this thread must be free to answer.
        s.spawn(move || {
            for line in rx {
                if stdin
                    .write_all(line.as_bytes())
                    .and_then(|()| stdin.write_all(b"\n"))
                    .is_err()
                {
                    break;
                }
            }
            // Dropping stdin here tells Node it is done.
        });
        let mut ids: VecDeque<u64> = VecDeque::new();
        let mut sent = 0;
        let out = 'all: {
            for (done, &i) in sequence.iter().enumerate() {
                while sent < sequence.len() && sent - done < AHEAD {
                    let r = &calls.records[sequence[sent]];
                    let (id, line) = w.line_for(&r.text, r.clock.as_deref());
                    if !r.one_way {
                        ids.push_back(id);
                    }
                    if tx.send(line).is_err() {
                        break 'all Err(runtime(
                            "the replay check's Node stopped reading requests",
                        ));
                    }
                    sent += 1;
                }
                let r = &calls.records[i];
                if r.one_way {
                    continue;
                }
                // The filters a JavaScript template asked for, answered as
                // the first time.
                w.answers_feed = Some(r.answers.iter().cloned().collect());
                let mut wait = w.wait();
                let got = loop {
                    match w.next_frame("a request of the replay check", &mut wait) {
                        Ok(f) if f.question => {
                            if let Err(why) =
                                w.answer(&f, &mut |_: &str, _: &[brrrty_js::Value]| {
                                    Err("asks for a filter in the replay check".to_string())
                                })
                            {
                                break 'all Err(runtime(why));
                            }
                        }
                        Ok(f) => break f,
                        Err(f) => break 'all Err(failed(f)),
                    }
                };
                if ids.pop_front() != Some(got.id) {
                    break 'all Err(runtime("the replay check got a reply out of order"));
                }
                let got = match heard(got) {
                    Ok(got) => got,
                    Err(e) => break 'all Err(e),
                };
                if got.body != r.body {
                    break 'all Err(Failure::Differs(
                        difference(r, &got.body),
                        r.handle.map(u64::from),
                    ));
                }
            }
            Ok(())
        };
        w.answers_feed = None;
        drop(tx);
        if out.is_err() {
            // The writer may be blocked on a Node that no longer reads.
            w.stop();
        }
        out
    })
}

/// The first run's handles of functions calls made, to the replay's, and
/// what the replay's worker has been sent.
struct State<'c> {
    calls: &'c Calls,
    functions: HashMap<u64, u64>,
    /// The dynamic definitions the worker has.
    defined: HashSet<u32>,
}

/// Replay one request at a time: for a build where requests name functions
/// that calls made (each worker numbers them as it makes them), or where a
/// call changed a kept value (the worker is then sent every value again).
fn stepped(
    w: &mut Worker,
    calls: &Calls,
    units: &[Unit],
    order: &[usize],
    interleave: bool,
) -> Result<(), Failure> {
    let mut st = State {
        calls,
        functions: HashMap::new(),
        defined: HashSet::new(),
    };
    let unready = || {
        Failure::Differs(
            "a function made by one page's call is used by another in a way Brrrty can't check"
                .into(),
            None,
        )
    };
    if interleave {
        let mut active: Vec<(usize, usize)> = order.iter().map(|&u| (u, 0)).collect();
        while !active.is_empty() {
            let mut progressed = false;
            for slot in active.iter_mut() {
                let (u, at) = *slot;
                let i = units[u].records[at];
                if !calls.uses(i).iter().all(|h| st.functions.contains_key(h)) {
                    continue;
                }
                st.step(w, i)?;
                slot.1 += 1;
                progressed = true;
            }
            active.retain(|(u, at)| *at < units[*u].records.len());
            if !progressed && !active.is_empty() {
                return Err(unready());
            }
        }
    } else {
        let mut pending: Vec<usize> = order.to_vec();
        while !pending.is_empty() {
            let Some(pos) = pending
                .iter()
                .position(|&u| units[u].needs.iter().all(|h| st.functions.contains_key(h)))
            else {
                return Err(unready());
            };
            let u = pending.remove(pos);
            for &i in &units[u].records {
                st.step(w, i)?;
            }
        }
    }
    Ok(())
}

impl State<'_> {
    fn function(&self, h: u64) -> u64 {
        if h < self.calls.load_handles {
            h
        } else {
            self.functions.get(&h).copied().unwrap_or(h)
        }
    }

    /// The first run's text with its functions renamed to the replay's.
    fn rewrite(&self, j: &Json, top: bool) -> Json {
        match j {
            Json::Array(a) => Json::Array(a.iter().map(|x| self.rewrite(x, false)).collect()),
            Json::Object(o) => {
                if let Some(("$f", Json::Number(n))) = tagged(j)
                    && let Some(h) = n.as_u64()
                {
                    return json!({"$f": self.function(h)});
                }
                let mut out = serde_json::Map::with_capacity(o.len());
                for (k, v) in o {
                    let v = match (top, k.as_str(), v.as_u64()) {
                        (true, "handle", Some(h)) => json!(self.function(h)),
                        _ => self.rewrite(v, false),
                    };
                    out.insert(k.clone(), v);
                }
                Json::Object(out)
            }
            other => other.clone(),
        }
    }

    /// Send the first run's request `i` again, and compare the reply.
    fn step(&mut self, w: &mut Worker, i: usize) -> Result<(), Failure> {
        let r = &self.calls.records[i];
        for root in &r.dynamic {
            if self.defined.contains(root) {
                continue;
            }
            if let Some(d) = self.calls.dynamic.get(root) {
                let line = self.rewrite(&parse(&d.line), false).to_string();
                w.write_line(&line).map_err(Failure::Runtime)?;
                self.defined.insert(*root);
            }
        }
        let rewritten;
        let text: &str = if r.names_made {
            rewritten = self.rewrite(&parse(&r.text), true).to_string();
            &rewritten
        } else {
            &r.text
        };
        if r.one_way {
            let (_, line) = w.line_for(text, None);
            return w.write_line(&line).map_err(Failure::Runtime);
        }
        w.answers_feed = Some(r.answers.iter().cloned().collect());
        let got = send(w, text, r.clock.as_deref());
        w.answers_feed = None;
        let got = got?;
        // A function made by a call is matched up with the first run's as
        // it first appears.
        let same = if r.unchecked {
            // It ran on what a call sent with it had changed: the first
            // run made nothing of its reply.
            true
        } else if r.made || got.made {
            self.same(&parse(&r.body), &parse(&got.body))
        } else {
            got.body == r.body
        };
        if !same {
            return Err(Failure::Differs(
                difference(r, &got.body),
                r.handle.map(u64::from),
            ));
        }
        if got.head.get("dropShared").is_some() {
            // The worker forgot its values: every one again, made afresh.
            send(w, r#"{"op":"resetDefs"}"#, None)?;
            for (line, dynamic) in &self.calls.lines {
                if !dynamic {
                    w.write_line(line).map_err(Failure::Runtime)?;
                }
            }
            self.defined.clear();
        }
        Ok(())
    }

    /// Whether a replay's reply is the first run's, functions made by calls
    /// matched up as they first appear.
    fn same(&mut self, a: &Json, b: &Json) -> bool {
        match (a, b) {
            (Json::Array(x), Json::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(p, q)| self.same(p, q))
            }
            (Json::Object(x), Json::Object(y)) => {
                if let (Some(("$f", Json::Number(m))), Some(("$f", Json::Number(n)))) =
                    (tagged(a), tagged(b))
                {
                    let (Some(m), Some(n)) = (m.as_u64(), n.as_u64()) else {
                        return false;
                    };
                    if m < self.calls.load_handles {
                        return m == n;
                    }
                    return *self.functions.entry(m).or_insert(n) == n;
                }
                x.len() == y.len()
                    && x.iter()
                        .all(|(k, v)| y.get(k).is_some_and(|w| self.same(v, w)))
            }
            (x, y) => x == y,
        }
    }
}

/// The message for a reply that differs on the replay.
fn difference(r: &Record, got: &str) -> String {
    let shown = |body: &str| {
        let j = parse(body);
        let text = match (j.get("refused"), j.get("thrown"), j.get("ok")) {
            (Some(why), _, _) => format!("refused: {}", why.as_str().unwrap_or("")),
            (_, Some(t), _) => format!(
                "threw {}",
                t.get("message").and_then(Json::as_str).unwrap_or("")
            ),
            (_, _, Some(v)) => v.to_string(),
            _ => j.to_string(),
        };
        let mut t: String = text.chars().take(80).collect();
        if text.chars().count() > 80 {
            t.push('…');
        }
        t
    };
    let what = match r.op {
        "call" => "{fn}, which a template calls,",
        "pageCall" | "pageVars" => "{fn}, a computed or permalink function,",
        "render" => "a JavaScript template's render",
        _ => "a function from JavaScript data",
    };
    let place = match where_(&r.label) {
        Some(w) => format!(" ({w})"),
        None => String::new(),
    };
    format!(
        "{what}{place} gives a different result when the pages' calls, or the templates' and data files' module code, run in another order ({} the first time, {} the second): it shows state its module or the data share, which follows the order 11ty loads files and builds pages in, and that differs between 11ty and Brrrty",
        shown(&r.body),
        shown(got)
    )
}

/// A render's name as a message shows it: "in the content of src/a.njk".
fn where_(label: &str) -> Option<String> {
    let (kind, rest) = label.split_once(' ')?;
    let page = |rest: &str| match rest.rsplit_once(' ') {
        Some((file, n)) if n.parse::<usize>().is_ok() => format!("page {n} of {file}"),
        _ => rest.to_string(),
    };
    Some(match kind {
        "1" => format!("computing the data of {}", rest.strip_prefix("load ")?),
        "2" => format!("placing {}", page(rest.strip_prefix("place ")?)),
        "3" => format!(
            "computing the data of {}",
            page(rest.strip_prefix("page ")?)
        ),
        "5" => format!(
            "in the content of {}",
            rest.strip_prefix("content ")
                .or_else(|| rest.strip_prefix("laid-out "))?
        ),
        "6" => format!("in the layouts of {}", rest.strip_prefix("layout ")?),
        _ => return None,
    })
}

#[cfg(test)]
mod order_tests {
    use super::{by_name, like_11ty};

    /// The order 11ty 3.1.8 made its first call from each of these in
    /// (probe sites in a `node:22` container), from names given in
    /// name order.
    #[test]
    fn renders_sort_in_the_order_11ty_reaches_them() {
        let mut names = vec![
            "6 layout src/a1.njk",
            "5 content src/a1.11ty.js",
            "5 content src/b1.md",
            "5 content src/c1.html",
            "5 content src/d1.njk",
            "5 content src/e1.liquid",
            "5 content src/sub/m1.njk",
            "5 content src/sub/m2.liquid",
            "5 content src/sub/m3.md",
            "5 content src/v1.11ty.js",
            "5 content src/w1.md",
            "5 content src/x1.html",
            "5 content src/y1.njk",
            "5 content src/z1.liquid",
            "5 laid-out src/a2.njk",
            "5 laid-out src/a3.11ty.js",
            "5 laid-out src/a4.md",
            "5 content src/p.njk (pagination page 10)",
            "5 content src/p.njk (pagination page 2)",
            "3 page src/z1.liquid 0",
            "3 page src/a1.11ty.js 10",
            "3 page src/a1.11ty.js 9",
            "2 place src/z1.liquid 0",
            "1 load src/z1.liquid",
            "",
        ];
        names.sort();
        // By name, the word for a layout doesn't count.
        let mut named = names.clone();
        named.sort_by(|a, b| by_name(a, b));
        let at = |n: &str| named.iter().position(|x| *x == n).unwrap();
        assert!(at("5 content src/a1.11ty.js") < at("5 laid-out src/a2.njk"));
        assert!(at("5 laid-out src/a4.md") < at("5 content src/b1.md"));
        assert!(at("5 content src/z1.liquid") < at("6 layout src/a1.njk"));
        names.sort_by(|a, b| like_11ty(a, b));
        assert_eq!(
            names,
            [
                "",
                "1 load src/z1.liquid",
                "2 place src/z1.liquid 0",
                "3 page src/a1.11ty.js 9",
                "3 page src/a1.11ty.js 10",
                "3 page src/z1.liquid 0",
                "5 content src/e1.liquid",
                "5 content src/z1.liquid",
                "5 content src/sub/m2.liquid",
                "5 content src/d1.njk",
                "5 content src/p.njk (pagination page 2)",
                "5 content src/p.njk (pagination page 10)",
                "5 content src/y1.njk",
                "5 content src/sub/m1.njk",
                "5 content src/c1.html",
                "5 content src/x1.html",
                "5 content src/b1.md",
                "5 content src/w1.md",
                "5 content src/sub/m3.md",
                "5 laid-out src/a2.njk",
                "5 laid-out src/a4.md",
                "5 content src/a1.11ty.js",
                "5 content src/v1.11ty.js",
                "5 laid-out src/a3.11ty.js",
                "6 layout src/a1.njk",
            ]
        );
    }
}
