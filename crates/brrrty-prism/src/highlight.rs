//! `Prism.highlight` (prism-core.js) with the hooks the plugin's Prism
//! has: `before-tokenize`, then `tokenize`, then `after-tokenize`, then
//! `Prism.util.encode` and `Token.stringify` (see [`crate::stringify`] for
//! the `wrap` hooks).
//!
//! The hooks, in the order they are added (each acts only for its own
//! language, so the order never shows):
//!
//! - diff-highlight (`before-tokenize`): `diff-<x>` is made a name for the
//!   diff grammar if Prism has no `diff-<x>`.
//! - php and liquid, through markup-templating (`before-tokenize` and
//!   `after-tokenize`): the PHP or Liquid parts of the code are swapped for
//!   placeholders, the rest is tokenized as markup, and each placeholder
//!   becomes a token holding its part tokenized as PHP or Liquid.
//! - markdown (`after-tokenize`): each fenced code block gets a
//!   `language-<x>` alias, which its `wrap` hook highlights with.
//! - jsx (`after-tokenize`): text inside a tag (not in braces) is merged
//!   into `plain-text` tokens.
//!
//! The regular expressions in the hooks are translated like the grammars'
//! (see [`hook_regex`]); a hook that can't be followed exactly refuses.
//!
//! Some hooks look a language up in `Prism.languages`, which in 11ty holds
//! whatever the build has loaded so far. [`Languages`] answers that.

use crate::grammar::{GrammarId, grammars};
use crate::regex::{BacktrackBudget, JsRegex, JsText, Match, translate, translate_spans};
use crate::stringify::encode;
use crate::tokenize::{Content, Item, MAX_DEPTH, Refusal, Token, Tokenizer, Units};

/// `Prism.languages`, as a hook sees it.
pub trait Languages {
    /// `Prism.languages[name]`: its grammar, `None` if Prism has none by
    /// that name, or a refusal if that can't be known.
    fn get(&self, name: &str) -> Result<Option<GrammarId>, Refusal>;
    /// `Prism.languages[name] = grammar`.
    fn set(&mut self, name: &str, grammar: GrammarId);
}

/// Names `Prism.languages` has that aren't grammars: its own functions and
/// what every object inherits. A lookup finds a function or an object with
/// nothing to tokenize with, so Prism leaves the text as one string.
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
];

/// `Prism.languages` once every language in grammars/prism.json is loaded
/// (and any name a hook adds).
#[derive(Clone, Debug, Default)]
pub struct AllLoaded {
    added: Vec<(String, GrammarId)>,
}

impl Languages for AllLoaded {
    fn get(&self, name: &str) -> Result<Option<GrammarId>, Refusal> {
        let g = grammars();
        if let Some((_, id)) = self.added.iter().find(|(k, _)| k == name) {
            return Ok(Some(*id));
        }
        if let Some(id) = g.language(name) {
            return Ok(Some(id));
        }
        if OBJECT_KEYS.contains(&name) {
            return Ok(Some(plain()));
        }
        Ok(None)
    }

    fn set(&mut self, name: &str, grammar: GrammarId) {
        self.added.retain(|(k, _)| k != name);
        self.added.push((name.to_string(), grammar));
    }
}

fn language(name: &str) -> GrammarId {
    grammars()
        .language(name)
        .unwrap_or_else(|| panic!("grammars/prism.json has {name}"))
}

/// A grammar with no tokens.
fn plain() -> GrammarId {
    language("plain")
}

/// diff's line prefixes, by token type: `PREFIXES` in prism-diff.js, and
/// the type's first word, the prefix token's alias.
pub const PREFIXES: &[(&str, &str, &str)] = &[
    ("deleted-sign", "-", "deleted"),
    ("deleted-arrow", "<", "deleted"),
    ("inserted-sign", "+", "inserted"),
    ("inserted-arrow", ">", "inserted"),
    ("unchanged", " ", "unchanged"),
    ("diff", "!", "diff"),
];

pub(crate) fn units(s: &str) -> Units {
    s.encode_utf16().collect()
}

