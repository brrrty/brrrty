//! markdown-it 15's `linkify` (core and inline), `replacements` and `smartquotes` rules, on
//! the port's tree.
//!
//! markdown-it's core rules read and change the flat token stream of each inline block: they
//! look at token levels, tell `text` from `text_special`, and read the neighbors' `content`
//! across emphasis, links and code. [`flatten`] gives that stream for the port's tree (a token
//! for each node, an open and a close token around a node with children), the rules run on it
//! as markdown-it's do, line by line, and the changes are written back to the nodes.

use markdown_it::parser::core::{CoreRule, Root};
use markdown_it::parser::extset::{MarkdownItExt, NodeExt, RootExt};
use markdown_it::parser::inline::builtin::InlineParserRule;
use markdown_it::parser::inline::{InlineRoot, InlineRule, InlineState, Text, TextSpecial};
use markdown_it::parser::linkfmt::LinkFormatter;
use markdown_it::plugins::cmark::inline::autolink::Autolink;
use markdown_it::plugins::cmark::inline::backticks::CodeInline;
use markdown_it::plugins::cmark::inline::emphasis::{Em, Strong};
use markdown_it::plugins::cmark::inline::image::Image;
use markdown_it::plugins::cmark::inline::link::Link;
use markdown_it::plugins::cmark::inline::newline::{Hardbreak, Softbreak};
use markdown_it::plugins::extra::strikethrough::Strikethrough;
use markdown_it::plugins::html::html_inline::HtmlInline;
use markdown_it::{MarkdownIt, Node, NodeValue, Renderer};

use crate::linkify::Linkify;
use crate::uc_micro;

/// The source of an inline block (markdown-it's `inline` token `content`), kept on its
/// [`InlineGroup`].
#[derive(Debug)]
pub(crate) struct InlineSource(pub String);
impl NodeExt for InlineSource {}

/// markdown-it's `inline` token: one inline block's nodes, written as they are. The port puts a
/// tight list item's paragraphs' inline nodes straight into the item, next to its blocks, so
/// each inline block is wrapped to keep it apart (for the delimiter scopes, linkify and the
/// typographer).
#[derive(Debug)]
pub(crate) struct InlineGroup;

impl NodeValue for InlineGroup {
    fn render(&self, node: &Node, fmt: &mut dyn Renderer) {
        fmt.contents(&node.children);
    }
}

/// Wraps each inline block in an [`InlineGroup`] that keeps its source.
struct WrapInline;

impl CoreRule for WrapInline {
    fn run(root: &mut Node, _: &MarkdownIt) {
        root.walk_mut(|n, _| {
            if n.is::<InlineGroup>() {
                return;
            }
            for child in n.children.iter_mut() {
                let Some(source) = child.cast::<InlineRoot>().map(|r| r.content.clone()) else {
                    continue;
                };
                let inline = std::mem::replace(child, Node::new(InlineGroup));
                // Where the inline text is in the source, from its line mapping.
                child.srcmap = inline.srcmap.or_else(|| {
                    let root = inline.cast::<InlineRoot>()?;
                    let &(_, first) = root.mapping.first()?;
                    let &(dst, src) = root.mapping.last()?;
                    let end = src + root.content.len().saturating_sub(dst);
                    Some(markdown_it::common::sourcemap::SourcePos::new(first, end))
                });
                child.ext.insert(InlineSource(source));
                child.children.push(inline);
            }
        });
    }
}

/// Wraps inline blocks (see [`InlineGroup`]); every parser has it.
pub(crate) fn add_wrap(md: &mut MarkdownIt) {
    md.add_rule::<WrapInline>().before::<InlineParserRule>();
}

/// The linkifier the inline `linkify` rule uses.
#[derive(Debug)]
struct LinkifyConfig(Linkify);
impl MarkdownItExt for LinkifyConfig {}

/// The first thing the inline `linkify` rule refused, if any.
#[derive(Debug)]
struct Refused(String);
impl RootExt for Refused {}

/// Sets `md` up for [`run`]: keeps inline sources, and with `linkify` adds markdown-it's inline
/// `linkify` rule.
pub(crate) fn add(md: &mut MarkdownIt, linkify: Option<Linkify>) {
    if let Some(l) = linkify {
        md.ext.insert(LinkifyConfig(l));
        // markdown-it's `linkify` comes right after `text`, before every other inline rule.
        md.inline.add_rule::<LinkifyInline>().before_all();
    }
}

