//! HTML filters.

use brrrty_js::Value;

use super::{Out, string, stringify};
use crate::context::Val;
use crate::render::Renderer;

fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// `&(amp|lt|gt|#34|#39);` back to its character, in one pass.
fn unescape_str(s: &str) -> String {
    const ENTITIES: [(&str, char); 5] = [
        ("&amp;", '&'),
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&#34;", '"'),
        ("&#39;", '\''),
    ];
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        match ENTITIES.iter().find(|(e, _)| rest.starts_with(e)) {
            Some((e, c)) => {
                out.push(*c);
                rest = &rest[e.len()..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

pub(super) fn escape(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    Ok(string(escape_str(&stringify(r, &v.to_value()))))
}

pub(super) fn xml_escape(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    escape(r, v, a)
}

pub(super) fn escape_once(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    Ok(string(escape_str(&unescape_str(&stringify(
        r,
        &v.to_value(),
    )))))
}

/// `\r?\n` to `<br />\n`.
pub(super) fn newline_to_br(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                out.push_str("<br />\n");
            }
            '\n' => out.push_str("<br />\n"),
            c => out.push(c),
        }
    }
    Ok(Value::String(out.into()))
}

/// Removes tags, and the contents of `<script>`, `<style>` and comments.
/// An opener with no closer is dropped from the list and read as a plain
/// `<` from then on, as LiquidJS does.
pub(super) fn strip_html(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    let mut blocks: Vec<(&str, &str, bool)> = vec![
        ("<script", "</script>", true),
        ("<style", "</style>", true),
        ("<!--", "-->", true),
        ("<", ">", true),
    ];
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        let Some(lt) = s[i..].find('<').map(|at| at + i) else {
            out.push_str(&s[i..]);
            return Ok(string(out));
        };
        out.push_str(&s[i..lt]);
        for block in blocks.iter_mut() {
            let (opener, closer, alive) = *block;
            if !alive || !s[lt..].starts_with(opener) {
                continue;
            }
            match s[lt + opener.len()..].find(closer) {
                Some(e) => {
                    i = lt + opener.len() + e + closer.len();
                    break;
                }
                None => block.2 = false,
            }
        }
        if i <= lt {
            out.push_str(&s[lt..]);
            return Ok(string(out));
        }
    }
    Ok(string(out))
}
