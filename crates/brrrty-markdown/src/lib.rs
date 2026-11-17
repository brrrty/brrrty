//! Markdown rendering for Brrrty.

mod backticks;
mod core_rules;
mod emphasis;
mod escape;
mod linkify;
mod linkify_re;
mod mdurl;
mod uc_micro;

/// Renders Markdown source to an HTML string.
pub trait MarkdownRenderer: Send + Sync {
    fn render(&self, source: &str) -> String;

    /// Renders `source` as [`MarkdownRenderer::render`] does, but with markdown-it's `highlight`
    /// option set (11ty's syntax highlighting plugin sets it on 11ty's own instance). Each fenced
    /// code block whose info string names a language is handed to `highlight`, in document order,
    /// and what it returns replaces the whole block (the plugin's output starts with `<pre`, which
    /// markdown-it writes as it is, plus a newline); `None` keeps the block as markdown-it writes
    /// it when `highlight` returns nothing. A renderer that has no such option
    /// renders as [`MarkdownRenderer::render`] and never calls `highlight`.
    fn render_highlighted(&self, source: &str, highlight: &mut Highlight<'_>) -> String {
        let _ = highlight;
        self.render(source)
    }
}

/// A markdown-it `highlight` function: given a fenced code block, the HTML to write for it.
pub type Highlight<'a> = dyn FnMut(&Fence<'_>) -> Option<String> + 'a;

/// A fenced code block, as markdown-it's `highlight` option is given it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence<'a> {
    /// The code: `token.content`.
    pub content: &'a str,
    /// The first word of the unescaped, trimmed info string: `langName`. Never empty.
    pub language: &'a str,
    /// The line the fence opens on in the source, from 1.
    pub line: usize,
}

/// markdown-it's `langName`: the info string unescaped, trimmed as JavaScript trims, up to its
/// first whitespace (`info.split(/(\s+)/g)[0]`).
fn lang_name(info: &str) -> String {
    let info = markdown_it::common::utils::unescape_all(info);
    let info = info.trim_matches(js_space);
    info.split(js_space).next().unwrap_or("").to_string()
}

/// JavaScript's whitespace, as `String.prototype.trim` and regex `\s` see it. Unlike Rust's
/// `char::is_whitespace` it includes U+FEFF and leaves out U+0085.
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// A fenced code block markdown-it's `highlight` wrote: `highlighted + '\n'`.
#[derive(Debug)]
struct Highlighted(String);

impl markdown_it::NodeValue for Highlighted {
    fn render(&self, _: &markdown_it::Node, fmt: &mut dyn markdown_it::Renderer) {
        fmt.cr();
        fmt.text_raw(&self.0);
        fmt.cr();
    }
}

/// Hands each fence in `ast` that names a language to `highlight`, in document order, and puts
/// what it returns in the fence's place.
fn highlight_fences(ast: &mut markdown_it::Node, source: &str, highlight: &mut Highlight<'_>) {
    use markdown_it::plugins::cmark::block::fence::CodeFence;
    ast.walk_mut(|n, _| {
        let Some(fence) = n.cast::<CodeFence>() else {
            return;
        };
        let language = lang_name(&fence.info);
        if language.is_empty() {
            return;
        }
        let start = n.srcmap.map_or(0, |m| m.get_byte_offsets().0);
        let line = source[..start.min(source.len())].matches('\n').count() + 1;
        let html = highlight(&Fence {
            content: &fence.content,
            language: &language,
            line,
        });
        if let Some(html) = html {
            debug_assert!(html.starts_with("<pre"), "markdown-it wraps anything else");
            n.replace(Highlighted(html));
        }
    });
}

/// markdown-it's table rule needs a `|` in the header row too, so `a\n|--` is a paragraph. The
/// port's rule doesn't check that; this wraps it, and is registered where `tables::add` would be.
struct PipeTable;

impl markdown_it::parser::block::BlockRule for PipeTable {
    fn check(state: &mut markdown_it::parser::block::BlockState) -> Option<()> {
        use markdown_it::plugins::extra::tables::TableScanner;
        state.get_line(state.line).contains('|').then_some(())?;
        TableScanner::check(state)
    }

