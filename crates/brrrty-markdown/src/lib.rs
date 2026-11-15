//! Markdown rendering for Brrrty.

use pulldown_cmark::{Options, Parser, html};

/// Renders Markdown source to an HTML string.
pub trait MarkdownRenderer: Send + Sync {
    fn render(&self, source: &str) -> String;
}

/// A stand-in renderer built on pulldown-cmark, with tables and strikethrough on and raw
/// HTML passed through. A closer markdown-it match replaces it later.
pub struct Provisional;

impl MarkdownRenderer for Provisional {
    fn render(&self, source: &str) -> String {
        let source = source.replace("\r\n", "\n");
        let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
        let mut out = String::new();
        html::push_html(&mut out, Parser::new_ext(&source, options));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_markdown_it_on_basics() {
        assert_eq!(
            Provisional.render("# Hello\n\nWorld\n"),
            "<h1>Hello</h1>\n<p>World</p>\n"
        );
        assert_eq!(
            Provisional.render("About *me*\n"),
            "<p>About <em>me</em></p>\n"
        );
        assert_eq!(Provisional.render("CR body\r\n"), "<p>CR body</p>\n");
        assert_eq!(Provisional.render(""), "");
    }
}
