//! `@11ty/eleventy-plugin-syntaxhighlight` 5.0.2: Markdown fences and the
//! Liquid `{% highlight %}` tag, highlighted with Prism 1.30.0
//! ([`brrrty_prism`]).
//!
//! The plugin's files, and where each lives here:
//!
//! - `markdownSyntaxHighlightOptions.js` is [`Page::markdown`], markdown-it's
//!   `highlight` option on 11ty's own instance (not on one a config makes);
//! - `HighlightPairedShortcode.js` is [`Page::paired`], and
//!   `LiquidHighlightTag.js` is [`SyntaxHighlight::liquid_tag`];
//! - `PrismLoader.js` and `PrismNormalizeAlias.js` are [`Page::loader`] and
//!   [`normalize`];
//! - `HighlightLines.js` and `HighlightLinesGroup.js` are in [`lines`];
//! - `getAttributes.js` is [`attributes`].
//!
//! Prism's `Prism.languages` is one object for the whole build in 11ty: a
//! language is there once any page loaded it, and a hook that looks one up
//! sees whatever pages rendered first. Brrrty knows what is surely there
//! (what Prism starts with, the `languages` option, and what this page
//! loaded so far, with everything each load brings), and refuses output
//! that would depend on the rest.

mod lines;

use std::collections::{HashMap, HashSet};

use brrrty_js::Value;
use brrrty_prism::grammar::{GrammarId, grammars};
use brrrty_prism::{AllLoaded, Languages, Refusal};

use lines::HighlightLinesGroup;

/// Why code can't be highlighted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Brrrty can't reproduce what 11ty writes here.
    Unsupported(String),
    /// The plugin throws here, which stops 11ty's build.
    Thrown(String),
}

/// HTML attributes for `<pre>` or `<code>` (`preAttributes`,
/// `codeAttributes`), in JavaScript's key order. `None` is a falsy option:
/// only the language class.
pub type Attributes = Option<Vec<(String, Value)>>;

/// The plugin's options, as `Object.assign` leaves them over its defaults.
#[derive(Debug, Clone)]
pub struct Options {
    /// `lineSeparator`: what joins the lines; empty means `<br>`.
    pub line_separator: String,
    pub error_on_invalid_language: bool,
    pub always_wrap_line_highlights: bool,
    pub pre_attributes: Attributes,
    pub code_attributes: Attributes,
    /// `trim`: whether `{% highlight %}` trims its code a second time.
    pub trim: bool,
    /// `languages`: loaded when the plugin is added.
    pub languages: Vec<String>,
    /// `templateFormats`, as a list (a string is a list of one; anything
    /// that isn't a string never matches). `None` is the default, every
    /// format.
    pub template_formats: Option<Vec<String>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            line_separator: "\n".into(),
            error_on_invalid_language: false,
            always_wrap_line_highlights: false,
            pre_attributes: Some(Vec::new()),
            code_attributes: Some(Vec::new()),
            trim: true,
            languages: Vec::new(),
            template_formats: None,
        }
    }
}

impl Options {
    /// `hasTemplateFormat(options.templateFormats, format)`.
    pub fn has_format(&self, format: &str) -> bool {
        match &self.template_formats {
            None => true,
            Some(list) => list.iter().any(|f| f == "*" || f == format),
        }
    }
}

/// The plugin as a config added it.
#[derive(Debug, Clone)]
pub struct SyntaxHighlight {
    pub options: Options,
    /// What `Prism.languages` surely has once the plugin is added.
    loaded: HashSet<String>,
}

impl SyntaxHighlight {
    /// The plugin with `options`: `Err` when a language it loads is one
    /// Brrrty doesn't have.
    pub fn new(options: Options) -> Result<Self, String> {
        let mut plugin = Self {
            options,
            loaded: grammars().base_keys.iter().cloned().collect(),
        };
        let mut page = plugin.page();
        // `PrismLoader(language)`, without the options.
        page.error_on_invalid = false;
        for language in &plugin.options.languages {
            page.loader(language).map_err(|e| match e {
                Error::Unsupported(why) | Error::Thrown(why) => {
                    format!(
                        "add_plugin(\"syntaxhighlight\", ...): `languages` has `{language}`: {why}"
                    )
                }
            })?;
        }
        let loaded = page.state.keys;
        plugin.loaded = loaded;
        Ok(plugin)
    }

