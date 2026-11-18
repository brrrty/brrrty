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

use crate::client::{Record, RunMode, Worker};
use crate::find::NodeInfo;

/// What a build recorded.
pub(crate) struct Log<'a> {
    pub node: &'a NodeInfo,
    pub root: &'a Path,
    pub run_mode: RunMode,
    pub loads: &'a [Json],
    pub clock: &'a Json,
    pub load_handles: u64,
    pub calls: &'a [Record],
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
        if self.calls.is_empty() {
            return Ok(());
        }
        let units = self.units();
        let reverse: Vec<usize> = (0..units.len()).rev().collect();
        let mut named: Vec<usize> = (0..units.len()).collect();
        named.sort_by(|a, b| units[*a].label.cmp(&units[*b].label).then(a.cmp(b)));
        let (a, b) = std::thread::scope(|s| {
            let a = s.spawn(|| self.replay(&units, &reverse, true));
            let b = s.spawn(|| self.replay(&units, &named, false));
            (a.join(), b.join())
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

    fn units(&self) -> Vec<Unit> {
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
                let mut used = Vec::new();
                handles_in(&r.req, &mut used, true);
                for h in used {
                    if h >= self.load_handles && !made.contains(&h) {
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

    fn replay(&self, units: &[Unit], order: &[usize], interleave: bool) -> Result<(), Failure> {
        let mut w = Worker::start(self.node, self.root, self.run_mode)
            .map_err(|e| Self::runtime(format!("the replay check couldn't start Node: {e}")))?;
        w.keep_raw.store(true, std::sync::atomic::Ordering::SeqCst);
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
        send(&mut w, json!({"op": "feedClock", "values": self.clock}))?;
        for req in self.loads {
            send(&mut w, req.clone())?;
        }
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
        // Every value Brrrty kept in the worker, as it was first sent.
        let mut kept: HashMap<u64, Json> = HashMap::new();
        for r in self.calls {
            kept_values(&r.req, &mut kept);
        }
        let mut st = State {
            load_handles: self.load_handles,
            functions: HashMap::new(),
            pages: HashMap::new(),
            defined: HashSet::new(),
            kept,
        };
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
                    let mut used = Vec::new();
                    handles_in(&self.calls[i].req, &mut used, true);
                    if !used
                        .iter()
                        .all(|h| *h < self.load_handles || st.functions.contains_key(h))
                    {
                        continue;
                    }
                    self.step(&mut w, &mut st, i, &send)?;
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
                    self.step(&mut w, &mut st, i, &send)?;
                }
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
        send: &dyn Fn(&mut Worker, Json) -> Result<crate::client::Frame, Failure>,
    ) -> Result<(), Failure> {
        let r = &self.calls[i];
        let mut req = st.rewrite(&r.req, true);
        // The clock and random values the call read the first time.
        if let Some(clock) = r.head.get("clock") {
            req["feed"] = clock.clone();
        }
        let got = send(w, req)?;
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
        // A page's number is the worker's own count: it is matched
        // up, not compared.
        let opens = r.req.get("op").and_then(Json::as_str) == Some("pageOpen");
        let same = st.same(&want, &head)
            && match (&r.raw, &got.raw) {
                _ if opens => true,
                (Some(a), Some(b)) => st.same(a, b),
                (None, None) => true,
                _ => false,
            };
        if !same {
            let handle = r.req.get("handle").and_then(Json::as_u64);
            return Err(Failure::Differs(
                difference(r, &head, got.raw.as_ref()),
                handle,
            ));
        }
        if r.req.get("op").and_then(Json::as_str) == Some("pageOpen")
            && let (Some(a), Some(b)) = (
                r.raw.as_ref().and_then(Json::as_u64),
                got.raw.as_ref().and_then(Json::as_u64),
            )
        {
            st.pages.insert(a, b);
        }
        if head.get("dropShared").is_some() {
            st.defined.clear();
        }
        Ok(())
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
    let what = match r.req.get("op").and_then(Json::as_str) {
        Some("call") => "{fn}, which a template calls,",
        Some("pageCall" | "pageVars") => "{fn}, a computed or permalink function,",
        _ => "a function from JavaScript data",
    };
    let place = match where_(&r.label) {
        Some(w) => format!(" ({w})"),
        None => String::new(),
    };
    format!(
        "{what}{place} gives a different result when the pages' calls come in another order ({} the first time, {} the second), so it keeps state between calls and its results follow the order pages are built in, which differs between 11ty and Brrrty",
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
    /// Kept values the replay's worker has.
    defined: HashSet<u64>,
    kept: HashMap<u64, Json>,
}

/// A wire form with one key: `{"$f": 3}` gives `("$f", 3)`.
fn tagged(j: &Json) -> Option<(&str, &Json)> {
    let o = j.as_object()?;
    if o.len() != 1 {
        return None;
    }
    o.iter().next().map(|(k, v)| (k.as_str(), v))
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

fn kept_values(j: &Json, out: &mut HashMap<u64, Json>) {
    match j {
        Json::Array(a) => a.iter().for_each(|x| kept_values(x, out)),
        Json::Object(o) => {
            if let Some(("$S", Json::Array(pair))) = tagged(j)
                && let [Json::Number(id), inner] = pair.as_slice()
                && let Some(id) = id.as_u64()
            {
                out.entry(id).or_insert_with(|| inner.clone());
                kept_values(inner, out);
                return;
            }
            o.values().for_each(|v| kept_values(v, out));
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
    /// functions and pages renamed, and a kept value it doesn't have yet
    /// sent in full.
    fn rewrite(&mut self, j: &Json, top: bool) -> Json {
        match j {
            Json::Array(a) => Json::Array(a.iter().map(|x| self.rewrite(x, false)).collect()),
            Json::Object(o) => {
                match tagged(j) {
                    Some(("$f", Json::Number(n))) => {
                        if let Some(h) = n.as_u64() {
                            return json!({"$f": self.function(h)});
                        }
                    }
                    Some(("$R", Json::Number(n))) => {
                        if let Some(id) = n.as_u64()
                            && !self.defined.contains(&id)
                            && let Some(inner) = self.kept.get(&id).cloned()
                        {
                            self.defined.insert(id);
                            let inner = self.rewrite(&inner, false);
                            return json!({"$S": [id, inner]});
                        }
                        return j.clone();
                    }
                    Some(("$S", Json::Array(pair))) => {
                        if let [Json::Number(id), inner] = pair.as_slice()
                            && let Some(id) = id.as_u64()
                        {
                            if self.defined.contains(&id) {
                                return json!({"$R": id});
                            }
                            self.defined.insert(id);
                            let inner = self.rewrite(inner, false);
                            return json!({"$S": [id, inner]});
                        }
                    }
                    _ => {}
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
