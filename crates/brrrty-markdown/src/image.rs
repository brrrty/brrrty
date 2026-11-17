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

/// The caches of the state an image label was found in, set aside while the label is parsed
/// with new ones (markdown-it's new state), with where the label starts.
#[derive(Debug)]
struct Outer {
    at: usize,
    caches: markdown_it::parser::extset::InlineRootExtSet,
}
impl InlineRootExt for Outer {}

/// Runs at every position (after the text scanner): at the first rule run in the image label
/// being parsed, swaps in new caches (the port's link-label cache too, so a link in the label is
/// scanned again, as in markdown-it's new state). [`ImageLabel`] swaps them back.
struct FreshLabel;

impl InlineRule for FreshLabel {
    const MARKER: char = '\0';

    fn check(_: &mut InlineState) -> Option<usize> {
        None
    }

    fn run(state: &mut InlineState) -> Option<(Node, usize)> {
        let &at = state
            .inline_ext
            .get::<ImageLabels>()
            .and_then(|l| l.0.last())?;
        if state.inline_ext.get::<Outer>().is_some_and(|o| o.at == at) {
            return None;
        }
        let labels = state.inline_ext.remove::<ImageLabels>();
        let caches = std::mem::take(&mut *state.inline_ext);
        if let Some(l) = labels {
            state.inline_ext.insert(l);
        }
        state.inline_ext.insert(Outer { at, caches });
        None
    }
}

/// Undoes [`FreshLabel`] for the label at `at`.
fn swap_back(state: &mut InlineState, at: usize) {
    if !state.inline_ext.get::<Outer>().is_some_and(|o| o.at == at) {
        return;
    }
    let outer = state.inline_ext.remove::<Outer>().expect("checked above");
    let labels = state.inline_ext.remove::<ImageLabels>();
    *state.inline_ext = outer.caches;
    if let Some(l) = labels {
        state.inline_ext.insert(l);
    }
}

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
        swap_back(state, label);
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
    md.inline.add_rule::<FreshLabel>().before_all();
}
