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
//! Only syntax that means exactly the same in JavaScript and in Rust is
//! accepted: literals, `\d \s \w \b` and their negations, `\n \r \t \f \v \0
//! \xHH \uHHHH`, escaped punctuation, `.`, `^`, `$`, `|`, groups `(...)`,
//! `(?:...)`, `(?<name>...)`, quantifiers and classes. Anything else (`\a`,
//! `\p{..}`, `(?i)`, `[]`, a bare `{`, ...) is refused. Some text is
//! refused too, where the result would differ: non-ASCII text with `i`,
//! text with characters outside the BMP (emoji) when the pattern could
//! split a surrogate pair, and text with `\r`, U+2028 or U+2029 when `m`
//! is used with `^` or `$`.
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
    icase: bool,
    multiline: bool,
    unicode: bool,
    info: Info,
    /// The pattern behaves the same on text with characters outside the
    /// BMP, which JavaScript (without `u`) sees as two units each.
    astral_safe: bool,
}

/// What a pattern uses, for the checks made against text.
#[derive(Default, Clone, Copy)]
struct Info {
    /// `.`, a negated class or a negated class escape: these match half
    /// of a surrogate pair in JavaScript.
    matches_units: bool,
    /// `^` or `$`.
    anchors: bool,
    /// A letter (or `\w`, or an escape that may be one) that `i` would
    /// fold; without one, `i` changes nothing.
    cased: bool,
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
    let (mut global, mut icase, mut multi, mut dotall, mut unicode) =
        (false, false, false, false, false);
    for f in flags.chars() {
        let slot = match f {
            'g' => &mut global,
            'i' => &mut icase,
            'm' => &mut multi,
            's' => &mut dotall,
            'u' => &mut unicode,
            'y' | 'd' | 'v' => return Err(format!("the `{f}` flag isn't supported")),
            _ => return Err(format!("`{f}` isn't a regular expression flag")),
        };
        if *slot {
            return Err(format!("the `{f}` flag is repeated"));
        }
        *slot = true;
    }
    if icase && !pattern.is_ascii() {
        return Err("the `i` flag is only supported for patterns that are ASCII: JavaScript and Rust fold the case of other characters differently".into());
    }
    if !unicode && pattern.chars().any(|c| c as u32 > 0xFFFF) {
        return Err("a character outside the BMP needs the `u` flag, which makes JavaScript see it as one character and not two".into());
    }
    let (translated, info) = translate(pattern, dotall)?;
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
    let astral_safe = !info.matches_units && !re.is_match("");
    Ok(JsRegex {
        re,
        global,
        named,
        icase,
        multiline: multi,
        unicode,
        info,
        astral_safe,
    })
}

const NO_LOOKAROUND: &str = "lookaround (lookahead and lookbehind) isn't supported: Brrrty's regular expressions have no lookaround or backreferences, so write the pattern without them";
const NO_BACKREF: &str = "backreferences aren't supported: Brrrty's regular expressions have no lookaround or backreferences, so write the pattern without them";

/// Characters `\` may escape to themselves.
const SYNTAX: &str = "^$\\.*+?()[]{}|/";

fn hex(chars: &[char], at: usize, n: usize) -> bool {
    chars.len() >= at + n && chars[at..at + n].iter().all(char::is_ascii_hexdigit)
}