    fn run(
        state: &mut markdown_it::parser::block::BlockState,
    ) -> Option<(markdown_it::Node, usize)> {
        use markdown_it::plugins::extra::tables::TableScanner;
        state.get_line(state.line).contains('|').then_some(())?;
        TableScanner::run(state)
    }
}

fn add_tables(md: &mut markdown_it::MarkdownIt) {
    // markdown-it tries `table` before every other block rule.
    md.block.add_rule::<PipeTable>().before_all();
}

/// markdown-it's `html_inline` rule (14.3.3 and 15 are the same). The port follows CommonMark
/// 0.31's regular expressions; markdown-it scans a comment, processing instruction, declaration
/// or CDATA section to the first terminator (so `<!-->` and `--` inside are fine), and after one
/// that is never closed it takes no more of them in the same inline text.
struct HtmlInline;

/// `skipHtmlSpecial`: an earlier special construct in this inline text wasn't closed.
#[derive(Debug, Default)]
struct SkipHtmlSpecial;
impl markdown_it::parser::extset::InlineRootExt for SkipHtmlSpecial {}

/// JavaScript's `\s`.
const JS_SPACE_CLASS: &str = r"[\t\n\x0B\x0C\r \u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]";

/// markdown-it's `HTML_OPEN_CLOSE_TAG_RE`. `regex` matches leftmost-first, as a backtracking
/// engine does.
fn html_open_close_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        let s = JS_SPACE_CLASS;
        let attr_value = r#"(?:[^"'=<>`\x00-\x20]+|'[^']*'|"[^"]*")"#;
        let attribute = format!(r"(?:{s}+[a-zA-Z_:][a-zA-Z0-9:._-]*(?:{s}*={s}*{attr_value})?)");
        let open_tag = format!(r"<[A-Za-z][A-Za-z0-9\-]*{attribute}*{s}*/?>");
        let close_tag = format!(r"</[A-Za-z][A-Za-z0-9\-]*{s}*>");
        regex::Regex::new(&format!("^(?:{open_tag}|{close_tag})")).unwrap()
    })
}

fn is_letter(c: Option<u8>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphabetic())
}

/// markdown-it's `scanSpecial`: where the construct at `pos` ends, if it does.
fn scan_special(state: &mut markdown_it::parser::inline::InlineState) -> Option<usize> {
    let pos = state.pos;
    let src = state.src.as_bytes();
    let rest = &state.src[pos..];
    let (start, terminator) = if let Some(body) = rest.strip_prefix("<!--") {
        if body.starts_with('>') {
            return Some(pos + 5);
        }
        if body.starts_with("->") {
            return Some(pos + 6);
        }
        (pos + 4, "-->")
    } else if rest.starts_with("<![CDATA[") {
        (pos + 9, "]]>")
    } else if src.get(pos + 1) == Some(&b'?') {
        (pos + 2, "?>")
    } else if is_letter(src.get(pos + 2).copied()) {
        (pos + 3, ">")
    } else {
        return None;
    };
    if state.inline_ext.get::<SkipHtmlSpecial>().is_some() {
        return None;
    }
    match state.src[start..].find(terminator) {
        Some(at) => Some(start + at + terminator.len()),
        None => {
            state.inline_ext.insert(SkipHtmlSpecial);
            None
        }
    }
}

/// Where the HTML at `state.pos` ends, as markdown-it's `html_inline` finds it.
fn scan_html_inline(state: &mut markdown_it::parser::inline::InlineState) -> Option<usize> {
    let rest = &state.src[state.pos..state.pos_max];
    let mut units = rest.chars().map(char::len_utf16);
    // `pos + 2 >= max`: fewer than three UTF-16 units left.
    if !rest.starts_with('<') || units.by_ref().take(3).sum::<usize>() < 3 {
        return None;
    }
    let ch = rest.as_bytes()[1];
    if ch == b'!' || ch == b'?' {
        scan_special(state)
    } else if ch == b'/' || ch.is_ascii_alphabetic() {
        html_open_close_re()
            .find(&state.src[state.pos..])
            .map(|m| state.pos + m.len())
    } else {
        None
    }
}

fn is_link_open(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3
        && b[0] == b'<'
        && b[1].eq_ignore_ascii_case(&b'a')
        && (b[2] == b'>' || s[2..].starts_with(|c: char| crate::js_space(c)))
}

fn is_link_close(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("</") else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(['a', 'A']) else {
        return false;
    };
    rest.trim_start_matches(crate::js_space).starts_with('>')
}

