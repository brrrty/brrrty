//! Eleventy 3.1.8's own filters: `slug`, `slugify`, `url` and `log`.
//!
//! They are engine-neutral: each is a [`FilterImpl`], a plain function from
//! the input and the positional arguments to a result. An engine adapts
//! them into its own filter type. `url` uses a path prefix of `/`.
//!
//! Ported from `Filters/Slug.js`, `Filters/Slugify.js` and `Filters/Url.js`
//! in `@11ty/eleventy`, and from the packages they call: `slugify` 1.6.9,
//! `@sindresorhus/slugify` 2.2.1 and `@sindresorhus/transliterate`. Their
//! data tables are in `eleventy_tables.rs`, which `tools/gen-eleventy.mjs`
//! generates from those packages.

use std::sync::Arc;

use unicode_normalization::UnicodeNormalization;

use crate::eleventy_tables::{DIACRITICS, SLUG_CHARMAP, TRANSLITERATE};
use crate::{Value, is_js_whitespace, js_number_to_string};

/// A filter that does not depend on a template engine.
pub type FilterImpl = fn(&Value, &[Value]) -> Result<Value, String>;

/// Eleventy's filters, by name.
pub fn eleventy_filters() -> Vec<(&'static str, FilterImpl)> {
    vec![
        ("slug", slug_filter),
        ("slugify", slugify_filter),
        ("url", url_filter),
        ("log", log_filter),
    ]
}

fn string(s: String) -> Value {
    Value::String(Arc::from(s))
}

// ---- slug: slugify 1.6.9 with `{ replacement: "-", lower: true }` ----

fn slug_filter(input: &Value, _args: &[Value]) -> Result<Value, String> {
    Ok(string(slug(&input.to_js_string())))
}

/// Characters slugify keeps: `[\w\s$*_+~.()'"!\-:@]`.
fn slug_allowed(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || is_js_whitespace(c)
        || matches!(
            c,
            '_' | '$' | '*' | '+' | '~' | '.' | '(' | ')' | '\'' | '"' | '!' | '-' | ':' | '@'
        )
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    // JS splits into UTF-16 units; a character above the BMP is two lone
    // surrogates, neither in the map, and both removed. So it is dropped.
    for c in text.nfc() {
        if c as u32 > 0xFFFF {
            continue;
        }
        let mapped = SLUG_CHARMAP
            .binary_search_by_key(&c, |(k, _)| *k)
            .map(|i| SLUG_CHARMAP[i].1);
        let mut buf = [0u8; 4];
        let piece = mapped.unwrap_or_else(|_| c.encode_utf8(&mut buf));
        let piece = if piece == "-" { " " } else { piece };
        out.extend(piece.chars().filter(|c| slug_allowed(*c)));
    }
    let trimmed = out.trim_matches(is_js_whitespace);
    let mut slug = String::new();
    let mut in_space = false;
    for c in trimmed.chars() {
        if is_js_whitespace(c) {
            if !in_space {
                slug.push('-');
            }
            in_space = true;
        } else {
            slug.push(c);
            in_space = false;
        }
    }
    slug.to_lowercase()
}

// ---- slugify: @sindresorhus/slugify 2.2.1 with `{ decamelize: false }` ----

fn slugify_filter(input: &Value, _args: &[Value]) -> Result<Value, String> {
    Ok(string(slugify(&input.to_js_string())))
}

fn is_diacritic(c: char) -> bool {
    let c = c as u32;
    DIACRITICS
        .binary_search_by(|&(lo, hi)| {
            if c < lo {
                std::cmp::Ordering::Greater
            } else if c > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

fn transliterate(text: &str) -> String {
    let mut s: String = text.nfc().collect();
    for (key, value) in TRANSLITERATE {
        if s.contains(key) {
            s = s.replace(key, value);
        }
    }
    s.nfd().filter(|c| !is_diacritic(*c)).nfc().collect()
}

fn slugify(text: &str) -> String {
    let lowered = transliterate(text).to_lowercase();
    let lowered = drop_contraction_apostrophes(&lowered);
    // `[^a-z\d]+` becomes one separator, and a separator at either end goes.
    let mut out = String::new();
    let mut pending = false;
    for c in lowered.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if pending && !out.is_empty() {
                out.push('-');
            }
            pending = false;
            out.push(c);
        } else {
            pending = true;
        }
    }
    out
}

/// `s.replace(/([a-zA-Z\d]+)'([ts])(\s|$)/g, "$1$2$3")`.
fn drop_contraction_apostrophes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_alphanumeric() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_alphanumeric() {
            i += 1;
        }
        out.extend(&chars[start..i]);
        if chars.get(i) == Some(&'\'')
            && matches!(chars.get(i + 1), Some('t' | 's'))
            && chars.get(i + 2).is_none_or(|c| is_js_whitespace(*c))
        {
            out.push(chars[i + 1]);
            i += 2;
            if let Some(c) = chars.get(i) {
                out.push(*c);
                i += 1;
            }
        }
    }
    out
}

// ---- url ----

