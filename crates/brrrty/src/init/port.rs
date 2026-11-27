//! Writes `brrrty.config.rhai` from an 11ty config file, reading it without
//! running it.
//!
//! Only what can be read exactly becomes Rhai: the folders, the template
//! engines, passthrough copies and ignores with literal paths, layout aliases,
//! and the plugins Brrrty has, with literal options. Every other statement is
//! quoted as it is, under a `// TODO(brrrty):` line that says where to read
//! about porting it; a file it can't port at all is one `// TODO(brrrty-file):`.
//! A build refuses every page while one of those is left (see
//! `brrrty_config::markers`). Statements that only set up 11ty's watcher are
//! quoted under a `// Left out:` line instead, as they change nothing 11ty
//! writes.
//!
//! The config object is followed where it goes, as far as that can be known
//! without running anything: into helper functions in the same file or in
//! the site's own modules, into local plugins (which 11ty runs after the
//! config function), and through loops over literal arrays and objects or
//! over a local module's exports. A statement that runs only when a
//! condition holds (`if (process.env.X)`) is one TODO, quoted whole. Where the
//! config object goes somewhere Brrrty can't follow, nothing in the file is
//! certain, and the whole file is one TODO.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use brrrty_config::markers::{TODO, TODO_FILE};

use super::js::{self, Lit, Tok, Token};
use brrrty_config::eleventy_cli::{Cli, CliTodo};

const DOCS: &str = "https://brrrty.com/docs/configuration/";
const RUNS_ON_LOAD: &str = "this loads a module whose own code runs when 11ty loads the config; port what it does if it changes files 11ty reads or any setting, or delete this line";
const GETTING_STARTED: &str = "https://brrrty.com/docs/getting-started/#1-port-the-config";
const PORTING: &str = "https://github.com/brrrty/brrrty/blob/main/crates/brrrty-config/PORTING.md";
const SYNTAX_HIGHLIGHT: &str = "@11ty/eleventy-plugin-syntaxhighlight";
const RSS: &str = "@11ty/eleventy-plugin-rss";

/// The TODO for a declaration whose uses weren't all ported or quoted.
const DECLARED: &str = "port this code along with the code that uses it";
/// The TODO for code a call runs in its arguments, which Brrrty doesn't port.
const ARGUMENTS: &str = "this runs code in its arguments that Brrrty doesn't port (it follows the rest of the call); port what that code does";

/// The config line that turns the JS config on.
const JS_CONFIG_LINE: &str = "config.set_js_config(\"node\");";
/// Why a statement isn't ported when the JS config runs it.
const JS_RUNS_WHY: &str = "Brrrty runs this in your own Node, as 11ty does (config.set_js_config)";
/// The config methods the JS config applies.
const JS_RUNS: &[&str] = &[
    "addFilter",
    "addAsyncFilter",
    "addLiquidFilter",
    "addNunjucksFilter",
    "addNunjucksAsyncFilter",
    "addJavaScriptFilter",
    "addTransform",
    "addGlobalData",
    "addPassthroughCopy",
    "addBundle",
    "ignores",
    "setTemplateFormats",
    "addTemplateFormats",
    "addPlugin",
    "setInputDirectory",
    "setOutputDirectory",
    "setIncludesDirectory",
    "setDataDirectory",
    "setLayoutsDirectory",
];
/// Config methods it records as unsupported that leave the written pages alone: only
/// these stay a warning. Every other unsupported method blocks (a TODO when ported,
/// a refusal at build time).
const JS_HARMLESS: &[&str] = &[
    "addWatchTarget",
    "setWatchThrottleWaitTime",
    "setWatchJavaScriptDependencies",
    "setChokidarConfig",
    "setServerOptions",
    "setServerPassthroughCopyBehavior",
    "setBrowserSyncConfig",
    "setQuietMode",
    "setFreezeReservedData",
    "addLinter",
];

/// The recorder reports `method` as unsupported: it changes what pages are written
/// unless it's known harmless, and the JS config doesn't apply it.
pub(crate) fn js_blocks(method: &str) -> bool {
    let m = method.trim_end_matches(" = ");
    !JS_HARMLESS.contains(&m) && !JS_RUNS.contains(&m) && !is_shortcode(m)
}

fn is_shortcode(m: &str) -> bool {
    (m.starts_with("add") && m.ends_with("Shortcode"))
        || (m.starts_with("addPaired") && m.contains("Shortcode"))
}

/// An identifier that reads like a config method call (`addX`, `setX`, `amendX`), plus `on`.
fn config_method_like(i: &str) -> bool {
    i == "on"
        || i == "htmlTransformer"
        || ["add", "set", "amend"].iter().any(|p| {
            i.strip_prefix(p)
                .is_some_and(|r| r.starts_with(|c: char| c.is_ascii_uppercase()))
        })
}

/// How deep helpers, plugins and modules are followed.
const MAX_FOLLOW: usize = 16;
/// How many statements (loop turns included) are read before Brrrty stops.
const MAX_STEPS: usize = 100_000;
/// How deep a value's definition is followed.
const MAX_EVAL: usize = 32;

/// `brrrty.config.rhai`'s text, and what it says.
#[derive(Debug)]
pub(crate) struct Port {
    pub text: String,
    /// How many `// TODO(brrrty)` markers it has.
    pub todos: usize,
    /// `dir.input`, when the config sets it.
    pub input: Option<String>,
    /// `dir.output`, when the config sets it.
    pub output: Option<String>,
    /// `dir.data`, when the config sets it.
    pub data: Option<String>,
    /// The site's own modules read while porting, by path, with their text.
    pub modules: Vec<(String, String)>,
    /// It adds the RSS plugin, whose version a build checks.
    pub uses_rss: bool,
    /// The config needs `config.set_js_config("node")`, and the text says so.
    pub js: bool,
}

/// Reads one of the site's files by its path from the site's folder.
pub(crate) type Load<'l> = &'l dyn Fn(&str) -> Result<Option<String>, String>;

/// A function to follow: its module, its code, and the names it closes over.
type Callee = (Rc<Module>, Rc<FnDef>, Option<Rc<Scope>>);

/// What the porter may know about the site besides the config file.
#[derive(Clone, Copy, Default)]
pub(crate) struct Site<'l> {
    /// The flags the build script passes to 11ty, which win over the config.
    pub cli: Option<&'l Cli>,
    /// Reads one of the site's files by its path from the site's folder: `None` when
    /// there is none, an error when it can't be read.
    pub load: Option<Load<'l>>,
    /// The major version of `@11ty/eleventy-plugin-rss` the site installs.
    pub rss: Option<u64>,
    /// Port for `config.set_js_config("node")`: what the JS config runs is not a TODO.
    pub js: bool,
}

/// How a module was brought in.
#[derive(Debug, Clone, Copy, PartialEq)]
enum How {
    /// `import x from`: the default export (`module.exports` for CommonJS).
    Default,
    /// `require(...)`: `module.exports`, or an ES module's namespace.
    Require,
    /// `import * as x from`.
    Namespace,
}

/// What a name from an `import` or `require` stands for.
#[derive(Debug, Clone)]
enum Binding {
    /// The module itself, or its default export.
    Module(String, How),
    /// One export of the module.
    Export(String, String),
}

/// A top-level declaration: what a name is defined as.
#[derive(Debug, Clone, Copy)]
enum Decl {
    /// A function declaration, from `function` (or `async`) to its end.
    Func(usize, usize),
    /// `const name = ...`: the value's tokens.
    Const(usize, usize),
    /// Something Brrrty doesn't read (`let`, `var`, `class`).
    Unknown,
}

/// A module's export statement, read lazily.
#[derive(Debug, Clone)]
enum Export {
    /// `module.exports = ...`.
    Main(usize, usize),
    /// `module.exports.name = ...` or (`exports`) `exports.name = ...`.
    Prop {
        name: String,
        value: (usize, usize),
        exports: bool,
    },
    /// `export default ...`.
    Default(usize, usize),
    /// `export function a`, `export const a`, `export { a as b }`: the
    /// exported name and the local one.
    Named(String, String),
    /// `export * from`, `export { a } from`: Brrrty doesn't follow these.
    Opaque,
}

/// One JavaScript file: the config, or a module it uses.
struct Module {
    /// Its path from the site's folder.
    file: String,
    src: String,
    toks: Vec<Token>,
    /// Where each line of `src` starts and ends, without its terminator.
    lines: Vec<(usize, usize)>,
    /// For each name: how often it appears, and how often as an `addPlugin` argument.
    uses: HashMap<String, (usize, usize)>,
    /// Where each name appears, other than as a property, by token.
    occ: HashMap<String, Vec<usize>>,
    bindings: HashMap<String, Binding>,
    /// Top-level names that can be assigned again (`let`, `var`).
    mutable: HashSet<String>,
    decls: HashMap<String, Decl>,
    exports: Vec<Export>,
    /// It has `import` or `export`.
    esm: bool,
    /// Tokens whose name was ported or quoted where it was used.
    consumed: RefCell<HashSet<usize>>,
    /// Declarations quoted as they are.
    consumed_decls: RefCell<HashSet<String>>,
    /// [`Module::stable`] over the whole module, by name.
    stable_names: RefCell<HashMap<String, bool>>,
    /// The site's modules it names in `require` or `import`, anywhere.
    deps: Vec<String>,
}

/// A place in a module, for quoting.
#[derive(Clone)]
struct Src {
    m: Rc<Module>,
    /// Byte offsets.
    span: (usize, usize),
}

/// A function's parameter.
#[derive(Debug, Clone)]
enum Param {
    /// A name, and its default value's tokens.
    Name(String, Option<(usize, usize)>),
    /// `[a, b]`: names, or holes.
    Array(Vec<Option<String>>),
    /// Anything else: the names it may bind.
    Other(Vec<String>),
}

/// A function Brrrty can follow.
#[derive(Debug, Clone)]
struct FnDef {
    params: Vec<Param>,
    /// The body's tokens, inside its braces, or the expression of `=> expr`.
    body: (usize, usize),
    expr: bool,
    /// The whole function's tokens, for quoting.
    at: (usize, usize),
}

/// What an expression is, as far as Brrrty can tell without running it.
#[derive(Clone)]
enum Val {
    /// The config object 11ty passes in.
    Config,
    Lit(Lit),
    /// `undefined`, as a missing argument is.
    Undefined,
    Func(Rc<Module>, Rc<FnDef>, Option<Rc<Scope>>),
    Obj(Vec<Entry>),
    Arr(Vec<Val>),
    /// One of the site's own modules.
    Module(Rc<Module>, How),
    /// A package from npm, or one of its exports.
    Npm(Binding),
    /// Code Brrrty doesn't read, and where it is.
    Code(Src),
    Unknown,
}

/// An object's property.
#[derive(Clone)]
struct Entry {
    key: String,
    val: Val,
}

/// The names a function's code can see besides its module's.
#[derive(Clone, Default)]
struct Scope {
    vars: HashMap<String, Val>,
    /// The loop this turn of code runs in: where its statement starts.
    in_loop: Option<Src>,
    /// Where the function body being read ends, by token.
    body_end: usize,
}

/// Which function a body belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    /// The config function itself: its `return` is the config.
    Config,
    /// A function the config calls.
    Helper,
    /// A plugin 11ty runs.
    Plugin,
}

enum Item {
    Rhai(String),
    /// A statement not ported. `marker`: it may change what 11ty writes, so it gets a
    /// `TODO(brrrty)` marker; otherwise it is only noted as left out.
    Todo {
        why: String,
        link: String,
        at: Option<Src>,
        marker: bool,
    },
    /// A declaration or import: a TODO unless every use of what it names
    /// was ported or quoted. `main`: a name nothing uses still gets one, as
    /// the config file's own code always did.
    Decl {
        why: &'static str,
        names: Vec<(String, usize)>,
        at: Src,
        main: bool,
    },
    /// A declaration in a function: the same, for the uses in `range`.
    Local {
        name: String,
        decl: usize,
        range: (usize, usize),
        at: Src,
    },
}

struct Setting {
    /// `dir.input`, `markdownTemplateEngine` and so on.
    key: &'static str,
    /// The value, as Rhai.
    value: String,
    /// The value as written, for `dir.*`.
    raw: Option<String>,
    at: Option<Src>,
}

/// What every module's reader shares.
struct Out<'l> {
    items: Vec<Item>,
    /// TODOs for code that runs when a module loads.
    loaded_items: Vec<Item>,
    settings: Vec<Setting>,
    left_data_engine: bool,
    /// Set when nothing can be ported: the whole file becomes one TODO.
    whole: Option<String>,
    site: Site<'l>,
    /// The site's modules by path; `None` when one can't be read.
    modules: HashMap<String, Option<Rc<Module>>>,
    /// Modules read, in order, with their text.
    read: Vec<(String, String)>,
    /// Modules being loaded, for cycles.
    loading: Vec<String>,
    depth: usize,
    evals: usize,
    steps: usize,
    /// Plugins the config function adds, which 11ty runs after it.
    deferred: Option<Vec<(Callee, Val)>>,
    /// 11ty is running plugins: `addPlugin` runs the plugin at once.
    in_plugin: bool,
    /// Inside a statement that is one TODO as a whole.
    cond: usize,
    /// Names read while evaluating, by module and token, until ported or dropped.
    touched: Vec<(Rc<Module>, usize)>,
    /// Decls touched, until ported or dropped.
    touched_decls: Vec<(Rc<Module>, String)>,
    /// What the last config call did with the names it read.
    keep: bool,
    /// 11ty reads no config file.
    no_config: bool,
    /// Flags from the build script.
    cli: Option<Cli>,
    /// Something was left to the JS config (`Site::js`).
    js_ran: bool,
    /// In js mode, the reason nothing here could be read: the JS config runs it all.
    js_whole: Option<String>,
}

struct Reader<'o, 'l> {
    m: Rc<Module>,
    out: &'o mut Out<'l>,
}

