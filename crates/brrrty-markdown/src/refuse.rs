//! Shapes the port's block rules still parse differently from markdown-it, refused instead of
//! written wrong. Each check is a little wider than the difference, so it can refuse input
//! markdown-it and Brrrty would agree on, but never lets a difference through.
//!
//! - A paragraph that starts like a reference definition (`[label]:`): markdown-it takes
//!   destinations the port refuses (`[docs]: C:\path\`). And any definition-like line ending in
//!   `\`, which the port can read on into the next line.
//! - A blockquote paragraph with a lazy line (fewer `>` than its quotes) that is only a list
//!   marker (`> quote` then `-`): markdown-it ends the quote and starts an empty list item.
//! - A table right after a blockquote whose paragraph runs to its last line: markdown-it reads
//!   the table's lines as that paragraph's lazy continuation.
//! - A blockquote paragraph with an indented lazy line and a code span or inline HTML across a
//!   line break: markdown-it keeps the lazy line's indentation in the span, the port drops it.
//! - A tab after a blockquote's `>` among a line's container markers (`>\t`, `>> *\t`): the
//!   port counts the tab's columns differently (checked on the source's lines).

use markdown_it::Node;
use markdown_it::plugins::cmark::block::blockquote::Blockquote;
use markdown_it::plugins::cmark::block::paragraph::Paragraph;
use markdown_it::plugins::extra::tables::Table;

fn text<'a>(n: &Node, source: &'a str) -> Option<&'a str> {
    let (a, b) = n.srcmap?.get_byte_offsets();
    source.get(a..b)
}

/// The source's lines, split as markdown-it splits them (`\r\n`, `\r` and `\n`).
fn lines(s: &str) -> Vec<&str> {
    s.split('\n').flat_map(|l| l.split('\r')).collect()
}

/// Whether `line` starts with a link label (no unescaped `[` inside) right before a `:`.
fn starts_like_definition(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('[') else {
        return false;
    };
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '[' => return false,
            ']' => return chars.next() == Some(':'),
            _ => {}
        }
    }
    false
}

