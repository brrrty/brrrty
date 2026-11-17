//! markdown-it's `table` rule (14.3.3 and 15 are the same), in place of the port's, which
//! turned down a one-column table (`| Name |` then `---`) and a header with an escaped `|`,
//! and split rows differently.

use markdown_it::parser::block::{BlockRule, BlockState};
use markdown_it::parser::inline::InlineRoot;
use markdown_it::plugins::cmark::block::blockquote::BlockquoteScanner;
use markdown_it::plugins::cmark::block::fence::FenceScanner;
use markdown_it::plugins::cmark::block::heading::HeadingScanner;
use markdown_it::plugins::cmark::block::hr::HrScanner;
use markdown_it::plugins::extra::tables::{
    ColumnAlignment, Table, TableBody, TableCell, TableHead, TableRow,
};
use markdown_it::{MarkdownIt, Node};

const MAX_AUTOCOMPLETED_CELLS: usize = 65536;

/// markdown-it's `escapedSplit`: split on `|`, where `\|` is a `|` in the cell.
fn escaped_split(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut last = 0;
    let mut escaped = false;
    for (pos, b) in s.bytes().enumerate() {
        if b == b'|' {
            if !escaped {
                result.push(current + &s[last..pos]);
                current = String::new();
                last = pos + 1;
            } else {
                current += &s[last..pos - 1];
                last = pos;
            }
        }
        escaped = b == b'\\';
    }
    result.push(current + &s[last..]);
    result
}

/// JavaScript's `String.prototype.trim`.
fn js_trim(s: &str) -> &str {
    s.trim_matches(|c| crate::js_space(c) || matches!(c, '\u{2028}' | '\u{2029}'))
}

fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// The header cells and the alignments, when a table starts at `state.line`.
fn header(state: &BlockState) -> Option<(Vec<String>, Vec<ColumnAlignment>)> {
    let start = state.line;
    if start + 2 > state.line_max {
        return None;
    }
    let next = start + 1;
    let indent = state.line_indent(next);
    if !(0..4).contains(&indent) {
        return None;
    }
    let delim = state.get_line(next).as_bytes();
    let first = *delim.first()?;
    if !matches!(first, b'|' | b'-' | b':') {
        return None;
    }
    let second = *delim.get(1)?;
    if !matches!(second, b'|' | b'-' | b':') && !is_space(second) {
        return None;
    }
    if first == b'-' && is_space(second) {
        return None;
    }
    if !delim[2..]
        .iter()
        .all(|&b| matches!(b, b'|' | b'-' | b':') || is_space(b))
    {
        return None;
    }
    let delim = state.get_line(next);
    let columns: Vec<&str> = delim.split('|').collect();
    let mut aligns = Vec::new();
    for (i, c) in columns.iter().enumerate() {
        let t = js_trim(c);
        if t.is_empty() {
            if i == 0 || i == columns.len() - 1 {
                continue;
            }
            return None;
        }
        let core = t.strip_prefix(':').unwrap_or(t);
        let core = core.strip_suffix(':').unwrap_or(core);
        if core.is_empty() || !core.bytes().all(|b| b == b'-') {
            return None;
        }
        aligns.push(match (t.starts_with(':'), t.ends_with(':')) {
            (true, true) => ColumnAlignment::Center,
            (false, true) => ColumnAlignment::Right,
            (true, false) => ColumnAlignment::Left,
            (false, false) => ColumnAlignment::None,
        });
    }
    let line = js_trim(state.get_line(start));
    if !line.contains('|') {
        return None;
    }
    if state.line_indent(start) >= 4 {
        return None;
    }
    let cells = trimmed_cells(line);
    if cells.is_empty() || cells.len() != aligns.len() {
        return None;
    }
    Some((cells, aligns))
}

/// `escapedSplit`, without an empty first and last cell.
fn trimmed_cells(line: &str) -> Vec<String> {
    let mut cells = escaped_split(line);
    if cells.first().is_some_and(String::is_empty) {
        cells.remove(0);
    }
    if cells.last().is_some_and(String::is_empty) {
        cells.pop();
    }
    cells
}

