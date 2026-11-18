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

/// Why [`stringify`] stopped before the end.
#[derive(Debug, PartialEq)]
pub(super) enum Stop {
    /// It met a value 11ty's `JSON.stringify` can't write (a collection
    /// item holds 11ty's template object, which is circular). The trap is
    /// recorded, so the page is refused; going on would only walk the
    /// items' shared data, which can be exponential in size.
    Trap,
    /// The text grew past the output limit.
    Limit,
}

/// `Ok(None)` is `undefined`, which `JSON.stringify` returns for a value it
/// cannot write. Stops as soon as the text passes `limit` bytes.
pub(super) fn stringify(value: &Value, indent: &str, limit: usize) -> Result<Option<String>, Stop> {
    let mut out = String::new();
    let mut w = Writer {
        indent,
        limit,
        out: &mut out,
    };
    let wrote = w.write(value, 0)?;
    Ok(wrote.then_some(out))
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

struct Writer<'a> {
    indent: &'a str,
    limit: usize,
    out: &'a mut String,
}

impl Writer<'_> {
    fn check(&self) -> Result<(), Stop> {
        if self.out.len() > self.limit {
            return Err(Stop::Limit);
        }
        Ok(())
    }

    /// Writes `value`; false when it is `undefined` (nothing was written).
    fn write(&mut self, value: &Value, depth: usize) -> Result<bool, Stop> {
        let indent = self.indent;
        match value {
            // JSON.stringify leaves functions out, as it does undefined.
            Value::Undefined | Value::Function(_) => return Ok(false),
            Value::Null => self.out.push_str("null"),
            Value::Bool(b) => self.out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) if n.is_finite() => self.out.push_str(&js_number_to_string(*n)),
            Value::Number(_) => self.out.push_str("null"),
            Value::String(s) => quote(s, self.out),
            Value::Date(d) => quote(&iso(d), self.out),
            Value::Array(items) => {
                if items.is_empty() {
                    self.out.push_str("[]");
                    return Ok(true);
                }
                self.out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.out.push(',');
                    }
                    newline(indent, depth + 1, self.out);
                    if !grow(|| self.write(item, depth + 1))? {
                        self.out.push_str("null");
                    }
                    self.check()?;
                }
                newline(indent, depth, self.out);
                self.out.push(']');
            }
            Value::Object(o) => {
                let entries = Value::js_entries_or_trap(o).ok_or(Stop::Trap)?;
                let mut wrote_any = false;
                self.out.push('{');
                for (k, v) in entries {
                    let mark = self.out.len();
                    if wrote_any {
                        self.out.push(',');
                    }
                    newline(indent, depth + 1, self.out);
                    quote(k, self.out);
                    self.out.push(':');
                    if !indent.is_empty() {
                        self.out.push(' ');
                    }
                    if grow(|| self.write(v, depth + 1))? {
                        wrote_any = true;
                    } else {
                        self.out.truncate(mark);
                    }
                    self.check()?;
                }
                if wrote_any {
                    newline(indent, depth, self.out);
                }
                self.out.push('}');
            }
        }
        self.check()?;
        Ok(true)
    }
}