impl markdown_it::parser::inline::InlineRule for HtmlInline {
    const MARKER: char = '<';

    fn check(state: &mut markdown_it::parser::inline::InlineState) -> Option<usize> {
        scan_html_inline(state).map(|end| end - state.pos)
    }

    fn run(
        state: &mut markdown_it::parser::inline::InlineState,
    ) -> Option<(markdown_it::Node, usize)> {
        let end = scan_html_inline(state)?;
        let content = state.src[state.pos..end].to_string();
        if is_link_open(&content) {
            state.link_level += 1;
        }
        if is_link_close(&content) {
            state.link_level -= 1;
        }
        let node =
            markdown_it::Node::new(markdown_it::plugins::html::html_inline::HtmlInline { content });
        Some((node, end - state.pos))
    }
}

/// markdown-it's `html: true`: HTML blocks from the port, inline HTML from [`HtmlInline`].
fn add_html(md: &mut markdown_it::MarkdownIt) {
    markdown_it::plugins::html::html_block::add(md);
    md.inline.add_rule::<HtmlInline>();
}

/// An empty blockquote is `<blockquote></blockquote>` in markdown-it; the port puts a newline
/// inside it.
#[derive(Debug)]
struct EmptyBlockquote;

impl markdown_it::NodeValue for EmptyBlockquote {
    fn render(&self, _: &markdown_it::Node, fmt: &mut dyn markdown_it::Renderer) {
        fmt.cr();
        fmt.text_raw("<blockquote></blockquote>");
        fmt.cr();
    }
}

/// A blockquote with nothing to show (or only reference definitions) is
/// `<blockquote></blockquote>` in markdown-it.
fn fix_tree(ast: &mut markdown_it::Node) {
    use markdown_it::plugins::cmark::block::blockquote::Blockquote;
    ast.walk_mut(|n, _| {
        if n.is::<Blockquote>() && n.children.iter().all(|c| c.render().is_empty()) {
            n.replace(EmptyBlockquote);
        }
    });
}

/// The markdown-it port (`markdown-it` 0.6.1) configured like 11ty's `Markdown.js`: CommonMark
/// without the indented `code` rule, plus tables, strikethrough, and raw HTML. No linkify,
/// typographer, or smart quotes.
pub struct MarkdownIt {
    md: markdown_it::MarkdownIt,
}

impl MarkdownIt {
    pub fn new() -> Self {
        let mut md = new_parser();
        // `cmark::add` minus `block::code`: 11ty calls `disable("code")`.
        add_commonmark(&mut md, false, true);
        add_html(&mut md);
        emphasis::add_strikethrough(&mut md);
        add_tables(&mut md);
        // The port only limits indentation (a line indented 4 or more is
        // not a list item, a fence, a heading...) when the `code` rule is
        // added; markdown-it always does.
        md.max_indent = 4;
        MarkdownIt { md }
    }
}

impl Default for MarkdownIt {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownRenderer for MarkdownIt {
    fn render(&self, source: &str) -> String {
        let mut ast = self.md.parse(source);
        fix_tree(&mut ast);
        let marker = fix_end_of_text(&mut ast, source);
        render_marked(&ast, marker)
    }

