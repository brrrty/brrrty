//! JavaScript regular expressions, translated exactly to `fancy-regex`, or
//! refused.
//!
//! JavaScript matches UTF-16 units. A [`JsText`] holds text as one `char`
//! per unit: a unit outside the surrogate range is that character, and a
//! surrogate (half of a character outside the BMP, or a lone one) is a
//! private character from U+10F800 to U+10FFFF, which real text never has
//! once split this way. So `.`, a negated class and a range across the
//! surrogates match one unit each, as in JavaScript, and every position is
//! a unit boundary.
//!
//! The pattern is parsed as Node parses it without `u` (see
//! [`crate::ast`]), and written out with nothing left to the engine's
//! defaults: each class is an explicit set of units (`\s`, `\w`, `\d` and
//! `.` as JavaScript defines them, case folding with `i` done to the set by
//! JavaScript's Canonicalize), `\b` and `\B` as ASCII lookaround, `^` and `$`
//! as the text's ends or, with `m`, JavaScript's line terminators.
//!
//! A pattern is refused ([`Untranslatable`]) where a backtracking engine
//! can answer differently: a repetition over something that can match
//! nothing, a group captured inside a repetition that a pass can skip, a
//! backreference to a group that may be unset, a group in a lookbehind, and
//! the `u`, `v` and `y` flags.
//!
//! A backreference with `i` is compared by fancy-regex by ASCII case when
//! both sides are ASCII, as in JavaScript. When its group can hold other
//! units, [`JsRegex::exec`] refuses text with a unit outside ASCII that
//! either side folds (see `fold_risk`) where such a match could start (see
//! `JsRegex::fold_guard`), as the two can differ there.
//!
//! Backtracking is bounded (see [`BacktrackBudget`]): a match that takes
//! too many steps is refused, never left to run.

use crate::ast::{self, Node};
use crate::classes;
use crate::set::UnitSet;

/// Backtracking limits.
///
/// A pattern with nothing but what the `regex` crate matches (no lookaround,
/// `\b`, multiline anchor or backreference) runs there, in linear time, and
/// is never limited. Any other runs in two steps:
///
/// - The prefilter, the pattern with each lookaround (and `\b`, multiline
///   `^`/`$`) left out (a trailing positive lookahead matched as text) and
///   each backreference taken as any text, finds where a match can start.
///   It matches wherever the pattern can, in the `regex` crate, in linear
///   time: a reverse lazy DFA finds all its starts in one pass (kept for a
///   whole text, which greedy patterns and hook loops search again and
///   again), and forward searches find the first few in a part of a text.
/// - At each such start, from `lastIndex` on, the pattern anchored there
///   (`\G`) runs in fancy-regex's backtracking engine until one matches.
///
/// Each anchored attempt runs with a step limit of [`FIRST_STEPS`], doubled
/// until it answers, and every limit tried is charged to the highlight's
/// budget (within four times the steps taken, plus [`FIRST_STEPS`]). The
/// budget is one total for the whole highlight, nested ones included:
/// [`BUDGET_BASE`] plus [`BUDGET_PER_UNIT`] per unit of code. Measured
/// charges: at most 47 thousand for a code block on the test sites, 4.2
/// million for Prism's own test fixtures (a 9,000-unit vim file, 470 a
/// unit), 86 million for a 90,594-unit minified bundle (955 a unit). When the
/// next attempt won't fit, the code is refused: only backtracking that grows
/// faster than the code does (as in a long run of unclosed template strings)
/// gets there, in seconds (tests/backtrack.rs).
pub const FIRST_STEPS: u64 = 64;
/// See [`FIRST_STEPS`].
pub const BUDGET_BASE: u64 = 20_000_000;
/// See [`FIRST_STEPS`].
pub const BUDGET_PER_UNIT: u64 = 2_000;

/// The step limits each pattern can be compiled with (on first use).
const TIERS: [u64; 40] = {
    let mut t = [0u64; 40];
    let mut i = 0;
    while i < 40 {
        t[i] = FIRST_STEPS << i;
        i += 1;
    }
    t
};

/// What is left of a highlight's backtracking budget.
#[derive(Clone, Debug)]
pub struct BacktrackBudget {
    remaining: u64,
    /// What was charged so far.
    spent: u64,
}

impl BacktrackBudget {
    /// The budget for highlighting `units` units of code.
    pub fn for_code(units: usize) -> Self {
        let n = units as u64;
        let total = BUDGET_BASE.saturating_add(BUDGET_PER_UNIT.saturating_mul(n));
        Self {
            remaining: total,
            spent: 0,
        }
    }

    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    /// The steps charged so far.
    pub fn spent(&self) -> u64 {
        self.spent
    }
}