/// A link markdown-it's linkify made: `link_open` with `info: 'auto'` and only an `href`.
#[derive(Debug)]
pub(crate) struct LinkifyLink {
    pub url: String,
}

impl NodeValue for LinkifyLink {
    fn render(&self, node: &Node, fmt: &mut dyn Renderer) {
        let mut attrs = node.attrs.clone();
        attrs.insert(0, ("href", self.url.clone()));
        fmt.open("a", &attrs);
        fmt.contents(&node.children);
        fmt.close("a");
    }
}

fn link_node(url: String, text: String) -> Node {
    let mut node = Node::new(LinkifyLink { url });
    node.children.push(Node::new(Text { content: text }));
    node
}

fn is_scheme_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.'
}

/// markdown-it's inline `linkify` rule: a link with a schema, found where its `://` is.
struct LinkifyInline;

/// What [`LinkifyInline`] takes at `state.pos`: the URL (from the schema on), how many bytes of
/// schema are before `pos`, and the normalized URL.
fn scan_linkify(state: &mut InlineState) -> Option<(String, usize, String)> {
    if state.link_level > 0 {
        return None;
    }
    let pos = state.pos;
    if !state.src[pos..state.pos_max].starts_with("://") {
        return None;
    }
    let pending = state.trailing_text_get().encode_utf16().count();
    let limit = 10.min(pending);
    let bytes = state.src.as_bytes();
    let mut k = 0;
    while k < limit && k < pos && is_scheme_char(bytes[pos - 1 - k]) {
        k += 1;
    }
    if k == 0 || !bytes[pos - k].is_ascii_alphabetic() {
        return None;
    }
    let linkify = state.md.ext.get::<LinkifyConfig>()?.0;
    let units: Vec<u16> = state.src[pos - k..crate::image::text_end(state)]
        .encode_utf16()
        .collect();
    let link = match linkify.match_at_start(&units) {
        Ok(link) => link?,
        Err(e) => {
            if state.root_ext.get::<Refused>().is_none() {
                state.root_ext.insert(Refused(e));
            }
            return None;
        }
    };
    if link.url.encode_utf16().count() <= k {
        return None;
    }
    let url = link.url.trim_end_matches('*').to_string();
    let full = state.md.link_formatter.normalize_link(&url);
    state.md.link_formatter.validate_link(&full)?;
    Some((url, k, full))
}

impl InlineRule for LinkifyInline {
    const MARKER: char = ':';

    fn check(state: &mut InlineState) -> Option<usize> {
        scan_linkify(state).map(|(url, k, _)| url.len() - k)
    }

    fn run(state: &mut InlineState) -> Option<(Node, usize)> {
        let (url, k, full) = scan_linkify(state)?;
        let text = state.md.link_formatter.normalize_link_text(&url);
        state.trailing_text_pop(k);
        state.pos -= k;
        let mut node = link_node(full, text);
        node.srcmap = state.get_map(state.pos, state.pos + url.len());
        Some((node, url.len()))
    }
}

/// The options [`run`] follows.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rules {
    pub linkify: Option<Linkify>,
    pub typographer: bool,
}

/// markdown-it's core `linkify`, `replacements` and `smartquotes` on every inline block of
/// `ast` (parsed by a parser set up with [`add`]); `source` is what was parsed.
pub(crate) fn run(
    ast: &mut Node,
    md: &MarkdownIt,
    source: &str,
    rules: Rules,
) -> Result<(), String> {
    if let Some(Refused(e)) = ast.cast::<Root>().and_then(|r| r.ext.get::<Refused>()) {
        return Err(e.clone());
    }
    let mut failed = None;
    ast.walk_mut(|block, _| {
        if failed.is_some() {
            return;
        }
        let Some(content) = block.ext.get::<InlineSource>().map(|s| s.0.clone()) else {
            return;
        };
        if let Err(e) = run_block(block, &content, md, source, rules) {
            failed = Some(e);
        }
    });
    failed.map_or(Ok(()), Err)
}

