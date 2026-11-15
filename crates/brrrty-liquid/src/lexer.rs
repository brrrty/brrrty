//! A port of LiquidJS 10.30.1's tokenizer.
//!
//! Positions are byte offsets. LiquidJS works in UTF-16 code units, but
//! every place it steps one unit at a time either stays on ASCII or steps
//! over a whole character here, so the two agree on every boundary.

use crate::ast::{
    BinaryOp, Expr, Filter, FilterArg, FilteredValue, HashArg, Literal, Segment, Span,
};
use crate::error::{ErrorKind, Located};

/// The deepest an expression may nest, such as `a[a[a[...]]]`.
pub(crate) const MAX_DEPTH: usize = 256;

// LiquidJS's character table for ASCII.
#[rustfmt::skip]
const TYPES: [u8; 128] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 20, 4, 4, 4, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    20, 2, 8, 0, 0, 0, 0, 8, 0, 0, 0, 64, 0, 65, 0, 0, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 0, 0, 2, 2, 2, 1,
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 1,
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0,
];
const WORD: u8 = 1;
const BLANK: u8 = 4;
const QUOTE: u8 = 8;
const NUMBER: u8 = 32;
const SIGN: u8 = 64;
const PUNCTUATION: u8 = 128;

fn char_type(c: char) -> u8 {
    let u = c as u32;
    if u < 128 {
        return TYPES[u as usize];
    }
    match u {
        160 | 5760 | 6158 | 8192..=8202 | 8232 | 8233 | 8239 | 8287 | 12288 => BLANK,
        8220 | 8221 => PUNCTUATION,
        _ => 0,
    }
}

fn is_word(c: Option<char>) -> bool {
    match c {
        None => false,
        Some(c) if (c as u32) >= 128 => char_type(c) == 0,
        Some(c) => char_type(c) & WORD != 0,
    }
}

pub(crate) fn is_blank(c: char) -> bool {
    char_type(c) & BLANK != 0
}

/// `input[a..b]`, or `""` when the range is empty or reversed, like JS `slice`.
pub(crate) fn slice(input: &str, a: usize, b: usize) -> &str {
    let b = b.min(input.len());
    if a >= b { "" } else { &input[a..b] }
}

/// Byte offset to 1-based line and UTF-16 column.
pub(crate) struct LineIndex<'a> {
    input: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub fn new(input: &'a str) -> LineIndex<'a> {
        let mut starts = vec![0];
        starts.extend(input.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { input, starts }
    }

    pub fn position(&self, offset: usize) -> (usize, usize) {
        // LiquidJS can point one past the end; count that as more columns.
        let past = offset.saturating_sub(self.input.len());
        let offset = offset.min(self.input.len());
        let line = self.starts.partition_point(|&s| s <= offset);
        let start = self.starts[line - 1];
        let col = self.input[start..offset].encode_utf16().count() + 1 + past;
        (line, col)
    }
}

/// Makes spans for one source.
pub(crate) enum Spanner<'a> {
    Index(LineIndex<'a>),
    /// Everything gets this span: the source is a quoted partial name and
    /// the span is where that name sits in the real template.
    Fixed(Span),
}

impl Spanner<'_> {
    pub fn span(&self, start: usize, end: usize) -> Span {
        match self {
            Spanner::Index(index) => {
                let (line, column) = index.position(start);
                Span {
                    start,
                    end: end.max(start),
                    line,
                    column,
                }
            }
            Spanner::Fixed(span) => *span,
        }
    }
}

/// JavaScript's `JSON.stringify` for a string.
pub(crate) fn json_string(units: &[u16]) -> String {
    let mut out = String::from("\"");
    for r in char::decode_utf16(units.iter().copied()) {
        match r {
            Ok('"') => out.push_str("\\\""),
            Ok('\\') => out.push_str("\\\\"),
            Ok('\u{8}') => out.push_str("\\b"),
            Ok('\u{c}') => out.push_str("\\f"),
            Ok('\n') => out.push_str("\\n"),
            Ok('\r') => out.push_str("\\r"),
            Ok('\t') => out.push_str("\\t"),
            Ok(c) if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            Ok(c) => out.push(c),
            Err(e) => out.push_str(&format!("\\u{:04x}", e.unpaired_surrogate())),
        }
    }
    out.push('"');
    out
}

