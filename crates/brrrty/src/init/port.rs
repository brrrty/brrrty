//! Writes `brrrty.config.rhai` from an 11ty config file, reading it without
//! running it.
//!
//! Only what can be read exactly becomes Rhai: the folders, the template
//! engines, passthrough copies and ignores with literal paths, and the plugins
//! Brrrty has, with literal options. Every other statement is quoted as it is,
//! under a `// TODO:` that says where to read about porting it.

use std::collections::HashMap;

use super::js::{self, Lit, Tok, Token};

const DOCS: &str = "https://brrrty.com/docs/configuration/";
const PORTING: &str = "https://github.com/brrrty/brrrty/blob/main/crates/brrrty-config/PORTING.md";
const SYNTAX_HIGHLIGHT: &str = "@11ty/eleventy-plugin-syntaxhighlight";
const RSS: &str = "@11ty/eleventy-plugin-rss";

/// `brrrty.config.rhai`'s text, and what it says.
#[derive(Debug)]
pub(crate) struct Port {
    pub text: String,
    /// How many `// TODO:` blocks it has.
    pub todos: usize,
    /// `dir.input`, when the config sets it.
    pub input: Option<String>,
    /// `dir.output`, when the config sets it.
    pub output: Option<String>,
}

/// What a name from an `import` or `require` stands for.
#[derive(Debug, Clone)]
enum Binding {
    /// The module itself, or its default export.
    Module(String),
    /// One export of the module.
    Export(String, String),
}

enum Item {
    Rhai(String),
    Todo {
        why: String,
        link: String,
        span: (usize, usize),
    },
}

struct Reader<'a> {
    src: &'a str,
    file: &'a str,
    toks: Vec<Token>,
    /// Where each line of `src` starts and ends, without its terminator.
    lines: Vec<(usize, usize)>,
    items: Vec<Item>,
    bindings: HashMap<String, Binding>,
    /// For each name: how often it appears, and how often as an `addPlugin` argument.
    uses: HashMap<String, (usize, usize)>,
    /// `dir.*` and the template engines, with where each was set.
    settings: Vec<Setting>,
    left_data_engine: bool,
    /// Set when nothing can be ported: the whole file becomes one TODO.
    whole: Option<String>,
}

struct Setting {
    /// `dir.input`, `markdownTemplateEngine` and so on.
    key: &'static str,
    /// The value, as Rhai.
    value: String,
    /// The value as written, for `dir.*`.
    raw: Option<String>,
    span: (usize, usize),
}

/// Line terminators: JavaScript's, and every one Rhai might end a comment at.
fn is_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}' | '\u{85}')
}

/// The lines of `src`: CR LF, LF, CR, U+2028, U+2029 and NEL each end one.
fn split_lines(src: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = src.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if is_terminator(c) {
            out.push((start, i));
            let mut next = i + c.len_utf8();
            if c == '\r' && it.peek().is_some_and(|&(_, d)| d == '\n') {
                it.next();
                next += 1;
            }
            start = next;
        }
    }
    out.push((start, src.len()));
    out
}

/// `text` with every line terminator made a space, safe inside a `//` comment.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if is_terminator(c) { ' ' } else { c })
        .collect()
}

/// Port the 11ty config file `file`, whose text is `src`.
pub(crate) fn port(src: &str, file: &str) -> Port {
    let toks = match js::tokenize(src) {
        Ok(t) => t,
        Err(why) => {
            return unreadable(
                src,
                file,
                &format!("Brrrty couldn't read this file ({why})"),
            );
        }
    };
    let mut r = Reader::new(src, file, toks);
    r.read();
    r.finish()
}

/// A port that is one TODO quoting all of `src`.
pub(crate) fn unreadable(src: &str, file: &str, why: &str) -> Port {
    let mut r = Reader::new(src, file, Vec::new());
    r.whole = Some(why.to_string());
    r.finish()
}

/// Keywords whose `(...) {` is not a function body.
const NOT_METHODS: [&str; 6] = ["if", "for", "while", "switch", "catch", "with"];

impl<'a> Reader<'a> {
    fn new(src: &'a str, file: &'a str, toks: Vec<Token>) -> Self {
        let mut uses: HashMap<String, (usize, usize)> = HashMap::new();
        for (k, t) in toks.iter().enumerate() {
            let Some(name) = t.ident() else { continue };
            if k > 0 && (toks[k - 1].is(".") || toks[k - 1].is("?.")) {
                continue;
            }
            let entry = uses.entry(name.to_string()).or_default();
            entry.0 += 1;
            // `addPlugin(name, ...)` or `addPlugin(name.export, ...)`.
            let call = k >= 3
                && toks[k - 1].is("(")
                && toks[k - 2].is_ident("addPlugin")
                && toks[k - 3].is(".");
            let ends = |i: usize| toks.get(i).is_some_and(|t| t.is(",") || t.is(")"));
            let member = toks.get(k + 1).is_some_and(|t| t.is("."))
                && toks.get(k + 2).and_then(Token::ident).is_some()
                && ends(k + 3);
            if call && (ends(k + 1) || member) {
                entry.1 += 1;
            }
        }
        Reader {
            src,
            file,
            toks,
            lines: split_lines(src),
            items: Vec::new(),
            bindings: HashMap::new(),
            uses,
            settings: Vec::new(),
            left_data_engine: false,
            whole: None,
        }
    }

    fn span(&self, a: usize, b: usize) -> (usize, usize) {
        (self.toks[a].start, self.toks[b - 1].end)
    }

    fn todo(&mut self, a: usize, b: usize, why: impl Into<String>, link: impl Into<String>) {
        let span = self.span(a, b);
        self.items.push(Item::Todo {
            why: why.into(),
            link: link.into(),
            span,
        });
    }

    fn give_up(&mut self, why: impl Into<String>) {
        if self.whole.is_none() {
            self.whole = Some(why.into());
        }
    }

    /// What a top-level statement exports, if anything: `module.exports`,
    /// `exports.x` or `export` (but not `import`).
    fn is_export(&self, a: usize) -> bool {
        let t = &self.toks[a];
        t.is_ident("export")
            || t.is_ident("exports")
            || (t.is_ident("module")
                && self.toks.get(a + 1).is_some_and(|t| t.is("."))
                && self.toks.get(a + 2).is_some_and(|t| t.is_ident("exports")))
    }

