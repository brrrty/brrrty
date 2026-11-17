//! Emphasis (`*`, `_`) and strikethrough (`~~`) as markdown-it (14.3.3 and 15) does them.
//!
//! markdown-it tokenizes each marker as a text token and records it in the delimiter list of
//! its scope (the inline text, or a link's or image's label), then pairs them up after the
//! whole text is tokenized (`balance_pairs`), turns the pairs into tags (`strikethrough` and
//! `emphasis` `postProcess`) and joins the text that's left (`fragments_join`). The port matched
//! as it went, with its own bookkeeping, which differed (and decided "can open" from Unicode `P`
//! only, where markdown-it takes `P` or `S`, so an emoji counts). This is markdown-it's
//! pipeline, on each scope's children: a delimiter is a [`Delim`] child, and the list lives on
//! the scope node until [`BalancePairs`] runs.

use std::collections::HashMap;

use markdown_it::common::sourcemap::SourcePos;
use markdown_it::generics::inline::emph_pair::FragmentsJoin;
use markdown_it::parser::core::CoreRule;
use markdown_it::parser::extset::{MarkdownItExt, NodeExt};
use markdown_it::parser::inline::builtin::InlineParserRule;
use markdown_it::parser::inline::{InlineRule, InlineState, Text};
use markdown_it::plugins::cmark::inline::emphasis::{Em, Strong};
use markdown_it::plugins::extra::strikethrough::Strikethrough;
use markdown_it::{MarkdownIt, Node, NodeValue, Renderer};

