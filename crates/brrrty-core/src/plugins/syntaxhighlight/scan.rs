//! Which Prism grammars other pages change.
//!
//! `Prism.languages` is one object for the whole build in 11ty. A page that
//! loads a language Brrrty doesn't have can change the grammars of the ones
//! it has (`regex` reloads `javascript`, `css-extras` changes `css`, ...),
//! and every page 11ty renders after it is highlighted with the changed
//! grammar. Brrrty can't know the order 11ty renders pages in, and doesn't
//! have the changed grammars, so a page that highlights with one of them is
//! refused once any page could load such a language.
//!
//! "Could" is read from the source, before anything renders, so that pages
//! Brrrty refuses or doesn't render (Nunjucks, includes, layouts, pages
//! that fail) count too: every Markdown fence's info string in every file
//! under the input folder, and every `{% highlight %}` tag in a file that
//! a template language with the tag reads. A language name that a
//! template writes (`{{ lang }}`) could be any language.
//!
//! Which languages read a page is decided the way the build decides it:
//! by its extension, its front matter's `templateEngineOverride` and the
//! config's `markdownTemplateEngine` and `htmlTemplateEngine`. When that
//! can't be known before the build (the data cascade could set the
//! override, the page could be included or used as a layout, the file is
//! data), both Liquid and Nunjucks count as reading it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use brrrty_js::Value;
use brrrty_prism::grammar::grammars;

use super::normalize;
use crate::frontmatter::{Data, split_front_matter};
use crate::options::{TemplateEngines, TemplateLanguage as Lang};
use crate::render::{DataEngine, Engines, engines_for};

/// A language some file could load that changes grammars Brrrty has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The file, as shown in problems.
    pub path: PathBuf,
    /// 1-based.
    pub line: usize,
    /// The language as written.
    pub language: String,
    /// The `Prism.languages` keys it changes.
    pub keys: Vec<String>,
}

/// What the scan needs from the config.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// What renders Markdown and HTML.
    pub engines: TemplateEngines,
    /// Whether Liquid has `{% highlight %}` (the plugin's `templateFormats`
    /// has `liquid`).
    pub liquid_tag: bool,
    /// Whether Nunjucks has it (`templateFormats` has `njk`).
    pub nunjucks_tag: bool,
    /// The config's global data (`addGlobalData`).
    pub global_data: &'a Data,
    /// The includes and layouts folders, relative to the input folder.
    pub includes: &'a Path,
    pub layouts: &'a Path,
    /// The global data folder, relative to the input folder.
    pub data: &'a Path,
}

/// Larger files aren't templates anyone writes by hand; they are skipped.
const MAX_FILE: u64 = 16 << 20;

/// The template languages that read some text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Readers {
    liquid: bool,
    nunjucks: bool,
}

impl Readers {
    /// Either could: the scan can't tell which.
    const ANY: Readers = Readers {
        liquid: true,
        nunjucks: true,
    };

    fn of(lang: Option<Lang>) -> Readers {
        Readers {
            liquid: lang == Some(Lang::Liquid),
            nunjucks: lang == Some(Lang::Nunjucks),
        }
    }

    fn some(self) -> bool {
        self.liquid || self.nunjucks
    }
}

/// One file under the input folder.
struct File {
    /// Relative to the input folder.
    rel: PathBuf,
    shown: PathBuf,
    text: String,
    /// Where the body starts, after any front matter.
    body_at: usize,
    /// What reads the front matter (a page's permalink and computed
    /// values) and the body.
    front: Readers,
    body: Readers,
}

impl File {
    fn read(&self) -> bool {
        self.front.some() || self.body.some()
    }

    /// The parts of the text that something reads, with the line each
    /// starts on (0-based) and what reads it.
    fn parts(&self) -> [(&str, usize, Readers); 2] {
        let (front, body) = self.text.split_at(self.body_at);
        let lines = front.matches('\n').count();
        [(front, 0, self.front), (body, lines, self.body)]
    }
}

/// Every file under `input` (as `input_rel` shows it) that names a
/// language changing grammars Brrrty has, skipping `node_modules`, `.git`
/// and anything under `skip` (the output folder).
pub fn scan(input: &Path, input_rel: &Path, skip: &Path, cx: Context<'_>) -> Vec<Found> {
    let mut paths = Vec::new();
    walk(input, skip, &mut paths);
    paths.sort();
    let mut files = Vec::new();
    for full in paths {
        let Ok(bytes) = fs::read(&full) else { continue };
        if bytes.iter().take(8192).any(|&b| b == 0) {
            continue;
        }
        let rel = full.strip_prefix(input).unwrap_or(&full).to_path_buf();
        files.push(File {
            shown: input_rel.join(&rel),
            rel,
            text: String::from_utf8_lossy(&bytes).into_owned(),
            body_at: 0,
            front: Readers::default(),
            body: Readers::default(),
        });
    }
    let global = Global::of(&files, cx.global_data, cx.data);
    for f in &mut files {
        decide(f, input, input_rel, &cx, &global);
    }
    includes(&mut files, &global);

    let mut out = Vec::new();
    for f in &files {
        let mut add = |line: usize, language: String| {
            if let Some(keys) = changes(&language) {
                out.push(Found {
                    path: f.shown.clone(),
                    line,
                    language,
                    keys,
                });
            }
        };
        let mut found = fences(&f.text);
        for (text, offset, readers) in f.parts() {
            let liquid = readers.liquid && cx.liquid_tag;
            let nunjucks = readers.nunjucks && cx.nunjucks_tag;
            if liquid || nunjucks {
                found.extend(
                    tags(text, nunjucks)
                        .into_iter()
                        .map(|(line, l)| (line + offset, l)),
                );
            }
        }
        found.sort_by_key(|(line, _)| *line);
        for (line, language) in found {
            add(line, language);
        }
    }
    out
}

