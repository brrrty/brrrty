//! JavaScript data files, sorted by what `brrrty init` can do with each:
//! translate it to Rhai (its value is a literal), leave it for a hand port
//! (it only uses what Brrrty has helpers for), or keep running it in Node.

use super::js::{Lit, Tok, Token, literal, order_safe, rhai, statements, tokenize};

/// What `init` does with one JavaScript data file.
#[derive(Debug, Clone, PartialEq)]
pub enum DataClass {
    /// Its export is a literal (or a function that only returns one), and a
    /// Rhai map keeps its key order: the Rhai for it.
    Translatable(String),
    /// It only reads files, parses JSON or CSV, joins paths, makes dates,
    /// formats for en-US or compares strings: Brrrty's helpers for those.
    HandPort(Vec<&'static str>),
    /// Anything else: it keeps running in Node.
    NeedsNode,
}

/// Modules a hand port can stand in for, and the Brrrty helper that does.
const PORTABLE_MODULES: &[(&str, &str)] = &[
    ("fs", "read_text, read_json, read_dir"),
    ("node:fs", "read_text, read_json, read_dir"),
    ("fs/promises", "read_text, read_json, read_dir"),
    ("node:fs/promises", "read_text, read_json, read_dir"),
    ("path", "path_join"),
    ("node:path", "path_join"),
    ("csv-parse/sync", "parse_csv"),
];

/// Names that mean the file needs Node: the network, the environment,
/// other processes, timers.
const NODE_ONLY: &[&str] = &[
    "fetch",
    "process",
    "child_process",
    "setTimeout",
    "setInterval",
    "globalThis",
    "eval",
    "Function",
    "XMLHttpRequest",
    "WebSocket",
];

/// Sort the data file whose text is `src`.
pub fn classify(src: &str) -> DataClass {
    let Ok(toks) = tokenize(src) else {
        return DataClass::NeedsNode;
    };
    // `__proto__` in a literal sets the prototype, not a key.
    let proto = toks.iter().any(|t| {
        t.is_ident("__proto__") || matches!(&t.tok, Tok::Str(Some(s)) if s == "__proto__")
    });
    if proto {
        return DataClass::NeedsNode;
    }
    if let Some(lit) = exported_literal(&toks) {
        if order_safe(&lit) {
            return DataClass::Translatable(rhai(&lit, 0));
        }
        // A Rhai map sorts its keys: `object()` keeps them in order.
        return DataClass::HandPort(vec!["object"]);
    }
    // Every module it imports must be one a helper stands in for.
    let mut helpers: Vec<&'static str> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if let Some(name) = t.ident()
            && NODE_ONLY.contains(&name)
            && !toks.get(i.wrapping_sub(1)).is_some_and(|p| p.is("."))
        {
            return DataClass::NeedsNode;
        }
        let module = if t.is_ident("require") && toks.get(i + 1).is_some_and(|p| p.is("(")) {
            match toks.get(i + 2).map(|t| &t.tok) {
                Some(Tok::Str(Some(m))) => Some(m.clone()),
                _ => return DataClass::NeedsNode,
            }
        } else if t.is_ident("from")
            || (t.is_ident("import") && !toks.get(i + 1).is_some_and(|p| p.is(".")))
        {
            match toks.get(i + 1).map(|t| &t.tok) {
                Some(Tok::Str(Some(m))) => Some(m.clone()),
                // `import x from "m"`: the module comes after `from`.
                _ if t.is_ident("import") => None,
                _ => None,
            }
        } else {
            None
        };
        if let Some(m) = module {
            match PORTABLE_MODULES.iter().find(|(name, _)| *name == m) {
                Some((_, h)) => {
                    if !helpers.contains(h) {
                        helpers.push(h);
                    }
                }
                None => return DataClass::NeedsNode,
            }
        }
        // `import(...)` loads code at run time.
        if t.is_ident("import") && toks.get(i + 1).is_some_and(|p| p.is("(")) {
            return DataClass::NeedsNode;
        }
    }
    for (word, helper) in [
        ("JSON", "parse_json"),
        ("Date", "date"),
        ("toLocaleString", "to_locale_string"),
        ("localeCompare", "locale_compare"),
    ] {
        if toks.iter().any(|t| t.is_ident(word)) && !helpers.contains(&helper) {
            helpers.push(helper);
        }
    }
    DataClass::HandPort(helpers)
}

/// The literal a file exports: `module.exports = <lit>`, `export default
/// <lit>`, or either of those as a function whose body only returns one.
fn exported_literal(toks: &[Token]) -> Option<Lit> {
    let stmts = statements(toks, 0, toks.len());
    let [(a, b)] = stmts.as_slice() else {
        return None;
    };
    let (a, mut b) = (*a, *b);
    if b > a && toks[b - 1].is(";") {
        b -= 1;
    }
    let start = if toks[a].is_ident("module")
        && toks.get(a + 1)?.is(".")
        && toks.get(a + 2)?.is_ident("exports")
        && toks.get(a + 3)?.is("=")
    {
        a + 4
    } else if toks[a].is_ident("export") && toks.get(a + 1)?.is_ident("default") {
        a + 2
    } else {
        return None;
    };
    if let Some(lit) = literal(toks, start, b) {
        return Some(lit);
    }
    returned_literal(toks, start, b)
}