/// LiquidJS's `JSON.stringify(ellipsis(text, 32))`.
pub(crate) fn snapshot_of(text: &str) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() > 32 {
        let mut cut = units[..29].to_vec();
        cut.extend("...".encode_utf16());
        json_string(&cut)
    } else {
        json_string(&units)
    }
}

/// LiquidJS's `parseStringLiteral`: drops the first and last character and
/// reads backslash escapes.
fn parse_string_literal(text: &str) -> String {
    let s: Vec<u16> = text.encode_utf16().collect();
    let at = |i: usize| s.get(i).copied();
    let is_hex =
        |u: Option<u16>| u.is_some_and(|u| (u as u8 as u16 == u) && (u as u8).is_ascii_hexdigit());
    let is_oct = |u: Option<u16>| u.is_some_and(|u| (b'0' as u16..=b'7' as u16).contains(&u));
    let hex_val = |u: u16| -> u32 {
        let u = u as u32;
        if u >= 97 {
            u - 87
        } else if u >= 65 {
            u - 55
        } else {
            u - 48
        }
    };
    let mut out: Vec<u16> = Vec::new();
    let mut i = 1;
    while i + 1 < s.len() {
        if s[i] != b'\\' as u16 {
            out.push(s[i]);
            i += 1;
            continue;
        }
        let next = at(i + 1);
        let esc = match next.map(|u| u as u32) {
            Some(0x62) => Some(0x08),
            Some(0x66) => Some(0x0c),
            Some(0x6e) => Some(0x0a),
            Some(0x72) => Some(0x0d),
            Some(0x74) => Some(0x09),
            Some(0x76) => Some(0x0b),
            _ => None,
        };
        if let Some(e) = esc {
            out.push(e);
            i += 2;
        } else if next == Some(b'u' as u16) {
            let mut val: u32 = 0;
            let mut j = i + 2;
            // LiquidJS crashes when an unterminated string ends inside
            // `\u`; this stops at the end instead.
            while j <= i + 5 && is_hex(at(j)) {
                val = val * 16 + hex_val(s[j]);
                j += 1;
            }
            out.push(val as u16);
            i = j;
        } else if !is_oct(next) {
            if let Some(n) = next {
                out.push(n);
            }
            i += 2;
        } else {
            let mut j = i + 1;
            let mut val: u32 = 0;
            while j <= i + 3 && is_oct(at(j)) {
                val = val * 8 + hex_val(s[j]);
                j += 1;
            }
            out.push(val as u16);
            i = j;
        }
    }
    String::from_utf16_lossy(&out)
}

/// The start of a tag or output: LiquidJS's `DelimitedToken`. Returns the
/// trimmed content range and whether each end has a `-`.
pub(crate) fn delimited(input: &str, begin: usize, end: usize) -> ((usize, usize), bool, bool) {
    let bytes = input.as_bytes();
    let tl = bytes.get(begin) == Some(&b'-');
    let tr = end >= 1 && bytes.get(end - 1) == Some(&b'-');
    let mut l = if tl { begin + 1 } else { begin };
    let mut r = if tr { end - 1 } else { end };
    while l < r {
        match input[l..].chars().next() {
            Some(c) if is_blank(c) => l += c.len_utf8(),
            _ => break,
        }
    }
    while r > l {
        match input[..r].chars().next_back() {
            Some(c) if is_blank(c) => r -= c.len_utf8(),
            _ => break,
        }
    }
    ((l, r), tl, tr)
}