/// Rewrite a JavaScript pattern as one `regex` reads the same way. Only
/// syntax that means exactly the same in both is accepted; anything else
/// is refused, so a pattern never matches differently without notice.
fn translate(pattern: &str, dotall: bool) -> Result<(String, Info), String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut info = Info::default();
    let mut i = 0;
    // Whether the last thing written can take a quantifier.
    let mut atom = false;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '\\' => {
                let Some(&n) = chars.get(i) else {
                    return Err("a backslash at the end of the pattern".into());
                };
                i += 1;
                atom = true;
                match n {
                    's' | 'd' | 'w' => {
                        info.cased |= n == 'w';
                        let set = match n {
                            's' => SPACE,
                            'd' => "0-9",
                            _ => WORD,
                        };
                        out.push('[');
                        out.push_str(set);
                        out.push(']');
                    }
                    'S' | 'D' | 'W' => {
                        info.matches_units = true;
                        info.cased |= n == 'W';
                        let set = match n {
                            'S' => SPACE,
                            'D' => "0-9",
                            _ => WORD,
                        };
                        out.push_str("[^");
                        out.push_str(set);
                        out.push(']');
                    }
                    'b' => {
                        out.push_str(r"(?-u:\b)");
                        atom = false;
                    }
                    'B' => {
                        info.matches_units = true;
                        out.push_str(r"(?-u:\B)");
                        atom = false;
                    }
                    '0' if !chars.get(i).is_some_and(char::is_ascii_digit) => out.push_str(r"\x00"),
                    '1'..='9' => return Err(NO_BACKREF.into()),
                    'k' => return Err(NO_BACKREF.into()),
                    'n' | 'r' | 't' | 'f' | 'v' => {
                        out.push('\\');
                        out.push(n);
                    }
                    'x' if hex(&chars, i, 2) => {
                        info.cased = true;
                        out.push_str("\\x");
                        out.extend(&chars[i..i + 2]);
                        i += 2;
                    }
                    'u' if hex(&chars, i, 4) => {
                        info.cased = true;
                        let unit =
                            u32::from_str_radix(&chars[i..i + 4].iter().collect::<String>(), 16)
                                .unwrap_or(0);
                        if (0xD800..0xE000).contains(&unit) {
                            return Err("an escaped surrogate isn't supported".into());
                        }
                        out.push_str(&format!("\\x{{{unit:X}}}"));
                        i += 4;
                    }
                    '/' => out.push('/'),
                    n if SYNTAX.contains(n) => {
                        out.push('\\');
                        out.push(n);
                    }
                    n => {
                        return Err(format!(
                            "the escape `\\{n}` isn't supported: it means something different, or is an error, in Rust's regular expressions"
                        ));
                    }
                }
            }
            '[' => {
                atom = true;
                out.push('[');
                if chars.get(i) == Some(&'^') {
                    info.matches_units = true;
                    out.push('^');
                    i += 1;
                }
                if chars.get(i) == Some(&']') {
                    return Err("an empty character class (`[]` or `[^]`) isn't supported".into());
                }
                // `prev_set`: the last item was `\d`, `\s` or `\w`.
                let mut prev_set = false;
                let mut closed = false;
                while i < chars.len() {
                    let c = chars[i];
                    i += 1;
                    let mut set = false;
                    match c {
                        ']' => {
                            closed = true;
                            break;
                        }
                        '\\' => {
                            let Some(&n) = chars.get(i) else {
                                return Err("a backslash at the end of the pattern".into());
                            };
                            i += 1;
                            match n {
                                's' | 'd' | 'w' => {
                                    set = true;
                                    info.cased |= n == 'w';
                                    out.push_str(match n {
                                        's' => SPACE,
                                        'd' => "0-9",
                                        _ => WORD,
                                    });
                                }
                                'b' => out.push_str(r"\x08"),
                                '0' if !chars.get(i).is_some_and(char::is_ascii_digit) => {
                                    out.push_str(r"\x00")
                                }
                                'n' | 'r' | 't' | 'f' | 'v' => {
                                    out.push('\\');
                                    out.push(n);
                                }
                                'x' if hex(&chars, i, 2) => {
                                    info.cased = true;
                                    out.push_str("\\x");
                                    out.extend(&chars[i..i + 2]);
                                    i += 2;
                                }
                                'u' if hex(&chars, i, 4) => {
                                    info.cased = true;
                                    let unit = u32::from_str_radix(
                                        &chars[i..i + 4].iter().collect::<String>(),
                                        16,
                                    )
                                    .unwrap_or(0);
                                    if (0xD800..0xE000).contains(&unit) {
                                        return Err("an escaped surrogate isn't supported".into());
                                    }
                                    out.push_str(&format!("\\x{{{unit:X}}}"));
                                    i += 4;
                                }
                                '/' => out.push('/'),
                                n if SYNTAX.contains(n) || n == '-' => {
                                    out.push('\\');
                                    out.push(n);
                                }
                                n => {
                                    return Err(format!(
                                        "the escape `\\{n}` isn't supported in a character class"
                                    ));
                                }
                            }
                        }
                        '-' => {
                            // A range's ends must be single characters, and
                            // `--` is a set difference in Rust.
                            let next = chars.get(i).copied();
                            if next == Some('-') {
                                return Err("`--` in a character class isn't supported".into());
                            }
                            let next_set = next == Some('\\')
                                && matches!(
                                    chars.get(i + 1),
                                    Some('d' | 's' | 'w' | 'D' | 'S' | 'W')
                                );
                            let range =
                                next != Some(']') && !out.ends_with('[') && !out.ends_with("[^");
                            if range && (prev_set || next_set) {
                                return Err(
                                    "a range with a class like `\\w` at an end isn't supported"
                                        .into(),
                                );
                            }
                            out.push('-');
                        }
                        '[' | '&' | '~' => {
                            out.push('\\');
                            out.push(c);
                        }
                        _ => {
                            info.cased |= c.is_ascii_alphabetic();
                            out.push(c)
                        }
                    }
                    prev_set = set;
                }
                if !closed {
                    return Err("a character class is never closed".into());
                }
                out.push(']');
            }
            '.' => {
                atom = true;
                info.matches_units = true;
                out.push_str(if dotall {
                    "(?s:.)"
                } else {
                    r"[^\n\r\u2028\u2029]"
                });
            }
            '^' | '$' => {
                info.anchors = true;
                atom = false;
                out.push(c);
            }
            '|' => {
                atom = false;
                out.push(c);
            }
            '(' => {
                atom = false;
                if chars.get(i) == Some(&'?') {
                    let rest: String = chars[i + 1..].iter().take(2).collect();
                    if rest.starts_with('=')
                        || rest.starts_with('!')
                        || rest == "<="
                        || rest == "<!"
                    {
                        return Err(NO_LOOKAROUND.into());
                    }
                    if rest.starts_with(':') {
                        out.push_str("(?:");
                        i += 2;
                    } else if rest.starts_with('<') {
                        let end = chars[i + 2..].iter().position(|&c| c == '>');
                        let name: String = end
                            .map(|e| chars[i + 2..i + 2 + e].iter().collect())
                            .unwrap_or_default();
                        let ok = name
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                        if !ok {
                            return Err("a group name must be ASCII letters, digits and `_`".into());
                        }
                        out.push_str(&format!("(?P<{name}>"));
                        i += 2 + name.len() + 1;
                    } else {
                        return Err("a group starting `(?` other than `(?:` and `(?<name>` isn't supported (Rust reads `(?i)` and the like as flags)".into());
                    }
                } else {
                    out.push('(');
                }
            }
            ')' => {
                atom = true;
                out.push(')');
            }
            '*' | '+' | '?' => {
                if !atom {
                    return Err(format!("`{c}` has nothing to repeat"));
                }
                out.push(c);
                if chars.get(i) == Some(&'?') {
                    out.push('?');
                    i += 1;
                }
                atom = false;
            }
            '{' => {
                // `{n}`, `{n,}`, `{n,m}`; a bare `{` is a literal in
                // JavaScript and an error in Rust.
                let close = chars[i..].iter().position(|&c| c == '}');
                let body: String = close
                    .map(|e| chars[i..i + e].iter().collect())
                    .unwrap_or_default();
                let valid = {
                    let mut parts = body.splitn(2, ',');
                    let lo = parts.next().unwrap_or("");
                    let hi = parts.next();
                    !lo.is_empty()
                        && lo.bytes().all(|b| b.is_ascii_digit())
                        && hi.is_none_or(|h| h.bytes().all(|b| b.is_ascii_digit()))
                };
                if !valid || !atom {
                    return Err("a `{` that isn't a `{n}`, `{n,}` or `{n,m}` quantifier isn't supported; escape it as `\\{`".into());
                }
                out.push('{');
                out.push_str(&body);
                out.push('}');
                i += body.chars().count() + 1;
                if chars.get(i) == Some(&'?') {
                    out.push('?');
                    i += 1;
                }
                atom = false;
            }
            ']' | '}' => {
                return Err(format!(
                    "a bare `{c}` isn't supported; escape it as `\\{c}`"
                ));
            }
            _ => {
                atom = true;
                info.cased |= c.is_ascii_alphabetic();
                out.push(c);
            }
        }
    }
    Ok((out, info))
}