fn is_marker_only(line: &str) -> bool {
    let t = line.trim_matches([' ', '\t']);
    if matches!(t, "-" | "+" | "*") {
        return true;
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    (1..=9).contains(&digits) && t.len() == digits + 1 && t.ends_with(['.', ')'])
}

/// Where the paragraph a blockquote ends with ends.
fn last_paragraph_end(n: &Node) -> Option<usize> {
    match n.children.last() {
        Some(c) if c.is::<Paragraph>() => Some(c.srcmap?.get_byte_offsets().1),
        Some(c) if c.is::<Blockquote>() => last_paragraph_end(c),
        _ => None,
    }
}

fn ends_in_paragraph(n: &Node) -> bool {
    match n.children.last() {
        Some(c) if c.is::<Paragraph>() => true,
        Some(c) if c.is::<Blockquote>() => ends_in_paragraph(c),
        _ => false,
    }
}

/// The container markers at the start of a line: `>`, list markers and the spaces and tabs
/// between them.
fn prefix(line: &str) -> &str {
    let b = line.as_bytes();
    let mut i = 0;
    let mut end = 0;
    loop {
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        let marker = match b.get(i) {
            Some(b'>') => 1,
            Some(b'-' | b'+' | b'*') => 1,
            Some(b'0'..=b'9') => {
                let d = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
                if d <= 9 && matches!(b.get(i + d), Some(b'.' | b')')) {
                    d + 1
                } else {
                    0
                }
            }
            _ => 0,
        };
        // A list marker needs a space, a tab or the end after it.
        if marker == 0
            || (b[i] != b'>' && !b.get(i + marker).is_none_or(|&c| c == b' ' || c == b'\t'))
        {
            break;
        }
        i += marker;
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        end = i;
    }
    &line[..end]
}

/// Whether a tab comes after a `>` in `line`'s container markers.
fn tab_after_quote(line: &str) -> bool {
    let p = prefix(line);
    p.find('>').is_some_and(|i| p[i..].contains('\t'))
}

/// The `>` markers at the start of `line`, and what follows them.
fn quote_markers(line: &str) -> (usize, &str) {
    let end = line
        .find(|c: char| !matches!(c, '>' | ' ' | '\t'))
        .unwrap_or(line.len());
    let head = &line[..end];
    (head.matches('>').count(), &line[end..])
}

/// Whether `n` (under `depth` blockquotes) is something the port writes differently there.
fn quoted_difference(n: &Node, source: &str, depth: usize) -> Option<String> {
    if n.is::<Table>() {
        let (a, b) = n.srcmap?.get_byte_offsets();
        let start = source[..a].rfind(['\n', '\r']).map_or(0, |i| i + 1);
        let lazy = lines(&source[start..b])
            .iter()
            .any(|l| quote_markers(l).0 < depth);
        return lazy.then(|| {
            "a table on a blockquote's lazy lines, which markdown-it reads as paragraph text"
                .to_string()
        });
    }
    use markdown_it::plugins::cmark::inline::backticks::CodeInline;
    use markdown_it::plugins::html::html_inline::HtmlInline;
    if n.is::<Paragraph>() || n.is::<crate::core_rules::InlineGroup>() {
        let t = text(n, source)?;
        // A line with fewer `>` than the paragraph's quotes is lazy for the inner ones.
        let lazy: Vec<(&str, &str)> = lines(t)
            .into_iter()
            .skip(1)
            .filter_map(|l| {
                let (count, rest) = quote_markers(l);
                (count < depth).then_some((l, rest))
            })
            .collect();
        if let Some((l, _)) = lazy.iter().find(|(_, rest)| is_marker_only(rest)) {
            return Some(format!(
                "a line with only a list marker right after a blockquote ({:?}), which markdown-it reads as an empty list item",
                l.trim()
            ));
        }
        let lazy_indented = lazy.iter().any(|(l, rest)| {
            let markers = l.len() - rest.len();
            let (count, _) = quote_markers(l);
            markers > 2 * count
        });
        // A tab after a `>` keeps its columns in a span across the line, too.
        let tabbed = lines(t).iter().skip(1).any(|l| tab_after_quote(l));
        if !lazy_indented && !tabbed {
            return None;
        }
        let mut spans = false;
        n.walk(|c, _| {
            // A span's source map can stop at its first line, so with a tab any span counts.
            if (c.is::<CodeInline>() || c.is::<HtmlInline>())
                && (tabbed || text(c, source).is_some_and(|s| s.contains(['\n', '\r'])))
            {
                spans = true;
            }
        });
        return spans.then(|| {
            "a code span or inline HTML in a blockquote paragraph with an indented lazy line or a tab after a `>`".to_string()
        });
    }
    None
}

fn walk_quoted(n: &Node, source: &str, depth: usize, found: &mut Option<String>) {
    if found.is_some() {
        return;
    }
    if depth > 0
        && let Some(why) = quoted_difference(n, source, depth)
    {
        *found = Some(why);
        return;
    }
    let depth = depth + usize::from(n.is::<Blockquote>());
    for c in &n.children {
        walk_quoted(c, source, depth, found);
    }
}

/// Each line of `source` with the byte offsets where it starts and ends.
fn spans(source: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let b = source.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\n' || b[i] == b'\r' {
            out.push((start, i));
            if b[i] == b'\r' && b.get(i + 1) == Some(&b'\n') {
                i += 1;
            }
            start = i + 1;
        }
        i += 1;
    }
    out.push((start, b.len()));
    out
}

/// What the block parser made of the source: the ranges its leaf blocks cover (a line in
/// none of them was taken by a reference definition, or is blank), its code and HTML blocks,
/// and its blockquotes.
#[derive(Default)]
struct Blocks {
    leaves: Vec<(usize, usize)>,
    quotes: Vec<(usize, usize)>,
    /// HTML blocks: their text is kept as written, tabs included.
    html: Vec<(usize, usize)>,
    /// Fenced code blocks, without their opening line.
    fence_bodies: Vec<(usize, usize)>,
    /// Indented code blocks.
    code: Vec<(usize, usize)>,
}