fn run_block(
    block: &mut Node,
    content: &str,
    md: &MarkdownIt,
    source: &str,
    rules: Rules,
) -> Result<(), String> {
    if let Some(linkify) = rules.linkify {
        let units: Vec<u16> = content.encode_utf16().collect();
        if linkify.test(&units)? {
            let tokens = flatten(block, source)?;
            core_linkify(block, &tokens, &linkify, md.link_formatter.as_ref())?;
        }
    }
    if rules.typographer {
        let mut tokens = flatten(block, source)?;
        if scoped_abbr_test(content) {
            replace_scoped(&mut tokens);
        }
        if rare_test(content) {
            replace_rare(&mut tokens);
        }
        if content.contains(['\'', '"']) {
            smartquotes(&mut tokens);
        }
        for t in &tokens {
            if t.changed {
                set_content(node_mut(block, &t.path), t.content.clone());
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    TextSpecial,
    LinkOpen,
    LinkClose,
    Open,
    Close,
    Html,
    Code,
    Image,
    Softbreak,
    Hardbreak,
}

/// A token of markdown-it's inline stream.
#[derive(Debug, Clone)]
struct Tok {
    kind: Kind,
    /// `info === 'auto'` on a link: an autolink, or one linkify made.
    auto: bool,
    level: usize,
    content: String,
    /// Child indexes from the block to the node.
    path: Vec<usize>,
    changed: bool,
}

fn node_mut<'a>(block: &'a mut Node, path: &[usize]) -> &'a mut Node {
    path.iter().fold(block, |n, &i| &mut n.children[i])
}

fn set_content(node: &mut Node, content: String) {
    if let Some(t) = node.cast_mut::<Text>() {
        t.content = content;
    } else if let Some(t) = node.cast_mut::<TextSpecial>() {
        t.content = content;
    }
}

/// markdown-it's flat token stream for the inline nodes under `block`.
fn flatten(block: &Node, source: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let mut path = Vec::new();
    walk(&block.children, 0, &mut path, source, &mut out)?;
    Ok(out)
}

fn walk(
    children: &[Node],
    level: usize,
    path: &mut Vec<usize>,
    source: &str,
    out: &mut Vec<Tok>,
) -> Result<(), String> {
    for (i, n) in children.iter().enumerate() {
        path.push(i);
        let tok = |kind: Kind, content: String| Tok {
            kind,
            auto: false,
            level,
            content,
            path: path.clone(),
            changed: false,
        };
        let pair = |open: Kind, close: Kind, auto: bool| {
            (
                Tok {
                    auto,
                    ..tok(open, String::new())
                },
                Tok {
                    auto,
                    ..tok(close, String::new())
                },
            )
        };
        let wrap = if n.is::<Em>() || n.is::<Strong>() || n.is::<Strikethrough>() {
            Some(pair(Kind::Open, Kind::Close, false))
        } else if n.is::<Link>() {
            Some(pair(Kind::LinkOpen, Kind::LinkClose, false))
        } else if n.is::<Autolink>() || n.is::<LinkifyLink>() {
            Some(pair(Kind::LinkOpen, Kind::LinkClose, true))
        } else {
            None
        };
        if let Some((open, close)) = wrap {
            out.push(open);
            walk(&n.children, level + 1, path, source, out)?;
            out.push(close);
        } else if let Some(t) = n.cast::<Text>() {
            out.push(tok(Kind::Text, t.content.clone()));
        } else if let Some(t) = n.cast::<TextSpecial>() {
            // An autolink's text is a `text` token in markdown-it.
            let kind = if t.info == "autolink" {
                Kind::Text
            } else {
                Kind::TextSpecial
            };
            out.push(tok(kind, t.content.clone()));
        } else if let Some(h) = n.cast::<HtmlInline>() {
            out.push(tok(Kind::Html, h.content.clone()));
        } else if n.is::<CodeInline>() {
            let code: String = n
                .children
                .iter()
                .filter_map(|c| c.cast::<Text>().map(|t| t.content.as_str()))
                .collect();
            out.push(tok(Kind::Code, code));
        } else if n.is::<Image>() {
            out.push(tok(Kind::Image, image_label_ends(n, source)?));
        } else if n.is::<Softbreak>() {
            out.push(tok(Kind::Softbreak, String::new()));
        } else if n.is::<Hardbreak>() {
            out.push(tok(Kind::Hardbreak, String::new()));
        } else {
            return Err(format!(
                "typographer: an inline node Brrrty doesn't know ({})",
                n.name()
            ));
        }
        path.pop();
    }
    Ok(())
}

/// The first and last characters of an image's label as written (markdown-it's `content` for an
/// `image` token), which is all that smart quotes read of it; empty for an empty label. They
/// come from where the label's first and last nodes are in the source.
fn image_label_ends(image: &Node, source: &str) -> Result<String, String> {
    let refuse = || "typographer: can't tell where an image's label is in the source".to_string();
    let (Some(first), Some(last)) = (image.children.first(), image.children.last()) else {
        let start = image.srcmap.ok_or_else(refuse)?.get_byte_offsets().0;
        return if source.get(start..).is_some_and(|s| s.starts_with("![]")) {
            Ok(String::new())
        } else {
            Err(refuse())
        };
    };
    let start = first.srcmap.ok_or_else(refuse)?.get_byte_offsets().0;
    let end = last.srcmap.ok_or_else(refuse)?.get_byte_offsets().1;
    let label = source.get(start..end).ok_or_else(refuse)?;
    let mut chars = label.chars();
    let a = chars.next().ok_or_else(refuse)?;
    Ok(match chars.next_back() {
        Some(b) => format!("{a}{b}"),
        None => a.to_string(),
    })
}

fn units_string(units: &[u16]) -> Result<String, String> {
    String::from_utf16(units)
        .map_err(|_| "linkify: a link would split a character outside the BMP in two".into())
}

/// markdown-it's core `linkify` on one inline block.
fn core_linkify(
    block: &mut Node,
    tokens: &[Tok],
    linkify: &Linkify,
    fmt: &dyn LinkFormatter,
) -> Result<(), String> {
    let mut replacements: Vec<(Vec<usize>, Vec<Node>)> = Vec::new();
    let mut html_link_level = 0i64;
    let mut i = tokens.len() as isize - 1;
    while i >= 0 {
        let current = &tokens[i as usize];
        if current.kind == Kind::LinkClose {
            i -= 1;
            while tokens[i as usize].level != current.level
                && tokens[i as usize].kind != Kind::LinkOpen
            {
                i -= 1;
            }
            i -= 1;
            continue;
        }
        if current.kind == Kind::Html {
            if crate::is_link_open(&current.content) && html_link_level > 0 {
                html_link_level -= 1;
            }
            if crate::is_link_close(&current.content) {
                html_link_level += 1;
            }
        }
        if html_link_level > 0 {
            i -= 1;
            continue;
        }
        let text: Vec<u16> = current.content.encode_utf16().collect();
        if current.kind == Kind::Text && linkify.test(&text)? {
            let mut links = linkify.find_all(&text)?;
            if links.is_empty() {
                return Err(
                    "linkify: test found a link that match didn't (markdown-it throws)".into(),
                );
            }
            if links[0].index == 0 && i > 0 && tokens[i as usize - 1].kind == Kind::TextSpecial {
                links.remove(0);
            }
            let mut nodes = Vec::new();
            let mut last = 0;
            for link in &links {
                let full = fmt.normalize_link(&link.url);
                if fmt.validate_link(&full).is_none() {
                    continue;
                }
                let url_text = if link.schema.is_empty() {
                    let t = fmt.normalize_link_text(&format!("http://{}", link.text));
                    t.strip_prefix("http://").map(str::to_string).unwrap_or(t)
                } else if link.schema == "mailto:"
                    && !link
                        .text
                        .get(..7)
                        .is_some_and(|s| s.eq_ignore_ascii_case("mailto:"))
                {
                    let t = fmt.normalize_link_text(&format!("mailto:{}", link.text));
                    t.strip_prefix("mailto:").map(str::to_string).unwrap_or(t)
                } else {
                    fmt.normalize_link_text(&link.text)
                };
                if link.index > last {
                    nodes.push(Node::new(Text {
                        content: units_string(&text[last..link.index])?,
                    }));
                }
                nodes.push(link_node(full, url_text));
                last = link.last_index;
            }
            if last < text.len() {
                nodes.push(Node::new(Text {
                    content: units_string(&text[last..])?,
                }));
            }
            replacements.push((current.path.clone(), nodes));
        }
        i -= 1;
    }
    // In reverse document order, so each path still points where it did.
    for (path, nodes) in replacements {
        let (index, parent) = path.split_last().expect("a token has a path");
        let parent = node_mut(block, parent);
        parent.children.splice(*index..=*index, nodes);
    }
    Ok(())
}

/// `/\((c|tm|r)\)/i`.
fn scoped_abbr_test(s: &str) -> bool {
    let b = s.as_bytes();
    (0..b.len()).any(|i| scoped_abbr_at(b, i).is_some())
}

/// The symbol and length of `(c)`, `(tm)` or `(r)` (any case) at `i`.
fn scoped_abbr_at(b: &[u8], i: usize) -> Option<(&'static str, usize)> {
    if b.get(i) != Some(&b'(') {
        return None;
    }
    let lower = |j: usize| b.get(i + j).map(u8::to_ascii_lowercase);
    match (lower(1), lower(2), lower(3)) {
        (Some(b'c'), Some(b')'), _) => Some(("©", 3)),
        (Some(b'r'), Some(b')'), _) => Some(("®", 3)),
        (Some(b't'), Some(b'm'), Some(b')')) => Some(("™", 4)),
        _ => None,
    }
}

/// `RARE_RE`: `/\+-|\.\.|\?\?\?\?|!!!!|,,|--/`.
fn rare_test(s: &str) -> bool {
    ["+-", "..", "????", "!!!!", ",,", "--"]
        .iter()
        .any(|p| s.contains(p))
}

/// `replace_scoped`: `(c)`, `(tm)` and `(r)` in text outside autolinks.
fn replace_scoped(tokens: &mut [Tok]) {
    let mut inside_autolink = 0i64;
    for t in tokens.iter_mut().rev() {
        if t.kind == Kind::Text && inside_autolink == 0 {
            let b = t.content.as_bytes();
            let mut out = String::with_capacity(t.content.len());
            let mut i = 0;
            let mut from = 0;
            while i < b.len() {
                if let Some((sym, len)) = scoped_abbr_at(b, i) {
                    out.push_str(&t.content[from..i]);
                    out.push_str(sym);
                    i += len;
                    from = i;
                } else {
                    i += 1;
                }
            }
            if from > 0 {
                out.push_str(&t.content[from..]);
                t.content = out;
                t.changed = true;
            }
        }
        if t.kind == Kind::LinkOpen && t.auto {
            inside_autolink -= 1;
        }
        if t.kind == Kind::LinkClose && t.auto {
            inside_autolink += 1;
        }
    }
}

/// `replace_rare` in text outside autolinks.
fn replace_rare(tokens: &mut [Tok]) {
    let mut inside_autolink = 0i64;
    for t in tokens.iter_mut().rev() {
        if t.kind == Kind::Text && inside_autolink == 0 && rare_test(&t.content) {
            let s = rare(&t.content);
            if s != t.content {
                t.content = s;
                t.changed = true;
            }
        }
        if t.kind == Kind::LinkOpen && t.auto {
            inside_autolink -= 1;
        }
        if t.kind == Kind::LinkClose && t.auto {
            inside_autolink += 1;
        }
    }
}

/// JavaScript's line terminators (`^` and `$` with the `m` flag).
fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `replace_rare`'s chain of `replace` calls on one text.
fn rare(s: &str) -> String {
    let s = s.replace("+-", "±");
    // /\.{2,}/g
    let s = replace_runs(&s, |c| c == '.', 2, |_, _| "…".to_string());
    // /([?!])…/g
    let s = {
        let c: Vec<char> = s.chars().collect();
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        while i < c.len() {
            if matches!(c[i], '?' | '!') && c.get(i + 1) == Some(&'…') {
                out.push(c[i]);
                out.push_str("..");
                i += 2;
            } else {
                out.push(c[i]);
                i += 1;
            }
        }
        out
    };
    // /([?!]){4,}/g: the last of the run, three times.
    let s = replace_runs(
        &s,
        |c| matches!(c, '?' | '!'),
        4,
        |run, _| {
            let last = *run.last().expect("a run");
            [last; 3].iter().collect()
        },
    );
    // /,{2,}/g
    let s = replace_runs(&s, |c| c == ',', 2, |_, _| ",".to_string());
    // /(^|[^-])---(?=[^-]|$)/gm
    let s = replace_dashes(
        &s,
        3,
        "—",
        |c| c != '-',
        |next| next.is_none_or(|c| c != '-'),
    );
    // /(^|\s)--(?=\s|$)/gm
    let s = replace_dashes(&s, 2, "–", crate::js_space, |next| {
        next.is_none_or(crate::js_space)
    });
    // /(^|[^-\s])--(?=[^-\s]|$)/gm
    replace_dashes(
        &s,
        2,
        "–",
        |c| c != '-' && !crate::js_space(c),
        |next| next.is_none_or(|c| is_line_terminator(c) || (c != '-' && !crate::js_space(c))),
    )
}

/// Replaces each maximal run of at least `min` characters that `is` takes.
fn replace_runs(
    s: &str,
    is: impl Fn(char) -> bool,
    min: usize,
    with: impl Fn(&[char], usize) -> String,
) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < c.len() {
        if is(c[i]) {
            let start = i;
            while i < c.len() && is(c[i]) {
                i += 1;
            }
            if i - start >= min {
                out.push_str(&with(&c[start..i], start));
            } else {
                out.extend(&c[start..i]);
            }
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// `s.replace(/(^|BEFORE)-{n}(?=AFTER|$)/gm, '$1' + with)`, where `before` is the class a
/// character before the dashes must be in, and `after` decides the lookahead from the character
/// after them (`None` at the end of the text).
fn replace_dashes(
    s: &str,
    n: usize,
    with: &str,
    before: impl Fn(char) -> bool,
    after: impl Fn(Option<char>) -> bool,
) -> String {
    let c: Vec<char> = s.chars().collect();
    let dashes_at = |p: usize| p + n <= c.len() && c[p..p + n].iter().all(|&d| d == '-');
    let mut out = String::with_capacity(s.len());
    let mut p = 0;
    while p < c.len() {
        let line_start = p == 0 || is_line_terminator(c[p - 1]);
        if line_start && dashes_at(p) && after(c.get(p + n).copied()) {
            out.push_str(with);
            p += n;
        } else if before(c[p]) && dashes_at(p + 1) && after(c.get(p + 1 + n).copied()) {
            out.push(c[p]);
            out.push_str(with);
            p += 1 + n;
        } else {
            out.push(c[p]);
            p += 1;
        }
    }
    out
}

fn is_white_space(u: u16) -> bool {
    crate::emphasis::is_white_space(u32::from(u))
}

/// markdown-it's `isMdAsciiPunct(u) || isPunctCharCode(u)` on a UTF-16 unit (a surrogate is
/// neither).
fn is_punct(u: u16) -> bool {
    crate::emphasis::is_punct(uc_micro::PUNCT_OR_SYMBOL, u32::from(u))
}

const MAX_OPENERS: usize = 1000;

struct Opener {
    token: usize,
    pos: usize,
    single: bool,
    level: usize,
    prev_same: isize,
}

/// markdown-it's `smartquotes` (`process_inlines`) on one inline block, quotes `“”‘’`.
fn smartquotes(tokens: &mut [Tok]) {
    const OPEN_DOUBLE: u16 = 0x201c;
    const CLOSE_DOUBLE: u16 = 0x201d;
    const OPEN_SINGLE: u16 = 0x2018;
    const CLOSE_SINGLE: u16 = 0x2019;
    const APOSTROPHE: u16 = 0x2019;
    let units: Vec<Vec<u16>> = tokens
        .iter()
        .map(|t| t.content.encode_utf16().collect())
        .collect();
    let mut stack: Vec<Opener> = Vec::new();
    let (mut head_single, mut head_double) = (-1isize, -1isize);
    let mut replacements: Vec<Vec<(usize, u16)>> = vec![Vec::new(); tokens.len()];
    let truncate = |stack: &mut Vec<Opener>, hs: &mut isize, hd: &mut isize, len: usize| {
        while stack.len() > len {
            let item = stack.pop().expect("longer than len");
            if item.single {
                *hs = item.prev_same;
            } else {
                *hd = item.prev_same;
            }
        }
    };
    for i in 0..tokens.len() {
        let this_level = tokens[i].level;
        let mut j = stack.len() as isize - 1;
        while j >= 0 {
            if stack[j as usize].level <= this_level {
                break;
            }
            j -= 1;
        }
        truncate(
            &mut stack,
            &mut head_single,
            &mut head_double,
            (j + 1) as usize,
        );
        if tokens[i].kind != Kind::Text {
            continue;
        }
        let text = &units[i];
        let max = text.len();
        let mut pos = 0;
        'outer: while pos < max {
            let Some(at) = text[pos..]
                .iter()
                .position(|&u| u == u16::from(b'\'') || u == u16::from(b'"'))
                .map(|k| pos + k)
            else {
                break;
            };
            let mut can_open = true;
            let mut can_close = true;
            pos = at + 1;
            let single = text[at] == u16::from(b'\'');
            let mut last_char = 0x20;
            if at >= 1 {
                last_char = text[at - 1];
            } else {
                for k in (0..i).rev() {
                    if matches!(tokens[k].kind, Kind::Softbreak | Kind::Hardbreak) {
                        break;
                    }
                    if let Some(&u) = units[k].last() {
                        last_char = u;
                        break;
                    }
                }
            }
            let mut next_char = 0x20;
            if pos < max {
                next_char = text[pos];
            } else {
                for (k, t) in tokens.iter().enumerate().skip(i + 1) {
                    if matches!(t.kind, Kind::Softbreak | Kind::Hardbreak) {
                        break;
                    }
                    if let Some(&u) = units[k].first() {
                        next_char = u;
                        break;
                    }
                }
            }
            let last_punct = is_punct(last_char);
            let next_punct = is_punct(next_char);
            let last_space = is_white_space(last_char);
            let next_space = is_white_space(next_char);
            if next_space || (next_punct && !(last_space || last_punct)) {
                can_open = false;
            }
            if last_space || (last_punct && !(next_space || next_punct)) {
                can_close = false;
            }
            if next_char == 0x22 && text[at] == 0x22 && (0x30..=0x39).contains(&last_char) {
                can_close = false;
                can_open = false;
            }
            if can_open && can_close {
                can_open = last_punct;
                can_close = next_punct;
            }
            if !can_open && !can_close {
                if single {
                    replacements[i].push((at, APOSTROPHE));
                }
                continue;
            }
            if can_close {
                let j = if single { head_single } else { head_double };
                if j >= 0 && stack[j as usize].level == this_level {
                    let item = &stack[j as usize];
                    let (open, close) = if single {
                        (OPEN_SINGLE, CLOSE_SINGLE)
                    } else {
                        (OPEN_DOUBLE, CLOSE_DOUBLE)
                    };
                    let (token, item_pos) = (item.token, item.pos);
                    replacements[i].push((at, close));
                    replacements[token].push((item_pos, open));
                    truncate(&mut stack, &mut head_single, &mut head_double, j as usize);
                    continue 'outer;
                }
            }
            if can_open {
                if stack.len() >= MAX_OPENERS {
                    return;
                }
                stack.push(Opener {
                    token: i,
                    pos: at,
                    single,
                    level: this_level,
                    prev_same: if single { head_single } else { head_double },
                });
                if single {
                    head_single = stack.len() as isize - 1;
                } else {
                    head_double = stack.len() as isize - 1;
                }
            } else if can_close && single {
                replacements[i].push((at, APOSTROPHE));
            }
        }
    }
    for (i, mut reps) in replacements.into_iter().enumerate() {
        if reps.is_empty() {
            continue;
        }
        // JavaScript's sort is stable.
        reps.sort_by_key(|&(p, _)| p);
        let text = &units[i];
        let mut out: Vec<u16> = Vec::with_capacity(text.len());
        let mut last = 0;
        for (p, ch) in reps {
            out.extend_from_slice(&text[last.min(p)..p]);
            out.push(ch);
            last = p + 1;
        }
        out.extend_from_slice(&text[last.min(text.len())..]);
        tokens[i].content = String::from_utf16_lossy(&out);
        tokens[i].changed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rare_replacements_follow_markdown_it() {
        for (s, want) in [
            ("a +- b", "a ± b"),
            ("wait... what?..", "wait… what?.."),
            ("!!!!!? ok", "??? ok"),
            ("a,, b,,,c", "a, b,c"),
            ("a---b", "a—b"),
            ("a----b", "a----b"),
            ("---\n---", "—\n—"),
            ("a -- b", "a – b"),
            ("a--b", "a–b"),
            ("--", "–"),
            ("x -- -- y", "x – – y"),
            ("1--2--3", "1–2–3"),
        ] {
            assert_eq!(rare(s), want, "{s:?}");
        }
    }
}
