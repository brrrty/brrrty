//! The top-level keys a JavaScript data file exports, read off its source
//! without running it.
//!
//! Brrrty can't run JavaScript, so it only claims to know a file's keys
//! when the whole file is one canonical statement, with comments around
//! it:
//!
//! ```js
//! module.exports = { ... };   // or, as a module:
//! export default { ... };
//! ```
//!
//! The object's values must be ones that run nothing when the file loads:
//! literals, lists and objects of them, and functions (whose bodies only
//! run if something calls them, and a page that reads the key is refused
//! anyway). Anything else, anywhere, gives `None`: the keys are unknown.
//! Function bodies are only checked for balanced brackets.

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    /// A quoted string, and whether it had an escape.
    Str(String, bool),
    Num(String),
    Punct(char),
    /// A template literal without `${`.
    Template,
}

/// Which export forms the file may use, from its extension and package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModuleKind {
    /// `.cjs`, or `.js` in a `"type": "commonjs"` package.
    CommonJs,
    /// `.mjs`, or `.js` in a `"type": "module"` package.
    Esm,
    /// `.js` with no package type: Node detects either (probed).
    Either,
}

/// Tokens of `src`, or `None` where Brrrty can't be sure of them: a `/`
/// that isn't a comment may start a regular expression, and a template
/// literal with `${` holds code.
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
            let mut escaped = false;
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
                    escaped = true;
                    chars.get(i + 1)?;
                    i += 2;
                    continue;
                }
                s.push(ch);
                i += 1;
            }
            out.push(Tok::Str(s, escaped));
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
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    Some(out)
}

struct Parser {
    toks: Vec<Tok>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn peek_at(&self, n: usize) -> Option<&Tok> {
        self.toks.get(self.at + n)
    }

    fn eat(&mut self, t: &Tok) -> Option<()> {
        (self.peek()? == t).then(|| self.at += 1)
    }

    fn eat_ident(&mut self, name: &str) -> Option<()> {
        matches!(self.peek(), Some(Tok::Ident(s)) if s == name).then(|| self.at += 1)
    }

    fn is_punct(&self, c: char) -> bool {
        self.peek() == Some(&Tok::Punct(c))
    }

    /// Skip the bracketed group starting here, checking that every bracket
    /// closes with its own kind.
    fn group(&mut self, open: char) -> Option<()> {
        if !self.is_punct(open) {
            return None;
        }
        let mut stack: Vec<char> = Vec::new();
        loop {
            let t = self.peek()?.clone();
            self.at += 1;
            match t {
                Tok::Punct(o @ ('(' | '[' | '{')) => stack.push(o),
                Tok::Punct(c @ (')' | ']' | '}')) => {
                    let want = match stack.pop()? {
                        '(' => ')',
                        '[' => ']',
                        _ => '}',
                    };
                    if c != want {
                        return None;
                    }
                    if stack.is_empty() {
                        return Some(());
                    }
                }
                _ => {}
            }
        }
    }

    /// `{ key: value, ... }`, returning its keys.
    fn object(&mut self) -> Option<Vec<String>> {
        self.eat(&Tok::Punct('{'))?;
        let mut keys = Vec::new();
        loop {
            if self.eat(&Tok::Punct('}')).is_some() {
                return Some(keys);
            }
            let key = match self.peek()? {
                Tok::Ident(s) => s.clone(),
                Tok::Str(s, false) => s.clone(),
                // A number key is canonical in JavaScript (`1.0` is `"1"`).
                Tok::Num(n) if n.parse::<u32>().is_ok_and(|v| v.to_string() == *n) => n.clone(),
                _ => return None,
            };
            self.at += 1;
            if self.eat(&Tok::Punct(':')).is_some() {
                self.value()?;
            } else if self.is_punct('(') {
                // A method: its parameters, then its body.
                self.group('(')?;
                self.group('{')?;
            } else {
                // Shorthand names a variable, and getters, setters and
                // async methods hide what they do.
                return None;
            }
            keys.push(key);
            if self.eat(&Tok::Punct(',')).is_none() && !self.is_punct('}') {
                return None;
            }
        }
    }

    /// `[ value, ... ]`, without holes.
    fn array(&mut self) -> Option<()> {
        self.eat(&Tok::Punct('['))?;
        loop {
            if self.eat(&Tok::Punct(']')).is_some() {
                return Some(());
            }
            self.value()?;
            if self.eat(&Tok::Punct(',')).is_none() && !self.is_punct(']') {
                return None;
            }
        }
    }