/// The regular expressions of the hooks, as written in Prism 1.30.0 (each
/// is checked against the hook's source in grammars/prism.json by the
/// tests).
pub mod hook_regex {
    /// A pattern: source, flags, and whether only where it matches is used
    /// (see [`crate::regex::translate_spans`]).
    pub type HookRegex = (usize, &'static str, &'static str, bool);

    /// diff-highlight `LANGUAGE_REGEX`.
    pub const LANGUAGE: HookRegex = (0, r"^diff-([\w-]+)", "i", false);
    /// diff-highlight `HTML_TAG`.
    pub const HTML_TAG: HookRegex = (
        1,
        r#"<\/?(?!\d)[^\s>\/=$<%]+(?:\s(?:\s*[^\s>\/=]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s'">=]+(?=[\s>]))|(?=[\s/>])))+)?\s*\/?>"#,
        "g",
        true,
    );
    /// diff-highlight `HTML_LINE`: `(?:__|[^\r\n<])*(?:\r\n?|\n|(?:__|[^\r\n<])(?![^\r\n]))`
    /// with each `__` replaced by `HTML_TAG`'s source.
    pub const HTML_LINE: HookRegex = (
        2,
        r#"(?:<\/?(?!\d)[^\s>\/=$<%]+(?:\s(?:\s*[^\s>\/=]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s'">=]+(?=[\s>]))|(?=[\s/>])))+)?\s*\/?>|[^\r\n<])*(?:\r\n?|\n|(?:<\/?(?!\d)[^\s>\/=$<%]+(?:\s(?:\s*[^\s>\/=]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s'">=]+(?=[\s>]))|(?=[\s/>])))+)?\s*\/?>|[^\r\n<])(?![^\r\n]))"#,
        "gi",
        true,
    );
    /// diff-highlight: the one-character prefix of each line.
    pub const LINE_PREFIX: HookRegex = (3, r"(^|[\r\n]).", "g", false);
    /// diff-highlight: whether the last line is one character.
    pub const LAST_LINE: HookRegex = (4, r"(?:^|[\r\n]).$", "", true);
    /// php `before-tokenize`: whether there is PHP.
    pub const PHP_OPEN: HookRegex = (5, r"<\?", "", true);
    /// php `before-tokenize`: the PHP parts.
    pub const PHP: HookRegex = (
        6,
        r#"<\?(?:[^"'/#]|\/(?![*/])|("|')(?:\\[\s\S]|(?!\1)[^\\])*\1|(?:\/\/|#(?!\[))(?:[^?\n\r]|\?(?!>))*(?=$|\?>|[\r\n])|#\[|\/\*(?:[^*]|\*(?!\/))*(?:\*\/|$))*?(?:\?>|$)"#,
        "g",
        true,
    );
    /// liquid `before-tokenize`: the Liquid parts.
    pub const LIQUID: HookRegex = (
        7,
        r"\{%\s*comment\s*%\}[\s\S]*?\{%\s*endcomment\s*%\}|\{(?:%[\s\S]*?%|\{\{[\s\S]*?\}\}|\{[\s\S]*?\})\}",
        "g",
        true,
    );
    /// liquid `before-tokenize`: a tag's name.
    pub const LIQUID_TAG: HookRegex = (8, r"^\{%-?\s*(\w+)", "", false);
    /// markdown `after-tokenize`: C#, F#.
    pub const SHARP: HookRegex = (9, r"\b#", "g", true);
    /// markdown `after-tokenize`: C++.
    pub const PLUS_PLUS: HookRegex = (10, r"\b\+\+", "g", true);
    /// markdown `after-tokenize`: the language's first word.
    pub const FIRST_WORD: HookRegex = (11, r"[a-z][\w-]*", "i", true);
    /// markdown `wrap`: the code block's language class.
    pub const LANGUAGE_CLASS: HookRegex = (12, r"language-(.+)", "", false);
    /// markdown `textContent`: an entity.
    pub const ENTITY: HookRegex = (13, r"&(\w{1,8}|#x?[\da-f]{1,8});", "gi", false);
    /// markdown `textContent`: a tag (markup's tag pattern with `gi`; the
    /// source is the grammar's).
    pub const TAG: HookRegex = (14, "", "gi", true);