#[derive(Debug, Clone)]
pub(crate) struct HtmlToken {
    pub begin: usize,
    pub end: usize,
    pub trim_left: usize,
    pub trim_right: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct TagToken {
    pub begin: usize,
    pub end: usize,
    pub content: (usize, usize),
    pub trim_left: bool,
    pub trim_right: bool,
    pub name: String,
    /// Where the arguments start, after the name and any blanks.
    pub args_begin: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct OutputToken {
    pub begin: usize,
    pub end: usize,
    pub content: (usize, usize),
    pub trim_left: bool,
    pub trim_right: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum Token {
    Html(HtmlToken),
    Tag(TagToken),
    Output(OutputToken),
}

impl Token {
    pub fn begin(&self) -> usize {
        match self {
            Token::Html(t) => t.begin,
            Token::Tag(t) => t.begin,
            Token::Output(t) => t.begin,
        }
    }

    pub fn end(&self) -> usize {
        match self {
            Token::Html(t) => t.end,
            Token::Tag(t) => t.end,
            Token::Output(t) => t.end,
        }
    }
}

impl TagToken {
    /// LiquidJS's `TagToken` / `LiquidTagToken` constructor: reads the name.
    fn new(
        input: &str,
        begin: usize,
        end: usize,
        content_begin: usize,
        content_end: usize,
        empty_name: &str,
    ) -> Result<TagToken, Located> {
        let (content, trim_left, trim_right) = delimited(input, content_begin, content_end);
        let mut c = Cursor::bare(input, content.0, content.1);
        let name = c.read_tag_name();
        if name.is_empty() {
            return Err(c.error(ErrorKind::Syntax, empty_name, c.p));
        }
        c.skip_blank();
        Ok(TagToken {
            begin,
            end,
            content,
            trim_left,
            trim_right,
            name,
            args_begin: c.p,
        })
    }

    pub fn args<'s>(&self, input: &'s str) -> &'s str {
        slice(input, self.args_begin, self.content.1)
    }
}

/// An operator in an expression, before it is built into a tree.
#[derive(Debug, Clone, Copy)]
enum Op {
    Binary(BinaryOp),
    Not,
}

impl Op {
    fn precedence(self) -> u8 {
        match self {
            Op::Binary(BinaryOp::And | BinaryOp::Or) => 0,
            Op::Not => 1,
            Op::Binary(_) => 2,
        }
    }
}

const OPERATORS: [(&str, Op); 10] = [
    ("==", Op::Binary(BinaryOp::Eq)),
    ("!=", Op::Binary(BinaryOp::Ne)),
    (">", Op::Binary(BinaryOp::Gt)),
    ("<", Op::Binary(BinaryOp::Lt)),
    (">=", Op::Binary(BinaryOp::Ge)),
    ("<=", Op::Binary(BinaryOp::Le)),
    ("contains", Op::Binary(BinaryOp::Contains)),
    ("not", Op::Not),
    ("and", Op::Binary(BinaryOp::And)),
    ("or", Op::Binary(BinaryOp::Or)),
];

const LITERALS: [(&str, Literal); 6] = [
    ("true", Literal::True),
    ("false", Literal::False),
    ("nil", Literal::Nil),
    ("null", Literal::Nil),
    ("empty", Literal::Empty),
    ("blank", Literal::Blank),
];

enum ExprTok {
    Op(Op, Span),
    Val(Expr),
}

/// LiquidJS's `Tokenizer`: a position `p` reading `input` up to `n`.
pub(crate) struct Cursor<'a> {
    pub input: &'a str,
    pub p: usize,
    pub n: usize,
    spans: Option<&'a Spanner<'a>>,
    depth: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(input: &'a str, p: usize, n: usize, spans: &'a Spanner<'a>) -> Cursor<'a> {
        Cursor {
            input,
            p,
            n,
            spans: Some(spans),
            depth: 0,
        }
    }

    /// A cursor that reads names and positions but never builds expressions.
    fn bare(input: &'a str, p: usize, n: usize) -> Cursor<'a> {
        Cursor {
            input,
            p,
            n,
            spans: None,
            depth: 0,
        }
    }

    fn span(&self, start: usize, end: usize) -> Span {
        match self.spans {
            Some(s) => s.span(start, end),
            None => Span {
                start,
                end,
                line: 0,
                column: 0,
            },
        }
    }

    pub fn error(&self, kind: ErrorKind, message: impl Into<String>, offset: usize) -> Located {
        Located {
            kind,
            message: message.into(),
            help: None,
            offset,
        }
    }

    pub fn end(&self) -> bool {
        self.p >= self.n
    }

    /// The character `off` bytes ahead, if it is before `n`.
    fn peek_at(&self, off: usize) -> Option<char> {
        let i = self.p + off;
        if i >= self.n {
            return None;
        }
        self.input.get(i..).and_then(|s| s.chars().next())
    }

    pub fn peek(&self) -> Option<char> {
        self.peek_at(0)
    }

    fn peek_type(&self, off: usize) -> u8 {
        self.peek_at(off).map_or(0, char_type)
    }

    /// Steps over one character. Past the end of the input it steps one
    /// position, as a JS string index would.
    fn bump(&mut self) {
        match self.input.get(self.p..).and_then(|s| s.chars().next()) {
            Some(c) => self.p += c.len_utf8(),
            None => self.p += 1,
        }
    }

    pub fn advance(&mut self) {
        self.bump();
    }

    /// `input[p++]`: reads even past `n`.
    fn read(&mut self) -> Option<char> {
        let c = self.input.get(self.p..).and_then(|s| s.chars().next());
        self.bump();
        c
    }

    fn matches(&self, word: &str) -> bool {
        self.input
            .as_bytes()
            .get(self.p..)
            .is_some_and(|b| b.starts_with(word.as_bytes()))
    }

    fn rmatch(&self, pattern: &str) -> bool {
        self.p >= pattern.len()
            && self
                .input
                .as_bytes()
                .get(self.p - pattern.len()..self.p)
                .is_some_and(|b| b == pattern.as_bytes())
    }

    pub fn skip_blank(&mut self) {
        while self.peek_type(0) & BLANK != 0 {
            self.bump();
        }
    }

    pub fn snapshot(&self, begin: usize) -> String {
        snapshot_of(slice(self.input, begin, self.n))
    }

    pub fn read_identifier(&mut self) -> (usize, usize) {
        self.skip_blank();
        let begin = self.p;
        while !self.end() && is_word(self.peek()) {
            self.bump();
        }
        (begin, self.p)
    }

    pub fn read_identifier_text(&mut self) -> &'a str {
        let (b, e) = self.read_identifier();
        slice(self.input, b, e)
    }

    fn read_non_empty_identifier(&mut self) -> Option<(usize, usize)> {
        let (b, e) = self.read_identifier();
        (e > b).then_some((b, e))
    }

    fn read_tag_name(&mut self) -> String {
        self.skip_blank();
        if self.peek() == Some('#') {
            self.bump();
            return "#".to_string();
        }
        self.read_identifier_text().to_string()
    }

    /// Moves past the next `end`. `None` (at `n`) when there is none.
    pub fn read_to(&mut self, end: &str) -> Option<usize> {
        while self.p < self.n {
            self.bump();
            if self.rmatch(end) {
                return Some(self.p);
            }
        }
        None
    }

    fn read_to_delimiter(&mut self, delimiter: &str, respect_quoted: bool) -> Option<usize> {
        self.skip_blank();
        while self.p < self.n {
            if respect_quoted && self.peek_type(0) & QUOTE != 0 {
                self.read_quoted_range();
                continue;
            }
            self.bump();
            if self.rmatch(delimiter) {
                return Some(self.p);
            }
        }
        None
    }

    fn read_quoted_range(&mut self) -> Option<(usize, usize)> {
        self.skip_blank();
        let begin = self.p;
        if self.peek_type(0) & QUOTE == 0 {
            return None;
        }
        let quote = self.peek();
        self.bump();
        let mut escaped = false;
        while self.p < self.n {
            let c = self.input[self.p..].chars().next();
            self.bump();
            if c == quote && !escaped {
                break;
            }
            if escaped {
                escaped = false;
            } else if c == Some('\\') {
                escaped = true;
            }
        }
        Some((begin, self.p))
    }

    /// A quoted string and its parsed content.
    pub fn read_quoted(&mut self) -> Option<(Span, String)> {
        let (b, e) = self.read_quoted_range()?;
        Some((
            self.span(b, e),
            parse_string_literal(slice(self.input, b, e)),
        ))
    }

    /// The longest of `words` at `p`. A word that ends in a word character
    /// must not run into another one. Like LiquidJS's tries, a longer match
    /// that fails that check does not fall back to a shorter one.
    fn match_words<T: Clone>(&self, words: &[(&str, T)]) -> Option<(usize, T)> {
        let rest = self.input.as_bytes().get(self.p..self.n.max(self.p))?;
        let (word, value) = words
            .iter()
            .filter(|(w, _)| rest.starts_with(w.as_bytes()))
            .max_by_key(|(w, _)| w.len())?;
        let last = word.chars().next_back();
        if is_word(last) && is_word(self.peek_at(word.len())) {
            return None;
        }
        Some((self.p + word.len(), value.clone()))
    }

    fn read_operator(&mut self) -> Option<(Op, Span)> {
        self.skip_blank();
        let (end, op) = self.match_words(&OPERATORS)?;
        let span = self.span(self.p, end);
        self.p = end;
        Some((op, span))
    }

    fn read_literal(&mut self) -> Option<Expr> {
        self.skip_blank();
        let (end, value) = self.match_words(&LITERALS)?;
        let span = self.span(self.p, end);
        self.p = end;
        Some(Expr::Literal { span, value })
    }

    fn read_number(&mut self) -> Option<Expr> {
        self.skip_blank();
        let mut decimal = false;
        let mut digit = false;
        let mut n = 0;
        if self.peek_type(0) & SIGN != 0 {
            n += 1;
        }
        while self.p + n <= self.n {
            if self.peek_type(n) & NUMBER != 0 {
                digit = true;
                n += 1;
            } else if self.peek_at(n) == Some('.') && self.peek_at(n + 1) != Some('.') {
                if decimal || !digit {
                    return None;
                }
                decimal = true;
                n += 1;
            } else {
                break;
            }
        }
        if digit && !is_word(self.peek_at(n)) {
            let text = &self.input[self.p..self.p + n];
            let span = self.span(self.p, self.p + n);
            self.p += n;
            return Some(Expr::Literal {
                span,
                value: Literal::Number(js_number(text)),
            });
        }
        None
    }

    pub fn read_value(&mut self) -> Result<Option<Expr>, Located> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error(
                ErrorKind::TooDeep,
                format!("expressions nest too deeply (more than {MAX_DEPTH} levels)"),
                self.p,
            ));
        }
        let r = grow(|| self.read_value_inner());
        self.depth -= 1;
        r
    }

    fn read_value_inner(&mut self) -> Result<Option<Expr>, Located> {
        self.skip_blank();
        let begin = self.p;
        let mut variable = self.read_literal();
        if variable.is_none() {
            variable = self.read_quoted().map(|(span, s)| Expr::Literal {
                span,
                value: Literal::String(s),
            });
        }
        if variable.is_none() {
            variable = self.read_number();
        }
        if variable.is_none() && self.peek() == Some('(') {
            variable = self.read_group_or_range()?;
        }
        let segments = self.read_properties(variable.is_none())?;
        if segments.is_empty() {
            return Ok(variable);
        }
        Ok(Some(Expr::Path {
            span: self.span(begin, self.p),
            root: variable.map(Box::new),
            segments,
        }))
    }

    fn read_properties(&mut self, is_begin: bool) -> Result<Vec<Segment>, Located> {
        let mut props = Vec::new();
        loop {
            if self.peek() == Some('[') {
                self.bump();
                let at = self.p;
                let prop = match self.read_value()? {
                    Some(e) => Segment::Index(e),
                    None => Segment::Key {
                        span: self.span(at, at),
                        name: String::new(),
                    },
                };
                if self.read_to("]").is_none() {
                    return Err(self.error(ErrorKind::Syntax, "[ not closed", self.p));
                }
                props.push(prop);
                continue;
            }
            if is_begin
                && props.is_empty()
                && let Some((b, e)) = self.read_non_empty_identifier()
            {
                props.push(self.key(b, e));
                continue;
            }
            if self.peek() == Some('.') && self.peek_at(1) != Some('.') {
                self.bump();
                match self.read_non_empty_identifier() {
                    Some((b, e)) => {
                        props.push(self.key(b, e));
                        continue;
                    }
                    None => break,
                }
            }
            break;
        }
        Ok(props)
    }

    fn key(&self, b: usize, e: usize) -> Segment {
        Segment::Key {
            span: self.span(b, e),
            name: self.input[b..e].to_string(),
        }
    }

    fn read_group_or_range(&mut self) -> Result<Option<Expr>, Located> {
        self.skip_blank();
        let begin = self.p;
        if self.peek() != Some('(') {
            return Ok(None);
        }
        self.bump();
        let lhs = self.read_value_or_throw()?;
        self.skip_blank();
        if self.peek() == Some('.') && self.peek_at(1) == Some('.') {
            self.p += 2;
            let rhs = self.read_value_or_throw()?;
            self.skip_blank();
            if self.read() != Some(')') {
                return Err(self.error(ErrorKind::Syntax, "invalid range syntax", self.p));
            }
            return Ok(Some(Expr::Range {
                span: self.span(begin, self.p),
                start: Box::new(lhs),
                end: Box::new(rhs),
            }));
        }
        Err(self.error(ErrorKind::Syntax, "invalid range syntax", self.p))
    }

    pub fn read_value_or_throw(&mut self) -> Result<Expr, Located> {
        match self.read_value()? {
            Some(v) => Ok(v),
            None => Err(self.error(
                ErrorKind::Syntax,
                format!("unexpected token {}, value expected", self.snapshot(self.p)),
                self.p,
            )),
        }
    }

    /// Operators and operands until neither reads, built into a tree the
    /// way LiquidJS evaluates its postfix form.
    fn read_expression(&mut self) -> Result<Option<Expr>, Located> {
        let mut toks = Vec::new();
        while self.p < self.n {
            if let Some((op, span)) = self.read_operator() {
                toks.push(ExprTok::Op(op, span));
                continue;
            }
            match self.read_value()? {
                Some(v) => toks.push(ExprTok::Val(v)),
                None => break,
            }
        }
        Ok(build_expression(toks))
    }

    pub fn read_filtered_value(&mut self) -> Result<FilteredValue, Located> {
        let begin = self.p;
        let Some(expr) = self.read_expression()? else {
            return Err(self.error(
                ErrorKind::Syntax,
                format!("invalid value expression: {}", self.snapshot(self.p)),
                self.p,
            ));
        };
        let mut filters = Vec::new();
        while let Some(f) = self.read_filter()? {
            filters.push(f);
        }
        Ok(FilteredValue {
            span: self.span(begin, self.p),
            text: slice(self.input, begin, self.p).to_string(),
            expr,
            filters,
        })
    }

    fn read_filter(&mut self) -> Result<Option<Filter>, Located> {
        self.skip_blank();
        if self.end() {
            return Ok(None);
        }
        if matches!(self.peek(), Some(')' | ',')) {
            return Ok(None);
        }
        if self.read() != Some('|') {
            return Err(self.error(ErrorKind::Syntax, "expected \"|\" before filter", self.p));
        }
        let (nb, ne) = self.read_identifier();
        if ne == nb {
            if !self.end() {
                return Err(self.error(ErrorKind::Syntax, "expected filter name", self.p));
            }
            return Ok(None);
        }
        let mut args = Vec::new();
        self.skip_blank();
        if self.peek() == Some(':') {
            loop {
                self.bump();
                if let Some(arg) = self.read_filter_arg()? {
                    args.push(arg);
                }
                self.skip_blank();
                if !(self.end() || matches!(self.peek(), Some(',' | '|' | ')'))) {
                    return Err(self.error(
                        ErrorKind::Syntax,
                        format!("unexpected character {}", self.snapshot(self.p)),
                        self.p,
                    ));
                }
                if self.peek() != Some(',') {
                    break;
                }
            }
        } else if !(self.end() || matches!(self.peek(), Some('|' | ')' | ','))) {
            return Err(self.error(
                ErrorKind::Syntax,
                "expected \":\" after filter name",
                self.p,
            ));
        }
        Ok(Some(Filter {
            span: self.span(nb, self.p),
            name: self.input[nb..ne].to_string(),
            args,
        }))
    }

    fn read_filter_arg(&mut self) -> Result<Option<FilterArg>, Located> {
        self.skip_blank();
        let begin = self.p;
        let Some(key) = self.read_value()? else {
            return Ok(None);
        };
        let key_end = self.p;
        self.skip_blank();
        if self.peek() != Some(':') {
            return Ok(Some(FilterArg::Positional(key)));
        }
        self.bump();
        let value = self.read_value()?;
        Ok(Some(FilterArg::Keyword {
            name: slice(self.input, begin, key_end).to_string(),
            value,
        }))
    }

    /// `name: value` pairs, with optional commas, until no name reads.
    pub fn read_hashes(&mut self) -> Result<Vec<HashArg>, Located> {
        let mut out = Vec::new();
        loop {
            self.skip_blank();
            if self.peek() == Some(',') {
                self.bump();
            }
            let begin = self.p;
            let Some((nb, ne)) = self.read_non_empty_identifier() else {
                return Ok(out);
            };
            self.skip_blank();
            let mut value = None;
            if self.peek() == Some(':') {
                self.bump();
                value = self.read_value()?;
            }
            out.push(HashArg {
                span: self.span(begin, self.p),
                name: self.input[nb..ne].to_string(),
                value,
            });
        }
    }

    /// The lines of a `{% liquid %}` tag, one tag token each.
    pub fn read_liquid_tag_tokens(&mut self) -> Result<Vec<Token>, Located> {
        let mut tokens = Vec::new();
        while self.p < self.n {
            self.skip_blank();
            if self.end() {
                break;
            }
            let begin = self.p;
            self.read_to_delimiter("\n", false);
            let end = self.p;
            tokens.push(Token::Tag(TagToken::new(
                self.input,
                begin,
                end,
                begin,
                end,
                "illegal liquid tag syntax",
            )?));
        }
        Ok(tokens)
    }
}