fn is_container(n: &Node) -> bool {
    use markdown_it::parser::core::Root;
    use markdown_it::plugins::cmark::block::list::{BulletList, ListItem, OrderedList};
    n.is::<Root>()
        || n.is::<Blockquote>()
        || n.is::<BulletList>()
        || n.is::<OrderedList>()
        || n.is::<ListItem>()
}

fn blocks(ast: &Node) -> Blocks {
    fn walk(n: &Node, out: &mut Blocks) {
        let range = n.srcmap.map(|m| m.get_byte_offsets());
        if n.is::<Blockquote>()
            && let Some(r) = range
        {
            out.quotes.push(r);
        }
        if is_container(n) {
            for c in &n.children {
                walk(c, out);
            }
        } else if let Some(r) = range {
            use markdown_it::plugins::cmark::block::code::CodeBlock;
            use markdown_it::plugins::html::html_block::HtmlBlock;
            out.leaves.push(r);
            use markdown_it::plugins::cmark::block::fence::CodeFence;
            if n.is::<HtmlBlock>() {
                out.html.push(r);
            } else if n.is::<CodeBlock>() {
                out.code.push(r);
            } else if n.is::<CodeFence>() {
                out.fence_bodies.push((r.0 + 1, r.1));
            }
        }
    }
    let mut out = Blocks::default();
    walk(ast, &mut out);
    out
}

/// Whether the line at `a..b` starts inside one of `ranges` (a fence's body, after its
/// opening line's first character).
fn touches_body(ranges: &[(usize, usize)], (a, b): (usize, usize)) -> bool {
    ranges
        .iter()
        .any(|&(s, e)| s <= a && e >= b && a > 0 && s < a)
}

fn touches(ranges: &[(usize, usize)], (a, b): (usize, usize)) -> bool {
    ranges.iter().any(|&(s, e)| s <= b && e >= a)
}

/// A reference definition's destination: the first word after `]:`, or of the next line.
fn destination<'a>(first: &'a str, next: Option<&'a str>) -> Option<&'a str> {
    let after = first.split_once("]:")?.1;
    match after.split_whitespace().next() {
        Some(d) => Some(d),
        None => next?
            .trim_start_matches(['>', ' ', '\t'])
            .split_whitespace()
            .next(),
    }
}