    fn read(&mut self) {
        let n = self.toks.len();
        let stmts = js::statements(&self.toks, 0, n);
        // Exactly one config export (and, in ESM, one `export const config`).
        let mut exports = Vec::new();
        let mut config_exports = Vec::new();
        for &(a, b) in &stmts {
            if !self.is_export(a) {
                continue;
            }
            if let Some(rest) = self.export_start(a, b) {
                exports.push((a, rest, b));
            } else if let Some(rest) = self.config_export(a, b) {
                config_exports.push((a, rest, b));
            } else {
                return self.give_up("it exports in a way Brrrty can't follow");
            }
        }
        if exports.len() != 1 || config_exports.len() > 1 {
            return self.give_up(if exports.is_empty() {
                "Brrrty found no config function exported here"
            } else {
                "it exports more than once, and only the last export counts in JavaScript"
            });
        }
        let (export_at, rest, export_end) = exports[0];
        if !config_exports.is_empty() && !self.toks[export_at].is_ident("export") {
            return self.give_up("it mixes `module.exports` with `export`");
        }
        // `module.exports = config` with `function config(...) {...}`.
        let mut config_name: Option<String> = None;
        if rest + 1 == export_end
            && let Some(name) = self.toks[rest].ident()
        {
            let decls = stmts
                .iter()
                .filter(|&&(a, _)| self.fn_decl(a) == Some(name))
                .count();
            if decls != 1 || self.uses.get(name).map_or(0, |u| u.0) != 2 {
                return self.give_up(format!(
                    "the exported `{name}` isn't one function declared once, so Brrrty can't tell what runs"
                ));
            }
            config_name = Some(name.to_string());
        }
        for (a, b) in stmts {
            if self.whole.is_some() {
                return;
            }
            let t = self.toks[a].clone();
            if b == a + 1 && matches!(t.tok, Tok::Str(_)) {
                continue; // "use strict"
            }
            if t.is_ident("import") {
                if !self.import(a, b) {
                    self.todo(a, b, "Brrrty couldn't read this import", PORTING);
                }
                continue;
            }
            if matches!(t.ident(), Some("const" | "let" | "var")) && self.require(a, b) {
                continue;
            }
            if a == export_at {
                if config_name.is_some() {
                    continue;
                }
                if !self.config_fn(rest, b) {
                    self.give_up("Brrrty couldn't read the config function");
                }
                continue;
            }
            if let Some(&(at, rest, _)) = config_exports.first()
                && at == a
            {
                self.returned(rest, b);
                continue;
            }
            if let Some(name) = &config_name
                && self.fn_decl(a) == Some(name.as_str())
            {
                if !self.config_fn(a, b) {
                    self.give_up("Brrrty couldn't read the config function");
                }
                continue;
            }
            self.todo(
                a,
                b,
                "port this code, or drop it if nothing below needs it",
                PORTING,
            );
        }
    }

    /// The name of the function declared at `a`, if it is a declaration.
    fn fn_decl(&self, a: usize) -> Option<&str> {
        let t = &self.toks;
        let k = if t[a].is_ident("async") { a + 1 } else { a };
        if !t.get(k)?.is_ident("function") {
            return None;
        }
        t.get(k + 1)?.ident()
    }

    /// Whether `toks[a..b]` can return or throw at its own level: outside any
    /// function it defines.
    fn may_stop(&self, a: usize, b: usize) -> bool {
        let t = &self.toks;
        let mut k = a;
        while k < b {
            let tok = &t[k];
            let skip_block = |open: usize| -> Option<usize> {
                t.get(open)
                    .filter(|o| o.is("{"))
                    .and_then(|o| o.pair)
                    .map(|c| c + 1)
            };
            if tok.is("=>") {
                if let Some(next) = skip_block(k + 1) {
                    k = next;
                    continue;
                }
            } else if tok.is_ident("function") || tok.is_ident("class") {
                // Past the name and parameters to the body.
                let mut j = k + 1;
                while j < b && !t[j].is("{") && !t[j].is("(") {
                    j += 1;
                }
                if j < b && t[j].is("(") {
                    j = t[j].pair.map_or(b, |c| c + 1);
                }
                while j < b && !t[j].is("{") {
                    j += 1;
                }
                if let Some(next) = skip_block(j) {
                    k = next;
                    continue;
                }
                return true;
            } else if tok.is(")")
                && t.get(k + 1).is_some_and(|n| n.is("{"))
                && let Some(open) = tok.pair
                && open > 0
                && t[open - 1]
                    .ident()
                    .is_some_and(|w| !NOT_METHODS.contains(&w))
                && let Some(next) = skip_block(k + 1)
            {
                // A method: `name(...) { ... }`.
                k = next;
                continue;
            } else if tok.is_ident("return") || tok.is_ident("throw") {
                return true;
            }
            k += 1;
        }
        false
    }

    /// `module.exports = ` or `export default `: the index after it.
    fn export_start(&self, a: usize, b: usize) -> Option<usize> {
        let t = &self.toks;
        if b >= a + 4
            && t[a].is_ident("module")
            && t[a + 1].is(".")
            && t[a + 2].is_ident("exports")
            && t[a + 3].is("=")
        {
            return Some(a + 4);
        }
        if b >= a + 3 && t[a].is_ident("export") && t[a + 1].is_ident("default") {
            return Some(a + 2);
        }
        None
    }

    /// `export const config = ` or `module.exports.config = `: the index after it.
    fn config_export(&self, a: usize, b: usize) -> Option<usize> {
        let t = &self.toks;
        if b > a + 4
            && t[a].is_ident("export")
            && t[a + 1].is_ident("const")
            && t[a + 2].is_ident("config")
            && t[a + 3].is("=")
        {
            return Some(a + 4);
        }
        if b > a + 6
            && t[a].is_ident("module")
            && t[a + 2].is_ident("exports")
            && t[a + 3].is(".")
            && t[a + 4].is_ident("config")
            && t[a + 5].is("=")
        {
            return Some(a + 6);
        }
        None
    }

    /// An `import` statement: records its names.
    fn import(&mut self, a: usize, b: usize) -> bool {
        let t = &self.toks;
        let Some(Tok::Str(Some(module))) = t.get(b - 1).map(|t| &t.tok) else {
            return false;
        };
        let module = module.clone();
        if b == a + 2 {
            return true; // import "x"
        }
        if !t[b - 2].is_ident("from") {
            return false;
        }
        let mut k = a + 1;
        let end = b - 2;
        let mut found = Vec::new();
        while k < end {
            if t[k].is(",") {
                k += 1;
            } else if t[k].is("*") && k + 2 < end + 1 && t[k + 1].is_ident("as") {
                if let Some(n) = t[k + 2].ident() {
                    found.push((n.to_string(), Binding::Module(module.clone())));
                }
                k += 3;
            } else if t[k].is("{") {
                let Some(close) = js::matching(t, k) else {
                    return false;
                };
                for (x, y) in js::split_commas(t, k + 1, close) {
                    let name = t[x].ident().unwrap_or_default().to_string();
                    let local = if y == x + 3 && t[x + 1].is_ident("as") {
                        t[x + 2].ident().unwrap_or_default().to_string()
                    } else {
                        name.clone()
                    };
                    found.push((local, Binding::Export(module.clone(), name)));
                }
                k = close + 1;
            } else if let Some(n) = t[k].ident() {
                found.push((n.to_string(), Binding::Module(module.clone())));
                k += 1;
            } else {
                return false;
            }
        }
        self.bindings.extend(found);
        true
    }