    pub const ALL: [HookRegex; 15] = [
        LANGUAGE,
        HTML_TAG,
        HTML_LINE,
        LINE_PREFIX,
        LAST_LINE,
        PHP_OPEN,
        PHP,
        LIQUID,
        LIQUID_TAG,
        SHARP,
        PLUS_PLUS,
        FIRST_WORD,
        LANGUAGE_CLASS,
        ENTITY,
        TAG,
    ];
}

/// The source of markup's tag pattern.
pub fn markup_tag_source() -> &'static str {
    let g = grammars();
    let tag = g
        .grammar(language("markup"))
        .tokens
        .iter()
        .find(|t| t.name == "tag")
        .expect("markup has tag");
    &g.regex_entry(tag.patterns[0].regex).source
}

/// A hook's pattern, translated on first use.
pub fn compile_hook_regex(
    r: hook_regex::HookRegex,
) -> Result<&'static JsRegex, &'static crate::regex::Untranslatable> {
    use std::sync::OnceLock;
    type Slot = OnceLock<Result<JsRegex, crate::regex::Untranslatable>>;
    static SLOTS: [Slot; 15] = [const { OnceLock::new() }; 15];
    let (i, source, flags, spans) = r;
    SLOTS[i]
        .get_or_init(|| {
            let source = if i == hook_regex::TAG.0 {
                markup_tag_source()
            } else {
                source
            };
            if spans {
                translate_spans(source, flags)
            } else {
                translate(source, flags)
            }
        })
        .as_ref()
}

/// `str.replace(re, f)`: with `global`, every match left to right (an empty
/// match moves on one unit), else the first. `f` gets the match, the string
/// and the match's text.
pub(crate) fn js_replace(
    h: &mut Highlighter<'_>,
    re: hook_regex::HookRegex,
    s: &[u16],
    global: bool,
    mut f: impl FnMut(&Match, &[u16], &[u16]) -> Result<Units, Refusal>,
) -> Result<Units, Refusal> {
    let mut out = Vec::with_capacity(s.len());
    let mut done = 0;
    let mut last = 0;
    let js = JsText::from_units(s);
    while last <= s.len() {
        let Some(m) = h.exec_js(re, &js, last)? else {
            break;
        };
        out.extend_from_slice(&s[done..m.index]);
        out.extend(f(&m, s, &s[m.index..m.end])?);
        done = m.end;
        last = if m.end == m.index { m.end + 1 } else { m.end };
        if !global {
            break;
        }
    }
    out.extend_from_slice(&s[done..]);
    Ok(out)
}

/// The numbers `i` whose placeholder (`placeholder(language, i)`) occurs in
/// `code`: `env.code.indexOf(placeholder) !== -1`, for every `i` at once.
/// An occurrence is `___`, the language, a run of digits (`i` written
/// without leading zeros) and `___`.
fn placeholders_in(code: &[u16], language: &str) -> std::collections::HashSet<usize> {
    let head = units(&format!("___{}", language.to_ascii_uppercase()));
    let mut out = std::collections::HashSet::new();
    let mut at = 0;
    while let Some(i) = index_of(&code[at..], &head) {
        let start = at + i + head.len();
        let digits = code[start..]
            .iter()
            .take_while(|u| (0x30..=0x39).contains(*u))
            .count();
        let run = &code[start..start + digits];
        let closed = code[start + digits..].starts_with(&units("___"));
        if digits > 0 && closed && (digits == 1 || run[0] != 0x30) && digits < 19 {
            let n = run
                .iter()
                .fold(0usize, |n, &d| n * 10 + usize::from(d - 0x30));
            out.insert(n);
        }
        at += i + 1;
    }
    out
}

/// How deep markup-templating's `walkTokens` may recurse here (once per
/// placeholder in a run of markup, and per token level). Node runs out of
/// stack at about 4,400 to 5,500 (it varies, and 11ty's own frames take
/// some), so where Node is sure to succeed is far below; Brrrty refuses
/// past this.
pub const PLACEHOLDER_DEPTH: usize = 1000;