/// The next character boundary after `i`.
fn advance(s: &str, i: usize) -> usize {
    i + s[i..].chars().next().map_or(1, char::len_utf8)
}

impl JsRegex {
    /// Refuse text the pattern would treat differently from JavaScript.
    fn check(&self, s: &str) -> Result<(), String> {
        if self.icase && self.info.cased && !s.is_ascii() {
            return Err("the `i` flag is only supported for ASCII text: JavaScript and Rust fold the case of other characters differently".into());
        }
        if !self.unicode && !self.astral_safe && s.chars().any(|c| c as u32 > 0xFFFF) {
            return Err("this pattern can't be used on text with characters outside the BMP (such as emoji): JavaScript sees each as two characters, and the pattern would match differently; add the `u` flag if that is what it should do".into());
        }
        if self.multiline && self.info.anchors && s.contains(['\r', '\u{2028}', '\u{2029}']) {
            return Err("`^` and `$` with the `m` flag only break lines at `\\n` here, and JavaScript also breaks them at `\\r`, U+2028 and U+2029, which this text has".into());
        }
        Ok(())
    }

    pub fn is_match(&self, s: &str) -> Result<bool, String> {
        self.check(s)?;
        Ok(self.re.is_match(s))
    }

    /// `s.replace(re, replacement)`: every match with `g`, else the first.
    pub fn replace(&self, s: &str, replacement: &str) -> Result<String, String> {
        self.check(s)?;
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
        Ok(out)
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
    pub fn split(&self, s: &str) -> Result<Vec<Option<String>>, String> {
        self.check(s)?;
        if s.is_empty() {
            return Ok(if self.re.is_match(s) {
                Vec::new()
            } else {
                vec![Some(String::new())]
            });
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
        Ok(out)
    }
}