    /// A value that runs nothing when the file loads.
    fn value(&mut self) -> Option<()> {
        let t = self.peek()?.clone();
        match t {
            Tok::Str(..) | Tok::Template => {
                self.at += 1;
                Some(())
            }
            Tok::Num(n) => {
                n.parse::<f64>().ok()?;
                self.at += 1;
                Some(())
            }
            Tok::Punct('-' | '+') if matches!(self.peek_at(1), Some(Tok::Num(_))) => {
                self.at += 1;
                self.value()
            }
            Tok::Punct('[') => self.array(),
            Tok::Punct('{') => self.object().map(|_| ()),
            Tok::Punct('(') => self.arrow(),
            Tok::Ident(s) => match s.as_str() {
                "true" | "false" | "null" | "undefined" | "NaN" | "Infinity" => {
                    self.at += 1;
                    Some(())
                }
                "function" => self.function(),
                "async" => {
                    self.at += 1;
                    match self.peek()? {
                        Tok::Ident(f) if f == "function" => self.function(),
                        _ => self.arrow(),
                    }
                }
                // `x => ...`
                _ if self.peek_at(1) == Some(&Tok::Punct('='))
                    && self.peek_at(2) == Some(&Tok::Punct('>')) =>
                {
                    self.arrow()
                }
                // Any other name is a variable or a global: unknown.
                _ => None,
            },
            _ => None,
        }
    }

    /// `function name(params) { body }`.
    fn function(&mut self) -> Option<()> {
        self.eat_ident("function")?;
        if self.is_punct('*') {
            return None;
        }
        if matches!(self.peek(), Some(Tok::Ident(_))) {
            self.at += 1;
        }
        self.group('(')?;
        self.group('{')
    }

    /// `(params) => body` or `name => body`.
    fn arrow(&mut self) -> Option<()> {
        match self.peek()? {
            Tok::Ident(_) => self.at += 1,
            _ => self.group('(')?,
        }
        self.eat(&Tok::Punct('='))?;
        self.eat(&Tok::Punct('>'))?;
        if self.is_punct('{') {
            return self.group('{');
        }
        // A concise body: up to the comma or bracket that ends the value.
        let start = self.at;
        loop {
            match self.peek()? {
                Tok::Punct(',' | '}' | ']') => break,
                Tok::Punct(o @ ('(' | '[' | '{')) => {
                    let o = *o;
                    self.group(o)?;
                }
                Tok::Punct(')') => return None,
                _ => self.at += 1,
            }
        }
        (self.at > start).then_some(())
    }
}

fn count(src: &str, word: &str) -> usize {
    src.matches(word).count()
}

/// The keys `src` exports, when it is exactly one canonical export of an
/// object literal; `kind` says which export forms the file can use.
pub(crate) fn export_keys(src: &str, kind: ModuleKind) -> Option<Vec<String>> {
    // Any other mention of these, in code, strings or comments, could
    // change what the file exports.
    if src.contains("defineProperty") {
        return None;
    }
    let (modules, exports, export) = (
        count(src, "module"),
        count(src, "exports"),
        count(src, "export"),
    );
    let esm = match (modules, exports, export) {
        (1, 1, 1) => false,
        (0, 0, 1) => true,
        _ => return None,
    };
    match (kind, esm) {
        (ModuleKind::CommonJs, true) | (ModuleKind::Esm, false) => return None,
        _ => {}
    }
    let mut p = Parser {
        toks: tokens(src)?,
        at: 0,
    };
    if esm {
        p.eat_ident("export")?;
        p.eat_ident("default")?;
    } else {
        p.eat_ident("module")?;
        p.eat(&Tok::Punct('.'))?;
        p.eat_ident("exports")?;
        p.eat(&Tok::Punct('='))?;
    }
    let keys = p.object()?;
    let _ = p.eat(&Tok::Punct(';'));
    p.peek().is_none().then_some(keys)
}

