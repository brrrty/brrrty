//! URL filters: `encodeURIComponent` and friends.

use std::sync::OnceLock;

use brrrty_js::Value;
use regex::Regex;

use super::{Args, Out, js_string, string, stringify, throw};
use crate::context::Val;
use crate::render::Renderer;

/// `encodeURIComponent` leaves these alone, besides ASCII letters and digits.
const COMPONENT_SAFE: &str = "-_.!~*'()";
/// `encodeURI` also leaves the characters that separate URL parts.
const URI_SAFE: &str = "-_.!~*'();/?:@&=+$,#";

fn encode(s: &str, safe: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut buf = [0u8; 4];
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || (c.is_ascii() && safe.contains(c)) {
            out.push(c);
        } else {
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// `decodeURIComponent`. A bad escape or bad UTF-8 is a `URIError`.
fn decode_component(s: &str) -> Result<String, ()> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'%' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let hex = |at: usize| -> Result<u8, ()> {
            let h = b.get(at..at + 2).ok_or(())?;
            u8::from_str_radix(std::str::from_utf8(h).map_err(|_| ())?, 16).map_err(|_| ())
        };
        // `from_str_radix` accepts a leading `+`, which `%+1` must not.
        if b.get(i + 1) == Some(&b'+') || b.get(i + 2) == Some(&b'+') {
            return Err(());
        }
        out.push(hex(i + 1)?);
        i += 3;
    }
    String::from_utf8(out).map_err(|_| ())
}

pub(super) fn url_decode(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let s = stringify(r, &v.to_value()).replace('+', " ");
    decode_component(&s)
        .map(string)
        .map_err(|()| throw("URI malformed"))
}

pub(super) fn url_encode(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    Ok(string(encode(&s, COMPONENT_SAFE).replace("%20", "+")))
}

pub(super) fn cgi_escape(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    // What `encodeURIComponent` leaves alone but CGI escaping does not.
    let mut out = String::new();
    for c in encode(&s, COMPONENT_SAFE).replace("%20", "+").chars() {
        if matches!(c, '!' | '\'' | '(' | ')' | '*') {
            out.push_str(&format!("%{:X}", c as u32));
        } else {
            out.push(c);
        }
    }
    Ok(string(out))
}

pub(super) fn uri_escape(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    Ok(string(
        encode(&s, URI_SAFE).replace("%5B", "[").replace("%5D", "]"),
    ))
}

/// JavaScript's `\s`.
const JS_SPACE: &str = "\\t\\n\\x0B\\x0C\\r \\u{A0}\\u{1680}\\u{2000}-\\u{200A}\\u{2028}\\u{2029}\\u{202F}\\u{205F}\\u{3000}\\u{FEFF}";

fn replacers() -> &'static [(&'static str, Regex)] {
    static R: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    R.get_or_init(|| {
        let re = |p: &str| Regex::new(p).unwrap();
        vec![
            ("raw", re(&format!("[{JS_SPACE}]+"))),
            ("default", re(r"[^\p{M}\p{L}\p{Nd}]+")),
            ("pretty", re(r"[^\p{M}\p{L}\p{Nd}._~!$&'()+,;=@]+")),
            ("ascii", re(r"[^A-Za-z0-9]+")),
            ("latin", re(r"[^\p{M}\p{L}\p{Nd}]+")),
        ]
    })
}

fn remove_accents(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => out.push('a'),
            'æ' => out.push_str("ae"),
            'ç' => out.push('c'),
            'è' | 'é' | 'ê' | 'ë' => out.push('e'),
            'ì' | 'í' | 'î' | 'ï' => out.push('i'),
            'ð' => out.push('d'),
            'ñ' => out.push('n'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => out.push('o'),
            'ù' | 'ú' | 'û' | 'ü' => out.push('u'),
            'ý' | 'ÿ' => out.push('y'),
            'ß' => out.push_str("ss"),
            'œ' => out.push_str("oe"),
            'þ' => out.push_str("th"),
            'ẞ' => out.push_str("SS"),
            'Œ' => out.push_str("OE"),
            'Þ' => out.push_str("TH"),
            c => out.push(c),
        }
    }
    out
}

/// `slugify(str, mode = 'default', cased = false)`.
pub(super) fn slugify(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let a = Args(a);
    let mut s = stringify(r, &v.to_value());
    let mode = match a.value(0) {
        Value::Undefined => "default".to_string(),
        m => js_string(r, &m),
    };
    let cased = a.value(1).js_truthy();
    if let Some((_, re)) = replacers().iter().find(|(name, _)| *name == mode) {
        if mode == "latin" {
            s = remove_accents(&s);
        }
        s = re.replace_all(&s, "-").into_owned();
        // `.replace(/^-|-$/g, '')`: one dash from each end.
        let mut t = s.as_str();
        if let Some(rest) = t.strip_prefix('-') {
            t = rest;
        }
        if let Some(rest) = t.strip_suffix('-') {
            t = rest;
        }
        s = t.to_string();
    }
    Ok(string(if cased { s } else { s.to_lowercase() }))
}