/// The first of the private characters that stand for surrogates.
const SURROGATE_BASE: u32 = 0x10_F800;

/// The `char` that stands for UTF-16 unit `u`.
fn unit_char(u: u16) -> char {
    let u = u32::from(u);
    let c = if (0xD800..0xE000).contains(&u) {
        SURROGATE_BASE + (u - 0xD800)
    } else {
        u
    };
    char::from_u32(c).unwrap_or('\u{FFFD}')
}

/// The UTF-16 unit a [`JsText`] `char` stands for.
fn char_unit(c: char) -> u16 {
    let c = c as u32;
    if c >= SURROGATE_BASE {
        (c - SURROGATE_BASE + 0xD800) as u16
    } else {
        c as u16
    }
}

/// The text a [`JsText`] is a view of.
#[derive(Debug)]
struct Buf {
    /// One `char` per unit (see the module docs).
    s: String,
    /// The byte offset in `s` of each unit, and `s.len()` last.
    starts: Vec<usize>,
    /// For each unit (and the end), the first unit at or after it that is
    /// outside ASCII and has another case (see [`fold_risk`]), or
    /// `u32::MAX`.
    next_cased: Vec<u32>,
    /// Where each guard prefix occurs (see `JsRegex::fold_guard`), found on
    /// first use.
    occurrences: std::sync::Mutex<Vec<(String, std::sync::Arc<Vec<usize>>)>>,
    /// Where each pattern's prefilter can match in the whole text, by
    /// pattern id, found on first use.
    prefiltered: std::sync::Mutex<Vec<(usize, std::sync::Arc<Vec<usize>>)>>,
}

/// Text as JavaScript sees it: a sequence of UTF-16 units. A slice is a
/// view of the same text, made in constant time.
#[derive(Clone, Debug)]
pub struct JsText {
    buf: std::sync::Arc<Buf>,
    from: usize,
    to: usize,
}

impl PartialEq for JsText {
    fn eq(&self, other: &Self) -> bool {
        self.hay() == other.hay()
    }
}

impl Eq for JsText {}

impl JsText {
    pub fn new(text: &str) -> Self {
        let units: Vec<u16> = text.encode_utf16().collect();
        Self::from_units(&units)
    }

    pub fn from_units(units: &[u16]) -> Self {
        let mut s = String::with_capacity(units.len());
        let mut starts = Vec::with_capacity(units.len() + 1);
        for &u in units {
            starts.push(s.len());
            s.push(unit_char(u));
        }
        starts.push(s.len());
        let risk = fold_risk();
        let mut next_cased = vec![u32::MAX; units.len() + 1];
        for i in (0..units.len()).rev() {
            let u = units[i];
            next_cased[i] = if u >= 0x80 && risk.contains(u) {
                i as u32
            } else {
                next_cased[i + 1]
            };
        }
        let to = units.len();
        Self {
            buf: std::sync::Arc::new(Buf {
                s,
                starts,
                next_cased,
                occurrences: std::sync::Mutex::new(Vec::new()),
                prefiltered: std::sync::Mutex::new(Vec::new()),
            }),
            from: 0,
            to,
        }
    }

    /// The number of UTF-16 units.
    pub fn len(&self) -> usize {
        self.to - self.from
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn units(&self) -> impl Iterator<Item = u16> + '_ {
        self.hay().chars().map(char_unit)
    }

    /// Units `from..to`, a view of the same text.
    pub fn slice(&self, from: usize, to: usize) -> Self {
        assert!(from <= to && to <= self.len());
        Self {
            buf: self.buf.clone(),
            from: self.from + from,
            to: self.from + to,
        }
    }

    /// The text as a Rust string, with a lone surrogate as U+FFFD (as Node
    /// writes it out in UTF-8).
    pub fn to_string_lossy(&self) -> String {
        char::decode_utf16(self.units())
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect()
    }

    /// The view as JsText characters.
    fn hay(&self) -> &str {
        &self.buf.s[self.buf.starts[self.from]..self.buf.starts[self.to]]
    }

    /// The byte offset in [`JsText::hay`] of unit `u` of the view.
    fn byte(&self, u: usize) -> usize {
        self.buf.starts[self.from + u] - self.buf.starts[self.from]
    }

    /// The unit of the view at byte offset `b` of [`JsText::hay`].
    fn unit_at(&self, b: usize) -> usize {
        let abs = self.buf.starts[self.from] + b;
        self.buf.starts.binary_search(&abs).unwrap_or_else(|i| i) - self.from
    }

    /// Whether the view is all of its text.
    fn is_whole(&self) -> bool {
        self.from == 0 && self.to + 1 == self.buf.starts.len()
    }