/// The literal a function expression returns, when returning it is all the
/// function does: `function () { return <lit>; }`, `() => <lit>`,
/// `() => (<lit>)`, `() => { return <lit>; }`, `async` forms too.
fn returned_literal(toks: &[Token], from: usize, to: usize) -> Option<Lit> {
    let mut i = from;
    if toks.get(i)?.is_ident("async") {
        i += 1;
    }
    let body_start = if toks.get(i)?.is_ident("function") {
        i += 1;
        if toks.get(i)?.ident().is_some() {
            i += 1;
        }
        // No parameters: anything read from them isn't a literal anyway,
        // but a default value would run code.
        if !(toks.get(i)?.is("(") && toks.get(i + 1)?.is(")")) {
            return None;
        }
        i + 2
    } else {
        if !(toks.get(i)?.is("(") && toks.get(i + 1)?.is(")") && toks.get(i + 2)?.is("=>")) {
            return None;
        }
        let at = i + 3;
        if !toks.get(at)?.is("{") {
            // An expression body.
            if toks[at].is("(") && toks[at].pair == Some(to - 1) {
                return literal(toks, at + 1, to - 1);
            }
            return literal(toks, at, to);
        }
        at
    };
    if !toks.get(body_start)?.is("{") || toks[body_start].pair != Some(to - 1) {
        return None;
    }
    let inner = statements(toks, body_start + 1, to - 1);
    let [(a, b)] = inner.as_slice() else {
        return None;
    };
    let (a, mut b) = (*a, *b);
    if !toks[a].is_ident("return") {
        return None;
    }
    if b > a && toks[b - 1].is(";") {
        b -= 1;
    }
    if toks.get(a + 1)?.is("(") && toks[a + 1].pair == Some(b - 1) {
        return literal(toks, a + 2, b - 1);
    }
    literal(toks, a + 1, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translated(src: &str) -> String {
        match classify(src) {
            DataClass::Translatable(r) => r,
            other => panic!("{src}: {other:?}"),
        }
    }

    #[test]
    fn literal_exports_translate() {
        assert_eq!(
            translated("module.exports = { a: 1, b: [\"x\", null] };"),
            "#{ a: 1, b: [\"x\", null_value()] }"
        );
        assert_eq!(
            translated("export default { name: \"Site\" };"),
            "#{ name: \"Site\" }"
        );
        assert_eq!(
            translated("module.exports = function () {\n  return { a: true };\n};"),
            "#{ a: true }"
        );
        assert_eq!(
            translated("module.exports = () => ({ a: 2 });"),
            "#{ a: 2 }"
        );
        assert_eq!(
            translated("export default async () => { return [1, 2]; }"),
            "[1, 2]"
        );
    }

    #[test]
    fn key_order_rhai_cannot_keep_is_not_translated() {
        // Rhai sorts map keys; JavaScript keeps `b` before `a`.
        assert_eq!(
            classify("module.exports = { b: 1, a: 2 };"),
            DataClass::HandPort(vec!["object"])
        );
        assert_eq!(
            classify("module.exports = { 2: 1, 10: 2 };"),
            DataClass::HandPort(vec!["object"])
        );
    }

    #[test]
    fn file_reading_code_is_a_hand_port_with_its_helpers() {
        let src = "const fs = require(\"fs\");\nconst path = require(\"path\");\nmodule.exports = JSON.parse(fs.readFileSync(path.join(__dirname, \"x.json\"), \"utf8\"));";
        assert_eq!(
            classify(src),
            DataClass::HandPort(vec![
                "read_text, read_json, read_dir",
                "path_join",
                "parse_json"
            ])
        );
        let csv = "import { parse } from \"csv-parse/sync\";\nimport fs from \"node:fs\";\nexport default parse(fs.readFileSync(\"a.csv\"), { columns: true });";
        assert_eq!(
            classify(csv),
            DataClass::HandPort(vec!["parse_csv", "read_text, read_json, read_dir"])
        );
    }

    #[test]
    fn packages_the_network_and_the_environment_need_node() {
        for src in [
            "const { DateTime } = require(\"luxon\");\nmodule.exports = DateTime.now().year;",
            "module.exports = async () => (await fetch(\"https://example.com\")).json();",
            "module.exports = { env: process.env.NODE_ENV };",
            "const { execSync } = require(\"child_process\");\nmodule.exports = execSync(\"git log\").toString();",
            "import x from \"./other.js\";\nexport default x;",
            "module.exports = require(name);",
            "export default await import(\"./x.js\");",
            "module.exports = {",
        ] {
            assert_eq!(classify(src), DataClass::NeedsNode, "{src}");
        }
    }
}
