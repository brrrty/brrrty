//! `Prism.util.encode` and `Token.stringify` (prism-core.js), with the
//! `wrap` hooks of the components the plugin loads, in the order they are
//! added: markup's entity titles (core), diff-highlight's line prefixes (the
//! plugin always loads it) and markdown's code blocks. Each acts on token
//! types the others don't, so their order never shows.

use crate::highlight::{Highlighter, PREFIXES, hook_regex, js_replace, units};
use crate::tokenize::{Content, Item, Refusal, Token, Units};

/// `Prism.util.encode`: `&` and `<` escaped and no-break spaces made plain,
/// in every string of the stream.
pub fn encode(items: &mut [Item]) {
    for item in items {
        match item {
            Item::Str(s) => *s = encode_units(s),
            Item::Token(t) => encode_token(t),
        }
    }
}

fn encode_token(t: &mut Token) {
    match &mut t.content {
        Content::Str(s) => *s = encode_units(s),
        Content::Items(xs) => encode(xs),
    }
}

/// `encode` of one string.
pub fn encode_units(s: &[u16]) -> Units {
    let mut out = Vec::with_capacity(s.len());
    for &u in s {
        match u {
            0x26 => out.extend(units("&amp;")),
            0x3C => out.extend(units("&lt;")),
            0xA0 => out.push(0x20),
            _ => out.push(u),
        }
    }
    out
}

/// What a `wrap` hook sees and may change.
pub(crate) struct WrapEnv<'a> {
    pub kind: &'a str,
    pub content: Units,
    pub classes: Vec<String>,
    /// `attributes`, in the order they were set.
    pub attributes: Vec<(String, Units)>,
    pub language: &'a str,
}

/// Replace the first occurrence of `from` in `s` by `to`.
fn replace_first(s: &[u16], from: &str, to: &str) -> Units {
    let from = units(from);
    match s.windows(from.len()).position(|w| w == from.as_slice()) {
        Some(i) => {
            let mut out = s[..i].to_vec();
            out.extend(units(to));
            out.extend_from_slice(&s[i + from.len()..]);
            out
        }
        None => s.to_vec(),
    }
}

