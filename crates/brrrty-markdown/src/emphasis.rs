//! Emphasis (`*`, `_`, `**`, `__`) and strikethrough (`~~`), from the port's `emph_pair` (the
//! `markdown-it` crate 0.6.1, MIT, Copyright (c) 2022 Alex Kocharin) with markdown-it's
//! `scanDelims` in place of the port's.
//!
//! The port decides whether a run of markers can open or close from Unicode punctuation (`P`)
//! and Rust's whitespace. markdown-it takes punctuation *or symbols* (uc.micro `P` or `S`, so an
//! emoji is one) and its own whitespace list. The rest of the matching is the port's, unchanged.

use std::cmp::min;

use markdown_it::common::sourcemap::SourcePos;
use markdown_it::generics::inline::emph_pair::{EmphMarker, FragmentsJoin};
use markdown_it::parser::extset::{MarkdownItExt, NodeExt};
use markdown_it::parser::inline::builtin::InlineParserRule;
use markdown_it::parser::inline::{InlineRule, InlineState};
use markdown_it::plugins::cmark::inline::emphasis::{Em, Strong};
use markdown_it::plugins::extra::strikethrough::Strikethrough;
use markdown_it::{MarkdownIt, Node};

/// Which `isPunctChar` table the parser uses (its markdown-it version's uc.micro).
#[derive(Debug)]
struct PunctTable(&'static [(u32, u32)]);
impl MarkdownItExt for PunctTable {}

#[derive(Debug, Default)]
struct PairConfig<const MARKER: char> {
    inserted: bool,
    fns: [Option<fn() -> Node>; 3],
}
impl<const MARKER: char> MarkdownItExt for PairConfig<MARKER> {}

#[derive(Debug, Default)]
struct OpenersBottom<const MARKER: char>([usize; 6]);
impl<const MARKER: char> NodeExt for OpenersBottom<MARKER> {}

/// markdown-it 15's emphasis (`*`, `_`) with its punctuation table.
pub(crate) fn add_emphasis(md: &mut MarkdownIt, eleventy: bool) {
    md.ext.insert(PunctTable(if eleventy {
        crate::uc_micro::PUNCT_OR_SYMBOL_11TY
    } else {
        crate::uc_micro::PUNCT_OR_SYMBOL
    }));
    add_with::<'*', 1, true>(md, || Node::new(Em { marker: '*' }));
    add_with::<'_', 1, false>(md, || Node::new(Em { marker: '_' }));
    add_with::<'*', 2, true>(md, || Node::new(Strong { marker: '*' }));
    add_with::<'_', 2, false>(md, || Node::new(Strong { marker: '_' }));
}

/// markdown-it's strikethrough (`~~`). Call after [`add_emphasis`].
pub(crate) fn add_strikethrough(md: &mut MarkdownIt) {
    add_with::<'~', 2, true>(md, || Node::new(Strikethrough { marker: '~' }));
}

fn add_with<const MARKER: char, const LENGTH: u8, const CAN_SPLIT_WORD: bool>(
    md: &mut MarkdownIt,
    f: fn() -> Node,
) {
    let pair_config = md.ext.get_or_insert_default::<PairConfig<MARKER>>();
    pair_config.fns[LENGTH as usize - 1] = Some(f);
    if !pair_config.inserted {
        pair_config.inserted = true;
        md.inline
            .add_rule::<EmphPairScanner<MARKER, CAN_SPLIT_WORD>>();
    }
    if !md.has_rule::<FragmentsJoin>() {
        md.add_rule::<FragmentsJoin>()
            .before_all()
            .after::<InlineParserRule>();
    }
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
    let last_char = state.src[..start]
        .chars()
        .next_back()
        .map_or(0x20, u32::from);
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

struct EmphPairScanner<const MARKER: char, const CAN_SPLIT_WORD: bool>;

impl<const MARKER: char, const CAN_SPLIT_WORD: bool> InlineRule
    for EmphPairScanner<MARKER, CAN_SPLIT_WORD>
{
    const MARKER: char = MARKER;

    // This rule works on a closing marker, so any rule trying to skip it should see plain text.
    fn check(_: &mut InlineState) -> Option<usize> {
        None
    }

    fn run(state: &mut InlineState) -> Option<(Node, usize)> {
        if !state.src[state.pos..state.pos_max].starts_with(MARKER) {
            return None;
        }
        let scanned = scan_delims(state, state.pos, CAN_SPLIT_WORD);
        let mut node = Node::new(EmphMarker {
            marker: MARKER,
            length: scanned.length,
            remaining: scanned.length,
            open: scanned.can_open,
            close: scanned.can_close,
        });
        node.srcmap = state.get_map(state.pos, state.pos + scanned.length);
        node = scan_and_match_delimiters::<MARKER>(state, node);
        let map = node.srcmap.expect("set above").get_byte_offsets();
        // Back up to keep the source maps right.
        state.pos += scanned.length;
        let token_len = map.1 - map.0;
        state.pos -= token_len;
        Some((node, token_len))
    }
}

/// With a closing delimiter just inserted last, finds its openers and moves what is between
/// into an emphasis node.
fn scan_and_match_delimiters<const MARKER: char>(
    state: &mut InlineState,
    mut closer_token: Node,
) -> Node {
    if state.node.children.is_empty() {
        return closer_token;
    }
    let mut closer = closer_token
        .cast_mut::<EmphMarker>()
        .expect("an EmphMarker")
        .clone();
    if !closer.close {
        return closer_token;
    }
    // Lower bounds from earlier failures, per delimiter length modulo 3 and whether this
    // closer can open.
    let openers_for_marker = state
        .node
        .ext
        .get_or_insert_default::<OpenersBottom<MARKER>>();
    let openers_parameter = (closer.open as usize) * 3 + closer.length % 3;
    let min_opener_idx = openers_for_marker.0[openers_parameter];
    let mut idx = state.node.children.len() - 1;
    let mut new_min_opener_idx = idx;
    while idx > min_opener_idx {
        idx -= 1;
        let Some(opener) = state.node.children[idx].cast::<EmphMarker>() else {
            continue;
        };
        let mut opener = opener.clone();
        if opener.open && opener.marker == closer.marker && !is_odd_match(&opener, &closer) {
            while closer.remaining > 0 && opener.remaining > 0 {
                let max_marker_len = min(3, min(opener.remaining, closer.remaining));
                let fns = &state
                    .md
                    .ext
                    .get::<PairConfig<MARKER>>()
                    .expect("set by add_with")
                    .fns;
                let matched = (1..=max_marker_len)
                    .rev()
                    .find_map(|len| fns[len - 1].map(|f| (len, f)));
                // A function only for a longer marker than there is: no match.
                let Some((marker_len, marker_fn)) = matched else {
                    break;
                };
                closer.remaining -= marker_len;
                opener.remaining -= marker_len;
                let mut new_token = marker_fn();
                new_token.children = state.node.children.split_off(idx + 1);
                let mut end_map_pos = 0;
                if let Some(map) = closer_token.srcmap {
                    let (start, end) = map.get_byte_offsets();
                    closer_token.srcmap = Some(SourcePos::new(start + marker_len, end));
                    end_map_pos = start + marker_len;
                }
                let mut start_map_pos = 0;
                let opener_token = state.node.children.last_mut().expect("the opener");
                if let Some(map) = opener_token.srcmap {
                    let (start, end) = map.get_byte_offsets();
                    opener_token.srcmap = Some(SourcePos::new(start, end - marker_len));
                    start_map_pos = end - marker_len;
                }
                new_token.srcmap = state.get_map(start_map_pos, end_map_pos);
                if opener.remaining == 0 {
                    state.node.children.pop();
                }
                new_min_opener_idx = 0;
                state.node.children.push(new_token);
            }
        }
        if opener.remaining > 0 {
            state.node.children[idx].replace(opener);
        }
    }
    if new_min_opener_idx != 0 {
        // Keeps the search linear: https://github.com/commonmark/cmark/issues/178
        let openers_for_marker = state
            .node
            .ext
            .get_or_insert_default::<OpenersBottom<MARKER>>();
        openers_for_marker.0[openers_parameter] = new_min_opener_idx;
    }
    if closer.remaining > 0 {
        closer_token.replace(closer);
        closer_token
    } else {
        state.node.children.pop().expect("the closer")
    }
}

/// The spec's rule of three: when either delimiter can both open and close, the lengths can't
/// add up to a multiple of 3 unless both are multiples of 3.
fn is_odd_match(opener: &EmphMarker, closer: &EmphMarker) -> bool {
    (opener.close || closer.open)
        && (opener.length + closer.length).is_multiple_of(3)
        && (!opener.length.is_multiple_of(3) || !closer.length.is_multiple_of(3))
}