    /// Where `pre` (the prefilter of pattern `id`) can match in the view,
    /// as byte offsets of [`JsText::hay`]; kept for a view of the whole
    /// text, which a greedy pattern runs on again and again.
    fn starts_of(&self, id: usize, pre: &Starts) -> std::sync::Arc<Vec<usize>> {
        let whole = self.is_whole();
        if whole
            && let Some((_, s)) = self
                .buf
                .prefiltered
                .lock()
                .expect("not poisoned")
                .iter()
                .find(|(k, _)| *k == id)
        {
            return s.clone();
        }
        let s = std::sync::Arc::new(pre.find(self.hay()));
        if whole {
            self.buf
                .prefiltered
                .lock()
                .expect("not poisoned")
                .push((id, s.clone()));
        }
        s
    }

    /// Whether a unit in [`fold_risk`] is at or after unit `u` of the view.
    fn cased_from(&self, u: usize) -> bool {
        (self.buf.next_cased[self.from + u] as usize) < self.to
    }

    /// The first unit at or after `u` where `prefix` (JsText characters)
    /// starts and fits in the view.
    fn find_from(&self, prefix: &str, u: usize) -> Option<usize> {
        let occ = {
            let mut cache = self.buf.occurrences.lock().expect("not poisoned");
            match cache.iter().find(|(p, _)| p == prefix) {
                Some((_, o)) => o.clone(),
                None => {
                    let mut o = Vec::new();
                    let s = &self.buf.s;
                    let mut b = 0;
                    while let Some(i) = s[b..].find(prefix) {
                        let at = b + i;
                        o.push(self.buf.starts.binary_search(&at).unwrap_or_else(|x| x));
                        b = at + s[at..].chars().next().map_or(1, char::len_utf8);
                    }
                    let o = std::sync::Arc::new(o);
                    cache.push((prefix.to_string(), o.clone()));
                    o
                }
            }
        };
        let len = prefix.chars().count();
        let abs = self.from + u;
        let i = occ.partition_point(|&x| x < abs);
        occ.get(i)
            .filter(|&&x| x + len <= self.to)
            .map(|&x| x - self.from)
    }
}

/// Why a pattern can't be translated exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Untranslatable(pub String);

/// A match, in UTF-16 units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub index: usize,
    pub end: usize,
    /// Groups 1 and up: `None` for one that took no part.
    pub groups: Vec<Option<(usize, usize)>>,
}

/// A match was refused: the backtracking budget ran out, the case-folding
/// guard applies, or the engine failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecError(pub String);

/// The units outside ASCII that `i` folds with another unit, in
/// JavaScript (Canonicalize) or in fancy-regex (Unicode simple case
/// folding). A backreference with `i` to a group that can hold such a unit
/// can compare differently in the two.
fn fold_risk() -> &'static UnitSet {
    static RISK: std::sync::OnceLock<UnitSet> = std::sync::OnceLock::new();
    RISK.get_or_init(|| {
        use regex_syntax::hir::{ClassUnicode, ClassUnicodeRange};
        let mut out = Vec::new();
        for u in 0x80u16..=0xFFFF {
            if (0xD800..0xE000).contains(&u) {
                continue;
            }
            let js = classes::cased_units().contains(u);
            let c = unit_char(u);
            let mut class = ClassUnicode::new([ClassUnicodeRange::new(c, c)]);
            class.case_fold_simple();
            let rust = class.ranges() != [ClassUnicodeRange::new(c, c)];
            if js || rust {
                out.push((u, u));
            }
        }
        UnitSet::from_ranges(out)
    })
}

/// A translated pattern.
#[derive(Debug)]
pub struct JsRegex {
    /// Unique to this pattern (keys the start positions a text caches).
    id: usize,
    /// `/source/flags`.
    js: String,
    /// The `fancy-regex` pattern.
    pattern: String,
    /// The pattern in the `regex` crate, when it has nothing that crate
    /// can't match (it then runs in linear time, unlimited).
    easy: Option<regex::Regex>,
    /// Otherwise, the prefilter (see [`FIRST_STEPS`]), and the pattern
    /// after `\G` in fancy-regex, compiled with each tier's limit on first
    /// use.
    prefilter: Option<Starts>,
    anchored: Option<(
        String,
        [std::sync::OnceLock<fancy_regex::Regex>; TIERS.len()],
    )>,
    groups: u32,
    /// For a pattern with a backreference with `i` to a group that can hold
    /// units outside ASCII: literal units (as [`JsText`] characters), one of
    /// which starts every match that can reach such a backreference. The
    /// match is refused when a unit in [`fold_risk`] comes at or after the
    /// first of them at or after `lastIndex`: every unit the backreference
    /// and its group can touch is there.
    fold_guard: Option<Vec<String>>,
}

