//! markdown-it renderer rules (`md.renderer.rules[type] = (tokens, idx, options, env, self) =>
//! ...`) for inline tokens.
//!
//! The port renders from its tree. When an instance has rules, each inline block is flattened
//! to the token list markdown-it hands `renderInline` (the `children` of that `inline` token,
//! after linkify, the typographer and `text_join`), and a [`RenderRules`] renders it: a rule
//! for a type that has one, else markdown-it's default rule or `renderToken`
//! ([`render_default`]). Block tokens keep the tree's rendering, so rules for them are refused
//! (see [`is_block_type`]).

use markdown_it::parser::inline::{Text, TextSpecial};
use markdown_it::plugins::cmark::inline::autolink::Autolink;
use markdown_it::plugins::cmark::inline::backticks::CodeInline;
use markdown_it::plugins::cmark::inline::emphasis::{Em, Strong};
use markdown_it::plugins::cmark::inline::link::Link;
use markdown_it::plugins::cmark::inline::newline::{Hardbreak, Softbreak};
use markdown_it::plugins::extra::strikethrough::Strikethrough;
use markdown_it::plugins::html::html_inline::HtmlInline;
use markdown_it::{Node, NodeValue, Renderer};

use crate::core_rules::{InlineGroup, LinkifyLink};

/// A markdown-it `Token` in an inline token list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// `type`.
    pub kind: String,
    pub tag: String,
    /// `null` is `None`.
    pub attrs: Option<Vec<(String, String)>>,
    pub nesting: i64,
    pub level: i64,
    pub content: String,
    pub markup: String,
    pub info: String,
    pub block: bool,
    pub hidden: bool,
    /// On an `image`: what `renderInlineAsText(token.children)` gives, which markdown-it's
    /// `image` rule writes as `alt`. The children themselves aren't given.
    pub alt_text: Option<String>,
}

impl Token {
    fn new(kind: &str, tag: &str, nesting: i64, level: i64) -> Token {
        Token {
            kind: kind.into(),
            tag: tag.into(),
            attrs: None,
            nesting,
            level,
            content: String::new(),
            markup: String::new(),
            info: String::new(),
            block: false,
            hidden: false,
            alt_text: None,
        }
    }

    /// `attrIndex(name)`.
    pub fn attr_index(&self, name: &str) -> Option<usize> {
        self.attrs.as_ref()?.iter().position(|(n, _)| n == name)
    }

    /// `attrGet(name)`.
    pub fn attr_get(&self, name: &str) -> Option<&str> {
        let i = self.attr_index(name)?;
        Some(&self.attrs.as_ref()?[i].1)
    }

    /// `attrPush([name, value])`.
    pub fn attr_push(&mut self, name: String, value: String) {
        self.attrs.get_or_insert_with(Vec::new).push((name, value));
    }

    /// `attrSet(name, value)`.
    pub fn attr_set(&mut self, name: String, value: String) {
        match self.attr_index(&name) {
            Some(i) => self.attrs.as_mut().expect("has attrs")[i].1 = value,
            None => self.attr_push(name, value),
        }
    }

    /// `attrJoin(name, value)`.
    pub fn attr_join(&mut self, name: String, value: String) {
        match self.attr_index(&name) {
            Some(i) => {
                let v = &mut self.attrs.as_mut().expect("has attrs")[i].1;
                v.push(' ');
                v.push_str(&value);
            }
            None => self.attr_push(name, value),
        }
    }
}

/// The markdown-it options a renderer rule is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOptions {
    pub html: bool,
    pub xhtml_out: bool,
    pub breaks: bool,
    pub linkify: bool,
    pub typographer: bool,
    /// markdown-it 15's renderer (`markdownIt()` and `markdown()`), not 14.3.3's (11ty's own
    /// instance): their `renderToken` differ around hidden tokens.
    pub v15: bool,
}

/// `options.langPrefix`, which no instance Brrrty has changes.
pub const LANG_PREFIX: &str = "language-";
/// `options.quotes`, the same in every preset.
pub const QUOTES: &str = "\u{201c}\u{201d}\u{2018}\u{2019}";

/// markdown-it's `renderInline` with the instance's renderer rules: how an inline token list is
/// written. An `Err` is a rule that failed; it is reported as it is.
pub trait RenderRules {
    fn render_inline(&self, tokens: Vec<Token>, options: &RenderOptions) -> Result<String, String>;
}

