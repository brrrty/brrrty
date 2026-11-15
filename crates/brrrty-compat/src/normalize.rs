//! Conservative markup normalization.

const RAW_ELEMENTS: [&str; 4] = ["pre", "textarea", "script", "style"];

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

/// Find the end (exclusive) of the tag starting at `start` (a `<`), honoring quoted attribute values.
fn tag_end(b: &[u8], start: usize) -> usize {
    let mut quote: Option<u8> = None;
    let mut i = start + 1;
    while i < b.len() {
        match (quote, b[i]) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b[i]),
            (None, b'>') => return i + 1,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// Lowercased tag name of an opening tag at `start`, if it is a plain opening tag.
fn open_tag_name(b: &[u8], start: usize) -> Option<String> {
    let rest = &b[start + 1..];
    if !rest.first()?.is_ascii_alphabetic() {
        return None;
    }
    let len = rest
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || **c == b'-')
        .count();
    Some(String::from_utf8_lossy(&rest[..len]).to_ascii_lowercase())
}

/// Index of the case-insensitive `</name` at or after `from`, or the end of input.
fn find_close(b: &[u8], from: usize, name: &str) -> usize {
    let pat = format!("</{name}").into_bytes();
    let mut i = from;
    while i + pat.len() <= b.len() {
        if b[i..i + pat.len()].eq_ignore_ascii_case(&pat) {
            return i;
        }
        i += 1;
    }
    b.len()
}

/// Collapse each run of ASCII whitespace in markup text to one space.
///
/// Tags, attributes, comments, the doctype, and the contents of `pre`,
/// `textarea`, `script` and `style` elements are kept byte-for-byte.
/// Whitespace is never removed entirely.
pub fn normalize_markup(input: &str) -> String {
    let b = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            if b[i..].starts_with(b"<!--") {
                let end = input[i + 4..]
                    .find("-->")
                    .map_or(b.len(), |p| i + 4 + p + 3);
                out.push_str(&input[i..end]);
                i = end;
                continue;
            }
            let next = b.get(i + 1).copied().unwrap_or(0);
            if next.is_ascii_alphabetic() || matches!(next, b'/' | b'!' | b'?') {
                let end = tag_end(b, i);
                out.push_str(&input[i..end]);
                if let Some(name) = open_tag_name(b, i)
                    && RAW_ELEMENTS.contains(&name.as_str())
                    && b[end - 1] == b'>'
                    && b.get(end.wrapping_sub(2)) != Some(&b'/')
                {
                    let close = find_close(b, end, &name);
                    out.push_str(&input[end..close]);
                    i = close;
                } else {
                    i = end;
                }
                continue;
            }
        }
        if is_ws(b[i]) {
            while i < b.len() && is_ws(b[i]) {
                i += 1;
            }
            out.push(' ');
        } else {
            let start = i;
            while i < b.len() && !is_ws(b[i]) && b[i] != b'<' {
                i += 1;
            }
            if i == start {
                i += 1; // lone '<' that does not start a tag
            }
            out.push_str(&input[start..i]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collapses_text_whitespace_but_never_removes_it() {
        assert_eq!(
            normalize_markup("<p>a\n  b</p>\n\n<p>c</p>\n"),
            "<p>a b</p> <p>c</p> "
        );
    }
    #[test]
    fn pre_is_untouched() {
        let s = "<pre><code>a\n  b</code></pre>";
        assert_eq!(normalize_markup(s), s);
    }
    #[test]
    fn script_and_style_are_untouched_even_with_lt() {
        let s = "<script>if (a  <  b) {\n}</script><STYLE>a  {}</STYLE>";
        assert_eq!(normalize_markup(s), s);
    }
    #[test]
    fn comments_and_attributes_are_untouched() {
        let s = "<!--  x  y  --><a  href=\"x  y\">t</a>";
        assert_eq!(normalize_markup(s), s);
    }
}