/// The prefilter: where a match of it can start, found for a whole text
/// at once by a reverse lazy DFA (every match, `MatchKind::All`), in
/// linear time.
#[derive(Debug)]
struct Starts {
    /// The prefilter, forward, for the first start alone.
    forward: regex::Regex,
    dfa: regex_automata::hybrid::dfa::DFA,
    caches: std::sync::Mutex<Vec<regex_automata::hybrid::dfa::Cache>>,
}

impl Starts {
    fn new(pattern: &str) -> Result<Self, String> {
        use regex_automata::hybrid::dfa::DFA;
        use regex_automata::{MatchKind, nfa::thompson};
        let dfa = DFA::builder()
            .configure(DFA::config().match_kind(MatchKind::All))
            .thompson(thompson::Config::new().reverse(true))
            .build(pattern)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            forward: regex::Regex::new(pattern).map_err(|e| e.to_string())?,
            dfa,
            caches: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// The byte offsets in `hay` where a match can start, in order. If the
    /// DFA gives up, every character boundary (still every start).
    fn find(&self, hay: &str) -> Vec<usize> {
        let mut cache = self
            .caches
            .lock()
            .expect("not poisoned")
            .pop()
            .unwrap_or_else(|| self.dfa.create_cache());
        let out = self.scan(hay, &mut cache).unwrap_or_else(|| {
            hay.char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(hay.len()))
                .collect()
        });
        self.caches.lock().expect("not poisoned").push(cache);
        out
    }

    fn scan(
        &self,
        hay: &str,
        cache: &mut regex_automata::hybrid::dfa::Cache,
    ) -> Option<Vec<usize>> {
        let input = regex_automata::Input::new(hay);
        let bytes = hay.as_bytes();
        let mut out = Vec::new();
        let mut sid = self.dfa.start_state_reverse(cache, &input).ok()?;
        // A match state is entered one byte late: after the byte at `at`,
        // a match starts at `at + 1`.
        for at in (0..bytes.len()).rev() {
            sid = self.dfa.next_state(cache, sid, bytes[at]).ok()?;
            if sid.is_tagged() {
                if sid.is_match() {
                    out.push(at + 1);
                } else if sid.is_dead() {
                    break;
                } else if sid.is_quit() {
                    return None;
                }
            }
        }
        sid = self.dfa.next_eoi_state(cache, sid).ok()?;
        if sid.is_match() {
            out.push(0);
        }
        out.reverse();
        out.retain(|&i| hay.is_char_boundary(i));
        Some(out)
    }
}

fn runtime(e: fancy_regex::Error) -> ExecError {
    ExecError(e.to_string())
}

impl JsRegex {
    /// `exec` with the `g` flag and `lastIndex` = `last`: the first match
    /// at or after unit `last` in `text`, with a budget of its own.
    pub fn exec(&self, text: &JsText, last: usize) -> Result<Option<Match>, ExecError> {
        self.exec_budgeted(text, last, &mut BacktrackBudget::for_code(text.len()))
    }