/// Runs `f` with enough stack for one more level of nesting. Debug builds
/// use several kilobytes per level, more than a 2 MiB thread stack holds
/// for 256 levels.
pub(crate) fn grow<T>(f: impl FnOnce() -> T) -> T {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, f)
}

/// JavaScript's `Number(text)` for the numbers `read_number` accepts.
fn js_number(text: &str) -> f64 {
    let t = text.strip_prefix('+').unwrap_or(text);
    let t = t.strip_suffix('.').unwrap_or(t);
    t.parse().unwrap_or(f64::NAN)
}

fn join(a: Span, b: Span) -> Span {
    let (first, last) = if a.start <= b.start { (a, b) } else { (b, a) };
    Span {
        end: first.end.max(last.end),
        ..first
    }
}

/// LiquidJS's `toPostfix` and `Expression.evaluate`, run on syntax instead
/// of values. Operators of equal precedence group to the right. An
/// operator with no operand left gets `Missing`. The result is the bottom
/// of the stack, so in `a b` the `b` is dropped.
fn build_expression(toks: Vec<ExprTok>) -> Option<Expr> {
    let mut postfix = Vec::new();
    let mut ops: Vec<(Op, Span)> = Vec::new();
    for t in toks {
        match t {
            ExprTok::Op(op, span) => {
                while let Some(&(top, top_span)) = ops.last()
                    && top.precedence() > op.precedence()
                {
                    ops.pop();
                    postfix.push(ExprTok::Op(top, top_span));
                }
                ops.push((op, span));
            }
            v => postfix.push(v),
        }
    }
    while let Some((op, span)) = ops.pop() {
        postfix.push(ExprTok::Op(op, span));
    }
    let mut stack: Vec<Expr> = Vec::new();
    for t in postfix {
        match t {
            ExprTok::Val(v) => stack.push(v),
            ExprTok::Op(Op::Not, span) => {
                let operand = stack.pop().unwrap_or(Expr::Missing { span });
                stack.push(Expr::Not {
                    span: join(span, operand.span()),
                    operand: Box::new(operand),
                });
            }
            ExprTok::Op(Op::Binary(op), span) => {
                let rhs = stack.pop().unwrap_or(Expr::Missing { span });
                let lhs = stack.pop().unwrap_or(Expr::Missing { span });
                stack.push(Expr::Binary {
                    span: join(join(lhs.span(), span), rhs.span()),
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                });
            }
        }
    }
    stack.into_iter().next()
}

