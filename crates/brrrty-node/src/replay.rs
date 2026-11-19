//! The replay check: whether any call a build made to a function in the
//! JavaScript data gave a result that depended on the calls made before it.
//!
//! 11ty makes the calls one page after another, in an order of its own;
//! Brrrty makes them as its render threads reach them. When a function
//! keeps state between calls (a registry, a counter, a "seen" set), the two
//! orders give different pages, and Brrrty's order isn't even the same from
//! one build to the next. So after the pages render, every request made
//! since the data files loaded is run again in two fresh workers: the
//! renders (each a unit whose own calls keep their order) in the reverse of
//! the order they first called in, and in the order of their names (page
//! and kind), which is the same every build. Every reply must be the same
//! as the first time. A cache that only remembers what it would compute
//! anyway gives the same replies in any order and passes; a function whose
//! results follow the order of calls gives a different reply in one of the
//! orders: whichever order puts a write before a read the first run didn't,
//! or the reverse.
//!
//! The data files load again first, in their own order, handed the clock
//! and random values they read the first time, so state they set up from
//! the time is the same.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use brrrty_js::runtime::RuntimeFailure;
use serde_json::{Value as Json, json};

use crate::client::{LoadRecord, Record, RunMode, Worker};
use crate::find::NodeInfo;

/// What a build recorded.
pub(crate) struct Log<'a> {
    pub node: &'a NodeInfo,
    pub root: &'a Path,
    pub run_mode: RunMode,
    pub loads: &'a [LoadRecord],
    /// How many of `loads` loaded global data.
    pub global_end: usize,
    pub load_handles: u64,
    pub calls: &'a [Record],
    /// Every value the first run kept in its worker, as sent, each with
    /// the kept values inside it as references, in the order sent.
    pub kept_defs: &'a [Box<str>],
    /// The longest any request took the first time.
    pub slowest: std::time::Duration,
}

pub(crate) enum Failure {
    /// A reply differed: why, for the build's problem, and the first run's
    /// handle of the function called, when it was a call, to name it.
    Differs(String, Option<u64>),
    /// A replay's Node couldn't run.
    Runtime(RuntimeFailure),
}

/// One render's requests, in order.
struct Unit {
    label: Arc<str>,
    records: Vec<usize>,
    /// Handles of functions made by calls that its requests use before its
    /// own replies made them: another unit must have made them.
    needs: HashSet<u64>,
}