/// What the global data could set, before it is loaded.
struct Global {
    /// `templateEngineOverride`, for every page.
    engine: bool,
    /// A `layout` naming no file the scan can see.
    layout_unknown: bool,
    /// The `layout` names it sets.
    layouts: Vec<String>,
}

impl Global {
    fn of(files: &[File], config: &Data, data: &Path) -> Global {
        let mut g = Global {
            engine: false,
            layout_unknown: false,
            layouts: Vec::new(),
        };
        let mut key = |k: &str, value: Option<&Value>| match k {
            "templateEngineOverride" => g.engine = true,
            "eleventyComputed" => {
                g.engine = true;
                g.layout_unknown = true;
            }
            "layout" => match value {
                Some(v) => layout_names(v, &mut g.layouts, &mut g.layout_unknown),
                None => g.layout_unknown = true,
            },
            _ => {}
        };
        for (k, v) in config {
            key(k, Some(v));
        }
        // A `_data` file's top-level key is its name.
        for f in files {
            if let Ok(inside) = f.rel.strip_prefix(data)
                && let Some(first) = inside.components().next()
            {
                let name = first.as_os_str().to_string_lossy();
                key(name.split('.').next().unwrap_or(""), None);
            }
        }
        g
    }
}

/// The `layout` names in a value: every string under a `layout` key, and
/// the value itself when it is one. One a template writes could name any
/// file.
fn layout_names(v: &Value, out: &mut Vec<String>, unknown: &mut bool) {
    match v {
        Value::String(s) if s.contains("{{") || s.contains("{%") => *unknown = true,
        Value::String(s) => out.push(s.to_string()),
        _ => {}
    }
}

/// Every `layout` in some data, at any depth (`eleventyComputed` too).
fn layouts_in(data: &Value, out: &mut Vec<String>, unknown: &mut bool) {
    match data {
        Value::Object(o) => {
            for (k, v) in o.iter() {
                if k == "layout" {
                    layout_names(v, out, unknown);
                }
                layouts_in(v, out, unknown);
            }
        }
        Value::Array(a) => {
            for v in a.iter() {
                layouts_in(v, out, unknown);
            }
        }
        _ => {}
    }
}

fn extension(rel: &Path) -> String {
    rel.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn is_page_ext(ext: &str) -> bool {
    matches!(ext, "md" | "html" | "liquid" | "njk")
}

/// What reads `f`, before anything includes it.
fn decide(f: &mut File, input: &Path, input_rel: &Path, cx: &Context<'_>, g: &Global) {
    let engines = cx.engines;
    let ext = extension(&f.rel);
    // Includes and layouts are read by whatever includes them; data by
    // the language a page's permalink or computed values, or
    // `dataTemplateEngine`, runs; JavaScript could be a template.
    if f.rel.starts_with(cx.includes)
        || f.rel.starts_with(cx.layouts)
        || f.rel.starts_with(cx.data)
        || matches!(ext.as_str(), "json" | "js" | "cjs" | "mjs" | "rhai")
    {
        f.front = Readers::ANY;
        f.body = Readers::ANY;
        return;
    }
    if !is_page_ext(&ext) {
        // Read only when included.
        return;
    }
    let unsure = |f: &mut File| {
        f.front = Readers::ANY;
        f.body = Readers::ANY;
    };
    // `---js` and the like: front matter Brrrty doesn't parse.
    let first = f.text.trim_start_matches('\u{feff}').lines().next();
    if first.is_some_and(|l| l.starts_with("---") && !l[3..].trim().is_empty()) {
        return unsure(f);
    }
    let Ok((front, body)) = split_front_matter(&f.text) else {
        return unsure(f);
    };
    let body_at = f.text.len() - body.len();
    // The cascade sets the override when the global data or a data file
    // that applies to the page could.
    if g.engine || local_override(input, input_rel, &f.rel) {
        return unsure(f);
    }
    if front.get("eleventyComputed").is_some_and(
        |c| !matches!(c, Value::Object(o) if !o.contains_key("templateEngineOverride")),
    ) {
        return unsure(f);
    }
    let over = front.get("templateEngineOverride");
    let (Ok((body_engines, _)), Ok(data)) = (
        engines_for(&ext, over, engines),
        DataEngine::new(&ext, over, engines),
    ) else {
        return unsure(f);
    };
    f.body_at = body_at;
    f.body = Readers::of(match body_engines {
        Engines::Raw | Engines::Markdown { pre: None } | Engines::Js(_) => None,
        Engines::Template(l) | Engines::Markdown { pre: Some(l) } => Some(l),
    });
    f.front = Readers::of(data.lang);
}

/// Whether a data file that applies to the page at `rel` could set its
/// `templateEngineOverride`: a JSON one that has it (or computed values,
/// or that doesn't parse), or one that runs code.
fn local_override(input: &Path, input_rel: &Path, rel: &Path) -> bool {
    crate::data::local_data_files(input, input_rel, rel)
        .iter()
        .any(|shown| {
            let full = input.join(shown.strip_prefix(input_rel).unwrap_or(shown));
            if extension(&full) != "json" {
                return true;
            }
            let Ok(text) = fs::read_to_string(&full) else {
                return true;
            };
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(serde_json::Value::Object(o)) => {
                    o.contains_key("templateEngineOverride") || o.contains_key("eleventyComputed")
                }
                Ok(_) => false,
                Err(_) => true,
            }
        })
}