/// Reads a whole template into tokens and applies whitespace control:
/// LiquidJS's `readTopLevelTokens`.
pub(crate) fn tokenize(input: &str) -> Result<Vec<Token>, Located> {
    let mut c = Cursor::bare(input, 0, input.len());
    let mut raw_begin: Option<usize> = None;
    let mut tokens = Vec::new();
    while c.p < c.n {
        let token = if let Some(rb) = raw_begin {
            read_endraw_or_raw_content(&mut c, rb, &mut raw_begin)?
        } else if c.matches("{%") {
            let begin = c.p;
            if c.read_to_delimiter("%}", false).is_none() {
                return Err(c.error(
                    ErrorKind::Unclosed,
                    format!("tag {} not closed", c.snapshot(begin)),
                    begin,
                ));
            }
            let t = TagToken::new(
                input,
                begin,
                c.p,
                begin + 2,
                c.p - 2,
                "illegal tag syntax, tag name expected",
            )?;
            if t.name == "raw" {
                raw_begin = Some(begin);
            }
            Token::Tag(t)
        } else if c.matches("{{") {
            let begin = c.p;
            if c.read_to_delimiter("}}", true).is_none() {
                return Err(c.error(
                    ErrorKind::Unclosed,
                    format!("output {} not closed", c.snapshot(begin)),
                    begin,
                ));
            }
            let (content, trim_left, trim_right) = delimited(input, begin + 2, c.p - 2);
            Token::Output(OutputToken {
                begin,
                end: c.p,
                content,
                trim_left,
                trim_right,
            })
        } else {
            let begin = c.p;
            while c.p < c.n && !c.matches("{%") && !c.matches("{{") {
                c.bump();
            }
            Token::Html(HtmlToken {
                begin,
                end: c.p,
                trim_left: 0,
                trim_right: 0,
            })
        };
        tokens.push(token);
    }
    whitespace_control(input, &mut tokens);
    Ok(tokens)
}

