//! String filters.

use brrrty_js::{Value, is_js_whitespace};

use super::{
    Args, Out, array, is_nil, js_string, string, stringify, text, throw, to_array, utf16_len,
};
use crate::context::Val;
use crate::ops;
use crate::render::Renderer;

// ---- JavaScript string helpers ---------------------------------------------

/// `str.split(/\s+/)`.
fn split_whitespace_runs(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_run = false;
    let mut run_start = 0;
    for (i, c) in s.char_indices() {
        if is_js_whitespace(c) {
            if !in_run {
                in_run = true;
                run_start = i;
            }
        } else if in_run {
            parts.push(&s[start..run_start]);
            start = i;
            in_run = false;
        }
    }
    if in_run {
        parts.push(&s[start..run_start]);
        parts.push("");
    } else {
        parts.push(&s[start..]);
    }
    parts
}

/// `str.replace(/\s+/g, with)`.
fn collapse_whitespace(s: &str, with: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_run = false;
    for c in s.chars() {
        if is_js_whitespace(c) {
            if !in_run {
                out.push_str(with);
                in_run = true;
            }
        } else {
            in_run = false;
            out.push(c);
        }
    }
    out
}

/// `str.split('')`: UTF-16 units. A surrogate pair becomes two replacement
/// characters, as writing the halves to a file does.
fn utf16_units(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    for c in s.chars() {
        if c.len_utf16() == 2 {
            out.push("\u{FFFD}".to_string());
            out.push("\u{FFFD}".to_string());
        } else {
            out.push(c.to_string());
        }
    }
    out
}

/// How many code points `advanceCodePoints(str, 0, count)` steps over in a
/// string of `cp_len` code points. `count` is whatever JavaScript's
/// `i < count` makes of it.
fn advance(count: f64, cp_len: usize) -> usize {
    if count.is_nan() || count <= 0.0 {
        0
    } else if count >= cp_len as f64 {
        cp_len
    } else {
        count.ceil() as usize
    }
}

fn chars_set(s: &str) -> Vec<char> {
    s.chars().collect()
}

// ---- concatenation -----------------------------------------------------------

/// `lhs` and `rhs` joined, with the size checked before anything is built.
fn concat(r: &Renderer<'_>, lhs: &str, rhs: &str) -> Out {
    r.check_bytes(lhs.len().saturating_add(rhs.len()))?;
    let mut out = String::with_capacity(lhs.len() + rhs.len());
    out.push_str(lhs);
    out.push_str(rhs);
    Ok(string(out))
}

pub(super) fn append(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    if a.len() != 1 {
        return Err(throw("append expect 2 arguments"));
    }
    let (lhs, rhs) = (v.to_value(), a[0].to_value());
    concat(r, &text(r, &lhs), &text(r, &rhs))
}

pub(super) fn prepend(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    if a.len() != 1 {
        return Err(throw("prepend expect 2 arguments"));
    }
    let (lhs, rhs) = (v.to_value(), a[0].to_value());
    concat(r, &text(r, &rhs), &text(r, &lhs))
}

// ---- trimming and case ------------------------------------------------------------

fn trim_with(r: &Renderer<'_>, v: &Val, a: &[Val], start: bool, end: bool) -> Out {
    let s = stringify(r, &v.to_value());
    let chars = Args(a).value(0);
    if !chars.js_truthy() {
        let t = match (start, end) {
            (true, false) => s.trim_start_matches(is_js_whitespace),
            (false, true) => s.trim_end_matches(is_js_whitespace),
            _ => s.trim_matches(is_js_whitespace),
        };
        return Ok(string(t));
    }
    let set = chars_set(&stringify(r, &chars));
    let mut t = s.as_str();
    if start {
        t = t.trim_start_matches(|c| set.contains(&c));
    }
    if end {
        t = t.trim_end_matches(|c| set.contains(&c));
    }
    Ok(string(t))
}

pub(super) fn lstrip(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    trim_with(r, v, a, true, false)
}

pub(super) fn rstrip(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    trim_with(r, v, a, false, true)
}

pub(super) fn strip(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    trim_with(r, v, a, true, true)
}

pub(super) fn downcase(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    Ok(string(stringify(r, &v.to_value()).to_lowercase()))
}

pub(super) fn upcase(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    Ok(string(stringify(r, &v.to_value()).to_uppercase()))
}

pub(super) fn capitalize(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    let mut chars = s.chars();
    let mut out = String::with_capacity(s.len());
    if let Some(first) = chars.next() {
        out.extend(first.to_uppercase());
        out.push_str(&chars.as_str().to_lowercase());
    }
    Ok(string(out))
}

pub(super) fn strip_newlines(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    Ok(string(s.replace("\r\n", "").replace('\n', "")))
}

pub(super) fn squish(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    Ok(string(
        collapse_whitespace(&s, " ").trim_matches(is_js_whitespace),
    ))
}

pub(super) fn normalize_whitespace(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    Ok(string(collapse_whitespace(&s, " ")))
}

// ---- removing and replacing ---------------------------------------------------------

pub(super) fn remove(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    let pattern = stringify(r, &Args(a).value(0));
    Ok(string(if pattern.is_empty() {
        s
    } else {
        s.replace(&pattern, "")
    }))
}

pub(super) fn remove_first(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    let pattern = stringify(r, &Args(a).value(0));
    Ok(string(s.replacen(&pattern, "", 1)))
}

fn replace_last_in(s: &str, pattern: &str, replacement: &str) -> String {
    match s.rfind(pattern) {
        Some(at) => format!("{}{replacement}{}", &s[..at], &s[at + pattern.len()..]),
        None => s.to_string(),
    }
}