    /// `const x = require("m")`, `const { a, b: c } = require("m")` or
    /// `const x = require("m").a`: records its names.
    fn require(&mut self, a: usize, b: usize) -> bool {
        let t = &self.toks;
        let Some(eq) = (a..b).find(|&k| t[k].is("=")) else {
            return false;
        };
        let Some(module) = self.required(eq + 1, b.min(eq + 5)) else {
            return false;
        };
        let after = eq + 5;
        let export = if after == b {
            None
        } else if after + 2 == b && t[after].is(".") {
            t[after + 1].ident().map(str::to_string)
        } else {
            return false;
        };
        let mut found = Vec::new();
        if eq == a + 2 {
            let Some(n) = t[a + 1].ident() else {
                return false;
            };
            let binding = match export {
                Some(e) => Binding::Export(module, e),
                None => Binding::Module(module),
            };
            found.push((n.to_string(), binding));
        } else if export.is_none() && t[a + 1].is("{") && js::matching(t, a + 1) == Some(eq - 1) {
            for (x, y) in js::split_commas(t, a + 2, eq - 1) {
                let Some(name) = t[x].ident() else {
                    return false;
                };
                let local = if y == x + 3 && t[x + 1].is(":") {
                    t[x + 2].ident().unwrap_or(name)
                } else if y == x + 1 {
                    name
                } else {
                    return false;
                };
                found.push((
                    local.to_string(),
                    Binding::Export(module.clone(), name.into()),
                ));
            }
        } else {
            return false;
        }
        self.bindings.extend(found);
        true
    }

    /// The module of `require("m")`, when that is exactly `toks[a..b]`.
    fn required(&self, a: usize, b: usize) -> Option<String> {
        let t = &self.toks;
        if b != a + 4 || !t[a].is_ident("require") || !t[a + 1].is("(") || !t[a + 3].is(")") {
            return None;
        }
        match &t[a + 2].tok {
            Tok::Str(Some(m)) => Some(m.clone()),
            _ => None,
        }
    }

    /// A config function in `toks[a..b]`; ports its body.
    fn config_fn(&mut self, a: usize, b: usize) -> bool {
        let t = &self.toks;
        let mut k = a;
        if a >= b || b > t.len() {
            return false;
        }
        if t[k].is_ident("async") {
            k += 1;
        }
        let arrow = !t.get(k).is_some_and(|t| t.is_ident("function"));
        if !arrow {
            k += 1;
            if t.get(k).and_then(Token::ident).is_some() {
                k += 1;
            }
        }
        // The parameter.
        let param = if t.get(k).is_some_and(|t| t.is("(")) {
            match (t.get(k + 1), t.get(k + 2)) {
                (Some(p), Some(close)) if close.is(")") && p.ident().is_some() => {
                    k += 3;
                    p.ident().map(str::to_string)
                }
                (Some(close), _) if close.is(")") => {
                    k += 2;
                    None
                }
                _ => return false,
            }
        } else if arrow && t.get(k).and_then(Token::ident).is_some() {
            k += 1;
            t[k - 1].ident().map(str::to_string)
        } else {
            return false;
        };
        if arrow {
            if !t.get(k).is_some_and(|t| t.is("=>")) {
                return false;
            }
            k += 1;
        }
        if !t.get(k).is_some_and(|t| t.is("{")) || js::matching(t, k) != Some(b - 1) {
            return false;
        }
        let param = param.unwrap_or_default();
        let body = (k + 1, b - 1);
        // The parameter must stay the config object.
        if !param.is_empty() {
            for j in body.0..body.1 {
                if t[j].is_ident(&param)
                    && (j == 0 || !(t[j - 1].is(".") || t[j - 1].is("?.")))
                    && t.get(j + 1)
                        .is_some_and(|n| matches!(n.tok, Tok::Punct("=" | "||=" | "&&=" | "??=")))
                {
                    self.give_up(format!("`{param}` is assigned in the config function"));
                    return true;
                }
            }
        }
        let stmts = js::statements(t, body.0, body.1);
        // `return` and a line break returns `undefined`: what follows is not the config.
        for &(x, y) in &stmts {
            if t[x].is_ident("return")
                && y == x + 1
                && t.get(y).is_some_and(|n| !n.is(";"))
                && y < body.1
            {
                self.give_up("`return` is followed by a line break, so JavaScript returns `undefined` there and what follows never runs");
                return true;
            }
        }
        let mut stopped: Option<&'static str> = None;
        for (x, y) in stmts {
            if let Some(why) = stopped {
                self.todo(x, y, why, PORTING);
                continue;
            }
            let returns = self.toks[x].is_ident("return");
            let before = self.items.len();
            self.statement(&param, x, y);
            if self.whole.is_some() {
                return true;
            }
            if returns {
                stopped = Some("11ty never runs this: it comes after `return`");
                continue;
            }
            let unported = self.items[before..]
                .iter()
                .all(|i| matches!(i, Item::Todo { .. }));
            if !unported {
                continue;
            }
            // Code Brrrty doesn't port could end the function early, or change
            // any setting through the config object: then nothing is certain.
            if self.may_stop(x, y) {
                self.give_up("code in the config function can return or throw early, so Brrrty can't tell which settings 11ty ends up with");
                return true;
            }
            if !param.is_empty() && self.mentions(&param, x, y) && !self.inert(&param, x, y) {
                self.give_up(format!(
                    "code Brrrty can't port uses `{param}`, and could change any setting, so nothing here is certain"
                ));
                return true;
            }
        }
        true
    }

    /// Whether `toks[a..b]` uses the name `name`, other than as a property.
    fn mentions(&self, name: &str, a: usize, b: usize) -> bool {
        (a..b).any(|k| {
            self.toks[k].is_ident(name)
                && !(k > 0 && (self.toks[k - 1].is(".") || self.toks[k - 1].is("?.")))
        })
    }

    /// A call `p.method(...)` that can't change a folder or template engine:
    /// `p` isn't in its arguments, and it isn't a `set*Directory` call whose
    /// folder Brrrty couldn't read.
    fn inert(&self, p: &str, a: usize, b: usize) -> bool {
        let t = &self.toks;
        let start = if t[a].is_ident("await") { a + 1 } else { a };
        if b < start + 4
            || !t[start].is_ident(p)
            || !t[start + 1].is(".")
            || !t[start + 3].is("(")
            || js::matching(t, start + 3) != Some(b - 1)
        {
            return false;
        }
        let Some(method) = t[start + 2].ident() else {
            return false;
        };
        if self.mentions(p, start + 4, b - 1) {
            return false;
        }
        // A plugin's options with `immediate` run it outside plugin mode,
        // where it can change any setting. Only absent options, or a plain
        // object without that key, are certain.
        if method == "addPlugin" {
            let args = js::split_commas(t, start + 4, b - 1);
            return match args.as_slice() {
                [_] => true,
                [_, (x, y)] => js::object_props(t, *x, *y)
                    .is_some_and(|props| props.iter().all(|p| p.key != "immediate")),
                _ => false,
            };
        }
        if method.ends_with("Directory") {
            let args = js::split_commas(t, start + 4, b - 1);
            return args.len() == 1 && matches!(self.lit(args[0]), Some(Lit::Str(_)));
        }
        true
    }

    fn statement(&mut self, p: &str, a: usize, b: usize) {
        let t = &self.toks;
        if t[a].is_ident("return") {
            if b == a + 1 {
                return;
            }
            self.returned(a + 1, b);
            return;
        }
        let start = if t[a].is_ident("await") { a + 1 } else { a };
        // `p.name(...)` or `p.a.b(...)`.
        let mut names = Vec::new();
        let mut k = start + 1;
        if !p.is_empty() && t[start].is_ident(p) {
            while k + 1 < b && t[k].is(".") && t[k + 1].ident().is_some() {
                names.push(t[k + 1].ident().unwrap_or_default().to_string());
                k += 2;
            }
        }
        if names.is_empty() || k >= b || !t[k].is("(") || js::matching(t, k) != Some(b - 1) {
            self.todo(a, b, "port this code", PORTING);
            return;
        }
        let args = js::split_commas(t, k + 1, b - 1);
        let method = names.join(".");
        self.call(&method, &args, a, b);
    }

