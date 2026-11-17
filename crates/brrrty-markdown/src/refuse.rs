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
    let end = line
        .find(|c: char| {
            !matches!(
                c,
                '>' | ' ' | '\t' | '-' | '+' | '*' | '.' | ')' | '0'..='9'
            )
        })
        .unwrap_or(line.len());
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
        if !lazy_indented {
            return None;
        }
        let mut spans = false;
        n.walk(|c, _| {
            if (c.is::<CodeInline>() || c.is::<HtmlInline>())
                && text(c, source).is_some_and(|s| s.contains(['\n', '\r']))
            {
                spans = true;
            }
        });
        return spans.then(|| {
            "a code span or inline HTML across an indented lazy line of a blockquote".to_string()
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

/// What Brrrty can't render exactly in `ast` (parsed from `source`), if anything.
pub(crate) fn check(ast: &Node, source: &str) -> Result<(), String> {
    let all = lines(source);
    if let Some(l) = all.iter().enumerate().find_map(|(i, l)| {
        let rest = l[prefix(l).len()..].trim_end_matches([' ', '\t']);
        let next_is_marker = all.get(i + 1).is_some_and(|n| {
            is_marker_only(&n[n.len() - n.trim_start_matches(['>', ' ', '\t']).len()..])
        });
        let next_has_backslash = all.get(i + 1).is_some_and(|n| n.contains('\\'));
        let after_label = rest.split_once("]:").map_or("", |(_, r)| r);
        (starts_like_definition(rest)
            && (after_label.contains('\\')
                || (rest.ends_with("]:") && (next_is_marker || next_has_backslash))))
            .then_some(*l)
    }) {
        return Err(format!(
            "a link reference definition with a `\\` in its destination, or one on the next line, which markdown-it and the port read differently: {:?}",
            l.trim()
        ));
    }
    if let Some(l) = lines(source).into_iter().find(|l| tab_after_quote(l)) {
        return Err(format!(
            "a tab after a blockquote's `>` (the port counts its columns differently): {:?}",
            l.trim_end()
        ));
    }
    let mut found: Option<String> = None;
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
            let (Some(qm), Some(tm)) = (q.srcmap, t.srcmap) else { continue };
            let between = source
                .get(qm.get_byte_offsets().1..tm.get_byte_offsets().0)
                .unwrap_or("");
            if lines(between).len() <= 2 && between.trim_matches([' ', '\t', '\r', '\n']).is_empty() {
                found = Some(
                    "a table right after a blockquote's paragraph, which markdown-it reads as that paragraph's text".into(),
                );
                return;
            }
        }
    });
    found.map_or(Ok(()), Err)
}