/// Marks every file a read template could include, or name as its
/// layout, as read by both languages, until no more are.
fn includes(files: &mut [File], g: &Global) {
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut unknown = g.layout_unknown;
    let add = |n: &str, names: &mut BTreeSet<String>| {
        let last = n.rsplit(['/', '\\']).next().unwrap_or(n).to_lowercase();
        names.insert(last);
    };
    for n in &g.layouts {
        add(n, &mut names);
    }
    // Layouts named in front matter and JSON (data files); a data file
    // that runs code could name any.
    for f in files.iter() {
        let ext = extension(&f.rel);
        let mut found = Vec::new();
        if is_page_ext(&ext) {
            if let Ok((front, _)) = split_front_matter(&f.text) {
                layouts_in(
                    &Value::Object(std::sync::Arc::new(front)),
                    &mut found,
                    &mut unknown,
                );
            }
        } else if ext == "json" {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&f.text) {
                layouts_in(&Value::from_json(&v), &mut found, &mut unknown);
            }
        } else if matches!(ext.as_str(), "js" | "cjs" | "mjs" | "rhai") && f.text.contains("layout")
        {
            unknown = true;
        }
        for n in found {
            add(&n, &mut names);
        }
    }
    let mut seen: Vec<bool> = vec![false; files.len()];
    loop {
        if unknown {
            for f in files.iter_mut() {
                f.front = Readers::ANY;
                f.body = Readers::ANY;
            }
            return;
        }
        let mut grew = false;
        for (i, f) in files.iter().enumerate() {
            if seen[i] || !f.read() {
                continue;
            }
            seen[i] = true;
            for (text, _, readers) in f.parts() {
                if readers.some() && !included(text, &mut |n| add(n, &mut names)) {
                    unknown = true;
                }
            }
        }
        for (i, f) in files.iter_mut().enumerate() {
            let name = f
                .rel
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let named = names.iter().any(|n| {
                name == *n
                    || name
                        .strip_prefix(n.as_str())
                        .is_some_and(|r| r.starts_with('.'))
            });
            if named && (f.front != Readers::ANY || f.body != Readers::ANY) {
                f.front = Readers::ANY;
                f.body = Readers::ANY;
                seen[i] = false;
                grew = true;
            }
        }
        if !grew && !unknown {
            return;
        }
    }
}

/// The tags that make Liquid or Nunjucks read another file: Liquid's
/// `include`, `render` and `layout`, and Nunjucks' `include`, `extends`,
/// `import` and `from`.
const INCLUDE_TAGS: [&str; 6] = ["include", "render", "layout", "extends", "import", "from"];

/// How deep `{% liquid %}` tags nested as statements are read; deeper ones
/// could hold anything.
const MAX_LIQUID_DEPTH: usize = 16;

/// The name of the tag that starts `tag` (a [`Tag`]'s text), when it is
/// `name`: followed by anything but a letter, digit or `_`.
fn tag_named<'t>(tag: &'t str, name: &str) -> Option<&'t str> {
    tag.strip_prefix(name)
        .filter(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
}

/// One tag as Liquid or Nunjucks reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Tag<'t> {
    /// 1-based: the line of its `{%`, or of its statement.
    line: usize,
    /// From its name to its end: after `{%`, its whitespace control (`-`)
    /// and any spaces and line breaks, up to `%}` (or the text's end).
    text: &'t str,
    /// Whether a `%}` ends it.
    closed: bool,
    /// `{% liquid %}` tags nested deeper than [`MAX_LIQUID_DEPTH`]: what
    /// it holds isn't read, and could be anything.
    unknown: bool,
}

