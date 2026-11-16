//! Markdown rendering for Brrrty.

/// Renders Markdown source to an HTML string.
pub trait MarkdownRenderer: Send + Sync {
    fn render(&self, source: &str) -> String;
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
        markdown_it::plugins::extra::tables::add(&mut md);
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
        self.md.parse(source).render()
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
    markdown_it::plugins::extra::tables::add(&mut md);
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
        if !source.ends_with('\n') {
            // markdown-it leaves the newline off the last line of a fence that runs to the end of
            // the text; the port adds one.
            ast.walk_mut(|n, _| {
                let Some(map) = n.srcmap else { return };
                let (start, end) = map.get_byte_offsets();
                let Some(fence) = n.cast_mut::<CodeFence>() else {
                    return;
                };
                if end != source.len() || !fence.content.ends_with('\n') {
                    return;
                }
                let lines = &source[start..end];
                let last = lines.rsplit('\n').next().unwrap_or("");
                let body = last.trim_start_matches([' ', '>', '\t']);
                let marks = body.chars().take_while(|&c| c == fence.marker).count();
                let closed = lines.contains('\n')
                    && marks >= fence.marker_len
                    && body[marks..].trim_matches([' ', '\t']).is_empty();
                if !closed {
                    fence.content.pop();
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

    /// 11ty disables markdown-it's `code` rule, so indented lines are paragraph text.
    #[test]
    fn indented_code_is_a_paragraph() {
        assert_eq!(
            md().render("para\n\n    indented\n"),
            "<p>para</p>\n<p>indented</p>\n"
        );
    }
}