    fn render_highlighted(&self, source: &str, highlight: &mut Highlight<'_>) -> String {
        let mut ast = self.md.parse(source);
        fix_tree(&mut ast);
        let marker = fix_end_of_text(&mut ast, source);
        highlight_fences(&mut ast, source, highlight);
        render_marked(&ast, marker)
    }
}

/// The markdown-it port set up like `require('markdown-it')()` with no options: the `default`
/// preset, which has HTML **off** (unlike 11ty's own instance), linkify, the typographer and
/// `breaks` off, and every CommonMark rule on, including indented code blocks, plus tables and
/// strikethrough. It also renders one line of text with only the inline rules, as `renderInline`
/// does.
pub struct DefaultMarkdownIt {
    block: markdown_it::MarkdownIt,
    inline: markdown_it::MarkdownIt,
}

/// The port's parser with markdown-it's link functions.
fn new_parser() -> markdown_it::MarkdownIt {
    let mut md = markdown_it::MarkdownIt::new();
    md.link_formatter = Box::new(mdurl::JsLinkFormatter);
    md
}

/// The port's `cmark::add`, in its order, with markdown-it's emphasis (see [`emphasis`]) and,
/// unless `code`, without the indented code rule. `eleventy` picks 14.3.3's punctuation.
fn add_commonmark(md: &mut markdown_it::MarkdownIt, code: bool, eleventy: bool) {
    use markdown_it::plugins::cmark::{block, inline};
    inline::newline::add(md);
    escape::add(md);
    backticks::add(md, eleventy);
    emphasis::add_emphasis(md, eleventy);
    inline::link::add(md);
    inline::image::add(md);
    inline::autolink::add(md);
    inline::entity::add(md);
    if code {
        block::code::add(md);
    }
    block::fence::add(md);
    block::blockquote::add(md);
    block::hr::add(md);
    block::list::add(md);
    block::reference::add(md);
    block::heading::add(md);
    block::lheading::add(md);
    block::paragraph::add(md);
}

fn default_parser() -> markdown_it::MarkdownIt {
    let mut md = new_parser();
    add_commonmark(&mut md, true, false);
    emphasis::add_strikethrough(&mut md);
    add_tables(&mut md);
    md
}

/// `renderInline`'s core: no block rules, the source is one `inline` token.
struct InlineOnly;

impl markdown_it::parser::core::CoreRule for InlineOnly {
    fn run(root: &mut markdown_it::Node, _: &markdown_it::MarkdownIt) {
        use markdown_it::parser::core::Root;
        use markdown_it::parser::inline::InlineRoot;
        let data = root.cast_mut::<Root>().expect("root");
        let source = std::mem::take(&mut data.content);
        // markdown-it's `normalize` rule: newlines, then NUL.
        let content = source
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\0', "\u{fffd}");
        root.children.push(markdown_it::Node::new(InlineRoot::new(
            content,
            vec![(0, 0)],
        )));
    }
}

/// A line break written out as markdown-it writes it. The port's own break nodes use `cr()`,
/// which writes nothing when the output already ends in a newline, so `a\n\nb` would lose one.
#[derive(Debug)]
struct RawBreak(&'static str);

impl markdown_it::NodeValue for RawBreak {
    fn render(&self, _: &markdown_it::Node, fmt: &mut dyn markdown_it::Renderer) {
        fmt.text_raw(self.0);
    }
}

/// Stands in for the start and the end of an inline source: the port trims spaces and tabs from
/// both ends of it (markdown-it's paragraph does that, but `renderInline` does not). It is
/// whitespace to the emphasis rules, as the start and end of the text are, and is taken out of
/// the output.
const EDGE: char = '\u{a0}';

impl DefaultMarkdownIt {
    pub fn new() -> Self {
        let block = default_parser();
        let mut inline = default_parser();
        inline
            .add_rule::<InlineOnly>()
            .before::<markdown_it::parser::block::builtin::BlockParserRule>();
        DefaultMarkdownIt { block, inline }
    }

    /// `md.renderInline(source)`.
    pub fn render_inline(&self, source: &str) -> String {
        use markdown_it::plugins::cmark::inline::newline::{Hardbreak, Softbreak};
        let mut padded = String::with_capacity(source.len() + 2 * EDGE.len_utf8());
        padded.push(EDGE);
        padded.push_str(source);
        padded.push(EDGE);
        let mut ast = self.inline.parse(&padded);
        fix_tree(&mut ast);
        ast.walk_mut(|n, _| {
            if n.is::<Softbreak>() {
                n.replace(RawBreak("\n"));
            } else if n.is::<Hardbreak>() {
                n.replace(RawBreak("<br>\n"));
            }
        });
        let html = ast.render();
        let inner = html.strip_prefix(EDGE).unwrap_or(&html);
        inner.strip_suffix(EDGE).unwrap_or(inner).to_string()
    }

    /// `md.render(source)`.
    fn render_block(&self, source: &str) -> String {
        let mut ast = self.block.parse(source);
        fix_tree(&mut ast);
        let marker = fix_end_of_text(&mut ast, source);
        render_marked(&ast, marker)
    }
}

/// Options for [`ConfiguredMarkdownIt`]: markdown-it's `html`, `linkify` and `typographer`, and
/// linkify-it's `fuzzyLink` (`md.linkify.set({ fuzzyLink })`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Options {
    pub html: bool,
    pub linkify: bool,
    pub typographer: bool,
    /// Links without a schema (`example.com`) when `linkify` is on. linkify-it 6 (markdown-it
    /// 15's) has it off by default.
    pub fuzzy_link: bool,
}

/// markdown-it 15.0.3 set up like `markdownIt({ html, linkify, typographer })` (the `default`
/// preset, so the indented code rule is on), with linkify-it 6.1.0. Rendering can refuse (see
/// [`ConfiguredMarkdownIt::render`]).
pub struct ConfiguredMarkdownIt {
    md: markdown_it::MarkdownIt,
    rules: core_rules::Rules,
    plain: bool,
}

impl ConfiguredMarkdownIt {
    pub fn new(options: Options) -> Self {
        let mut md = new_parser();
        add_commonmark(&mut md, true, false);
        if options.html {
            add_html(&mut md);
        }
        emphasis::add_strikethrough(&mut md);
        add_tables(&mut md);
        let linkify = options.linkify.then_some(linkify::Linkify {
            fuzzy_link: options.fuzzy_link,
        });
        let plain = linkify.is_none() && !options.typographer;
        if !plain {
            core_rules::add(&mut md, linkify);
        }
        ConfiguredMarkdownIt {
            md,
            rules: core_rules::Rules {
                linkify,
                typographer: options.typographer,
            },
            plain,
        }
    }

