//! A port of Nunjucks 3.2.4's `lexer.js`, with the default delimiters and
//! `trimBlocks` and `lstripBlocks` off, as 11ty leaves them.
//!
//! Nunjucks walks the source one UTF-16 code unit at a time. Every place
//! it stops is on an ASCII character or the end, so walking by `char`
//! here stops in the same places. Lines advance after each `\n` only, and
//! columns count UTF-16 code units, as in Nunjucks.

use crate::ast::Span;
use crate::error::Fail;

const BLOCK_START: &str = "{%";
const BLOCK_END: &str = "%}";
const VARIABLE_START: &str = "{{";
const VARIABLE_END: &str = "}}";
const COMMENT_START: &str = "{#";
const COMMENT_END: &str = "#}";

fn is_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\n' | '\t' | '\r' | '\u{a0}')
}

fn is_delim(c: char) -> bool {
    "()[]{}%*-+~/#,:|.<>=!".contains(c)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    String,
    Whitespace,
    Data,
    BlockStart,
    BlockEnd,
    VariableStart,
    VariableEnd,
    Comment,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    LeftCurly,
    RightCurly,
    Operator,
    Comma,
    Colon,
    Tilde,
    Pipe,
    Int,
    Float,
    Boolean,
    None,
    Symbol,
    Regex,
}

impl Kind {
    /// The name Nunjucks's messages use.
    pub fn name(self) -> &'static str {
        match self {
            Kind::String => "string",
            Kind::Whitespace => "whitespace",
            Kind::Data => "data",
            Kind::BlockStart => "block-start",
            Kind::BlockEnd => "block-end",
            Kind::VariableStart => "variable-start",
            Kind::VariableEnd => "variable-end",
            Kind::Comment => "comment",
            Kind::LeftParen => "left-paren",
            Kind::RightParen => "right-paren",
            Kind::LeftBracket => "left-bracket",
            Kind::RightBracket => "right-bracket",
            Kind::LeftCurly => "left-curly",
            Kind::RightCurly => "right-curly",
            Kind::Operator => "operator",
            Kind::Comma => "comma",
            Kind::Colon => "colon",
            Kind::Tilde => "tilde",
            Kind::Pipe => "pipe",
            Kind::Int => "int",
            Kind::Float => "float",
            Kind::Boolean => "boolean",
            Kind::None => "none",
            Kind::Symbol => "symbol",
            Kind::Regex => "regex",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub kind: Kind,
    /// The text, or a string's value with its escapes read, or a regular
    /// expression's pattern.
    pub value: String,
    /// A regular expression's flags.
    pub flags: String,
    pub span: Span,
}

impl Token {
    /// `tok.value` as JavaScript would put it in a message. A regular
    /// expression token's value is an object.
    pub fn shown(&self) -> &str {
        if self.kind == Kind::Regex {
            "[object Object]"
        } else {
            &self.value
        }
    }

    pub fn at(&self) -> (usize, usize) {
        (self.span.line, self.span.column)
    }

    pub fn is_symbol(&self, name: &str) -> bool {
        self.kind == Kind::Symbol && self.value == name
    }
}

pub(crate) struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    /// 0-based, as Nunjucks keeps them.
    line: usize,
    col: usize,
    in_code: bool,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Lexer<'a> {
        Lexer {
            src,
            pos: 0,
            line: 0,
            col: 0,
            in_code: false,
        }
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn current(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn is_finished(&self) -> bool {
        self.pos >= self.src.len()
    }

    /// One character on. Nunjucks steps one UTF-16 unit, so a character
    /// outside the BMP is two columns.
    fn forward(&mut self) {
        if let Some(c) = self.current() {
            self.pos += c.len_utf8();
            if c == '\n' {
                self.line += 1;
                self.col = 0;
            } else {
                self.col += c.len_utf16();
            }
        }
    }

    /// Steps over `text`, which the caller has already matched.
    fn forward_over(&mut self, text: &str) {
        let end = self.pos + text.len();
        while self.pos < end {
            self.forward();
        }
    }

    fn matches(&self, s: &str) -> bool {
        self.rest().starts_with(s)
    }

    fn extract_string(&mut self, s: &str) -> bool {
        if self.matches(s) {
            self.forward_over(s);
            true
        } else {
            false
        }
    }

    /// `_extractMatching`: steps while `keep` holds. `None` at the end.
    fn extract_while(&mut self, keep: impl Fn(char) -> bool) -> Option<&'a str> {
        if self.is_finished() {
            return None;
        }
        let start = self.pos;
        while let Some(c) = self.current() {
            if !keep(c) {
                break;
            }
            self.forward();
        }
        Some(&self.src[start..self.pos])
    }