    /// Highlighting for one page: what it loads is known to be loaded for
    /// the rest of it.
    pub fn page(&self) -> Page<'_> {
        Page {
            options: &self.options,
            error_on_invalid: self.options.error_on_invalid_language,
            state: State {
                keys: self.loaded.clone(),
                added: HashMap::new(),
            },
        }
    }

    /// `LiquidHighlightTag`'s `render`: `{% highlight js 1,3 %}`. The
    /// arguments are split on single spaces, the first being the language;
    /// the body is trimmed.
    pub fn liquid_tag(&self, args: &str, body: &str) -> Result<String, Error> {
        let mut split = args.split(' ');
        let language = split.next().unwrap_or("");
        let numbers: Vec<&str> = split.collect();
        let body = js_trim(body);
        self.page().paired(body, language, &numbers.join(" "))
    }
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(brrrty_js::is_js_whitespace)
}

/// Names `Prism.languages` or Prism's components list have that aren't
/// grammars (`Object.prototype`'s, and Prism's own functions): 11ty uses
/// whatever they hold, or throws.
const OBJECT_KEYS: &[&str] = &[
    "extend",
    "insertBefore",
    "DFS",
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "__proto__",
    "toLocaleString",
    // components.json's own entry, not a language: loading it throws.
    "meta",
];

/// `PrismNormalizeAlias`: a Prism component's id for `language`, an alias
/// of one, or `language` as it is.
pub fn normalize(language: &str) -> String {
    let language = match language {
        "njk" | "nunjucks" => "jinja2",
        l => l,
    };
    let components = &grammars().components;
    if components.iter().any(|(id, _)| id == language) {
        return language.to_string();
    }
    components
        .iter()
        .find(|(_, aliases)| aliases.iter().any(|a| a == language))
        .map_or_else(|| language.to_string(), |(id, _)| id.clone())
}

/// Whether some page could have put `name` in `Prism.languages`: a
/// component or one of its aliases, a grammar Brrrty has, or a `diff-`
/// name (the plugin and the diff-highlight hook add those).
fn could_be_loaded(name: &str) -> bool {
    grammars().language(name).is_some()
        || grammars()
            .components
            .iter()
            .any(|(id, aliases)| id == name || aliases.iter().any(|a| a == name))
        || name.to_ascii_lowercase().starts_with("diff-")
}

/// `Prism.languages`, as far as one page can know it.
#[derive(Debug, Clone)]
struct State {
    /// Names surely loaded.
    keys: HashSet<String>,
    /// Names set to a grammar (`diff-<x>`).
    added: HashMap<String, GrammarId>,
}

impl Languages for State {
    fn get(&self, name: &str) -> Result<Option<GrammarId>, Refusal> {
        if let Some(&g) = self.added.get(name) {
            return Ok(Some(g));
        }
        if self.keys.contains(name) {
            return Ok(grammars().language(name));
        }
        if could_be_loaded(name) {
            return Err(Refusal(format!(
                "Prism looks up `{name}` here, and whether 11ty has loaded it by then depends on the order it renders pages in"
            )));
        }
        AllLoaded::default().get(name)
    }

    fn set(&mut self, name: &str, grammar: GrammarId) {
        self.added.insert(name.to_string(), grammar);
    }
}

/// Highlighting on one page.
pub struct Page<'a> {
    options: &'a Options,
    error_on_invalid: bool,
    state: State,
}