    fn lit(&self, (a, b): (usize, usize)) -> Option<Lit> {
        js::literal(&self.toks, a, b)
    }

    fn call(&mut self, method: &str, args: &[(usize, usize)], a: usize, b: usize) {
        let filters = format!("{DOCS}#filters");
        match method {
            "addPassthroughCopy" => {
                let lit = if args.len() == 1 { self.lit(args[0]) } else { None };
                match lit {
                    Some(Lit::Str(path)) if !glob(&path) => self.items.push(Item::Rhai(format!(
                        "config.add_passthrough_copy({});",
                        js::rhai_string(&path)
                    ))),
                    Some(Lit::Object(pairs))
                        if !pairs.is_empty()
                            && pairs
                                .iter()
                                .all(|(k, v)| !glob(k) && matches!(v, Lit::Str(_))) =>
                    {
                        let lines: Vec<String> = pairs
                            .iter()
                            .map(|(from, to)| {
                                format!(
                                    "config.add_passthrough_copy(#{{ {}: {} }});",
                                    js::rhai_string(from),
                                    js::rhai(to, 0)
                                )
                            })
                            .collect();
                        self.items.push(Item::Rhai(lines.join("\n")));
                    }
                    Some(Lit::Str(_)) | Some(Lit::Object(_)) => self.todo(
                        a,
                        b,
                        "Brrrty copies files and folders, not globs; list what this copies",
                        format!("{DOCS}#the-config-object"),
                    ),
                    _ => self.todo(
                        a,
                        b,
                        "port this passthrough copy",
                        format!("{DOCS}#the-config-object"),
                    ),
                }
            }
            "ignores.add" | "ignores.delete" => {
                match (args.len(), args.first().and_then(|&r| self.lit(r))) {
                    (1, Some(Lit::Str(glob))) => {
                        let m = method.rsplit('.').next().unwrap_or("add");
                        self.items.push(Item::Rhai(format!(
                            "config.ignores.{m}({});",
                            js::rhai_string(&glob)
                        )))
                    }
                    _ => self.todo(a, b, "port this ignore", format!("{DOCS}#the-config-object")),
                }
            }
            "addPlugin" => self.plugin(args, a, b),
            "setInputDirectory" | "setOutputDirectory" | "setIncludesDirectory"
            | "setDataDirectory" | "setLayoutsDirectory" => {
                let key = match method {
                    "setInputDirectory" => "input",
                    "setOutputDirectory" => "output",
                    "setIncludesDirectory" => "includes",
                    "setDataDirectory" => "data",
                    _ => "layouts",
                };
                match (args.len(), args.first().and_then(|&r| self.lit(r))) {
                    (1, Some(Lit::Str(v))) => self.set_dir(key, v, a, b),
                    _ => self.todo(a, b, "port this folder", format!("{DOCS}#what-the-script-returns")),
                }
            }
            "addFilter" | "addAsyncFilter" | "addLiquidFilter" | "addNunjucksFilter"
            | "addNunjucksAsyncFilter" | "addJavaScriptFunction" => self.todo(
                a,
                b,
                "port this filter to Rhai with config.add_filter",
                filters,
            ),
            "addCollection" => self.todo(
                a,
                b,
                "port this collection to Rhai with config.add_collection",
                format!("{DOCS}#collections"),
            ),
            "addGlobalData" => self.todo(
                a,
                b,
                "port this to config.add_global_data",
                format!("{DOCS}#the-config-object"),
            ),
            "setTemplateFormats" | "addTemplateFormats" => self.todo(
                a,
                b,
                "Brrrty builds 11ty's default template formats (Liquid, Markdown, Nunjucks and HTML) and doesn't read this setting yet",
                format!("{DOCS}#what-the-script-returns"),
            ),
            "setQuietMode" | "setWatchThrottleWaitTime" | "setServerOptions" | "addWatchTarget"
            | "setWatchJavaScriptDependencies" | "setChokidarConfig"
            | "setServerPassthroughCopyBehavior" => self.todo(
                a,
                b,
                "this is for 11ty's watcher and dev server and doesn't change the files 11ty writes; Brrrty's `serve` has no setting for it, so drop it",
                "https://brrrty.com/docs/serve/",
            ),
            _ => self.todo(a, b, "port this to Rhai", PORTING),
        }
    }

    /// What the plugin `toks[a..b]` is: its module and export.
    fn plugin_ref(&self, (a, b): (usize, usize)) -> Option<Binding> {
        let t = &self.toks;
        // A name counts only when nothing else in the file declares or assigns it.
        let trusted = |name: &str| {
            self.uses
                .get(name)
                .is_some_and(|&(all, plugin)| all == plugin + 1)
        };
        if b == a + 1 {
            let name = t[a].ident()?;
            if !trusted(name) {
                return None;
            }
            return self.bindings.get(name).cloned();
        }
        if let Some(m) = self.required(a, b) {
            return Some(Binding::Module(m));
        }
        if b >= 3 && t[b - 2].is(".") {
            let export = t[b - 1].ident()?.to_string();
            let module = if b == a + 3 {
                let name = t[a].ident()?;
                if !trusted(name) {
                    return None;
                }
                match self.bindings.get(name)? {
                    Binding::Module(m) => m.clone(),
                    Binding::Export(..) => return None,
                }
            } else {
                self.required(a, b - 2)?
            };
            return Some(Binding::Export(module, export));
        }
        None
    }

    fn plugin(&mut self, args: &[(usize, usize)], a: usize, b: usize) {
        let link = format!("{DOCS}#plugins");
        let Some(&first) = args.first() else {
            return self.todo(a, b, "port this plugin", link);
        };
        let options = match args.get(1) {
            None => Some(Vec::new()),
            Some(&r) => match self.lit(r) {
                Some(Lit::Object(pairs)) => Some(pairs),
                _ => None,
            },
        };
        let (name, known): (&str, &[(&str, &str)]) = match self.plugin_ref(first) {
            Some(Binding::Module(m)) if m == SYNTAX_HIGHLIGHT => (
                "syntaxhighlight",
                &[
                    ("lineSeparator", "line_separator"),
                    ("errorOnInvalidLanguage", "error_on_invalid_language"),
                    ("alwaysWrapLineHighlights", "always_wrap_line_highlights"),
                    ("preAttributes", "pre_attributes"),
                    ("codeAttributes", "code_attributes"),
                    ("trim", "trim"),
                    ("languages", "languages"),
                    ("templateFormats", "template_formats"),
                ],
            ),
            Some(Binding::Export(m, e)) if m == RSS && e == "feedPlugin" => (
                "rss",
                &[
                    ("type", "type"),
                    ("outputPath", "output_path"),
                    ("inputPath", "input_path"),
                    ("collection", "collection"),
                    ("metadata", "metadata"),
                    ("templateData", "template_data"),
                    ("stylesheet", "stylesheet"),
                    ("script", "script"),
                ],
            ),
            Some(Binding::Module(m)) if m == RSS => {
                return self.todo(
                    a,
                    b,
                    "Brrrty adds the RSS plugin's filters along with a feed, config.add_plugin(\"rss\", ...), and has no way to add them alone yet",
                    link,
                );
            }
            _ => {
                return self.todo(
                    a,
                    b,
                    "Brrrty has its own versions of the syntax highlighting and RSS feed plugins only; port what this plugin does, if a page needs it",
                    link,
                );
            }
        };
        let Some(options) = options else {
            return self.todo(
                a,
                b,
                format!("port this plugin's options to config.add_plugin(\"{name}\", #{{ ... }})"),
                link,
            );
        };
        if name == "rss" && options.is_empty() {
            return self.todo(a, b, "give this feed its options", link);
        }
        let mut converted = Vec::new();
        for (k, v) in options {
            let Some((_, rhai)) = known.iter().find(|(js, _)| *js == k) else {
                return self.todo(
                    a,
                    b,
                    format!("Brrrty's {name} plugin doesn't take the option `{k}`"),
                    link,
                );
            };
            // Attributes and template data keep their order in 11ty.
            if matches!(
                k.as_str(),
                "preAttributes" | "codeAttributes" | "templateData"
            ) && !js::order_safe(&v)
            {
                return self.todo(
                    a,
                    b,
                    format!("`{k}` has keys whose order would change in a #{{ }} map; build it with object()"),
                    format!("{PORTING}#object-and-key-order"),
                );
            }
            converted.push((rhai.to_string(), v));
        }
        let line = if converted.is_empty() {
            format!("config.add_plugin(\"{name}\");")
        } else {
            format!(
                "config.add_plugin(\"{name}\", {});",
                js::rhai(&Lit::Object(converted), 0)
            )
        };
        self.items.push(Item::Rhai(line));
    }

