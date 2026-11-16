//! `JSON.stringify(value, replacer, space)`.

use brrrty_js::{Value, js_number_to_string};

use crate::lexer::grow;

/// The indent `JSON.stringify` makes of its `space` argument: a number of
/// spaces up to ten, or the first ten units of a string.
pub(super) fn indent_of(space: &Value) -> String {
    match space {
        Value::Number(n) => {
            let count = if n.is_nan() { 0.0 } else { n.trunc().min(10.0) };
            " ".repeat(count.max(0.0) as usize)
        }
        Value::String(s) => {
            let units: Vec<u16> = s.encode_utf16().take(10).collect();
            String::from_utf16_lossy(&units)
        }
        _ => String::new(),
    }
}

/// `None` is `undefined`, which `JSON.stringify` returns for a value it
/// cannot write.
pub(super) fn stringify(value: &Value, indent: &str) -> Option<String> {
    let mut out = String::new();
    write(value, indent, 0, &mut out).then_some(out)
}

pub(crate) fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn newline(indent: &str, depth: usize, out: &mut String) {
    if !indent.is_empty() {
        out.push('\n');
        for _ in 0..depth {
            out.push_str(indent);
        }
    }
}

/// `Date.prototype.toISOString`.
fn iso(d: &chrono::DateTime<chrono::FixedOffset>) -> String {
    use chrono::Datelike;
    let utc = d.with_timezone(&chrono::Utc);
    let year = utc.year();
    let rest = utc.format("-%m-%dT%H:%M:%S%.3fZ");
    if (0..=9999).contains(&year) {
        format!("{year:04}{rest}")
    } else {
        format!(
            "{}{:06}{rest}",
            if year < 0 { '-' } else { '+' },
            year.abs()
        )
    }
}

/// Writes `value`; false when it is `undefined` (nothing was written).
fn write(value: &Value, indent: &str, depth: usize, out: &mut String) -> bool {
    match value {
        Value::Undefined => return false,
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) if n.is_finite() => out.push_str(&js_number_to_string(*n)),
        Value::Number(_) => out.push_str("null"),
        Value::String(s) => quote(s, out),
        Value::Date(d) => quote(&iso(d), out),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return true;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(indent, depth + 1, out);
                if !grow(|| write(item, indent, depth + 1, out)) {
                    out.push_str("null");
                }
            }
            newline(indent, depth, out);
            out.push(']');
        }
        Value::Object(o) => {
            let mut wrote_any = false;
            out.push('{');
            for (k, v) in Value::js_entries(o) {
                let mark = out.len();
                if wrote_any {
                    out.push(',');
                }
                newline(indent, depth + 1, out);
                quote(k, out);
                out.push(':');
                if !indent.is_empty() {
                    out.push(' ');
                }
                if grow(|| write(v, indent, depth + 1, out)) {
                    wrote_any = true;
                } else {
                    out.truncate(mark);
                }
            }
            if wrote_any {
                newline(indent, depth, out);
            }
            out.push('}');
        }
    }
    true
}