impl Page<'_> {
    /// `PrismLoader(language, options)`: the grammar to highlight with,
    /// loading the language first; `None` when Prism has none.
    fn loader(&mut self, language: &str) -> Result<Option<GrammarId>, Error> {
        let diff = language.strip_prefix("diff-");
        // Aliasing ignores `diff-`.
        let aliased = normalize(diff.unwrap_or(language));
        if OBJECT_KEYS.contains(&aliased.as_str()) {
            return Err(Error::Unsupported(format!(
                "`{aliased}` names a property of Prism's objects, not a language"
            )));
        }
        let g = grammars();
        if g.components.iter().any(|(id, _)| *id == aliased) {
            // `loadLanguages(aliased)` when Prism doesn't have it yet; once
            // it has, everything loading it brings is there too.
            if g.language(&aliased).is_none() {
                return Err(Error::Unsupported(format!(
                    "Brrrty doesn't have Prism's `{aliased}` grammar yet"
                )));
            }
            self.state.keys.insert(aliased.clone());
            if let Some(keys) = g.keys_after_loading(&aliased) {
                self.state.keys.extend(keys.iter().cloned());
            }
        }
        let found = self
            .state
            .get(&aliased)
            .map_err(|r| Error::Unsupported(r.0))?;
        if self.error_on_invalid && found.is_none() {
            return Err(Error::Thrown(format!(
                "\"{language}\" is not a valid Prism.js language for eleventy-plugin-syntaxhighlight"
            )));
        }
        if diff.is_none() {
            return Ok(found);
        }
        let Some(d) = g.language("diff") else {
            return Err(Error::Unsupported(
                "Brrrty's Prism grammars have no `diff` grammar, which `diff-` languages need"
                    .into(),
            ));
        };
        self.state.set(&format!("diff-{aliased}"), d);
        // The diff-highlight hook names `language` (when it matches
        // /^diff-([\w-]+)/i) the diff grammar too before it tokenizes; it is
        // nothing else when Prism has it already.
        if diff
            .unwrap_or("")
            .starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            self.state.set(language, d);
        }
        Ok(Some(d))
    }

    /// `Prism.highlight(code, grammar, language)`.
    fn prism(&mut self, code: &str, grammar: GrammarId, language: &str) -> Result<String, Error> {
        let html = brrrty_prism::highlight(code, grammar, language, &mut self.state)
            .map_err(|r| Error::Unsupported(r.0))?;
        // prism-markdown is loaded again when YAML is loaded after it, which
        // adds its hooks a second time: a code block in a language then has
        // that class twice. Whether that happened, and whether its front
        // matter is YAML, depends on what other pages loaded first.
        if html.contains("token code-block language-") {
            return Err(Error::Unsupported(
                "it has Markdown with a fenced code block in a language, which 11ty highlights one way or another depending on the order it renders pages in".into(),
            ));
        }
        if html.contains("token front-matter") && !self.state.keys.contains("yaml") {
            return Err(Error::Unsupported(
                "it has Markdown with front matter, which 11ty highlights as YAML only when a page it rendered earlier loaded YAML".into(),
            ));
        }
        Ok(html)
    }

    /// The code highlighted in `language`, or as it is (not escaped) for
    /// `text` and for a language Prism doesn't have.
    fn code(&mut self, code: &str, language: &str) -> Result<String, Error> {
        if language == "text" {
            return Ok(code.to_string());
        }
        match self.loader(language)? {
            Some(grammar) => self.prism(code, grammar, language),
            None => Ok(code.to_string()),
        }
    }

    /// `<pre ...><code ...>` around `lines`.
    fn wrap(&self, lines: &[String], language: &str) -> Result<String, Error> {
        let pre = attributes(&self.options.pre_attributes, language)?;
        let code = attributes(&self.options.code_attributes, language)?;
        let separator = match self.options.line_separator.as_str() {
            "" => "<br>",
            s => s,
        };
        Ok(format!(
            "<pre{pre}><code{code}>{}</code></pre>",
            lines.join(separator)
        ))
    }

    /// markdown-it's `highlight(str, language)`: `language` is the info
    /// string's first word, `js/1-2` with lines to mark.
    pub fn markdown(&mut self, code: &str, language: &str) -> Result<String, Error> {
        let mut split = language.split('/');
        let language = split.next().unwrap_or("");
        let numbers: Vec<&str> = split.collect();
        let html = self.code(code, language)?;
        let wrap = self.options.always_wrap_line_highlights || !numbers.is_empty();
        let group = HighlightLinesGroup::new(&numbers.join("/"), "/")?;
        let mut lines: Vec<&str> = html.split('\n').collect();
        if lines.last() == Some(&"") {
            lines.pop();
        }
        let lines: Vec<String> = lines
            .iter()
            .enumerate()
            .map(|(j, line)| {
                if wrap {
                    group.line_markup(j, line)
                } else {
                    line.to_string()
                }
            })
            .collect();
        self.wrap(&lines, language)
    }

    /// `HighlightPairedShortcode(content, language, highlightNumbers)`.
    pub fn paired(
        &mut self,
        content: &str,
        language: &str,
        numbers: &str,
    ) -> Result<String, Error> {
        let content = if self.options.trim {
            js_trim(content)
        } else {
            content
        };
        let html = self.code(content, language)?;
        let group = HighlightLinesGroup::new(numbers, " ")?;
        let wrap = self.options.always_wrap_line_highlights || !numbers.is_empty();
        // `split(/\r?\n/)`.
        let pieces: Vec<&str> = html.split('\n').collect();
        let last = pieces.len() - 1;
        let lines: Vec<String> = pieces
            .iter()
            .enumerate()
            .map(|(j, piece)| {
                let line = if j < last {
                    piece.strip_suffix('\r').unwrap_or(piece)
                } else {
                    piece
                };
                if wrap {
                    group.line_markup(j, line)
                } else {
                    line.to_string()
                }
            })
            .collect();
        self.wrap(&lines, language)
    }
}

