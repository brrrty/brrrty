//! A tokenizer for 11ty config files, enough to read them without running them.
//!
//! It splits JavaScript into tokens and statements and reads literal values
//! (strings, numbers, booleans, `null`, arrays and objects of those). Anything
//! else is left for a person to port, so it only has to know where code starts
//! and ends, not what it means.

/// What a token is.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    /// A name or keyword.
    Ident(String),
    /// A string, or a template literal with no `${}`, decoded. `None` when an
    /// escape can't be decoded exactly (a lone surrogate).
    Str(Option<String>),
    /// A template literal with substitutions.
    Template,
    /// A number, as written.
    Num(String),
    /// A regular expression literal.
    Regex,
    /// Punctuation.
    Punct(&'static str),
}

/// A token and where it is in the source.
#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub tok: Tok,
    /// Byte offsets.
    pub start: usize,
    pub end: usize,
    /// A line break comes between this token and the one before.
    pub nl_before: bool,
    /// The bracket that closes or opens this one, for `(`, `[`, `{` and their pairs.
    pub pair: Option<usize>,
    /// A template literal (which on a new line continues the statement as a tag).
    pub tick: bool,
}

/// How deep brackets and template literals may nest before Brrrty stops reading.
pub(crate) const MAX_DEPTH: usize = 64;

impl Token {
    pub(crate) fn is(&self, p: &str) -> bool {
        matches!(&self.tok, Tok::Punct(q) if *q == p)
    }