    fn set_dir(&mut self, key: &'static str, value: String, a: usize, b: usize) {
        let link = format!("{DOCS}#what-the-script-returns");
        let default = match key {
            "includes" | "layouts" => Some("_includes"),
            "data" => Some("_data"),
            _ => None,
        };
        if let Some(d) = default
            && value != d
        {
            return self.todo(
                a,
                b,
                format!("Brrrty reads only the default `{d}` folder for `dir.{key}` so far; move the files there"),
                link,
            );
        }
        let span = self.span(a, b);
        let key = match key {
            "input" => "dir.input",
            "output" => "dir.output",
            "includes" => "dir.includes",
            "data" => "dir.data",
            _ => "dir.layouts",
        };
        self.settings.push(Setting {
            key,
            value: js::rhai_string(&value),
            raw: Some(value),
            span,
        });
    }

    /// The object the config returns, in `toks[a..b]`.
    fn returned(&mut self, a: usize, b: usize) {
        let link = format!("{DOCS}#what-the-script-returns");
        let Some(props) = js::object_props(&self.toks, a, b) else {
            return self.give_up(
                "Brrrty can't read what the config returns, so it can't tell which settings 11ty uses",
            );
        };
        for p in props {
            let (x, y) = p.range;
            match p.key.as_str() {
                "dir" => {
                    let Some(dirs) = js::object_props(&self.toks, p.value.0, p.value.1) else {
                        self.todo(x, y, "port `dir`", link.clone());
                        continue;
                    };
                    for d in dirs {
                        let key = match d.key.as_str() {
                            "input" => "input",
                            "output" => "output",
                            "includes" => "includes",
                            "data" => "data",
                            "layouts" => "layouts",
                            _ => {
                                self.todo(d.range.0, d.range.1, "11ty doesn't read this folder", link.clone());
                                continue;
                            }
                        };
                        match self.lit(d.value) {
                            Some(Lit::Str(v)) => self.set_dir(key, v, d.range.0, d.range.1),
                            _ => self.todo(d.range.0, d.range.1, "port this folder", link.clone()),
                        }
                    }
                }
                "markdownTemplateEngine" | "htmlTemplateEngine" => {
                    let key = if p.key == "htmlTemplateEngine" {
                        "htmlTemplateEngine"
                    } else {
                        "markdownTemplateEngine"
                    };
                    match self.lit(p.value) {
                        Some(Lit::Str(e)) if e == "liquid" || e == "njk" => {
                            let span = self.span(x, y);
                            self.settings.push(Setting {
                                key,
                                value: js::rhai_string(&e),
                                raw: None,
                                span,
                            })
                        }
                        Some(Lit::Bool(false)) => {
                            let span = self.span(x, y);
                            self.settings.push(Setting {
                                key,
                                value: "false".into(),
                                raw: None,
                                span,
                            })
                        }
                        _ => self.todo(
                            x,
                            y,
                            "Brrrty runs Liquid or Nunjucks here, or nothing (false)",
                            link.clone(),
                        ),
                    }
                }
                "dataTemplateEngine" => self.left_data_engine = true,
                "templateFormats" => self.todo(
                    x,
                    y,
                    "Brrrty builds 11ty's default template formats (Liquid, Markdown, Nunjucks and HTML) and doesn't read this setting yet",
                    link.clone(),
                ),
                _ => self.todo(x, y, "Brrrty doesn't support this setting yet", link.clone()),
            }
        }
    }

    /// Quote the lines `span` covers, whole, each as its own `//` line.
    fn quoted(&self, out: &mut String, span: (usize, usize)) {
        let line_of = |at: usize| self.lines.partition_point(|&(_, end)| end < at);
        let first = line_of(span.0);
        let last = line_of(span.1).max(first).min(self.lines.len() - 1);
        if first == last {
            out.push_str(&format!("// {}, line {}:\n", self.file, first + 1));
        } else {
            out.push_str(&format!(
                "// {}, lines {}-{}:\n",
                self.file,
                first + 1,
                last + 1
            ));
        }
        for &(a, b) in &self.lines[first..=last] {
            let l = &self.src[a..b];
            if l.trim().is_empty() {
                out.push_str("//\n");
            } else {
                out.push_str(&format!("//     {}\n", one_line(l)));
            }
        }
    }

