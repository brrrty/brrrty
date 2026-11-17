//! markdown-it's `html_block` rule (14.3.3 and 15 are the same), in place of the port's. The
//! port ends any HTML block at a line indented less than the list item it's in, a blank line
//! included, so `<pre>`, `<script>`, `<style>`, `<textarea>`, comments, processing instructions,
//! declarations and CDATA lost everything after a blank line in a list item. markdown-it ends
//! those only at their closing text (or a non-blank line outside the item). Declarations take
//! `<!a` as well as `<!A`, and tag names match ASCII case only, as JavaScript's `i` does.

use markdown_it::parser::block::{BlockRule, BlockState};
use markdown_it::plugins::html::html_block::HtmlBlock;
use markdown_it::{MarkdownIt, Node};

/// markdown-it's `html_blocks` (the same in 14.3.3 and 15).
const BLOCK_NAMES: &str = "address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h1|h2|h3|h4|h5|h6|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul";

/// One of `HTML_SEQUENCES`: its opening and closing tests, and whether it can interrupt a
/// paragraph.
struct Sequence {
    open: regex::Regex,
    close: regex::Regex,
    /// The open and close tests are case-insensitive: they run on the ASCII-lowercased line.
    fold: bool,
    terminates_paragraph: bool,
}

fn sequences() -> &'static [Sequence; 7] {
    static S: std::sync::OnceLock<[Sequence; 7]> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        let s = crate::JS_SPACE_CLASS;
        let re = |p: &str| regex::Regex::new(p).expect("a valid pattern");
        let seq = |open: &str, close: &str, fold: bool, terminates_paragraph: bool| Sequence {
            open: re(open),
            close: re(close),
            fold,
            terminates_paragraph,
        };
        [
            seq(
                &format!(r"^<(?:script|pre|style|textarea)(?:{s}|>|$)"),
                r"</(?:script|pre|style|textarea)>",
                true,
                true,
            ),
            seq(r"^<!--", r"-->", false, true),
            seq(r"^<\?", r"\?>", false, true),
            seq(r"^<![A-Za-z]", r">", false, true),
            seq(r"^<!\[CDATA\[", r"\]\]>", false, true),
            seq(
                &format!(r"^</?(?:{BLOCK_NAMES})(?:{s}|/?>|$)"),
                r"^$",
                true,
                true,
            ),
            seq(
                &format!("{}{s}*$", crate::html_open_close_re().as_str()),
                r"^$",
                false,
                false,
            ),
        ]
    })
}

impl Sequence {
    fn opens(&self, line: &str) -> bool {
        if self.fold {
            self.open.is_match(&line.to_ascii_lowercase())
        } else {
            self.open.is_match(line)
        }
    }

    fn closes(&self, line: &str) -> bool {
        if self.fold {
            self.close.is_match(&line.to_ascii_lowercase())
        } else {
            self.close.is_match(line)
        }
    }
}

struct HtmlBlockRule;

fn sequence(state: &BlockState) -> Option<&'static Sequence> {
    if state.line_indent(state.line) >= state.md.max_indent {
        return None;
    }
    let line = state.get_line(state.line);
    if !line.starts_with('<') {
        return None;
    }
    sequences().iter().find(|s| s.opens(line))
}

impl BlockRule for HtmlBlockRule {
    fn check(state: &mut BlockState) -> Option<()> {
        sequence(state)?.terminates_paragraph.then_some(())
    }

    fn run(state: &mut BlockState) -> Option<(Node, usize)> {
        let seq = sequence(state)?;
        let start = state.line;
        let mut next = start + 1;
        let ends_on_blank = seq.closes("");
        if !seq.closes(state.get_line(start)) {
            while next < state.line_max {
                if state.line_indent(next) < 0 && (ends_on_blank || !state.is_empty(next)) {
                    break;
                }
                let line = state.get_line(next);
                if seq.closes(line) {
                    if !line.is_empty() {
                        next += 1;
                    }
                    break;
                }
                next += 1;
            }
        }
        let (content, _) = state.get_lines(start, next, state.blk_indent, true);
        Some((Node::new(HtmlBlock { content }), next - start))
    }
}

/// Whether an HTML block that can end a paragraph starts at `state.line` (`html_block` in
/// validation mode).
pub(crate) fn terminates(state: &BlockState) -> bool {
    sequence(state).is_some_and(|s| s.terminates_paragraph)
}

pub(crate) fn add(md: &mut MarkdownIt) {
    md.block.add_rule::<HtmlBlockRule>();
}