pub(super) fn remove_last(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    let pattern = stringify(r, &Args(a).value(0));
    Ok(string(replace_last_in(&s, &pattern, "")))
}

pub(super) fn replace(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let a = Args(a);
    let s = stringify(r, &v.to_value());
    let pattern = stringify(r, &a.value(0));
    let replacement = stringify(r, &a.value(1));
    if pattern.is_empty() {
        if replacement.is_empty() {
            // The units join back into the string.
            return Ok(string(s));
        }
        // `split('')` cuts between UTF-16 units; the replacement goes
        // between all of them.
        let parts = utf16_units(&s);
        let size = s.len()
            + parts
                .len()
                .saturating_sub(1)
                .saturating_mul(replacement.len());
        r.check_bytes(size)?;
        return Ok(string(parts.join(&replacement)));
    }
    let count = s.matches(&pattern).count();
    let size =
        (s.len() + count.saturating_mul(replacement.len())).saturating_sub(count * pattern.len());
    r.check_bytes(size)?;
    Ok(string(s.replace(&pattern, &replacement)))
}

pub(super) fn replace_first(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let a = Args(a);
    let s = stringify(r, &v.to_value());
    let pattern = stringify(r, &a.value(0));
    let replacement = stringify(r, &a.value(1));
    r.check_bytes(s.len().saturating_add(replacement.len()))?;
    Ok(string(s.replacen(&pattern, &replacement, 1)))
}

pub(super) fn replace_last(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let a = Args(a);
    let s = stringify(r, &v.to_value());
    let pattern = stringify(r, &a.value(0));
    let replacement = stringify(r, &a.value(1));
    r.check_bytes(s.len().saturating_add(replacement.len()))?;
    Ok(string(replace_last_in(&s, &pattern, &replacement)))
}

pub(super) fn split(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    let sep = stringify(r, &Args(a).value(0));
    let mut parts: Vec<String> = if sep.is_empty() {
        utf16_units(&s)
    } else {
        s.split(&sep).map(str::to_string).collect()
    };
    // Align to Ruby's split: trailing empty strings go.
    while parts.last().is_some_and(String::is_empty) {
        parts.pop();
    }
    Ok(array(parts.into_iter().map(string).collect()))
}

// ---- truncation --------------------------------------------------------------------------

pub(super) fn truncate(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let a = Args(a);
    let input = v.to_value();
    let s = stringify(r, &input);
    let l = a.or(0, Value::Number(50.0));
    let o = stringify(r, &a.or(1, string("...")));
    let ln = l.to_js_number();
    let cp_len = s.chars().count();
    if utf16_len(&s) as f64 <= ln || advance(ln, cp_len) >= cp_len {
        return Ok(input);
    }
    let room = ln - o.chars().count() as f64;
    let keep = advance(
        if room.is_nan() {
            f64::NAN
        } else {
            room.max(0.0)
        },
        cp_len,
    );
    let mut out: String = s.chars().take(keep).collect();
    out.push_str(&o);
    Ok(string(out))
}

pub(super) fn truncatewords(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let a = Args(a);
    let s = stringify(r, &v.to_value());
    let mut words = a.or(0, Value::Number(15.0)).to_js_number();
    let o = stringify(r, &a.or(1, string("...")));
    let parts = split_whitespace_runs(s.trim_start_matches(is_js_whitespace));
    if words <= 0.0 {
        words = 1.0;
    }
    let (from, to) = ops::slice_bounds(parts.len(), 0.0, Some(words));
    let mut ret = parts[from..to].join(" ");
    if parts.len() as f64 >= words {
        ret.push_str(&o);
    }
    Ok(string(ret))
}

// ---- counting ---------------------------------------------------------------------------------

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{3040}'..='\u{309F}'
        | '\u{30A0}'..='\u{30FF}'
        | '\u{AC00}'..='\u{D7AF}')
}

pub(super) fn number_of_words(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let s = stringify(r, &v.to_value());
    let input = s.trim_matches(is_js_whitespace);
    if input.is_empty() {
        return Ok(Value::Number(0.0));
    }
    let cjk = input.chars().filter(|c| is_cjk(*c)).count();
    let runs = || {
        let mut n = 0;
        let mut in_run = false;
        for c in input.chars() {
            if is_cjk(c) || is_js_whitespace(c) {
                in_run = false;
            } else if !in_run {
                in_run = true;
                n += 1;
            }
        }
        n
    };
    let words = || split_whitespace_runs(input).len();
    let mode = match Args(a).value(0) {
        Value::String(m) => m.to_string(),
        _ => String::new(),
    };
    let n = match mode.as_str() {
        "cjk" => cjk + runs(),
        "auto" if cjk > 0 => cjk + runs(),
        _ => words(),
    };
    Ok(Value::Number(n as f64))
}

pub(super) fn array_to_sentence_string(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let items = to_array(&v.to_value());
    let connector = stringify(r, &Args(a).or(0, string("and")));
    // Template literals use `String(x)`; `join` writes null and undefined
    // as nothing.
    let shown = |x: &Value| js_string(r, x);
    let joined = |x: &Value| if is_nil(x) { String::new() } else { shown(x) };
    Ok(match items.len() {
        0 => string(""),
        1 => items[0].clone(),
        2 => string(format!(
            "{} {connector} {}",
            shown(&items[0]),
            shown(&items[1])
        )),
        n => {
            let head: Vec<String> = items[..n - 1].iter().map(joined).collect();
            string(format!(
                "{}, {connector} {}",
                head.join(", "),
                shown(&items[n - 1])
            ))
        }
    })
}