/// `'___' + language.toUpperCase() + index + '___'`.
fn placeholder(language: &str, index: usize) -> Units {
    units(&format!("___{}{index}___", language.to_ascii_uppercase()))
}

fn index_of(s: &[u16], needle: &[u16]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    s.windows(needle.len()).position(|w| w == needle)
}

/// Highlighting state: the languages, the backtracking budget, and how
/// deep the hooks have nested.
pub struct Highlighter<'a> {
    pub(crate) languages: &'a mut dyn Languages,
    budget: BacktrackBudget,
    depth: usize,
}

/// `Prism.highlight(code, grammar, language)` with the plugin's hooks:
/// the HTML Prism gives, or why it can't be reproduced exactly.
pub fn highlight(
    code: &str,
    grammar: GrammarId,
    language: &str,
    languages: &mut dyn Languages,
) -> Result<String, Refusal> {
    let code = units(code);
    let mut h = Highlighter {
        languages,
        budget: BacktrackBudget::for_code(code.len()),
        depth: 0,
    };
    let out = h.highlight_units(&code, grammar, language)?;
    Ok(String::from_utf16_lossy(&out))
}

impl Highlighter<'_> {
    /// A hook pattern's `exec` on `s`, once (a loop builds the text once
    /// and calls [`Highlighter::exec_js`]).
    pub(crate) fn exec(
        &mut self,
        re: hook_regex::HookRegex,
        s: &[u16],
        last: usize,
    ) -> Result<Option<Match>, Refusal> {
        self.exec_js(re, &JsText::from_units(s), last)
    }

    pub(crate) fn exec_js(
        &mut self,
        re: hook_regex::HookRegex,
        s: &JsText,
        last: usize,
    ) -> Result<Option<Match>, Refusal> {
        let compiled = compile_hook_regex(re).map_err(|e| {
            Refusal(format!(
                "a Prism hook's pattern /{}/{} can't be matched exactly: {}",
                re.1, re.2, e.0
            ))
        })?;
        compiled
            .exec_budgeted(s, last, &mut self.budget)
            .map_err(|e| Refusal(e.0))
    }

    fn tokenize(&mut self, code: &[u16], grammar: GrammarId) -> Result<Vec<Item>, Refusal> {
        Tokenizer {
            g: grammars(),
            budget: &mut self.budget,
        }
        .tokenize(code, grammar, self.depth)
    }

    /// `Prism.highlight` on UTF-16 units.
    pub(crate) fn highlight_units(
        &mut self,
        code: &[u16],
        grammar: GrammarId,
        language: &str,
    ) -> Result<Units, Refusal> {
        if self.depth > MAX_DEPTH {
            return Err(Refusal(format!(
                "the code nests highlighting more than {MAX_DEPTH} levels deep"
            )));
        }
        self.depth += 1;
        let out = self.highlight_inner(code, grammar, language);
        self.depth -= 1;
        out
    }

    fn highlight_inner(
        &mut self,
        code: &[u16],
        mut grammar: GrammarId,
        language: &str,
    ) -> Result<Units, Refusal> {
        let mut code = code.to_vec();

        // before-tokenize: diff-highlight.
        if self
            .exec(hook_regex::LANGUAGE, &units(language), 0)?
            .is_some()
            && self.languages.get(language)?.is_none()
        {
            self.languages.set(language, language_id("diff"));
        }
        // before-tokenize: php.
        let mut stack = None;
        if self.exec(hook_regex::PHP_OPEN, &code, 0)?.is_some() && language == "php" {
            let (c, s) = self.build_placeholders(&code, "php", hook_regex::PHP, false)?;
            code = c;
            stack = Some(s);
            grammar = language_id("markup");
        }
        // before-tokenize: liquid.
        if language == "liquid" {
            let (c, s) = self.build_placeholders(&code, "liquid", hook_regex::LIQUID, true)?;
            code = c;
            stack = Some(s);
            grammar = language_id("markup");
        }

        let mut tokens = self.tokenize(&code, grammar)?;

        // after-tokenize.
        if let Some(stack) = stack {
            self.tokenize_placeholders(&mut tokens, language, &stack)?;
        }
        if language == "markdown" || language == "md" {
            self.markdown_languages(&mut tokens)?;
        }
        if language == "jsx" || language == "tsx" {
            jsx_walk(&mut tokens)?;
        }

        encode(&mut tokens);
        self.stringify(&tokens, language)
    }

    /// markup-templating `buildPlaceholders`: each match of `pattern` (that
    /// the liquid filter keeps) is swapped for a placeholder not already in
    /// the code. The parts, by placeholder number.
    fn build_placeholders(
        &mut self,
        code: &[u16],
        language: &str,
        pattern: hook_regex::HookRegex,
        liquid_filter: bool,
    ) -> Result<(Units, Vec<(usize, Units)>), Refusal> {
        let mut stack: Vec<(usize, Units)> = Vec::new();
        let mut inside_raw = false;
        let taken = placeholders_in(code, language);
        let mut keep = Vec::new();
        // The filter runs on each match in order; collect its answers first
        // (it can't see the replacement).
        let mut matches = Vec::new();
        {
            let mut last = 0;
            let js = JsText::from_units(code);
            while last <= code.len() {
                let Some(m) = self.exec_js(pattern, &js, last)? else {
                    break;
                };
                last = if m.end == m.index { m.end + 1 } else { m.end };
                matches.push(m);
            }
        }
        for m in &matches {
            let text = &code[m.index..m.end];
            let replace = if liquid_filter {
                // `/^\{%-?\s*(\w+)/` names the tag.
                match self.exec(hook_regex::LIQUID_TAG, text, 0)? {
                    Some(t) => {
                        let (a, b) = t.groups[0].expect("group 1 is always set");
                        let tag = &text[a..b];
                        if tag == units("raw").as_slice() && !inside_raw {
                            inside_raw = true;
                            true
                        } else if tag == units("endraw").as_slice() {
                            inside_raw = false;
                            true
                        } else {
                            !inside_raw
                        }
                    }
                    None => !inside_raw,
                }
            } else {
                true
            };
            keep.push(replace);
        }
        let mut out = Vec::with_capacity(code.len());
        let mut done = 0;
        for (m, replace) in matches.iter().zip(keep) {
            out.extend_from_slice(&code[done..m.index]);
            done = m.end;
            let text = &code[m.index..m.end];
            if !replace {
                out.extend_from_slice(text);
                continue;
            }
            // `tokenStack.length` of the sparse array.
            let mut i = stack.last().map_or(0, |(k, _)| k + 1);
            while taken.contains(&i) {
                i += 1;
            }
            stack.push((i, text.to_vec()));
            out.extend(placeholder(language, i));
        }
        out.extend_from_slice(&code[done..]);
        Ok((out, stack))
    }

    /// markup-templating `tokenizePlaceholders`: each placeholder, in
    /// order, becomes a token of its part tokenized in `language`.
    fn tokenize_placeholders(
        &mut self,
        tokens: &mut Vec<Item>,
        language: &str,
        stack: &[(usize, Units)],
    ) -> Result<(), Refusal> {
        let grammar = language_id(language);
        let mut j = 0;
        self.walk_placeholders(tokens, language, grammar, stack, &mut j, 0)
    }

    fn walk_placeholders(
        &mut self,
        tokens: &mut Vec<Item>,
        language: &str,
        grammar: GrammarId,
        stack: &[(usize, Units)],
        j: &mut usize,
        depth: usize,
    ) -> Result<(), Refusal> {
        if depth > PLACEHOLDER_DEPTH {
            return Err(Refusal(format!(
                "the code has more than {PLACEHOLDER_DEPTH} {language} parts in one stretch of markup: Prism's markup-templating recurses once per part, and past about 4,400 Node runs out of stack"
            )));
        }
        // JavaScript splices the replacement into the array and goes on
        // with its second element; here the list is rebuilt as it goes
        // (a splice per placeholder would shift the rest each time).
        let mut rest: std::collections::VecDeque<Item> = std::mem::take(tokens).into();
        let mut out = Vec::with_capacity(rest.len());
        while let Some(mut item) = rest.pop_front() {
            if *j >= stack.len() {
                out.push(item);
                out.extend(rest.drain(..));
                break;
            }
            let s: Option<Units> = match &item {
                Item::Str(s) => Some(s.clone()),
                Item::Token(Token {
                    content: Content::Str(s),
                    ..
                }) if !s.is_empty() => Some(s.clone()),
                _ => None,
            };
            if let Some(s) = s {
                let (k, t) = &stack[*j];
                let ph = placeholder(language, *k);
                if let Some(index) = index_of(&s, &ph) {
                    *j += 1;
                    let before = &s[..index];
                    let middle = Token {
                        kind: language.to_string(),
                        content: Content::Items(self.tokenize(t, grammar)?),
                        alias: vec![format!("language-{language}")],
                        length: t.len(),
                    };
                    let after = &s[index + ph.len()..];
                    let mut replacement = Vec::new();
                    if !before.is_empty() {
                        let mut b = vec![Item::Str(before.to_vec())];
                        self.walk_placeholders(&mut b, language, grammar, stack, j, depth + 1)?;
                        replacement.extend(b);
                    }
                    replacement.push(Item::Token(middle));
                    if !after.is_empty() {
                        let mut a = vec![Item::Str(after.to_vec())];
                        self.walk_placeholders(&mut a, language, grammar, stack, j, depth + 1)?;
                        replacement.extend(a);
                    }
                    match item {
                        Item::Str(_) => {
                            let mut r = replacement.into_iter();
                            out.extend(r.next());
                            for x in r.rev() {
                                rest.push_front(x);
                            }
                        }
                        Item::Token(mut tok) => {
                            tok.content = Content::Items(replacement);
                            out.push(Item::Token(tok));
                        }
                    }
                    continue;
                }
            } else if let Item::Token(Token {
                content: Content::Items(xs),
                ..
            }) = &mut item
            {
                self.walk_placeholders(xs, language, grammar, stack, j, depth + 1)?;
            }
            out.push(item);
        }
        *tokens = out;
        Ok(())
    }

    /// markdown `after-tokenize`: the `language-<x>` alias of each fenced
    /// code block, from its info string.
    fn markdown_languages(&mut self, tokens: &mut [Item]) -> Result<(), Refusal> {
        for item in tokens.iter_mut() {
            let Item::Token(token) = item else {
                continue;
            };
            if token.kind != "code" {
                if let Content::Items(xs) = &mut token.content {
                    self.markdown_languages(xs)?;
                }
                continue;
            }
            let Content::Items(xs) = &mut token.content else {
                continue;
            };
            let lang = match (xs.get(1), xs.get(3)) {
                (Some(Item::Token(l)), Some(Item::Token(b)))
                    if l.kind == "code-language" && b.kind == "code-block" =>
                {
                    match &l.content {
                        Content::Str(s) => s.clone(),
                        Content::Items(_) => continue,
                    }
                }
                _ => continue,
            };
            let lang = js_replace(self, hook_regex::SHARP, &lang, true, |_, _, _| {
                Ok(units("sharp"))
            })?;
            let lang = js_replace(self, hook_regex::PLUS_PLUS, &lang, true, |_, _, _| {
                Ok(units("pp"))
            })?;
            let lang = match self.exec(hook_regex::FIRST_WORD, &lang, 0)? {
                Some(m) => String::from_utf16_lossy(&lang[m.index..m.end]).to_ascii_lowercase(),
                None => String::new(),
            };
            if let Some(Item::Token(block)) = xs.get_mut(3) {
                block.alias.push(format!("language-{lang}"));
            }
        }
        Ok(())
    }

    /// prism-markdown.js `textContent`: tags removed, entities decoded.
    pub(crate) fn text_content(&mut self, html: &[u16]) -> Result<Units, Refusal> {
        let text = js_replace(self, hook_regex::TAG, html, true, |_, _, _| Ok(Vec::new()))?;
        js_replace(self, hook_regex::ENTITY, &text, true, |m, s, whole| {
            let (a, b) = m.groups[0].expect("group 1 is always set");
            let code = String::from_utf16_lossy(&s[a..b]).to_ascii_lowercase();
            if let Some(num) = code.strip_prefix('#') {
                let value = match num.strip_prefix('x') {
                    Some(hex) => u64::from_str_radix(hex, 16).ok(),
                    None => js_number(num),
                };
                match value {
                    Some(v) if v <= 0x10_FFFF => {
                        let mut out = Vec::new();
                        if v <= 0xFFFF {
                            out.push(v as u16);
                        } else {
                            let v = v - 0x1_0000;
                            out.push(0xD800 + (v >> 10) as u16);
                            out.push(0xDC00 + (v & 0x3FF) as u16);
                        }
                        Ok(out)
                    }
                    _ => Err(Refusal(format!(
                        "the entity `{}` in a Markdown code block makes Prism throw (String.fromCodePoint)",
                        String::from_utf16_lossy(whole)
                    ))),
                }
            } else {
                Ok(match code.as_str() {
                    "amp" => units("&"),
                    "lt" => units("<"),
                    "gt" => units(">"),
                    "quot" => units("\""),
                    _ => whole.to_vec(),
                })
            }
        })
    }
}

