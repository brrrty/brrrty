//! JavaScript-style regular expressions for scripts, on the `regex` crate.
//!
//! `regex_replace(s, pattern, replacement, flags)`, `regex_split(s,
//! pattern)` and `regex_test(s, pattern)` take a JavaScript pattern, the
//! text between the slashes of `/.../`, and flags (`g`, `i`, `m`, `s`,
//! `u`). A pattern is translated so the classes JavaScript defines mean
//! the same here: `\s` is JavaScript's whitespace, `\d` and `\w` are ASCII
//! only, `\b` is ASCII, and `.` stops at `\n`, `\r`, `\u2028` and `\u2029`.
//! `replace` and `split` follow the JavaScript algorithms, including `$1`,
//! `$&`, `$<name>` in replacements and captured groups in `split` results.
//!
//! The `regex` crate guarantees linear-time matching, so it has no
//! lookaround (`(?=`, `(?!`, `(?<=`, `(?<!`) and no backreferences (`\1`,
//! `\k<name>`). A pattern with either is refused with an error naming the
//! pattern. Other differences: matching is by Unicode code point, not
//! UTF-16 unit, so an emoji is one character to `.`; `^` and `$` with `m`
//! break lines at `\n` only; `{` is never a plain character; and the `y`,
//! `d` and `v` flags aren't supported.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use regex::{Captures, Regex, RegexBuilder};

/// JavaScript's `\s`.
const SPACE: &str = r"\t\n\x0B\x0C\r \u00A0\u1680\u2000-\u200A\u2028\u2029\u202F\u205F\u3000\uFEFF";
const WORD: &str = "A-Za-z0-9_";

/// A compiled JavaScript pattern.
pub struct JsRegex {
    re: Regex,
    pub global: bool,
    named: bool,
}

type Cache = HashMap<(String, String), Arc<JsRegex>>;
static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
const CACHE_LIMIT: usize = 512;

/// The compiled `pattern` with `flags`, or why it can't be.
pub fn compile(pattern: &str, flags: &str) -> Result<Arc<JsRegex>, String> {
    let key = (pattern.to_string(), flags.to_string());
    if let Some(hit) = CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|c| c.get(&key))
    {
        return Ok(hit.clone());
    }
    let made = Arc::new(build(pattern, flags).map_err(|why| {
        format!("invalid regular expression `{pattern}` with flags `{flags}`: {why}")
    })?);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, made.clone());
    Ok(made)
}

fn build(pattern: &str, flags: &str) -> Result<JsRegex, String> {
    let (mut global, mut icase, mut multi, mut dotall) = (false, false, false, false);
    for f in flags.chars() {
        let slot = match f {
            'g' => &mut global,
            'i' => &mut icase,
            'm' => &mut multi,
            's' => &mut dotall,
            'u' => continue,
            'y' | 'd' | 'v' => return Err(format!("the `{f}` flag isn't supported")),
            _ => return Err(format!("`{f}` isn't a regular expression flag")),
        };
        if *slot {
            return Err(format!("the `{f}` flag is repeated"));
        }
        *slot = true;
    }
    let translated = translate(pattern, dotall)?;
    let re = RegexBuilder::new(&translated)
        .case_insensitive(icase)
        .multi_line(multi)
        .size_limit(20 * 1024 * 1024)
        .build()
        .map_err(|e| {
            let text = e.to_string();
            text.lines()
                .rev()
                .find_map(|l| l.strip_prefix("error: "))
                .unwrap_or(&text)
                .to_string()
        })?;
    let named = re.capture_names().flatten().next().is_some();
    Ok(JsRegex { re, global, named })
}

const NO_LOOKAROUND: &str = "lookaround (lookahead and lookbehind) isn't supported: Brrrty's regular expressions have no lookaround or backreferences, so write the pattern without them";
const NO_BACKREF: &str = "backreferences aren't supported: Brrrty's regular expressions have no lookaround or backreferences, so write the pattern without them";

/// Rewrite a JavaScript pattern as one `regex` reads the same way.
fn translate(pattern: &str, dotall: bool) -> Result<String, String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut in_class = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c == '\\' {
            let Some(&n) = chars.get(i) else {
                return Err("a backslash at the end of the pattern".into());
            };
            i += 1;
            match n {
                's' | 'd' | 'w' => {
                    let set = match n {
                        's' => SPACE,
                        'd' => "0-9",
                        _ => WORD,
                    };
                    if in_class {
                        out.push_str(set);
                    } else {
                        out.push('[');
                        out.push_str(set);
                        out.push(']');
                    }
                }
                'S' | 'D' | 'W' => {
                    if in_class {
                        return Err(format!("`\\{n}` inside a character class isn't supported"));
                    }
                    let set = match n {
                        'S' => SPACE,
                        'D' => "0-9",
                        _ => WORD,
                    };
                    out.push_str("[^");
                    out.push_str(set);
                    out.push(']');
                }
                'b' if in_class => out.push_str(r"\x08"),
                'b' => out.push_str(r"(?-u:\b)"),
                'B' if !in_class => out.push_str(r"(?-u:\B)"),
                '0' => out.push_str(r"\x00"),
                '1'..='9' if !in_class => return Err(NO_BACKREF.into()),
                'k' => return Err(NO_BACKREF.into()),
                'c' => return Err("`\\c` control escapes aren't supported".into()),
                n if n.is_ascii_alphanumeric() => {
                    out.push('\\');
                    out.push(n);
                }
                n => push_literal(&mut out, n),
            }
            continue;
        }
        if in_class {
            match c {
                ']' => {
                    in_class = false;
                    out.push(']');
                }
                '[' | '&' | '~' => {
                    out.push('\\');
                    out.push(c);
                }
                _ => out.push(c),
            }
            continue;
        }
        match c {
            '[' => {
                in_class = true;
                out.push('[');
                if chars.get(i) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
            }
            '.' if dotall => out.push_str("(?s:.)"),
            '.' => out.push_str(r"[^\n\r\u2028\u2029]"),
            '(' if chars.get(i) == Some(&'?') => {
                let rest: String = chars[i + 1..].iter().take(2).collect();
                if rest.starts_with('=') || rest.starts_with('!') || rest == "<=" || rest == "<!" {
                    return Err(NO_LOOKAROUND.into());
                }
                out.push('(');
            }
            _ => out.push(c),
        }
    }
    if in_class {
        return Err("a character class is never closed".into());
    }
    Ok(out)
}