    /// `md.render(source)`. An error says what Brrrty can't render exactly: a text linkify can't
    /// be sure to read as linkify-it does, or a smart quote next to an image whose label can't be
    /// found in the source.
    pub fn render(&self, source: &str) -> Result<String, String> {
        let mut ast = self.md.parse(source);
        fix_tree(&mut ast);
        if !self.plain {
            core_rules::run(&mut ast, &self.md, source, self.rules)?;
        }
        let marker = fix_end_of_text(&mut ast, source);
        Ok(render_marked(&ast, marker))
    }
}

/// markdown-it leaves the newline off the last line of a block that runs to the end of a text
/// with no final newline: a fence that isn't closed, and an HTML block. The port adds one.
///
/// An HTML block is written as it is, so the port's renderer adds a newline after it whatever
/// its content ends with. Its content ends in a marker instead, and [`render_marked`] takes the
/// marker out with the newline written after it. Returns the marker, if one was used.
fn fix_end_of_text(ast: &mut markdown_it::Node, source: &str) -> Option<char> {
    use markdown_it::plugins::cmark::block::fence::CodeFence;
    use markdown_it::plugins::html::html_block::HtmlBlock;
    // markdown-it turns `\r\n` and `\r` into `\n` first.
    if source.ends_with(['\n', '\r']) {
        return None;
    }
    // A noncharacter the source doesn't have.
    let marker = ('\u{fdd0}'..='\u{fdef}').find(|&c| !source.contains(c));
    let mut used = None;
    ast.walk_mut(|n, _| {
        let Some(map) = n.srcmap else { return };
        let (start, end) = map.get_byte_offsets();
        if let Some(html) = n.cast_mut::<HtmlBlock>() {
            if let Some(marker) = marker
                && end >= source.len()
                && html.content.ends_with('\n')
            {
                html.content.pop();
                html.content.push(marker);
                used = Some(marker);
            }
            return;
        }
        let Some(fence) = n.cast_mut::<CodeFence>() else {
            return;
        };
        if end < source.trim_end_matches([' ', '\t']).len() {
            return;
        }
        let lines = &source[start..end];
        let last = lines.rsplit('\n').next().unwrap_or("");
        let body = last.trim_start_matches([' ', '>', '\t']);
        let marks = body.chars().take_while(|&c| c == fence.marker).count();
        let closed = lines.contains('\n')
            && marks >= fence.marker_len
            && body[marks..].trim_matches([' ', '\t']).is_empty();
        if closed {
            return;
        }
        if fence.content.ends_with('\n') {
            fence.content.pop();
        }
        // A last line of only spaces is empty to markdown-it.
        let tail = fence.content.rfind('\n').map_or(0, |i| i + 1);
        if fence.content[tail..]
            .bytes()
            .all(|b| b == b' ' || b == b'\t')
        {
            fence.content.truncate(tail);
        }
    });
    used
}

/// Renders `ast`, taking out `marker` and the newline written after it (see [`fix_end_of_text`]).
fn render_marked(ast: &markdown_it::Node, marker: Option<char>) -> String {
    let html = ast.render();
    match marker {
        Some(m) => html.replace(&format!("{m}\n"), ""),
        None => html,
    }
}

impl Default for DefaultMarkdownIt {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownRenderer for DefaultMarkdownIt {
    /// `md.render(source)`.
    fn render(&self, source: &str) -> String {
        self.render_block(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md() -> MarkdownIt {
        MarkdownIt::new()
    }

    #[test]
    fn heading_and_paragraph() {
        assert_eq!(
            md().render("# Hello\n\nWorld\n"),
            "<h1>Hello</h1>\n<p>World</p>\n"
        );
        assert_eq!(md().render("About *me*\n"), "<p>About <em>me</em></p>\n");
        assert_eq!(md().render("CR body\r\n"), "<p>CR body</p>\n");
        assert_eq!(md().render(""), "");
    }

    #[test]
    fn raw_html_block_passes_through() {
        let out = md().render("<div class=\"x\">hi</div>\n");
        assert!(out.contains("<div class=\"x\">hi</div>"), "{out}");
    }

    #[test]
    fn fenced_code_with_language() {
        let out = md().render("```js\nlet a = 1;\n```\n");
        assert!(out.contains("<pre><code class=\"language-js\">"), "{out}");
    }

    #[test]
    fn tables_and_strikethrough() {
        let out = md().render("a | b\n--|--\n1 | 2\n");
        assert!(out.contains("<table>"), "{out}");
        assert_eq!(md().render("~~x~~\n"), "<p><s>x</s></p>\n");
    }

    #[test]
    fn no_panic_on_edge_inputs() {
        let nested = ">".repeat(10_000);
        for input in ["", "\u{feff}x", nested.as_str()] {
            let input = input.to_string();
            let handle = std::thread::Builder::new()
                .stack_size(64 * 1024 * 1024)
                .spawn(move || {
                    let _ = MarkdownIt::new().render(&input);
                })
                .unwrap();
            assert!(handle.join().is_ok());
        }
    }

    /// One renderer is shared by every render thread, with no lock, and
    /// each thread still gets its own output.
    #[test]
    fn renders_from_many_threads_at_once() {
        let md = MarkdownIt::new();
        let start = std::sync::Barrier::new(8);
        std::thread::scope(|s| {
            for t in 0..8 {
                let (md, start) = (&md, &start);
                s.spawn(move || {
                    start.wait();
                    for i in 0..200 {
                        let out = md.render(&format!("# T{t}\n\nitem *{i}*\n"));
                        assert_eq!(out, format!("<h1>T{t}</h1>\n<p>item <em>{i}</em></p>\n"));
                    }
                });
            }
        });
    }

    /// Every fence that names a language, in document order: what the `highlight` option of
    /// markdown-it is called with.
    fn fences(source: &str) -> Vec<(String, String, usize)> {
        let mut seen = Vec::new();
        md().render_highlighted(source, &mut |f: &Fence<'_>| {
            seen.push((f.language.to_string(), f.content.to_string(), f.line));
            None
        });
        seen
    }

    #[test]
    fn hands_fences_that_name_a_language_to_highlight() {
        assert!(fences("```\nplain\n```\n").is_empty());
        assert!(fences("~~~   \nplain\n~~~\n").is_empty());
        assert!(fences("para\n\n    indented\n").is_empty());
        assert!(fences("`inline` code\n").is_empty());
        assert_eq!(
            fences(
                "# T\n\n```\nx\n```\n\n~~~js {1}\nlet a;\n~~~\n\n> ```text\n> x\n> ```\n\n- a\n\n  ```php/1\n  y\n  ```\n"
            ),
            [
                ("js".into(), "let a;\n".into(), 7),
                ("text".into(), "x\n".into(), 11),
                ("php/1".into(), "y\n".into(), 17),
            ]
        );
        assert_eq!(
            fences("```a&amp;b c\n```\n"),
            [("a&b".into(), String::new(), 1)]
        );
    }

    #[test]
    fn highlighted_html_replaces_the_block() {
        let out = md().render_highlighted("a\n\n```js\nx\n```\nb\n", &mut |f: &Fence<'_>| {
            Some(format!("<pre>{}</pre>", f.content.trim()))
        });
        assert_eq!(out, "<p>a</p>\n<pre>x</pre>\n<p>b</p>\n");
        let kept = md().render_highlighted("```js\n<x>\n```\n", &mut |_: &Fence<'_>| None);
        assert_eq!(kept, md().render("```js\n<x>\n```\n"));
    }

    /// markdown-it calls `highlight` with `unescapeAll(info).trim()`, and JavaScript's `trim`
    /// keeps U+0085 and removes U+FEFF. Node printed the code points `highlight` got for each info
    /// string: `85` for "\u{85}" and " \u{85} ", none for "\u{feff}", " \u{a0}\u{3000}" and
    /// "\u{2028}", and `180e` and `200b` for those (whitespace in neither language).
    #[test]
    fn info_strings_are_trimmed_as_javascript_trims() {
        let f = |info: &str| fences(&format!("```{info}\ncode\n```\n"));
        for named in ["\u{85}", " \u{85} ", "\u{85}x", "\u{180e}", "\u{200b}"] {
            assert_eq!(f(named).len(), 1, "{named:?}");
        }
        assert_eq!(f(" \u{85} ")[0].0, "\u{85}");
        assert_eq!(f("js\u{3000}x")[0].0, "js");
        for blank in ["\u{feff}", " \u{a0}\u{3000}", "\u{2028}"] {
            assert!(f(blank).is_empty(), "{blank:?}");
        }
    }

    /// markdown-it limits indentation without its `code` rule too: a line indented 4 or more
    /// (from where the block starts) continues the paragraph above it. Node's output for each.
    #[test]
    fn indentation_is_limited_without_the_code_rule() {
        assert_eq!(
            md().render("3. Deploy\n  1. Copy:\n    * A\n    * B\n  2. Link:\n    * C\n"),
            "<ol start=\"3\">\n<li>Deploy</li>\n<li>Copy:\n* A\n* B</li>\n<li>Link:\n* C</li>\n</ol>\n"
        );
        assert_eq!(md().render("a\n    # b\n"), "<p>a\n# b</p>\n");
        assert_eq!(md().render("    # b\n"), "<p># b</p>\n");
        assert_eq!(
            md().render("    ```\n    x\n    ```\n"),
            "<p><code>    x    </code></p>\n"
        );
    }

    /// 11ty disables markdown-it's `code` rule, so indented lines are paragraph text.
    #[test]
    fn indented_code_is_a_paragraph() {
        assert_eq!(
            md().render("para\n\n    indented\n"),
            "<p>para</p>\n<p>indented</p>\n"
        );
    }

    /// A code span of only spaces: 11ty's bundled markdown-it 14.3.3 strips one space from each
    /// end of three or more, and 15.0.3 (`DefaultMarkdownIt`) keeps them all. Expected values are
    /// what each version printed in Node.
    #[test]
    fn space_only_code_spans_follow_each_version() {
        let cases = [
            (
                "`   `",
                "<p><code> </code></p>\n",
                "<p><code>   </code></p>\n",
            ),
            (
                "`    `",
                "<p><code>  </code></p>\n",
                "<p><code>    </code></p>\n",
            ),
            (
                "` \n `",
                "<p><code> </code></p>\n",
                "<p><code>   </code></p>\n",
            ),
            (
                "a `   ` b",
                "<p>a <code> </code> b</p>\n",
                "<p>a <code>   </code> b</p>\n",
            ),
            (
                "`  `",
                "<p><code>  </code></p>\n",
                "<p><code>  </code></p>\n",
            ),
        ];
        let (v14, v15) = (MarkdownIt::new(), DefaultMarkdownIt::new());
        for (src, want14, want15) in cases {
            assert_eq!(v14.render(src), want14, "14.3.3: {src:?}");
            assert_eq!(v15.render(src), want15, "15.0.3: {src:?}");
        }
        assert_eq!(v15.render_inline("a `   ` b"), "a <code>   </code> b");
    }

    /// A renderer without markdown-it's `highlight` option renders as it always does and never
    /// calls `highlight`.
    #[test]
    fn renderers_without_the_option_never_call_highlight() {
        struct Upper;
        impl MarkdownRenderer for Upper {
            fn render(&self, source: &str) -> String {
                source.to_uppercase()
            }
        }
        let mut calls = 0;
        let out = Upper.render_highlighted("```js\nx\n```\n", &mut |_: &Fence<'_>| {
            calls += 1;
            Some("<pre>no</pre>".into())
        });
        assert_eq!(out, "```JS\nX\n```\n");
        assert_eq!(calls, 0);
    }