    fn token(&self, kind: Kind, value: String, start: (usize, usize, usize)) -> Token {
        Token {
            kind,
            value,
            flags: String::new(),
            span: Span {
                start: start.0,
                end: self.pos,
                line: start.1 + 1,
                column: start.2 + 1,
            },
        }
    }

    /// `nextToken`. `None` at the end of the source.
    pub fn next_token(&mut self) -> Result<Option<Token>, Fail> {
        if self.is_finished() {
            return Ok(None);
        }
        let start = (self.pos, self.line, self.col);
        if self.in_code {
            Ok(Some(self.code_token(start)))
        } else {
            self.data_token(start).map(Some)
        }
    }

    fn code_token(&mut self, start: (usize, usize, usize)) -> Token {
        let cur = self.current().unwrap_or_default();
        if cur == '"' || cur == '\'' {
            let value = self.parse_string(cur);
            return self.token(Kind::String, value, start);
        }
        if let Some(ws) = self.extract_while(is_whitespace).filter(|s| !s.is_empty()) {
            return self.token(Kind::Whitespace, ws.to_string(), start);
        }
        for (end, kind) in [
            (BLOCK_END, Kind::BlockEnd),
            (VARIABLE_END, Kind::VariableEnd),
        ] {
            let dashed = format!("-{end}");
            let tok = if self.extract_string(end) {
                end.to_string()
            } else if self.extract_string(&dashed) {
                dashed
            } else {
                continue;
            };
            self.in_code = false;
            return self.token(kind, tok, start);
        }
        if cur == 'r' && self.rest()[1..].starts_with('/') {
            return self.regex(start);
        }
        if is_delim(cur) {
            self.forward();
            let mut op = cur.to_string();
            const COMPLEX: [&str; 8] = ["==", "===", "!=", "!==", "<=", ">=", "//", "**"];
            if let Some(next) = self.current() {
                let two = format!("{cur}{next}");
                if COMPLEX.contains(&two.as_str()) {
                    self.forward();
                    op = two;
                    if let Some(third) = self.current() {
                        let three = format!("{op}{third}");
                        if COMPLEX.contains(&three.as_str()) {
                            self.forward();
                            op = three;
                        }
                    }
                }
            }
            let kind = match op.as_str() {
                "(" => Kind::LeftParen,
                ")" => Kind::RightParen,
                "[" => Kind::LeftBracket,
                "]" => Kind::RightBracket,
                "{" => Kind::LeftCurly,
                "}" => Kind::RightCurly,
                "," => Kind::Comma,
                ":" => Kind::Colon,
                "~" => Kind::Tilde,
                "|" => Kind::Pipe,
                _ => Kind::Operator,
            };
            return self.token(kind, op, start);
        }
        // Not whitespace or a delimiter, so this reads at least one
        // character.
        let word = self
            .extract_while(|c| !is_whitespace(c) && !is_delim(c))
            .unwrap_or_default();
        if !word.is_empty() && word.bytes().all(|b| b.is_ascii_digit()) {
            if self.current() == Some('.') {
                self.forward();
                // `_extract` gives `null` at the end, which JavaScript
                // writes into the token as "null".
                let dec = self.extract_while(|c| c.is_ascii_digit()).unwrap_or("null");
                return self.token(Kind::Float, format!("{word}.{dec}"), start);
            }
            return self.token(Kind::Int, word.to_string(), start);
        }
        let kind = match word {
            "true" | "false" => Kind::Boolean,
            "none" | "null" => Kind::None,
            _ => Kind::Symbol,
        };
        self.token(kind, word.to_string(), start)
    }

    /// `r/body/flags`. A `/` ends the body unless the character before it
    /// is a backslash.
    fn regex(&mut self, start: (usize, usize, usize)) -> Token {
        self.forward();
        self.forward();
        let mut body = String::new();
        let mut previous = '/';
        while let Some(c) = self.current() {
            self.forward();
            if c == '/' && previous != '\\' {
                break;
            }
            body.push(c);
            previous = c;
        }
        let mut flags = String::new();
        while let Some(c) = self.current().filter(|c| "gimy".contains(*c)) {
            flags.push(c);
            self.forward();
        }
        let mut tok = self.token(Kind::Regex, body, start);
        tok.flags = flags;
        tok
    }

