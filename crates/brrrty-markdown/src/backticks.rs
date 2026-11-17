//! markdown-it's `backtick` rule (code spans), in place of the port's: 14.3.3's (11ty's) and
//! 15's, which differ. Both take a run of backticks with no closer as text, so the backticks
//! after it never start a code span; the port retries each one. Positions are bytes; every
//! character compared is a backtick, so they line up with markdown-it's units.

use markdown_it::parser::extset::InlineRootExt;
use markdown_it::parser::inline::{InlineRule, InlineState, Text};
use markdown_it::plugins::cmark::inline::backticks::CodeInline;
use markdown_it::{MarkdownIt, Node};

/// `state.backticksScanned` and `state.backticks`.
#[derive(Debug, Default)]
pub(crate) struct Scanned {
    /// The inline source (`state.src` can't be read while this is borrowed).
    src: Option<std::sync::Arc<str>>,
    done: bool,
    /// What `check` found at each position: markdown-it's `skipToken` caches the position it
    /// skips to (`state.cache`), so a second scan of a label doesn't run the rule again.
    memo: std::collections::HashMap<usize, Option<usize>>,
    /// Run length to a position (14.3.3: an upper bound for a closer; 15: the last run).
    runs: std::collections::HashMap<usize, usize>,
}
impl InlineRootExt for Scanned {}

/// Takes the backtick caches out (an image label starts with none).
pub(crate) fn take(state: &mut InlineState) -> Option<Scanned> {
    state.inline_ext.remove::<Scanned>()
}

/// Puts back what [`take`] took.
pub(crate) fn restore(state: &mut InlineState, saved: Option<Scanned>) {
    state.inline_ext.remove::<Scanned>();
    if let Some(s) = saved {
        state.inline_ext.insert(s);
    }
}

/// The rule's state, with the source.
fn scanned<'a>(state: &'a mut InlineState) -> (&'a mut Scanned, std::sync::Arc<str>) {
    let have = state
        .inline_ext
        .get::<Scanned>()
        .is_some_and(|s| s.src.is_some());
    let fresh = (!have).then(|| std::sync::Arc::<str>::from(state.src.as_str()));
    let cache = state.inline_ext.get_or_insert_default::<Scanned>();
    if let Some(f) = fresh {
        cache.src = Some(f);
    }
    let src = cache.src.clone().expect("set above");
    (cache, src)
}

/// What the rule does at `state.pos`.
enum Found {
    /// A code span: the content and how far it reaches.
    Code {
        marker_len: usize,
        content: String,
        end: usize,
    },
    /// The opening run, as text.
    Text(usize),
}

fn run_len(src: &[u8], from: usize, max: usize) -> usize {
    src[from..max].iter().take_while(|&&b| b == b'`').count()
}

/// markdown-it 14.3.3's `backtick`.
fn scan_14(state: &mut InlineState) -> Option<Found> {
    let start = state.pos;
    if state.src.as_bytes().get(start) != Some(&b'`') {
        return None;
    }
    let max = state.pos_max;
    let limit = crate::image::text_end(state);
    let (cache, text) = scanned(state);
    let src = text.as_bytes();
    let pos = start + run_len(src, start, max);
    let opener = pos - start;
    if cache.done && cache.runs.get(&opener).copied().unwrap_or(0) <= start {
        return Some(Found::Text(opener));
    }
    let mut match_end = pos;
    while let Some(k) = text[match_end..limit].find('`') {
        let match_start = match_end + k;
        match_end = match_start + 1;
        while match_end < max && src[match_end] == b'`' {
            match_end += 1;
        }
        let closer = match_end - match_start;
        if closer == opener {
            let content = text[pos..match_start].replace('\n', " ");
            // .replace(/^ (.+) $/, '$1'): `.` stops at a line terminator.
            let strip = content.len() >= 3
                && content.starts_with(' ')
                && content.ends_with(' ')
                && !content[1..content.len() - 1].contains(['\r', '\u{2028}', '\u{2029}']);
            let content = if strip {
                content[1..content.len() - 1].to_string()
            } else {
                content
            };
            return Some(Found::Code {
                marker_len: opener,
                content,
                end: match_end,
            });
        }
        cache.runs.insert(closer, match_start);
    }
    cache.done = true;
    Some(Found::Text(opener))
}

/// markdown-it 15's `backtick`.
fn scan_15(state: &mut InlineState) -> Option<Found> {
    let start = state.pos;
    if state.src.as_bytes().get(start) != Some(&b'`') {
        return None;
    }
    let max = state.pos_max;
    let (cache, text) = scanned(state);
    let src = text.as_bytes();
    let pos = start + run_len(src, start, max);
    let opener = pos - start;
    if !cache.done {
        // buildLastRuns: where the last run of each length starts.
        let mut p = 0;
        while let Some(k) = text[p..].find('`') {
            let s = p + k;
            p = s + run_len(src, s, src.len());
            cache.runs.insert(p - s, s);
        }
        cache.done = true;
    }
    if cache.runs.get(&opener).is_some_and(|&last| last >= pos) {
        let mut match_end = pos;
        while let Some(k) = text[match_end..].find('`') {
            let match_start = match_end + k;
            if match_start >= max {
                break;
            }
            match_end = match_start + 1 + run_len(src, match_start + 1, src.len());
            if match_end > max {
                break;
            }
            if match_end - match_start == opener {
                let mut content = text[pos..match_start].replace('\n', " ");
                if content.starts_with(' ')
                    && content.ends_with(' ')
                    && content.contains(|c| c != ' ')
                {
                    content = content[1..content.len() - 1].to_string();
                }
                return Some(Found::Code {
                    marker_len: opener,
                    content,
                    end: match_end,
                });
            }
        }
    }
    Some(Found::Text(opener))
}

struct Backticks<const V15: bool>;

impl<const V15: bool> Backticks<V15> {
    fn scan(state: &mut InlineState) -> Option<Found> {
        if V15 { scan_15(state) } else { scan_14(state) }
    }
}

impl<const V15: bool> InlineRule for Backticks<V15> {
    const MARKER: char = '`';

    fn check(state: &mut InlineState) -> Option<usize> {
        let pos = state.pos;
        if let Some(&seen) = state
            .inline_ext
            .get::<Scanned>()
            .and_then(|c| c.memo.get(&pos))
        {
            return seen;
        }
        let found = match Self::scan(state) {
            None => None,
            Some(Found::Code { end, .. }) => Some(end - pos),
            Some(Found::Text(len)) => Some(len),
        };
        state
            .inline_ext
            .get_or_insert_default::<Scanned>()
            .memo
            .insert(pos, found);
        found
    }

    fn run(state: &mut InlineState) -> Option<(Node, usize)> {
        match Self::scan(state)? {
            Found::Code {
                marker_len,
                content,
                end,
            } => {
                let mut node = Node::new(CodeInline {
                    marker: '`',
                    marker_len,
                });
                let inner_start = state.pos + marker_len;
                let mut inner = Node::new(Text { content });
                inner.srcmap = state.get_map(inner_start, end - marker_len);
                node.children.push(inner);
                Some((node, end - state.pos))
            }
            Found::Text(len) => {
                let node = Node::new(Text {
                    content: state.src[state.pos..state.pos + len].to_string(),
                });
                Some((node, len))
            }
        }
    }
}

/// markdown-it's code spans: 14.3.3's when `eleventy`, else 15's.
pub(crate) fn add(md: &mut MarkdownIt, eleventy: bool) {
    if eleventy {
        md.inline.add_rule::<Backticks<false>>();
    } else {
        md.inline.add_rule::<Backticks<true>>();
    }
}