/// What Brrrty can't render exactly in `ast` (parsed from `source`), if anything.
///
/// The checks look at what the block parser made: lines inside a code or HTML block, or
/// a list item's or quote's markers, aren't taken for Markdown they aren't.
pub(crate) fn check(ast: &Node, source: &str) -> Result<(), String> {
    use markdown_it::plugins::cmark::block::paragraph::Paragraph;
    let blocks = blocks(ast);
    let line_spans = spans(source);
    let line = |(a, b): (usize, usize)| &source[a..b];
    // A definition the port took where markdown-it may not: `[label]:` with only a list
    // marker on the next line, or a destination ending in `\`.
    let mut found: Option<String> = None;
    ast.walk(|n, _| {
        use markdown_it::plugins::cmark::block::reference::Definition;
        if found.is_some() || !n.is::<Definition>() {
            return;
        }
        let Some(t) = text(n, source) else { return };
        let ls = lines(t);
        let first = ls[0].trim_end_matches([' ', '\t']);
        let next = ls.get(1).map(|l| l.trim_start_matches(['>', ' ', '\t']));
        if (first.ends_with("]:") && next.is_some_and(is_marker_only))
            || destination(first, next).is_some_and(|d| d.ends_with('\\'))
        {
            found = Some(format!(
                "a link reference definition whose destination ends in `\\`, or with only a list marker on the next line, which markdown-it and the port read differently: {first:?}"
            ));
        }
    });
    if let Some(why) = found {
        return Err(why);
    }
    // A paragraph markdown-it takes for a definition: one whose destination ends in `\`.
    ast.walk(|n, _| {
        if found.is_some()
            || !(n.is::<Paragraph>() || n.is::<crate::core_rules::InlineGroup>())
        {
            return;
        }
        let Some(t) = text(n, source) else { return };
        let ls = lines(t);
        if !starts_like_definition(ls[0]) {
            return;
        }
        if destination(ls[0], ls.get(1).copied()).is_some_and(|d| d.ends_with('\\')) {
            found = Some(format!(
                "a link reference definition whose destination ends in `\\`, which markdown-it and the port read differently: {:?}",
                ls[0].trim()
            ));
        }
    });
    if let Some(why) = found {
        return Err(why);
    }
    // A tab after a quote's `>`, on a line in a blockquote.
    if let Some(&span) = line_spans.iter().find(|&&span| {
        let l = line(span);
        let p = prefix(l);
        // Where markdown-it keeps a partly used tab as written, and the port writes
        // spaces or counts columns differently: an HTML block's or a fence's text after
        // `>\t` or under two or more `>`, indented code under two or more `>`, and a list
        // item after a tab between two `>`.
        let nested = p.matches('>').count() >= 2;
        let last = p.rfind('>');
        // The whitespace after the last `>`, and what follows it.
        let after_last = last.map_or("", |i| &l[i + 1..]);
        let ws_after =
            &after_last[..after_last.len() - after_last.trim_start_matches([' ', '\t']).len()];
        // Under two or more `>`, any tab among the markers after the second one: the port
        // counts nested columns differently (code, list items, fences, tables).
        let nested_tab = p
            .match_indices('>')
            .nth(1)
            .is_some_and(|(i, _)| p[i..].contains('\t'));
        // markdown-it uses a tab right after `>` as its one optional space, and keeps it as
        // written, unless the tab starts at a column of 3 modulo 4.
        // Any `>` in the markers (in an HTML block, later ones are text).
        let kept_tab = p.char_indices().any(|(i, c)| {
            c == '>'
                && p[i + 1..].starts_with('\t')
                && l[..i + 1].chars().fold(0, |col, c| {
                    if c == '\t' {
                        col + 4 - col % 4
                    } else {
                        col + 1
                    }
                }) % 4
                    != 3
        });
        let html_nested_indent = touches(&blocks.html, span) && nested && ws_after.len() >= 2;
        touches(&blocks.quotes, span)
            && (tab_after_quote(l) || html_nested_indent)
            && ((touches(&blocks.html, span)
                && (kept_tab || (nested && (ws_after.contains('\t') || ws_after.len() >= 2))))
                || (touches_body(&blocks.fence_bodies, span) && (p.contains(">\t") || nested))
                || nested_tab)
    }) {
        return Err(format!(
            "a tab after a blockquote's `>` (the port counts its columns differently): {:?}",
            line(span).trim_end()
        ));
    }
    walk_quoted(ast, source, 0, &mut found);
    ast.walk(|n, _| {
        if found.is_some() {
            return;
        }
        for pair in n.children.windows(2) {
            let (q, t) = (&pair[0], &pair[1]);
            if !(q.is::<Blockquote>() && t.is::<Table>() && ends_in_paragraph(q)) {
                continue;
            }
            // An empty `>` line after the paragraph ends it, for markdown-it too.
            let para_end = last_paragraph_end(q);
            if para_end.is_some_and(|e| {
                q.srcmap
                    .is_some_and(|m| source[e..m.get_byte_offsets().1].contains(['\n', '\r']))
            }) {
                continue;
            }
            let (Some(qm), Some(tm)) = (q.srcmap, t.srcmap) else { continue };
            let between = source
                .get(qm.get_byte_offsets().1..tm.get_byte_offsets().0)
                .unwrap_or("");
            if lines(between).len() <= 2
                && between.trim_matches([' ', '\t', '\r', '\n', '>']).is_empty()
            {
                found = Some(
                    "a table right after a blockquote's paragraph, which markdown-it reads as that paragraph's text".into(),
                );
                return;
            }
        }
    });
    found.map_or(Ok(()), Err)
}