    /// [`JsRegex::exec`], charging `budget` for backtracking (see
    /// [`FIRST_STEPS`]).
    pub fn exec_budgeted(
        &self,
        text: &JsText,
        last: usize,
        budget: &mut BacktrackBudget,
    ) -> Result<Option<Match>, ExecError> {
        if last > text.len() {
            return Ok(None);
        }
        if let Some(prefixes) = &self.fold_guard
            && text.cased_from(last)
        {
            let first = prefixes
                .iter()
                .filter_map(|p| text.find_from(p, last))
                .min();
            if first.is_some_and(|p| text.cased_from(p)) {
                return Err(ExecError(
                    "this pattern compares a backreference without case, and the text has a letter outside ASCII where it could apply, which JavaScript and fancy-regex fold differently".into(),
                ));
            }
        }
        let hay = text.hay();
        let found: Option<Vec<Option<(usize, usize)>>> =
            match (&self.easy, &self.prefilter, &self.anchored) {
                (Some(re), _, _) => re.captures_at(hay, text.byte(last)).map(|c| {
                    (0..=self.groups as usize)
                        .map(|i| c.get(i).map(|m| (m.start(), m.end())))
                        .collect()
                }),
                (None, Some(pre), Some((pattern, tiers))) => {
                    let b = text.byte(last);
                    let starts = if text.is_whole() {
                        text.starts_of(self.id, pre)
                    } else {
                        // A view of part of a text (a non-greedy pattern on one
                        // string of the token list) is usually matched once,
                        // near its start: candidates come from forward searches,
                        // which stop at each, until they have read twice the
                        // view; then all starts are found at once.
                        let mut from = b;
                        let mut read = 0;
                        loop {
                            let Some(m) = pre.forward.find_at(hay, from) else {
                                return Ok(None);
                            };
                            let at = m.start();
                            read += m.end() - from;
                            if let Some(caps) = Self::attempt(pattern, tiers, hay, at, budget)? {
                                let g: Vec<Option<(usize, usize)>> = (0..=self.groups as usize)
                                    .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                                    .collect();
                                return Ok(Some(self.to_match(text, &g)));
                            }
                            match hay[at..].chars().next() {
                                Some(ch) => from = at + ch.len_utf8(),
                                None => return Ok(None),
                            }
                            if read > 2 * hay.len() + 64 {
                                let mut all = pre.find(hay);
                                all.retain(|&x| x >= from);
                                break std::sync::Arc::new(all);
                            }
                        }
                    };
                    let from = starts.partition_point(|&x| x < b);
                    let mut found = None;
                    for &at in &starts[from..] {
                        if let Some(caps) = Self::attempt(pattern, tiers, hay, at, budget)? {
                            found = Some(
                                (0..=self.groups as usize)
                                    .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                                    .collect(),
                            );
                            break;
                        }
                    }
                    found
                }
                _ => unreachable!("a pattern is easy or has a prefilter"),
            };
        Ok(found.map(|g| self.to_match(text, &g)))
    }

    /// Byte spans of `hay` (group 0 first) as a [`Match`] in units.
    fn to_match(&self, text: &JsText, g: &[Option<(usize, usize)>]) -> Match {
        let span = |(a, b): (usize, usize)| (text.unit_at(a), text.unit_at(b));
        let (index, end) = span(g[0].expect("group 0"));
        Match {
            index,
            end,
            groups: g[1..].iter().map(|x| x.map(span)).collect(),
        }
    }

    /// The pattern anchored at byte `at` of `hay`, its limit doubled until
    /// it answers, each limit charged.
    fn attempt<'h>(
        pattern: &str,
        tiers: &[std::sync::OnceLock<fancy_regex::Regex>; TIERS.len()],
        hay: &'h str,
        at: usize,
        budget: &mut BacktrackBudget,
    ) -> Result<Option<fancy_regex::Captures<'h, str>>, ExecError> {
        for (tier, &limit) in TIERS.iter().enumerate() {
            if limit > budget.remaining {
                return Err(ExecError(format!(
                    "matching this code backtracks too much: the highlight took its {} steps (one match was at more than {})",
                    budget.spent + budget.remaining,
                    limit / 2
                )));
            }
            budget.remaining -= limit;
            budget.spent += limit;
            let re = tiers[tier].get_or_init(|| {
                fancy_regex::RegexBuilder::new(pattern)
                    .backtrack_limit(limit as usize)
                    .build()
                    .expect("the pattern compiled before")
            });
            match re.captures_from_pos(hay, at) {
                Ok(c) => return Ok(c),
                Err(fancy_regex::Error::RuntimeError(
                    fancy_regex::RuntimeError::BacktrackLimitExceeded,
                )) => {}
                Err(e) => return Err(runtime(e)),
            }
        }
        Err(ExecError("matching this code backtracks too much".into()))
    }

    /// The JavaScript pattern, as `/source/flags`.
    pub fn js_source(&self) -> &str {
        &self.js
    }

    /// The `fancy-regex` pattern.
    pub fn as_str(&self) -> &str {
        &self.pattern
    }
}

#[derive(Clone, Copy, Default)]
struct Flags {
    icase: bool,
    multiline: bool,
    dot_all: bool,
    /// Write the prefilter: lookaround left out, a backreference as any text.
    loose: bool,
}

/// Translate the JavaScript pattern `source` with `flags`.
pub fn translate(source: &str, flags: &str) -> Result<JsRegex, Untranslatable> {
    translate_with(source, flags, false)
}

/// [`translate`], for a caller that reads only where a match is, never its
/// groups (a `replace` with a function that takes only the match). A group
/// that captures inside a repetition and can sit out a pass is then allowed
/// when every backreference to it is inside that repetition and reached,
/// in each pass, only after the pass set it: the value JavaScript clears at
/// the start of a pass is never read, so only the groups reported (which
/// [`JsRegex::exec`] then leaves out) could differ.
pub fn translate_spans(source: &str, flags: &str) -> Result<JsRegex, Untranslatable> {
    translate_with(source, flags, true)
}

