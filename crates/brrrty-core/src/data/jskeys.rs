//! The top-level keys a JavaScript data file exports, read off its source
//! without running it.
//!
//! Brrrty can't run JavaScript, but a data file written as
//! `module.exports = { ... }` or `export default { ... }` with an object
//! literal names its keys plainly. Anything less plain (a function, a
//! variable, spread or computed keys, a second export, a regular expression
//! anywhere) gives `None`: the keys are unknown.

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Num(String),
    Punct(char),
    /// `...`
    Spread,
    /// A template literal; its text doesn't matter here.
    Template,
}

/// Tokens of `src`, or `None` where Brrrty can't be sure of them: a `/`
/// that isn't a comment may start a regular expression, and a template
/// literal with `${` may hold code.
fn tokens(src: &str) -> Option<Vec<Tok>> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            i += 2;
            loop {
                if i + 1 >= chars.len() {
                    return None;
                }
                if chars[i] == '*' && chars[i + 1] == '/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
        } else if c == '/' {
            return None;
        } else if c == '"' || c == '\'' {
            let mut s = String::new();
            i += 1;
            loop {
                let ch = *chars.get(i)?;
                if ch == c {
                    i += 1;
                    break;
                }
                if ch == '\n' {
                    return None;
                }
                if ch == '\\' {
                    // Escapes in keys are rare; refuse rather than decode.
                    return None;
                }
                s.push(ch);
                i += 1;
            }
            out.push(Tok::Str(s));
        } else if c == '`' {
            i += 1;
            loop {
                let ch = *chars.get(i)?;
                if ch == '`' {
                    i += 1;
                    break;
                }
                if ch == '\\' {
                    i += 2;
                    continue;
                }
                if ch == '$' && chars.get(i + 1) == Some(&'{') {
                    return None;
                }
                i += 1;
            }
            out.push(Tok::Template);
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                i += 1;
            }
            out.push(Tok::Num(chars[start..i].iter().collect()));
        } else if c == '_' || c == '$' || c.is_alphabetic() {
            let start = i;
            while i < chars.len()
                && (chars[i] == '_' || chars[i] == '$' || chars[i].is_alphanumeric())
            {
                i += 1;
            }
            out.push(Tok::Ident(chars[start..i].iter().collect()));
        } else if c == '.' && next == Some('.') && chars.get(i + 2) == Some(&'.') {
            out.push(Tok::Spread);
            i += 3;
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    Some(out)
}

fn is_ident(t: Option<&Tok>, name: &str) -> bool {
    matches!(t, Some(Tok::Ident(s)) if s == name)
}