fn url_filter(input: &Value, args: &[Value]) -> Result<Value, String> {
    // The registered filter takes a truthy string argument as the path
    // prefix, and uses the configured one, `/`, otherwise.
    let prefix = match args.first() {
        Some(Value::String(s)) if !s.is_empty() => s.to_string(),
        _ => "/".to_string(),
    };
    if !input.js_truthy() {
        return Ok(string(add_path_prefix("", &prefix)));
    }
    let Value::String(s) = input else {
        // Only a string has `startsWith`. `new URL(value)` still runs first
        // on the value's string form, so an array holding a URL passes.
        return if is_valid_url(&input.to_js_string()) {
            Ok(input.clone())
        } else {
            Err("url.startsWith is not a function".to_string())
        };
    };
    if is_valid_url(s) || (s.starts_with("//") && &**s != "//") {
        return Ok(input.clone());
    }
    Ok(string(add_path_prefix(s, &prefix)))
}

fn is_valid_url(s: &str) -> bool {
    url::Url::parse(s).is_ok()
}

fn add_path_prefix(url: &str, prefix: &str) -> String {
    let norm_url = normalize_url_path(&[url]);
    let norm_root = normalize_url_path(&["/", prefix]);
    let norm_full = normalize_url_path(&["/", prefix, url]);
    if norm_url == "/" || norm_url == norm_root {
        let slash = if norm_root.ends_with('/') { "" } else { "/" };
        format!("{norm_root}{slash}")
    } else if norm_url.starts_with('/') {
        norm_full
    } else {
        norm_url
    }
}

/// `TemplatePath.normalizeUrlPath`: `path.posix.join`, then trailing
/// slashes collapse to one.
fn normalize_url_path(parts: &[&str]) -> String {
    let joined = posix_join(parts);
    match joined.trim_end_matches('/').len() {
        n if n == joined.len() => joined,
        n => format!("{}/", &joined[..n]),
    }
}

fn posix_join(parts: &[&str]) -> String {
    let joined = parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    if joined.is_empty() {
        ".".to_string()
    } else {
        posix_normalize(&joined)
    }
}

/// Node's `path.posix.normalize`.
fn posix_normalize(path: &str) -> String {
    let absolute = path.starts_with('/');
    let trailing = path.ends_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if stack.last().is_some_and(|l| *l != "..") {
                    stack.pop();
                } else if !absolute {
                    stack.push("..");
                }
            }
            s => stack.push(s),
        }
    }
    let mut out = stack.join("/");
    if out.is_empty() {
        return if absolute {
            "/".to_string()
        } else if trailing {
            "./".to_string()
        } else {
            ".".to_string()
        };
    }
    if trailing {
        out.push('/');
    }
    if absolute { format!("/{out}") } else { out }
}

// ---- log ----

/// Eleventy's `log` is `console.log(input, ...messages)`, then returns the
/// input. This prints to stderr, so a build's own output stays clean.
fn log_filter(input: &Value, args: &[Value]) -> Result<Value, String> {
    let mut all = Vec::with_capacity(args.len() + 1);
    all.push(input.clone());
    all.extend_from_slice(args);
    eprintln!("{}", console_format(&all));
    Ok(input.clone())
}

/// What `console.log(...values)` prints: strings as they are, other values
/// as `util.inspect` shows them, joined by spaces. Format specifiers such
/// as `%s` are not interpreted, and nested values print in the single-line
/// form Node uses when they fit in 80 columns.
pub fn console_format(values: &[Value]) -> String {
    values
        .iter()
        .map(|v| match v {
            Value::String(s) => s.to_string(),
            v => inspect(v, 0),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn inspect(v: &Value, depth: usize) -> String {
    match v {
        Value::String(s) => quote(s),
        Value::Number(n) if *n == 0.0 && n.is_sign_negative() => "-0".to_string(),
        Value::Number(n) => js_number_to_string(*n),
        Value::Array(a) => {
            if a.is_empty() {
                "[]".to_string()
            } else if depth > 2 {
                "[Array]".to_string()
            } else {
                let items: Vec<String> = a.iter().map(|x| inspect(x, depth + 1)).collect();
                wrap('[', ']', &items, depth)
            }
        }
        Value::Object(o) => {
            if o.is_empty() {
                "{}".to_string()
            } else if depth > 2 {
                "[Object]".to_string()
            } else {
                let items: Vec<String> = Value::js_entries(o)
                    .into_iter()
                    .map(|(k, x)| format!("{}: {}", key(k), inspect(x, depth + 1)))
                    .collect();
                wrap('{', '}', &items, depth)
            }
        }
        v => v.to_js_string(),
    }
}

fn wrap(open: char, close: char, items: &[String], depth: usize) -> String {
    let total: usize = items.iter().map(|s| s.len()).sum::<usize>() + items.len();
    let start = items.len() + depth * 2 + 1 + 10;
    if total + start <= 80 && items.iter().all(|s| !s.contains('\n')) {
        format!("{open} {} {close}", items.join(", "))
    } else {
        let pad = "  ".repeat(depth);
        format!(
            "{open}\n{pad}  {}\n{pad}{close}",
            items.join(&format!(",\n{pad}  "))
        )
    }
}

fn key(k: &str) -> String {
    let mut chars = k.chars();
    let ident = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ident { k.to_string() } else { quote(k) }
}

fn quote(s: &str) -> String {
    let q = if !s.contains('\'') {
        '\''
    } else if !s.contains('"') {
        '"'
    } else if !s.contains('`') {
        '`'
    } else {
        '\''
    };
    let mut out = String::from(q);
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}