/// JavaScript's `Number(s)` for `s` of digits and `a`-`f` (what an entity
/// can hold), when it is a whole number: decimal digits with an optional
/// exponent, or `0b` and binary digits. `None` for NaN or a fraction.
fn js_number(s: &str) -> Option<u64> {
    if let Some(bin) = s.strip_prefix("0b") {
        if bin.is_empty() || !bin.bytes().all(|b| b == b'0' || b == b'1') {
            return None;
        }
        return u64::from_str_radix(bin, 2).ok();
    }
    let (mantissa, exponent) = match s.split_once('e') {
        Some((m, e)) => (m, Some(e)),
        None => (s, None),
    };
    if mantissa.is_empty() || !mantissa.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut v: u64 = mantissa.parse().ok()?;
    if let Some(e) = exponent {
        if e.is_empty() || !e.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let e: u32 = e.parse().ok()?;
        for _ in 0..e {
            if v == 0 {
                break;
            }
            v = v.checked_mul(10)?;
        }
    }
    Some(v)
}

fn language_id(name: &str) -> GrammarId {
    language(name)
}

// prism-jsx.js `after-tokenize`. JavaScript reads properties of whatever
// is there (a token, a string, a character of a string, or nothing); these
// helpers do the same.

/// A value as the jsx hook reads it.
#[derive(Clone, Copy)]
enum JsVal<'a> {
    Undefined,
    /// A string: a stream's string, or a character of a token's string.
    Str(&'a [u16]),
    Token(&'a Token),
}

impl<'a> JsVal<'a> {
    fn of(item: Option<&'a Item>) -> Self {
        match item {
            None => JsVal::Undefined,
            Some(Item::Str(s)) => JsVal::Str(s),
            Some(Item::Token(t)) => JsVal::Token(t),
        }
    }

    fn truthy(self) -> bool {
        match self {
            JsVal::Undefined => false,
            JsVal::Str(s) => !s.is_empty(),
            JsVal::Token(_) => true,
        }
    }

    /// `.type`.
    fn kind(self) -> Option<&'a str> {
        match self {
            JsVal::Token(t) => Some(&t.kind),
            _ => None,
        }
    }

    /// `.content[i]`: a TypeError unless this is a token (a string's
    /// `.content` is undefined).
    fn content_at(self, i: isize) -> Result<JsVal<'a>, Refusal> {
        match self {
            JsVal::Token(t) => Ok(index(&t.content, i)),
            JsVal::Str(_) | JsVal::Undefined => Err(type_error()),
        }
    }

    /// `.content === s`.
    fn content_is(self, s: &str) -> Result<bool, Refusal> {
        match self {
            JsVal::Token(t) => Ok(matches!(&t.content, Content::Str(c) if *c == units(s))),
            JsVal::Str(_) => Ok(false),
            JsVal::Undefined => Err(type_error()),
        }
    }
}