    pub(crate) fn ident(&self) -> Option<&str> {
        match &self.tok {
            Tok::Ident(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn is_ident(&self, name: &str) -> bool {
        self.ident() == Some(name)
    }
}

const PUNCTS: [&str; 52] = [
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=",
    "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
    "**", "<<", ">>", "{", "}", "(", ")", "[", "]", ";", ",", "<", ">", "+", "-", "*", "%", "&",
    "|", "^", "!", "~",
];
const MORE_PUNCTS: [&str; 6] = ["?", ":", "=", ".", "/", "@"];

/// Keywords after which a `/` starts a regular expression.
const REGEX_AFTER: [&str; 12] = [
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
];

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$' || b >= 0x80
}

fn is_ident_part(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit()
}

/// Split `src` into tokens, or say why it can't be.
pub(crate) fn tokenize(src: &str) -> Result<Vec<Token>, String> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out: Vec<Token> = Vec::new();
    let mut nl = false;
    // For each open `(`: whether it holds the condition of `if`, `while`,
    // `for` or `with`, after which a `/` could start a regular expression.
    let mut parens: Vec<bool> = Vec::new();
    let mut closed_condition = false;
    if src.starts_with("#!") {
        while i < b.len() && b[i] != b'\n' {
            i += 1;
        }
    }
    while i < b.len() {
        let c = b[i];
        if c == b'\n' || c == b'\r' {
            nl = true;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // U+2028, U+2029 and U+FEFF (a BOM).
        if src[i..].starts_with('\u{2028}') || src[i..].starts_with('\u{2029}') {
            nl = true;
            i += 3;
            continue;
        }
        if src[i..].starts_with('\u{feff}') || src[i..].starts_with('\u{a0}') {
            i += src[i..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if src[i..].starts_with("//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if src[i..].starts_with("/*") {
            let Some(end) = src[i + 2..].find("*/") else {
                return Err("a /* comment is never closed".into());
            };
            if src[i..i + 2 + end].contains('\n') {
                nl = true;
            }
            i += end + 4;
            continue;
        }
        let start = i;
        let tok = if c == b'"' || c == b'\'' {
            let (end, value) = string(src, i)?;
            i = end;
            Tok::Str(value)
        } else if c == b'`' {
            let (end, value) = template(src, i, 0)?;
            i = end;
            match value {
                Some(v) => Tok::Str(v),
                None => Tok::Template,
            }
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                // An exponent's sign.
                if (b[i] == b'e' || b[i] == b'E')
                    && !src[start..].starts_with("0x")
                    && matches!(b.get(i + 1), Some(b'+' | b'-'))
                {
                    i += 1;
                }
                i += 1;
            }
            Tok::Num(src[start..i].to_string())
        } else if is_ident_start(c) || c == b'#' || c == b'\\' {
            i += 1;
            while i < b.len() && (is_ident_part(b[i]) || b[i] == b'\\') {
                i += 1;
            }
            // `\u0065` in a name means `e`: never compare such names as text.
            if src[start..i].contains('\\') {
                return Err("a name is written with a `\\` escape".into());
            }
            Tok::Ident(src[start..i].to_string())
        } else if c == b'/' && slash(out.last(), closed_condition)? {
            i = regex(b, i)?;
            Tok::Regex
        } else {
            let p = PUNCTS
                .iter()
                .chain(MORE_PUNCTS.iter())
                .find(|p| src[i..].starts_with(**p));
            match p {
                Some(p) => {
                    i += p.len();
                    Tok::Punct(p)
                }
                None => {
                    let ch = src[i..].chars().next().unwrap_or('?');
                    return Err(format!("unexpected `{ch}`"));
                }
            }
        };
        closed_condition = false;
        match tok {
            Tok::Punct("(") => parens.push(
                out.last()
                    .and_then(Token::ident)
                    .is_some_and(|w| matches!(w, "if" | "while" | "for" | "with")),
            ),
            Tok::Punct(")") => closed_condition = parens.pop().unwrap_or(false),
            _ => {}
        }
        out.push(Token {
            tok,
            start,
            end: i,
            nl_before: nl,
            pair: None,
            tick: c == b'`',
        });
        nl = false;
    }
    pair_up(&mut out)?;
    Ok(out)
}

/// Fill in `pair` for every bracket, in one pass.
fn pair_up(toks: &mut [Token]) -> Result<(), String> {
    let mut stack: Vec<usize> = Vec::new();
    for k in 0..toks.len() {
        match toks[k].tok {
            Tok::Punct("(" | "[" | "{") => {
                if stack.len() >= MAX_DEPTH {
                    return Err(format!("brackets nest more than {MAX_DEPTH} deep"));
                }
                stack.push(k);
            }
            Tok::Punct(close @ (")" | "]" | "}")) => {
                let Some(o) = stack.pop() else {
                    return Err(format!("a `{close}` closes nothing"));
                };
                let want = match toks[o].tok {
                    Tok::Punct("(") => ")",
                    Tok::Punct("[") => "]",
                    _ => "}",
                };
                if want != close {
                    return Err(format!(
                        "a `{close}` closes a bracket opened with something else"
                    ));
                }
                toks[o].pair = Some(k);
                toks[k].pair = Some(o);
            }
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err("a bracket is never closed".into());
    }
    Ok(())
}

/// Whether a `/` after `prev` starts a regular expression (`true`) or is
/// division (`false`). Where that can't be known without parsing the
/// statement around it, the file is unreadable rather than guessed at.
fn slash(prev: Option<&Token>, after_condition: bool) -> Result<bool, String> {
    let unsure = || {
        Err(
            "Brrrty can't tell whether a `/` here starts a regular expression or divides"
                .to_string(),
        )
    };
    match prev.map(|t| &t.tok) {
        None => Ok(true),
        Some(Tok::Ident(k)) => match k.as_str() {
            "of" | "yield" | "await" | "let" | "async" | "get" | "set" | "static" => unsure(),
            k if REGEX_AFTER.contains(&k) => Ok(true),
            _ => Ok(false),
        },
        Some(Tok::Punct(")")) if after_condition => unsure(),
        Some(Tok::Punct(")" | "]")) => Ok(false),
        Some(Tok::Punct("}" | "++" | "--")) => unsure(),
        Some(Tok::Punct(_)) => Ok(true),
        Some(_) => Ok(false),
    }
}

fn regex(b: &[u8], mut i: usize) -> Result<usize, String> {
    i += 1;
    let mut class = false;
    loop {
        match b.get(i) {
            None | Some(b'\n') => return Err("a regular expression is never closed".into()),
            Some(b'\\') => i += 2,
            Some(b'[') => {
                class = true;
                i += 1;
            }
            Some(b']') => {
                class = false;
                i += 1;
            }
            Some(b'/') if !class => {
                i += 1;
                while i < b.len() && is_ident_part(b[i]) {
                    i += 1;
                }
                return Ok(i);
            }
            Some(_) => i += 1,
        }
    }
}

/// A quoted string from `i`: where it ends, and its value.
fn string(src: &str, i: usize) -> Result<(usize, Option<String>), String> {
    let b = src.as_bytes();
    let quote = b[i];
    let mut j = i + 1;
    let mut units: Vec<u16> = Vec::new();
    let mut exact = true;
    loop {
        match b.get(j) {
            None | Some(b'\n') => return Err("a string is never closed".into()),
            Some(&q) if q == quote => break,
            Some(b'\\') => {
                let (next, ok) = escape(src, j, &mut units);
                exact &= ok;
                j = next;
            }
            Some(_) => {
                let ch = src[j..].chars().next().unwrap_or('\u{fffd}');
                let mut buf = [0u16; 2];
                units.extend_from_slice(ch.encode_utf16(&mut buf));
                j += ch.len_utf8();
            }
        }
    }
    let value = if exact {
        String::from_utf16(&units).ok()
    } else {
        None
    };
    Ok((j + 1, value))
}

/// The escape at `j` (a backslash): where it ends, pushing its UTF-16 units.
/// `false` when it isn't one that can be read exactly.
fn escape(src: &str, j: usize, units: &mut Vec<u16>) -> (usize, bool) {
    let b = src.as_bytes();
    let Some(&c) = b.get(j + 1) else {
        return (j + 1, false);
    };
    let simple = match c {
        b'n' => Some(0x0a),
        b'r' => Some(0x0d),
        b't' => Some(0x09),
        b'b' => Some(0x08),
        b'f' => Some(0x0c),
        b'v' => Some(0x0b),
        b'0' if !b.get(j + 2).is_some_and(u8::is_ascii_digit) => Some(0),
        _ => None,
    };
    if let Some(u) = simple {
        units.push(u);
        return (j + 2, true);
    }
    let hex = |s: &str| u32::from_str_radix(s, 16).ok();
    match c {
        b'\n' => (j + 2, true),
        b'\r' => (
            if b.get(j + 2) == Some(&b'\n') {
                j + 3
            } else {
                j + 2
            },
            true,
        ),
        b'x' => match src.get(j + 2..j + 4).and_then(hex) {
            Some(v) => {
                units.push(v as u16);
                (j + 4, true)
            }
            None => (j + 2, false),
        },
        b'u' if b.get(j + 2) == Some(&b'{') => {
            let Some(close) = src[j + 3..].find('}') else {
                return (j + 2, false);
            };
            match hex(&src[j + 3..j + 3 + close]).and_then(char::from_u32) {
                Some(ch) => {
                    let mut buf = [0u16; 2];
                    units.extend_from_slice(ch.encode_utf16(&mut buf));
                    (j + 4 + close, true)
                }
                None => (j + 4 + close, false),
            }
        }
        b'u' => match src.get(j + 2..j + 6).and_then(hex) {
            Some(v) => {
                units.push(v as u16);
                (j + 6, true)
            }
            None => (j + 2, false),
        },
        b'1'..=b'9' | b'0' => (j + 2, false),
        _ => {
            let ch = src[j + 1..].chars().next().unwrap_or('\u{fffd}');
            let mut buf = [0u16; 2];
            units.extend_from_slice(ch.encode_utf16(&mut buf));
            (j + 1 + ch.len_utf8(), true)
        }
    }
}

/// A template literal from `i`: where it ends, and its value when it has no
/// substitutions.
fn template(src: &str, i: usize, depth: usize) -> Result<(usize, Option<Option<String>>), String> {
    if depth > MAX_DEPTH {
        return Err(format!("template literals nest more than {MAX_DEPTH} deep"));
    }
    let b = src.as_bytes();
    let mut j = i + 1;
    let mut units: Vec<u16> = Vec::new();
    let mut exact = true;
    let mut plain = true;
    loop {
        match b.get(j) {
            None => return Err("a template literal is never closed".into()),
            Some(b'`') => break,
            Some(b'\\') => {
                let (next, ok) = escape(src, j, &mut units);
                exact &= ok;
                j = next;
            }
            Some(b'$') if b.get(j + 1) == Some(&b'{') => {
                plain = false;
                j = substitution(src, j + 2, depth + 1)?;
            }
            Some(b'\r') => {
                // A template's CR LF and CR read as LF.
                units.push(0x0a);
                j += if b.get(j + 1) == Some(&b'\n') { 2 } else { 1 };
            }
            Some(_) => {
                let ch = src[j..].chars().next().unwrap_or('\u{fffd}');
                let mut buf = [0u16; 2];
                units.extend_from_slice(ch.encode_utf16(&mut buf));
                j += ch.len_utf8();
            }
        }
    }
    let value = if !plain {
        None
    } else if exact {
        Some(String::from_utf16(&units).ok())
    } else {
        Some(None)
    };
    Ok((j + 1, value))
}

/// Skip a `${ ... }` body starting at `j`; returns the offset after its `}`.
fn substitution(src: &str, mut j: usize, nest: usize) -> Result<usize, String> {
    let b = src.as_bytes();
    let mut depth = 0usize;
    loop {
        match b.get(j) {
            None => return Err("a template literal is never closed".into()),
            Some(b'{') => {
                depth += 1;
                j += 1;
            }
            Some(b'}') if depth == 0 => return Ok(j + 1),
            Some(b'}') => {
                depth -= 1;
                j += 1;
            }
            Some(b'"' | b'\'') => j = string(src, j)?.0,
            Some(b'`') => j = template(src, j, nest)?.0,
            Some(b'/') if b.get(j + 1) == Some(&b'/') => {
                while j < b.len() && b[j] != b'\n' {
                    j += 1;
                }
            }
            Some(b'/') if b.get(j + 1) == Some(&b'*') => match src[j + 2..].find("*/") {
                Some(e) => j += e + 4,
                None => return Err("a /* comment is never closed".into()),
            },
            // A regular expression could hold a `}` or a quote.
            Some(b'/') => {
                return Err("Brrrty can't read a `/` inside a template literal's `${}`".into());
            }
            Some(_) => j += 1,
        }
    }
}

/// The index of the token that closes the bracket at `open`.
pub(crate) fn matching(toks: &[Token], open: usize) -> Option<usize> {
    let t = toks.get(open)?;
    if !matches!(t.tok, Tok::Punct("(" | "[" | "{")) {
        return None;
    }
    t.pair
}

/// Keywords that end their statement at a line break (JavaScript's
/// restricted productions): `return\n{...}` returns `undefined`.
const RESTRICTED: [&str; 5] = ["return", "throw", "break", "continue", "yield"];

fn can_end(t: &Token) -> bool {
    match &t.tok {
        Tok::Punct(p) => matches!(*p, ")" | "]" | "}" | "++" | "--"),
        _ => true,
    }
}

fn continues(t: &Token) -> bool {
    if t.tick {
        return true;
    }
    match &t.tok {
        Tok::Punct(p) => !matches!(*p, "!" | "~" | "++" | "--"),
        Tok::Ident(k) => matches!(k.as_str(), "else" | "catch" | "finally"),
        _ => false,
    }
}

/// Split `toks[from..to]` into statements, as index ranges without their `;`.
pub(crate) fn statements(toks: &[Token], from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = from;
    for k in from..to {
        let t = &toks[k];
        if depth == 0
            && k > start
            && t.nl_before
            && (toks[k - 1].ident().is_some_and(|w| RESTRICTED.contains(&w))
                || (can_end(&toks[k - 1]) && !continues(t)))
        {
            out.push((start, k));
            start = k;
        }
        match t.tok {
            Tok::Punct("(" | "[" | "{") => depth += 1,
            Tok::Punct(")" | "]" | "}") => depth -= 1,
            Tok::Punct(";") if depth == 0 => {
                if k > start {
                    out.push((start, k));
                }
                start = k + 1;
            }
            _ => {}
        }
    }
    if start < to {
        out.push((start, to));
    }
    out
}

/// Split `toks[from..to]` at its top-level commas.
pub(crate) fn split_commas(toks: &[Token], from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = from;
    for (k, t) in toks.iter().enumerate().take(to).skip(from) {
        match t.tok {
            Tok::Punct("(" | "[" | "{") => depth += 1,
            Tok::Punct(")" | "]" | "}") => depth -= 1,
            Tok::Punct(",") if depth == 0 => {
                out.push((start, k));
                start = k + 1;
            }
            _ => {}
        }
    }
    if start < to {
        out.push((start, to));
    }
    out
}

/// A literal value read exactly.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Lit {
    Str(String),
    /// As written: a whole number or a plain decimal.
    Num(String),
    Bool(bool),
    Null,
    Array(Vec<Lit>),
    /// Keys in the order written.
    Object(Vec<(String, Lit)>),
}

/// One property of an object literal: its key and where its value is.
pub(crate) struct Prop {
    pub key: String,
    /// The whole property.
    pub range: (usize, usize),
    /// The value.
    pub value: (usize, usize),
}

/// The properties of the object literal that is exactly `toks[from..to]`,
/// when each is a plain `key: value`.
pub(crate) fn object_props(toks: &[Token], from: usize, to: usize) -> Option<Vec<Prop>> {
    if to <= from || !toks[from].is("{") || matching(toks, from)? != to - 1 {
        return None;
    }
    let mut out = Vec::new();
    for (a, b) in split_commas(toks, from + 1, to - 1) {
        if b < a + 3 || !toks[a + 1].is(":") {
            return None;
        }
        let key = match &toks[a].tok {
            Tok::Ident(s) => s.clone(),
            Tok::Str(Some(s)) => s.clone(),
            Tok::Num(n) if n.bytes().all(|c| c.is_ascii_digit()) => n.clone(),
            _ => return None,
        };
        out.push(Prop {
            key,
            range: (a, b),
            value: (a + 2, b),
        });
    }
    Some(out)
}

/// The literal that is exactly `toks[from..to]`.
pub(crate) fn literal(toks: &[Token], from: usize, to: usize) -> Option<Lit> {
    let first = toks.get(from)?;
    if to == from + 1 {
        return match &first.tok {
            Tok::Str(Some(s)) => Some(Lit::Str(s.clone())),
            Tok::Num(n) => number(n, false),
            Tok::Ident(k) => match k.as_str() {
                "true" => Some(Lit::Bool(true)),
                "false" => Some(Lit::Bool(false)),
                "null" => Some(Lit::Null),
                _ => None,
            },
            _ => None,
        };
    }
    if to == from + 2 && first.is("-") {
        if let Tok::Num(n) = &toks[from + 1].tok {
            return number(n, true);
        }
        return None;
    }
    if first.is("[") {
        if matching(toks, from)? != to - 1 {
            return None;
        }
        let mut items = Vec::new();
        for (a, b) in split_commas(toks, from + 1, to - 1) {
            items.push(literal(toks, a, b)?);
        }
        return Some(Lit::Array(items));
    }
    if first.is("{") {
        let mut pairs: Vec<(String, Lit)> = Vec::new();
        for p in object_props(toks, from, to)? {
            let v = literal(toks, p.value.0, p.value.1)?;
            // A repeated key: JavaScript keeps the first place and the last value.
            if let Some(slot) = pairs.iter_mut().find(|(k, _)| *k == p.key) {
                slot.1 = v;
            } else {
                pairs.push((p.key, v));
            }
        }
        return Some(Lit::Object(pairs));
    }
    None
}

fn number(n: &str, negative: bool) -> Option<Lit> {
    let (whole, frac) = match n.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (n, None),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    if !digits(whole) || (whole.len() > 1 && whole.starts_with('0')) {
        return None;
    }
    if let Some(f) = frac
        && !digits(f)
    {
        return None;
    }
    // Rhai's integers are 64-bit; JavaScript's numbers are doubles, exact to 2^53.
    if frac.is_none() && whole.len() > 15 {
        return None;
    }
    Some(Lit::Num(if negative {
        format!("-{n}")
    } else {
        n.to_string()
    }))
}

/// A Rhai string literal for `s`.
pub(crate) fn rhai_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            '\u{85}' | '\u{2028}' | '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", ch as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Rhai's reserved words, which a map key can't be written as bare.
const RHAI_RESERVED: &[&str] = &[
    "true",
    "false",
    "let",
    "const",
    "if",
    "else",
    "switch",
    "do",
    "while",
    "until",
    "loop",
    "for",
    "in",
    "continue",
    "break",
    "return",
    "throw",
    "try",
    "catch",
    "import",
    "export",
    "as",
    "global",
    "private",
    "fn",
    "this",
    "is",
    "var",
    "static",
    "shared",
    "sync",
    "async",
    "await",
    "yield",
    "default",
    "void",
    "null",
    "nil",
    "spawn",
    "thread",
    "go",
    "exit",
    "goto",
    "match",
    "case",
    "public",
    "protected",
    "new",
    "use",
    "with",
    "module",
    "package",
    "super",
    "type_of",
    "print",
    "debug",
    "eval",
    "Fn",
    "call",
    "curry",
];

fn rhai_key(k: &str) -> String {
    let bare = k.bytes().next().is_some_and(|c| c.is_ascii_alphabetic())
        && k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && !RHAI_RESERVED.contains(&k);
    if bare { k.to_string() } else { rhai_string(k) }
}

/// The Rhai for `lit`, starting at `indent` levels.
pub(crate) fn rhai(lit: &Lit, indent: usize) -> String {
    let flat = rhai_flat(lit);
    if indent * 4 + flat.len() <= 76 {
        return flat;
    }
    let pad = "    ".repeat(indent + 1);
    let close = "    ".repeat(indent);
    match lit {
        Lit::Array(items) => {
            let mut s = "[\n".to_string();
            for v in items {
                s += &format!("{pad}{},\n", rhai(v, indent + 1));
            }
            format!("{s}{close}]")
        }
        Lit::Object(pairs) => {
            let mut s = "#{\n".to_string();
            for (k, v) in pairs {
                s += &format!("{pad}{}: {},\n", rhai_key(k), rhai(v, indent + 1));
            }
            format!("{s}{close}}}")
        }
        _ => flat,
    }
}

fn rhai_flat(lit: &Lit) -> String {
    match lit {
        Lit::Str(s) => rhai_string(s),
        Lit::Num(n) => n.clone(),
        Lit::Bool(b) => b.to_string(),
        Lit::Null => "null_value()".into(),
        Lit::Array(items) => format!(
            "[{}]",
            items.iter().map(rhai_flat).collect::<Vec<_>>().join(", ")
        ),
        Lit::Object(pairs) if pairs.is_empty() => "#{}".into(),
        Lit::Object(pairs) => format!(
            "#{{ {} }}",
            pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", rhai_key(k), rhai_flat(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `#{ }` maps sort their keys, and JavaScript objects keep integer-like keys
/// first. True when an object's keys come out in the same order either way.
pub(crate) fn order_safe(lit: &Lit) -> bool {
    match lit {
        Lit::Object(pairs) => {
            pairs
                .iter()
                .all(|(k, _)| !k.bytes().all(|c| c.is_ascii_digit()))
                && pairs.windows(2).all(|w| w[0].0 < w[1].0)
                && pairs.iter().all(|(_, v)| order_safe(v))
        }
        Lit::Array(items) => items.iter().all(order_safe),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(src: &str) -> Option<Lit> {
        let t = tokenize(src).unwrap();
        literal(&t, 0, t.len())
    }

    #[test]
    fn reads_literals() {
        assert_eq!(lit("'a\\'b'"), Some(Lit::Str("a'b".into())));
        assert_eq!(lit("`plain`"), Some(Lit::Str("plain".into())));
        assert_eq!(lit("`x${y}`"), None);
        assert_eq!(lit("-2"), Some(Lit::Num("-2".into())));
        assert_eq!(lit("0x10"), None);
        assert_eq!(lit("1e3"), None);
        assert_eq!(
            lit("{ a: [1, 'x'], 'b-c': null, }"),
            Some(Lit::Object(vec![
                (
                    "a".into(),
                    Lit::Array(vec![Lit::Num("1".into()), Lit::Str("x".into())])
                ),
                ("b-c".into(), Lit::Null),
            ]))
        );
        assert_eq!(lit("{ a }"), None);
        assert_eq!(lit("{ ...a }"), None);
        assert_eq!(lit("f()"), None);
        assert_eq!(lit("'\\ud83d\\ude00'"), Some(Lit::Str("😀".into())));
        assert_eq!(lit("'\\ud83d'"), None);
    }

    #[test]
    fn splits_statements_with_and_without_semicolons() {
        let src = "a.b(1)\nc.d({\n x: 1\n})\nif (x) {\n} else {\n}\nreturn { y: 2 };";
        let t = tokenize(src).unwrap();
        let s = statements(&t, 0, t.len());
        let text: Vec<&str> = s
            .iter()
            .map(|&(a, b)| &src[t[a].start..t[b - 1].end])
            .collect();
        assert_eq!(
            text,
            [
                "a.b(1)",
                "c.d({\n x: 1\n})",
                "if (x) {\n} else {\n}",
                "return { y: 2 }"
            ]
        );
    }

    #[test]
    fn regex_and_comments() {
        let t = tokenize("x = /a\\/[/]b/g; // c\n/* d */ y = 4 / 2").unwrap();
        assert!(t.iter().any(|t| t.tok == Tok::Regex));
        assert_eq!(t.iter().filter(|t| t.is("/")).count(), 1);
        assert!(tokenize("'open").is_err());
    }

    #[test]
    fn writes_rhai() {
        let v = lit("{ name: 'a\"b', type: 'x', 'data-x': 1, n: null }").unwrap();
        assert_eq!(
            rhai(&v, 0),
            "#{ name: \"a\\\"b\", type: \"x\", \"data-x\": 1, n: null_value() }"
        );
        assert!(!order_safe(&lit("{ b: 1, a: 2 }").unwrap()));
        assert!(!order_safe(&lit("{ 2: 1 }").unwrap()));
        assert!(order_safe(&lit("{ a: 1, b: { c: 1 } }").unwrap()));
    }
}
