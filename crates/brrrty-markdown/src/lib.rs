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