/// Line terminators: JavaScript's, and every one Rhai might end a comment at.
fn is_terminator(c: char) -> bool {
    js::is_line_terminator(c) || c == '\u{85}'
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
#[cfg(test)]
pub(crate) fn port(src: &str, file: &str) -> Port {
    port_site(src, file, Site::default())
}

/// [`port`], knowing what [`Site`] says.
pub(crate) fn port_site(src: &str, file: &str, site: Site) -> Port {
    let mut out = Out::new(site);
    let toks = match js::tokenize(src) {
        Ok(t) => t,
        Err(why) => {
            out.whole = Some(format!("Brrrty couldn't read this file ({why})"));
            let m = Rc::new(Module::new(file, src, Vec::new()));
            return out.finish(&m);
        }
    };
    let m = Rc::new(Module::new(file, src, toks));
    let mut r = Reader {
        m: m.clone(),
        out: &mut out,
    };
    r.read();
    out.finish(&m)
}

/// A port that is one TODO quoting all of `src`.
pub(crate) fn unreadable(src: &str, file: &str, why: &str, cli: Option<&Cli>) -> Port {
    let mut out = Out::new(Site {
        cli,
        ..Site::default()
    });
    out.whole = Some(why.to_string());
    out.finish(&Rc::new(Module::new(file, src, Vec::new())))
}

/// The config for a site 11ty builds with no config file: the build script's
/// flags, if any.
pub(crate) fn no_config(cli: Option<&Cli>) -> Port {
    let mut out = Out::new(Site {
        cli,
        ..Site::default()
    });
    out.no_config = true;
    out.finish(&Rc::new(Module::new("", "", Vec::new())))
}

/// Keywords whose `(...) {` is not a function body.
const NOT_METHODS: [&str; 6] = ["if", "for", "while", "switch", "catch", "with"];

/// Whether the statement at `toks[a]` exports: `module.exports`, `exports.x`
/// or `export` (but not `import`).
fn is_export(t: &[Token], a: usize) -> bool {
    t[a].is_ident("export")
        || t[a].is_ident("exports")
        || (t[a].is_ident("module")
            && t.get(a + 1).is_some_and(|t| t.is("."))
            && t.get(a + 2).is_some_and(|t| t.is_ident("exports")))
}

/// `module.exports = ` or `export default `: the index after it.
fn export_start(t: &[Token], a: usize, b: usize) -> Option<usize> {
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
fn config_export(t: &[Token], a: usize, b: usize) -> Option<usize> {
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

/// The name of the function declared at `a`, if it is a declaration.
fn fn_decl(t: &[Token], a: usize) -> Option<&str> {
    let k = if t.get(a)?.is_ident("async") {
        a + 1
    } else {
        a
    };
    if !t.get(k)?.is_ident("function") {
        return None;
    }
    t.get(k + 1)?.ident()
}

/// The module of `require("m")`, when that is exactly `toks[a..b]`.
fn required(t: &[Token], a: usize, b: usize) -> Option<String> {
    if b != a + 4 || !t[a].is_ident("require") || !t[a + 1].is("(") || !t[a + 3].is(")") {
        return None;
    }
    match &t[a + 2].tok {
        Tok::Str(Some(m)) => Some(m.clone()),
        _ => None,
    }
}

/// The names an `import` statement binds, with the token of each local name.
/// `Some(vec![])` for `import "x"`; `None` when it can't be read.
fn import_names(t: &[Token], a: usize, b: usize) -> Option<Vec<(String, Binding, usize)>> {
    let Some(Tok::Str(Some(module))) = t.get(b - 1).map(|t| &t.tok) else {
        return None;
    };
    if b == a + 2 {
        return Some(Vec::new());
    }
    if b < a + 4 || !t[b - 2].is_ident("from") {
        return None;
    }
    let mut k = a + 1;
    let end = b - 2;
    let mut found = Vec::new();
    while k < end {
        if t[k].is(",") {
            k += 1;
        } else if t[k].is("*") && k + 2 < end + 1 && t[k + 1].is_ident("as") {
            let n = t[k + 2].ident()?;
            found.push((
                n.to_string(),
                Binding::Module(module.clone(), How::Namespace),
                k + 2,
            ));
            k += 3;
        } else if t[k].is("{") {
            let close = js::matching(t, k)?;
            for (x, y) in js::split_commas(t, k + 1, close) {
                let name = t[x].ident().or(match &t[x].tok {
                    Tok::Str(Some(s)) => Some(s.as_str()),
                    _ => None,
                })?;
                let (local, at) = if y == x + 3 && t[x + 1].is_ident("as") {
                    (t[x + 2].ident()?, x + 2)
                } else if y == x + 1 {
                    (name, x)
                } else {
                    return None;
                };
                found.push((
                    local.to_string(),
                    Binding::Export(module.clone(), name.to_string()),
                    at,
                ));
            }
            k = close + 1;
        } else {
            let n = t[k].ident()?;
            found.push((
                n.to_string(),
                Binding::Module(module.clone(), How::Default),
                k,
            ));
            k += 1;
        }
    }
    Some(found)
}

/// `const x = require("m")`, `const { a, b: c } = require("m")` or
/// `const x = require("m").a`: the names it binds.
fn require_names(t: &[Token], a: usize, b: usize) -> Option<Vec<(String, Binding, usize)>> {
    if !matches!(t[a].ident(), Some("const" | "let" | "var")) {
        return None;
    }
    let eq = (a..b).find(|&k| t[k].is("="))?;
    let module = required(t, eq + 1, b.min(eq + 5))?;
    let after = eq + 5;
    let export = if after == b {
        None
    } else if after + 2 == b && t[after].is(".") {
        Some(t[after + 1].ident()?.to_string())
    } else {
        return None;
    };
    let mut found = Vec::new();
    if eq == a + 2 {
        let n = t[a + 1].ident()?;
        let binding = match export {
            Some(e) => Binding::Export(module, e),
            None => Binding::Module(module, How::Require),
        };
        found.push((n.to_string(), binding, a + 1));
    } else if export.is_none() && t[a + 1].is("{") && js::matching(t, a + 1) == Some(eq - 1) {
        for (x, y) in js::split_commas(t, a + 2, eq - 1) {
            let name = t[x].ident()?;
            let (local, at) = if y == x + 3 && t[x + 1].is(":") {
                (t[x + 2].ident()?, x + 2)
            } else if y == x + 1 {
                (name, x)
            } else {
                return None;
            };
            found.push((
                local.to_string(),
                Binding::Export(module.clone(), name.into()),
                at,
            ));
        }
    } else {
        return None;
    }
    Some(found)
}

/// Whether a module path is one of the site's own files.
fn is_local(module: &str) -> bool {
    module.starts_with("./") || module.starts_with("../")
}

/// The parameters in `toks[a..b]` (inside the parentheses).
fn params(t: &[Token], a: usize, b: usize) -> Vec<Param> {
    let names = |x: usize, y: usize| -> Vec<String> {
        (x..y)
            .filter(|&k| !(k > 0 && t[k - 1].is(".")))
            .filter_map(|k| t[k].ident().map(str::to_string))
            .collect()
    };
    js::split_commas(t, a, b)
        .into_iter()
        .map(|(x, y)| {
            if y == x + 1
                && let Some(n) = t[x].ident()
            {
                return Param::Name(n.to_string(), None);
            }
            if y > x + 2
                && t[x + 1].is("=")
                && let Some(n) = t[x].ident()
            {
                return Param::Name(n.to_string(), Some((x + 2, y)));
            }
            if t[x].is("[") && js::matching(t, x) == Some(y - 1) {
                let mut items = Vec::new();
                for (p, q) in js::split_commas(t, x + 1, y - 1) {
                    if q == p + 1
                        && let Some(n) = t[p].ident()
                    {
                        items.push(Some(n.to_string()));
                    } else {
                        return Param::Other(names(x, y));
                    }
                }
                return Param::Array(items);
            }
            Param::Other(names(x, y))
        })
        .collect()
}

/// The function that is exactly `toks[a..b]`: a function expression, an
/// arrow function, or a function declaration.
fn fn_at(t: &[Token], a: usize, b: usize) -> Option<FnDef> {
    if a >= b || b > t.len() {
        return None;
    }
    let mut k = a;
    if t[k].is_ident("async") && b > k + 1 {
        k += 1;
    }
    if t[k].is_ident("function") {
        k += 1;
        if t.get(k)?.is("*") {
            return None;
        }
        if t.get(k)?.ident().is_some() {
            k += 1;
        }
        if !t.get(k)?.is("(") {
            return None;
        }
        let close = js::matching(t, k)?;
        let ps = params(t, k + 1, close);
        let open = close + 1;
        if !t.get(open)?.is("{") || js::matching(t, open)? != b - 1 {
            return None;
        }
        return Some(FnDef {
            params: ps,
            body: (open + 1, b - 1),
            expr: false,
            at: (a, b),
        });
    }
    let ps = if t[k].is("(") {
        let close = js::matching(t, k)?;
        let ps = params(t, k + 1, close);
        k = close + 1;
        ps
    } else {
        let n = t[k].ident()?;
        k += 1;
        vec![Param::Name(n.to_string(), None)]
    };
    if !t.get(k)?.is("=>") || t.get(k)?.nl_before {
        return None;
    }
    k += 1;
    if k >= b {
        return None;
    }
    if t[k].is("{") && js::matching(t, k) == Some(b - 1) {
        return Some(FnDef {
            params: ps,
            body: (k + 1, b - 1),
            expr: false,
            at: (a, b),
        });
    }
    Some(FnDef {
        params: ps,
        body: (k, b),
        expr: true,
        at: (a, b),
    })
}

/// One property of an object literal, as written.
enum RawEntry {
    /// `key: value`.
    Value(String, (usize, usize)),
    /// `key`, short for `key: key`.
    Short(String, usize),
    /// `key(...) { ... }`.
    Method(String, FnDef),
}

/// The properties of the object literal that is exactly `toks[a..b]`, when
/// each is `key: value`, `key` or a method.
fn raw_entries(t: &[Token], a: usize, b: usize) -> Option<Vec<RawEntry>> {
    if b <= a || !t[a].is("{") || js::matching(t, a)? != b - 1 {
        return None;
    }
    let mut out = Vec::new();
    for (x, y) in js::split_commas(t, a + 1, b - 1) {
        let key_of = |k: usize| -> Option<String> {
            match &t[k].tok {
                Tok::Ident(s) => Some(s.clone()),
                Tok::Str(Some(s)) => Some(s.clone()),
                Tok::Num(n) if n.bytes().all(|c| c.is_ascii_digit()) => {
                    Some(n.trim_start_matches('0').to_string())
                        .map(|s| if s.is_empty() { "0".to_string() } else { s })
                }
                _ => None,
            }
        };
        if y == x + 1 {
            let n = t[x].ident()?;
            out.push(RawEntry::Short(n.to_string(), x));
            continue;
        }
        if t.get(x + 1).is_some_and(|n| n.is(":")) {
            if y < x + 3 {
                return None;
            }
            out.push(RawEntry::Value(key_of(x)?, (x + 2, y)));
            continue;
        }
        // `name(...) {}` or `async name(...) {}`.
        let k = if t[x].is_ident("async") && !t[x + 1].is("(") {
            x + 1
        } else {
            x
        };
        if t.get(k + 1).is_some_and(|n| n.is("(")) {
            let key = key_of(k)?;
            let close = js::matching(t, k + 1)?;
            let open = close + 1;
            if !t.get(open)?.is("{") || js::matching(t, open)? != y - 1 {
                return None;
            }
            out.push(RawEntry::Method(
                key,
                FnDef {
                    params: params(t, k + 2, close),
                    body: (open + 1, y - 1),
                    expr: false,
                    at: (x, y),
                },
            ));
            continue;
        }
        return None;
    }
    Some(out)
}

/// JavaScript's property order: integer keys first, in order, then the rest
/// as added. A repeated key keeps its first place and its last value.
fn object(entries: Vec<Entry>) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for e in entries {
        if let Some(slot) = out.iter_mut().find(|o| o.key == e.key) {
            *slot = e;
        } else {
            out.push(e);
        }
    }
    let index = |k: &str| -> Option<u64> {
        if k == "0" || (!k.starts_with('0') && !k.is_empty() && k.len() <= 10) {
            k.parse::<u64>().ok().filter(|n| *n < u32::MAX as u64)
        } else {
            None
        }
    };
    let mut ints: Vec<Entry> = Vec::new();
    let mut rest: Vec<Entry> = Vec::new();
    for e in out {
        if index(&e.key).is_some() {
            ints.push(e);
        } else {
            rest.push(e);
        }
    }
    ints.sort_by_key(|e| index(&e.key));
    ints.extend(rest);
    ints
}

impl Module {
    fn new(file: &str, src: &str, toks: Vec<Token>) -> Self {
        let mut uses: HashMap<String, (usize, usize)> = HashMap::new();
        let mut occ: HashMap<String, Vec<usize>> = HashMap::new();
        for (k, t) in toks.iter().enumerate() {
            for n in &t.inner {
                uses.entry(n.clone()).or_default().0 += 1;
            }
            let Some(name) = t.ident() else { continue };
            if k > 0 && (toks[k - 1].is(".") || toks[k - 1].is("?.")) {
                continue;
            }
            occ.entry(name.to_string()).or_default().push(k);
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
        let mut m = Module {
            file: file.to_string(),
            src: src.to_string(),
            lines: split_lines(src),
            uses,
            occ,
            bindings: HashMap::new(),
            mutable: HashSet::new(),
            decls: HashMap::new(),
            exports: Vec::new(),
            esm: false,
            consumed: RefCell::new(HashSet::new()),
            consumed_decls: RefCell::new(HashSet::new()),
            stable_names: RefCell::new(HashMap::new()),
            deps: Vec::new(),
            toks,
        };
        m.scan();
        m
    }

    /// Read the top-level declarations, imports and exports.
    fn scan(&mut self) {
        let t = &self.toks;
        let mut bindings = HashMap::new();
        let mut decls = HashMap::new();
        let mut mutable = HashSet::new();
        let mut exports = Vec::new();
        let mut esm = false;
        for (a, b) in js::statements(t, 0, t.len()) {
            if t[a].is_ident("import") && !t.get(a + 1).is_some_and(|n| n.is("(") || n.is(".")) {
                esm = true;
                if let Some(found) = import_names(t, a, b) {
                    for (n, binding, _) in found {
                        bindings.insert(n, binding);
                    }
                }
                continue;
            }
            if let Some(found) = require_names(t, a, b) {
                for (n, binding, _) in found {
                    if !t[a].is_ident("const") {
                        mutable.insert(n.clone());
                    }
                    bindings.insert(n, binding);
                }
                continue;
            }
            if let Some(name) = fn_decl(t, a) {
                decls.insert(name.to_string(), Decl::Func(a, b));
                continue;
            }
            if let Some(kw @ ("const" | "let" | "var")) = t[a].ident() {
                if b > a + 3
                    && let Some(n) = t[a + 1].ident()
                    && t[a + 2].is("=")
                    && js::split_commas(t, a + 3, b).len() == 1
                {
                    decls.insert(
                        n.to_string(),
                        if kw == "const" {
                            Decl::Const(a + 3, b)
                        } else {
                            Decl::Unknown
                        },
                    );
                    if kw != "const" {
                        mutable.insert(n.to_string());
                    }
                } else {
                    // Destructuring, or more than one name: none of them is read.
                    let eq = (a..b).find(|&k| t[k].is("=")).unwrap_or(b);
                    for k in a + 1..eq {
                        if let Some(n) = t[k].ident()
                            && !t[k - 1].is(":")
                        {
                            decls.insert(n.to_string(), Decl::Unknown);
                            mutable.insert(n.to_string());
                        }
                    }
                }
                continue;
            }
            if t[a].is_ident("class")
                && let Some(n) = t.get(a + 1).and_then(Token::ident)
            {
                decls.insert(n.to_string(), Decl::Unknown);
                continue;
            }
            // Exports.
            if t[a].is_ident("export") {
                esm = true;
                if t.get(a + 1).is_some_and(|n| n.is_ident("default")) {
                    let k = a + 2;
                    if let Some(name) = fn_decl(t, k) {
                        decls.insert(name.to_string(), Decl::Func(k, b));
                    }
                    exports.push(Export::Default(k, b));
                    continue;
                }
                if let Some(name) = fn_decl(t, a + 1) {
                    decls.insert(name.to_string(), Decl::Func(a + 1, b));
                    exports.push(Export::Named(name.to_string(), name.to_string()));
                    continue;
                }
                if t.get(a + 1).is_some_and(|n| n.is_ident("const"))
                    && b > a + 4
                    && let Some(n) = t[a + 2].ident()
                    && t[a + 3].is("=")
                    && js::split_commas(t, a + 4, b).len() == 1
                {
                    decls.insert(n.to_string(), Decl::Const(a + 4, b));
                    exports.push(Export::Named(n.to_string(), n.to_string()));
                    continue;
                }
                if t.get(a + 1).is_some_and(|n| n.is("{"))
                    && let Some(close) = js::matching(t, a + 1)
                    && close + 1 == b
                {
                    for (x, y) in js::split_commas(t, a + 2, close) {
                        let (Some(local), exported) = (
                            t[x].ident(),
                            if y == x + 3 && t[x + 1].is_ident("as") {
                                t[x + 2].ident()
                            } else if y == x + 1 {
                                t[x].ident()
                            } else {
                                None
                            },
                        ) else {
                            exports.push(Export::Opaque);
                            continue;
                        };
                        match exported {
                            Some(e) => {
                                exports.push(Export::Named(e.to_string(), local.to_string()))
                            }
                            None => exports.push(Export::Opaque),
                        }
                    }
                    continue;
                }
                exports.push(Export::Opaque);
                continue;
            }
            if let Some(rest) = export_start(t, a, b) {
                exports.push(Export::Main(rest, b));
                continue;
            }
            let prop = |k: usize| -> Option<String> {
                if t.get(k)?.is(".") && t.get(k + 2)?.is("=") {
                    t.get(k + 1)?.ident().map(str::to_string)
                } else {
                    None
                }
            };
            if t[a].is_ident("module")
                && t.get(a + 1).is_some_and(|n| n.is("."))
                && t.get(a + 2).is_some_and(|n| n.is_ident("exports"))
                && let Some(name) = prop(a + 3)
            {
                exports.push(Export::Prop {
                    name,
                    value: (a + 6, b),
                    exports: false,
                });
                continue;
            }
            if t[a].is_ident("exports")
                && let Some(name) = prop(a + 1)
            {
                exports.push(Export::Prop {
                    name,
                    value: (a + 4, b),
                    exports: true,
                });
                continue;
            }
        }
        // Every local module named after `require(`, `import` or `from`.
        let mut deps = Vec::new();
        for (k, tok) in t.iter().enumerate() {
            let Tok::Str(Some(spec)) = &tok.tok else {
                continue;
            };
            let named = k >= 2 && t[k - 1].is("(") && t[k - 2].is_ident("require")
                || k >= 1 && (t[k - 1].is_ident("from") || t[k - 1].is_ident("import"));
            if named && is_local(spec) && !deps.contains(spec) {
                deps.push(spec.clone());
            }
        }
        self.deps = deps;
        self.bindings = bindings;
        self.decls = decls;
        self.mutable = mutable;
        self.exports = exports;
        self.esm = esm;
    }

    /// Whether `name` keeps the value it was given in `toks[range]`: nothing
    /// there assigns it, assigns or deletes one of its properties, calls a
    /// method that changes it (`push`, `splice`, ...) or hands it to a
    /// function that could. A name used inside a template literal's `${}`
    /// counts as changed, as those uses aren't read.
    fn stable(&self, name: &str, range: (usize, usize)) -> bool {
        const MUTATING: [&str; 13] = [
            "push",
            "pop",
            "shift",
            "unshift",
            "splice",
            "sort",
            "reverse",
            "fill",
            "copyWithin",
            "set",
            "add",
            "delete",
            "clear",
        ];
        const READING: [&str; 22] = [
            "keys",
            "values",
            "entries",
            "forEach",
            "map",
            "filter",
            "some",
            "every",
            "find",
            "includes",
            "join",
            "indexOf",
            "flatMap",
            "reduce",
            "concat",
            "slice",
            "at",
            "isArray",
            "stringify",
            "log",
            "warn",
            "require",
        ];
        let t = &self.toks;
        let (from, to) = (range.0.min(t.len()), range.1.min(t.len()));
        let assigns = |k: usize| {
            t.get(k).is_some_and(|n| {
                matches!(
                    n.tok,
                    Tok::Punct(
                        "=" | "+="
                            | "-="
                            | "*="
                            | "/="
                            | "%="
                            | "**="
                            | "<<="
                            | ">>="
                            | ">>>="
                            | "&="
                            | "|="
                            | "^="
                            | "&&="
                            | "||="
                            | "??="
                            | "++"
                            | "--"
                    )
                )
            })
        };
        if t[from..to]
            .iter()
            .any(|x| x.inner.iter().any(|n| n == name))
        {
            return false;
        }
        let Some(occ) = self.occ.get(name) else {
            return true;
        };
        for &k in occ.iter().filter(|&&k| k >= from && k < to) {
            let prev = k.checked_sub(1).map(|p| &t[p]);
            // `const name = ...` declares it.
            if prev.is_some_and(|p| matches!(p.ident(), Some("const" | "let" | "var"))) {
                continue;
            }
            if prev.is_some_and(|p| p.is("++") || p.is("--") || p.is_ident("delete")) {
                return false;
            }
            // Along `.a`, `[b]` and `(...)` to what the expression does.
            let mut j = k + 1;
            let mut last_name: Option<&str> = None;
            let mut called = false;
            loop {
                match t.get(j) {
                    Some(n)
                        if (n.is(".") || n.is("?."))
                            && t.get(j + 1).and_then(Token::ident).is_some() =>
                    {
                        last_name = t[j + 1].ident();
                        called = false;
                        j += 2;
                    }
                    Some(n) if n.is("[") => {
                        let Some(c) = n.pair else { return false };
                        last_name = None;
                        called = false;
                        j = c + 1;
                    }
                    Some(n) if n.is("(") && j > k + 1 => {
                        if last_name.is_some_and(|m| MUTATING.contains(&m)) {
                            return false;
                        }
                        let Some(c) = n.pair else { return false };
                        called = true;
                        j = c + 1;
                    }
                    _ => break,
                }
            }
            if !called && assigns(j) {
                return false;
            }
            // Handed to a call as an argument.
            if j == k + 1
                && prev.is_some_and(|p| p.is("(") || p.is(","))
                && t.get(j).is_some_and(|n| n.is(")") || n.is(","))
            {
                let mut open = None;
                let mut i = k;
                while i > from {
                    i -= 1;
                    if t[i].is("(") && t[i].pair.is_some_and(|c| c > k) {
                        open = Some(i);
                        break;
                    }
                }
                let Some(open) = open else { return false };
                let callee = open.checked_sub(1).and_then(|c| t[c].ident());
                let method = open >= 2 && t[open - 2].is(".");
                let ok = callee.is_some_and(|c| {
                    (method
                        && (c.starts_with("add") || c.starts_with("set") || READING.contains(&c)))
                        || c == "require"
                });
                if !ok {
                    return false;
                }
            }
        }
        true
    }

    /// [`Module::stable`] over the whole module, read once per name.
    fn stable_everywhere(&self, name: &str) -> bool {
        if let Some(&s) = self.stable_names.borrow().get(name) {
            return s;
        }
        let s = self.stable(name, (0, self.toks.len()));
        self.stable_names.borrow_mut().insert(name.to_string(), s);
        s
    }

    /// The byte span of `toks[a..b]`.
    fn span(&self, a: usize, b: usize) -> (usize, usize) {
        if a >= b || b > self.toks.len() {
            return (0, 0);
        }
        (self.toks[a].start, self.toks[b - 1].end)
    }

    /// The line `byte` is on, from 1.
    fn line_of(&self, byte: usize) -> usize {
        self.lines.partition_point(|&(_, end)| end < byte) + 1
    }

    /// Whether every use of `name` other than at `decl`, in `range`, was
    /// ported or quoted. `any`: a name nothing uses counts as unused, so not.
    fn all_consumed(&self, name: &str, decl: usize, range: (usize, usize), any: bool) -> bool {
        if self.consumed_decls.borrow().contains(name) {
            return true;
        }
        let consumed = self.consumed.borrow();
        let occ: Vec<usize> = self
            .occ
            .get(name)
            .map(|v| {
                v.iter()
                    .copied()
                    .filter(|&k| k != decl && k >= range.0 && k < range.1)
                    .collect()
            })
            .unwrap_or_default();
        let in_template = self.toks[range.0.min(self.toks.len())..range.1.min(self.toks.len())]
            .iter()
            .any(|t| t.inner.iter().any(|n| n == name));
        if in_template {
            return false;
        }
        if any && occ.is_empty() {
            return false;
        }
        occ.iter().all(|k| consumed.contains(k))
    }
}

impl<'l> Out<'l> {
    fn new(site: Site<'l>) -> Self {
        Out {
            items: Vec::new(),
            loaded_items: Vec::new(),
            settings: Vec::new(),
            left_data_engine: false,
            whole: None,
            cli: site.cli.cloned(),
            site,
            modules: HashMap::new(),
            read: Vec::new(),
            loading: Vec::new(),
            depth: 0,
            evals: 0,
            steps: 0,
            deferred: None,
            in_plugin: false,
            cond: 0,
            touched: Vec::new(),
            touched_decls: Vec::new(),
            keep: false,
            no_config: false,
            js_ran: false,
            js_whole: None,
        }
    }

    fn give_up(&mut self, why: impl Into<String>) {
        let slot = if self.site.js {
            &mut self.js_whole
        } else {
            &mut self.whole
        };
        if slot.is_none() {
            *slot = Some(why.into());
        }
    }

    /// Count the names read since `mark` as ported or quoted, outside a
    /// statement that is one TODO as a whole.
    fn commit(&mut self, mark: (usize, usize)) {
        let touched: Vec<_> = self.touched.drain(mark.0..).collect();
        let decls: Vec<_> = self.touched_decls.drain(mark.1..).collect();
        if self.cond > 0 {
            return;
        }
        for (m, k) in touched {
            m.consumed.borrow_mut().insert(k);
        }
        for (m, n) in decls {
            m.consumed_decls.borrow_mut().insert(n);
        }
    }

    /// Forget the names read since `mark`.
    fn drop_touched(&mut self, mark: (usize, usize)) {
        self.touched.truncate(mark.0);
        self.touched_decls.truncate(mark.1);
    }

    fn mark(&self) -> (usize, usize) {
        (self.touched.len(), self.touched_decls.len())
    }
}

/// Whether evaluating a value ran nothing: it is data, a function, or a module.
fn is_pure(v: &Val) -> bool {
    match v {
        Val::Config
        | Val::Lit(_)
        | Val::Undefined
        | Val::Func(..)
        | Val::Module(..)
        | Val::Npm(_) => true,
        Val::Obj(es) => es.iter().all(|e| is_pure(&e.val)),
        Val::Arr(items) => items.iter().all(is_pure),
        Val::Code(_) | Val::Unknown => false,
    }
}

/// The literal a value is, when it is one throughout.
fn to_lit(v: &Val) -> Option<Lit> {
    match v {
        Val::Lit(l) => Some(l.clone()),
        Val::Arr(items) => items
            .iter()
            .map(to_lit)
            .collect::<Option<Vec<_>>>()
            .map(Lit::Array),
        Val::Obj(es) => es
            .iter()
            .map(|e| to_lit(&e.val).map(|l| (e.key.clone(), l)))
            .collect::<Option<Vec<_>>>()
            .map(Lit::Object),
        _ => None,
    }
}

/// A literal as a value: objects and arrays as their parts, objects in
/// JavaScript's property order.
fn from_lit(l: Lit) -> Val {
    match l {
        Lit::Array(items) => Val::Arr(items.into_iter().map(from_lit).collect()),
        Lit::Object(pairs) => Val::Obj(object(
            pairs
                .into_iter()
                .map(|(key, v)| Entry {
                    key,
                    val: from_lit(v),
                })
                .collect(),
        )),
        l => Val::Lit(l),
    }
}

/// A property key from a value, as JavaScript makes one.
fn key_of(v: &Val) -> Option<String> {
    match v {
        Val::Lit(Lit::Str(s)) => Some(s.clone()),
        Val::Lit(Lit::Num(n)) if n.bytes().all(|c| c.is_ascii_digit()) => Some(n.clone()),
        _ => None,
    }
}

/// `spec`, imported from `from`, as a path from the site's folder; `None`
/// when it leaves the folder.
fn resolve(from: &str, spec: &str) -> Option<String> {
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for p in spec.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

impl<'o, 'l> Reader<'o, 'l> {
    fn src(&self, a: usize, b: usize) -> Src {
        Src {
            m: self.m.clone(),
            span: self.m.span(a, b),
        }
    }

    /// In js mode, a statement the JS config runs is left to it, not a TODO: `true` when
    /// `toks[a..b]` calls what the recorder applies and nothing it doesn't.
    fn left_to_js(&mut self, a: usize, b: usize) -> bool {
        if !self.out.site.js {
            return false;
        }
        let idents = || {
            self.m.toks[a.min(b)..b.min(self.m.toks.len())]
                .iter()
                .filter_map(|t| t.ident())
        };
        let runs = idents().any(|i| JS_RUNS.contains(&i) || is_shortcode(i));
        if !runs || idents().any(|i| config_method_like(i) && js_blocks(i)) {
            return false;
        }
        self.js_note(a, b);
        true
    }

    /// A statement left to the JS config: noted, and what it reads is used.
    fn js_note(&mut self, a: usize, b: usize) {
        let at = Some(self.src(a, b));
        self.out.items.push(Item::Todo {
            why: JS_RUNS_WHY.to_string(),
            link: DOCS.to_string(),
            at,
            marker: false,
        });
        self.out.js_ran = true;
        self.out.keep = true;
    }

    fn todo(&mut self, a: usize, b: usize, why: impl Into<String>, link: impl Into<String>) {
        let why = why.into();
        // A glob isn't copied on the JS config path either.
        if !why.starts_with("Brrrty copies files and folders, not globs") && self.left_to_js(a, b) {
            return;
        }
        let at = Some(self.src(a, b));
        self.out.items.push(Item::Todo {
            why,
            link: link.into(),
            at,
            marker: true,
        });
    }

    /// A statement left out because it can't change what 11ty writes.
    fn left_out(&mut self, a: usize, b: usize, why: impl Into<String>, link: impl Into<String>) {
        let at = Some(self.src(a, b));
        self.out.items.push(Item::Todo {
            why: why.into(),
            link: link.into(),
            at,
            marker: false,
        });
    }

    fn give_up(&mut self, why: impl Into<String>) {
        self.out.give_up(why);
    }

    fn read(&mut self) {
        let m = self.m.clone();
        let t = &m.toks;
        let n = t.len();
        let stmts = js::statements(t, 0, n);
        // Exactly one config export (and one `config` export).
        let mut exports = Vec::new();
        let mut config_exports = Vec::new();
        for &(a, b) in &stmts {
            if !is_export(t, a) {
                continue;
            }
            if let Some(rest) = export_start(t, a, b) {
                exports.push((a, rest, b));
            } else if let Some(rest) = config_export(t, a, b) {
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
        if let Some(&(at, _, _)) = config_exports.first() {
            let esm = t[export_at].is_ident("export");
            if t[at].is_ident("export") != esm {
                return self.give_up("it mixes `module.exports` with `export`");
            }
            // `module.exports = ...` replaces the object `config` was set on.
            if !esm && at < export_at {
                return self.give_up(
                    "it sets `module.exports.config` before `module.exports`, which replaces it",
                );
            }
        }
        // `module.exports = config` with `function config(...) {...}`.
        let mut config_name: Option<String> = None;
        if rest + 1 == export_end
            && let Some(name) = t[rest].ident()
        {
            let decls = stmts
                .iter()
                .filter(|&&(a, _)| fn_decl(t, a) == Some(name))
                .count();
            if decls != 1 || m.uses.get(name).map_or(0, |u| u.0) != 2 {
                return self.give_up(format!(
                    "the exported `{name}` isn't one function declared once, so Brrrty can't tell what runs"
                ));
            }
            config_name = Some(name.to_string());
        }
        for (a, b) in stmts {
            if self.out.whole.is_some() {
                return;
            }
            if b == a + 1 && matches!(t[a].tok, Tok::Str(_)) {
                continue; // "use strict"
            }
            if t[a].is_ident("import") {
                match import_names(t, a, b) {
                    None => self.todo(a, b, "Brrrty couldn't read this import", PORTING),
                    Some(names) => {
                        let local = self.local_module(a, b);
                        if names.is_empty() || local {
                            self.out.items.push(Item::Decl {
                                why: RUNS_ON_LOAD,
                                names: names.into_iter().map(|(n, _, k)| (n, k)).collect(),
                                at: self.src(a, b),
                                main: true,
                            });
                        }
                    }
                }
                continue;
            }
            if let Some(names) = require_names(t, a, b) {
                if self.local_module(a, b) {
                    self.out.items.push(Item::Decl {
                        why: RUNS_ON_LOAD,
                        names: names.into_iter().map(|(n, _, k)| (n, k)).collect(),
                        at: self.src(a, b),
                        main: true,
                    });
                }
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
                self.returned(&Scope::default(), rest, b);
                continue;
            }
            if let Some(name) = &config_name
                && fn_decl(t, a) == Some(name.as_str())
            {
                if !self.config_fn(a, b) {
                    self.give_up("Brrrty couldn't read the config function");
                }
                continue;
            }
            let runs = format!("this runs when 11ty loads {}; port what it does", m.file);
            match self.declared(a, b) {
                // A declaration runs nothing when its value is plain.
                Some(names) if self.plain_declaration(a, b) => {
                    let at = self.src(a, b);
                    self.out.items.push(Item::Decl {
                        why: DECLARED,
                        names,
                        at,
                        main: true,
                    });
                }
                _ => self.todo(a, b, runs, PORTING),
            }
        }
    }

    /// Whether the declaration at `toks[a..b]` runs nothing: a function or
    /// class, or `const x = ` a value that is data, a function or a module.
    fn plain_declaration(&mut self, a: usize, b: usize) -> bool {
        let m = self.m.clone();
        let t = &m.toks;
        if !matches!(t[a].ident(), Some("const" | "let" | "var")) {
            return true;
        }
        if b <= a + 3 {
            return false;
        }
        let mark = self.out.mark();
        let v = self.eval(&Scope::default(), a + 3, b);
        self.out.drop_touched(mark);
        is_pure(&v)
    }

    /// A value 11ty works out and Brrrty neither ports nor quotes, at
    /// `toks[a..b]`: `Ok` when working it out runs nothing and doesn't use the
    /// config object; `Err(true)` when it uses the config object, `Err(false)`
    /// when it runs code.
    fn side_free(&mut self, scope: &Scope, a: usize, b: usize) -> Result<Val, bool> {
        if self.mentions_config(scope, a, b) {
            return Err(true);
        }
        let v = self.eval(scope, a, b);
        if is_pure(&v) { Ok(v) } else { Err(false) }
    }

    /// Give up on the file: code Brrrty doesn't port, at `toks[a..b]`, uses
    /// the config object.
    fn config_escapes(&mut self, scope: &Scope, a: usize, b: usize) {
        let p = self
            .config_named(scope, a, b)
            .unwrap_or_else(|| "the config object".to_string());
        let at = self.stops_at(a, b);
        self.give_up(format!(
            "code Brrrty can't port{at} uses `{p}`, and could change any setting, so nothing here is certain"
        ));
    }

    /// ` at file, line N (`code`)` for the tokens `toks[a..b]`, or nothing.
    fn stops_at(&self, a: usize, b: usize) -> String {
        let (s, e) = self.m.span(a, b);
        if s >= e || e > self.m.src.len() {
            return String::new();
        }
        let text = self.m.src[s..e].lines().next().unwrap_or("").trim();
        let text = text.trim_end_matches(';').trim_end();
        let text = match text.char_indices().nth(80) {
            Some((i, _)) => format!("{}...", &text[..i]),
            None => text.to_string(),
        };
        format!(" at {}, line {} (`{text}`)", self.m.file, self.m.line_of(s))
    }

    /// The names a top-level declaration at `toks[a..b]` declares, with the
    /// token of each: `function f`, `const x = ...` or `class C`.
    fn declared(&self, a: usize, b: usize) -> Option<Vec<(String, usize)>> {
        let t = &self.m.toks;
        let k = if t[a].is_ident("async") { a + 1 } else { a };
        if k >= b {
            return None;
        }
        if t[k].is_ident("function") || t[k].is_ident("class") {
            let n = t.get(k + 1)?.ident()?;
            return Some(vec![(n.to_string(), k + 1)]);
        }
        if matches!(t[a].ident(), Some("const" | "let" | "var")) && b > a + 3 && t[a + 2].is("=") {
            let n = t[a + 1].ident()?;
            return Some(vec![(n.to_string(), a + 1)]);
        }
        None
    }

    /// Whether the import or `require` at `toks[a..b]` loads one of the site's own files
    /// (a path starting `.` or `/`).
    fn local_module(&self, a: usize, b: usize) -> bool {
        self.m.toks[a..b].iter().any(
            |t| matches!(&t.tok, Tok::Str(Some(m)) if m.starts_with('.') || m.starts_with('/')),
        )
    }

    /// Whether `toks[a..b]` can return or throw at its own level: outside any
    /// function it defines.
    fn may_stop(&self, a: usize, b: usize) -> bool {
        let t = &self.m.toks;
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

    /// A config function in `toks[a..b]`; ports its body, then the plugins it adds.
    fn config_fn(&mut self, a: usize, b: usize) -> bool {
        let m = self.m.clone();
        let Some(def) = fn_at(&m.toks, a, b) else {
            return false;
        };
        if def.expr {
            return false;
        }
        match def.params.as_slice() {
            [] | [Param::Name(_, None)] => {}
            _ => return false,
        }
        self.out.deferred = Some(Vec::new());
        self.run_fn(&def, vec![Val::Config], Kind::Config, None, None);
        let deferred = self.out.deferred.take().unwrap_or_default();
        // 11ty runs the plugins the config added once it returns.
        self.out.in_plugin = true;
        for ((pm, def, env), options) in deferred {
            if self.out.whole.is_some() {
                break;
            }
            self.follow(pm, def, env, vec![Val::Config, options], Kind::Plugin, None);
        }
        self.out.in_plugin = false;
        true
    }

    /// Run the function `def` (of this reader's module) with `args`.
    fn run_fn(
        &mut self,
        def: &FnDef,
        args: Vec<Val>,
        kind: Kind,
        env: Option<Rc<Scope>>,
        in_loop: Option<Src>,
    ) {
        let m = self.m.clone();
        let t = &m.toks;
        let mut scope = env.map(|e| (*e).clone()).unwrap_or_default();
        if in_loop.is_some() {
            scope.in_loop = in_loop;
        }
        let mut configs = Vec::new();
        for (i, p) in def.params.iter().enumerate() {
            let arg = args.get(i).cloned().unwrap_or(Val::Undefined);
            match p {
                Param::Name(n, default) => {
                    let v = match (&arg, default) {
                        (Val::Undefined, Some((x, y))) => {
                            let mark = self.out.mark();
                            let v = self.side_free(&scope, *x, *y);
                            self.out.drop_touched(mark);
                            match v {
                                Ok(v) => v,
                                Err(true) => return self.config_escapes(&scope, *x, *y),
                                Err(false) => {
                                    self.todo(
                                        *x,
                                        *y,
                                        "this default value runs code when the function is called without it; port what it does",
                                        PORTING,
                                    );
                                    Val::Unknown
                                }
                            }
                        }
                        _ => arg,
                    };
                    let v = match v {
                        Val::Config => {
                            configs.push(n.clone());
                            v
                        }
                        // A parameter the body changes is unknown.
                        _ if !m.stable(n, def.body) => Val::Unknown,
                        v => v,
                    };
                    scope.vars.insert(n.clone(), v);
                }
                Param::Array(names) => {
                    let items = match &arg {
                        Val::Arr(items) => items.clone(),
                        _ => Vec::new(),
                    };
                    for (j, n) in names.iter().enumerate() {
                        if let Some(n) = n {
                            let v = match items.get(j) {
                                Some(v) if m.stable(n, def.body) => v.clone(),
                                _ => Val::Unknown,
                            };
                            scope.vars.insert(n.clone(), v);
                        }
                    }
                }
                Param::Other(names) => {
                    for n in names {
                        scope.vars.insert(n.clone(), Val::Unknown);
                    }
                }
            }
        }
        // The parameter must stay the config object.
        for p in &configs {
            for j in def.body.0..def.body.1 {
                if t[j].is_ident(p)
                    && (j == 0 || !(t[j - 1].is(".") || t[j - 1].is("?.")))
                    && t.get(j + 1)
                        .is_some_and(|n| matches!(n.tok, Tok::Punct("=" | "||=" | "&&=" | "??=")))
                {
                    let place = if kind == Kind::Config {
                        "the config function"
                    } else {
                        "a function the config runs"
                    };
                    return self.give_up(format!("`{p}` is assigned in {place}"));
                }
            }
        }
        let stmts = if def.expr {
            vec![def.body]
        } else {
            js::statements(t, def.body.0, def.body.1)
        };
        self.run(&mut scope, &stmts, def.body.1, kind, false);
    }

    /// Follow the function `def` of module `fm`.
    fn follow(
        &mut self,
        fm: Rc<Module>,
        def: Rc<FnDef>,
        env: Option<Rc<Scope>>,
        args: Vec<Val>,
        kind: Kind,
        in_loop: Option<Src>,
    ) {
        if self.out.depth >= MAX_FOLLOW {
            return self.give_up(format!(
                "its helpers and plugins call each other more than {MAX_FOLLOW} deep"
            ));
        }
        self.out.depth += 1;
        let mut child = Reader {
            m: fm,
            out: &mut *self.out,
        };
        child.run_fn(&def, args, kind, env, in_loop);
        self.out.depth -= 1;
    }

    /// Port the statements `stmts` of a function body that ends at `end`.
    /// `resumed`: the first statement is known to end the function early.
    fn run(
        &mut self,
        scope: &mut Scope,
        stmts: &[(usize, usize)],
        end: usize,
        kind: Kind,
        resumed: bool,
    ) {
        let m = self.m.clone();
        let t = &m.toks;
        // `return` and a line break returns `undefined`: what follows is not the config.
        for &(x, y) in stmts {
            if t[x].is_ident("return")
                && y == x + 1
                && t.get(y).is_some_and(|n| !n.is(";"))
                && y < end
            {
                return self.give_up("`return` is followed by a line break, so JavaScript returns `undefined` there and what follows never runs");
            }
        }
        let saved_end = scope.body_end;
        scope.body_end = end;
        let mut stopped: Option<&'static str> = None;
        for (i, &(x, y)) in stmts.iter().enumerate() {
            if self.out.whole.is_some() {
                break;
            }
            if let Some(why) = stopped {
                self.todo(x, y, why, PORTING);
                continue;
            }
            let returns = t[x].is_ident("return");
            if !returns && !(resumed && i == 0) && self.may_stop(x, y) {
                if kind == Kind::Config {
                    self.give_up("code in the config function can return or throw early, so Brrrty can't tell which settings 11ty ends up with");
                    break;
                }
                // What follows may not run: it is one TODO with this statement.
                let last = stmts.last().map_or(y, |s| s.1);
                let rest = stmts[i..].to_vec();
                self.conditional(
                    scope,
                    x,
                    last,
                    "Brrrty can't tell whether 11ty runs this: the code at its start can end the function early".to_string(),
                    |r, sc| r.run(sc, &rest, end, kind, true),
                );
                break;
            }
            let handled = self.statement(scope, x, y, kind);
            if self.out.whole.is_some() {
                break;
            }
            if returns {
                stopped = Some("11ty never runs this: it comes after `return`");
                continue;
            }
            if handled {
                continue;
            }
            // Code Brrrty doesn't port could change any setting through the config object.
            if let Some(p) = self.config_named(scope, x, y)
                && !self.inert(scope, x, y)
            {
                let at = self.stops_at(x, y);
                self.give_up(format!(
                    "code Brrrty can't port{at} uses `{p}`, and could change any setting, so nothing here is certain"
                ));
                break;
            }
        }
        scope.body_end = saved_end;
    }

    /// Port `toks[a..b]` as one TODO that quotes it whole: what `f` finds
    /// in it is only checked, as 11ty may or may not run it.
    fn conditional(
        &mut self,
        scope: &Scope,
        a: usize,
        b: usize,
        why: String,
        f: impl FnOnce(&mut Self, &mut Scope),
    ) {
        let items = self.out.items.len();
        let settings = self.out.settings.len();
        let mark = self.out.mark();
        self.out.cond += 1;
        let mut inner = scope.clone();
        f(self, &mut inner);
        self.out.cond -= 1;
        self.out.drop_touched(mark);
        if self.out.whole.is_some() {
            return;
        }
        if self.out.settings.len() > settings {
            self.out.settings.truncate(settings);
            return self.give_up(format!(
                "{}; it sets a folder or template engine there, so Brrrty can't tell which ones 11ty uses",
                why.split(';').next().unwrap_or(&why)
            ));
        }
        self.out.items.truncate(items);
        self.todo(a, b, why, PORTING);
    }

    /// The first name in `toks[a..b]` that is the config object.
    fn config_named(&self, scope: &Scope, a: usize, b: usize) -> Option<String> {
        let t = &self.m.toks;
        let is_config = |n: &str| matches!(scope.vars.get(n), Some(Val::Config));
        for k in a..b {
            if let Some(n) = t[k].ident()
                && !(k > 0 && (t[k - 1].is(".") || t[k - 1].is("?.")))
                && is_config(n)
            {
                return Some(n.to_string());
            }
            if let Some(n) = t[k].inner.iter().find(|n| is_config(n)) {
                return Some(n.clone());
            }
        }
        None
    }

    fn mentions_config(&self, scope: &Scope, a: usize, b: usize) -> bool {
        self.config_named(scope, a, b).is_some()
    }

    /// A call `p.method(...)` that can't change a folder or template engine:
    /// `p` isn't in its arguments, and it isn't a `set*Directory` call whose
    /// folder Brrrty couldn't read.
    fn inert(&mut self, scope: &Scope, a: usize, b: usize) -> bool {
        let m = self.m.clone();
        let t = &m.toks;
        let start = if t[a].is_ident("await") { a + 1 } else { a };
        if b < start + 4
            || !t[start]
                .ident()
                .is_some_and(|n| matches!(scope.vars.get(n), Some(Val::Config)))
            || !t[start + 1].is(".")
        {
            return false;
        }
        // `config.htmlTransformer.method(...)` only registers posthtml
        // plugins; it can't change a folder or template engine.
        if b > start + 6
            && t[start + 2].is_ident("htmlTransformer")
            && t[start + 3].is(".")
            && t[start + 4].ident().is_some()
            && t[start + 5].is("(")
            && js::matching(t, start + 5) == Some(b - 1)
        {
            return !self.mentions_config(scope, start + 6, b - 1);
        }
        if !t[start + 3].is("(") || js::matching(t, start + 3) != Some(b - 1) {
            return false;
        }
        let Some(method) = t[start + 2].ident() else {
            return false;
        };
        if self.mentions_config(scope, start + 4, b - 1) {
            return false;
        }
        // A plugin's options with `immediate` run it outside plugin mode,
        // where it can change any setting. Only absent options, or a plain
        // object without that key, are certain.
        if method == "addPlugin" {
            let args = js::split_commas(t, start + 4, b - 1);
            return match args.as_slice() {
                [_] => true,
                [_, (x, y)] => match js::object_props(t, *x, *y) {
                    Some(props) => props.iter().all(|p| p.key != "immediate"),
                    // An identifier bound to a pure object works the same way.
                    None => {
                        let mark = self.out.mark();
                        let r = self.side_free(scope, *x, *y);
                        self.out.drop_touched(mark);
                        matches!(r, Ok(Val::Obj(es)) if es.iter().all(|e| e.key != "immediate"))
                    }
                },
                _ => false,
            };
        }
        if method.ends_with("Directory") {
            let args = js::split_commas(t, start + 4, b - 1);
            return args.len() == 1 && matches!(self.lit(scope, args[0]), Some(Lit::Str(_)));
        }
        true
    }

    fn statement(&mut self, scope: &mut Scope, a: usize, b: usize, kind: Kind) -> bool {
        self.out.steps += 1;
        if self.out.steps > MAX_STEPS {
            self.give_up("there is more code to follow than Brrrty reads");
            return true;
        }
        let m = self.m.clone();
        let t = &m.toks;
        if t[a].is_ident("return") {
            if b == a + 1 {
                return true;
            }
            if kind == Kind::Config {
                self.returned(scope, a + 1, b);
                return true;
            }
            // What a helper or plugin returns isn't read; only what it does counts.
            if self.mentions_config(scope, a + 1, b) {
                return self.statement(scope, a + 1, b, kind);
            }
            let mark = self.out.mark();
            let v = self.eval(scope, a + 1, b);
            self.out.drop_touched(mark);
            if !is_pure(&v) {
                self.todo(a, b, "port this code", PORTING);
                return false;
            }
            return true;
        }
        if t[a].is_ident("if") && self.mentions_config(scope, a, b) {
            return self.if_statement(scope, a, b, kind);
        }
        if t[a].is_ident("for") && self.mentions_config(scope, a, b) {
            return self.for_statement(scope, a, b, kind);
        }
        if matches!(t[a].ident(), Some("const" | "let" | "var")) {
            return self.declare(scope, a, b);
        }
        let start = if t[a].is_ident("await") && b > a + 1 {
            a + 1
        } else {
            a
        };
        // `p.name(...)` or `p.a.b(...)`.
        if t[start]
            .ident()
            .is_some_and(|n| matches!(scope.vars.get(n), Some(Val::Config)))
        {
            let mut names = Vec::new();
            let mut k = start + 1;
            while k + 1 < b && t[k].is(".") && t[k + 1].ident().is_some() {
                names.push(t[k + 1].ident().unwrap_or_default().to_string());
                k += 2;
            }
            if !names.is_empty() && k < b && t[k].is("(") && js::matching(t, k) == Some(b - 1) {
                let args = js::split_commas(t, k + 1, b - 1);
                let method = names.join(".");
                let mark = self.out.mark();
                self.out.keep = false;
                let ported = self.call(scope, &method, &args, a, b);
                if ported || self.out.keep {
                    self.out.commit(mark);
                } else {
                    self.out.drop_touched(mark);
                }
                return ported;
            }
        }
        if self.mentions_config(scope, start, b) {
            if let Some(r) = self.for_each(scope, start, b, kind) {
                return r;
            }
            if let Some(r) = self.helper_call(scope, start, b) {
                return r;
            }
        }
        self.todo(a, b, "port this code", PORTING);
        false
    }

    /// `if (...) ... else ...` that uses the config object: one TODO.
    fn if_statement(&mut self, scope: &mut Scope, a: usize, b: usize, kind: Kind) -> bool {
        let m = self.m.clone();
        let t = &m.toks;
        let cond = match (t.get(a + 1), js::matching(t, a + 1)) {
            (Some(o), Some(c)) if o.is("(") && c > a + 2 => {
                let (x, y) = m.span(a + 2, c);
                let text = one_line(&m.src[x..y]);
                let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if text.chars().count() > 80 {
                    format!("{}...", text.chars().take(77).collect::<String>())
                } else {
                    text
                }
            }
            _ => {
                self.todo(a, b, "port this code", PORTING);
                return false;
            }
        };
        let why = format!(
            "11ty runs this only when `{cond}`, which Brrrty can't know; port what it does for the builds you compare, or delete it"
        );
        self.conditional(scope, a, b, why, |r, sc| r.branches(sc, a, b, kind));
        true
    }

    /// Run every branch of the `if` statement at `toks[a..b]`.
    fn branches(&mut self, scope: &mut Scope, a: usize, b: usize, kind: Kind) {
        let m = self.m.clone();
        let t = &m.toks;
        // The statement that starts at `s`, as a body: a block, or one
        // statement up to an `else`.
        let body = |s: usize| -> Option<((usize, usize), usize)> {
            if s >= b {
                return None;
            }
            if t[s].is("{") {
                let close = js::matching(t, s)?;
                return Some(((s + 1, close), close + 1));
            }
            let mut k = s;
            while k < b {
                if t[k].is_ident("else") {
                    break;
                }
                if let Some(p) = t[k].pair
                    && p > k
                {
                    k = p + 1;
                    continue;
                }
                k += 1;
            }
            Some(((s, k), k))
        };
        let mut k = a;
        loop {
            if !t[k].is_ident("if") {
                break;
            }
            let Some(close) = js::matching(t, k + 1) else {
                break;
            };
            let Some(((x, y), next)) = body(close + 1) else {
                break;
            };
            let stmts = js::statements(t, x, y);
            self.run(&mut scope.clone(), &stmts, y, kind, false);
            if next >= b || !t[next].is_ident("else") {
                break;
            }
            let s = next + 1;
            if t.get(s).is_some_and(|n| n.is_ident("if")) {
                k = s;
                continue;
            }
            if let Some(((x, y), _)) = body(s) {
                let stmts = js::statements(t, x, y);
                self.run(&mut scope.clone(), &stmts, y, kind, false);
            }
            break;
        }
    }

    /// `for (const x of ...)` or `for (const k in ...)` that uses the config.
    fn for_statement(&mut self, scope: &mut Scope, a: usize, b: usize, kind: Kind) -> bool {
        let m = self.m.clone();
        let t = &m.toks;
        let loop_why = "this loop goes over something Brrrty can't read, so it can't tell what the loop adds; port what it does";
        let parsed = (|| {
            let open = a + 1;
            if !t.get(open)?.is("(") {
                return None;
            }
            let close = js::matching(t, open)?;
            if !matches!(t.get(open + 1)?.ident(), Some("const" | "let" | "var")) {
                return None;
            }
            let pat = open + 2;
            let pat_end = if t.get(pat)?.is("[") {
                js::matching(t, pat)? + 1
            } else {
                t.get(pat)?.ident()?;
                pat + 1
            };
            let of = match t.get(pat_end)?.ident()? {
                "of" => true,
                "in" => false,
                _ => return None,
            };
            let body = if t.get(close + 1)?.is("{") && js::matching(t, close + 1)? == b - 1 {
                (close + 2, b - 1)
            } else {
                (close + 1, b)
            };
            Some((pat, pat_end, of, (pat_end + 1, close), body))
        })();
        let Some((pat, pat_end, of, source, body)) = parsed else {
            // Another kind of loop: its body is checked, as it may run.
            let body = (|| {
                let close = js::matching(t, a + 1)?;
                if t.get(close + 1)?.is("{") && js::matching(t, close + 1)? == b - 1 {
                    Some(((a + 2, close), (close + 2, b - 1)))
                } else {
                    Some(((a + 2, close), (close + 1, b)))
                }
            })();
            self.conditional(scope, a, b, loop_why.to_string(), |r, sc| {
                let Some(((hx, hy), (x, y))) = body else {
                    return;
                };
                // The names the loop's head declares are unknown in its body.
                for k in hx..hy {
                    if let Some(n) = t[k].ident()
                        && !(k > 0 && t[k - 1].is("."))
                        && !matches!(sc.vars.get(n), Some(Val::Config))
                    {
                        sc.vars.insert(n.to_string(), Val::Unknown);
                    }
                }
                let stmts = js::statements(t, x, y);
                r.run(sc, &stmts, y, kind, false);
            });
            return true;
        };
        let params = params(t, pat, pat_end);
        if self.mentions_config(scope, source.0, source.1) {
            self.config_escapes(scope, source.0, source.1);
            return true;
        }
        let mark = self.out.mark();
        let items = if of {
            self.iterate(scope, source.0, source.1)
        } else {
            let v = self.eval(scope, source.0, source.1);
            if is_pure(&v) {
                self.entries_of(&v).map(|es| {
                    es.into_iter()
                        .map(|e| Val::Lit(Lit::Str(e.key)))
                        .collect::<Vec<_>>()
                })
            } else {
                None
            }
        };
        let stmts = js::statements(t, body.0, body.1);
        let def = FnDef {
            params,
            body,
            expr: false,
            at: (a, b),
        };
        match items {
            Some(items) => {
                self.out.commit(mark);
                let at = self.src(a, b);
                for item in items {
                    if self.out.whole.is_some() {
                        break;
                    }
                    let mut sc = scope.clone();
                    sc.in_loop = Some(at.clone());
                    self.bind(&mut sc, &def.params, vec![item], body);
                    self.run(&mut sc, &stmts, body.1, kind, false);
                }
            }
            None => {
                self.out.drop_touched(mark);
                self.conditional(scope, a, b, loop_why.to_string(), |r, sc| {
                    r.bind(sc, &def.params, Vec::new(), body);
                    r.run(sc, &stmts, body.1, kind, false);
                });
            }
        }
        true
    }

    /// Bind `params` to `args` in `scope`, for code in `toks[range]`: a name
    /// that code changes is unknown.
    fn bind(&mut self, scope: &mut Scope, params: &[Param], args: Vec<Val>, range: (usize, usize)) {
        let m = self.m.clone();
        for (i, p) in params.iter().enumerate() {
            let arg = args.get(i).cloned().unwrap_or(Val::Undefined);
            match p {
                Param::Name(n, _) => {
                    let v = if m.stable(n, range) {
                        arg
                    } else {
                        Val::Unknown
                    };
                    scope.vars.insert(n.clone(), v);
                }
                Param::Array(names) => {
                    let items = match &arg {
                        Val::Arr(items) => items.clone(),
                        _ => Vec::new(),
                    };
                    for (j, n) in names.iter().enumerate() {
                        if let Some(n) = n {
                            let v = match items.get(j) {
                                Some(v) if m.stable(n, range) => v.clone(),
                                _ => Val::Unknown,
                            };
                            scope.vars.insert(n.clone(), v);
                        }
                    }
                }
                Param::Other(names) => {
                    for n in names {
                        scope.vars.insert(n.clone(), Val::Unknown);
                    }
                }
            }
        }
    }

    /// The values `toks[a..b]` goes over: an array, or `Object.keys(...)`,
    /// `Object.values(...)` or `Object.entries(...)` of an object or module.
    fn iterate(&mut self, scope: &Scope, a: usize, b: usize) -> Option<Vec<Val>> {
        let items = self.iterate_inner(scope, a, b)?;
        // Working the list out must run nothing.
        items.iter().all(is_pure).then_some(items)
    }

    fn iterate_inner(&mut self, scope: &Scope, a: usize, b: usize) -> Option<Vec<Val>> {
        let m = self.m.clone();
        let t = &m.toks;
        if b > a + 4
            && t[a].is_ident("Object")
            && !scope.vars.contains_key("Object")
            && !m.decls.contains_key("Object")
            && t[a + 1].is(".")
            && t[a + 3].is("(")
            && js::matching(t, a + 3) == Some(b - 1)
        {
            let method = t[a + 2].ident()?;
            let v = self.eval(scope, a + 4, b - 1);
            // Working the object out must run nothing, even for its keys alone.
            if !is_pure(&v) {
                return None;
            }
            let es = self.entries_of(&v)?;
            return match method {
                "keys" => Some(es.into_iter().map(|e| Val::Lit(Lit::Str(e.key))).collect()),
                "values" => Some(es.into_iter().map(|e| e.val).collect()),
                "entries" => Some(
                    es.into_iter()
                        .map(|e| Val::Arr(vec![Val::Lit(Lit::Str(e.key)), e.val]))
                        .collect(),
                ),
                _ => None,
            };
        }
        match self.eval(scope, a, b) {
            Val::Arr(items) => Some(items),
            _ => None,
        }
    }

    /// The own properties of an object or module.
    fn entries_of(&mut self, v: &Val) -> Option<Vec<Entry>> {
        match v {
            Val::Obj(es) => Some(es.clone()),
            Val::Module(m, how) => self.module_entries(m.clone(), *how),
            _ => None,
        }
    }

    /// `X.forEach(fn)`.
    fn for_each(&mut self, scope: &mut Scope, a: usize, b: usize, kind: Kind) -> Option<bool> {
        let m = self.m.clone();
        let t = &m.toks;
        if !t[b - 1].is(")") {
            return None;
        }
        let open = t[b - 1].pair?;
        if open < a + 3 || !t[open - 1].is_ident("forEach") || !t[open - 2].is(".") {
            return None;
        }
        let args = js::split_commas(t, open + 1, b - 1);
        if args.len() != 1 {
            return None;
        }
        if self.mentions_config(scope, a, open - 2) {
            self.config_escapes(scope, a, open - 2);
            return Some(true);
        }
        let mark = self.out.mark();
        let items = self.iterate(scope, a, open - 2);
        let f = self.eval(scope, args[0].0, args[0].1);
        let Val::Func(fm, def, env) = f else {
            self.out.drop_touched(mark);
            return None;
        };
        match items {
            Some(items) => {
                self.out.commit(mark);
                let at = self.src(a, b);
                let all = Val::Arr(items.clone());
                for (i, item) in items.into_iter().enumerate() {
                    if self.out.whole.is_some() {
                        break;
                    }
                    let args = vec![item, Val::Lit(Lit::Num(i.to_string())), all.clone()];
                    self.follow(
                        fm.clone(),
                        def.clone(),
                        env.clone(),
                        args,
                        kind_of(kind),
                        Some(at.clone()),
                    );
                }
            }
            None => {
                self.out.drop_touched(mark);
                let why = "this loop goes over something Brrrty can't read, so it can't tell what the loop adds; port what it does".to_string();
                self.conditional(scope, a, b, why, |r, _| {
                    r.follow(fm, def, env, Vec::new(), kind_of(kind), None);
                });
            }
        }
        Some(true)
    }

    /// `helper(config, ...)`: follow the helper, when it is a function Brrrty can read.
    fn helper_call(&mut self, scope: &mut Scope, a: usize, b: usize) -> Option<bool> {
        let m = self.m.clone();
        let t = &m.toks;
        if !t[b - 1].is(")") {
            return None;
        }
        let open = t[b - 1].pair?;
        if open <= a {
            return None;
        }
        // The callee: a name, names joined by `.`, or `require("./x")`.
        let names = (a..open).all(|k| {
            if (k - a).is_multiple_of(2) {
                t[k].ident().is_some()
            } else {
                t[k].is(".")
            }
        }) && (open - a) % 2 == 1;
        if !names && required(t, a, open).is_none() {
            return None;
        }
        let mark = self.out.mark();
        let callee = self.eval(scope, a, open);
        let Some((fm, def, env)) = self.callable(callee) else {
            self.out.drop_touched(mark);
            return None;
        };
        let args_mark = self.out.mark();
        let mut args = Vec::new();
        let mut runs_code = false;
        for (x, y) in js::split_commas(t, open + 1, b - 1) {
            // The config object itself is what the helper is followed with.
            let bare = y == x + 1
                && t[x]
                    .ident()
                    .is_some_and(|n| matches!(scope.vars.get(n), Some(Val::Config)));
            if bare {
                args.push(Val::Config);
                continue;
            }
            match self.side_free(scope, x, y) {
                Ok(v) => args.push(v),
                Err(true) => {
                    self.out.drop_touched(mark);
                    self.config_escapes(scope, x, y);
                    return Some(true);
                }
                Err(false) => {
                    runs_code = true;
                    args.push(Val::Unknown);
                }
            }
        }
        self.out.drop_touched(args_mark);
        self.out.commit(mark);
        if runs_code {
            self.todo(a, b, ARGUMENTS, PORTING);
        }
        self.follow(fm, def, env, args, Kind::Helper, None);
        Some(true)
    }

    /// The function a value calls, when Brrrty can follow it.
    fn callable(&mut self, v: Val) -> Option<Callee> {
        match v {
            Val::Func(m, def, env) => Some((m, def, env)),
            Val::Module(m, how) => match self.module_value(m, how) {
                Val::Func(m, def, env) => Some((m, def, env)),
                _ => None,
            },
            _ => None,
        }
    }

    /// `const x = ...` in a function.
    fn declare(&mut self, scope: &mut Scope, a: usize, b: usize) -> bool {
        let m = self.m.clone();
        let t = &m.toks;
        let kw = t[a].ident().unwrap_or("const");
        if b > a + 3
            && let Some(n) = t[a + 1].ident()
            && t[a + 2].is("=")
            && js::split_commas(t, a + 3, b).len() == 1
        {
            let mark = self.out.mark();
            let v = self.eval(scope, a + 3, b);
            self.out.drop_touched(mark);
            if kw == "const" && matches!(v, Val::Config) {
                scope.vars.insert(n.to_string(), Val::Config);
                return true;
            }
            if kw == "const" && is_pure(&v) && m.stable(n, (b, scope.body_end.max(b))) {
                scope.vars.insert(n.to_string(), v);
                let at = self.src(a, b);
                self.out.items.push(Item::Local {
                    name: n.to_string(),
                    decl: a + 1,
                    range: (b, scope.body_end.max(b)),
                    at,
                });
                return true;
            }
            let v = if kw == "const" {
                Val::Code(self.src(a + 3, b))
            } else {
                Val::Unknown
            };
            scope.vars.insert(n.to_string(), v);
        } else {
            let eq = (a..b).find(|&k| t[k].is("=")).unwrap_or(b);
            for k in a + 1..eq {
                if let Some(n) = t[k].ident()
                    && !t[k - 1].is(":")
                {
                    scope.vars.insert(n.to_string(), Val::Unknown);
                }
            }
        }
        self.todo(a, b, "port this code", PORTING);
        false
    }

    fn lit(&mut self, scope: &Scope, (a, b): (usize, usize)) -> Option<Lit> {
        if let Some(l) = js::literal(&self.m.toks, a, b) {
            return Some(l);
        }
        let v = self.eval(scope, a, b);
        to_lit(&v)
    }

    /// What `toks[a..b]` is.
    fn eval(&mut self, scope: &Scope, a: usize, b: usize) -> Val {
        if self.out.evals >= MAX_EVAL {
            return Val::Unknown;
        }
        self.out.evals += 1;
        let v = self.eval_inner(scope, a, b);
        self.out.evals -= 1;
        v
    }

    fn eval_inner(&mut self, scope: &Scope, a: usize, b: usize) -> Val {
        let m = self.m.clone();
        let t = &m.toks;
        if a >= b || b > t.len() {
            return Val::Unknown;
        }
        if t[a].is("(") && js::matching(t, a) == Some(b - 1) {
            return self.eval(scope, a + 1, b - 1);
        }
        if b == a + 1
            && let Some(n) = t[a].ident()
            && !matches!(n, "true" | "false" | "null")
        {
            return self.name(scope, n, a);
        }
        if let Some(l) = js::literal(t, a, b) {
            return from_lit(l);
        }
        if let Some(def) = fn_at(t, a, b) {
            return Val::Func(m.clone(), Rc::new(def), Some(Rc::new(scope.clone())));
        }
        if t[a].is("{") && js::matching(t, a) == Some(b - 1) {
            let Some(raw) = raw_entries(t, a, b) else {
                return Val::Code(self.src(a, b));
            };
            let mut entries = Vec::new();
            for e in raw {
                entries.push(match e {
                    RawEntry::Value(key, (x, y)) => Entry {
                        key,
                        val: self.eval(scope, x, y),
                    },
                    RawEntry::Short(key, k) => Entry {
                        val: self.name(scope, &key, k),
                        key,
                    },
                    RawEntry::Method(key, def) => Entry {
                        key,
                        val: Val::Func(m.clone(), Rc::new(def), Some(Rc::new(scope.clone()))),
                    },
                });
            }
            // A repeated key keeps the last value: an earlier one must run nothing.
            let dropped = entries.iter().enumerate().any(|(i, e)| {
                !is_pure(&e.val) && entries[i + 1..].iter().any(|later| later.key == e.key)
            });
            if dropped {
                return Val::Code(self.src(a, b));
            }
            return Val::Obj(object(entries));
        }
        if t[a].is("[") && js::matching(t, a) == Some(b - 1) {
            let mut items = Vec::new();
            for (x, y) in js::split_commas(t, a + 1, b - 1) {
                if t[x].is("...") {
                    return Val::Code(self.src(a, b));
                }
                items.push(self.eval(scope, x, y));
            }
            return Val::Arr(items);
        }
        if let Some(spec) = required(t, a, b) {
            return self.module_val(&spec, How::Require);
        }
        // `x.name`.
        if b >= a + 3
            && (t[b - 2].is(".") || t[b - 2].is("?."))
            && let Some(key) = t[b - 1].ident()
        {
            let base = self.eval(scope, a, b - 2);
            return self.member(base, key);
        }
        // `x[key]`.
        if t[b - 1].is("]")
            && let Some(open) = t[b - 1].pair
            && open > a
        {
            let base = self.eval(scope, a, open);
            let key = self.eval(scope, open + 1, b - 1);
            if let Some(k) = key_of(&key) {
                return self.member(base, &k);
            }
        }
        Val::Code(self.src(a, b))
    }

    /// What the name `n`, at token `k`, stands for here.
    fn name(&mut self, scope: &Scope, n: &str, k: usize) -> Val {
        let m = self.m.clone();
        if let Some(v) = scope.vars.get(n) {
            self.out.touched.push((m, k));
            return v.clone();
        }
        if n == "undefined" {
            return Val::Undefined;
        }
        if let Some(&d) = m.decls.get(n) {
            if !m.stable_everywhere(n) {
                return Val::Unknown;
            }
            self.out.touched.push((m.clone(), k));
            return match d {
                Decl::Func(x, y) => match fn_at(&m.toks, x, y) {
                    Some(def) => Val::Func(m.clone(), Rc::new(def), None),
                    None => Val::Code(self.src(x, y)),
                },
                Decl::Const(x, y) => self.eval(&Scope::default(), x, y),
                Decl::Unknown => Val::Unknown,
            };
        }
        if let Some(binding) = m.bindings.get(n) {
            if m.mutable.contains(n) || !m.stable_everywhere(n) {
                return Val::Unknown;
            }
            self.out.touched.push((m.clone(), k));
            return match binding.clone() {
                Binding::Module(spec, how) if is_local(&spec) => self.module_val(&spec, how),
                Binding::Export(spec, name) if is_local(&spec) => {
                    let v = self.module_val(&spec, How::Require);
                    self.member(v, &name)
                }
                b => Val::Npm(b),
            };
        }
        Val::Code(self.src(k, k + 1))
    }

    /// A property of a value.
    fn member(&mut self, base: Val, key: &str) -> Val {
        // Taking one part of a list or object drops the rest: the rest must
        // run nothing. (A module's exports run when it loads, which `load_top`
        // marks.)
        if matches!(base, Val::Obj(_) | Val::Arr(_)) && !is_pure(&base) {
            return Val::Unknown;
        }
        match base {
            Val::Obj(es) => es
                .into_iter()
                .find(|e| e.key == key)
                .map_or(Val::Undefined, |e| e.val),
            Val::Module(m, how) => match self.module_entries(m, how) {
                Some(es) => es
                    .into_iter()
                    .find(|e| e.key == key)
                    .map_or(Val::Undefined, |e| e.val),
                None => Val::Unknown,
            },
            Val::Npm(Binding::Module(spec, _)) => Val::Npm(Binding::Export(spec, key.to_string())),
            Val::Arr(items) => key
                .parse::<usize>()
                .ok()
                .and_then(|i| items.get(i).cloned())
                .unwrap_or(Val::Unknown),
            _ => Val::Unknown,
        }
    }

    /// The site's module `spec`, as `how` brings it in.
    fn module_val(&mut self, spec: &str, how: How) -> Val {
        match self.load(spec) {
            // In a cycle, one of the modules gets the other's exports before
            // it has finished loading: what it sees depends on the order.
            Some(m) if self.cyclic(&m) => Val::Unknown,
            Some(m) => Val::Module(m, how),
            None => Val::Unknown,
        }
    }

    /// Whether `target` names this reader's module again, directly or
    /// through the modules it names.
    fn cyclic(&mut self, target: &Rc<Module>) -> bool {
        let me = self.m.file.clone();
        let mut seen = HashSet::new();
        let mut stack = vec![target.clone()];
        while let Some(m) = stack.pop() {
            if m.file == me {
                return true;
            }
            if !seen.insert(m.file.clone()) || seen.len() > 256 {
                continue;
            }
            for spec in m.deps.clone() {
                let mut child = Reader {
                    m: m.clone(),
                    out: &mut *self.out,
                };
                if let Some(d) = child.load(&spec) {
                    stack.push(d);
                }
            }
        }
        false
    }

    /// Read the site's module `spec`, once, noting what runs when it loads.
    fn load(&mut self, spec: &str) -> Option<Rc<Module>> {
        if !is_local(spec) {
            return None;
        }
        let load = self.out.site.load?;
        let path = resolve(&self.m.file, spec)?;
        let has_ext = [".js", ".cjs", ".mjs"].iter().any(|e| path.ends_with(e));
        let candidates = if has_ext {
            vec![path]
        } else {
            vec![format!("{path}.js"), format!("{path}/index.js")]
        };
        for cand in candidates {
            if let Some(cached) = self.out.modules.get(&cand) {
                return cached.clone();
            }
            if self.out.loading.contains(&cand) || self.out.loading.len() >= MAX_FOLLOW {
                return None;
            }
            let src = match load(&cand) {
                Ok(Some(src)) => src,
                Ok(None) => continue,
                Err(_) => {
                    self.out.modules.insert(cand, None);
                    return None;
                }
            };
            let src = src.strip_prefix('\u{feff}').unwrap_or(&src).to_string();
            let Ok(toks) = js::tokenize(&src) else {
                self.out.modules.insert(cand, None);
                return None;
            };
            let m = Rc::new(Module::new(&cand, &src, toks));
            self.out.modules.insert(cand.clone(), Some(m.clone()));
            self.out.read.push((cand.clone(), src));
            self.out.loading.push(cand);
            let mut child = Reader {
                m: m.clone(),
                out: &mut *self.out,
            };
            child.load_top();
            self.out.loading.pop();
            return Some(m);
        }
        None
    }

    /// The TODOs for a module's top-level code, which runs when it loads.
    fn load_top(&mut self) {
        let m = self.m.clone();
        let t = &m.toks;
        let runs = format!("this runs when 11ty loads {}; port what it does", m.file);
        let push = |r: &mut Self, item: Item| r.out.loaded_items.push(item);
        for (a, b) in js::statements(t, 0, t.len()) {
            if b == a + 1 && matches!(t[a].tok, Tok::Str(_)) {
                continue;
            }
            if t[a].is_ident("import") && !t.get(a + 1).is_some_and(|n| n.is("(") || n.is(".")) {
                match import_names(t, a, b) {
                    Some(names) if names.is_empty() || self.local_module(a, b) => {
                        let at = self.src(a, b);
                        push(
                            self,
                            Item::Decl {
                                why: RUNS_ON_LOAD,
                                names: names.into_iter().map(|(n, _, k)| (n, k)).collect(),
                                at,
                                main: true,
                            },
                        );
                    }
                    Some(_) => {}
                    None => {
                        let at = Some(self.src(a, b));
                        push(
                            self,
                            Item::Todo {
                                why: "Brrrty couldn't read this import".into(),
                                link: PORTING.into(),
                                at,
                                marker: true,
                            },
                        );
                    }
                }
                continue;
            }
            if let Some(names) = require_names(t, a, b) {
                if self.local_module(a, b) {
                    let at = self.src(a, b);
                    push(
                        self,
                        Item::Decl {
                            why: RUNS_ON_LOAD,
                            names: names.into_iter().map(|(n, _, k)| (n, k)).collect(),
                            at,
                            main: true,
                        },
                    );
                }
                continue;
            }
            // `export function f` and `export const x = ...` are declarations too.
            if t[a].is_ident("export") {
                let k = if t.get(a + 1).is_some_and(|n| n.is_ident("async")) {
                    a + 2
                } else {
                    a + 1
                };
                let declared = if fn_decl(t, a + 1).is_some() {
                    t.get(k + 1)
                        .and_then(Token::ident)
                        .map(|n| (n.to_string(), k + 1, None))
                } else if t.get(a + 1).is_some_and(|n| n.is_ident("const"))
                    && b > a + 4
                    && t[a + 3].is("=")
                {
                    t[a + 2]
                        .ident()
                        .map(|n| (n.to_string(), a + 2, Some((a + 4, b))))
                } else {
                    None
                };
                if let Some((name, at_tok, value)) = declared {
                    let pure = match value {
                        None => true,
                        Some((x, y)) => {
                            let mark = self.out.mark();
                            let v = self.eval(&Scope::default(), x, y);
                            self.out.drop_touched(mark);
                            is_pure(&v)
                        }
                    };
                    let at = self.src(a, b);
                    let item = if pure {
                        Item::Decl {
                            why: DECLARED,
                            names: vec![(name, at_tok)],
                            at,
                            main: false,
                        }
                    } else {
                        Item::Todo {
                            why: runs.clone(),
                            link: PORTING.into(),
                            at: Some(at),
                            marker: true,
                        }
                    };
                    push(self, item);
                    continue;
                }
            }
            // Declarations and exports run nothing when their values are plain.
            let value = if let Some(rest) = export_start(t, a, b) {
                Some((rest, b))
            } else if t[a].is_ident("export") {
                match t.get(a + 1) {
                    Some(n) if n.is("{") && js::matching(t, a + 1) == Some(b - 1) => None,
                    _ => Some((b, b)),
                }
            } else if (t[a].is_ident("module") || t[a].is_ident("exports"))
                && let Some(eq) = (a..b).find(|&k| t[k].is("="))
                && m.exports
                    .iter()
                    .any(|e| matches!(e, Export::Prop { value, .. } if value.0 == eq + 1))
            {
                Some((eq + 1, b))
            } else if let Some(names) = self.declared(a, b) {
                let pure = match t[a].ident() {
                    Some("const" | "let" | "var") => {
                        let mark = self.out.mark();
                        let v = self.eval(&Scope::default(), a + 3, b);
                        self.out.drop_touched(mark);
                        is_pure(&v)
                    }
                    _ => true,
                };
                let at = self.src(a, b);
                if pure {
                    push(
                        self,
                        Item::Decl {
                            why: DECLARED,
                            names,
                            at,
                            main: false,
                        },
                    );
                } else {
                    push(
                        self,
                        Item::Todo {
                            why: runs.clone(),
                            link: PORTING.into(),
                            at: Some(at),
                            marker: true,
                        },
                    );
                }
                continue;
            } else {
                Some((b, b))
            };
            let pure = match value {
                None => true,
                Some((x, y)) if x < y => {
                    let mark = self.out.mark();
                    let v = self.eval(&Scope::default(), x, y);
                    self.out.drop_touched(mark);
                    is_pure(&v)
                }
                Some(_) => false,
            };
            if !pure {
                let at = Some(self.src(a, b));
                push(
                    self,
                    Item::Todo {
                        why: runs.clone(),
                        link: PORTING.into(),
                        at,
                        marker: true,
                    },
                );
            }
        }
    }

    /// The value of module `m` brought in as `how`: what calling it calls.
    fn module_value(&mut self, m: Rc<Module>, how: How) -> Val {
        let mut child = Reader {
            m: m.clone(),
            out: &mut *self.out,
        };
        let s = Scope::default();
        if m.esm {
            if how != How::Default {
                return Val::Unknown;
            }
            for e in &m.exports {
                if let Export::Default(x, y) = e {
                    return child.eval(&s, *x, *y);
                }
            }
            return Val::Undefined;
        }
        let mut main = None;
        for e in &m.exports {
            if let Export::Main(x, y) = e {
                main = Some((*x, *y));
            }
        }
        match main {
            Some((x, y)) => child.eval(&s, x, y),
            None => Val::Unknown,
        }
    }

    /// The properties of module `m` brought in as `how`, in JavaScript's order.
    fn module_entries(&mut self, m: Rc<Module>, how: How) -> Option<Vec<Entry>> {
        let mut child = Reader {
            m: m.clone(),
            out: &mut *self.out,
        };
        let s = Scope::default();
        if m.esm {
            if how == How::Default {
                let v = child.module_value(m.clone(), how);
                return child.entries_of(&v);
            }
            // A namespace: every export, sorted by name.
            let mut out = Vec::new();
            for e in &m.exports {
                match e {
                    Export::Default(x, y) => out.push(Entry {
                        key: "default".into(),
                        val: child.eval(&s, *x, *y),
                    }),
                    Export::Named(name, local) => {
                        let k = m
                            .occ
                            .get(local)
                            .and_then(|v| v.first().copied())
                            .unwrap_or(0);
                        let val = child.name(&s, local, k);
                        out.push(Entry {
                            key: name.clone(),
                            val,
                        });
                    }
                    Export::Opaque => return None,
                    _ => {}
                }
            }
            out.sort_by(|a, b| a.key.encode_utf16().cmp(b.key.encode_utf16()));
            return Some(out);
        }
        // CommonJS: `module.exports`, as its statements build it.
        let mut entries: Vec<Entry> = Vec::new();
        let mut replaced = false;
        for e in &m.exports {
            match e {
                Export::Main(x, y) => {
                    let v = child.eval(&s, *x, *y);
                    entries = match v {
                        Val::Obj(es) => es,
                        Val::Func(..) => Vec::new(),
                        _ => return None,
                    };
                    replaced = true;
                }
                Export::Prop {
                    name,
                    value: (x, y),
                    exports,
                } => {
                    // `exports.x` after `module.exports = ...` sets a property
                    // on the object 11ty never sees.
                    if *exports && replaced {
                        continue;
                    }
                    let val = child.eval(&s, *x, *y);
                    entries.push(Entry {
                        key: name.clone(),
                        val,
                    });
                }
                _ => return None,
            }
        }
        Some(object(entries))
    }

    /// Where a value is written, to quote it.
    fn where_val(&self, v: &Val) -> Option<Src> {
        match v {
            Val::Func(m, def, _) => Some(Src {
                m: m.clone(),
                span: m.span(def.at.0, def.at.1),
            }),
            Val::Code(src) => Some(src.clone()),
            _ => None,
        }
    }

    /// A TODO for something a call adds: in a loop, it quotes what this turn
    /// of the loop adds, as the loop's own code is the same every turn.
    fn todo_added(
        &mut self,
        scope: &Scope,
        a: usize,
        b: usize,
        args: &[(usize, usize)],
        why: &str,
        link: String,
    ) -> bool {
        if self.left_to_js(a, b) {
            return false;
        }
        if let Some(lp) = scope.in_loop.clone()
            && self.out.cond == 0
        {
            let name = args
                .first()
                .and_then(|&r| self.lit(scope, r))
                .and_then(|l| match l {
                    Lit::Str(s) => Some(s),
                    _ => None,
                });
            // The last argument with a place to quote; every other argument
            // must run nothing, as the loop's own code isn't quoted.
            let mut target = None;
            let mut others_plain = true;
            for &(x, y) in args.iter().rev() {
                let v = self.eval(scope, x, y);
                if target.is_none()
                    && let Some(at) = self.where_val(&v)
                {
                    target = Some(at);
                    continue;
                }
                others_plain &= is_pure(&v);
            }
            if !others_plain {
                target = None;
            }
            if let (Some(name), Some(at)) = (name, target) {
                let line = lp.m.line_of(lp.span.0);
                self.out.items.push(Item::Todo {
                    why: format!(
                        "{why} (`{name}`, which the loop at {}, line {line}, adds)",
                        lp.m.file
                    ),
                    link,
                    at: Some(at),
                    marker: true,
                });
                self.out.keep = true;
                return false;
            }
        }
        self.todo(a, b, why, link);
        false
    }
}

/// The kind of function a loop's callback is: it runs as part of `kind`.
fn kind_of(kind: Kind) -> Kind {
    match kind {
        Kind::Config => Kind::Helper,
        k => k,
    }
}

impl<'o, 'l> Reader<'o, 'l> {
    /// A call on the config object. `true` when it was ported.
    fn call(
        &mut self,
        scope: &Scope,
        method: &str,
        args: &[(usize, usize)],
        a: usize,
        b: usize,
    ) -> bool {
        let filters = format!("{DOCS}#filters");
        match method {
            "addPassthroughCopy" => {
                let lit = if args.len() == 1 {
                    self.lit(scope, args[0])
                } else {
                    None
                };
                match lit {
                    Some(Lit::Str(path)) if !glob(&path) => {
                        self.out.items.push(Item::Rhai(format!(
                            "config.add_passthrough_copy({});",
                            js::rhai_string(&path)
                        )));
                        true
                    }
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
                        self.out.items.push(Item::Rhai(lines.join("\n")));
                        true
                    }
                    Some(Lit::Str(_)) | Some(Lit::Object(_)) => {
                        self.todo(
                            a,
                            b,
                            "Brrrty copies files and folders, not globs; list what this copies",
                            format!("{DOCS}#the-config-object"),
                        );
                        false
                    }
                    _ => {
                        self.todo(
                            a,
                            b,
                            "port this passthrough copy",
                            format!("{DOCS}#the-config-object"),
                        );
                        false
                    }
                }
            }
            "ignores.add" | "ignores.delete" => {
                let lit = args.first().and_then(|&r| self.lit(scope, r));
                match (args.len(), lit) {
                    (1, Some(Lit::Str(glob))) => {
                        let m = method.rsplit('.').next().unwrap_or("add");
                        self.out.items.push(Item::Rhai(format!(
                            "config.ignores.{m}({});",
                            js::rhai_string(&glob)
                        )));
                        true
                    }
                    _ => {
                        self.todo(
                            a,
                            b,
                            "port this ignore",
                            format!("{DOCS}#the-config-object"),
                        );
                        false
                    }
                }
            }
            "addLayoutAlias" => {
                let from = args.first().and_then(|&r| self.lit(scope, r));
                let to = args.get(1).and_then(|&r| self.lit(scope, r));
                match (args.len(), from, to) {
                    (2, Some(Lit::Str(from)), Some(Lit::Str(to))) => {
                        self.out.items.push(Item::Rhai(format!(
                            "config.add_layout_alias({}, {});",
                            js::rhai_string(&from),
                            js::rhai_string(&to)
                        )));
                        true
                    }
                    _ => {
                        self.todo(
                            a,
                            b,
                            "port this layout alias to config.add_layout_alias",
                            format!("{DOCS}#the-config-object"),
                        );
                        false
                    }
                }
            }
            "addPlugin" => self.add_plugin(scope, args, a, b),
            "setInputDirectory"
            | "setOutputDirectory"
            | "setIncludesDirectory"
            | "setDataDirectory"
            | "setLayoutsDirectory" => {
                let key = match method {
                    "setInputDirectory" => "input",
                    "setOutputDirectory" => "output",
                    "setIncludesDirectory" => "includes",
                    "setDataDirectory" => "data",
                    _ => "layouts",
                };
                if self.out.in_plugin {
                    self.todo(
                        a,
                        b,
                        "11ty stops here: plugins can't set folders (\"The `set*Directory` configuration API methods are not yet allowed in plugins.\")",
                        format!("{DOCS}#what-the-script-returns"),
                    );
                    return false;
                }
                let lit = args.first().and_then(|&r| self.lit(scope, r));
                match (args.len(), lit) {
                    (1, Some(Lit::Str(v))) => self.set_dir(key, v, a, b),
                    _ => {
                        self.todo(
                            a,
                            b,
                            "port this folder",
                            format!("{DOCS}#what-the-script-returns"),
                        );
                        false
                    }
                }
            }
            "addFilter"
            | "addAsyncFilter"
            | "addLiquidFilter"
            | "addNunjucksFilter"
            | "addNunjucksAsyncFilter"
            | "addJavaScriptFunction" => self.todo_added(
                scope,
                a,
                b,
                args,
                "port this filter to Rhai with config.add_filter",
                filters,
            ),
            "addCollection" => self.todo_added(
                scope,
                a,
                b,
                args,
                "port this collection to Rhai with config.add_collection",
                format!("{DOCS}#collections"),
            ),
            "addGlobalData" => self.todo_added(
                scope,
                a,
                b,
                args,
                "port this to config.add_global_data",
                format!("{DOCS}#the-config-object"),
            ),
            "setTemplateFormats" | "addTemplateFormats" => {
                let formats = match args {
                    [one] => self
                        .lit(scope, *one)
                        .and_then(|l| template_format_names(&l)),
                    _ => None,
                };
                match formats {
                    Some(names) => {
                        let call = if method == "setTemplateFormats" {
                            "set_template_formats"
                        } else {
                            "add_template_formats"
                        };
                        self.out.items.push(Item::Rhai(format!(
                            "config.{call}({});",
                            rhai_names(&names)
                        )));
                        true
                    }
                    None => {
                        self.todo(
                            a,
                            b,
                            "port these template formats to config.set_template_formats or config.add_template_formats; Brrrty builds liquid, md, njk, html and 11ty.js",
                            format!("{DOCS}#what-the-script-returns"),
                        );
                        false
                    }
                }
            }
            "setQuietMode"
            | "setWatchThrottleWaitTime"
            | "setServerOptions"
            | "addWatchTarget"
            | "setWatchJavaScriptDependencies"
            | "setChokidarConfig"
            | "setServerPassthroughCopyBehavior" => {
                if args
                    .iter()
                    .all(|&arg| js::literal(&self.m.toks, arg.0, arg.1).is_some())
                {
                    self.left_out(
                        a,
                        b,
                        "this is for 11ty's watcher and dev server and doesn't change the files 11ty writes, and Brrrty's `serve` has no setting for it",
                        "https://brrrty.com/docs/serve/",
                    );
                } else {
                    self.todo(
                        a,
                        b,
                        "this only sets up 11ty's watcher or dev server, but its arguments run code when 11ty loads the config; port what they do if it changes files 11ty reads, or delete this line",
                        PORTING,
                    );
                }
                false
            }
            m if m.starts_with("htmlTransformer.") => {
                self.todo(
                    a,
                    b,
                    "Brrrty doesn't run posthtml plugins; port what this changes in the HTML to a Rhai transform, or delete it if it changes nothing you compare",
                    PORTING,
                );
                false
            }
            _ => self.todo_added(scope, a, b, args, "port this to Rhai", PORTING.to_string()),
        }
    }

    /// `addPlugin(plugin, options)`: a plugin of the site's own is followed,
    /// as 11ty runs it; a plugin Brrrty has is ported.
    fn add_plugin(&mut self, scope: &Scope, args: &[(usize, usize)], a: usize, b: usize) -> bool {
        let link = format!("{DOCS}#plugins");
        let Some(&first) = args.first() else {
            self.todo(a, b, "port this plugin", link);
            return false;
        };
        let mark = self.out.mark();
        let v = self.eval(scope, first.0, first.1);
        if let Some((fm, def, env)) = self.plugin_fn(v) {
            // Options that use the config object can change any setting;
            // options that run code get a TODO, and the plugin is still followed.
            if let Some(&(x, y)) = args.get(1)
                && self.mentions_config(scope, x, y)
            {
                self.out.drop_touched(mark);
                self.config_escapes(scope, x, y);
                return true;
            }
            let mut runs_code = false;
            let options = match args.get(1) {
                None => Some(Val::Obj(Vec::new())),
                Some(&(x, y)) if args.len() == 2 => {
                    let v = self.eval(scope, x, y);
                    runs_code = !is_pure(&v);
                    Some(v)
                }
                Some(_) => None,
            };
            // `immediate` runs the plugin at once, outside 11ty's plugin stage.
            let immediate = match &options {
                Some(Val::Obj(es)) => match es.iter().find(|e| e.key == "immediate") {
                    None => Some(false),
                    Some(Entry {
                        val: Val::Lit(Lit::Bool(i)),
                        ..
                    }) => Some(*i),
                    Some(_) => None,
                },
                _ => None,
            };
            let (Some(options), Some(immediate)) = (options, immediate) else {
                self.out.drop_touched(mark);
                self.todo(a, b, "port this plugin, and what its options do", link);
                return false;
            };
            self.out.commit(mark);
            self.out.keep = true;
            if runs_code {
                self.todo(a, b, ARGUMENTS, link);
            }
            if immediate || self.out.in_plugin || self.out.cond > 0 || self.out.deferred.is_none() {
                let was = self.out.in_plugin;
                // 11ty runs a plugin added while plugins run, or with `immediate`, at
                // once; only `immediate` from the config function runs outside its
                // plugin stage.
                self.out.in_plugin = was || !immediate;
                self.follow(fm, def, env, vec![Val::Config, options], Kind::Plugin, None);
                self.out.in_plugin = was;
            } else if let Some(d) = self.out.deferred.as_mut() {
                d.push(((fm, def, env), options));
            }
            return true;
        }
        self.out.drop_touched(mark);
        self.plugin(scope, args, a, b)
    }

    /// The function 11ty runs for a plugin value, when Brrrty can follow it:
    /// a function, or an object's `configFunction`.
    fn plugin_fn(&mut self, v: Val) -> Option<Callee> {
        let v = match v {
            Val::Module(m, how) => self.module_value(m, how),
            v => v,
        };
        match v {
            Val::Func(m, def, env) => Some((m, def, env)),
            Val::Obj(es) => match es.into_iter().find(|e| e.key == "configFunction") {
                Some(Entry {
                    val: Val::Func(m, def, env),
                    ..
                }) => Some((m, def, env)),
                _ => None,
            },
            _ => None,
        }
    }

    /// What the plugin `toks[a..b]` is: its module and export.
    fn plugin_ref(&self, (a, b): (usize, usize)) -> Option<Binding> {
        let t = &self.m.toks;
        // A name counts only when nothing else in the file declares or assigns it.
        let trusted = |name: &str| {
            self.m
                .uses
                .get(name)
                .is_some_and(|&(all, plugin)| all == plugin + 1)
        };
        if b == a + 1 {
            let name = t[a].ident()?;
            if !trusted(name) {
                return None;
            }
            return self.m.bindings.get(name).cloned();
        }
        if let Some(m) = required(t, a, b) {
            return Some(Binding::Module(m, How::Require));
        }
        if b >= 3 && t[b - 2].is(".") {
            let export = t[b - 1].ident()?.to_string();
            let module = if b == a + 3 {
                let name = t[a].ident()?;
                if !trusted(name) {
                    return None;
                }
                match self.m.bindings.get(name)? {
                    Binding::Module(m, How::Default | How::Require) => m.clone(),
                    _ => return None,
                }
            } else {
                required(t, a, b - 2)?
            };
            return Some(Binding::Export(module, export));
        }
        None
    }

    fn plugin(&mut self, scope: &Scope, args: &[(usize, usize)], a: usize, b: usize) -> bool {
        let link = format!("{DOCS}#plugins");
        let Some(&first) = args.first() else {
            self.todo(a, b, "port this plugin", link);
            return false;
        };
        let options = match args.get(1) {
            None => Some(Vec::new()),
            Some(&r) => match self.lit(scope, r) {
                Some(Lit::Object(pairs)) => Some(pairs),
                _ => None,
            },
        };
        let rss_version = self.out.site.rss;
        // A plugin named where it is added, or a value that is one (a loop's).
        let binding = match self.plugin_ref(first) {
            Some(b) => Some(b),
            None => {
                let mark = self.out.mark();
                let v = self.eval(scope, first.0, first.1);
                self.out.drop_touched(mark);
                match v {
                    Val::Npm(b) => Some(b),
                    _ => None,
                }
            }
        };
        let (name, known): (&str, &[(&str, &str)]) = match binding {
            Some(Binding::Module(m, How::Default | How::Require)) if m == SYNTAX_HIGHLIGHT => (
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
            Some(Binding::Export(m, e)) if m == RSS && e == "feedPlugin" => {
                if let Some(v) = rss_version
                    && v != 3
                {
                    self.todo(
                        a,
                        b,
                        format!("this site installs version {v} of the RSS plugin, and Brrrty's feeds are version 3's; update the plugin and check its feeds, then port this to config.add_plugin(\"rss\", #{{ ... }})"),
                        link,
                    );
                    return false;
                }
                (
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
                )
            }
            // Version 3 is an ES module: `require()` gives its namespace,
            // which 11ty refuses as a plugin.
            Some(Binding::Module(m, How::Require)) if m == RSS && rss_version == Some(3) => {
                self.todo(
                    a,
                    b,
                    "11ty stops here: version 3 of the RSS plugin is an ES module, so `require()` gives its namespace, not a plugin (\"Invalid EleventyConfig.addPlugin signature\"); use `require(\"@11ty/eleventy-plugin-rss\").rssPlugin` and port that to config.add_plugin(\"rss\", #{ feed: false })",
                    link,
                );
                return false;
            }
            Some(Binding::Module(m, How::Default | How::Require)) if m == RSS => {
                return self.rss_filters(options.as_deref(), a, b);
            }
            Some(Binding::Export(m, e)) if m == RSS && e == "rssPlugin" => {
                return self.rss_filters(options.as_deref(), a, b);
            }
            _ => {
                self.todo(
                    a,
                    b,
                    "Brrrty has its own versions of the syntax highlighting and RSS feed plugins only; port what this plugin does, if a page needs it",
                    link,
                );
                return false;
            }
        };
        let Some(options) = options else {
            self.todo(
                a,
                b,
                format!("port this plugin's options to config.add_plugin(\"{name}\", #{{ ... }})"),
                link,
            );
            return false;
        };
        if name == "rss" && options.is_empty() {
            self.todo(a, b, "give this feed its options", link);
            return false;
        }
        let mut converted = Vec::new();
        for (k, v) in options {
            let Some((_, rhai)) = known.iter().find(|(js, _)| *js == k) else {
                self.todo(
                    a,
                    b,
                    format!("Brrrty's {name} plugin doesn't take the option `{k}`"),
                    link,
                );
                return false;
            };
            // Attributes and template data keep their order in 11ty.
            if matches!(
                k.as_str(),
                "preAttributes" | "codeAttributes" | "templateData"
            ) && !js::order_safe(&v)
            {
                self.todo(
                    a,
                    b,
                    format!("`{k}` has keys whose order would change in a #{{ }} map; build it with object()"),
                    format!("{PORTING}#object-and-key-order"),
                );
                return false;
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
        self.out.items.push(Item::Rhai(line));
        true
    }

    /// The RSS plugin's default export: its filters, without a feed.
    fn rss_filters(&mut self, options: Option<&[(String, Lit)]>, a: usize, b: usize) -> bool {
        let link = format!("{DOCS}#plugins");
        match options {
            Some([]) => {}
            Some(_) => {
                self.todo(
                    a,
                    b,
                    "Brrrty adds the RSS plugin's filters with config.add_plugin(\"rss\", #{ feed: false }), which takes no options yet; port what these options change",
                    link,
                );
                return false;
            }
            None => {
                self.todo(
                    a,
                    b,
                    "port this plugin's options; Brrrty adds the RSS plugin's filters with config.add_plugin(\"rss\", #{ feed: false }), which takes no options yet",
                    link,
                );
                return false;
            }
        }
        match self.out.site.rss {
            Some(3) => {
                self.out.items.push(Item::Rhai(
                    "config.add_plugin(\"rss\", #{ feed: false });".into(),
                ));
                true
            }
            Some(v) => {
                self.todo(
                    a,
                    b,
                    format!("this site installs version {v} of the RSS plugin, and Brrrty's RSS filters are version 3's (version 2's `dateToRfc822` writes the time zone another way); update the plugin, then port this to config.add_plugin(\"rss\", #{{ feed: false }})"),
                    link,
                );
                false
            }
            None => {
                self.todo(
                    a,
                    b,
                    "Brrrty couldn't tell from package.json which version of the RSS plugin this site installs, and its filters are version 3's; if it is 3, port this to config.add_plugin(\"rss\", #{ feed: false })",
                    link,
                );
                false
            }
        }
    }

    fn set_dir(&mut self, key: &'static str, value: String, a: usize, b: usize) -> bool {
        let span = Some(self.src(a, b));
        let key = match key {
            "input" => "dir.input",
            "output" => "dir.output",
            "includes" => "dir.includes",
            "data" => "dir.data",
            _ => "dir.layouts",
        };
        self.out.settings.push(Setting {
            key,
            value: js::rhai_string(&value),
            raw: Some(value),
            at: span,
        });
        true
    }

    /// A TODO for a setting the config returns; in js mode, the JS config applies it.
    fn returned_todo(&mut self, a: usize, b: usize, why: &str, link: String) {
        let refused = why.starts_with("Brrrty doesn't support this setting yet")
            || why.starts_with("Brrrty runs Liquid or Nunjucks here");
        if self.out.site.js && !refused {
            self.js_note(a, b);
        } else {
            self.todo(a, b, why, link);
        }
    }

    /// The object the config returns, in `toks[a..b]`.
    fn returned(&mut self, scope: &Scope, a: usize, b: usize) {
        let link = format!("{DOCS}#what-the-script-returns");
        let m = self.m.clone();
        let Some(props) = js::object_props(&m.toks, a, b) else {
            return self.give_up(
                "Brrrty can't read what the config returns, so it can't tell which settings 11ty uses",
            );
        };
        for p in props {
            let (x, y) = p.range;
            match p.key.as_str() {
                "dir" => {
                    let Some(dirs) = js::object_props(&m.toks, p.value.0, p.value.1) else {
                        self.returned_todo(x, y, "port `dir`", link.clone());
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
                                self.returned_todo(d.range.0, d.range.1, "11ty doesn't read this folder", link.clone());
                                continue;
                            }
                        };
                        match self.lit(scope, d.value) {
                            Some(Lit::Str(v)) => {
                                self.set_dir(key, v, d.range.0, d.range.1);
                            }
                            _ => self.returned_todo(d.range.0, d.range.1, "port this folder", link.clone()),
                        }
                    }
                }
                "markdownTemplateEngine" | "htmlTemplateEngine" => {
                    let key = if p.key == "htmlTemplateEngine" {
                        "htmlTemplateEngine"
                    } else {
                        "markdownTemplateEngine"
                    };
                    let at = Some(self.src(x, y));
                    match self.lit(scope, p.value) {
                        Some(Lit::Str(e)) if e == "liquid" || e == "njk" => {
                            self.out.settings.push(Setting {
                                key,
                                value: js::rhai_string(&e),
                                raw: None,
                                at,
                            })
                        }
                        Some(Lit::Bool(false)) => self.out.settings.push(Setting {
                            key,
                            value: "false".into(),
                            raw: None,
                            at,
                        }),
                        _ => self.returned_todo(
                            x,
                            y,
                            "Brrrty runs Liquid or Nunjucks here, or nothing (false)",
                            link.clone(),
                        ),
                    }
                }
                "dataTemplateEngine" => self.out.left_data_engine = true,
                "templateFormats" => match self
                    .lit(scope, p.value)
                    .and_then(|l| template_format_names(&l))
                {
                    Some(names) => {
                        let at = Some(self.src(x, y));
                        self.out.settings.push(Setting {
                            key: "templateFormats",
                            value: rhai_names(&names),
                            raw: None,
                            at,
                        });
                    }
                    None => self.returned_todo(
                        x,
                        y,
                        "port these template formats; Brrrty builds liquid, md, njk, html and 11ty.js",
                        link.clone(),
                    ),
                },
                _ => self.returned_todo(x, y, "Brrrty doesn't support this setting yet", link.clone()),
            }
        }
    }
}

/// The template format names in a literal: a list of names, or one string
/// of names separated by commas. `None` when it isn't one, or names a format
/// Brrrty doesn't build.
fn template_format_names(l: &Lit) -> Option<Vec<String>> {
    let raw: Vec<String> = match l {
        // An empty string means "not set" in 11ty; leave it for the user.
        Lit::Str(s) if s.is_empty() => return None,
        Lit::Str(s) => s.split(',').map(str::to_string).collect(),
        Lit::Array(items) => items
            .iter()
            .map(|i| match i {
                Lit::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect::<Option<_>>()?,
        _ => return None,
    };
    let names: Vec<String> = raw
        .iter()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    names
        .iter()
        .all(|n| {
            matches!(
                n.as_str(),
                "liquid" | "md" | "njk" | "html" | "11ty.js" | "11ty.cjs" | "11ty.mjs"
            )
        })
        .then_some(names)
}

/// Names as a Rhai array of strings.
fn rhai_names(names: &[String]) -> String {
    let each: Vec<String> = names.iter().map(|n| js::rhai_string(n)).collect();
    format!("[{}]", each.join(", "))
}

/// Quote the lines `at` covers, whole, each as its own `//` line.
fn quoted(out: &mut String, at: &Src) {
    let m = &at.m;
    if m.src.is_empty() {
        return;
    }
    let line_of = |b: usize| m.lines.partition_point(|&(_, end)| end < b);
    let first = line_of(at.span.0);
    let last = line_of(at.span.1).max(first).min(m.lines.len() - 1);
    if first == last {
        out.push_str(&format!("// {}, line {}:\n", one_line(&m.file), first + 1));
    } else {
        out.push_str(&format!(
            "// {}, lines {}-{}:\n",
            one_line(&m.file),
            first + 1,
            last + 1
        ));
    }
    for &(a, b) in &m.lines[first..=last] {
        let l = &m.src[a..b];
        if l.trim().is_empty() {
            out.push_str("//\n");
        } else {
            out.push_str(&format!("//     {}\n", one_line(l)));
        }
    }
}

impl Out<'_> {
    /// The TODOs for the build script's flags, quoting it. With `all`, the
    /// folders it sets too, since nothing else is ported.
    fn cli_todos(&self, out: &mut String, all: bool) -> usize {
        let Some(cli) = &self.cli else { return 0 };
        let mut todos = cli.todos.clone();
        if all {
            let todo = |why: String| CliTodo {
                why,
                script: cli.script.clone(),
                command: cli.command.clone(),
            };
            if let Some(i) = &cli.input {
                todos.push(todo(format!("port `--input={i}` to `dir.input`")));
            }
            if let Some(o) = &cli.output {
                todos.push(todo(format!("port `--output={o}` to `dir.output`")));
            }
        }
        for t in &todos {
            out.push_str(&format!(
                "\n// {TODO}: {}.\n// See {GETTING_STARTED}\n// {}:\n//     {}\n",
                one_line(&t.why),
                one_line(&brrrty_config::eleventy_cli::place(&t.script)),
                one_line(&t.command)
            ));
        }
        todos.len()
    }

    /// An item as it is written: a TODO, unless a declaration's uses were
    /// all ported or quoted.
    fn settle(item: Item) -> Option<Item> {
        match item {
            Item::Decl {
                why,
                names,
                at,
                main,
            } => {
                let all = (0, at.m.toks.len());
                let used = !names.is_empty()
                    && names
                        .iter()
                        .all(|(n, k)| at.m.all_consumed(n, *k, all, main));
                (!used).then(|| Item::Todo {
                    why: why.to_string(),
                    link: PORTING.to_string(),
                    at: Some(at),
                    marker: true,
                })
            }
            Item::Local {
                name,
                decl,
                range,
                at,
            } => (!at.m.all_consumed(&name, decl, range, false)).then(|| Item::Todo {
                why: "port this code".to_string(),
                link: PORTING.to_string(),
                at: Some(at),
                marker: true,
            }),
            item => Some(item),
        }
    }

    fn finish(mut self, main: &Rc<Module>) -> Port {
        let file = &main.file;
        let mut out = if self.no_config {
            format!(
                "// Written by `brrrty init` from package.json. 11ty reads no config file\n\
                 // for this site, only the flags its build script passes, below.\n\
                 // Reference: {DOCS}\n"
            )
        } else {
            format!(
                "// Written by `brrrty init` from {file}, which Brrrty doesn't run.\n\
                 // Brrrty reads this file instead; 11ty still reads {file}.\n\
                 // Reference: {DOCS}\n"
            )
        };
        if let Some(why) = self.js_whole.take() {
            // Nothing could be read, so nothing is ported: the JS config runs it all.
            out.push_str(&format!(
                "\n// Brrrty can't port all of this file to Rhai ({}),\n// so it runs {file} in your own Node, as 11ty does.\n// See {DOCS}\n{JS_CONFIG_LINE}\n",
                one_line(&why)
            ));
            let todos = self.cli_todos(&mut out, true);
            return Port {
                text: out,
                todos,
                input: None,
                output: None,
                data: None,
                modules: Vec::new(),
                uses_rss: false,
                js: true,
            };
        }
        if let Some(why) = self.whole.take() {
            out.push_str(&format!(
                "\n// {TODO_FILE}: port all of this file by hand: {}.\n// Brrrty builds no pages until you port it and delete this line.\n// See {DOCS}\n",
                one_line(&why)
            ));
            if !main.src.is_empty() {
                quoted(
                    &mut out,
                    &Src {
                        m: main.clone(),
                        span: (0, main.src.len()),
                    },
                );
            }
            let todos = 1 + self.cli_todos(&mut out, true);
            return Port {
                text: out,
                todos,
                input: None,
                output: None,
                data: None,
                modules: Vec::new(),
                uses_rss: false,
                js: false,
            };
        }
        // `--input` and `--output` in the build script win over the config.
        let mut from_flags = Vec::new();
        if let Some(cli) = &self.cli {
            for (key, flag, v) in [
                ("dir.input", "input", &cli.input),
                ("dir.output", "output", &cli.output),
            ] {
                if let Some(v) = v {
                    self.settings.retain(|s| s.key != key);
                    self.settings.push(Setting {
                        key,
                        value: js::rhai_string(v),
                        raw: Some(v.clone()),
                        at: None,
                    });
                    from_flags.push(format!("--{flag}={v}"));
                }
            }
        }
        // A setting made more than once: 11ty's precedence decides, so neither is ported.
        let mut settings: Vec<&Setting> = Vec::new();
        let mut todos_after: Vec<Item> = Vec::new();
        let link = format!("{DOCS}#what-the-script-returns");
        for st in &self.settings {
            if self.settings.iter().filter(|o| o.key == st.key).count() > 1 {
                todos_after.push(Item::Todo {
                    why: format!("`{}` is set more than once; which one 11ty uses depends on where each is, so set it by hand", st.key),
                    link: link.clone(),
                    at: st.at.clone(),
                    marker: true,
                });
            } else {
                settings.push(st);
            }
        }
        let mut todos = 0;
        let mut last_single = false;
        let mut body = String::new();
        // Code that runs when a module loads runs in Node, too.
        let js = self.site.js;
        let loaded: Vec<Item> = std::mem::take(&mut self.loaded_items)
            .into_iter()
            .map(|i| match i {
                Item::Todo {
                    link,
                    at,
                    marker: true,
                    ..
                } if js => {
                    self.js_ran = true;
                    Item::Todo {
                        why: JS_RUNS_WHY.to_string(),
                        link,
                        at,
                        marker: false,
                    }
                }
                i => i,
            })
            .collect();
        let items: Vec<Item> = std::mem::take(&mut self.items)
            .into_iter()
            .chain(loaded)
            .chain(todos_after)
            .filter_map(Out::settle)
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
                Item::Todo {
                    why,
                    link,
                    at,
                    marker,
                } => {
                    let head = if *marker {
                        todos += 1;
                        TODO
                    } else {
                        "Left out"
                    };
                    body.push_str(&format!("\n// {head}: {}.\n// See {link}\n", one_line(why)));
                    if let Some(at) = at {
                        quoted(&mut body, at);
                    }
                    last_single = false;
                }
                Item::Decl { .. } | Item::Local { .. } => {}
            }
        }
        if self.js_ran {
            out.push_str(&format!(
                "\n// Some of {file} runs in your own Node, as 11ty does; what is ported below wins.\n{JS_CONFIG_LINE}\n"
            ));
        }
        out.push_str(&body);
        todos += self.cli_todos(&mut out, false);
        if !from_flags.is_empty()
            && let Some(cli) = &self.cli
        {
            out.push_str(&format!(
                "\n// {} from {}, which passes {} to 11ty; flags win over its config:\n//     {}\n",
                if from_flags.len() == 1 {
                    "This folder comes"
                } else {
                    "These folders come"
                },
                one_line(&brrrty_config::eleventy_cli::place(&cli.script)),
                from_flags.join(" "),
                one_line(&cli.command)
            ));
        }
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
            data: dir("dir.data"),
            modules: std::mem::take(&mut self.read),
            uses_rss: items
                .iter()
                .any(|i| matches!(i, Item::Rhai(l) if l.starts_with("config.add_plugin(\"rss\""))),
            js: self.js_ran,
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
        assert!(t.contains("// TODO(brrrty): port this filter to Rhai with config.add_filter.\n// See https://brrrty.com/docs/configuration/#filters\n// eleventy.config.js, line 15:\n//       eleventyConfig.addFilter(\"shout\", (s) => s.toUpperCase() + \"!\");\n"), "{t}");
        assert!(t.contains("globs"), "{t}");
        assert!(t.contains("// Left out: this is for 11ty's watcher"), "{t}");
        assert!(t.contains("#{\n    dir: #{\n        input: \"src\",\n        output: \"dist\",\n        includes: \"_partials\",\n    },\n    markdownTemplateEngine: \"njk\",\n    templateFormats: [\"md\", \"njk\"],\n}\n"), "{t}");
        // The watcher setting is left out, not a TODO: it changes nothing 11ty writes.
        assert_eq!(p.todos, 2, "{t}");
        assert_eq!(p.input.as_deref(), Some("src"));
    }

    #[test]
    fn says_which_statement_stops_porting() {
        let cases = [
            // `config_escapes`: config handed to an imported function.
            (
                "import setup from \"some-package\";\nexport default function (config) {\n  setup(config);\n}\n",
                "at eleventy.config.js, line 3 (`setup(config)`) uses `config`",
            ),
            // `config_escapes`: config stored where init can't follow.
            (
                "export default function (config) {\n  const c = config;\n  globalThis.cfg = c;\n}\n",
                "at eleventy.config.js, line 3 (`globalThis.cfg = c`) uses `c`",
            ),
            // The statement loop.
            (
                "export default function (config) {\n  config.on(\"x\", () => {});\n  Object.assign(config, { a: 1 });\n}\n",
                "at eleventy.config.js, line 3 (`Object.assign(config, { a: 1 })`) uses `config`",
            ),
        ];
        for (src, want) in cases {
            let p = port(src, "eleventy.config.js");
            assert!(p.text.contains("TODO(brrrty-file)"), "{src}\n{}", p.text);
            assert!(
                p.text.contains(&format!(
                    "can't port {want}, and could change any setting, so nothing here is certain"
                )),
                "{src}\n{}",
                p.text
            );
        }
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
        // Without the plugin's version, its filters are a TODO.
        assert!(t.contains("which version of the RSS plugin"), "{t}");
        assert!(t.contains("eleventy.config.mjs, line 9:"), "{t}");
        // Version 3's filters are ported.
        let site = Site {
            rss: Some(3),
            ..Site::default()
        };
        let p3 = port_site(src, "eleventy.config.mjs", site);
        assert!(
            p3.text
                .contains("config.add_plugin(\"rss\", #{ feed: false });"),
            "{}",
            p3.text
        );
        assert_eq!(p3.todos, 1, "{}", p3.text);
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
        assert!(
            t.contains("// TODO(brrrty): port this code along with the code that uses it."),
            "{t}"
        );
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

    /// Port `src` with the site's modules `files`, by path.
    fn port_files(src: &str, files: &[(&str, &str)]) -> Port {
        let map: HashMap<String, String> = files
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let load = move |p: &str| -> Result<Option<String>, String> { Ok(map.get(p).cloned()) };
        port_site(
            src,
            "eleventy.config.js",
            Site {
                load: Some(&load),
                rss: Some(3),
                ..Site::default()
            },
        )
    }

    /// The TODO reasons, in order.
    fn todos(p: &Port) -> Vec<String> {
        p.text
            .lines()
            .filter_map(|l| l.strip_prefix("// TODO(brrrty): "))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn helpers_in_the_same_file_are_followed() {
        let src = "function addFilters(eleventyConfig) {\n  eleventyConfig.addFilter(\"shout\", (s) => s.toUpperCase());\n  eleventyConfig.addPassthroughCopy(\"img\");\n}\nconst more = (c) => c.addLayoutAlias(\"post\", \"layouts/post.njk\");\n\nmodule.exports = function (eleventyConfig) {\n  addFilters(eleventyConfig);\n  more(eleventyConfig);\n  return { dir: { input: \"src\" } };\n};\n";
        let p = port(src, "eleventy.config.js");
        assert!(
            p.text.contains("config.add_passthrough_copy(\"img\");"),
            "{}",
            p.text
        );
        assert!(
            p.text
                .contains("config.add_layout_alias(\"post\", \"layouts/post.njk\");"),
            "{}",
            p.text
        );
        // Only the filter is left, quoted where it is; the helpers themselves are ported.
        assert_eq!(
            todos(&p),
            ["port this filter to Rhai with config.add_filter."],
            "{}",
            p.text
        );
        assert!(
            p.text.contains("// eleventy.config.js, line 2:\n"),
            "{}",
            p.text
        );
        assert_eq!(p.input.as_deref(), Some("src"));
    }

    #[test]
    fn local_modules_plugins_and_loops_are_followed() {
        let src = "const filters = require(\"./_config/filters.js\");\nconst shortcodes = require(\"./_config/shortcodes\");\nconst setup = require(\"./_config/setup.js\");\n\nmodule.exports = function (eleventyConfig) {\n  Object.keys(filters).forEach((name) => eleventyConfig.addFilter(name, filters[name]));\n  eleventyConfig.addPlugin(shortcodes);\n  setup(eleventyConfig);\n  eleventyConfig.addPassthroughCopy(\"a\");\n  return { dir: { input: \"src\" } };\n};\n";
        let files = [
            (
                "_config/filters.js",
                "const slug = (s) => s.toLowerCase();\nmodule.exports = {\n  slug,\n  year: () => new Date().getFullYear(),\n};\n",
            ),
            (
                "_config/shortcodes.js",
                "module.exports = function (eleventyConfig, options = {}) {\n  eleventyConfig.addShortcode(\"btn\", (t) => `<b>${t}</b>`);\n  eleventyConfig.addPassthroughCopy(\"from-plugin\");\n};\n",
            ),
            (
                "_config/setup.js",
                "const dirs = [\"assets/css\", \"assets/js\"];\nmodule.exports = (config) => {\n  dirs.forEach((d) => config.addPassthroughCopy(d));\n  for (const [from, to] of Object.entries({ \"favicon.ico\": \"/\" })) {\n    config.addPassthroughCopy({ \"static\": to });\n  }\n};\n",
            ),
        ];
        let p = port_files(src, &files);
        let t = &p.text;
        // The config function's calls in order, then the plugin's: 11ty runs
        // plugins once the config function returns.
        let copies: Vec<&str> = live(&p)
            .into_iter()
            .filter(|l| l.starts_with("config.add_passthrough_copy"))
            .collect();
        assert_eq!(
            copies,
            [
                "config.add_passthrough_copy(\"assets/css\");",
                "config.add_passthrough_copy(\"assets/js\");",
                "config.add_passthrough_copy(#{ \"static\": \"/\" });",
                "config.add_passthrough_copy(\"a\");",
                "config.add_passthrough_copy(\"from-plugin\");",
            ],
            "{t}"
        );
        // Each filter the loop adds is quoted where it is defined.
        assert_eq!(
            todos(&p),
            [
                "port this filter to Rhai with config.add_filter (`slug`, which the loop at eleventy.config.js, line 6, adds).",
                "port this filter to Rhai with config.add_filter (`year`, which the loop at eleventy.config.js, line 6, adds).",
                "port this to Rhai.",
            ],
            "{t}"
        );
        assert!(
            t.contains(
                "// _config/filters.js, line 1:\n//     const slug = (s) => s.toLowerCase();\n"
            ),
            "{t}"
        );
        assert!(
            t.contains(
                "// _config/filters.js, line 4:\n//       year: () => new Date().getFullYear(),\n"
            ),
            "{t}"
        );
        assert!(t.contains("// _config/shortcodes.js, line 2:\n"), "{t}");
        // The modules read are listed, to record their hashes.
        let read: Vec<&str> = p.modules.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(
            read,
            [
                "_config/filters.js",
                "_config/shortcodes.js",
                "_config/setup.js"
            ]
        );
        assert!(!t.contains("whose own code runs"), "{t}");
    }

    #[test]
    fn es_module_plugins_and_namespaces_are_followed() {
        let src = "import * as filters from \"./lib/filters.js\";\nimport plugin from \"./lib/plugin.js\";\n\nexport default function (eleventyConfig) {\n  for (const [name, fn] of Object.entries(filters)) {\n    eleventyConfig.addFilter(name, fn);\n  }\n  eleventyConfig.addPlugin(plugin, { dir: \"static\" });\n}\n\nexport const config = { dir: { input: \"src\" } };\n";
        let files = [
            (
                "lib/filters.js",
                "export function b(s) {\n  return s;\n}\nexport const a = (s) => s + \"!\";\n",
            ),
            (
                "lib/plugin.js",
                "import syntaxHighlight from \"@11ty/eleventy-plugin-syntaxhighlight\";\nimport rss from \"@11ty/eleventy-plugin-rss\";\n\nexport default function (eleventyConfig, options) {\n  eleventyConfig.addPlugin(syntaxHighlight);\n  eleventyConfig.addPlugin(rss);\n  eleventyConfig.addPassthroughCopy(options.dir);\n  eleventyConfig.setInputDirectory(\"x\");\n}\n",
            ),
        ];
        let p = port_files(src, &files);
        let t = &p.text;
        assert!(t.contains("config.add_plugin(\"syntaxhighlight\");\nconfig.add_plugin(\"rss\", #{ feed: false });\nconfig.add_passthrough_copy(\"static\");\n"), "{t}");
        let todos = todos(&p);
        assert_eq!(todos.len(), 3, "{t}");
        // A namespace's exports are in name order.
        assert!(todos[0].contains("(`a`, "), "{t}");
        assert!(todos[1].contains("(`b`, "), "{t}");
        assert!(
            todos[2].starts_with("11ty stops here: plugins can't set folders"),
            "{t}"
        );
        assert!(t.contains("// lib/filters.js, lines 1-3:\n"), "{t}");
        assert!(t.contains("        input: \"src\",\n"), "{t}");
    }

    #[test]
    fn html_transformer_call_gets_its_own_todo() {
        let plain = "export default function (config) {\n  config.htmlTransformer.addPosthtmlPlugin('html', (context) => {\n    return function (tree) { return tree; };\n  });\n  config.addPassthroughCopy(\"img\");\n}\n";
        let guarded = "export default function (config) {\n  if (config.htmlTransformer && 'addPosthtmlPlugin' in config.htmlTransformer) {\n    config.htmlTransformer.addPosthtmlPlugin('html', (context) => {\n      return function (tree) { return tree; };\n    }, { priority: 10 });\n  }\n  config.addPassthroughCopy(\"img\");\n}\n";
        for src in [plain, guarded] {
            let p = port(src, "eleventy.config.js");
            assert!(!p.text.contains("TODO(brrrty-file)"), "{}", p.text);
            assert_eq!(todos(&p).len(), 1, "{}", p.text);
            assert!(
                live(&p).contains(&"config.add_passthrough_copy(\"img\");"),
                "{}",
                p.text
            );
        }
    }

    #[test]
    fn a_conditional_is_one_todo() {
        let src = "module.exports = function (eleventyConfig) {\n  eleventyConfig.addPassthroughCopy(\"a\");\n  if (process.env.ELEVENTY_ENV === \"production\") {\n    eleventyConfig.addTransform(\"min\", (c) => c);\n    eleventyConfig.addPassthroughCopy(\"prod-only\");\n  } else eleventyConfig.addPassthroughCopy(\"dev-only\");\n  return { dir: { input: \"src\" } };\n};\n";
        let p = port(src, "eleventy.config.js");
        assert_eq!(
            todos(&p),
            [
                "11ty runs this only when `process.env.ELEVENTY_ENV === \"production\"`, which Brrrty can't know; port what it does for the builds you compare, or delete it."
            ],
            "{}",
            p.text
        );
        assert!(
            p.text.contains("// eleventy.config.js, lines 3-6:\n"),
            "{}",
            p.text
        );
        let lines = live(&p);
        assert!(
            lines.contains(&"config.add_passthrough_copy(\"a\");"),
            "{}",
            p.text
        );
        assert!(
            !p.text
                .contains("config.add_passthrough_copy(\"prod-only\")"),
            "{}",
            p.text
        );
        assert!(
            !p.text.contains("config.add_passthrough_copy(\"dev-only\")"),
            "{}",
            p.text
        );
        assert_eq!(p.input.as_deref(), Some("src"));
        // A setting made in a branch, or code that can't be followed there, still ports nothing.
        for src in [
            "module.exports = function (c) {\n  if (process.env.X) { c.setInputDirectory(\"x\"); }\n  c.addPassthroughCopy(\"a\");\n};\n",
            "module.exports = function (c) {\n  if (process.env.X) { require(\"pkg\")(c); }\n  c.addPassthroughCopy(\"a\");\n};\n",
            "module.exports = function (c) {\n  for (let i = 0; i < 2; i++) { c.setOutputDirectory(\"x\"); }\n  c.addPassthroughCopy(\"a\");\n};\n",
        ] {
            let p = port(src, "eleventy.config.js");
            assert!(live(&p).is_empty(), "{src}\n{}", p.text);
            assert_eq!(p.todos, 1, "{src}\n{}", p.text);
        }
    }

    #[test]
    fn what_cant_be_followed_is_marked_or_ports_nothing() {
        // A loop over something unknown: one TODO for the loop.
        let src = "const names = require(\"pkg\").names;\nmodule.exports = function (c) {\n  names.forEach((n) => c.addFilter(n, (s) => s));\n  c.addPassthroughCopy(\"a\");\n};\n";
        let p = port(src, "eleventy.config.js");
        assert_eq!(todos(&p).len(), 1, "{}", p.text);
        assert!(
            todos(&p)[0].starts_with("this loop goes over something Brrrty can't read"),
            "{}",
            p.text
        );
        assert!(
            p.text.contains("config.add_passthrough_copy(\"a\");"),
            "{}",
            p.text
        );
        // A helper Brrrty can't read, from a package or a missing file: nothing is certain.
        for src in [
            "const setup = require(\"pkg\");\nmodule.exports = function (c) {\n  setup(c);\n  c.addPassthroughCopy(\"a\");\n};\n",
            "const setup = require(\"./missing.js\");\nmodule.exports = function (c) {\n  setup(c);\n  c.addPassthroughCopy(\"a\");\n};\n",
            "module.exports = function (c) {\n  require(\"./missing\")(c);\n  c.addPassthroughCopy(\"a\");\n};\n",
            "const h = require(\"./h.js\");\nmodule.exports = function (c) {\n  h(c);\n  c.addPassthroughCopy(\"a\");\n};\n",
        ] {
            let p = port_files(
                src,
                &[("h.js", "module.exports = function (c) { c = 1; };\n")],
            );
            assert!(live(&p).is_empty(), "{src}\n{}", p.text);
            assert_eq!(p.todos, 1, "{src}\n{}", p.text);
        }
    }

    #[test]
    fn a_plugin_that_can_end_early_is_one_todo_from_there() {
        let src = "const p = require(\"./p.js\");\nmodule.exports = function (c) {\n  c.addPlugin(p);\n  c.addPassthroughCopy(\"a\");\n};\n";
        let files = [(
            "p.js",
            "module.exports = function (c, opts = {}) {\n  c.addPassthroughCopy(\"first\");\n  if (!opts.on) return;\n  c.addPassthroughCopy(\"x\");\n  c.addFilter(\"f\", (s) => s);\n};\n",
        )];
        let p = port_files(src, &files);
        assert_eq!(
            live(&p),
            [
                "config.add_passthrough_copy(\"a\");",
                "config.add_passthrough_copy(\"first\");"
            ],
            "{}",
            p.text
        );
        assert_eq!(todos(&p).len(), 1, "{}", p.text);
        assert!(p.text.contains("// p.js, lines 3-5:\n"), "{}", p.text);
    }

    #[test]
    fn module_code_that_runs_on_load_and_helpers_it_uses_are_todos() {
        let src = "const filters = require(\"./f.js\");\nmodule.exports = function (c) {\n  for (const name in filters) c.addFilter(name, filters[name]);\n};\n";
        let files = [(
            "f.js",
            "const fs = require(\"fs\");\nfs.writeFileSync(\"x\", \"y\");\nfunction fmt(d) { return d; }\nfunction unused() {}\nmodule.exports = { date: (d) => fmt(d) };\n",
        )];
        let p = port_files(src, &files);
        let todos = todos(&p);
        assert_eq!(todos.len(), 3, "{}", p.text);
        assert!(
            todos
                .iter()
                .any(|t| t.starts_with("this runs when 11ty loads f.js")),
            "{}",
            p.text
        );
        assert!(
            todos
                .iter()
                .any(|t| t.starts_with("port this code along with the code that uses it")),
            "{}",
            p.text
        );
        assert!(
            p.text
                .contains("// f.js, line 3:\n//     function fmt(d) { return d; }\n"),
            "{}",
            p.text
        );
        assert!(!p.text.contains("function unused"), "{}", p.text);
    }

    #[test]
    fn values_the_code_changes_are_never_ported() {
        for src in [
            // A list built up before the loop.
            "const dirs = [];\ndirs.push(\"b\");\nmodule.exports = function (c) {\n  dirs.forEach((d) => c.addPassthroughCopy(d));\n};\n",
            // A parameter assigned in the body.
            "function h(c, d) {\n  d = \"x\";\n  c.addPassthroughCopy(d);\n}\nmodule.exports = function (c) {\n  h(c, \"a\");\n};\n",
            // A property set later.
            "const o = { dir: \"a\" };\no.dir = \"b\";\nmodule.exports = function (c) {\n  c.addPassthroughCopy(o.dir);\n};\n",
            // An object handed to a function that could change it.
            "const o = { dir: \"a\" };\nmutate(o);\nmodule.exports = function (c) {\n  c.addPassthroughCopy(o.dir);\n};\n",
            // A loop variable the body changes.
            "module.exports = function (c) {\n  for (let d of [\"a\"]) {\n    d += \"x\";\n    c.addPassthroughCopy(d);\n  }\n};\n",
            // A local list changed after it is made.
            "module.exports = function (c) {\n  const dirs = [\"a\"];\n  dirs[0] = \"b\";\n  dirs.forEach((d) => c.addPassthroughCopy(d));\n};\n",
        ] {
            let p = port(src, "eleventy.config.js");
            assert!(
                !live(&p).iter().any(|l| l.contains("add_passthrough_copy")),
                "{src}\n{}",
                p.text
            );
            assert!(p.todos >= 1, "{src}\n{}", p.text);
        }
        // Read-only uses keep the value.
        let src = "const dirs = [\"a\", \"b\"];\nconsole.log(dirs.join(\",\"));\nmodule.exports = function (c) {\n  dirs.forEach((d) => c.addPassthroughCopy(d));\n};\n";
        let p = port(src, "eleventy.config.js");
        assert_eq!(
            live(&p),
            [
                "config.add_passthrough_copy(\"a\");",
                "config.add_passthrough_copy(\"b\");"
            ],
            "{}",
            p.text
        );
    }

    #[test]
    fn plugins_from_a_loop_are_ported() {
        let src = "const hl = require(\"@11ty/eleventy-plugin-syntaxhighlight\");\nconst other = require(\"other-plugin\");\nmodule.exports = function (c) {\n  [hl, other].forEach((p) => c.addPlugin(p));\n};\n";
        let p = port(src, "eleventy.config.js");
        assert!(
            p.text.contains("config.add_plugin(\"syntaxhighlight\");"),
            "{}",
            p.text
        );
        assert_eq!(p.todos, 1, "{}", p.text);
    }

    /// Values 11ty works out on the way (arguments, options, defaults, loop
    /// sources) are never dropped: code in them that uses the config object
    /// refuses the file, other code gets a TODO.
    #[test]
    fn values_worked_out_on_the_way_are_never_dropped() {
        let p_js = (
            "p.js",
            "module.exports = function (c, o) { c.addPassthroughCopy(\"a\"); };\n",
        );
        // Refused: the config object is used where Brrrty doesn't follow it.
        for src in [
            // 11ty builds from `elsewhere`.
            "function h(c, opts) { c.addPassthroughCopy(\"a\"); }\nmodule.exports = function (c) {\n  h(c, c.setInputDirectory(\"elsewhere\"));\n};\n",
            // 11ty copies `secret`.
            "const p = require(\"./p.js\");\nmodule.exports = function (c) {\n  c.addPlugin(p, { x: c.addPassthroughCopy(\"secret\") });\n};\n",
            "function setup(c) { c.addPassthroughCopy(\"secret\"); return 1; }\nfunction h(c, n) { c.addPassthroughCopy(\"a\"); }\nmodule.exports = function (c) {\n  h(c, setup(c));\n};\n",
            "function setup(c) { c.addPassthroughCopy(\"secret\"); return \"a\"; }\nmodule.exports = function (c) {\n  [setup(c), \"b\"].forEach(() => c.addPassthroughCopy(\"z\"));\n};\n",
            "function h(c, x = c.setOutputDirectory(\"o\")) { c.addPassthroughCopy(\"a\"); }\nmodule.exports = function (c) { h(c); };\n",
        ] {
            let p = port_files(src, &[p_js]);
            assert!(live(&p).is_empty(), "{src}\n{}", p.text);
            assert!(p.text.contains("TODO(brrrty-file)"), "{src}\n{}", p.text);
        }
        // A TODO quotes the code that runs; what can be followed still is.
        for (src, quoted) in [
            (
                "const fs = require(\"fs\");\nconst p = require(\"./p.js\");\nmodule.exports = function (c) {\n  c.addPlugin(p, { x: fs.writeFileSync(\"src/gen.md\", \"# gen\") });\n};\n",
                "eleventy.config.js, line 4:",
            ),
            (
                "const fs = require(\"fs\");\nfunction h(c, n) { c.addPassthroughCopy(\"a\"); }\nmodule.exports = function (c) {\n  h(c, fs.rmSync(\"src/drafts\", { recursive: true, force: true }));\n};\n",
                "eleventy.config.js, line 4:",
            ),
            (
                "const fs = require(\"fs\");\nfunction h(c, x = fs.writeFileSync(\"src/g.md\", \"# g\")) { c.addPassthroughCopy(\"a\"); }\nmodule.exports = function (c) { h(c); };\n",
                "eleventy.config.js, line 2:",
            ),
        ] {
            let p = port_files(src, &[p_js]);
            assert_eq!(
                live(&p),
                ["config.add_passthrough_copy(\"a\");"],
                "{src}\n{}",
                p.text
            );
            assert_eq!(p.todos, 1, "{src}\n{}", p.text);
            assert!(p.text.contains(quoted), "{src}\n{}", p.text);
        }
        // A loop over a list or object whose making runs code is one TODO,
        // and so is the top-level code that makes it.
        for (src, todos) in [
            (
                "const fs = require(\"fs\");\nconst list = [fs.rmSync(\"src/drafts\", { recursive: true, force: true }), 1];\nmodule.exports = function (c) {\n  list.forEach(() => c.addPassthroughCopy(\"z\"));\n};\n",
                2,
            ),
            (
                "const fs = require(\"fs\");\nmodule.exports = function (c) {\n  for (const k of Object.keys({ a: fs.writeFileSync(\"src/gen.md\", \"# gen\") })) {\n    c.addPassthroughCopy(\"z\");\n  }\n};\n",
                1,
            ),
        ] {
            let p = port(src, "eleventy.config.js");
            assert!(live(&p).is_empty(), "{src}\n{}", p.text);
            assert_eq!(p.todos, todos, "{src}\n{}", p.text);
            assert!(!p.text.contains("drop it"), "{src}\n{}", p.text);
        }
    }

    /// Taking one part of a list or object, or a repeated key, drops the
    /// rest: code there is never dropped.
    #[test]
    fn indexing_never_drops_code() {
        for src in [
            "const fs = require(\"fs\");\nmodule.exports = function (c) {\n  c.addPassthroughCopy([fs.rmSync(\"src/drafts\", { recursive: true, force: true }), \"a\"][1]);\n};\n",
            "const fs = require(\"fs\");\nconst dir = { x: fs.writeFileSync(\"src/gen.md\", \"# gen\"), y: \"a\" }.y;\nmodule.exports = function (c) {\n  c.addPassthroughCopy(dir);\n};\n",
            "const fs = require(\"fs\");\nmodule.exports = function (c) {\n  c.addLayoutAlias(\"post\", [fs.writeFileSync(\"x\", \"y\"), \"post.njk\"][1]);\n};\n",
            "const fs = require(\"fs\");\nmodule.exports = function (c) {\n  for (const d of [[fs.rmSync(\"src/drafts\"), \"a\"][1]]) c.addPassthroughCopy(d);\n};\n",
            "const fs = require(\"fs\");\nmodule.exports = function (c) {\n  c.addPassthroughCopy({ a: fs.rmSync(\"x\"), a: \"a\" }.a);\n};\n",
        ] {
            let p = port(src, "eleventy.config.js");
            assert!(
                !live(&p).iter().any(|l| l.starts_with("config.")),
                "{src}\n{}",
                p.text
            );
            assert!(p.todos >= 1, "{src}\n{}", p.text);
        }
        // Plain lists and objects still index.
        let src = "module.exports = function (c) {\n  c.addPassthroughCopy([\"x\", \"a\"][1]);\n  c.addPassthroughCopy({ y: \"b\" }.y);\n};\n";
        let p = port(src, "eleventy.config.js");
        assert_eq!(
            live(&p),
            [
                "config.add_passthrough_copy(\"a\");",
                "config.add_passthrough_copy(\"b\");"
            ],
            "{}",
            p.text
        );
    }

    /// In a cycle, a module gets the other's exports before it has finished
    /// loading (here `{}`, as `module.exports` is replaced): unknown.
    #[test]
    fn a_module_cycle_is_never_followed() {
        let src =
            "const a = require(\"./a.js\");\nmodule.exports = function (c) { a.register(c); };\n";
        let files = [
            (
                "a.js",
                "const b = require(\"./b.js\");\nmodule.exports = { static: 1, register(c) { b(c); } };\n",
            ),
            (
                "b.js",
                "const a = require(\"./a.js\");\nmodule.exports = function (c) { Object.keys(a).forEach((k) => c.addPassthroughCopy(k)); };\n",
            ),
        ];
        let p = port_files(src, &files);
        assert!(
            !p.text.contains("config.add_passthrough_copy(\"static\")"),
            "{}",
            p.text
        );
        assert!(p.todos >= 1, "{}", p.text);
    }

    #[test]
    fn the_rss_plugin_ports_by_version() {
        let src = "import pluginRss, { rssPlugin } from \"@11ty/eleventy-plugin-rss\";\nexport default function (c) {\n  c.addPlugin(rssPlugin);\n  c.addPlugin(pluginRss, { posthtmlRenderOptions: {} });\n}\n";
        // `require()` of version 3 gives its namespace, which 11ty refuses.
        let p = port_site(
            "const pluginRss = require(\"@11ty/eleventy-plugin-rss\");\nmodule.exports = function (c) {\n  c.addPlugin(pluginRss);\n};\n",
            "eleventy.config.js",
            Site {
                rss: Some(3),
                ..Site::default()
            },
        );
        assert!(todos(&p)[0].starts_with("11ty stops here"), "{}", p.text);
        for (rss, ported) in [(Some(3), true), (Some(2), false), (None, false)] {
            let p = port_site(
                src,
                "eleventy.config.js",
                Site {
                    rss,
                    ..Site::default()
                },
            );
            assert_eq!(
                p.text
                    .contains("config.add_plugin(\"rss\", #{ feed: false });"),
                ported,
                "{rss:?}\n{}",
                p.text
            );
            assert!(p.text.contains("takes no options yet"), "{}", p.text);
        }
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
    fn watcher_calls_that_run_code_and_local_modules_are_todos() {
        let src = "import \"./gen.js\";\nimport x from \"./x.js\";\nimport { a } from \"pkg\";\nconst y = require(\"../y\");\nconst z = require(\"z\");\nexport default function (c) {\n  c.addWatchTarget(\"src/css\");\n  c.setServerOptions({ port: 8080 });\n  c.addWatchTarget(build());\n}\n";
        let p = port(src, "eleventy.config.js");
        let t = &p.text;
        // The three local modules and the watcher call with code in it.
        assert_eq!(p.todos, 4, "{t}");
        assert_eq!(
            t.matches("// Left out: this is for 11ty's watcher").count(),
            2,
            "{t}"
        );
        assert_eq!(
            t.matches("whose own code runs when 11ty loads the config")
                .count(),
            3,
            "{t}"
        );
        assert!(t.contains("but its arguments run code"), "{t}");
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
            "module.exports = function (c) { c.addPassthroughCopy(\"a\") };\nexport const config = { dir: { output: \"x\" } };\n",
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
    fn commonjs_config_export_ports() {
        let src = "module.exports = async function (eleventyConfig) {\n  eleventyConfig.addPassthroughCopy(\"a\");\n};\nmodule.exports.config = {\n  dir: { input: \"src\", output: \"dist\" },\n  markdownTemplateEngine: \"njk\",\n};\n";
        let p = port(src, "eleventy.config.js");
        assert_eq!(p.todos, 0, "{}", p.text);
        assert!(!p.text.contains("mixes"), "{}", p.text);
        assert!(
            p.text.contains("        input: \"src\",\n        output: \"dist\",\n    },\n    markdownTemplateEngine: \"njk\",\n"),
            "{}",
            p.text
        );
        assert_eq!(p.input.as_deref(), Some("src"));
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
            // U+2028 and U+2029 in code make the file one TODO.
            let want: &[&str] = if nl != "\n" && nl != "\r\n" {
                &[]
            } else {
                &["config.add_passthrough_copy(\"a\");"]
            };
            assert_eq!(live(&p), want, "{nl:?}\n{}", p.text);
        }
        // A key with a line break in it, quoted in the reason.
        let src = "const h = require(\"@11ty/eleventy-plugin-syntaxhighlight\");\nmodule.exports = function (c) { c.addPlugin(h, { \"a\\nconfig.add_passthrough_copy('x');\": 1 }) };\n";
        let p = port(src, "eleventy.config.js");
        assert!(live(&p).is_empty(), "{}", p.text);
    }

    /// Modules that load each other, call each other and loop, from random
    /// pieces: the port stays bounded and never panics.
    #[test]
    fn fuzz_with_modules_never_panics() {
        let pieces = [
            "module.exports = function (c) {",
            "export default function (c) {",
            "}",
            "};",
            "c.addPassthroughCopy(\"a\");",
            "c.addPlugin(require(\"./a.js\"));",
            "require(\"./b.js\")(c);",
            "const h = require(\"./a.js\");",
            "import x from \"./b.js\";",
            "h(c);",
            "x(c);",
            "[1, 2].forEach((n) => h(c));",
            "Object.keys(h).forEach((k) => c.addFilter(k, h[k]));",
            "for (const k of [h, x]) k(c);",
            "if (process.env.X) { h(c); }",
            "if (!c) return;",
            "module.exports.f = (c) => c.addLayoutAlias(\"a\", \"b\");",
            "exports.g = h;",
            "c.setInputDirectory(\"src\");",
            "return { dir: { input: \"src\" } };",
            "(",
            ")",
            "{",
            "\n",
        ];
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let started = std::time::Instant::now();
        for _ in 0..1500 {
            let mut soup = || {
                let len = next(14);
                let mut src = String::new();
                for _ in 0..len {
                    src.push_str(pieces[next(pieces.len())]);
                    src.push('\n');
                }
                src
            };
            let (main, a, b) = (soup(), soup(), soup());
            let files = [("a.js", a.as_str()), ("b.js", b.as_str())];
            let p = port_files(&main, &files);
            for l in p.text.split('\n') {
                assert!(!l.contains(['\r', '\u{2028}', '\u{2029}']), "{main:?}");
            }
        }
        // Modules that call each other forever stop.
        let p = port_files(
            "const a = require(\"./a.js\");\nmodule.exports = function (c) { a(c); };\n",
            &[(
                "a.js",
                "module.exports = function f(c) { require(\"./a.js\")(c); f(c); };\n",
            )],
        );
        assert!(p.todos >= 1, "{}", p.text);
        assert!(started.elapsed().as_secs() < 30, "{:?}", started.elapsed());
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
            "forEach",
            "Object.keys",
            "Object.entries",
            "if",
            "else",
            "for",
            "of",
            "in",
            "let",
            "require(\"./x\")",
            "\"./x.js\"",
            "addFilter",
            "setInputDirectory",
            "addLayoutAlias",
            "push",
            "...",
            "?",
            "new",
            "this",
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
            "module.exports = function (c) {\n  c.setOutputDirectory(process.env.OUT);\n  c.addPassthroughCopy(\"a\");\n};\n",
            "module.exports = function (c) {\n  c.addPlugin(plugin, { config: c });\n  return { dir: { input: \"src\" } };\n};\n",
            "module.exports = function (c) {\n  return { ...base, dir: { input: \"src\" } };\n};\n",
        ] {
            let p = port(src, "eleventy.config.js");
            assert!(live(&p).is_empty(), "{src}\n{}", p.text);
            assert_eq!(p.todos, 1, "{src}\n{}", p.text);
        }
        // A loop over a literal runs as 11ty runs it.
        let src = "module.exports = function (c) {\n  c.setInputDirectory(\"src\");\n  [0].forEach(() => c.setOutputDirectory(\"evil\"));\n};\n";
        let p = port(src, "eleventy.config.js");
        assert_eq!(p.output.as_deref(), Some("evil"), "{}", p.text);
        assert_eq!(p.todos, 0, "{}", p.text);
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
        ] {
            let p = port(src, "eleventy.config.js");
            assert!(live(&p).is_empty(), "{src}\n{}", p.text);
            assert!(
                p.text.contains("port all of this file"),
                "{src}\n{}",
                p.text
            );
        }
        // Clear cases still read: division after a call, a regex after `(` or `,`,
        // and the same inside a template literal's `${}`.
        for src in [
            "module.exports = function (c) {\n  c.addFilter(\"f\", (s) => s.split(/,/).length / 2);\n  c.addFilter(\"g\", (s) => `${s}/x`);\n  c.addPassthroughCopy(\"a\");\n};\n",
            "module.exports = function (c) {\n  c.addFilter(\"f\", (s) => `${s.replace(/}/g, \"\")}`);\n  c.addPassthroughCopy(\"a\");\n};\n",
            "module.exports = function (c) {\n  c.addFilter(\"f\", (s) => `${a / b}/${`${s.replace(/'/g, \"\")}`}`);\n  c.addPassthroughCopy(\"a\");\n};\n",
        ] {
            let p = port(src, "eleventy.config.js");
            assert_eq!(
                live(&p),
                ["config.add_passthrough_copy(\"a\");"],
                "{src}\n{}",
                p.text
            );
        }
        // A `/` inside `${}` that could be either is still never guessed.
        let src = "module.exports = function (c) {\n  c.addFilter(\"f\", (s) => `${(x) /'/.test(s)}`);\n  c.addPassthroughCopy(\"a\");\n};\n";
        let p = port(src, "eleventy.config.js");
        assert!(live(&p).is_empty(), "{}", p.text);
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

    #[test]
    fn plugin_options_in_a_pure_const_still_port_the_rest() {
        let src = "import somePlugin from \"some-third-party-plugin\";\nexport default async function (eleventyConfig) {\n\tconst pluginConfig = { minify: true };\n\televentyConfig.addPlugin(somePlugin, pluginConfig);\n\televentyConfig.addPassthroughCopy(\"src/img\");\n\treturn { dir: { input: \"src\" } };\n}\n";
        let p = port(src, "eleventy.config.js");
        assert!(!p.text.contains("TODO(brrrty-file)"), "{}", p.text);
        assert!(p.text.contains("TODO(brrrty)"), "{}", p.text);
        assert!(
            p.text.contains("config.add_passthrough_copy(\"src/img\");"),
            "{}",
            p.text
        );
        assert!(p.text.contains("input: \"src\""), "{}", p.text);
        // Options that run code, or could hold `immediate`, still give up.
        for opts in [
            "{ minify: process.env.X }",
            "{ minify: build() }",
            "{ immediate: true }",
        ] {
            let src = format!(
                "import somePlugin from \"some-third-party-plugin\";\nexport default async function (eleventyConfig) {{\n\tconst pluginConfig = {opts};\n\televentyConfig.addPlugin(somePlugin, pluginConfig);\n\televentyConfig.addPassthroughCopy(\"src/img\");\n}}\n"
            );
            let p = port(&src, "eleventy.config.js");
            assert!(live(&p).is_empty(), "{src}\n{}", p.text);
        }
    }

    #[test]
    fn escaped_names_port_nothing() {
        for src in [
            // Real 11ty writes `./pluginout/`: the key is `immediate`.
            "const p = require(\"./p\");\nmodule.exports = function (c) {\n  c.addPlugin(p, { imm\\u0065diate: true });\n  c.addPassthroughCopy(\"a\");\n};\n",
            "module.exports = function (c) {\n  c.setOutputDir\\u0065ctory(x);\n  c.addPassthroughCopy(\"a\");\n};\n",
            "module.exports = function (c) {\n  c.addPassthroughCopy(\"a\");\n  \\u0072eturn\n  { dir: { output: \"x\" } };\n};\n",
            "\\u006dodule.exports = function (c) { c.addPassthroughCopy(\"a\") };\n",
            "module.exports = function (\\u0063) { c.addPassthroughCopy(\"a\") };\n",
        ] {
            let p = port(src, "eleventy.config.js");
            assert!(live(&p).is_empty(), "{src}\n{}", p.text);
            assert_eq!(p.todos, 1, "{src}\n{}", p.text);
            assert!(p.text.contains("escape"), "{src}\n{}", p.text);
        }
        // An escape inside a string is fine: its value is decoded.
        let src = "module.exports = function (c) { c.addPassthroughCopy(\"\\u0061\") };\n";
        let p = port(src, "eleventy.config.js");
        assert!(
            p.text.contains("config.add_passthrough_copy(\"a\");"),
            "{}",
            p.text
        );
    }

    #[test]
    fn non_ascii_outside_strings_and_comments_ports_nothing() {
        for src in [
            "const p = require(\"./p\");\nmodule.exports = function (c) {\n  c.addPlugin(p, { immediate\u{a0}: true });\n  c.addPassthroughCopy(\"a\");\n};\n",
            "module.exports = function (c) {\n  c\u{a0}.setOutputDirectory(x);\n  c.addPassthroughCopy(\"a\");\n};\n",
            "module.exports = function (c) {\n  c.addPassthroughCopy(\"a\");\u{2028}  c\u{3000}.setOutputDirectory(x);\n};\n",
            "module.exports = function (c) {\n  c.addPassthroughCopy(\"a\"); // note\u{2028}c.setOutputDirectory(x);\n};\n",
            "module.exports = function (c) {\n  c.addPassthroughCopy(\"a\"); // note\rc.setOutputDirectory(x);\n};\n",
            "module.exports = function (c) {\u{feff}\n  c.addPassthroughCopy(\"a\");\n};\n",
        ] {
            let p = port(src, "eleventy.config.js");
            assert!(live(&p).is_empty(), "{src:?}\n{}", p.text);
            assert_eq!(p.todos, 1, "{src:?}\n{}", p.text);
        }
        // Non-ASCII inside a regular expression is part of its pattern.
        let src = "module.exports = function (c) {\n  c.addFilter(\"f\", (s) => s.replace(/[éà\u{a0}]/g, \"\"));\n  c.addPassthroughCopy(\"a\");\n};\n";
        let p = port(src, "eleventy.config.js");
        assert_eq!(
            live(&p),
            ["config.add_passthrough_copy(\"a\");"],
            "{}",
            p.text
        );
        // A leading BOM, and non-ASCII in strings, templates and comments, still port.
        let src = "\u{feff}// Café config, ünïcode\u{a0}ok\n/* \u{3000} */\nmodule.exports = function (c) {\n  c.addFilter(\"f\", (s) => `${s}—é`);\n  c.addPassthroughCopy(\"bilder/größe\u{a0}x\");\n};\n";
        let p = port(src, "eleventy.config.js");
        assert_eq!(
            live(&p),
            ["config.add_passthrough_copy(\"bilder/größe\u{a0}x\");"],
            "{}",
            p.text
        );
    }

    #[test]
    fn unusual_line_terminators_anywhere_port_nothing() {
        for src in [
            // R8: a block comment holding only a CR or U+2028 is a line break after `return`.
            "module.exports = function (c) {\n  c.addPassthroughCopy(\"a\");\n  return /*\r*/ { dir: { output: \"x\" } };\n};\n",
            "module.exports = function (c) {\n  c.addPassthroughCopy(\"a\");\n  return /*\u{2028}*/ { dir: { output: \"x\" } };\n};\n",
            // R9: a hashbang line ends at CR too.
            "#!/usr/bin/env node\rmodule.exports = function (c) { c.setOutputDirectory(x); c.addPassthroughCopy(\"a\") };\n",
            // A lone CR or a separator anywhere, even in a string or comment.
            "module.exports = function (c) {\n  c.addPassthroughCopy(\"a\");\n  // ok\r\n  c.addFilter(\"f\", (s) => \"x\\\r\");\n};\n",
            "module.exports = function (c) {\n  c.addPassthroughCopy(\"a\u{2028}b\");\n};\n",
            "module.exports = function (c) {\n  c.addPassthroughCopy(\"a\");\n  /* \u{2029} */\n};\n",
            "\rmodule.exports = function (c) { c.addPassthroughCopy(\"a\") };\n",
        ] {
            let p = port(src, "eleventy.config.js");
            assert!(live(&p).is_empty(), "{src:?}\n{}", p.text);
            assert_eq!(p.todos, 1, "{src:?}\n{}", p.text);
        }
        // CR LF throughout still ports.
        let src = "#!/usr/bin/env node\r\n// c\r\nmodule.exports = function (c) {\r\n  /* a\r\n  b */\r\n  c.addPassthroughCopy(\"a\");\r\n  return {\r\n    dir: { input: \"src\" },\r\n  };\r\n};\r\n";
        let p = port(src, "eleventy.config.js");
        assert_eq!(p.todos, 0, "{}", p.text);
        assert!(
            p.text.contains("config.add_passthrough_copy(\"a\");"),
            "{}",
            p.text
        );
        assert!(p.text.contains("        input: \"src\",\n"), "{}", p.text);
    }

    /// [`port_files`] with `js` on, as `brrrty init` does when Rhai can't hold it all.
    fn port_js(src: &str, files: &[(&str, &str)]) -> Port {
        let map: HashMap<String, String> = files
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let load = move |p: &str| -> Result<Option<String>, String> { Ok(map.get(p).cloned()) };
        port_site(
            src,
            "eleventy.config.js",
            Site {
                load: Some(&load),
                rss: Some(3),
                js: true,
                ..Site::default()
            },
        )
    }

    #[test]
    fn js_mode_runs_filters_shortcodes_transforms_and_the_rest_in_node() {
        let src = "module.exports = function (c) {\n  c.addFilter(\"shout\", (s) => s.toUpperCase());\n  c.addShortcode(\"b\", () => \"x\");\n  c.addPairedShortcode(\"p\", (body) => body);\n  c.addTransform(\"t\", (h) => h);\n  c.addGlobalData(\"g\", () => 1);\n  c.addPassthroughCopy(\"a\");\n  c.addTemplateFormats(compute());\n  return { dir: { input: \"src\" } };\n};\n";
        let p = port_js(src, &[]);
        assert!(p.js, "{}", p.text);
        assert_eq!(p.todos, 0, "{}", p.text);
        assert!(!p.text.contains("TODO(brrrty"), "{}", p.text);
        assert!(
            p.text.contains("config.set_js_config(\"node\");"),
            "{}",
            p.text
        );
        // What Rhai can hold is still ported.
        assert!(
            p.text.contains("config.add_passthrough_copy(\"a\");"),
            "{}",
            p.text
        );
        assert_eq!(p.input.as_deref(), Some("src"));
    }

    #[test]
    fn js_mode_does_not_give_up_on_chained_calls_or_helpers_that_take_the_config() {
        let chained =
            "export default function (c) { c.addPassthroughCopy('a').addPassthroughCopy('b'); }";
        let p = port_js(chained, &[]);
        assert!(p.js, "{}", p.text);
        assert!(!p.text.contains("TODO(brrrty"), "{}", p.text);
        assert!(
            p.text.contains("config.set_js_config(\"node\");"),
            "{}",
            p.text
        );

        let src = "const { registerFilters } = require('./lib/filters')\nmodule.exports = (eleventyConfig) => {\n  registerFilters(eleventyConfig)\n  eleventyConfig.addPassthroughCopy('assets')\n  return { dir: { input: 'app' } }\n}\n";
        let lib = "const names = ['upper2']\nconst registerFilters = (eleventyConfig) => {\n  names.forEach((n) => {\n    try {\n      eleventyConfig.addFilter(n, require(`./f/${n}`))\n    } catch (e) { throw e }\n  })\n}\nmodule.exports = { registerFilters }\n";
        let p = port_js(src, &[("lib/filters", lib), ("lib/filters.js", lib)]);
        assert!(p.js, "{}", p.text);
        assert!(!p.text.contains("TODO(brrrty"), "{}", p.text);
        assert!(
            p.text.contains("config.set_js_config(\"node\");"),
            "{}",
            p.text
        );
    }

    #[test]
    fn js_mode_keeps_todos_for_what_the_js_config_does_not_run() {
        let src = "module.exports = function (c) {\n  c.addFilter(\"f\", (s) => s);\n  c.addCollection(\"all2\", (api) => api.getAll());\n};\n";
        let p = port_js(src, &[]);
        assert!(p.js, "{}", p.text);
        assert_eq!(todos(&p).len(), 1, "{}", p.text);
        assert!(todos(&p)[0].contains("collection"), "{}", p.text);
    }

    #[test]
    fn js_mode_keeps_the_glob_todo_because_the_js_config_copies_no_globs_either() {
        let p = port_js(
            "module.exports = function (c) {\n  c.addFilter(\"f\", (s) => s);\n  c.addPassthroughCopy(\"*.png\");\n};\n",
            &[],
        );
        assert!(p.js, "{}", p.text);
        assert_eq!(todos(&p).len(), 1, "{}", p.text);
        assert!(todos(&p)[0].contains("not globs"), "{}", p.text);
    }

    #[test]
    fn js_mode_is_off_when_nothing_needed_it() {
        let p = port_js(
            "module.exports = (c) => { c.addPassthroughCopy('a'); };",
            &[],
        );
        assert!(!p.js, "{}", p.text);
        assert!(!p.text.contains("set_js_config"), "{}", p.text);
    }
}