/// markdown-it's rules with `alt: ['blockquote']`, which end a table: fence, blockquote, hr,
/// list, HTML block (with HTML on) and heading.
fn terminates(state: &mut BlockState, line: usize) -> bool {
    let old = state.line;
    state.line = line;
    let html =
        state.md.ext.get::<crate::HtmlOn>().is_some() && crate::html_block::terminates(state);
    let ends = <FenceScanner as BlockRule>::check(state).is_some()
        || <BlockquoteScanner as BlockRule>::check(state).is_some()
        || <HrScanner as BlockRule>::check(state).is_some()
        || list_marker(state)
        || html
        || <HeadingScanner as BlockRule>::check(state).is_some();
    state.line = old;
    ends
}

/// markdown-it's `list` in validation mode outside a paragraph: a list marker (indented less
/// than 4) followed by a space, a tab or the end of the line.
fn list_marker(state: &BlockState) -> bool {
    if state.line_indent(state.line) >= 4 {
        return false;
    }
    let l = state.get_line(state.line).as_bytes();
    let after = match l.first() {
        Some(b'-' | b'+' | b'*') => 1,
        Some(b'0'..=b'9') => {
            let digits = l.iter().take_while(|b| b.is_ascii_digit()).count();
            if digits > 9 || !matches!(l.get(digits), Some(b'.' | b')')) {
                return false;
            }
            digits + 1
        }
        _ => return false,
    };
    l.get(after).is_none_or(|&b| is_space(b))
}

fn cell(content: &str, src: usize) -> Node {
    let mut n = Node::new(TableCell);
    if !content.is_empty() {
        n.children.push(Node::new(InlineRoot::new(
            content.to_string(),
            vec![(0, src)],
        )));
    }
    n
}

struct JsTable;

impl BlockRule for JsTable {
    fn check(state: &mut BlockState) -> Option<()> {
        use markdown_it::plugins::cmark::block::list::{BulletList, OrderedList};
        // markdown-it's table can end a paragraph or a reference (its `alt`), and so it can
        // interrupt a paragraph inside a list item (where the parser's node is the item). It
        // isn't one of the list's terminators: the port asks those with the list as its node.
        if state.node.is::<BulletList>() || state.node.is::<OrderedList>() {
            return None;
        }
        header(state).map(|_| ())
    }

    fn run(state: &mut BlockState) -> Option<(Node, usize)> {
        let (head, aligns) = header(state)?;
        let start = state.line;
        let count = head.len();
        let src_of = |state: &BlockState, line: usize| state.line_offsets[line].first_nonspace;
        let mut table = Node::new(Table {
            alignments: aligns.clone(),
        });
        let mut thead = Node::new(TableHead);
        thead.srcmap = state.get_map(start, start + 1);
        let mut row = Node::new(TableRow);
        row.srcmap = state.get_map(start, start);
        let at = src_of(state, start);
        for c in &head {
            row.children.push(cell(js_trim(c), at));
        }
        thead.children.push(row);
        table.children.push(thead);
        let mut body = Node::new(TableBody);
        let mut autocompleted = 0usize;
        let mut next = start + 2;
        while next < state.line_max {
            if state.line_indent(next) < 0 || terminates(state, next) {
                break;
            }
            let line = js_trim(state.get_line(next));
            if line.is_empty() || state.line_indent(next) >= 4 {
                break;
            }
            let cells = trimmed_cells(line);
            autocompleted += count.saturating_sub(cells.len());
            if autocompleted > MAX_AUTOCOMPLETED_CELLS {
                break;
            }
            let mut row = Node::new(TableRow);
            row.srcmap = state.get_map(next, next);
            let at = src_of(state, next);
            for i in 0..count {
                row.children
                    .push(cell(cells.get(i).map_or("", |c| js_trim(c)), at));
            }
            body.children.push(row);
            next += 1;
        }
        if !body.children.is_empty() {
            body.srcmap = state.get_map(start + 2, next - 1);
            table.children.push(body);
        }
        Some((table, next - start))
    }
}

/// markdown-it tries `table` before every other block rule.
pub(crate) fn add(md: &mut MarkdownIt) {
    md.block.add_rule::<JsTable>().before_all();
}