/// Every tag in `text`, the one place both scanners ([`tags`] and
/// [`included`]) find them: each `{%` (`{%-`), and each statement of a
/// Liquid `{% liquid %}` tag, which Liquid runs as a tag of its own
/// without `{%`, nested `liquid` statements included. A `{%` inside
/// another tag counts too: reading more never misses one.
fn all_tags(text: &str) -> Vec<Tag<'_>> {
    let mut out = Vec::new();
    let mut line = 1;
    let mut counted = 0;
    let mut rest = text;
    while let Some(at) = rest.find("{%") {
        let start = text.len() - rest.len() + at;
        line += text[counted..start].matches('\n').count();
        counted = start;
        rest = &rest[at + 2..];
        let (raw, closed) = match rest.find("%}") {
            Some(end) => (&rest[..end], true),
            None => (rest, false),
        };
        let tag = raw.trim_start_matches('-').trim_start();
        out.push(Tag {
            line,
            text: tag,
            closed,
            unknown: false,
        });
        if let Some(body) = tag_named(tag, "liquid") {
            let body_line = line + raw[..raw.len() - body.len()].matches('\n').count();
            statements(body, body_line, 1, &mut out);
        }
    }
    out
}

/// The statements of a `{% liquid %}` tag's `body`, which starts on line
/// `line`: one per line, as tags. A `liquid` statement's own statement is
/// the rest of its line.
fn statements<'t>(body: &'t str, line: usize, depth: usize, out: &mut Vec<Tag<'t>>) {
    for (i, statement) in body.split('\n').enumerate() {
        let text = statement.trim().trim_end_matches('-').trim_end();
        if text.is_empty() {
            continue;
        }
        out.push(Tag {
            line: line + i,
            text,
            closed: true,
            unknown: false,
        });
        if let Some(rest) = tag_named(text, "liquid") {
            if depth >= MAX_LIQUID_DEPTH {
                out.push(Tag {
                    line: line + i,
                    text,
                    closed: true,
                    unknown: true,
                });
            } else {
                statements(rest, line + i, depth + 1, out);
            }
        }
    }
}

/// The files `text`'s include tags ([`INCLUDE_TAGS`], as [`all_tags`]
/// finds them) name, given to `name`; false when one names its file with
/// anything but one string, which could be any file.
fn included(text: &str, name: &mut dyn FnMut(&str)) -> bool {
    for tag in all_tags(text) {
        if tag.unknown || (!tag.closed && tag_named(tag.text, "liquid").is_some()) {
            return false;
        }
        let Some(args) = INCLUDE_TAGS.iter().find_map(|t| tag_named(tag.text, t)) else {
            continue;
        };
        match literal(args).filter(|_| tag.closed) {
            Some(file) => name(file),
            None => return false,
        }
    }
    true
}

/// The file an include tag's arguments (up to the tag's end) name, when
/// they start with one string and go on only as a tag naming its file
/// that way can.
fn literal(args: &str) -> Option<&str> {
    let args = args.trim_start();
    let q = args.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let end = args[1..].find(q)?;
    let file = &args[1..1 + end];
    if file.contains('\\') || file.contains("{{") || file.contains("{%") || file.contains('\n') {
        return None;
    }
    let after = args[2 + end..].trim_start();
    let word = after
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()
        .unwrap_or("");
    let plain = after.trim_end().trim_end_matches('-').is_empty()
        || after.starts_with(',')
        || ["with", "for", "as", "import", "ignore", "only"].contains(&word)
        || (!word.is_empty() && after[word.len()..].trim_start().starts_with(':'));
    plain.then_some(file)
}

fn walk(dir: &Path, skip: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            let name = e.file_name();
            if name == "node_modules" || name == ".git" || is_same(&path, skip) {
                continue;
            }
            walk(&path, skip, out);
        } else if fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE) {
            out.push(path);
        }
    }
}

fn is_same(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The keys loading `language` (as a fence or a tag names it) changes.
/// A name a template writes could be any language: it changes every key
/// any language changes.
pub(crate) fn changes(language: &str) -> Option<Vec<String>> {
    let g = grammars();
    if language.contains("{{") || language.contains("{%") || language.contains('&') {
        let mut all: Vec<String> = Vec::new();
        for (_, keys) in &g.mutators {
            for k in keys {
                if !all.contains(k) {
                    all.push(k.clone());
                }
            }
        }
        return Some(all);
    }
    // `js/1-2`: the language is before the first `/`; `diff-x` loads `x`.
    let name = language.split('/').next().unwrap_or("");
    let name = name.strip_prefix("diff-").unwrap_or(name);
    g.mutated_by(&normalize(name)).map(<[String]>::to_vec)
}

/// The languages `text`'s fences could load, with their lines: the first
/// word of every fence's info string (markdown-it's `info.split(/\s+/)[0]`,
/// its backslash escapes removed). Looser than Markdown is: a fence
/// anywhere in a line counts. A word with a backslash left in it counts
/// both whole and cut there, as a string in a data file would be read.
fn fences(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        for fence in ["```", "~~~"] {
            let mut rest = line;
            while let Some(at) = rest.find(fence) {
                let after = rest[at..].trim_start_matches(fence.as_bytes()[0] as char);
                let info = after.trim_start_matches([' ', '\t']);
                let word: &str = info
                    .split(|c: char| c.is_whitespace() || (fence == "```" && c == '`'))
                    .next()
                    .unwrap_or("");
                if !word.is_empty() {
                    add_word(&mut out, i + 1, word);
                }
                rest = after;
            }
        }
    }
    out
}