/// Replace every occurrence of `from` in `s` by `to`, left to right.
pub(crate) fn replace_all_str(s: &[u16], from: &str, to: &str) -> Units {
    let from = units(from);
    let to = units(to);
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i..].starts_with(&from) {
            out.extend_from_slice(&to);
            i += from.len();
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

impl Highlighter<'_> {
    /// `Token.stringify` of a stream.
    pub(crate) fn stringify(&mut self, items: &[Item], language: &str) -> Result<Units, Refusal> {
        let mut out = Vec::new();
        for item in items {
            match item {
                Item::Str(s) => out.extend_from_slice(s),
                Item::Token(t) => out.extend(self.stringify_token(t, language)?),
            }
        }
        Ok(out)
    }

    /// `Token.stringify` of one token: its content, then the `wrap` hooks,
    /// then the element.
    pub(crate) fn stringify_token(&mut self, t: &Token, language: &str) -> Result<Units, Refusal> {
        let content = match &t.content {
            Content::Str(s) => s.clone(),
            Content::Items(xs) => self.stringify(xs, language)?,
        };
        let mut classes = vec!["token".to_string(), t.kind.clone()];
        classes.extend(t.alias.iter().cloned());
        let mut env = WrapEnv {
            kind: &t.kind,
            content,
            classes,
            attributes: Vec::new(),
            language,
        };
        self.wrap_entity(&mut env);
        self.wrap_diff(&mut env)?;
        self.wrap_markdown(&mut env)?;

        let mut out = units("<span class=\"");
        out.extend(units(&env.classes.join(" ")));
        out.push(u16::from(b'"'));
        for (name, value) in &env.attributes {
            out.push(u16::from(b' '));
            out.extend(units(name));
            out.extend(units("=\""));
            out.extend(replace_all_str(value, "\"", "&quot;"));
            out.push(u16::from(b'"'));
        }
        out.push(u16::from(b'>'));
        out.extend(env.content);
        out.extend(units("</span>"));
        Ok(out)
    }

    /// prism-markup.js: an entity gets its text as a title.
    fn wrap_entity(&mut self, env: &mut WrapEnv<'_>) {
        if env.kind == "entity" {
            let title = replace_first(&env.content, "&amp;", "&");
            env.attributes.push(("title".into(), title));
        }
    }

    /// prism-diff-highlight.js: in `diff` and `diff-<language>`, each block
    /// of changed lines is written line by line behind its prefix, the code
    /// highlighted as `<language>` when Prism has it.
    fn wrap_diff(&mut self, env: &mut WrapEnv<'_>) -> Result<(), Refusal> {
        let mut diff_language = None;
        let mut diff_grammar = None;
        if env.language != "diff" {
            let lang = units(env.language);
            let Some(m) = self.exec(hook_regex::LANGUAGE, &lang, 0)? else {
                return Ok(());
            };
            let (a, b) = m.groups[0].expect("group 1 is always set");
            let name = String::from_utf16_lossy(&lang[a..b]);
            diff_grammar = self.languages.get(&name)?;
            diff_language = Some(name);
        }
        let Some(&(_, prefix_text, word)) = PREFIXES.iter().find(|(k, _, _)| *k == env.kind) else {
            return Ok(());
        };
        let content = js_replace(self, hook_regex::HTML_TAG, &env.content, true, |_, _, _| {
            Ok(Vec::new())
        })?;
        let decoded = replace_all_str(&replace_all_str(&content, "&lt;", "<"), "&amp;", "&");
        let code = js_replace(self, hook_regex::LINE_PREFIX, &decoded, true, |m, s, _| {
            let (a, b) = m.groups[0].expect("group 1 is always set");
            Ok(s[a..b].to_vec())
        })?;
        let highlighted = match (diff_grammar, &diff_language) {
            (Some(g), Some(lang)) => self.highlight_units(&code, g, lang)?,
            _ => encode_units(&code),
        };
        let prefix_token = Token {
            kind: "prefix".into(),
            content: Content::Str(units(prefix_text)),
            alias: vec![word.into()],
            length: 0,
        };
        let prefix = self.stringify_token(&prefix_token, env.language)?;
        let mut lines = Vec::new();
        let mut last = 0;
        while let Some(m) = self.exec(hook_regex::HTML_LINE, &highlighted, last)? {
            if m.end == m.index {
                return Err(Refusal(
                    "diff-highlight's line pattern matched nothing, which loops forever in Prism"
                        .into(),
                ));
            }
            let mut line = prefix.clone();
            line.extend_from_slice(&highlighted[m.index..m.end]);
            lines.push(line);
            last = m.end;
        }
        if self.exec(hook_regex::LAST_LINE, &decoded, 0)?.is_some() {
            lines.push(prefix);
        }
        env.content = lines.concat();
        if diff_grammar.is_some()
            && let Some(lang) = diff_language
        {
            env.classes.push(format!("language-{lang}"));
        }
        Ok(())
    }

    /// prism-markdown.js: a fenced code block is highlighted in its
    /// language (the `language-` class the `after-tokenize` hook added),
    /// when Prism has it.
    fn wrap_markdown(&mut self, env: &mut WrapEnv<'_>) -> Result<(), Refusal> {
        if env.kind != "code-block" {
            return Ok(());
        }
        let mut code_lang = String::new();
        for class in &env.classes {
            if let Some(m) = self.exec(hook_regex::LANGUAGE_CLASS, &units(class), 0)? {
                let (a, b) = m.groups[0].expect("group 1 is always set");
                code_lang = String::from_utf16_lossy(&units(class)[a..b]);
                break;
            }
        }
        if let Some(grammar) = self.languages.get(&code_lang)? {
            let text = self.text_content(&env.content)?;
            env.content = self.highlight_units(&text, grammar, &code_lang)?;
        }
        Ok(())
    }
}