    fn finish(mut self) -> Port {
        let mut out = format!(
            "// Written by `brrrty init` from {file}, which Brrrty doesn't run.\n\
             // Brrrty reads this file instead; 11ty still reads {file}.\n\
             // Reference: {DOCS}\n",
            file = self.file
        );
        if let Some(why) = self.whole.take() {
            out.push_str(&format!(
                "\n// TODO: port all of this file by hand: {}.\n// See {DOCS}\n",
                one_line(&why)
            ));
            if !self.src.is_empty() {
                self.quoted(&mut out, (0, self.src.len()));
            }
            return Port {
                text: out,
                todos: 1,
                input: None,
                output: None,
            };
        }
        // A setting made more than once: 11ty's precedence decides, so neither is ported.
        let mut settings: Vec<&Setting> = Vec::new();
        let mut todos_after: Vec<(String, (usize, usize))> = Vec::new();
        for st in &self.settings {
            if self.settings.iter().filter(|o| o.key == st.key).count() > 1 {
                todos_after.push((
                    format!("`{}` is set more than once; which one 11ty uses depends on where each is, so set it by hand", st.key),
                    st.span,
                ));
            } else {
                settings.push(st);
            }
        }
        let link = format!("{DOCS}#what-the-script-returns");
        let mut todos = 0;
        let mut last_single = false;
        let mut body = String::new();
        let todo_items = todos_after.into_iter().map(|(why, span)| Item::Todo {
            why,
            link: link.clone(),
            span,
        });
        let items: Vec<Item> = std::mem::take(&mut self.items)
            .into_iter()
            .chain(todo_items)
            .collect();
        for item in &items {
            match item {
                Item::Rhai(line) => {
                    let single =
                        !line.contains('\n') || line.lines().all(|l| l.starts_with("config."));
                    if !(single && last_single) {
                        body.push('\n');
                    }
                    body.push_str(line);
                    body.push('\n');
                    last_single = single;
                }
                Item::Todo { why, link, span } => {
                    todos += 1;
                    body.push_str(&format!("\n// TODO: {}.\n// See {link}\n", one_line(why)));
                    self.quoted(&mut body, *span);
                    last_single = false;
                }
            }
        }
        out.push_str(&body);
        if self.left_data_engine {
            out.push_str("\n// Left out: `dataTemplateEngine`, which 11ty 3 doesn't read.\n");
        }
        let dirs: Vec<&&Setting> = settings
            .iter()
            .filter(|s| s.key.starts_with("dir."))
            .collect();
        let engines: Vec<&&Setting> = settings
            .iter()
            .filter(|s| !s.key.starts_with("dir."))
            .collect();
        if !settings.is_empty() {
            out.push_str("\n#{\n");
            if !dirs.is_empty() {
                out.push_str("    dir: #{\n");
                for st in &dirs {
                    out.push_str(&format!("        {}: {},\n", &st.key[4..], st.value));
                }
                out.push_str("    },\n");
            }
            for st in &engines {
                out.push_str(&format!("    {}: {},\n", st.key, st.value));
            }
            out.push_str("}\n");
        }
        let dir = |key: &str| {
            settings
                .iter()
                .find(|s| s.key == key)
                .and_then(|s| s.raw.clone())
        };
        Port {
            text: out,
            todos,
            input: dir("dir.input"),
            output: dir("dir.output"),
        }
    }
}