/// Whether, for each repetition with a group inside that a pass can leave
/// unset, every backreference to such a group is inside the repetition and
/// set earlier in the same pass.
fn stale_groups_unread(root: &Node) -> bool {
    fn walk(root: &Node, node: &Node) -> bool {
        match node {
            Node::Repeat { node: body, .. } => {
                let mut inside = Vec::new();
                capture_indexes(body, &mut inside);
                let set = ast::always_set(body);
                let optional: Vec<u32> = inside.into_iter().filter(|i| !set.contains(i)).collect();
                let ok = optional.is_empty()
                    || (ast::backrefs_resolved(body).is_ok()
                        && count_refs(root, &optional) == count_refs(body, &optional));
                ok && walk(root, body)
            }
            Node::Group { node, .. } | Node::Look { node, .. } => walk(root, node),
            Node::Concat(xs) | Node::Alt(xs) => xs.iter().all(|x| walk(root, x)),
            _ => true,
        }
    }
    walk(root, root)
}

fn capture_indexes(node: &Node, out: &mut Vec<u32>) {
    ast::capture_indexes(node, out)
}

/// How many backreferences to one of `groups` `node` has.
fn count_refs(node: &Node, groups: &[u32]) -> usize {
    let mut refs = Vec::new();
    backrefs(node, &mut refs);
    refs.iter().filter(|k| groups.contains(k)).count()
}

fn translate_with(source: &str, flags: &str, spans_only: bool) -> Result<JsRegex, Untranslatable> {
    let no = |s: String| Err(Untranslatable(s));
    let mut f = Flags::default();
    let mut seen = String::new();
    for c in flags.chars() {
        if seen.contains(c) {
            return no(format!("the `{c}` flag is repeated"));
        }
        seen.push(c);
        match c {
            // `g` and `d` don't change what matches.
            'g' | 'd' => {}
            'i' => f.icase = true,
            'm' => f.multiline = true,
            's' => f.dot_all = true,
            'u' | 'v' | 'y' => return no(format!("the `{c}` flag isn't supported")),
            _ => return no(format!("`{c}` isn't a regular expression flag")),
        }
    }
    let pattern = ast::parse(source, f.dot_all).map_err(Untranslatable)?;
    let node = &pattern.node;
    if ast::repeats_nothing(node) {
        return no("a repeated or optional part that can match nothing, such as `(?:a|)*`, isn't supported: JavaScript skips a repetition that matches nothing".into());
    }
    if !ast::captures_reset_alike(node) && !(spans_only && stale_groups_unread(node)) {
        return no("a group that captures inside a repeated group, and can sit out a repetition, isn't supported: JavaScript clears it at the start of every repetition".into());
    }
    if ast::captures_in_lookbehind(node) {
        return no("a capturing group in a lookbehind isn't supported: JavaScript matches a lookbehind from right to left".into());
    }
    ast::backrefs_resolved(node).map_err(Untranslatable)?;
    // fancy-regex compares a backreference with `i` by ASCII case when both
    // sides are ASCII, as JavaScript does; otherwise by Unicode folding.
    let mut refs = Vec::new();
    backrefs(node, &mut refs);
    let fold_guard = (f.icase
        && refs.iter().any(|&k| {
            ast::group_alphabet(node, k, true)
                .is_none_or(|a| a.ranges().iter().any(|r| r.1 >= 0x80))
        }))
    .then(|| {
        let risky: Vec<u32> = refs
            .iter()
            .copied()
            .filter(|&k| {
                ast::group_alphabet(node, k, true)
                    .is_none_or(|a| a.ranges().iter().any(|r| r.1 >= 0x80))
            })
            .collect();
        guard_prefixes(node, &risky)
            .into_iter()
            .map(|p| p.into_iter().map(unit_char).collect())
            .collect()
    });
    let mut out = String::new();
    emit(node, f, &mut out);
    let build = |p: &str| {
        fancy_regex::RegexBuilder::new(p)
            .backtrack_limit(TIERS[0] as usize)
            .build()
            .map_err(|e| Untranslatable(format!("fancy-regex refused the translation: {e}")))
    };
    // fancy-regex checks the pattern.
    build(&out)?;
    let easy = regex::Regex::new(&out).ok();
    let (prefilter, anchored) = if easy.is_none() {
        let mut loose = String::new();
        emit_prefilter(node, Flags { loose: true, ..f }, &mut loose);
        let prefilter = Starts::new(&loose).map_err(|e| {
            Untranslatable(format!(
                "the prefilter of this pattern doesn't compile: {e}"
            ))
        })?;
        let anchored = format!(r"\G(?:{out})");
        let tiers: [std::sync::OnceLock<fancy_regex::Regex>; TIERS.len()] =
            std::array::from_fn(|_| std::sync::OnceLock::new());
        let _ = tiers[0].set(build(&anchored)?);
        (Some(prefilter), Some((anchored, tiers)))
    } else {
        (None, None)
    };
    static IDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    Ok(JsRegex {
        id: IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        js: format!("/{source}/{flags}"),
        pattern: out,
        easy,
        prefilter,
        anchored,
        groups: if spans_only { 0 } else { pattern.groups },
        fold_guard,
    })
}