/// The module kind of the `.js`, `.cjs` or `.mjs` file at `full`: by its
/// extension, or for `.js` by the `type` of the nearest `package.json`.
pub(crate) fn module_kind(full: &std::path::Path) -> ModuleKind {
    let name = full.to_string_lossy();
    if name.ends_with(".cjs") {
        return ModuleKind::CommonJs;
    }
    if name.ends_with(".mjs") {
        return ModuleKind::Esm;
    }
    let mut dir = full.parent();
    while let Some(d) = dir {
        if let Ok(text) = std::fs::read_to_string(d.join("package.json")) {
            let ty = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string));
            return match ty.as_deref() {
                Some("module") => ModuleKind::Esm,
                Some("commonjs") => ModuleKind::CommonJs,
                _ => ModuleKind::Either,
            };
        }
        dir = d.parent();
    }
    ModuleKind::Either
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(src: &str) -> Option<Vec<String>> {
        export_keys(src, ModuleKind::Either)
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reads_plain_object_literals() {
        assert_eq!(
            keys(
                "// Keys\nmodule.exports = {\n  greeting: \"hi\", // c\n  'nested': { a: [1, -2, 'x\\'y'], b: `t` },\n  fn(x) { return { y: x }; },\n  arrow: (d) => d.x,\n  one: x => ({ x }),\n  f: async function named() {},\n  1: 'one',\n};\n// after\n"
            ),
            Some(strs(&[
                "greeting", "nested", "fn", "arrow", "one", "f", "1"
            ]))
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

    /// The reviewer's reproductions, each of which 11ty writes somewhere
    /// else (probed), and the forms around them.
    #[test]
    fn anything_but_the_canonical_export_is_unknown() {
        for src in [
            // Code after the object.
            "module.exports = {x:1} && {permalink:\"/a/\"};",
            "module.exports = {x:1}; module.exports.permalink = '/b/';",
            "module.exports = {x:1}\n;(0)",
            // Other ways to name the export.
            "module.exports = {x:1}; module[\"exports\"] = {permalink:\"/m/\"};",
            "module['exports'] = {permalink: '/m/'};",
            "Object.defineProperty(module,\"exports\",{value:{permalink:\"/d/\"}});",
            "module.exports = {x: 1}; // module.exports gets replaced below",
            "exports.a = 1;",
            "export const a = 1; export default {};",
            // Code before it.
            "globalThis.x = 1; module.exports = {a: 1};",
            "import x from './x.js';\nexport default { a: x };",
            // Values that run something.
            "module.exports = function () { return {}; };",
            "module.exports = data;",
            "module.exports = { a: f() };",
            "module.exports = { a: new Date() };",
            "module.exports = { a: x };",
            "module.exports = { short };",
            "module.exports = { ...base, a: 1 };",
            "module.exports = { [k]: 1 };",
            "module.exports = { get a() { return 1; } };",
            "module.exports = { async a() {} };",
            "module.exports = { *gen() {} };",
            "module.exports = { a: /x/ };",
            "module.exports = { a: 4 / 2 };",
            "module.exports = { a: 1 + 2 };",
            "module.exports = { a: `${x}` };",
            "module.exports = { 'a\\nb': 1 };",
            "module.exports = { 1.5: 1 };",
            "module.exports = { a: [1,,2] };",
            "module.exports == { a: 1 };",
            "export default async function () { return {}; }",
            "",
        ] {
            assert_eq!(keys(src), None, "{src}");
        }
    }

    /// Syntax errors, which stop 11ty (probed), are never read as valid.
    #[test]
    fn syntax_errors_are_unknown() {
        for src in [
            "module.exports = {a: 1 b: 2};",
            "module.exports = { a: (1] };",
            "module.exports = { f() { return [1); } };",
            "module.exports = { a: };",
            "module.exports = { a: 1,, b: 2 };",
            "module.exports = { a: 1",
            "module.exports = { a: x => };",
            "module.exports = { a: 'open };",
            "/* open",
        ] {
            assert_eq!(keys(src), None, "{src}");
        }
    }

    /// Probed: `.mjs` can't use `module.exports`, `.cjs` can't use
    /// `export default`, and `.js` follows its package's `type`.
    #[test]
    fn the_export_form_must_suit_the_module_kind() {
        let cjs = "module.exports = { a: 1 };";
        let esm = "export default { a: 1 };";
        assert_eq!(export_keys(cjs, ModuleKind::Esm), None);
        assert_eq!(export_keys(esm, ModuleKind::CommonJs), None);
        assert!(export_keys(cjs, ModuleKind::CommonJs).is_some());
        assert!(export_keys(esm, ModuleKind::Esm).is_some());
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("package.json"), r#"{"type": "module"}"#).unwrap();
        std::fs::create_dir_all(t.path().join("src")).unwrap();
        assert_eq!(module_kind(&t.path().join("src/a.js")), ModuleKind::Esm);
        assert_eq!(
            module_kind(&t.path().join("src/a.cjs")),
            ModuleKind::CommonJs
        );
    }
}
