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
//! `brrrty_jsregex`), and written out with nothing left to the engine's
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

use brrrty_jsregex::ast::{self, Node};
use brrrty_jsregex::classes;
use brrrty_jsregex::set::UnitSet;

/// Backtracking limits. fancy-regex counts one step per failed attempt,
/// including each start position an `exec` scans, so the steps a correct
/// match needs grow with the text it scans: on the five test sites at most
/// 138 per unit scanned (77k in one `exec`); on the fuzz at most 764 (vim's
/// list of a few hundred keywords tried at every position). Some patterns
/// are quadratic in a line's length in Node too: markdown's setext heading,
/// `\S.*(?:\n|\r\n?)(?:==+|--+)`, needs about n² steps on one line of n
/// units. So:
///
/// - Each `exec` first runs with [`FREE_STEPS`] plus [`FREE_STEPS_PER_UNIT`]
///   per unit from `lastIndex` to the end (rounded up to a power of two
///   times [`FREE_STEPS`]), uncharged: above what any site code needs, and
///   linear, so the free part stays a constant factor of a scan, as in
///   Prism.
/// - A match that needs more is run again with twice the limit, each run
///   charged in full to the highlight's spare budget: [`BUDGET_BASE`] plus
///   [`BUDGET_PER_UNIT_SQUARED`] times the square of the code's length, up
///   to [`BUDGET_MAX`].
/// - When the next run wouldn't fit in what is left, the code is refused.
///
/// Measured worst cases are in the tests (tests/backtrack.rs).
pub const FREE_STEPS: u64 = 10_000;
/// See [`FREE_STEPS`].
pub const FREE_STEPS_PER_UNIT: u64 = 256;
/// See [`FREE_STEPS`].
pub const BUDGET_BASE: u64 = 50_000_000;
/// See [`FREE_STEPS`].
pub const BUDGET_PER_UNIT_SQUARED: u64 = 2;
/// See [`FREE_STEPS`].
pub const BUDGET_MAX: u64 = 250_000_000;

/// The step limits each pattern can be compiled with (on first use).
const TIERS: [u64; 32] = {
    let mut t = [0u64; 32];
    let mut i = 0;
    while i < 32 {
        t[i] = FREE_STEPS << i;
        i += 1;
    }
    t
};

/// What is left of a highlight's backtracking budget.
#[derive(Clone, Debug)]
pub struct BacktrackBudget {
    remaining: u64,
}

impl BacktrackBudget {
    /// The budget for highlighting `units` units of code.
    pub fn for_code(units: usize) -> Self {
        let n = units as u64;
        let total = BUDGET_BASE
            .saturating_add(BUDGET_PER_UNIT_SQUARED.saturating_mul(n.saturating_mul(n)))
            .min(BUDGET_MAX);
        Self { remaining: total }
    }

    pub fn remaining(&self) -> u64 {
        self.remaining
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

/// Text as JavaScript sees it: a sequence of UTF-16 units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsText {
    /// One `char` per unit (see the module docs).
    s: String,
    /// The byte offset in `s` of each unit, and `s.len()` last.
    starts: Vec<usize>,
    /// The last unit outside ASCII that has another case (see
    /// [`fold_risk`]).
    last_cased: Option<usize>,
}

impl JsText {
    pub fn new(text: &str) -> Self {
        let mut buf = [0u16; 2];
        let units: Vec<u16> = text
            .chars()
            .flat_map(|c| c.encode_utf16(&mut buf).to_vec())
            .collect();
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
        let last_cased = last_cased(units.iter().copied());
        Self {
            s,
            starts,
            last_cased,
        }
    }

    /// The number of UTF-16 units.
    pub fn len(&self) -> usize {
        self.starts.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn units(&self) -> impl Iterator<Item = u16> + '_ {
        self.s.chars().map(char_unit)
    }

    /// Units `from..to`.
    pub fn slice(&self, from: usize, to: usize) -> Self {
        let (a, b) = (self.starts[from], self.starts[to]);
        let s = self.s[a..b].to_string();
        let starts = self.starts[from..=to].iter().map(|x| x - a).collect();
        let last_cased = last_cased(s.chars().map(char_unit));
        Self {
            s,
            starts,
            last_cased,
        }
    }

    /// The text as a Rust string, with a lone surrogate as U+FFFD (as Node
    /// writes it out in UTF-8).
    pub fn to_string_lossy(&self) -> String {
        char::decode_utf16(self.units())
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect()
    }