    /// 11ty's instance (markdown-it 14.3.3): a fence or an HTML block that runs to the end of a
    /// text with no final newline leaves the newline off its last line. `\r` at the end is a
    /// newline. Expected values are 14.3.3's `render` in Node.
    #[test]
    fn blocks_at_the_end_of_the_text_match_markdown_it_14() {
        for (src, want) in [
            ("<div>", "<div>"),
            ("<div>\nb c[>", "<div>\nb c[>"),
            ("a\n\n<!-- x", "<p>a</p>\n<!-- x"),
            ("> <div>", "<blockquote>\n<div></blockquote>\n"),
            ("- <div>", "<ul>\n<li>\n<div></li>\n</ul>\n"),
            ("<div>\n", "<div>\n"),
            ("<div>  ", "<div>  "),
            ("<div>\r", "<div>\n"),
            ("<div>\r\n", "<div>\n"),
            ("\"\n<div>[", "<p>&quot;</p>\n<div>["),
            ("<div>\n\n", "<div>\n"),
            (
                "```js\ncode",
                "<pre><code class=\"language-js\">code</code></pre>\n",
            ),
            ("```\na\n   ", "<pre><code>a\n</code></pre>\n"),
            (
                "> ```\n> x",
                "<blockquote>\n<pre><code>x</code></pre>\n</blockquote>\n",
            ),
            ("```\nx\n```", "<pre><code>x\n</code></pre>\n"),
        ] {
            assert_eq!(md().render(src), want, "{src:?}");
        }
    }