/// Why a render with rules failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// Markdown Brrrty can't render exactly (see [`crate::MarkdownRenderer::try_render`]).
    Refused(String),
    /// A renderer rule failed.
    Rule(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Refused(s) | RenderError::Rule(s) => f.write_str(s),
        }
    }
}

/// markdown-it's block token types (14.3.3 and 15, with tables). Renderer rules for them are
/// refused: blocks are written from the port's tree.
pub fn is_block_type(kind: &str) -> bool {
    matches!(
        kind,
        "paragraph_open"
            | "paragraph_close"
            | "heading_open"
            | "heading_close"
            | "blockquote_open"
            | "blockquote_close"
            | "bullet_list_open"
            | "bullet_list_close"
            | "ordered_list_open"
            | "ordered_list_close"
            | "list_item_open"
            | "list_item_close"
            | "code_block"
            | "fence"
            | "html_block"
            | "hr"
            | "table_open"
            | "table_close"
            | "thead_open"
            | "thead_close"
            | "tbody_open"
            | "tbody_close"
            | "tr_open"
            | "tr_close"
            | "th_open"
            | "th_close"
            | "td_open"
            | "td_close"
    )
}

/// The inline types markdown-it has a default rule for (`md.renderer.rules.text` and so on).
pub fn has_default_rule(kind: &str) -> bool {
    matches!(
        kind,
        "code_inline" | "image" | "hardbreak" | "softbreak" | "text" | "html_inline"
    )
}

/// markdown-it's `escapeHtml`.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// `renderAttrs(token)`.
pub fn render_attrs(token: &Token) -> String {
    let mut out = String::new();
    for (name, value) in token.attrs.iter().flatten() {
        out.push(' ');
        out.push_str(&escape_html(name));
        out.push_str("=\"");
        out.push_str(&escape_html(value));
        out.push('"');
    }
    out
}

/// `renderToken(tokens, idx, options)`: 14.3.3's, or 15's when `v15`. 15 looks past hidden
/// tokens with `nesting` 0 on both sides, and writes the newline before a block token only
/// after a hidden closing token.
pub fn render_token(tokens: &[Token], idx: usize, xhtml_out: bool, v15: bool) -> String {
    let token = &tokens[idx];
    if token.hidden {
        return String::new();
    }
    let skip = |t: &Token| v15 && t.hidden && t.nesting == 0;
    let mut out = String::new();
    let prev = if v15 {
        tokens[..idx].iter().rev().find(|t| !skip(t))
    } else {
        idx.checked_sub(1).map(|i| &tokens[i])
    };
    let lf_before = prev.is_some_and(|p| p.hidden && (!v15 || p.nesting == -1));
    if token.block && token.nesting != -1 && lf_before {
        out.push('\n');
    }
    out.push_str(if token.nesting == -1 { "</" } else { "<" });
    out.push_str(&token.tag);
    out.push_str(&render_attrs(token));
    if token.nesting == 0 && xhtml_out {
        out.push_str(" /");
    }
    let mut need_lf = false;
    if token.block {
        need_lf = true;
        let next = tokens[idx + 1..].iter().find(|t| !skip(t));
        if token.nesting == 1
            && let Some(next) = next
        {
            // An inline or hidden token next, or the closing tag of the same element.
            if next.kind == "inline" || next.hidden || (next.nesting == -1 && next.tag == token.tag)
            {
                need_lf = false;
            }
        }
    }
    out.push_str(if need_lf { ">\n" } else { ">" });
    out
}

/// markdown-it's default rule for `tokens[idx]`'s type, or `renderToken` when it has none.
/// The `image` rule sets the token's `alt` first, as markdown-it's does; an image with no `alt`
/// attribute is an error (markdown-it throws a `TypeError`).
pub fn render_default(
    tokens: &mut [Token],
    idx: usize,
    options: &RenderOptions,
) -> Result<String, String> {
    let br = if options.xhtml_out {
        "<br />\n"
    } else {
        "<br>\n"
    };
    let token = &tokens[idx];
    Ok(match token.kind.as_str() {
        "code_inline" => format!(
            "<code{}>{}</code>",
            render_attrs(token),
            escape_html(&token.content)
        ),
        "hardbreak" => br.into(),
        "softbreak" => {
            if options.breaks {
                br.into()
            } else {
                "\n".into()
            }
        }
        "text" => escape_html(&token.content),
        "html_inline" => token.content.clone(),
        kind => {
            if kind == "image" {
                // markdown-it reads the image's `children`, which only a real image has.
                let Some(alt) = token.alt_text.clone() else {
                    return Err(
                        "the image rule needs the image's children, which only an image token has (markdown-it throws)"
                            .into(),
                    );
                };
                let Some(i) = token.attr_index("alt") else {
                    return Err(
                        "the image rule needs an `alt` attribute, and markdown-it throws without one"
                            .into(),
                    );
                };
                tokens[idx].attrs.as_mut().expect("has attrs")[i].1 = alt;
            }
            render_token(tokens, idx, options.xhtml_out, options.v15)
        }
    })
}