fn add_word(out: &mut Vec<(usize, String)>, line: usize, word: &str) {
    let word = unescape(word);
    if let Some((head, _)) = word.split_once('\\') {
        out.push((line, head.to_string()));
    }
    out.push((line, word));
}

/// The languages `text`'s `{% highlight %}` tags (as [`all_tags`] finds
/// them, `highlight` statements of `{% liquid %}` tags included) could
/// load, with each tag's line: the first argument (quotes removed, for
/// Nunjucks). When Nunjucks may read the text (`nunjucks`), a tag whose
/// arguments aren't one string could load any language. So could a tag
/// whose arguments start on another line, one that never ends, and
/// `{% liquid %}` tags nested too deep to read.
fn tags(text: &str, nunjucks: bool) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for tag in all_tags(text) {
        if tag.unknown {
            add_word(&mut out, tag.line, &format!("{{{{ {} }}}}", tag.text));
            continue;
        }
        let Some(args) = tag.text.strip_prefix("highlight").filter(|a| {
            a.is_empty()
                || a.starts_with(char::is_whitespace)
                || a.trim_start_matches('-').trim().is_empty()
                || (nunjucks && a.starts_with(['"', '\'']))
        }) else {
            continue;
        };
        let written = args.trim().trim_end_matches('-').trim_end();
        let lead = &args[..args.len() - args.trim_start().len()];
        let word = written
            .split(|c: char| c.is_whitespace() || c == '%')
            .next()
            .unwrap_or("");
        let word = word.trim_matches(['"', '\'']);
        if !word.is_empty() {
            add_word(&mut out, tag.line, word);
        }
        let cut = lead.contains('\n') || (word.is_empty() && !tag.closed);
        if cut || (nunjucks && !word.is_empty() && !one_string(args)) {
            add_word(&mut out, tag.line, &format!("{{{{ {written} }}}}"));
        }
    }
    out
}

/// [`fences`] and [`tags`], in line order.
#[cfg(test)]
fn languages(text: &str, nunjucks: bool) -> Vec<(usize, String)> {
    let mut out = fences(text);
    out.extend(tags(text, nunjucks));
    out.sort_by_key(|(line, _)| *line);
    out
}

/// Whether Nunjucks tag arguments (up to the tag's end) are one string
/// literal without escapes.
fn one_string(args: &str) -> bool {
    let args = args.trim_start();
    let Some(q) = args.chars().next().filter(|c| *c == '"' || *c == '\'') else {
        return false;
    };
    let Some(end) = args[1..].find(q) else {
        return false;
    };
    let body = &args[1..1 + end];
    let after = args[2 + end..].trim();
    !body.contains('\\') && (after.is_empty() || after == "-")
}

