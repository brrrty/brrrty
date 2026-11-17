//! An image's `alt` as markdown-it writes it: `renderInlineAsText` on the image's tokens. The
//! port collects only its plain text nodes, so it dropped entities, escapes and line breaks.
//! 14.3.3 also leaves out code spans; 15 keeps them.

use markdown_it::parser::inline::{Text, TextSpecial};
use markdown_it::plugins::cmark::inline::backticks::CodeInline;
use markdown_it::plugins::cmark::inline::image::Image;
use markdown_it::plugins::cmark::inline::newline::{Hardbreak, Softbreak};
use markdown_it::plugins::html::html_inline::HtmlInline;
use markdown_it::{Node, NodeValue, Renderer};

/// An image with its `alt` worked out.
#[derive(Debug)]
struct AltImage {
    url: String,
    title: Option<String>,
    alt: String,
}

impl NodeValue for AltImage {
    fn render(&self, node: &Node, fmt: &mut dyn Renderer) {
        let mut attrs = node.attrs.clone();
        attrs.push(("src", self.url.clone()));
        attrs.push(("alt", self.alt.clone()));
        if let Some(title) = &self.title {
            attrs.push(("title", title.clone()));
        }
        fmt.self_close("img", &attrs);
    }
}

/// `renderInlineAsText` on `children` (the image's tokens, flattened). `joined`: 15's
/// `text_join` made the escapes and entities among them text, which it does for an image's own
/// tokens but not for an image's inside it.
fn as_text(children: &[Node], v15: bool, joined: bool, out: &mut String) {
    for n in children {
        if let Some(t) = n.cast::<Text>() {
            out.push_str(&t.content);
        } else if let Some(t) = n.cast::<TextSpecial>() {
            // An autolink's text is `text`. 15's `text_join` makes the others (escapes and
            // entities) text inside images too; 14.3.3's leaves them out.
            if (v15 && joined) || t.info == "autolink" {
                out.push_str(&t.content);
            }
        } else if let Some(h) = n.cast::<HtmlInline>() {
            out.push_str(&h.content);
        } else if n.is::<Softbreak>() || n.is::<Hardbreak>() {
            out.push('\n');
        } else if n.is::<CodeInline>() {
            if v15 {
                as_text(&n.children, v15, joined, out);
            }
        } else if n.is::<Image>() || n.is::<AltImage>() {
            as_text(&n.children, v15, false, out);
        } else {
            // Emphasis, links and the like are open and close tokens around tokens in the same
            // list.
            as_text(&n.children, v15, joined, out);
        }
    }
}

/// Gives every image in `ast` markdown-it's `alt` (15's when `v15`, else 14.3.3's).
pub(crate) fn fix_images(ast: &mut Node, v15: bool) {
    ast.walk_mut(|n, _| {
        let Some(image) = n.cast::<Image>() else {
            return;
        };
        let (url, title) = (image.url.clone(), image.title.clone());
        let mut alt = String::new();
        as_text(&n.children, v15, true, &mut alt);
        n.replace(AltImage { url, title, alt });
    });
}