impl Log<'_> {
    pub fn check(&self) -> Result<(), Failure> {
        // Template and directory data files and templates load in an order
        // of 11ty's own: with any, the reverse order is checked even when
        // no page calls a function.
        if self.calls.is_empty() && self.loads.len() <= self.global_end {
            return Ok(());
        }
        // The functions made by calls that each request uses, shared by
        // both replays.
        let uses: Vec<Vec<u64>> = self
            .calls
            .iter()
            .map(|r| handles_used(&r.req_text, self.load_handles))
            .collect();
        let units = self.units(&uses);
        let reverse: Vec<usize> = (0..units.len()).rev().collect();
        let mut named: Vec<usize> = (0..units.len()).collect();
        named.sort_by(|a, b| units[*a].label.cmp(&units[*b].label).then(a.cmp(b)));
        // Requests are parsed again here, and data may nest deep: a roomy
        // stack, as for replies.
        let thread = || std::thread::Builder::new().stack_size(crate::client::REPLY_STACK);
        let (a, b) = std::thread::scope(|s| {
            let a = thread().spawn_scoped(s, || self.replay(&units, &reverse, true, &uses));
            // Both load in another order, so both run even with no calls.
            let b = thread().spawn_scoped(s, || self.replay(&units, &named, false, &uses));
            let join = |h: std::io::Result<
                std::thread::ScopedJoinHandle<'_, Result<(), Failure>>,
            >| match h {
                Ok(h) => h.join(),
                Err(e) => Ok(Err(Self::runtime(format!(
                    "the replay check couldn't start: {e}"
                )))),
            };
            (join(a), join(b))
        });
        let joined = |r: std::thread::Result<Result<(), Failure>>| {
            r.unwrap_or_else(|_| {
                Err(Failure::Runtime(RuntimeFailure {
                    message: "the replay check stopped".into(),
                    stderr_tail: Vec::new(),
                    stopped: false,
                }))
            })
        };
        joined(a)?;
        joined(b)
    }

    /// The renders' requests, each render with the handles of functions
    /// made by calls that it uses before its own replies made them (`uses`
    /// gives each request's).
    fn units(&self, uses: &[Vec<u64>]) -> Vec<Unit> {
        let mut index: HashMap<u64, usize> = HashMap::new();
        let mut units: Vec<Unit> = Vec::new();
        for (i, r) in self.calls.iter().enumerate() {
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
        for unit in &mut units {
            let mut made: HashSet<u64> = HashSet::new();
            for &i in &unit.records {
                let r = &self.calls[i];
                for &h in &uses[i] {
                    if !made.contains(&h) {
                        unit.needs.insert(h);
                    }
                }
                let mut out = Vec::new();
                handles_in(&r.head, &mut out, false);
                if let Some(raw) = &r.raw {
                    handles_in(raw, &mut out, false);
                }
                made.extend(out.into_iter().filter(|h| *h >= self.load_handles));
            }
        }
        units
    }

    fn runtime(why: impl Into<String>) -> Failure {
        Failure::Runtime(RuntimeFailure {
            message: why.into(),
            stderr_tail: Vec::new(),
            stopped: false,
        })
    }

    fn replay(
        &self,
        units: &[Unit],
        order: &[usize],
        interleave: bool,
        uses: &[Vec<u64>],
    ) -> Result<(), Failure> {
        let mut w = Worker::start(self.node, self.root, self.run_mode)
            .map_err(|e| Self::runtime(format!("the replay check couldn't start Node: {e}")))?;
        w.keep_raw.store(
            crate::client::KEEP_RAW_ONLY,
            std::sync::atomic::Ordering::SeqCst,
        );
        // A request may take a while again, but not without end: the
        // first run finished.
        w.deadline = Some((self.slowest * 4).max(std::time::Duration::from_secs(10)));
        let send = |w: &mut Worker, req: Json| {
            w.exchange(req, "a request of the replay check").map_err(|f| {
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
            })
        };
        // The template and template/directory data modules run in the
        // reverse of the build's order in one worker, and rotated by one
        // (the first last) in the other: a rotation moves every module's
        // place, which the reverse leaves alone for the middle one.
        self.loads_reordered(&mut w, &send, interleave)?;
        let begun = send(&mut w, json!({"op": "callsBegin"}))?;
        let handles = begun
            .raw
            .as_ref()
            .and_then(|r| r.get("handles"))
            .and_then(Json::as_u64);
        if handles != Some(self.load_handles) {
            return Err(Failure::Differs(
                "the JavaScript data gave different functions when it loaded again, so Brrrty can't check that the functions' results don't depend on the order pages call them".into(),
                None,
            ));
        }
        // The clock and random values the first run's calls read, by kind,
        // in the order it read them.
        let mut by_kind = serde_json::Map::new();
        for r in self.calls {
            if let Some(Json::Array(reads)) = r.head.get("clock") {
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
            &mut w,
            json!({"op": "feedCalls", "values": Json::Object(by_kind)}),
        )?;
        let mut st = State {
            load_handles: self.load_handles,
            functions: HashMap::new(),
            pages: HashMap::new(),
        };
        // Every value the first run kept, given before the calls: each
        // request can then go as it was sent, in any order.
        self.define_kept(&mut w)?;
        let unready = || {
            Failure::Differs(
                "a function made by one page's call is used by another in a way Brrrty can't check"
                    .into(),
                None,
            )
        };
        if interleave {
            // One request from each render in turn: renders that call
            // functions at the same time, as Brrrty's threads may, and as
            // 11ty never does.
            let mut active: Vec<(usize, usize)> = order.iter().map(|&u| (u, 0)).collect();
            while !active.is_empty() {
                let mut progressed = false;
                for slot in active.iter_mut() {
                    let (u, at) = *slot;
                    let i = units[u].records[at];
                    if !uses[i].iter().all(|h| st.functions.contains_key(h)) {
                        continue;
                    }
                    self.step(&mut w, &mut st, i, &send, &uses[i])?;
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
                    self.step(&mut w, &mut st, i, &send, &uses[i])?;
                }
            }
        }
        Ok(())
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
        &self,
        w: &mut Worker,
        send: &dyn Fn(&mut Worker, Json) -> Result<crate::client::Frame, Failure>,
        reverse: bool,
    ) -> Result<(), Failure> {
        // Each load's reply, parsed here, on this thread's roomy stack.
        let replies: Vec<Json> = self
            .loads
            .iter()
            .map(|r| crate::client::parse_reply(r.text.as_bytes()).unwrap_or(Json::Null))
            .collect();
        let feed = |i: usize| replies[i].get("loadClock").cloned().unwrap_or(json!([]));
        // A replay worker, handed no clock of its own: each load brings its own.
        send(w, json!({"op": "feedClock", "values": []}))?;
        let split = self.global_end.min(self.loads.len());
        for (i, r) in self.loads[..split].iter().enumerate() {
            let mut req = r.req.clone();
            req["feed"] = feed(i);
            send(w, req)?;
        }
        let tail = &self.loads[split..];
        let op = |r: &LoadRecord| {
            r.req
                .get("op")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string()
        };
        let mut items: Vec<(usize, Json)> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (i, r) in tail.iter().enumerate() {
            let path = r
                .req
                .get("path")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string();
            let kind = match op(r).as_str() {
                "templateLoad" => "template",
                // A data file's module runs once, on its first load.
                "load" if seen.insert(path.clone()) => "data",
                _ => continue,
            };
            items.push((
                i,
                json!({"kind": kind, "path": path, "feed": feed(split + i)}),
            ));
        }
        if reverse {
            items.reverse();
        } else {
            if !items.is_empty() {
                items.rotate_left(1);
            }
        }
        let got = send(
            w,
            json!({"op": "preload", "items": items.iter().map(|(_, j)| j.clone()).collect::<Vec<_>>()}),
        )?;
        let consumed: Vec<usize> = got
            .raw
            .as_ref()
            .and_then(Json::as_array)
            .map(|a| a.iter().map(|n| n.as_u64().unwrap_or(0) as usize).collect())
            .unwrap_or_default();
        let mut used: HashMap<usize, usize> = HashMap::new();
        for ((i, _), n) in items.iter().zip(consumed) {
            used.insert(*i, n);
        }
        let strip = |h: &Json| {
            let mut h = h.clone();
            if let Some(o) = h.as_object_mut() {
                o.remove("id");
                o.remove("loadClock");
            }
            h
        };
        for (i, r) in tail.iter().enumerate() {
            let mut req = r.req.clone();
            // What its module code read ran already.
            let rest: Vec<Json> = feed(split + i)
                .as_array()
                .map(|a| {
                    a.iter()
                        .skip(used.get(&i).copied().unwrap_or(0))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            req["feed"] = Json::Array(rest);
            let got = send(w, req)?;
            let mut want = strip(&replies[split + i]);
            let want_ok = want.as_object_mut().and_then(|o| o.remove("ok"));
            if strip(&got.head) != want || got.raw != want_ok {
                let what = r.req.get("path").and_then(Json::as_str).unwrap_or("a file");
                let shown = Path::new(what)
                    .strip_prefix(self.root)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| what.to_string());
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

    /// Send the first run's request `i` again, and compare the reply.
    fn step(
        &self,
        w: &mut Worker,
        st: &mut State,
        i: usize,
        _send: &dyn Fn(&mut Worker, Json) -> Result<crate::client::Frame, Failure>,
        uses: &[u64],
    ) -> Result<(), Failure> {
        let r = &self.calls[i];
        // A page opens under the first run's number.
        let opens = r.req_text.starts_with(r#"{"op":"pageOpen""#);
        let as_page = if opens {
            r.raw.as_ref().and_then(Json::as_u64)
        } else {
            None
        };
        let req = if uses.is_empty() {
            // As sent the first time, with the clock and random values the
            // call read then, and its page's number.
            let mut text = String::with_capacity(r.req_text.len() + 64);
            text.push('{');
            if let Some(clock) = r.head.get("clock") {
                text.push_str(r#""feed":"#);
                text.push_str(&clock.to_string());
                text.push(',');
            }
            if let Some(n) = as_page {
                text.push_str(&format!(r#""as":{n},"#));
            }
            text.push_str(&r.req_text[1..]);
            text
        } else {
            // It uses a function an earlier call made: under the replay's
            // own handle for it.
            let mut req = st.rewrite(&r.req(), true);
            if let Some(clock) = r.head.get("clock") {
                req["feed"] = clock.clone();
            }
            if let Some(n) = as_page {
                req["as"] = json!(n);
            }
            req.to_string()
        };
        // The filters a JavaScript template asked for, answered as the
        // first time.
        w.answers_feed = Some(r.answers.iter().cloned().collect());
        let got = w
            .exchange_text(&req, "a request of the replay check")
            .map_err(Self::sent_failure);
        w.answers_feed = None;
        let got = got?;
        // The clock values a call read only feed the replay: a call
        // may read the clock more or less often in another order
        // (a cache warmed or not), which matters only when its
        // result shows it.
        let mut head = got.head.clone();
        if let Some(o) = head.as_object_mut() {
            o.remove("id");
            o.remove("clock");
        }
        let mut want = r.head.clone();
        if let Some(o) = want.as_object_mut() {
            o.remove("id");
            o.remove("clock");
        }
        // A page's number is given: it isn't compared.
        let same = st.same(&want, &head)
            && match (&r.raw, &got.raw) {
                _ if opens => true,
                (Some(a), Some(b)) => st.same(a, b),
                (None, None) => true,
                _ => false,
            };
        if !same {
            let handle = r.req().get("handle").and_then(Json::as_u64);
            return Err(Failure::Differs(
                difference(r, &head, got.raw.as_ref()),
                handle,
            ));
        }
        if opens
            && let (Some(a), Some(b)) = (
                r.raw.as_ref().and_then(Json::as_u64),
                got.raw.as_ref().and_then(Json::as_u64),
            )
        {
            st.pages.insert(a, b);
        }
        if head.get("dropShared").is_some() {
            // The worker forgot the values it kept: give them again.
            self.define_kept(w)?;
        }
        Ok(())
    }

    /// Give the replay's worker every value the first run kept.
    fn define_kept(&self, w: &mut Worker) -> Result<(), Failure> {
        let mut text = String::from(r#"{"op":"defineKept","defs":["#);
        for (n, d) in self.kept_defs.iter().enumerate() {
            if n > 0 {
                text.push(',');
            }
            text.push_str(d);
        }
        text.push_str("]}");
        w.exchange_text(&text, "a request of the replay check")
            .map(|_| ())
            .map_err(Self::sent_failure)
    }

    /// A request of the replay check that failed: a replay that ran past
    /// its deadline means the data behaves differently when it runs again.
    fn sent_failure(f: RuntimeFailure) -> Failure {
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
}

/// The message for a reply that differs on the replay.
fn difference(r: &Record, head: &Json, raw: Option<&Json>) -> String {
    let shown = |head: &Json, raw: Option<&Json>| {
        let text = match (head.get("refused"), head.get("thrown"), raw) {
            (Some(why), _, _) => format!("refused: {}", why.as_str().unwrap_or("")),
            (_, Some(t), _) => format!(
                "threw {}",
                t.get("message").and_then(Json::as_str).unwrap_or("")
            ),
            (_, _, Some(v)) => v.to_string(),
            _ => head.to_string(),
        };
        let mut t: String = text.chars().take(80).collect();
        if text.chars().count() > 80 {
            t.push('…');
        }
        t
    };
    let op = r.req().get("op").and_then(Json::as_str).map(str::to_string);
    let what = match op.as_deref() {
        Some("call") => "{fn}, which a template calls,",
        Some("pageCall" | "pageVars") => "{fn}, a computed or permalink function,",
        Some("render") => "a JavaScript template's render",
        _ => "a function from JavaScript data",
    };
    let place = match where_(&r.label) {
        Some(w) => format!(" ({w})"),
        None => String::new(),
    };
    format!(
        "{what}{place} gives a different result when the pages' calls, or the templates' and data files' module code, run in another order ({} the first time, {} the second): it shows state its module or the data share, which follows the order 11ty loads files and builds pages in, and that differs between 11ty and Brrrty",
        shown(&r.head, r.raw.as_ref()),
        shown(head, raw)
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
        "5" => format!("in the content of {}", rest.strip_prefix("content ")?),
        "6" => format!("in the layouts of {}", rest.strip_prefix("layout ")?),
        _ => return None,
    })
}

struct State {
    load_handles: u64,
    /// First run's handles of functions calls made, to the replay's.
    functions: HashMap<u64, u64>,
    pages: HashMap<u64, u64>,
}

/// A wire form with one key: `{"$f": 3}` gives `("$f", 3)`.
fn tagged(j: &Json) -> Option<(&str, &Json)> {
    let o = j.as_object()?;
    if o.len() != 1 {
        return None;
    }
    o.iter().next().map(|(k, v)| (k.as_str(), v))
}

/// The handles of functions made by calls (`load_handles` and above) that
/// a request uses, read from its text: `"$f":` and `"handle":` can only be
/// keys there (a quote inside a string is escaped), and only these are.
fn handles_used(text: &str, load_handles: u64) -> Vec<u64> {
    let mut out = Vec::new();
    for key in [r#""$f":"#, r#""handle":"#] {
        let mut rest = text;
        while let Some(at) = rest.find(key) {
            rest = &rest[at + key.len()..];
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if let Ok(h) = rest[..digits].parse::<u64>()
                && h >= load_handles
            {
                out.push(h);
            }
        }
    }
    out
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

impl State {
    fn function(&self, h: u64) -> u64 {
        if h < self.load_handles {
            h
        } else {
            self.functions.get(&h).copied().unwrap_or(h)
        }
    }

    /// The first run's request as the replay's worker must see it: its
    /// functions renamed.
    fn rewrite(&mut self, j: &Json, top: bool) -> Json {
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
                        (true, "page", Some(p)) => json!(self.pages.get(&p).copied().unwrap_or(p)),
                        _ => self.rewrite(v, false),
                    };
                    out.insert(k.clone(), v);
                }
                Json::Object(out)
            }
            other => other.clone(),
        }
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
                    if m < self.load_handles {
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