/// markdown-it's backslash escapes: `\` before ASCII punctuation goes.
fn unescape(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut chars = word.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&n) = chars.peek()
            && n.is_ascii_punctuation()
        {
            out.push(n);
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// What other pages change, by `Prism.languages` key: the first file and
/// line (in path order) that names a language changing it, and that
/// language.
pub type Mutated = BTreeMap<String, (PathBuf, usize, String)>;

/// `found`, by the keys each changes.
pub fn by_key(found: &[Found]) -> Mutated {
    let mut out = Mutated::new();
    for f in found {
        for k in &f.keys {
            out.entry(k.clone())
                .or_insert_with(|| (f.path.clone(), f.line, f.language.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_and_tags_name_their_languages() {
        let text = "x\n```regex\n~~~ diff-jsdoc/1 more\n  > ```js/1-2\n{%- highlight \"css-extras\" -%}\n{% highlight php-extras 1 %}\n`not` ``a``\n";
        let got: Vec<(usize, String)> = languages(text, false);
        assert_eq!(
            got,
            vec![
                (2, "regex".to_string()),
                (3, "diff-jsdoc/1".to_string()),
                (4, "js/1-2".to_string()),
                (5, "css-extras".to_string()),
                (6, "php-extras".to_string()),
            ]
        );
    }

    #[test]
    fn a_language_from_a_string_counts_cut_at_a_backslash() {
        assert_eq!(
            languages(r#"{"md": "```regex\nx\n```"}"#, false),
            vec![
                (1, "regex".to_string()),
                (1, r"regex\nx\n".to_string()),
                (1, "\"}".to_string())
            ]
        );
        assert_eq!(
            languages(r"```reg\-ex", false),
            vec![(1, "reg-ex".to_string())]
        );
    }

    #[test]
    fn nunjucks_arguments_other_than_one_string_could_be_any_language() {
        let text = "{% highlight \"regex 1\" %}\n{% highlight lang %}\n{%- highlight \"js\" ~ x -%}\n{% highlight'css' -%}\n{% highlight\n";
        let got = languages(text, true);
        assert_eq!(
            got,
            vec![
                (1, "regex".to_string()),
                (2, "lang".to_string()),
                (2, "{{ lang }}".to_string()),
                (3, "js".to_string()),
                (3, "{{ \"js\" ~ x }}".to_string()),
                (4, "css".to_string()),
                (5, "{{  }}".to_string()),
            ]
        );
        for (_, l) in &got[2..] {
            if l.starts_with("{{") {
                assert!(changes(l).is_some_and(|k| k.iter().any(|k| k == "javascript")));
            }
        }
        // Liquid reads the first word as written; only a tag whose
        // arguments start on another line could be any language, not one
        // without arguments.
        assert_eq!(
            languages(
                "{% highlight lang %}\n{% highlight\n  js %}\n{% highlight %}x{% endhighlight %}\n",
                false
            ),
            vec![
                (1, "lang".to_string()),
                (2, "js".to_string()),
                (2, "{{ js }}".to_string())
            ]
        );
        // A tag that never ends could be any language.
        assert_eq!(
            languages("{% highlight", false),
            vec![(1, "{{  }}".to_string())]
        );
    }

    /// Both languages read a tag's name after line breaks, and its
    /// arguments across lines: the tag counts, at the line of its `{%`.
    #[test]
    fn a_tag_split_across_lines_counts() {
        for nunjucks in [false, true] {
            assert_eq!(
                tags(
                    "x\n{%\n  highlight regex %}/a/{% endhighlight %}\n",
                    nunjucks
                )[0],
                (2, "regex".to_string()),
                "{nunjucks}"
            );
            // Arguments that start on another line could be any language.
            assert!(
                tags("{%-\r\n\thighlight\n\"css-extras\"\n-%}", nunjucks)
                    .contains(&(1, "{{ \"css-extras\" }}".to_string())),
                "{nunjucks}"
            );
        }
        // Lines after a split tag keep their numbers.
        assert_eq!(
            tags("{%\nhighlight js %}\n\n{% highlight regex %}", false),
            vec![(1, "js".to_string()), (4, "regex".to_string())]
        );
    }

    /// The files `scan` finds a mutating language in, for a site of
    /// `files` (relative to the input folder) with `markdownTemplateEngine`
    /// `markdown` and the plugin's formats.
    fn found_in(
        files: &[(&str, &str)],
        markdown: Option<Lang>,
        formats: (bool, bool),
        global: &Data,
    ) -> Vec<String> {
        let dir = tempfile::tempdir().expect("tempdir");
        for (rel, text) in files {
            let path = dir.path().join(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            fs::write(path, text).expect("write");
        }
        let cx = Context {
            engines: TemplateEngines {
                markdown,
                html: Some(Lang::Liquid),
            },
            liquid_tag: formats.0,
            nunjucks_tag: formats.1,
            global_data: global,
            includes: Path::new("_includes"),
            layouts: Path::new("_includes"),
            data: Path::new("_data"),
        };
        let mut got: Vec<String> =
            scan(dir.path(), Path::new("src"), &dir.path().join("_site"), cx)
                .into_iter()
                .map(|f| format!("{}:{}", f.path.display(), f.line))
                .collect();
        got.dedup();
        got
    }

    const BOTH: (bool, bool) = (true, true);

    /// A changelog that quotes tags, in a site where Markdown runs no
    /// template language: nothing reads them.
    const CHANGELOG: &str = "# Changes\n\n- `{% highlight js 1,3 %}` in `.njk` files\n- `{% highlight regex %}` too\n\n```rust\nfn main() {}\n```\n";

    #[test]
    fn tags_count_only_where_a_language_with_the_tag_reads_them() {
        let none = Data::new();
        let changelog = [("CHANGELOG.md", CHANGELOG)];
        assert!(found_in(&changelog, None, BOTH, &none).is_empty());
        // Liquid reads `js` from the first; Nunjucks could read any.
        assert_eq!(
            found_in(&changelog, Some(Lang::Liquid), BOTH, &none),
            ["src/CHANGELOG.md:4"]
        );
        assert_eq!(
            found_in(&changelog, Some(Lang::Nunjucks), BOTH, &none),
            ["src/CHANGELOG.md:3", "src/CHANGELOG.md:4"]
        );
        // Liquid reads it, but the plugin gave only Nunjucks the tag.
        assert!(found_in(&changelog, Some(Lang::Liquid), (false, true), &none).is_empty());
        // `.html` is Liquid here; a `.njk` file is always Nunjucks.
        assert_eq!(
            found_in(&[("a.html", "{% highlight regex %}\n")], None, BOTH, &none),
            ["src/a.html:1"]
        );
        assert!(
            found_in(
                &[("a.njk", "{% highlight regex %}\n")],
                None,
                (true, false),
                &none
            )
            .is_empty()
        );
        // Fences count anywhere.
        assert_eq!(
            found_in(&[("a.md", "x\n```regex\n```\n")], None, BOTH, &none),
            ["src/a.md:2"]
        );
    }

    #[test]
    fn an_override_decides_like_the_build() {
        let none = Data::new();
        let page = |over: &str| {
            format!("---\ntemplateEngineOverride: {over}\n---\n{{% highlight regex %}}\n")
        };
        for (over, found) in [
            ("njk,md", true),
            ("liquid", true),
            ("md", false),
            ("false", false),
            ("html", false),
        ] {
            let text = page(over);
            let got = found_in(&[("a.md", &text)], None, BOTH, &none);
            assert_eq!(got.len(), usize::from(found), "{over}: {got:?}");
            if found {
                assert_eq!(got, ["src/a.md:4"]);
            }
        }
        // With `md` alone, Liquid still reads the front matter's values.
        let text = "---\ntemplateEngineOverride: md\npermalink: \"{% highlight regex %}\"\n---\n{% highlight regex %}\n";
        assert_eq!(
            found_in(&[("a.liquid", text)], Some(Lang::Liquid), BOTH, &none),
            ["src/a.liquid:3"]
        );
        // An override the build refuses: either language could.
        let text = page("[1]");
        assert_eq!(found_in(&[("a.md", &text)], None, BOTH, &none).len(), 1);
    }

    #[test]
    fn data_that_could_set_the_override_counts_as_either_language() {
        let none = Data::new();
        let changelog = ("docs/CHANGELOG.md", CHANGELOG);
        for data in [
            ("docs/docs.json", "{\"templateEngineOverride\": \"njk,md\"}"),
            ("docs/docs.json", "{\"eleventyComputed\": {}}"),
            ("docs/docs.json", "not json"),
            ("docs/CHANGELOG.11tydata.js", "module.exports = {};"),
            ("docs/CHANGELOG.11tydata.rhai", "#{}"),
            ("_data/templateEngineOverride.json", "\"njk\""),
        ] {
            assert!(
                !found_in(&[changelog, data], None, BOTH, &none).is_empty(),
                "{data:?}"
            );
        }
        // Data that doesn't set it leaves the page alone.
        assert!(
            found_in(
                &[changelog, ("docs/docs.json", "{\"tags\": \"x\"}")],
                None,
                BOTH,
                &none
            )
            .is_empty()
        );
        let mut global = Data::new();
        global.insert("templateEngineOverride".into(), Value::String("njk".into()));
        assert!(!found_in(&[changelog], None, BOTH, &global).is_empty());
    }

    #[test]
    fn a_file_a_template_could_include_counts_as_either_language() {
        let none = Data::new();
        let changelog = ("CHANGELOG.md", CHANGELOG);
        for page in [
            ("a.njk", "{% include \"../CHANGELOG.md\" %}"),
            ("a.liquid", "{% render 'changelog.md', x: 1 %}"),
            ("a.njk", "{% include name %}"),
            ("a.njk", "{% include \"a\" ~ b %}"),
            ("_includes/x.njk", "{% include page.inputPath %}"),
            ("a.md", "---\nlayout: ../CHANGELOG.md\n---\n"),
            ("a.md", "---\nlayout: \"{{ x }}\"\n---\n"),
            ("a.11tydata.js", "module.exports = { layout: x };"),
            // A `{% liquid %}` tag's statements include without `{%`.
            ("a.liquid", "{% liquid include \"CHANGELOG.md\" %}"),
            (
                "a.liquid",
                "{%- liquid\n  assign x = 1\n  render 'changelog.md', y: 2\n-%}",
            ),
            ("a.liquid", "{% liquid\n  layout name\n%}"),
            ("a.liquid", "{% liquid include \"x\" | append: y %}"),
            ("a.liquid", "{% liquid include \"x.md\""),
            // Line breaks after `{%` and the tag's name.
            ("a.njk", "{%\n  include\n  \"../CHANGELOG.md\" %}"),
            ("a.liquid", "{%-\r\nlayout \"CHANGELOG.md\" %}"),
            ("a.njk", "{% from\n\"CHANGELOG.md\" import x %}"),
        ] {
            assert!(
                !found_in(&[changelog, page], None, BOTH, &none).is_empty(),
                "{page:?}"
            );
        }
        // Includes in text nothing reads, or naming other files, don't;
        // nor do tags whose names only start like one.
        for page in [
            ("a.md", "`{% include name %}`"),
            ("a.md", "`{% liquid include name %}`"),
            ("a.liquid", "{% liquidish %}{% included x %}{% fromage %}"),
            (
                "a.liquid",
                "{% liquid\n  assign x = 1\n  include 'other.md'\n%}",
            ),
            ("a.liquid", "{% liquid echo include %}"),
            ("a.njk", "{% include \"other.md\" %}"),
            ("a.md", "---\nlayout: base.njk\n---\n"),
        ] {
            assert!(
                found_in(&[changelog, page], None, BOTH, &none).is_empty(),
                "{page:?}"
            );
        }
        // Includes chain: a file one included file includes.
        let chain = [
            changelog,
            ("a.njk", "{% include \"notes.txt\" %}"),
            ("notes.txt", "{% include \"CHANGELOG.md\" %}"),
        ];
        assert!(!found_in(&chain, None, BOTH, &none).is_empty());
    }

    /// The review's repro: a page includes a Markdown file that nothing
    /// else reads from a `{% liquid %}` tag, and a tag in it loads
    /// `regex`; and the same tag split after `{%` in the page itself.
    #[test]
    fn the_review_repros_count() {
        let none = Data::new();
        let notes = (
            "notes.md",
            "- `{% highlight regex %}/a/{% endhighlight %}`\n",
        );
        let page = (
            "a.liquid",
            "{% highlight c %}int x;{% endhighlight %}\n{% liquid include \"notes.md\" %}\n",
        );
        assert!(
            found_in(&[page, notes], None, BOTH, &none).contains(&"src/notes.md:1".to_string())
        );
        let page = (
            "a.liquid",
            "{% highlight c %}int x;{% endhighlight %}\n{%\n  highlight regex %}/a/{% endhighlight %}\n",
        );
        assert_eq!(found_in(&[page], None, BOTH, &none), ["src/a.liquid:2"]);
    }

    /// Both scanners read their tags from `all_tags`: each `{%`, and each
    /// statement of a `{% liquid %}` tag, nested ones included.
    #[test]
    fn every_tag_is_found_in_one_place() {
        let text =
            "a {%- if x -%}\n{% liquid\n  # c\n  liquid include 'n.md'\n\n  echo 1 -%}{%\nendif";
        let got: Vec<(usize, &str, bool, bool)> = all_tags(text)
            .iter()
            .map(|t| (t.line, t.text, t.closed, t.unknown))
            .collect();
        assert_eq!(
            got,
            [
                (1, "if x -", true, false),
                (
                    2,
                    "liquid\n  # c\n  liquid include 'n.md'\n\n  echo 1 -",
                    true,
                    false
                ),
                (3, "# c", true, false),
                (4, "liquid include 'n.md'", true, false),
                (4, "include 'n.md'", true, false),
                (6, "echo 1", true, false),
                (6, "endif", false, false),
            ]
        );
        // Nested past the cap: unknown, which both scanners count as any.
        let deep = format!(
            "{{% {}include 'x' %}}",
            "liquid ".repeat(MAX_LIQUID_DEPTH + 1)
        );
        assert!(all_tags(&deep).iter().any(|t| t.unknown));
        assert!(!included(&deep, &mut |_| {}));
        assert!(tags(&deep, false).iter().any(|(_, l)| l.starts_with("{{")));
    }

    /// Re-review 1: a `highlight` statement in a `{% liquid %}` tag, and an
    /// include in a `liquid` statement nested in one, both count.
    #[test]
    fn liquid_statements_are_tags() {
        let none = Data::new();
        let page = (
            "a.liquid",
            "{% highlight c %}int x;{% endhighlight %}\n{% liquid\nhighlight regex\necho \"/a/\"\nendhighlight\n%}\n",
        );
        assert_eq!(found_in(&[page], None, BOTH, &none), ["src/a.liquid:3"]);
        let notes = (
            "notes.md",
            "- `{% highlight regex %}/a/{% endhighlight %}`\n",
        );
        let page = (
            "a.liquid",
            "{% highlight c %}int x;{% endhighlight %}\n{% liquid\nliquid include \"notes.md\"\n%}\n",
        );
        assert!(
            found_in(&[page, notes], None, BOTH, &none).contains(&"src/notes.md:1".to_string())
        );
        // A nested include of a variable could be any file.
        let page = ("a.liquid", "{% liquid\n  liquid include name\n%}\n");
        assert!(!found_in(&[page, notes], None, BOTH, &none).is_empty());
    }

    #[test]
    fn languages_that_change_grammars_brrrty_has() {
        let keys = changes("regex").unwrap();
        assert!(keys.iter().any(|k| k == "javascript"));
        assert!(!keys.iter().any(|k| k == "bash"));
        assert!(changes("diff-regex/1").is_some());
        assert_eq!(changes("css-extras"), changes("css-extras/2"));
        assert!(changes("js").is_none());
        assert!(changes("java").is_some());
        assert!(changes("kotlin").is_none());
        assert!(changes("REGEX").is_none());
        let any = changes("{{ lang }}").unwrap();
        assert!(any.iter().any(|k| k == "bash"));
    }
}