/// Which `isPunctChar` table the parser uses (its markdown-it version's uc.micro).
#[derive(Debug)]
struct PunctTable(&'static [(u32, u32)]);
impl MarkdownItExt for PunctTable {}

/// A delimiter's text token. [`BalancePairs`] turns each into a tag or plain text.
#[derive(Debug)]
pub(crate) struct Delim {
    pub content: String,
}

impl NodeValue for Delim {
    fn render(&self, _: &Node, fmt: &mut dyn Renderer) {
        fmt.text(&self.content);
    }
}

/// An entry of markdown-it's `state.delimiters`.
#[derive(Debug, Clone)]
struct DelimInfo {
    marker: char,
    length: usize,
    /// The child index of its token in the scope.
    token: usize,
    end: isize,
    open: bool,
    close: bool,
}

/// A scope's delimiter list.
#[derive(Debug, Default)]
struct Delimiters(Vec<DelimInfo>);
impl NodeExt for Delimiters {}

/// markdown-it's emphasis (`*`, `_`) with its punctuation table, and `balance_pairs`,
/// the post-processing and `fragments_join` after the inline parse.
pub(crate) fn add_emphasis(md: &mut MarkdownIt, eleventy: bool) {
    md.ext.insert(PunctTable(if eleventy {
        crate::uc_micro::PUNCT_OR_SYMBOL_11TY
    } else {
        crate::uc_micro::PUNCT_OR_SYMBOL
    }));
    md.inline.add_rule::<EmphasisScanner<'*'>>();
    md.inline.add_rule::<EmphasisScanner<'_'>>();
    if !md.has_rule::<BalancePairs>() {
        md.add_rule::<BalancePairs>().after::<InlineParserRule>();
    }
    if !md.has_rule::<FragmentsJoin>() {
        md.add_rule::<FragmentsJoin>().after::<BalancePairs>();
    }
}

/// markdown-it's strikethrough (`~~`). Call after [`add_emphasis`].
pub(crate) fn add_strikethrough(md: &mut MarkdownIt) {
    md.inline.add_rule::<StrikeScanner>();
}

fn in_table(table: &[(u32, u32)], c: u32) -> bool {
    let i = table.partition_point(|&(_, end)| end < c);
    table.get(i).is_some_and(|&(start, _)| start <= c)
}

/// markdown-it's `isWhiteSpace`, on a code point.
pub(crate) fn is_white_space(c: u32) -> bool {
    (0x2000..=0x200a).contains(&c)
        || matches!(
            c,
            0x09 | 0x0a | 0x0b | 0x0c | 0x0d | 0x20 | 0xa0 | 0x1680 | 0x202f | 0x205f | 0x3000
        )
}

/// markdown-it's `isMdAsciiPunct(c) || isPunctCharCode(c)`.
pub(crate) fn is_punct(table: &[(u32, u32)], c: u32) -> bool {
    matches!(c, 0x21..=0x2f | 0x3a..=0x40 | 0x5b..=0x60 | 0x7b..=0x7e) || in_table(table, c)
}

struct DelimiterRun {
    can_open: bool,
    can_close: bool,
    length: usize,
}

/// markdown-it's `scanDelims` (14.3.3 and 15): the characters on each side as code points.
fn scan_delims(state: &InlineState, start: usize, can_split_word: bool) -> DelimiterRun {
    let table = state
        .md
        .ext
        .get::<PunctTable>()
        .expect("set by add_emphasis")
        .0;
    // An image's label is its own text to markdown-it, starting at 0.
    let last_char = if crate::image::label_starts_at(state, start) {
        0x20
    } else {
        state.src[..start]
            .chars()
            .next_back()
            .map_or(0x20, u32::from)
    };
    let rest = &state.src[start..state.pos_max];
    let marker = rest.chars().next().expect("a marker");
    let length = rest.chars().take_while(|&c| c == marker).count();
    let next_char = rest[length..].chars().next().map_or(0x20, u32::from);
    let is_last_punct = is_punct(table, last_char);
    let is_next_punct = is_punct(table, next_char);
    let is_last_space = is_white_space(last_char);
    let is_next_space = is_white_space(next_char);
    let left_flanking = !is_next_space && (!is_next_punct || is_last_space || is_last_punct);
    let right_flanking = !is_last_space && (!is_last_punct || is_next_space || is_next_punct);
    DelimiterRun {
        can_open: left_flanking && (can_split_word || !right_flanking || is_last_punct),
        can_close: right_flanking && (can_split_word || !left_flanking || is_next_punct),
        length,
    }
}

/// Pushes a delimiter token for the scope, recording it, and returns it unpushed when `last`
/// (the tokenizer pushes the node a rule returns).
fn delim(
    state: &mut InlineState,
    content: &str,
    info: DelimInfo,
    at: usize,
    last: bool,
) -> Option<Node> {
    let mut node = Node::new(Delim {
        content: content.into(),
    });
    node.srcmap = state.get_map(at, at + content.len());
    let token = state.node.children.len();
    state
        .node
        .ext
        .get_or_insert_default::<Delimiters>()
        .0
        .push(DelimInfo { token, ..info });
    if last {
        Some(node)
    } else {
        state.node.children.push(node);
        None
    }
}

/// markdown-it's `emphasis` tokenize: a text token and a delimiter for each marker.
struct EmphasisScanner<const MARKER: char>;

impl<const MARKER: char> InlineRule for EmphasisScanner<MARKER> {
    const MARKER: char = MARKER;

    fn check(_: &mut InlineState) -> Option<usize> {
        None
    }

    fn run(state: &mut InlineState) -> Option<(Node, usize)> {
        if !state.src[state.pos..state.pos_max].starts_with(MARKER) {
            return None;
        }
        let start = state.pos;
        let scanned = scan_delims(state, start, MARKER == '*');
        let info = DelimInfo {
            marker: MARKER,
            length: scanned.length,
            token: 0,
            end: -1,
            open: scanned.can_open,
            close: scanned.can_close,
        };
        let mut last = None;
        let mark = MARKER.to_string();
        for i in 0..scanned.length {
            last = delim(
                state,
                &mark,
                info.clone(),
                start + i,
                i + 1 == scanned.length,
            );
        }
        state.pos = start + scanned.length - 1;
        Some((last.expect("a run has a marker"), 1))
    }
}

/// markdown-it's `strikethrough` tokenize: a lone `~` text token when the run is odd, then a
/// text token and a delimiter (of length 0) for each `~~`.
struct StrikeScanner;

impl InlineRule for StrikeScanner {
    const MARKER: char = '~';

    fn check(_: &mut InlineState) -> Option<usize> {
        None
    }

    fn run(state: &mut InlineState) -> Option<(Node, usize)> {
        if !state.src[state.pos..state.pos_max].starts_with('~') {
            return None;
        }
        let start = state.pos;
        let scanned = scan_delims(state, start, true);
        let len = scanned.length;
        if len < 2 {
            return None;
        }
        let mut at = start;
        if len % 2 == 1 {
            let mut t = Node::new(Text {
                content: "~".into(),
            });
            t.srcmap = state.get_map(start, start + 1);
            state.node.children.push(t);
            at += 1;
        }
        let info = DelimInfo {
            marker: '~',
            length: 0,
            token: 0,
            end: -1,
            open: scanned.can_open,
            close: scanned.can_close,
        };
        let mut last = None;
        let pairs = len / 2;
        for k in 0..pairs {
            last = delim(state, "~~", info.clone(), at + 2 * k, k + 1 == pairs);
        }
        state.pos = start + len - 2;
        Some((last.expect("a run has a pair"), 2))
    }
}

/// markdown-it's `processDelimiters` (`balance_pairs`).
fn process_delimiters(delimiters: &mut [DelimInfo]) {
    let max = delimiters.len();
    if max == 0 {
        return;
    }
    let mut openers_bottom: HashMap<char, [isize; 6]> = HashMap::new();
    let mut header_idx = 0usize;
    let mut last_token_idx: isize = -2;
    let mut jumps: Vec<isize> = Vec::with_capacity(max);
    for closer_idx in 0..max {
        jumps.push(0);
        let (marker, token) = (
            delimiters[closer_idx].marker,
            delimiters[closer_idx].token as isize,
        );
        if delimiters[header_idx].marker != marker || last_token_idx != token - 1 {
            header_idx = closer_idx;
        }
        last_token_idx = token;
        if !delimiters[closer_idx].close {
            continue;
        }
        let bottoms = openers_bottom.entry(marker).or_insert([-1; 6]);
        let closer = &delimiters[closer_idx];
        let slot = if closer.open { 3 } else { 0 } + closer.length % 3;
        let min_opener_idx = bottoms[slot];
        let mut opener_idx = header_idx as isize - jumps[header_idx] - 1;
        let mut new_min_opener_idx = opener_idx;
        while opener_idx > min_opener_idx {
            let o = opener_idx as usize;
            if delimiters[o].marker == marker && delimiters[o].open && delimiters[o].end < 0 {
                let (opener, closer) = (&delimiters[o], &delimiters[closer_idx]);
                let odd = (opener.close || closer.open)
                    && (opener.length + closer.length).is_multiple_of(3)
                    && (!opener.length.is_multiple_of(3) || !closer.length.is_multiple_of(3));
                if !odd {
                    let last_jump = if o > 0 && !delimiters[o - 1].open {
                        jumps[o - 1] + 1
                    } else {
                        0
                    };
                    jumps[closer_idx] = closer_idx as isize - opener_idx + last_jump;
                    jumps[o] = last_jump;
                    delimiters[closer_idx].open = false;
                    delimiters[o].end = closer_idx as isize;
                    delimiters[o].close = false;
                    new_min_opener_idx = -1;
                    last_token_idx = -2;
                    break;
                }
            }
            opener_idx -= jumps[o] + 1;
        }
        if new_min_opener_idx != -1 {
            let closer = &delimiters[closer_idx];
            let slot = if closer.open { 3 } else { 0 } + closer.length % 3;
            openers_bottom.get_mut(&marker).expect("inserted above")[slot] = new_min_opener_idx;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    Em(char),
    Strong(char),
    Strike,
}

/// A token of a scope after post-processing.
enum Item {
    Node(Node),
    Open(Tag, Option<SourcePos>),
    Close(Tag, Option<SourcePos>),
}

impl Item {
    fn srcmap(&self) -> Option<SourcePos> {
        match self {
            Item::Node(n) => n.srcmap,
            Item::Open(_, m) | Item::Close(_, m) => *m,
        }
    }

    fn is_lone_tilde(&self) -> bool {
        matches!(self, Item::Node(n) if n.cast::<Text>().is_some_and(|t| t.content == "~"))
    }
}

/// `balance_pairs`, strikethrough and emphasis `postProcess` on one scope's children.
fn process_scope(node: &mut Node, mut delimiters: Vec<DelimInfo>) {
    process_delimiters(&mut delimiters);
    let mut items: Vec<Item> = std::mem::take(&mut node.children)
        .into_iter()
        .map(Item::Node)
        .collect();
    let to_open = |items: &mut Vec<Item>, i: usize, tag: Tag| {
        let m = items[i].srcmap();
        items[i] = Item::Open(tag, m);
    };
    let to_close = |items: &mut Vec<Item>, i: usize, tag: Tag| {
        let m = items[i].srcmap();
        items[i] = Item::Close(tag, m);
    };
    // strikethrough postProcess.
    let mut lone = Vec::new();
    for d in &delimiters {
        if d.marker != '~' || d.end == -1 {
            continue;
        }
        let end = &delimiters[d.end as usize];
        to_open(&mut items, d.token, Tag::Strike);
        to_close(&mut items, end.token, Tag::Strike);
        if end.token > 0 && items[end.token - 1].is_lone_tilde() {
            lone.push(end.token - 1);
        }
    }
    while let Some(i) = lone.pop() {
        let mut j = i + 1;
        while j < items.len() && matches!(items[j], Item::Close(Tag::Strike, _)) {
            j += 1;
        }
        j -= 1;
        if i != j {
            items.swap(i, j);
        }
    }
    // emphasis postProcess.
    let mut i = delimiters.len() as isize - 1;
    while i >= 0 {
        let start = &delimiters[i as usize];
        if (start.marker != '_' && start.marker != '*') || start.end == -1 {
            i -= 1;
            continue;
        }
        let end_idx = start.end as usize;
        let end = &delimiters[end_idx];
        let is_strong = i > 0 && {
            let prev = &delimiters[i as usize - 1];
            prev.end == start.end + 1
                && prev.marker == start.marker
                && prev.token + 1 == start.token
                && delimiters[end_idx + 1].token == end.token + 1
        };
        let tag = if is_strong {
            Tag::Strong(start.marker)
        } else {
            Tag::Em(start.marker)
        };
        let (open_tok, close_tok) = (start.token, end.token);
        to_open(&mut items, open_tok, tag);
        to_close(&mut items, close_tok, tag);
        if is_strong {
            for t in [
                delimiters[i as usize - 1].token,
                delimiters[end_idx + 1].token,
            ] {
                if let Item::Node(n) = &mut items[t]
                    && let Some(d) = n.cast_mut::<Delim>()
                {
                    d.content.clear();
                }
            }
            i -= 1;
        }
        i -= 1;
    }
    // Back to a tree.
    let mut stack: Vec<(Tag, Option<SourcePos>, Vec<Node>)> = Vec::new();
    let mut top: Vec<Node> = Vec::new();
    for item in items {
        match item {
            Item::Node(mut n) => {
                if let Some(d) = n.cast::<Delim>() {
                    let content = d.content.clone();
                    n.replace(Text { content });
                }
                match stack.last_mut() {
                    Some(f) => f.2.push(n),
                    None => top.push(n),
                }
            }
            Item::Open(tag, m) => stack.push((tag, m, Vec::new())),
            Item::Close(_, m) => {
                let Some((tag, start, children)) = stack.pop() else {
                    continue;
                };
                let mut n = match tag {
                    Tag::Em(marker) => Node::new(Em { marker }),
                    Tag::Strong(marker) => Node::new(Strong { marker }),
                    Tag::Strike => Node::new(Strikethrough { marker: '~' }),
                };
                n.children = children;
                n.srcmap = match (start, m) {
                    (Some(a), Some(b)) => Some(SourcePos::new(
                        a.get_byte_offsets().0,
                        b.get_byte_offsets().1,
                    )),
                    _ => None,
                };
                match stack.last_mut() {
                    Some(f) => f.2.push(n),
                    None => top.push(n),
                }
            }
        }
    }
    // An open tag left unclosed can't happen; keep its contents if it does.
    while let Some((_, _, children)) = stack.pop() {
        match stack.last_mut() {
            Some(f) => f.2.extend(children),
            None => top.extend(children),
        }
    }
    node.children = top;
}

/// Runs markdown-it's delimiter post-processing on every scope.
struct BalancePairs;

impl CoreRule for BalancePairs {
    fn run(root: &mut Node, _: &MarkdownIt) {
        root.walk_mut(|n, _| {
            if let Some(Delimiters(d)) = n.ext.remove::<Delimiters>() {
                process_scope(n, d);
            }
        });
    }
}
