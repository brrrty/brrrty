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
    Left {
        why: String,
        span: (usize, usize),
    },
}

struct Reader<'a> {
    src: &'a str,
    file: &'a str,
    toks: Vec<Token>,
    items: Vec<Item>,
    bindings: HashMap<String, Binding>,
    dir: Vec<(&'static str, String)>,
    engines: Vec<(&'static str, String)>,
    left_data_engine: bool,
}

/// Port the 11ty config file `file`, whose text is `src`.
pub(crate) fn port(src: &str, file: &str) -> Port {
    let toks = match js::tokenize(src) {
        Ok(t) => t,
        Err(why) => {
            let mut r = Reader::new(src, file, Vec::new());
            r.items.push(Item::Todo {
                why: format!("Brrrty couldn't read this file ({why}), so port all of it"),
                link: PORTING.into(),
                span: (0, src.len()),
            });
            return r.finish();
        }
    };
    let mut r = Reader::new(src, file, toks);
    r.read();
    r.finish()
}

impl<'a> Reader<'a> {
    fn new(src: &'a str, file: &'a str, toks: Vec<Token>) -> Self {
        Reader {
            src,
            file,
            toks,
            items: Vec::new(),
            bindings: HashMap::new(),
            dir: Vec::new(),
            engines: Vec::new(),
            left_data_engine: false,
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

    fn read(&mut self) {
        let n = self.toks.len();
        let stmts = js::statements(&self.toks, 0, n);
        // The config function, when the export names a function declared elsewhere.
        let mut config_name: Option<String> = None;
        for &(a, b) in &stmts {
            if let Some(rest) = self.export_start(a, b)
                && rest + 1 == b
                && let Some(name) = self.toks[rest].ident()
            {
                config_name = Some(name.to_string());
            }
        }
        let mut found = false;
        for (a, b) in stmts {
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
            if let Some(rest) = self.export_start(a, b) {
                if config_name.is_some() && rest + 1 == b {
                    continue;
                }
                if self.config_fn(rest, b) {
                    found = true;
                } else {
                    self.todo(
                        a,
                        b,
                        "Brrrty couldn't read this config function, so port it by hand",
                        DOCS,
                    );
                }
                continue;
            }
            if let Some(rest) = self.config_export(a, b) {
                self.returned(rest, b);
                found = true;
                continue;
            }
            if let Some(name) = &config_name {
                let decl = if t.is_ident("async") { a + 1 } else { a };
                if self.toks[decl].is_ident("function")
                    && self.toks.get(decl + 1).is_some_and(|t| t.is_ident(name))
                    && self.config_fn(a, b)
                {
                    found = true;
                    continue;
                }
            }
            self.todo(
                a,
                b,
                "port this code, or drop it if nothing below needs it",
                PORTING,
            );
        }
        if !found && !self.toks.is_empty() {
            self.items.push(Item::Todo {
                why: "Brrrty found no config function exported here; check nothing is missing"
                    .into(),
                link: DOCS.into(),
                span: (0, 0),
            });
        }
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
        for (x, y) in js::statements(t, k + 1, b - 1) {
            self.statement(&param, x, y);
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
        if names.is_empty() || !t[k].is("(") || js::matching(t, k) != Some(b - 1) {
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
            | "setServerPassthroughCopyBehavior" => {
                let span = self.span(a, b);
                self.items.push(Item::Left {
                    why: "it is for 11ty's watcher and dev server, and doesn't change the files 11ty writes".into(),
                    span,
                });
            }
            _ => self.todo(a, b, "port this to Rhai", PORTING),
        }
    }

    /// What the plugin `toks[a..b]` is: its module and export.
    fn plugin_ref(&self, (a, b): (usize, usize)) -> Option<Binding> {
        let t = &self.toks;
        if b == a + 1 {
            return self.bindings.get(t[a].ident()?).cloned();
        }
        if let Some(m) = self.required(a, b) {
            return Some(Binding::Module(m));
        }
        if b >= 3 && t[b - 2].is(".") {
            let export = t[b - 1].ident()?.to_string();
            let module = if b == a + 3 {
                match self.bindings.get(t[a].ident()?)? {
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
        match self.dir.iter().find(|(k, _)| *k == key) {
            Some((_, old)) if *old != value => self.todo(
                a,
                b,
                format!("`dir.{key}` is set twice; check which one 11ty uses"),
                link,
            ),
            Some(_) => {}
            None => self.dir.push((key, value)),
        }
    }

    /// The object the config returns, in `toks[a..b]`.
    fn returned(&mut self, a: usize, b: usize) {
        let link = format!("{DOCS}#what-the-script-returns");
        let Some(props) = js::object_props(&self.toks, a, b) else {
            return self.todo(a, b, "port what the config returns", link);
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
                            self.engines.push((key, js::rhai_string(&e)))
                        }
                        Some(Lit::Bool(false)) => self.engines.push((key, "false".into())),
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

    /// The lines `span` covers, whole.
    fn quote(&self, span: (usize, usize)) -> (usize, usize, Vec<&'a str>) {
        let src = self.src;
        let start = src[..span.0].rfind('\n').map_or(0, |i| i + 1);
        let end = src[span.1..].find('\n').map_or(src.len(), |i| span.1 + i);
        let first = src[..start].matches('\n').count() + 1;
        let lines: Vec<&str> = src[start..end]
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .collect();
        (first, first + lines.len() - 1, lines)
    }

    fn quoted(&self, out: &mut String, span: (usize, usize)) {
        if span == (0, 0) {
            return;
        }
        let (first, last, lines) = self.quote(span);
        if first == last {
            out.push_str(&format!("// {}, line {first}:\n", self.file));
        } else {
            out.push_str(&format!("// {}, lines {first}-{last}:\n", self.file));
        }
        for l in lines {
            if l.trim().is_empty() {
                out.push_str("//\n");
            } else {
                out.push_str(&format!("//     {l}\n"));
            }
        }
    }

    fn finish(self) -> Port {
        let mut out = format!(
            "// Written by `brrrty init` from {file}, which Brrrty doesn't run.\n\
             // Brrrty reads this file instead; 11ty still reads {file}.\n\
             // Reference: {DOCS}\n",
            file = self.file
        );
        let mut todos = 0;
        let mut last_single = false;
        for item in &self.items {
            match item {
                Item::Rhai(line) => {
                    let single =
                        !line.contains('\n') || line.lines().all(|l| l.starts_with("config."));
                    if !(single && last_single) {
                        out.push('\n');
                    }
                    out.push_str(line);
                    out.push('\n');
                    last_single = single;
                }
                Item::Todo { why, link, span } => {
                    todos += 1;
                    out.push_str(&format!("\n// TODO: {why}.\n// See {link}\n"));
                    self.quoted(&mut out, *span);
                    last_single = false;
                }
                Item::Left { why, span } => {
                    out.push_str(&format!("\n// Left out: {why}.\n"));
                    self.quoted(&mut out, *span);
                    last_single = false;
                }
            }
        }
        if self.left_data_engine {
            out.push_str("\n// Left out: `dataTemplateEngine`, which 11ty 3 doesn't read.\n");
        }
        if !self.dir.is_empty() || !self.engines.is_empty() {
            out.push_str("\n#{\n");
            if !self.dir.is_empty() {
                out.push_str("    dir: #{\n");
                for (k, v) in &self.dir {
                    out.push_str(&format!("        {k}: {},\n", js::rhai_string(v)));
                }
                out.push_str("    },\n");
            }
            for (k, v) in &self.engines {
                out.push_str(&format!("    {k}: {v},\n"));
            }
            out.push_str("}\n");
        }
        let dir = |key: &str| {
            self.dir
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
        };
        Port {
            text: out,
            todos,
            input: dir("input"),
            output: dir("output"),
        }
    }
}

/// A path 11ty would read as a glob.
fn glob(path: &str) -> bool {
    path.contains(['*', '?', '[', ']', '{', '}']) || path.starts_with('!')
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
        assert!(t.contains("// Left out: it is for 11ty's watcher"), "{t}");
        assert!(t.contains("#{\n    dir: #{\n        input: \"src\",\n        output: \"dist\",\n    },\n    markdownTemplateEngine: \"njk\",\n}\n"), "{t}");
        assert_eq!(p.todos, 4, "{t}");
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
}