/// [`RenderRules`] with no rules: markdown-it's own `renderInline`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoRules;

impl RenderRules for NoRules {
    fn render_inline(
        &self,
        mut tokens: Vec<Token>,
        options: &RenderOptions,
    ) -> Result<String, String> {
        let mut out = String::new();
        for i in 0..tokens.len() {
            out.push_str(&render_default(&mut tokens, i, options)?);
        }
        Ok(out)
    }
}

/// HTML a rule wrote for an inline block, written as it is.
#[derive(Debug)]
struct Written(String);

impl NodeValue for Written {
    fn render(&self, _: &Node, fmt: &mut dyn Renderer) {
        fmt.text_raw(&self.0);
    }
}

/// Renders each inline block of `ast` through `rules`, in document order. `source` is what
/// was parsed.
pub(crate) fn apply(
    ast: &mut Node,
    source: &str,
    rules: &dyn RenderRules,
    options: &RenderOptions,
) -> Result<(), RenderError> {
    let mut failed = None;
    ast.walk_mut(|n, _| {
        if failed.is_some() || !n.is::<InlineGroup>() {
            return;
        }
        // A port marker after the inline nodes stays after them.
        let marks = n
            .children
            .iter()
            .rposition(|c| !c.is::<crate::Mark>())
            .map_or(0, |i| i + 1);
        let tail = n.children.split_off(marks);
        let tokens = match flatten(&n.children, source) {
            Ok(t) => t,
            Err(e) => {
                failed = Some(RenderError::Refused(e));
                return;
            }
        };
        match rules.render_inline(tokens, options) {
            Ok(html) => {
                n.children = vec![Node::new(Written(html))];
                n.children.extend(tail);
            }
            Err(e) => failed = Some(RenderError::Rule(e)),
        }
    });
    failed.map_or(Ok(()), Err)
}

/// A token of [`walk`]'s list, with what it was when markdown-it's `fragments_join` ran.
struct Flat {
    token: Token,
    stage: Stage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// A `text` token then (core linkify may have split it since).
    Text,
    /// The empty `text` token emphasis leaves beside a `strong` pair.
    Empty,
    /// Anything else.
    Other,
}

/// markdown-it's token list for an inline block's nodes, after `text_join`.
pub(crate) fn flatten(children: &[Node], source: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    walk(children, 0, source, &mut out)?;
    // `fragments_join` merged an empty token into the text next to it (keeping the later
    // token, whose `markup` and `info` are empty either way), and empty tokens next to each
    // other into one.
    let mut fragments: Vec<Flat> = Vec::with_capacity(out.len());
    for f in out {
        if let Some(last) = fragments.last()
            && (last.stage == Stage::Empty && f.stage != Stage::Other
                || last.stage == Stage::Text && f.stage == Stage::Empty)
        {
            if f.stage == Stage::Empty {
                // The empty token is the later one: the text's content goes into it, and it
                // stays a text token of the split text.
                continue;
            }
            fragments.pop();
        }
        fragments.push(f);
    }
    // `text_join`: runs of text become their last token, holding all of their content.
    let mut joined: Vec<Token> = Vec::with_capacity(fragments.len());
    for Flat { token: t, .. } in fragments {
        if t.kind == "text"
            && let Some(last) = joined.last_mut()
            && last.kind == "text"
        {
            let mut content = std::mem::take(&mut last.content);
            content.push_str(&t.content);
            *last = Token { content, ..t };
            continue;
        }
        joined.push(t);
    }
    Ok(joined)
}

fn text(content: &str, level: i64) -> Token {
    Token {
        content: content.into(),
        ..Token::new("text", "", 0, level)
    }
}