/// A character JavaScript escaped for no reason, as a literal.
fn push_literal(out: &mut String, c: char) {
    if "\\.+*?()|[]{}^$#&-~".contains(c) {
        out.push('\\');
    }
    out.push(c);
}

/// The next character boundary after `i`.
fn advance(s: &str, i: usize) -> usize {
    i + s[i..].chars().next().map_or(1, char::len_utf8)
}

impl JsRegex {
    pub fn is_match(&self, s: &str) -> bool {
        self.re.is_match(s)
    }

    /// `s.replace(re, replacement)`: every match with `g`, else the first.
    pub fn replace(&self, s: &str, replacement: &str) -> String {
        let mut out = String::new();
        let mut last = 0;
        let mut at = 0;
        while at <= s.len() {
            let Some(caps) = self.re.captures_at(s, at) else {
                break;
            };
            let m = caps.get(0).expect("group 0");
            out.push_str(&s[last..m.start()]);
            self.expand(replacement, &caps, s, &mut out);
            last = m.end();
            at = if m.is_empty() {
                advance(s, m.end())
            } else {
                m.end()
            };
            if !self.global {
                break;
            }
        }
        out.push_str(&s[last.min(s.len())..]);
        out
    }

    /// JavaScript's replacement patterns: `$$ $& $` $' $n $nn $<name>`.
    fn expand(&self, rep: &str, caps: &Captures<'_>, s: &str, out: &mut String) {
        let m = caps.get(0).expect("group 0");
        let groups = caps.len() - 1;
        let b = rep.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] != b'$' || i + 1 >= b.len() {
                let n = rep[i..].chars().next().map_or(1, char::len_utf8);
                out.push_str(&rep[i..i + n]);
                i += n;
                continue;
            }
            match b[i + 1] {
                b'$' => {
                    out.push('$');
                    i += 2;
                }
                b'&' => {
                    out.push_str(m.as_str());
                    i += 2;
                }
                b'`' => {
                    out.push_str(&s[..m.start()]);
                    i += 2;
                }
                b'\'' => {
                    out.push_str(&s[m.end()..]);
                    i += 2;
                }
                d1 @ b'0'..=b'9' => {
                    let one = usize::from(d1 - b'0');
                    let two = b
                        .get(i + 2)
                        .filter(|d| d.is_ascii_digit())
                        .map(|d| one * 10 + usize::from(d - b'0'));
                    match (two, one) {
                        (Some(n), _) if (1..=groups).contains(&n) => {
                            out.push_str(caps.get(n).map_or("", |g| g.as_str()));
                            i += 3;
                        }
                        (_, n) if (1..=groups).contains(&n) => {
                            out.push_str(caps.get(n).map_or("", |g| g.as_str()));
                            i += 2;
                        }
                        _ => {
                            out.push('$');
                            i += 1;
                        }
                    }
                }
                b'<' if self.named => match rep[i + 2..].find('>') {
                    Some(end) => {
                        let name = &rep[i + 2..i + 2 + end];
                        out.push_str(caps.name(name).map_or("", |g| g.as_str()));
                        i += end + 3;
                    }
                    None => {
                        out.push('$');
                        i += 1;
                    }
                },
                _ => {
                    out.push('$');
                    i += 1;
                }
            }
        }
    }

    /// `s.split(re)`: the pieces between matches, with each match's
    /// captured groups between them (`None` for a group that didn't take
    /// part).
    pub fn split(&self, s: &str) -> Vec<Option<String>> {
        if s.is_empty() {
            return if self.re.is_match(s) {
                Vec::new()
            } else {
                vec![Some(String::new())]
            };
        }
        let mut out = Vec::new();
        let mut p = 0;
        let mut q = 0;
        while q < s.len() {
            let Some(caps) = self.re.captures_at(s, q) else {
                break;
            };
            let m = caps.get(0).expect("group 0");
            // A match that ends where the last piece began, or at the end
            // of the text, splits nothing.
            if m.start() >= s.len() {
                break;
            }
            if m.end() == p || (m.is_empty() && m.start() == p) {
                q = advance(s, m.start().max(q));
                continue;
            }
            out.push(Some(s[p..m.start()].to_string()));
            for g in 1..caps.len() {
                out.push(caps.get(g).map(|g| g.as_str().to_string()));
            }
            p = m.end();
            q = p;
        }
        out.push(Some(s[p..].to_string()));
        out
    }
}
