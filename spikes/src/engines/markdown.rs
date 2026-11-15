//! Markdown candidates, each configured to match markdown-it's default preset with
//! `html: true` (CommonMark, tables, strikethrough, raw HTML, no linkify, no typographer).
//! Each adapter uses only its crate's public API.

use brrrty_markdown::MarkdownRenderer;
use std::sync::Mutex;

/// comrak 0.59.0 with `default-features = false` (no syntect, no CLI).
///
/// Options set: `extension.strikethrough`, `extension.table`, `render.unsafe_` (raw HTML
/// passes through). Everything else stays at its default: no autolink, no tasklist, no
/// footnotes, no header ids, `parse.smart` off.
pub struct Comrak {
    options: comrak::Options<'static>,
}

impl Comrak {
    pub fn new() -> Self {
        let mut options = comrak::Options::default();
        options.extension.strikethrough = true;
        options.extension.table = true;
        options.render.r#unsafe = true;
        Comrak { options }
    }
}

impl Default for Comrak {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownRenderer for Comrak {
    fn render(&self, source: &str) -> String {
        comrak::markdown_to_html(source, &self.options)
    }
}

/// pulldown-cmark 0.13.6, `html` feature only.
///
/// Options set: `ENABLE_TABLES | ENABLE_STRIKETHROUGH`. Raw HTML passes through by default.
/// No smart punctuation, footnotes, task lists or heading attributes.
pub struct Pulldown;

impl MarkdownRenderer for Pulldown {
    fn render(&self, source: &str) -> String {
        let options =
            pulldown_cmark::Options::ENABLE_TABLES | pulldown_cmark::Options::ENABLE_STRIKETHROUGH;
        let mut out = String::new();
        pulldown_cmark::html::push_html(&mut out, pulldown_cmark::Parser::new_ext(source, options));
        out
    }
}

/// markdown-it 0.6.1 with `default-features = false` (no linkify, no syntect).
///
/// Plugins added: every `cmark` rule except `block::code` (11ty disables indented code
/// blocks), `html` (raw HTML blocks and inline),
/// `extra::strikethrough`, `extra::tables`. Not added: `extra::linkify`, `typographer`,
/// `smartquotes`, `beautify_links`, `heading_anchors`, `syntect`, `sourcepos`.
pub struct MarkdownItRs {
    md: Mutex<markdown_it::MarkdownIt>,
}

impl MarkdownItRs {
    pub fn new() -> Self {
        let mut md = markdown_it::MarkdownIt::new();
        // `cmark::add` minus `block::code`: 11ty calls `disable("code")` on its markdown-it
        // instance, so indented lines are paragraph text, never code blocks.
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
        MarkdownItRs { md: Mutex::new(md) }
    }
}

impl Default for MarkdownItRs {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownRenderer for MarkdownItRs {
    fn render(&self, source: &str) -> String {
        let md = self.md.lock().unwrap_or_else(|e| e.into_inner());
        md.parse(source).render()
    }
}