/// `content[i]` (a character of a string content).
fn index(content: &Content, i: isize) -> JsVal<'_> {
    if i < 0 {
        return JsVal::Undefined;
    }
    let i = i as usize;
    match content {
        Content::Str(s) => s.get(i..i + 1).map_or(JsVal::Undefined, JsVal::Str),
        Content::Items(xs) => JsVal::of(xs.get(i)),
    }
}

fn content_len(content: &Content) -> isize {
    match content {
        Content::Str(s) => s.len() as isize,
        Content::Items(xs) => xs.len() as isize,
    }
}

fn type_error() -> Refusal {
    Refusal("the jsx hook reads a property of nothing here, which makes Prism throw".into())
}

/// jsx `stringifyToken`.
fn stringify_jsx(v: JsVal<'_>) -> Units {
    match v {
        JsVal::Undefined => Vec::new(),
        JsVal::Str(s) => s.to_vec(),
        JsVal::Token(t) => match &t.content {
            Content::Str(s) => s.clone(),
            Content::Items(xs) => xs
                .iter()
                .flat_map(|x| stringify_jsx(JsVal::of(Some(x))))
                .collect(),
        },
    }
}

fn is_plain(item: &Item) -> bool {
    match item {
        Item::Str(_) => true,
        Item::Token(t) => t.kind == "plain-text",
    }
}