fn walk(children: &[Node], level: i64, source: &str, out: &mut Vec<Flat>) -> Result<(), String> {
    let push = |out: &mut Vec<Flat>, token: Token, stage: Stage| out.push(Flat { token, stage });
    for n in children {
        if !n.attrs.is_empty() {
            return Err(format!(
                "renderer rules: an inline node with attributes Brrrty doesn't know ({})",
                n.name()
            ));
        }
        let pair = |kind: &str, tag: &str, markup: String, info: &str| {
            let open = Token {
                markup: markup.clone(),
                info: info.into(),
                ..Token::new(&format!("{kind}_open"), tag, 1, level)
            };
            let close = Token {
                markup,
                info: info.into(),
                ..Token::new(&format!("{kind}_close"), tag, -1, level)
            };
            (open, close)
        };
        let wrap = if let Some(e) = n.cast::<Em>() {
            Some(pair("em", "em", e.marker.to_string(), ""))
        } else if let Some(s) = n.cast::<Strong>() {
            Some(pair("strong", "strong", s.marker.to_string().repeat(2), ""))
        } else if let Some(s) = n.cast::<Strikethrough>() {
            Some(pair("s", "s", s.marker.to_string().repeat(2), ""))
        } else if let Some(l) = n.cast::<Link>() {
            let (mut open, close) = pair("link", "a", String::new(), "");
            let mut attrs = vec![("href".to_string(), l.url.clone())];
            if let Some(t) = l.title.as_ref().filter(|t| !t.is_empty()) {
                attrs.push(("title".into(), t.clone()));
            }
            open.attrs = Some(attrs);
            Some((open, close))
        } else if let Some(l) = n.cast::<Autolink>() {
            let (mut open, close) = pair("link", "a", "autolink".into(), "auto");
            open.attrs = Some(vec![("href".into(), l.url.clone())]);
            Some((open, close))
        } else if let Some(l) = n.cast::<LinkifyLink>() {
            let (mut open, close) = pair("link", "a", "linkify".into(), "auto");
            open.attrs = Some(vec![("href".into(), l.url.clone())]);
            Some((open, close))
        } else {
            None
        };
        if let Some((open, close)) = wrap {
            // markdown-it's emphasis turns the inner marker of each side of `**` into the tag
            // and empties the outer one, which stays a text token.
            let strong = n.is::<Strong>();
            if strong {
                push(out, text("", level), Stage::Empty);
            }
            // Core linkify's links were text when `fragments_join` ran.
            let stage = if n.cast::<LinkifyLink>().is_some_and(|l| l.core) {
                Stage::Text
            } else {
                Stage::Other
            };
            push(out, open, stage);
            walk(&n.children, level + 1, source, out)?;
            push(out, close, stage);
            if strong {
                push(out, text("", level), Stage::Empty);
            }
        } else if let Some(t) = n.cast::<Text>() {
            push(out, text(&t.content, level), Stage::Text);
        } else if let Some(d) = n.cast::<crate::emphasis::Delim>() {
            push(out, text(&d.content, level), Stage::Text);
        } else if let Some(t) = n.cast::<TextSpecial>() {
            // An autolink's text is a plain `text` token; escapes and entities are
            // `text_special`, which `text_join` makes `text`, keeping `markup` and `info`.
            let mut tok = text(&t.content, level);
            if t.info == "autolink" {
                push(out, tok, Stage::Text);
            } else {
                tok.markup = t.markup.clone();
                tok.info = t.info.into();
                push(out, tok, Stage::Other);
            }
        } else if let Some(h) = n.cast::<HtmlInline>() {
            push(
                out,
                Token {
                    content: h.content.clone(),
                    ..Token::new("html_inline", "", 0, level)
                },
                Stage::Other,
            );
        } else if let Some(c) = n.cast::<CodeInline>() {
            let code: String = n
                .children
                .iter()
                .filter_map(|c| c.cast::<Text>().map(|t| t.content.as_str()))
                .collect();
            push(
                out,
                Token {
                    content: code,
                    markup: c.marker.to_string().repeat(c.marker_len),
                    ..Token::new("code_inline", "code", 0, level)
                },
                Stage::Other,
            );
        } else if let Some(img) = n.cast::<crate::alt::AltImage>() {
            let mut attrs = vec![
                ("src".to_string(), img.url.clone()),
                ("alt".to_string(), String::new()),
            ];
            if let Some(t) = img.title.as_ref().filter(|t| !t.is_empty()) {
                attrs.push(("title".into(), t.clone()));
            }
            push(
                out,
                Token {
                    attrs: Some(attrs),
                    content: image_label(n, source)?,
                    alt_text: Some(img.alt.clone()),
                    ..Token::new("image", "img", 0, level)
                },
                Stage::Other,
            );
        } else if n.is::<Softbreak>() {
            push(out, Token::new("softbreak", "br", 0, level), Stage::Other);
        } else if n.is::<Hardbreak>() {
            push(out, Token::new("hardbreak", "br", 0, level), Stage::Other);
        } else {
            return Err(format!(
                "renderer rules: an inline node Brrrty doesn't know ({})",
                n.name()
            ));
        }
    }
    Ok(())
}

