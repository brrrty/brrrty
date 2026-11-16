//! `markdownIt().render(s || '')` and `markdownIt().renderInline(s || '')`, as the
//! `markdown` and `markdownify` filters of a site's own markdown-it instance run them.
//!
//! This is `markdownIt()` with no options, so HTML is **off** (11ty's own instance has it on),
//! linkify and the typographer are off, and the indented `code` rule is on.

use std::sync::OnceLock;

use brrrty_markdown::{DefaultMarkdownIt, MarkdownRenderer};

fn instance() -> &'static DefaultMarkdownIt {
    static MD: OnceLock<DefaultMarkdownIt> = OnceLock::new();
    MD.get_or_init(DefaultMarkdownIt::new)
}

/// `md.render(s)`. The caller maps JavaScript's falsy values (`null`, `undefined`, `""`) to
/// `""` first, as `s || ''` does.
pub fn markdown(s: &str) -> String {
    instance().render(s)
}

/// `md.renderInline(s)`.
pub fn markdown_inline(s: &str) -> String {
    instance().render_inline(s)
}