    /// markdown-it's HTML comments in text: `<!-->`, `<!--->` and `--` inside. 14.3.3's output.
    #[test]
    fn html_comments_in_text_match_markdown_it() {
        for (src, want) in [
            ("a <!--> b\n", "<p>a <!--> b</p>\n"),
            ("a <!---> b\n", "<p>a <!---> b</p>\n"),
            ("a <!-- x -- y --> b\n", "<p>a <!-- x -- y --> b</p>\n"),
            ("a <!-- x --->\n", "<p>a <!-- x ---></p>\n"),
            ("a <!-- x\ny --> b\n", "<p>a <!-- x\ny --> b</p>\n"),
            ("a <!-- *x* b\n", "<p>a &lt;!-- <em>x</em> b</p>\n"),
            ("a <!---x--> b\n", "<p>a <!---x--> b</p>\n"),
        ] {
            assert_eq!(md().render(src), want, "{src:?}");
        }
    }

    /// A fence that runs to the end of a text with no final newline: markdown-it leaves the
    /// newline off its last line, and a last line of only spaces or tabs is empty. A fence closed
    /// on the last line keeps its content's newline. Expected values are markdown-it 15.0.3's
    /// `render` in Node.
    #[test]
    fn a_fence_at_the_end_of_the_text_matches_markdown_it_15() {
        let md = DefaultMarkdownIt::default();
        for (src, want) in [
            (
                "```js\ncode",
                "<pre><code class=\"language-js\">code</code></pre>\n",
            ),
            ("```\na\n   ", "<pre><code>a\n</code></pre>\n"),
            ("```\na\n\t", "<pre><code>a\n</code></pre>\n"),
            (
                "> ```\n> x",
                "<blockquote>\n<pre><code>x</code></pre>\n</blockquote>\n",
            ),
            ("```\nx\n```", "<pre><code>x\n</code></pre>\n"),
            ("```\nx\n``", "<pre><code>x\n``</code></pre>\n"),
            ("~~~\nx\n~~~  ", "<pre><code>x\n</code></pre>\n"),
            (
                "- ```\n  y",
                "<ul>\n<li>\n<pre><code>y</code></pre>\n</li>\n</ul>\n",
            ),
            ("```\n\n", "<pre><code>\n</code></pre>\n"),
            ("a\n```\nb  ", "<p>a</p>\n<pre><code>b  </code></pre>\n"),
        ] {
            assert_eq!(md.render(src), want, "{src:?}");
        }
    }
}
