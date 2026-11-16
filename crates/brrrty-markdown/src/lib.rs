//! Markdown rendering for Brrrty.

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

/// Fixes to the parsed tree where the port's output differs from markdown-it's:
/// - a blockquote with nothing to show (or only reference definitions) is `<blockquote></blockquote>`;
/// - a code span of only spaces keeps all its spaces, in markdown-it 15 (`keep_space_only_code`).
///   11ty's bundled 14.3.3 still strips one space from each end of three or more.
fn fix_tree(ast: &mut markdown_it::Node, keep_space_only_code: bool) {
    use markdown_it::parser::inline::Text;
    use markdown_it::plugins::cmark::block::blockquote::Blockquote;
    use markdown_it::plugins::cmark::inline::backticks::CodeInline;
    ast.walk_mut(|n, _| {
        if n.is::<Blockquote>() && n.children.iter().all(|c| c.render().is_empty()) {
            n.replace(EmptyBlockquote);
        } else if let (true, Some(code)) = (keep_space_only_code, n.cast::<CodeInline>()) {
            let marker_len = code.marker_len;
            let Some(span) = n.srcmap.map(|m| m.get_byte_offsets()) else {
                return;
            };
            let outer = span.1 - span.0;
            if let Some(text) = n.children.first_mut().and_then(|t| t.cast_mut::<Text>()) {
                let len = text.content.len();
                if len > 0
                    && text.content.bytes().all(|b| b == b' ')
                    && outer == len + 2 + 2 * marker_len
                {
                    text.content = " ".repeat(len + 2);
                }
            }
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
        let mut md = markdown_it::MarkdownIt::new();
        // `cmark::add` minus `block::code`: 11ty calls `disable("code")`.
        use markdown_it::plugins::cmark::{block, inline};
        inline::newline::add(&mut md);
        inline::escape::add(&mut md);
        inline::backticks::add(&mut md);
        inline::emphasis::add(&mut md);
        inline::link::add(&mut md);
        inline::image::add(&mut md);
        inline::autolink::add(&mut md);
        inline::entity::add(&mut md);
        block::fence::add(&mut md);
        block::blockquote::add(&mut md);
        block::hr::add(&mut md);
        block::list::add(&mut md);
        block::reference::add(&mut md);
        block::heading::add(&mut md);
        block::lheading::add(&mut md);
        block::paragraph::add(&mut md);
        markdown_it::plugins::html::add(&mut md);
        markdown_it::plugins::extra::strikethrough::add(&mut md);
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
        fix_tree(&mut ast, false);
        ast.render()
    }

    fn render_highlighted(&self, source: &str, highlight: &mut Highlight<'_>) -> String {
        let mut ast = self.md.parse(source);
        fix_tree(&mut ast, false);
        highlight_fences(&mut ast, source, highlight);
        ast.render()
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

fn default_parser() -> markdown_it::MarkdownIt {
    let mut md = markdown_it::MarkdownIt::new();
    markdown_it::plugins::cmark::add(&mut md);
    markdown_it::plugins::extra::strikethrough::add(&mut md);
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
        fix_tree(&mut ast, true);
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
        use markdown_it::plugins::cmark::block::fence::CodeFence;
        let mut ast = self.block.parse(source);
        fix_tree(&mut ast, true);
        if !source.ends_with('\n') {
            // markdown-it leaves the newline off the last line of a fence that runs to the end of
            // the text; the port adds one.
            ast.walk_mut(|n, _| {
                let Some(map) = n.srcmap else { return };
                let (start, end) = map.get_byte_offsets();
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
        }
        ast.render()
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
}