/// An image's label as written (markdown-it's `content` for an `image` token): from after its
/// `![` to its `]`. The last node's source map can stop before the closing markers of emphasis
/// or strikethrough around it, which are all that can come between it and the `]`.
fn image_label(image: &Node, source: &str) -> Result<String, String> {
    let refuse =
        || "renderer rules: can't tell where an image's label is in the source".to_string();
    let open = image.srcmap.ok_or_else(refuse)?.get_byte_offsets().0;
    if source.get(open..open + 2) != Some("![") {
        return Err(refuse());
    }
    let start = open + 2;
    let mut end = match image.children.last() {
        None => start,
        Some(last) => last.srcmap.ok_or_else(refuse)?.get_byte_offsets().1,
    };
    let bytes = source.as_bytes();
    while matches!(bytes.get(end), Some(b'*' | b'_' | b'~')) {
        end += 1;
    }
    if end < start || bytes.get(end) != Some(&b']') {
        return Err(refuse());
    }
    Ok(source[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorder(RefCell<Vec<Vec<Token>>>);

    impl RenderRules for Recorder {
        fn render_inline(
            &self,
            tokens: Vec<Token>,
            options: &RenderOptions,
        ) -> Result<String, String> {
            self.0.borrow_mut().push(tokens.clone());
            NoRules.render_inline(tokens, options)
        }
    }

    fn short(t: &Token) -> String {
        format!(
            "{}:{}:{}:{}:{}:{:?}",
            t.kind, t.level, t.content, t.markup, t.info, t.attrs
        )
    }

    /// Pinned from markdown-it 15.0.3's `md.parse`: the empty text tokens a `strong` pair
    /// leaves, `text_join` keeping the last token's `markup` and `info`, and link attributes.
    #[test]
    fn inline_tokens_are_markdown_its() {
        let md = crate::DefaultMarkdownIt::new();
        let rec = Recorder::default();
        let html = md
            .render_with_rules("**a** x\\* &amp; [b](c \"t\") <http://d.e>", &rec)
            .unwrap();
        assert_eq!(
            html,
            "<p><strong>a</strong> x* &amp; <a href=\"c\" title=\"t\">b</a> <a href=\"http://d.e\">http://d.e</a></p>\n"
        );
        let got: Vec<String> = rec.0.borrow()[0].iter().map(short).collect();
        assert_eq!(
            got,
            [
                "text:0::::None",
                "strong_open:0::**::None",
                "text:1:a:::None",
                "strong_close:0::**::None",
                "text:0: x* & :::None",
                "link_open:0::::Some([(\"href\", \"c\"), (\"title\", \"t\")])",
                "text:1:b:::None",
                "link_close:0::::None",
                "text:0: :::None",
                "link_open:0::autolink:auto:Some([(\"href\", \"http://d.e\")])",
                "text:1:http://d.e:::None",
                "link_close:0::autolink:auto:None",
            ]
        );
    }

    #[test]
    fn render_token_and_attrs_follow_markdown_it() {
        let mut t = Token::new("link_open", "a", 1, 0);
        t.attr_set("href".into(), "a\"<&>".into());
        t.attr_join("class".into(), "x".into());
        t.attr_join("class".into(), "y".into());
        assert_eq!(
            render_token(std::slice::from_ref(&t), 0, false, true),
            "<a href=\"a&quot;&lt;&amp;&gt;\" class=\"x y\">"
        );
        let br = Token::new("softbreak", "br", 0, 0);
        assert_eq!(render_token(&[br], 0, true, true), "<br />");
        let mut hidden = Token::new("paragraph_open", "p", 1, 0);
        hidden.hidden = true;
        assert_eq!(render_token(&[hidden], 0, false, false), "");
    }
}