    /// The unit at byte offset `b` of the inner string.
    fn unit_at(&self, b: usize) -> usize {
        self.starts.binary_search(&b).unwrap_or_else(|i| i)
    }
}

/// The index of the last unit outside ASCII in [`fold_risk`].
fn last_cased(units: impl Iterator<Item = u16>) -> Option<usize> {
    let risk = fold_risk();
    let mut last = None;
    for (i, u) in units.enumerate() {
        if u >= 0x80 && risk.contains(u) {
            last = Some(i);
        }
    }
    last
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
    /// `/source/flags`.
    js: String,
    /// The `fancy-regex` pattern.
    pattern: String,
    /// The pattern compiled with each tier's backtracking limit, on first
    /// use.
    tiers: [std::sync::OnceLock<fancy_regex::Regex>; TIERS.len()],
    groups: u32,
    /// For a pattern with a backreference with `i` to a group that can hold
    /// units outside ASCII: literal units (as [`JsText`] characters), one of
    /// which starts every match that can reach such a backreference. The
    /// match is refused when a unit in [`fold_risk`] comes at or after the
    /// first of them at or after `lastIndex`: every unit the backreference
    /// and its group can touch is there.
    fold_guard: Option<Vec<String>>,
}

impl JsRegex {
    /// `exec` with the `g` flag and `lastIndex` = `last`: the first match
    /// at or after unit `last` in `text`, with a budget of its own.
    pub fn exec(&self, text: &JsText, last: usize) -> Result<Option<Match>, ExecError> {
        self.exec_budgeted(text, last, &mut BacktrackBudget::for_code(text.len()))
    }

    /// [`JsRegex::exec`], charging `budget` for backtracking past what is
    /// free (see [`FREE_STEPS`]).
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
            && let Some(cased) = text.last_cased
        {
            let rest = &text.s[text.starts[last]..];
            let first = prefixes.iter().filter_map(|p| rest.find(p.as_str())).min();
            if first.is_some_and(|b| text.unit_at(text.starts[last] + b) <= cased) {
                return Err(ExecError(
                    "this pattern compares a backreference without case, and the text has a letter outside ASCII where it could apply, which JavaScript and fancy-regex fold differently".into(),
                ));
            }
        }
        let free = FREE_STEPS
            .saturating_add(FREE_STEPS_PER_UNIT.saturating_mul((text.len() - last + 1) as u64));
        let first = TIERS
            .iter()
            .position(|&t| t >= free)
            .unwrap_or(TIERS.len() - 1);
        let mut tier = first;
        let caps = loop {
            let limit = TIERS[tier];
            if tier > first {
                if limit > budget.remaining {
                    return Err(ExecError(format!(
                        "matching this code backtracks too much: one match needed more than {} steps, past the {} it was allowed, with {} left of the highlight's spare steps",
                        TIERS[tier - 1],
                        free,
                        budget.remaining
                    )));
                }
                budget.remaining -= limit;
            }
            let re = self.tiers[tier].get_or_init(|| {
                fancy_regex::RegexBuilder::new(&self.pattern)
                    .backtrack_limit(limit as usize)
                    .build()
                    .expect("the pattern compiled before")
            });
            match re.captures_from_pos(&text.s, text.starts[last]) {
                Ok(c) => break c,
                Err(fancy_regex::Error::RuntimeError(
                    fancy_regex::RuntimeError::BacktrackLimitExceeded,
                )) if tier + 1 < TIERS.len() => tier += 1,
                Err(fancy_regex::Error::RuntimeError(
                    fancy_regex::RuntimeError::BacktrackLimitExceeded,
                )) => {
                    return Err(ExecError(format!(
                        "matching this code backtracks too much: one match needed more than {limit} steps"
                    )));
                }
                Err(e) => return Err(ExecError(e.to_string())),
            }
        };
        Ok(caps.map(|c| {
            let whole = c.get(0).expect("group 0");
            let span = |m: fancy_regex::Match<'_>| (text.unit_at(m.start()), text.unit_at(m.end()));
            let (index, end) = span(whole);
            Match {
                index,
                end,
                groups: (1..=self.groups as usize)
                    .map(|i| c.get(i).map(span))
                    .collect(),
            }
        }))
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
    let re = fancy_regex::RegexBuilder::new(&out)
        .backtrack_limit(TIERS[0] as usize)
        .build()
        .map_err(|e| Untranslatable(format!("fancy-regex refused the translation: {e}")))?;
    let tiers: [std::sync::OnceLock<fancy_regex::Regex>; TIERS.len()] = Default::default();
    let _ = tiers[0].set(re);
    Ok(JsRegex {
        js: format!("/{source}/{flags}"),
        pattern: out,
        tiers,
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

/// `c` as a pattern character.
fn push_char(out: &mut String, c: char) {
    if c.is_ascii_alphanumeric() {
        out.push(c);
    } else {
        out.push_str(&format!("\\x{{{:X}}}", c as u32));
    }
}

/// A class that matches the units in `set`, as [`JsText`] characters.
fn push_set(out: &mut String, set: &UnitSet) {
    if set.is_empty() {
        out.push_str("(?!)");
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
                push_set(out, &classes::case_closure(&UnitSet::unit(*u)));
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
            push_set(out, &if *negated { set.complement() } else { set });
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