struct OpenTag {
    name: Units,
    braces: usize,
}

/// jsx `walkTokens`.
fn jsx_walk(tokens: &mut Vec<Item>) -> Result<(), Refusal> {
    let mut opened: Vec<OpenTag> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let mut not_tag_nor_brace = false;
        let is_string = matches!(tokens[i], Item::Str(_));
        if let Item::Token(token) = &tokens[i] {
            let tv = JsVal::Token(token);
            let first = tv.content_at(0)?;
            if token.kind == "tag" && first.truthy() && first.kind() == Some("tag") {
                if first.content_at(0)?.content_is("</")? {
                    if let Some(top) = opened.last()
                        && top.name == stringify_jsx(first.content_at(1)?)
                    {
                        opened.pop();
                    }
                } else {
                    let last = tv.content_at(content_len(&token.content) - 1)?;
                    if !last.content_is("/>")? {
                        opened.push(OpenTag {
                            name: stringify_jsx(first.content_at(1)?),
                            braces: 0,
                        });
                    }
                }
            } else if !opened.is_empty()
                && token.kind == "punctuation"
                && matches!(&token.content, Content::Str(c) if *c == units("{"))
            {
                opened.last_mut().expect("not empty").braces += 1;
            } else if opened.last().is_some_and(|t| t.braces > 0)
                && token.kind == "punctuation"
                && matches!(&token.content, Content::Str(c) if *c == units("}"))
            {
                opened.last_mut().expect("not empty").braces -= 1;
            } else {
                not_tag_nor_brace = true;
            }
        }
        let mut walk_old = None;
        if (not_tag_nor_brace || is_string) && opened.last().is_some_and(|t| t.braces == 0) {
            // Text inside a tag, outside braces: plain text, merged with the
            // text next to it.
            let mut plain = stringify_jsx(JsVal::of(tokens.get(i)));
            if i + 1 < tokens.len() && is_plain(&tokens[i + 1]) {
                plain.extend(stringify_jsx(JsVal::of(tokens.get(i + 1))));
                tokens.remove(i + 1);
            }
            if i > 0 && is_plain(&tokens[i - 1]) {
                let mut p = stringify_jsx(JsVal::of(tokens.get(i - 1)));
                p.extend(plain);
                plain = p;
                tokens.remove(i - 1);
                i -= 1;
            }
            let len = plain.len();
            let old = std::mem::replace(
                &mut tokens[i],
                Item::Token(Token {
                    kind: "plain-text".into(),
                    content: Content::Str(plain),
                    alias: Vec::new(),
                    length: len,
                }),
            );
            walk_old = Some(old);
        }
        // `token` is still the token this pass started with.
        match walk_old {
            Some(Item::Token(mut old)) => {
                if let Content::Items(xs) = &mut old.content {
                    jsx_walk(xs)?;
                }
            }
            Some(Item::Str(_)) => {}
            None => {
                if let Item::Token(Token {
                    content: Content::Items(xs),
                    ..
                }) = &mut tokens[i]
                {
                    jsx_walk(xs)?;
                }
            }
        }
        i += 1;
    }
    Ok(())
}