/// A path 11ty would read as a glob.
fn glob(path: &str) -> bool {
    path.contains(['*', '?', '[', ']', '{', '}', '(', ')']) || path.starts_with('!')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_cjs_config() {
        let src = r#"const syntaxHighlight = require("@11ty/eleventy-plugin-syntaxhighlight");
const { feedPlugin } = require("@11ty/eleventy-plugin-rss");

module.exports = function (eleventyConfig) {
  eleventyConfig.addPassthroughCopy("src/images");
  eleventyConfig.addPassthroughCopy({ "static": "/", "src/a.svg": "a.svg" });
  eleventyConfig.addPassthroughCopy("src/**/*.png");
  eleventyConfig.addPlugin(syntaxHighlight);
  eleventyConfig.addPlugin(feedPlugin, {
    type: "atom",
    outputPath: "/feed.xml",
    collection: { name: "post", limit: 10 },
    metadata: { title: "T", base: "https://example.com/" },
  });
  eleventyConfig.addFilter("shout", (s) => s.toUpperCase() + "!");
  eleventyConfig.setQuietMode(true);
  return {
    dir: { input: "src", output: "dist", includes: "_partials" },
    markdownTemplateEngine: "njk",
    dataTemplateEngine: "njk",
    templateFormats: ["md", "njk"],
  };
};
"#;
        let p = port(src, "eleventy.config.js");
        let t = &p.text;
        assert!(t.contains("config.add_passthrough_copy(\"src/images\");\nconfig.add_passthrough_copy(#{ \"static\": \"/\" });\nconfig.add_passthrough_copy(#{ \"src/a.svg\": \"a.svg\" });\n"), "{t}");
        assert!(t.contains("config.add_plugin(\"syntaxhighlight\");"), "{t}");
        assert!(t.contains("config.add_plugin(\"rss\", #{\n    type: \"atom\",\n    output_path: \"/feed.xml\",\n"), "{t}");
        assert!(t.contains("// TODO: port this filter to Rhai with config.add_filter.\n// See https://brrrty.com/docs/configuration/#filters\n// eleventy.config.js, line 15:\n//       eleventyConfig.addFilter(\"shout\", (s) => s.toUpperCase() + \"!\");\n"), "{t}");
        assert!(t.contains("globs"), "{t}");
        assert!(t.contains("`_includes`"), "{t}");
        assert!(t.contains("// TODO: this is for 11ty's watcher"), "{t}");
        assert!(t.contains("#{\n    dir: #{\n        input: \"src\",\n        output: \"dist\",\n    },\n    markdownTemplateEngine: \"njk\",\n}\n"), "{t}");
        assert_eq!(p.todos, 5, "{t}");
        assert_eq!(p.input.as_deref(), Some("src"));
    }

    #[test]
    fn ports_esm_config() {
        let src = "import syntaxHighlight from \"@11ty/eleventy-plugin-syntaxhighlight\";\nimport pluginRss from \"@11ty/eleventy-plugin-rss\";\nimport { EleventyHtmlBasePlugin } from \"@11ty/eleventy\";\n\nexport default async function(eleventyConfig) {\n\televentyConfig.setInputDirectory(\"content\")\n\televentyConfig.addPlugin(syntaxHighlight, { preAttributes: { tabindex: 0 } })\n\televentyConfig.addPlugin(pluginRss)\n\televentyConfig.addPlugin(EleventyHtmlBasePlugin)\n}\n\nexport const config = {\n\thtmlTemplateEngine: false,\n};\n";
        let p = port(src, "eleventy.config.mjs");
        let t = &p.text;
        assert!(
            t.contains(
                "config.add_plugin(\"syntaxhighlight\", #{ pre_attributes: #{ tabindex: 0 } });"
            ),
            "{t}"
        );
        assert!(t.contains("RSS plugin's filters"), "{t}");
        assert!(t.contains("eleventy.config.mjs, line 9:"), "{t}");
        assert!(
            t.contains(
                "    dir: #{\n        input: \"content\",\n    },\n    htmlTemplateEngine: false,\n"
            ),
            "{t}"
        );
        assert_eq!(p.todos, 2, "{t}");
    }

    #[test]
    fn named_function_and_helpers() {
        let src = "const slug = (s) => s.toLowerCase();\nfunction config(c) {\n  c.addPassthroughCopy('a');\n}\nmodule.exports = config;\n";
        let t = port(src, ".eleventy.js").text;
        assert!(t.contains("config.add_passthrough_copy(\"a\");"), "{t}");
        assert!(t.contains("// TODO: port this code, or drop it"), "{t}");
        assert!(!t.contains("module.exports"), "{t}");
    }

    #[test]
    fn unreadable_file_is_one_todo() {
        let p = port(
            "module.exports = function (c) { c.x('unclosed); };\n",
            "eleventy.config.js",
        );
        assert_eq!(p.todos, 1);
        assert!(p.text.contains("couldn't read this file"), "{}", p.text);
    }

    /// Lines that aren't comments: what the generated config runs.
    fn live(p: &Port) -> Vec<&str> {
        p.text
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with("//"))
            .collect()
    }

    #[test]
    fn return_then_line_break_ports_nothing() {
        let src = "module.exports = function (eleventyConfig) {\n  eleventyConfig.addPassthroughCopy(\"a\");\n  return\n  {\n    dir: { output: \"elsewhere\" }\n  }\n};\n";
        let p = port(src, "eleventy.config.js");
        assert!(live(&p).is_empty(), "{}", p.text);
        assert!(
            p.text.contains("`return` is followed by a line break"),
            "{}",
            p.text
        );
        assert_eq!(p.output, None);
    }

    #[test]
    fn code_after_return_is_a_todo() {
        let src = "module.exports = function (c) {\n  return { dir: { output: \"out\" } };\n  c.addPassthroughCopy(\"secret\");\n};\n";
        let p = port(src, "eleventy.config.js");
        assert!(
            !p.text.contains("config.add_passthrough_copy"),
            "{}",
            p.text
        );
        assert!(p.text.contains("11ty never runs this"), "{}", p.text);
        // An early return inside an `if`: what follows may not run.
        let src = "module.exports = function (c) {\n  if (process.env.X) { return {}; }\n  c.addPassthroughCopy(\"a\");\n  c.addFilter(\"f\", function (s) { return s; });\n};\n";
        let p = port(src, "eleventy.config.js");
        assert!(
            !p.text.contains("config.add_passthrough_copy"),
            "{}",
            p.text
        );
        assert!(p.text.contains("can return or throw early"), "{}", p.text);
        assert!(live(&p).is_empty(), "{}", p.text);
        // A `return` inside a filter's own function doesn't stop anything.
        let src = "module.exports = function (c) {\n  c.addFilter(\"f\", function (s) { return s; });\n  c.addFilter(\"g\", (s) => { return s; });\n  c.addPassthroughCopy(\"a\");\n};\n";
        let p = port(src, "eleventy.config.js");
        assert!(
            p.text.contains("config.add_passthrough_copy(\"a\");"),
            "{}",
            p.text
        );
    }

    #[test]
    fn more_than_one_export_ports_nothing() {
        for src in [
            "module.exports = function (c) { c.addPassthroughCopy(\"old\") };\nmodule.exports = function (c) { c.addPassthroughCopy(\"new\") };\n",
            "module.exports.config = { dir: { output: \"x\" } };\nmodule.exports = function (c) { c.addPassthroughCopy(\"a\") };\n",
            "function config(c) { c.addPassthroughCopy(\"a\") }\nfunction config(c) { c.addPassthroughCopy(\"b\") }\nmodule.exports = config;\n",
            "module.exports = config;\nfunction config(c) {}\nasync",
            "export default function (c) { c.addPassthroughCopy(\"a\") }\nexport const config = { dir: { output: \"x\" } };\nexport const config2 = {};\n",
            "module.exports = function (c) { c = other; c.addPassthroughCopy(\"a\") };\n",
        ] {
            let p = port(src, "eleventy.config.js");
            assert!(live(&p).is_empty(), "{src}\n{}", p.text);
            assert_eq!(p.todos, 1, "{src}\n{}", p.text);
        }
    }

    #[test]
    fn shadowed_plugin_names_are_todos() {
        let inner = "const syntaxHighlight = require(\"@11ty/eleventy-plugin-syntaxhighlight\");\nmodule.exports = function (c) {\n  const syntaxHighlight = require(\"./my-highlighter\");\n  c.addPlugin(syntaxHighlight);\n};\n";
        let reassigned = "let hl = require(\"@11ty/eleventy-plugin-syntaxhighlight\");\nhl = require(\"./mine\");\nmodule.exports = function (c) {\n  c.addPlugin(hl);\n};\n";
        for src in [inner, reassigned] {
            let p = port(src, "eleventy.config.js");
            assert!(!p.text.contains("config.add_plugin"), "{}", p.text);
        }
    }

    #[test]
    fn settings_made_twice_are_todos() {
        let src = "export default function (c) {\n  c.setOutputDirectory(\"a\");\n}\nexport const config = {\n  dir: { output: \"b\" },\n  markdownTemplateEngine: \"njk\",\n};\n";
        let p = port(src, "eleventy.config.mjs");
        assert!(!live(&p).iter().any(|l| l.contains("output")), "{}", p.text);
        assert_eq!(p.todos, 2, "{}", p.text);
        assert!(
            p.text.contains("markdownTemplateEngine: \"njk\""),
            "{}",
            p.text
        );
    }

    #[test]
    fn every_line_terminator_stays_inside_comments() {
        for nl in ["\r", "\r\n", "\u{2028}", "\u{2029}", "\n"] {
            let src = format!(
                "module.exports = function (c) {{{nl}  c.addFilter(\"x\", (s) => s);{nl}  c.addPassthroughCopy(\"a\");{nl}}};{nl}"
            );
            let p = port(&src, "eleventy.config.js");
            for l in p.text.split('\n') {
                assert!(
                    !l.contains(['\r', '\u{2028}', '\u{2029}', '\u{85}']),
                    "{nl:?}: {l:?}"
                );
            }
            assert_eq!(
                live(&p),
                ["config.add_passthrough_copy(\"a\");"],
                "{nl:?}\n{}",
                p.text
            );
        }
        // A key with a line break in it, quoted in the reason.
        let src = "const h = require(\"@11ty/eleventy-plugin-syntaxhighlight\");\nmodule.exports = function (c) { c.addPlugin(h, { \"a\\nconfig.add_passthrough_copy('x');\": 1 }) };\n";
        let p = port(src, "eleventy.config.js");
        assert!(live(&p).is_empty(), "{}", p.text);
    }

    #[test]
    fn hostile_input_is_bounded() {
        let deep = |open: &str, n: usize| {
            format!(
                "module.exports = function (c) {{ c.addPassthroughCopy({}) }};",
                open.repeat(n)
            )
        };
        let started = std::time::Instant::now();
        for src in [
            deep("[", 100_000),
            deep("{a:", 20_000),
            format!(
                "module.exports = function (c) {{ c.x(`{}`) }};",
                "${`".repeat(10_000)
            ),
            "c.a();\n".repeat(20_000),
        ] {
            let p = port(&src, "eleventy.config.js");
            assert!(p.todos >= 1);
        }
        assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
    }

    #[test]
    fn fuzz_never_panics() {
        let sample = "const { feedPlugin } = require(\"@11ty/eleventy-plugin-rss\");\n// c\nmodule.exports = async function (c) {\n  c.addPassthroughCopy({ \"a\": \"b\" });\n  c.addPlugin(feedPlugin, { type: \"atom\", collection: { name: \"p\", limit: -1.5 } });\n  c.addFilter(\"f\", (s) => `${s}/x/` + /re[/]/g.source);\n  return { dir: { input: \"src\" }, htmlTemplateEngine: false };\n};\n";
        let started = std::time::Instant::now();
        // Every prefix of a real config.
        for (i, _) in sample.char_indices() {
            let _ = port(&sample[..i], "eleventy.config.js");
        }
        // Random soup of JavaScript pieces.
        let pieces = [
            "(",
            ")",
            "{",
            "}",
            "[",
            "]",
            ",",
            ";",
            "\n",
            "\r",
            "\u{2028}",
            "return",
            "c",
            ".",
            "addPlugin",
            "addPassthroughCopy",
            "module.exports",
            "=",
            "=>",
            "function",
            "\"s\"",
            "'",
            "`",
            "${",
            "/",
            "/*",
            "*/",
            "//",
            "-",
            "1",
            "async",
            "export",
            "default",
            "import",
            "from",
            "require",
            "const",
            "x",
            ":",
            "\\",
            "é",
            "\u{feff}",
        ];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..3000 {
            let mut src = String::new();
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let len = (seed % 60) as usize;
            for _ in 0..len {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                src.push_str(pieces[(seed % pieces.len() as u64) as usize]);
                src.push(' ');
            }
            let p = port(&src, "eleventy.config.js");
            for l in p.text.split('\n') {
                assert!(!l.contains(['\r', '\u{2028}', '\u{2029}']), "{src:?}");
            }
        }
        // The sample with a piece cut out or repeated, which often still tokenizes.
        let chars: Vec<char> = sample.chars().collect();
        for _ in 0..3000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let at = (seed % chars.len() as u64) as usize;
            let len = 1 + ((seed >> 20) % 12) as usize;
            let end = (at + len).min(chars.len());
            let mut m: Vec<char> = chars[..at].to_vec();
            if seed & 1 == 0 {
                m.extend_from_slice(&chars[at..end]);
                m.extend_from_slice(&chars[at..end]);
            }
            m.extend_from_slice(&chars[end..]);
            let src: String = m.into_iter().collect();
            let p = port(&src, "eleventy.config.js");
            for l in p.text.split('\n') {
                assert!(!l.contains(['\r', '\u{2028}', '\u{2029}']), "{src:?}");
            }
        }
        assert!(started.elapsed().as_secs() < 20, "{:?}", started.elapsed());
    }

    #[test]
    fn unported_code_using_the_config_ports_nothing() {
        for src in [
            // 11ty writes `dist` in production.
            "module.exports = function (eleventyConfig) {\n  eleventyConfig.setOutputDirectory(\"_site\");\n  if (process.env.NODE_ENV === \"production\") eleventyConfig.setOutputDirectory(\"dist\");\n};\n",
            // A helper that gets the config object.
            "const setup = require(\"./setup\");\nmodule.exports = function (eleventyConfig) {\n  eleventyConfig.addPassthroughCopy(\"a\");\n  setup(eleventyConfig);\n  return { dir: { input: \"src\" }, markdownTemplateEngine: \"njk\" };\n};\n",
            "module.exports = function (c) {\n  c.setInputDirectory(\"src\");\n  [0].forEach(() => c.setOutputDirectory(\"evil\"));\n};\n",
            "module.exports = function (c) {\n  const cfg = c;\n  cfg.setOutputDirectory(\"x\");\n  return { dir: { input: \"src\" } };\n};\n",
            "module.exports = function (c) {\n  c.setOutputDirectory(process.env.OUT);\n  c.addPassthroughCopy(\"a\");\n};\n",
            "module.exports = function (c) {\n  c.addPlugin(plugin, { config: c });\n  return { dir: { input: \"src\" } };\n};\n",
            "module.exports = function (c) {\n  return { ...base, dir: { input: \"src\" } };\n};\n",
        ] {
            let p = port(src, "eleventy.config.js");
            assert!(live(&p).is_empty(), "{src}\n{}", p.text);
            assert_eq!(p.todos, 1, "{src}\n{}", p.text);
        }
        // Calls on the config that can't change a setting still allow a port.
        let src = "module.exports = function (c) {\n  c.addFilter(\"f\", (s) => s);\n  c.addPlugin(somePlugin);\n  c.setIncludesDirectory(\"_partials\");\n  c.addPassthroughCopy(\"a\");\n  return { dir: { input: \"src\" } };\n};\n";
        let p = port(src, "eleventy.config.js");
        assert!(
            p.text.contains("config.add_passthrough_copy(\"a\");"),
            "{}",
            p.text
        );
        assert!(p.text.contains("input: \"src\""), "{}", p.text);
    }

    #[test]
    fn a_slash_that_could_be_either_is_never_guessed() {
        for src in [
            // Real 11ty writes `./evil`: `/'/` is a regular expression.
            "module.exports = function (c) {\n  c.setOutputDirectory(\"good\");\n  if (process.env.X) /'/.test(\"\"); c.setOutputDirectory(\"evil\"); // '\n};\n",
            "module.exports = function (c) {\n  while (x) /\"/.test(s);\n  c.addPassthroughCopy(\"a\");\n};\n",
            "module.exports = function (c) {\n  if (x) {}\n  /'/.test(\"\"); c.addPassthroughCopy(\"b\"); // '\n};\n",
            "module.exports = function (c) {\n  i++ /'/; c.addPassthroughCopy(\"b\"); // '\n};\n",
            "module.exports = function (c) {\n  c.addFilter(\"f\", (s) => `${s.replace(/}/g, \"\")}`);\n  c.addPassthroughCopy(\"a\");\n};\n",
            "module.exports = function (c) {\n  c.addFilter(\"f\", (s) => `${a / b}`);\n};\n",
        ] {
            let p = port(src, "eleventy.config.js");
            assert!(live(&p).is_empty(), "{src}\n{}", p.text);
            assert!(
                p.text.contains("port all of this file"),
                "{src}\n{}",
                p.text
            );
        }
        // Clear cases still read: division after a call, a regex after `(` or `,`.
        let src = "module.exports = function (c) {\n  c.addFilter(\"f\", (s) => s.split(/,/).length / 2);\n  c.addFilter(\"g\", (s) => `${s}/x`);\n  c.addPassthroughCopy(\"a\");\n};\n";
        let p = port(src, "eleventy.config.js");
        assert!(
            p.text.contains("config.add_passthrough_copy(\"a\");"),
            "{}",
            p.text
        );
    }

    #[test]
    fn plugins_run_immediately_port_nothing() {
        for src in [
            // Real 11ty writes `./pluginout/`.
            "const p = require(\"./p\");\nmodule.exports = function (c) {\n  c.addPlugin(p, { immediate: true });\n  c.addPassthroughCopy(\"a\");\n};\n",
            "const p = require(\"./p\");\nmodule.exports = function (c) {\n  c.addPlugin(p, { ...opts });\n  return { dir: { input: \"src\" } };\n};\n",
            "const p = require(\"./p\");\nmodule.exports = function (c) {\n  c.addPlugin(p, opts);\n  return { dir: { input: \"src\" } };\n};\n",
            "const p = require(\"./p\");\nmodule.exports = function (c) {\n  c.addPlugin(p, { [k]: true });\n};\n",
            "const p = require(\"./p\");\nmodule.exports = function (c) {\n  c.addPlugin(p, makeOptions());\n};\n",
            "const h = require(\"@11ty/eleventy-plugin-syntaxhighlight\");\nmodule.exports = function (c) {\n  c.addPlugin(h, { \"immediate\": true });\n  return { dir: { input: \"src\" } };\n};\n",
        ] {
            let p = port(src, "eleventy.config.js");
            assert!(live(&p).is_empty(), "{src}\n{}", p.text);
            assert_eq!(p.todos, 1, "{src}\n{}", p.text);
        }
        // Plain options without `immediate` still allow a port.
        let src = "const p = require(\"./p\");\nmodule.exports = function (c) {\n  c.addPlugin(p, { size: 2, fn: (x) => x });\n  c.addPassthroughCopy(\"a\");\n};\n";
        let p = port(src, "eleventy.config.js");
        assert!(
            p.text.contains("config.add_passthrough_copy(\"a\");"),
            "{}",
            p.text
        );
    }
}