/// LiquidJS's `readEndrawOrRawContent`. It finds `endraw` only right after
/// `{%` and blanks, so `{%- endraw %}` does not end a raw block.
fn read_endraw_or_raw_content(
    c: &mut Cursor<'_>,
    raw_begin: usize,
    raw_state: &mut Option<usize>,
) -> Result<Token, Located> {
    let begin = c.p;
    let left_of = |r: Option<usize>| r.map_or(-3, |p| p as i64 - 2);
    let mut left = left_of(c.read_to("{%"));
    while c.p < c.n {
        if c.read_identifier_text() != "endraw" {
            left = left_of(c.read_to("{%"));
            continue;
        }
        while c.p <= c.n {
            if c.rmatch("%}") {
                let end = c.p;
                if begin as i64 == left {
                    *raw_state = None;
                    return Ok(Token::Tag(TagToken::new(
                        c.input,
                        begin,
                        end,
                        begin + 2,
                        end - 2,
                        "illegal tag syntax, tag name expected",
                    )?));
                }
                // `left` is a real `{%` position here: it is past `begin`.
                c.p = left as usize;
                return Ok(Token::Html(HtmlToken {
                    begin,
                    end: c.p,
                    trim_left: 0,
                    trim_right: 0,
                }));
            }
            if c.rmatch("{%") {
                break;
            }
            c.bump();
        }
    }
    Err(c
        .error(
            ErrorKind::Unclosed,
            format!("raw {} not closed", c.snapshot(raw_begin)),
            begin,
        )
        .with_raw_help())
}