/// The index just past the bracket that closes the one at `open`.
fn skip_group(toks: &[Tok], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate().skip(open) {
        match t {
            Tok::Punct('(' | '[' | '{') => depth += 1,
            Tok::Punct(')' | ']' | '}') => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// The keys of the object literal whose `{` is at `open`.
fn object_keys(toks: &[Tok], open: usize) -> Option<Vec<String>> {
    let mut keys = Vec::new();
    let mut i = open + 1;
    loop {
        let key = match toks.get(i)? {
            Tok::Punct('}') => return Some(keys),
            Tok::Ident(s) | Tok::Str(s) | Tok::Num(s) => s.clone(),
            // Spread, computed keys and generators hide what is set.
            _ => return None,
        };
        // `get x()`, `set x(v)` and `async x()`.
        if matches!(
            toks.get(i + 1),
            Some(Tok::Ident(_) | Tok::Str(_) | Tok::Num(_))
        ) {
            return None;
        }
        i += 1;
        match toks.get(i)? {
            Tok::Punct(':') => {
                // Skip the value, up to the comma or brace that ends it.
                i += 1;
                loop {
                    match toks.get(i)? {
                        Tok::Punct('(' | '[' | '{') => i = skip_group(toks, i)?,
                        Tok::Punct(')' | ']') => return None,
                        Tok::Punct(',' | '}') => break,
                        _ => i += 1,
                    }
                }
            }
            Tok::Punct('(') => {
                // A method: its parameters, then its body.
                i = skip_group(toks, i)?;
                if toks.get(i)? != &Tok::Punct('{') {
                    return None;
                }
                i = skip_group(toks, i)?;
            }
            Tok::Punct(',' | '}') => {}
            _ => return None,
        }
        keys.push(key);
        if toks.get(i)? == &Tok::Punct(',') {
            i += 1;
        }
    }
}

/// The keys `src` exports, when it is plainly an object literal.
pub(crate) fn export_keys(src: &str) -> Option<Vec<String>> {
    let toks = tokens(src)?;
    let exports = toks
        .iter()
        .filter(|t| matches!(t, Tok::Ident(s) if s == "exports" || s == "export"))
        .count();
    if exports != 1 {
        return None;
    }
    let at = toks
        .iter()
        .position(|t| matches!(t, Tok::Ident(s) if s == "exports" || s == "export"))?;
    let open = if is_ident(toks.get(at), "export") {
        if !is_ident(toks.get(at + 1), "default") {
            return None;
        }
        at + 2
    } else {
        // `module.exports =`
        if at < 2 || !is_ident(toks.get(at - 2), "module") || toks[at - 1] != Tok::Punct('.') {
            return None;
        }
        if toks.get(at + 1) != Some(&Tok::Punct('=')) || toks.get(at + 2) == Some(&Tok::Punct('='))
        {
            return None;
        }
        at + 2
    };
    if toks.get(open) != Some(&Tok::Punct('{')) {
        return None;
    }
    let keys = object_keys(&toks, open)?;
    // A number key is canonical in JavaScript (`1.0` is `"1"`); refuse any
    // that isn't written that way.
    if keys.iter().any(|k| {
        k.starts_with(|c: char| c.is_ascii_digit())
            && k.parse::<u32>().map(|n| n.to_string()) != Ok(k.clone())
    }) {
        return None;
    }
    Some(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(src: &str) -> Option<Vec<String>> {
        export_keys(src)
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reads_plain_object_literals() {
        assert_eq!(
            keys(
                "// Keys\nmodule.exports = {\n  greeting: \"hi\", // c\n  'nested': { a: [1, 2], b: `t` },\n  fn(x) { return { y: x }; },\n  short,\n  1: 'one',\n};\n"
            ),
            Some(strs(&["greeting", "nested", "fn", "short", "1"]))
        );
        assert_eq!(
            keys("import x from './x.js';\nexport default { a: x(1, 2), b: { c: [d, e] } }"),
            Some(strs(&["a", "b"]))
        );
        assert_eq!(
            keys("export default {\n  eleventyComputed: { title: (data) => data.x },\n};"),
            Some(strs(&["eleventyComputed"]))
        );
        assert_eq!(keys("module.exports = {};"), Some(vec![]));
        assert_eq!(
            keys("/* x */ module.exports = { a: 1 }"),
            Some(strs(&["a"]))
        );
    }

    #[test]
    fn anything_less_plain_is_unknown() {
        for src in [
            "module.exports = function () { return {}; };",
            "module.exports = data;",
            "const d = {a: 1}; module.exports = d;",
            "module.exports = { ...base, a: 1 };",
            "module.exports = { [k]: 1 };",
            "module.exports = { get a() { return 1; } };",
            "module.exports = { async a() {} };",
            "module.exports = { *gen() {} };",
            "module.exports = { a: 1 }; module.exports.b = 2;",
            "exports.a = 1;",
            "module.exports = { a: /x/ };",
            "module.exports = { a: 4 / 2 };",
            "module.exports = { a: `${x}` };",
            "module.exports = { 'a\\nb': 1 };",
            "module.exports = { 1.5: 1 };",
            "module.exports == { a: 1 };",
            "export const a = 1;",
            "export default async function () { return {}; }",
            "module.exports = { a: 1",
            "/* open",
            "",
        ] {
            assert_eq!(export_keys(src), None, "{src}");
        }
    }
}