/// Whether `node` has a backreference to one of `groups`.
fn refers_to(node: &Node, groups: &[u32]) -> bool {
    let mut refs = Vec::new();
    backrefs(node, &mut refs);
    refs.iter().any(|k| groups.contains(k))
}

/// Literal units, one of which starts every match of `node` that reaches a
/// backreference to one of `risky`: the units every match starts with,
/// and where those lead to alternatives, each alternative's own (only the
/// alternatives that can reach such a backreference). An empty prefix
/// means any position.
fn guard_prefixes(node: &Node, risky: &[u32]) -> Vec<Vec<u16>> {
    let xs: &[Node] = match node {
        Node::Concat(xs) => xs,
        other => std::slice::from_ref(other),
    };
    let uncased = |u: u16| !classes::cased_units().contains(u);
    let mut prefix = Vec::new();
    for (i, x) in xs.iter().enumerate() {
        match x {
            Node::Unit(u) if uncased(*u) => prefix.push(*u),
            Node::Alt(branches) => {
                let after = xs[i + 1..].iter().any(|y| refers_to(y, risky));
                return branches
                    .iter()
                    .filter(|b| after || refers_to(b, risky))
                    .map(|b| {
                        let mut p = prefix.clone();
                        p.extend(literal_prefix(b));
                        p
                    })
                    .collect();
            }
            _ => break,
        }
    }
    vec![prefix]
}

/// The group of every backreference in `node`.
fn backrefs(node: &Node, out: &mut Vec<u32>) {
    match node {
        Node::Backref(k) => out.push(*k),
        Node::Group { node, .. } | Node::Look { node, .. } | Node::Repeat { node, .. } => {
            backrefs(node, out)
        }
        Node::Concat(xs) | Node::Alt(xs) => xs.iter().for_each(|x| backrefs(x, out)),
        _ => {}
    }
}