/// Whether `key` is an array index, which JavaScript lists before other
/// keys, in numeric order.
fn is_array_index(key: &str) -> bool {
    key == "0"
        || (!key.starts_with('0')
            && !key.is_empty()
            && key.len() <= 10
            && key.bytes().all(|b| b.is_ascii_digit())
            && key.parse::<u64>().is_ok_and(|n| n < 4_294_967_295))
}

/// `Object.entries` order: array indexes first, by number, then the rest
/// as added.
fn js_order(mut entries: Vec<(String, Value)>) -> Vec<(String, Value)> {
    let (mut indexes, rest): (Vec<_>, Vec<_>) =
        entries.drain(..).partition(|(k, _)| is_array_index(k));
    indexes.sort_by_key(|(k, _)| k.parse::<u64>().unwrap_or(0));
    indexes.extend(rest);
    indexes
}

/// A JavaScript `typeof`, for the plugin's message.
fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Undefined => "undefined",
        Value::Null | Value::Array(_) | Value::Object(_) | Value::Date(_) => "object",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
    }
}

/// `getAttributes(attributes, { language })`: ` key="value"` for each,
/// the language's class first unless there is a `class`.
pub fn attributes(attrs: &Attributes, language: &str) -> Result<String, Error> {
    let lang_class = (!language.is_empty()).then(|| format!("language-{language}"));
    let Some(list) = attrs else {
        return Ok(lang_class.map_or_else(String::new, |c| format!(" class=\"{c}\"")));
    };
    let mut list = list.clone();
    if let Some(c) = lang_class
        && !list.iter().any(|(k, _)| k == "class")
    {
        // "class attribute should be first in order".
        list.insert(0, ("class".to_string(), Value::String(c.into())));
    }
    let list = js_order(list);
    let mut out = String::new();
    for (key, value) in &list {
        let text = match value {
            Value::String(s) => s.to_string(),
            Value::Number(_) => value.to_js_string(),
            other => {
                return Err(Error::Thrown(format!(
                    "Attribute \"{key}\" must have, or evaluate to, a value of type string or number, not \"{}\".",
                    type_of(other)
                )));
            }
        };
        out.push_str(&format!(" {key}=\"{text}\""));
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
