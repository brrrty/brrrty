//! `markdownIt().render(s || '')` and `markdownIt().renderInline(s || '')`, as the
//! `markdown` and `markdownify` filters of a site's own markdown-it instance run them.
//!
//! This is `markdownIt()` with no options, so HTML is **off** (11ty's own instance has it on),
//! linkify and the typographer are off, and the indented `code` rule is on.

use std::sync::OnceLock;

pub use brrrty_markdown::Options;
use brrrty_markdown::{ConfiguredMarkdownIt, DefaultMarkdownIt, MarkdownRenderer};

fn instance() -> &'static DefaultMarkdownIt {
    static MD: OnceLock<DefaultMarkdownIt> = OnceLock::new();
    MD.get_or_init(DefaultMarkdownIt::new)
}

/// `md.render(s)`. The caller maps JavaScript's falsy values (`null`, `undefined`, `""`) to
/// `""` first, as `s || ''` does. An error names a construct Brrrty can't render exactly yet.
pub fn markdown(s: &str) -> Result<String, String> {
    instance().try_render(s)
}

/// `md.renderInline(s)`.
pub fn markdown_inline(s: &str) -> String {
    instance().render_inline(s)
}

/// `markdownIt({ html, linkify, typographer })` with `md.linkify.set({ fuzzyLink })`, then
/// `md.render(s)`. An error says what Brrrty can't render exactly (see
/// [`ConfiguredMarkdownIt::render`]).
pub fn markdown_with(s: &str, options: Options) -> Result<String, String> {
    static MDS: [OnceLock<ConfiguredMarkdownIt>; 16] = [const { OnceLock::new() }; 16];
    let index = usize::from(options.html)
        | usize::from(options.linkify) << 1
        | usize::from(options.typographer) << 2
        | usize::from(options.fuzzy_link) << 3;
    MDS[index]
        .get_or_init(|| ConfiguredMarkdownIt::new(options))
        .render(s)
}