impl Located {
    fn with_raw_help(mut self) -> Located {
        self.help = Some(
            "close it with `{% endraw %}`; a `-` before `endraw`, as in `{%- endraw %}`, \
             is not recognized"
                .to_string(),
        );
        self
    }
}

/// LiquidJS's `whiteSpaceCtrl`, with its default greedy trimming. Dashes
/// inside a raw block are ignored.
fn whitespace_control(input: &str, tokens: &mut [Token]) {
    let mut in_raw = false;
    for i in 0..tokens.len() {
        let (tl, tr, name) = match &tokens[i] {
            Token::Html(_) => continue,
            Token::Tag(t) => (t.trim_left, t.trim_right, Some(t.name.as_str())),
            Token::Output(o) => (o.trim_left, o.trim_right, None),
        };
        let name = name.map(str::to_string);
        if !in_raw
            && tl
            && i > 0
            && let Token::Html(h) = &mut tokens[i - 1]
        {
            // Trim blanks off the end.
            loop {
                let at = h.end.saturating_sub(h.trim_right);
                match input[..at].chars().next_back() {
                    Some(ch) if at > 0 && is_blank(ch) => h.trim_right += ch.len_utf8(),
                    _ => break,
                }
            }
        }
        match name.as_deref() {
            Some("raw") => in_raw = true,
            Some("endraw") => in_raw = false,
            _ => {}
        }
        if !in_raw
            && tr
            && let Some(Token::Html(h)) = tokens.get_mut(i + 1)
        {
            loop {
                match input
                    .get(h.begin + h.trim_left..)
                    .and_then(|s| s.chars().next())
                {
                    Some(ch) if is_blank(ch) => h.trim_left += ch.len_utf8(),
                    _ => break,
                }
            }
        }
    }
}

impl HtmlToken {
    /// The text after whitespace control.
    pub fn content<'s>(&self, input: &'s str) -> &'s str {
        slice(
            input,
            self.begin + self.trim_left,
            self.end.saturating_sub(self.trim_right),
        )
    }
}