    /// `_parseString`: up to the closing quote or the end. `\n`, `\t` and
    /// `\r` are escapes; a backslash before anything else keeps it.
    fn parse_string(&mut self, delimiter: char) -> String {
        self.forward();
        let mut out = String::new();
        while let Some(c) = self.current().filter(|c| *c != delimiter) {
            self.forward();
            if c != '\\' {
                out.push(c);
                continue;
            }
            if let Some(e) = self.current() {
                out.push(match e {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    other => other,
                });
                self.forward();
            }
        }
        self.forward();
        out
    }

    fn data_token(&mut self, start: (usize, usize, usize)) -> Result<Token, Fail> {
        for (open, kind) in [
            (BLOCK_START, Kind::BlockStart),
            (VARIABLE_START, Kind::VariableStart),
        ] {
            let dashed = format!("{open}-");
            let tok = if self.extract_string(&dashed) {
                dashed
            } else if self.extract_string(open) {
                open.to_string()
            } else {
                continue;
            };
            self.in_code = true;
            return Ok(self.token(kind, tok, start));
        }
        let in_comment = self.extract_string(COMMENT_START);
        // Text runs to the next `{%`, `{{` or `{#`; a comment to its `#}`.
        let mut at_end = false;
        loop {
            if self.extract_while(|c| c != '{' && c != '#').is_none() {
                at_end = true;
                break;
            }
            let opens = self.matches(BLOCK_START)
                || self.matches(VARIABLE_START)
                || self.matches(COMMENT_START);
            if opens && !in_comment {
                break;
            } else if self.matches(COMMENT_END) {
                if !in_comment {
                    return Err(Fail::js("Error", "unexpected end of comment"));
                }
                self.forward_over(COMMENT_END);
                break;
            } else {
                self.forward();
            }
        }
        if at_end && in_comment {
            return Err(Fail::js(
                "Error",
                "expected end of comment, got end of file",
            ));
        }
        let kind = if in_comment {
            Kind::Comment
        } else {
            Kind::Data
        };
        let text = self.src[start.0..self.pos].to_string();
        Ok(self.token(kind, text, start))
    }

    /// The `{% raw %}` scan: `_extractRegex` with
    /// `([\s\S]*?){%\s*(raw|endraw)\s*(?=%})%}` (or `verbatim`). Steps over
    /// the text up to and including the next such tag and returns the
    /// text before it, the tag and whether it opens. `None` when no such
    /// tag is left; nothing is consumed then.
    pub fn extract_raw_tag(&mut self, name: &str) -> Option<(&'a str, &'a str, bool)> {
        let rest = self.rest();
        let end_name = format!("end{name}");
        let mut from = 0;
        while let Some(i) = rest[from..].find(BLOCK_START) {
            let at = from + i;
            if let Some((len, opens)) = raw_tag_at(&rest[at..], name, &end_name) {
                let pre = &rest[..at];
                let tag = &rest[at..at + len];
                self.forward_over(&rest[..at + len]);
                return Some((pre, tag, opens));
            }
            from = at + 1;
        }
        None
    }

    /// Steps back to the start of a `{% endraw %}` tag just read, as
    /// Nunjucks's `backN` does. Nunjucks's `back` crashes when it steps
    /// back over a newline (it reads `this.src`, which is not set).
    pub fn back_over(&mut self, tag: &str) -> Result<(), Fail> {
        if tag.contains('\n') {
            return Err(Fail::undefined_read("lastIndexOf"));
        }
        self.pos -= tag.len();
        self.col -= tag.encode_utf16().count();
        Ok(())
    }
}

/// JavaScript's `\s`.
fn js_space(c: char) -> bool {
    brrrty_js::is_js_whitespace(c)
}

/// Whether `s` starts with `{%\s*(name|end_name)\s*%}`; its length and
/// whether it is `name`.
fn raw_tag_at(s: &str, name: &str, end_name: &str) -> Option<(usize, bool)> {
    let skip = |s: &str, at: usize| -> usize {
        at + s[at..]
            .char_indices()
            .find(|(_, c)| !js_space(*c))
            .map_or(s.len() - at, |(i, _)| i)
    };
    let after_open = skip(s, BLOCK_START.len());
    // `(raw|endraw)` tries `raw` first; the two never both match here.
    for (word, opens) in [(name, true), (end_name, false)] {
        if s[after_open..].starts_with(word) {
            let after = skip(s, after_open + word.len());
            if s[after..].starts_with(BLOCK_END) {
                return Some((after + BLOCK_END.len(), opens));
            }
        }
    }
    None
}
