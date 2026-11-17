//! Image labels as markdown-it parses them: with a new inline state over the label alone
//! (`state.md.inline.parse(label)`), so the label starts at position 0 with link level 0, its
//! own backtick and HTML caches, and nothing after it. The port parses the label in place.
//! This wraps the port's image rule to give the label that state.

use markdown_it::generics::inline::full_link::LinkPrefixScanner;
use markdown_it::parser::extset::InlineRootExt;
use markdown_it::parser::inline::{InlineRule, InlineState};
use markdown_it::{MarkdownIt, Node};

/// Where each image label being parsed starts, innermost last.
#[derive(Debug, Default)]
pub(crate) struct ImageLabels(pub Vec<usize>);
impl InlineRootExt for ImageLabels {}

/// Whether the parser is inside an image's label.
pub(crate) fn in_image(state: &InlineState) -> bool {
    state
        .inline_ext
        .get::<ImageLabels>()
        .is_some_and(|l| !l.0.is_empty())
}

/// Whether `pos` is where the innermost image label being parsed starts.
pub(crate) fn label_starts_at(state: &InlineState, pos: usize) -> bool {
    state
        .inline_ext
        .get::<ImageLabels>()
        .and_then(|l| l.0.last())
        == Some(&pos)
}

/// The end of the text an inline rule may read: the image label's when in one (its `pos_max`),
/// else the whole inline text, as markdown-it reads past `posMax` in a link's label.
pub(crate) fn text_end(state: &InlineState) -> usize {
    if in_image(state) {
        state.pos_max
    } else {
        state.src.len()
    }
}

type Image = LinkPrefixScanner<'!', true>;

struct ImageLabel;

impl InlineRule for ImageLabel {
    const MARKER: char = '!';

    fn check(state: &mut InlineState) -> Option<usize> {
        <Image as InlineRule>::check(state)
    }

    fn run(state: &mut InlineState) -> Option<(Node, usize)> {
        if !state.src[state.pos..state.pos_max].starts_with("![") {
            return None;
        }
        // markdown-it finds the label's end in this state (its caches and all), and only
        // parses the label in a new one.
        <Image as InlineRule>::check(state)?;
        let link_level = state.link_level;
        let backticks = crate::backticks::take(state);
        let html = state.inline_ext.remove::<crate::SkipHtmlSpecial>();
        let html_memo = state.inline_ext.remove::<crate::HtmlMemo>();
        let label = state.pos + 2;
        state
            .inline_ext
            .get_or_insert_default::<ImageLabels>()
            .0
            .push(label);
        // The port adds one for the label.
        state.link_level = -1;
        let found = <Image as InlineRule>::run(state);
        state.link_level = link_level;
        state
            .inline_ext
            .get_or_insert_default::<ImageLabels>()
            .0
            .pop();
        crate::backticks::restore(state, backticks);
        state.inline_ext.remove::<crate::SkipHtmlSpecial>();
        if let Some(h) = html {
            state.inline_ext.insert(h);
        }
        state.inline_ext.remove::<crate::HtmlMemo>();
        if let Some(m) = html_memo {
            state.inline_ext.insert(m);
        }
        found
    }
}

/// Call after the port's image rule is added: this takes its place.
pub(crate) fn add(md: &mut MarkdownIt) {
    md.inline.add_rule::<ImageLabel>().before::<Image>();
    md.inline.remove_rule::<Image>();
}
