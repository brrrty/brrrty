//! markdown-it's `escape` rule (14.3.3 and 15), in place of the port's: a backslash before a
//! space is a literal backslash that leaves the space alone, so a newline rule after it still
//! drops the space (`a\ ` then a newline is `a\`, a soft break). The port takes the space too.

use markdown_it::parser::inline::{InlineRule, InlineState, TextSpecial};
use markdown_it::plugins::cmark::inline::newline::Hardbreak;
use markdown_it::{MarkdownIt, Node};

struct Escape;

/// `ESCAPED`: the ASCII punctuation a backslash escapes.
fn escapable(c: char) -> bool {
    "\\!\"#$%&'()*+,./:;<=>?@[]^_`{|}~-".contains(c)
}

impl InlineRule for Escape {
    const MARKER: char = '\\';

    fn run(state: &mut InlineState) -> Option<(Node, usize)> {
        let mut chars = state.src[state.pos..state.pos_max].chars();
        if chars.next()? != '\\' {
            return None;
        }
        match chars.next()? {
            '\n' => {
                // The next line's leading spaces and tabs go too.
                let skipped = chars.take_while(|&c| c == ' ' || c == '\t').count();
                Some((Node::new(Hardbreak), 2 + skipped))
            }
            ' ' => Some((
                Node::new(TextSpecial {
                    content: "\\".into(),
                    markup: "\\".into(),
                    info: "escape",
                }),
                1,
            )),
            c => {
                let orig = format!("\\{c}");
                let content = if escapable(c) {
                    c.to_string()
                } else {
                    orig.clone()
                };
                Some((
                    Node::new(TextSpecial {
                        content,
                        markup: orig,
                        info: "escape",
                    }),
                    1 + c.len_utf8(),
                ))
            }
        }
    }
}

pub(crate) fn add(md: &mut MarkdownIt) {
    md.inline.add_rule::<Escape>();
}