/// The units every match of `node` starts with, while they have no other
/// case.
fn literal_prefix(node: &Node) -> Vec<u16> {
    let uncased = |u: u16| !classes::cased_units().contains(u);
    match node {
        Node::Unit(u) if uncased(*u) => vec![*u],
        Node::Concat(xs) => xs
            .iter()
            .map_while(|x| match x {
                Node::Unit(u) if uncased(*u) => Some(*u),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The prefilter of a whole pattern: `emit` with `loose`, except that a
/// positive lookahead the pattern ends with is matched as text. A match of
/// the pattern at `q` that ends at `e` has its lookahead match at `e`, so
/// the prefilter matches at `q` too: its starts still include every start
/// of the pattern.
fn emit_prefilter(node: &Node, f: Flags, out: &mut String) {
    let zero_width = |x: &Node| {
        matches!(x, Node::Empty | Node::Look { .. } | Node::Boundary { .. })
            || (f.multiline && matches!(x, Node::Start | Node::End))
    };
    match node {
        Node::Alt(xs) => {
            out.push_str("(?:");
            for (i, x) in xs.iter().enumerate() {
                if i > 0 {
                    out.push('|');
                }
                emit_prefilter(x, f, out);
            }
            out.push(')');
        }
        Node::Group { node, .. } => {
            out.push_str("(?:");
            emit_prefilter(node, f, out);
            out.push(')');
        }
        Node::Look {
            behind: false,
            negate: false,
            node,
        } => emit_prefilter(node, f, out),
        Node::Concat(xs) => {
            let k = xs.iter().rposition(|x| !zero_width(x)).map_or(0, |k| k + 1);
            for x in &xs[..k] {
                emit(x, f, out);
            }
            // The first positive lookahead after the last part that takes
            // text; the rest of that tail takes none, and is left out.
            if let Some(Node::Look { node, .. }) = xs[k..].iter().find(|x| {
                matches!(
                    x,
                    Node::Look {
                        behind: false,
                        negate: false,
                        ..
                    }
                )
            }) {
                emit_prefilter(node, f, out);
            }
        }
        other => emit(other, f, out),
    }
}

/// `c` as a pattern character.
fn push_char(out: &mut String, c: char) {
    if c.is_ascii_alphanumeric() {
        out.push(c);
    } else {
        out.push_str(&format!("\\x{{{:X}}}", c as u32));
    }
}

/// A class that matches the units in `set`, as [`JsText`] characters.
fn push_set(out: &mut String, set: &UnitSet, f: Flags) {
    if set.is_empty() {
        // Nothing matches; the `regex` crate has no `(?!)`.
        out.push_str(if f.loose {
            r"[^\x{0}-\x{10FFFF}]"
        } else {
            "(?!)"
        });
        return;
    }
    if set.len() == 1 {
        push_char(out, unit_char(set.ranges()[0].0));
        return;
    }
    out.push('[');
    let mut range = |a: char, b: char| {
        push_char(out, a);
        if b != a {
            out.push('-');
            push_char(out, b);
        }
    };
    for &(a, b) in set.ranges() {
        // Below the surrogates, the surrogates (as private characters),
        // and above them.
        for (lo, hi) in [(0, 0xD7FF), (0xD800, 0xDFFF), (0xE000, 0xFFFF)] {
            let (x, y) = (a.max(lo), b.min(hi));
            if x <= y {
                range(unit_char(x), unit_char(y));
            }
        }
    }
    out.push(']');
}

const WORD: &str = "[0-9A-Za-z_]";
const LINE: &str = r"[\x{A}\x{D}\x{2028}\x{2029}]";

fn emit(node: &Node, f: Flags, out: &mut String) {
    match node {
        Node::Empty => {}
        Node::Unit(u) => {
            if f.icase {
                push_set(out, &classes::case_closure(&UnitSet::unit(*u)), f);
            } else {
                push_char(out, unit_char(*u));
            }
        }
        Node::Set { set, negated } => {
            let set = if f.icase {
                classes::case_closure(set)
            } else {
                set.clone()
            };
            push_set(out, &if *negated { set.complement() } else { set }, f);
        }
        Node::Start | Node::End | Node::Boundary { .. } | Node::Look { .. }
            if f.loose
                && (f.multiline || matches!(node, Node::Boundary { .. } | Node::Look { .. })) => {}
        Node::Backref(_) if f.loose => out.push_str("(?s:.)*"),
        Node::Group { node, .. } if f.loose => {
            out.push_str("(?:");
            emit(node, f, out);
            out.push(')');
        }
        Node::Start if f.multiline => out.push_str(&format!(r"(?:\A|(?<={LINE}))")),
        Node::Start => out.push_str(r"\A"),
        Node::End if f.multiline => out.push_str(&format!(r"(?={LINE}|\z)")),
        Node::End => out.push_str(r"\z"),
        Node::Boundary { negate: false } => {
            out.push_str(&format!("(?:(?<={WORD})(?!{WORD})|(?<!{WORD})(?={WORD}))"))
        }
        Node::Boundary { negate: true } => {
            out.push_str(&format!("(?:(?<={WORD})(?={WORD})|(?<!{WORD})(?!{WORD}))"))
        }
        Node::Group { node, .. } => {
            out.push('(');
            emit(node, f, out);
            out.push(')');
        }
        Node::Look {
            behind,
            negate,
            node,
        } => {
            out.push_str(match (behind, negate) {
                (false, false) => "(?=",
                (false, true) => "(?!",
                (true, false) => "(?<=",
                (true, true) => "(?<!",
            });
            emit(node, f, out);
            out.push(')');
        }
        Node::Backref(k) if f.icase => out.push_str(&format!(r"(?i:\{k})")),
        Node::Backref(k) => out.push_str(&format!(r"(?:\{k})")),
        Node::Repeat {
            node,
            min,
            max,
            greedy,
        } => {
            out.push_str("(?:");
            emit(node, f, out);
            out.push(')');
            match max {
                Some(m) => out.push_str(&format!("{{{min},{m}}}")),
                None => out.push_str(&format!("{{{min},}}")),
            }
            if !greedy {
                out.push('?');
            }
        }
        Node::Concat(xs) => xs.iter().for_each(|x| emit(x, f, out)),
        Node::Alt(xs) => {
            out.push_str("(?:");
            for (i, x) in xs.iter().enumerate() {
                if i > 0 {
                    out.push('|');
                }
                emit(x, f, out);
            }
            out.push(')');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_round_trip() {
        let t = JsText::new("a😀é");
        assert_eq!(t.len(), 4);
        assert_eq!(t.units().collect::<Vec<_>>(), [0x61, 0xD83D, 0xDE00, 0xE9]);
        assert_eq!(t.to_string_lossy(), "a😀é");
        assert_eq!(t.slice(0, 2).to_string_lossy(), "a\u{FFFD}");
        assert_eq!(t.slice(1, 3).to_string_lossy(), "😀");
    }
}
