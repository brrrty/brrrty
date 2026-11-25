//! Builds the AST from tokens, following LiquidJS 10.30.1's tag
//! constructors one for one.

use crate::ast::{
    Binding, Branch, Expr, FilePath, FilteredValue, For, Include, Layout, Literal, Node, Output,
    Render, Tablerow, Tag, TagKind, Text, Trim, When,
};
use crate::error::{ErrorKind, Located, ParseErr};
use crate::lexer::{
    Cursor, LineIndex, MAX_DEPTH, Spanner, TagToken, Token, grow, is_blank, json_string, slice,
    tokenize,
};

/// The tags LiquidJS 10.30.1 registers by default.
pub(crate) const TAGS: [&str; 21] = [
    "assign",
    "for",
    "capture",
    "case",
    "comment",
    "include",
    "render",
    "decrement",
    "increment",
    "cycle",
    "if",
    "layout",
    "block",
    "raw",
    "tablerow",
    "unless",
    "break",
    "continue",
    "echo",
    "liquid",
    "#",
];

/// One source being parsed: the template, or a quoted partial name inside it.
struct Source<'a> {
    input: &'a str,
    spans: Spanner<'a>,
}

/// The tokens left to parse. Block tags take their bodies from here.
struct Stream {
    tokens: std::vec::IntoIter<Token>,
}

impl Stream {
    fn next(&mut self) -> Option<Token> {
        self.tokens.next()
    }
}

pub(crate) struct Parser<'f> {
    depth: usize,
    check: Check<'f>,
    /// The names of the raw block tags the engine has.
    raw_blocks: &'f [String],
    /// The shortcode tags the engine has, and whether each is paired.
    shortcodes: &'f [(String, bool)],
}

type Res<T> = Result<T, ParseErr>;

/// Checks a filtered value as LiquidJS's `Value` constructor does, which
/// is where `strictFilters` rejects unknown filters. `None` checks nothing.
pub(crate) type Check<'f> = Option<&'f dyn Fn(&FilteredValue) -> Res<()>>;

/// Runs `check` on a value LiquidJS builds a `Value` from.
fn checked(check: Check<'_>, value: FilteredValue) -> Res<FilteredValue> {
    if let Some(check) = check {
        check(&value)?;
    }
    Ok(value)
}

impl<'f> Parser<'f> {
    pub fn new(
        check: Check<'f>,
        raw_blocks: &'f [String],
        shortcodes: &'f [(String, bool)],
    ) -> Parser<'f> {
        Parser {
            depth: 0,
            check,
            raw_blocks,
            shortcodes,
        }
    }

    /// LiquidJS's `Parser.parse`: tokenize everything first, then parse.
    pub fn parse_source(&mut self, input: &str) -> Result<Vec<Node>, Located> {
        let src = Source {
            input,
            spans: Spanner::Index(LineIndex::new(input)),
        };
        self.parse_in(&src)
    }

    fn parse_in(&mut self, src: &Source<'_>) -> Result<Vec<Node>, Located> {
        let tokens = tokenize(src.input)?;
        let mut stream = Stream {
            tokens: tokens.into_iter(),
        };
        self.parse_tokens(src, &mut stream)
    }

    fn parse_tokens(
        &mut self,
        src: &Source<'_>,
        stream: &mut Stream,
    ) -> Result<Vec<Node>, Located> {
        let mut nodes = Vec::new();
        while let Some(token) = stream.next() {
            nodes.push(self.parse_token(src, token, stream)?);
        }
        Ok(nodes)
    }

    /// LiquidJS's `parseToken`. A `Plain` error raised while parsing this
    /// token, and not inside a nested one, takes this token's position.
    fn parse_token(
        &mut self,
        src: &Source<'_>,
        token: Token,
        stream: &mut Stream,
    ) -> Result<Node, Located> {
        match token {
            Token::Html(h) => Ok(Node::Text(Text {
                span: src.spans.span(h.begin, h.end),
                text: h.content(src.input).to_string(),
            })),
            Token::Output(o) => {
                let mut c = Cursor::new(src.input, o.content.0, o.content.1, &src.spans);
                let value = c.read_filtered_value()?;
                let value = checked(self.check, value).map_err(|e| e.locate(o.begin))?;
                Ok(Node::Output(Output {
                    span: src.spans.span(o.begin, o.end),
                    trim: Trim {
                        left: o.trim_left,
                        right: o.trim_right,
                    },
                    value,
                }))
            }
            Token::Tag(t) => {
                if self.depth >= MAX_DEPTH {
                    return Err(Located {
                        kind: ErrorKind::TooDeep,
                        message: format!(
                            "templates nest too deeply (more than {MAX_DEPTH} levels)"
                        ),
                        help: None,
                        offset: t.begin,
                    });
                }
                self.depth += 1;
                let kind = grow(|| self.parse_tag(src, &t, stream));
                self.depth -= 1;
                let kind = kind.map_err(|e| e.locate(t.begin))?;
                Ok(Node::Tag(Box::new(Tag {
                    span: src.spans.span(t.begin, t.end),
                    trim: Trim {
                        left: t.trim_left,
                        right: t.trim_right,
                    },
                    name: t.name.clone(),
                    kind,
                })))
            }
        }
    }

    /// Picks the tag. New tags are added here.
    fn parse_tag(&mut self, src: &Source<'_>, t: &TagToken, stream: &mut Stream) -> Res<TagKind> {
        let c = Cursor::new(src.input, t.args_begin, t.content.1, &src.spans);
        match t.name.as_str() {
            "assign" => assign(c, self.check),
            "for" => self.for_tag(src, t, c, stream),
            "capture" => self.capture(src, t, c, stream),
            "case" => self.case(src, t, c, stream),
            "comment" => comment(src, t, stream),
            "include" => self.include(src, c),
            "render" => self.render(src, c),
            "decrement" => Ok(TagKind::Decrement {
                name: identifier(c),
            }),
            "increment" => Ok(TagKind::Increment {
                name: identifier(c),
            }),
            "cycle" => cycle(src, t, c),
            "if" => self.if_tag(src, t, c, stream),
            "layout" => self.layout(src, c, stream),
            "block" => self.block(src, t, stream),
            "raw" => raw(src, t, stream),
            "tablerow" => self.tablerow(src, t, c, stream),
            "unless" => self.unless(src, t, c, stream),
            "break" => Ok(TagKind::Break),
            "continue" => Ok(TagKind::Continue),
            "echo" => echo(c, self.check),
            "liquid" => self.liquid(src, c),
            "#" => inline_comment(src, t),
            name if self.raw_blocks.iter().any(|b| b == name) => raw_block(src, t, stream),
            name if self.shortcodes.iter().any(|(b, _)| b == name) => {
                let paired = self.shortcodes.iter().any(|(b, p)| b == name && *p);
                self.shortcode(src, t, c, stream, paired)
            }
            _ => Err(unknown_tag(&t.name)),
        }
    }

    /// A shortcode: 11ty's legacy argument parsing, values separated by commas or blanks.
    fn shortcode(
        &mut self,
        src: &Source<'_>,
        t: &TagToken,
        mut c: Cursor<'_>,
        stream: &mut Stream,
        paired: bool,
    ) -> Res<TagKind> {
        let mut args = Vec::new();
        loop {
            while c.peek().is_some_and(|ch| ch == ',' || is_blank(ch)) {
                c.advance();
            }
            if c.end() {
                break;
            }
            match c.read_value()? {
                Some(e) => args.push(e),
                None => {
                    return Err(c
                        .error(
                            ErrorKind::Syntax,
                            format!("invalid argument of the shortcode `{}`", t.name),
                            c.p,
                        )
                        .into());
                }
            }
        }
        let body = if paired {
            Some(self.body_until(src, t, &format!("end{}", t.name), stream)?)
        } else {
            None
        };
        Ok(TagKind::Shortcode { args, body })
    }

    /// The next node of a block body, or `None` when `handle` took the token.
    fn next_in_block(
        &mut self,
        src: &Source<'_>,
        open: &TagToken,
        stream: &mut Stream,
        mut handle: impl FnMut(&TagToken) -> Res<Handled>,
    ) -> Res<Step> {
        let Some(token) = stream.next() else {
            return Err(not_closed(src, open));
        };
        if let Token::Tag(t) = &token {
            match handle(t)? {
                Handled::No => {}
                Handled::Yes => return Ok(Step::Handled),
                Handled::Stop => return Ok(Step::Stop),
            }
        }
        Ok(Step::Node(self.parse_token(src, token, stream)?))
    }

    fn if_tag(
        &mut self,
        src: &Source<'_>,
        t: &TagToken,
        mut c: Cursor<'_>,
        stream: &mut Stream,
    ) -> Res<TagKind> {
        let check = self.check;
        let mut branches = vec![Branch {
            span: src.spans.span(t.begin, t.end),
            condition: checked(check, c.read_filtered_value()?)?,
            body: Vec::new(),
        }];
        let mut else_body: Option<Vec<Node>> = None;
        loop {
            let step = self.next_in_block(src, t, stream, |tag| match tag.name.as_str() {
                "elsif" => {
                    if else_body.is_some() {
                        return Err(ParseErr::plain(
                            ErrorKind::Syntax,
                            "unexpected elsif after else",
                        ));
                    }
                    let mut c = Cursor::new(src.input, tag.args_begin, tag.content.1, &src.spans);
                    branches.push(Branch {
                        span: src.spans.span(tag.begin, tag.end),
                        condition: checked(check, c.read_filtered_value()?)?,
                        body: Vec::new(),
                    });
                    Ok(Handled::Yes)
                }
                "else" => {
                    assert_empty(src, tag)?;
                    if else_body.is_some() {
                        return Err(ParseErr::plain(ErrorKind::Syntax, "duplicated else"));
                    }
                    else_body = Some(Vec::new());
                    Ok(Handled::Yes)
                }
                "endif" => {
                    assert_empty(src, tag)?;
                    Ok(Handled::Stop)
                }
                _ => Ok(Handled::No),
            })?;
            match step {
                Step::Stop => break,
                Step::Handled => {}
                Step::Node(n) => match &mut else_body {
                    Some(b) => b.push(n),
                    None => branches.last_mut().expect("if has a branch").body.push(n),
                },
            }
        }
        Ok(TagKind::If {
            branches,
            else_body,
        })
    }

    fn unless(
        &mut self,
        src: &Source<'_>,
        t: &TagToken,
        mut c: Cursor<'_>,
        stream: &mut Stream,
    ) -> Res<TagKind> {
        let check = self.check;
        let mut branches = vec![Branch {
            span: src.spans.span(t.begin, t.end),
            condition: checked(check, c.read_filtered_value()?)?,
            body: Vec::new(),
        }];
        let mut else_body = Vec::new();
        let mut else_count = 0;
        let mut sink = Sink::Branch;
        loop {
            let step = self.next_in_block(src, t, stream, |tag| match tag.name.as_str() {
                "elsif" => {
                    if else_count > 0 {
                        sink = Sink::Discard;
                        return Ok(Handled::Yes);
                    }
                    let mut c = Cursor::new(src.input, tag.args_begin, tag.content.1, &src.spans);
                    branches.push(Branch {
                        span: src.spans.span(tag.begin, tag.end),
                        condition: checked(check, c.read_filtered_value()?)?,
                        body: Vec::new(),
                    });
                    sink = Sink::Branch;
                    Ok(Handled::Yes)
                }
                "else" => {
                    else_count += 1;
                    sink = Sink::Else;
                    Ok(Handled::Yes)
                }
                "endunless" => Ok(Handled::Stop),
                _ => Ok(Handled::No),
            })?;
            match step {
                Step::Stop => break,
                Step::Handled => {}
                Step::Node(n) => match sink {
                    Sink::Branch => branches
                        .last_mut()
                        .expect("unless has a branch")
                        .body
                        .push(n),
                    Sink::Else if else_count == 1 => else_body.push(n),
                    Sink::Else | Sink::Discard => {}
                },
            }
        }
        Ok(TagKind::Unless {
            branches,
            else_body,
        })
    }

    fn case(
        &mut self,
        src: &Source<'_>,
        t: &TagToken,
        mut c: Cursor<'_>,
        stream: &mut Stream,
    ) -> Res<TagKind> {
        let value = checked(self.check, c.read_filtered_value()?)?;
        let mut whens: Vec<When> = Vec::new();
        let mut else_body = Vec::new();
        let mut else_count = 0;
        // Nodes before the first `when` are parsed, then dropped.
        let mut sink = Sink::Discard;
        loop {
            let step = self.next_in_block(src, t, stream, |tag| match tag.name.as_str() {
                "when" => {
                    if else_count > 0 {
                        return Ok(Handled::Yes);
                    }
                    let mut wc = Cursor::new(src.input, tag.args_begin, tag.content.1, &src.spans);
                    let mut values = Vec::new();
                    while !wc.end() {
                        values.push(wc.read_value_or_throw()?);
                        wc.skip_blank();
                        if wc.peek() == Some(',') {
                            wc.read_to(",");
                        } else {
                            wc.read_to("or");
                        }
                    }
                    whens.push(When {
                        span: src.spans.span(tag.begin, tag.end),
                        values,
                        body: Vec::new(),
                    });
                    sink = Sink::Branch;
                    Ok(Handled::Yes)
                }
                "else" => {
                    else_count += 1;
                    sink = Sink::Else;
                    Ok(Handled::Yes)
                }
                "endcase" => Ok(Handled::Stop),
                _ => Ok(Handled::No),
            })?;
            match step {
                Step::Stop => break,
                Step::Handled => {}
                Step::Node(n) => match sink {
                    Sink::Branch => whens.last_mut().expect("a when was read").body.push(n),
                    Sink::Else if else_count == 1 => else_body.push(n),
                    Sink::Else | Sink::Discard => {}
                },
            }
        }
        Ok(TagKind::Case {
            value,
            whens,
            else_body,
        })
    }

    fn for_tag(
        &mut self,
        src: &Source<'_>,
        t: &TagToken,
        mut c: Cursor<'_>,
        stream: &mut Stream,
    ) -> Res<TagKind> {
        let (vb, ve) = c.read_identifier();
        let in_word = c.read_identifier_text();
        c.skip_blank();
        let cb = c.p;
        let collection = c.read_value()?;
        let ce = c.p;
        let (Some(collection), true, true) = (collection, ve > vb, in_word == "in") else {
            return Err(illegal_tag(src, t));
        };
        let args = c.read_hashes()?;
        let mut body = Vec::new();
        let mut else_body = Vec::new();
        let mut in_else = false;
        loop {
            let step = self.next_in_block(src, t, stream, |tag| match tag.name.as_str() {
                "else" => {
                    assert_empty(src, tag)?;
                    in_else = true;
                    Ok(Handled::Yes)
                }
                "endfor" => {
                    assert_empty(src, tag)?;
                    Ok(Handled::Stop)
                }
                _ => Ok(Handled::No),
            })?;
            match step {
                Step::Stop => break,
                Step::Handled => {}
                Step::Node(n) if in_else => else_body.push(n),
                Step::Node(n) => body.push(n),
            }
        }
        Ok(TagKind::For(For {
            variable: src.input[vb..ve].to_string(),
            collection,
            collection_text: slice(src.input, cb, ce).to_string(),
            args,
            body,
            else_body,
        }))
    }

    fn tablerow(
        &mut self,
        src: &Source<'_>,
        t: &TagToken,
        mut c: Cursor<'_>,
        stream: &mut Stream,
    ) -> Res<TagKind> {
        let variable = c.read_identifier_text().to_string();
        c.skip_blank();
        let predicate = c.read_identifier_text();
        c.skip_blank();
        let cb = c.p;
        let collection = c.read_value()?;
        let ce = c.p;
        let (Some(collection), true) = (collection, predicate == "in") else {
            return Err(illegal_tag(src, t));
        };
        let args = c.read_hashes()?;
        let mut body = Vec::new();
        loop {
            let step = self.next_in_block(src, t, stream, |tag| {
                Ok(if tag.name == "endtablerow" {
                    Handled::Stop
                } else {
                    Handled::No
                })
            })?;
            match step {
                Step::Stop => break,
                Step::Handled => {}
                Step::Node(n) => body.push(n),
            }
        }
        Ok(TagKind::Tablerow(Tablerow {
            variable,
            collection,
            collection_text: slice(src.input, cb, ce).to_string(),
            args,
            body,
        }))
    }

    /// Parses tokens up to a tag named `end`, as `capture` and `block` do.
    fn body_until(
        &mut self,
        src: &Source<'_>,
        t: &TagToken,
        end: &str,
        stream: &mut Stream,
    ) -> Res<Vec<Node>> {
        let mut body = Vec::new();
        loop {
            let step = self.next_in_block(src, t, stream, |tag| {
                Ok(if tag.name == end {
                    Handled::Stop
                } else {
                    Handled::No
                })
            })?;
            match step {
                Step::Stop => return Ok(body),
                Step::Handled => {}
                Step::Node(n) => body.push(n),
            }
        }
    }

    fn capture(
        &mut self,
        src: &Source<'_>,
        t: &TagToken,
        mut c: Cursor<'_>,
        stream: &mut Stream,
    ) -> Res<TagKind> {
        let ident = c.read_identifier_text();
        let name = if !ident.is_empty() {
            ident.to_string()
        } else if let Some((_, s)) = c.read_quoted() {
            s
        } else {
            return Err(c
                .error(ErrorKind::Syntax, "invalid capture name", c.p)
                .into());
        };
        let body = self.body_until(src, t, "endcapture", stream)?;
        Ok(TagKind::Capture { name, body })
    }

    fn block(&mut self, src: &Source<'_>, t: &TagToken, stream: &mut Stream) -> Res<TagKind> {
        // LiquidJS takes the first `\w+` anywhere in the arguments.
        let name = t
            .args(src.input)
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .find(|w| !w.is_empty())
            .unwrap_or("")
            .to_string();
        let body = self.body_until(src, t, "endblock", stream)?;
        Ok(TagKind::Block { name, body })
    }

    fn layout(&mut self, src: &Source<'_>, mut c: Cursor<'_>, stream: &mut Stream) -> Res<TagKind> {
        let file = self.file_path(src, &mut c)?;
        let args = c.read_hashes()?;
        // The layout takes every token left in this template or block.
        let body = self.parse_tokens(src, stream)?;
        Ok(TagKind::Layout(Layout { file, args, body }))
    }

    fn include(&mut self, src: &Source<'_>, mut c: Cursor<'_>) -> Res<TagKind> {
        let file = self.file_path(src, &mut c)?;
        let begin = c.p;
        let mut with = None;
        if c.read_identifier_text() == "with" {
            c.skip_blank();
            if c.peek() != Some(':') {
                with = c.read_value()?;
            } else {
                c.p = begin;
            }
        } else {
            c.p = begin;
        }
        let args = c.read_hashes()?;
        Ok(TagKind::Include(Include { file, with, args }))
    }

    fn render(&mut self, src: &Source<'_>, mut c: Cursor<'_>) -> Res<TagKind> {
        let file = self.file_path(src, &mut c)?;
        let mut with = None;
        let mut for_binding = None;
        while !c.end() {
            c.skip_blank();
            let begin = c.p;
            let keyword = c.read_identifier_text();
            if keyword == "with" || keyword == "for" {
                c.skip_blank();
                if c.peek() != Some(':') {
                    let vb = c.p;
                    if let Some(value) = c.read_value()? {
                        let text = slice(src.input, vb, c.p).to_string();
                        let before_as = c.p;
                        let alias = if c.read_identifier_text() == "as" {
                            Some(c.read_identifier_text().to_string())
                        } else {
                            c.p = before_as;
                            None
                        };
                        let binding = Binding { value, text, alias };
                        if keyword == "with" {
                            with = Some(binding);
                        } else {
                            for_binding = Some(binding);
                        }
                        c.skip_blank();
                        if c.peek() == Some(',') {
                            c.advance();
                        }
                        continue;
                    }
                }
            }
            c.p = begin;
            break;
        }
        let args = c.read_hashes()?;
        Ok(TagKind::Render(Render {
            file,
            with,
            for_binding,
            args,
        }))
    }

    /// LiquidJS's `parseFilePath` with `dynamicPartials` on, as 11ty runs it.
    fn file_path(&mut self, src: &Source<'_>, c: &mut Cursor<'_>) -> Res<FilePath> {
        c.skip_blank();
        let begin = c.p;
        let Some(file) = c.read_value()? else {
            return Err(c.error(ErrorKind::Syntax, "illegal file path", c.p).into());
        };
        if slice(src.input, begin, c.p) == "none" {
            return Ok(FilePath::None);
        }
        let Expr::Literal {
            span,
            value: Literal::String(name),
        } = file
        else {
            return Ok(FilePath::Expr(file));
        };
        // A quoted name is itself parsed as a template. LiquidJS reports
        // errors in it at positions inside the name; this points at the
        // name in the template instead.
        let inner = Source {
            input: &name,
            spans: Spanner::Fixed(span),
        };
        let mut nodes = self.parse_in(&inner).map_err(|mut e| {
            e.offset = begin;
            e
        })?;
        if nodes.len() == 1
            && let Node::Text(t) = &mut nodes[0]
        {
            return Ok(FilePath::Name(std::mem::take(&mut t.text)));
        }
        Ok(FilePath::Template(nodes))
    }

    fn liquid(&mut self, src: &Source<'_>, mut c: Cursor<'_>) -> Res<TagKind> {
        let tokens = c.read_liquid_tag_tokens()?;
        let mut stream = Stream {
            tokens: tokens.into_iter(),
        };
        let body = self.parse_tokens(src, &mut stream)?;
        Ok(TagKind::Liquid { body })
    }
}

enum Handled {
    No,
    Yes,
    Stop,
}

enum Step {
    Node(Node),
    Handled,
    Stop,
}

/// Where a block's next node goes.
#[derive(Clone, Copy)]
enum Sink {
    Branch,
    Else,
    Discard,
}

fn identifier(mut c: Cursor<'_>) -> String {
    c.read_identifier_text().to_string()
}

fn assign(mut c: Cursor<'_>, check: Check<'_>) -> Res<TagKind> {
    let name = c.read_identifier_text().to_string();
    if name.is_empty() {
        return Err(c
            .error(ErrorKind::Syntax, "expected variable name", c.p)
            .into());
    }
    c.skip_blank();
    if c.peek() != Some('=') {
        return Err(c.error(ErrorKind::Syntax, "expected \"=\"", c.p).into());
    }
    c.advance();
    let value = checked(check, c.read_filtered_value()?)?;
    Ok(TagKind::Assign { name, value })
}

fn echo(mut c: Cursor<'_>, check: Check<'_>) -> Res<TagKind> {
    c.skip_blank();
    let value = if c.end() {
        None
    } else {
        Some(checked(check, c.read_filtered_value()?)?)
    };
    Ok(TagKind::Echo { value })
}

fn cycle(src: &Source<'_>, t: &TagToken, mut c: Cursor<'_>) -> Res<TagKind> {
    let begin = c.p;
    let mut group = c.read_value()?;
    c.skip_blank();
    if group.is_some() {
        if c.peek() == Some(':') {
            c.advance();
        } else {
            c.p = begin;
            group = None;
        }
    }
    let mut candidates = Vec::new();
    while !c.end() {
        candidates.push(c.read_filtered_value()?);
        c.read_to(",");
    }
    if candidates.is_empty() {
        let text = slice(src.input, t.begin, t.end);
        return Err(c
            .error(
                ErrorKind::Syntax,
                format!("empty candidates: \"{text}\""),
                c.p,
            )
            .into());
    }
    Ok(TagKind::Cycle { group, candidates })
}

fn comment(src: &Source<'_>, t: &TagToken, stream: &mut Stream) -> Res<TagKind> {
    while let Some(token) = stream.next() {
        if let Token::Tag(tag) = &token
            && tag.name == "endcomment"
        {
            return Ok(TagKind::Comment);
        }
    }
    Err(not_closed(src, t))
}

fn raw(src: &Source<'_>, t: &TagToken, stream: &mut Stream) -> Res<TagKind> {
    let mut text = String::new();
    while let Some(token) = stream.next() {
        if let Token::Tag(tag) = &token
            && tag.name == "endraw"
        {
            return Ok(TagKind::Raw { text });
        }
        text.push_str(slice(src.input, token.begin(), token.end()));
    }
    Err(not_closed(src, t))
}

/// A raw block tag: every token up to the first `end<name>` tag, as
/// written, the way a LiquidJS tag that reads `parseStream` tokens and joins
/// their text sees it.
fn raw_block(src: &Source<'_>, t: &TagToken, stream: &mut Stream) -> Res<TagKind> {
    let end = format!("end{}", t.name);
    let mut body = String::new();
    while let Some(token) = stream.next() {
        if let Token::Tag(tag) = &token
            && tag.name == end
        {
            return Ok(TagKind::RawBlock {
                args: t.args(src.input).to_string(),
                body,
            });
        }
        body.push_str(slice(src.input, token.begin(), token.end()));
    }
    Err(not_closed(src, t))
}

fn inline_comment(src: &Source<'_>, t: &TagToken) -> Res<TagKind> {
    // LiquidJS rejects `/\n\s*[^#\s]/` in the arguments.
    let args = t.args(src.input);
    for (i, _) in args.match_indices('\n') {
        let rest = args[i + 1..].trim_start_matches(js_space);
        if rest.chars().next().is_some_and(|ch| ch != '#') {
            return Err(ParseErr::plain(
                ErrorKind::Syntax,
                "every line of an inline comment must start with a '#' character",
            ));
        }
    }
    Ok(TagKind::InlineComment)
}

/// JavaScript's regex `\s`.
fn js_space(c: char) -> bool {
    brrrty_js::is_js_whitespace(c)
}

/// LiquidJS's `assertEmpty(tag.args)`.
fn assert_empty(src: &Source<'_>, tag: &TagToken) -> Res<()> {
    let args = tag.args(src.input);
    if args.is_empty() {
        return Ok(());
    }
    let units: Vec<u16> = args.encode_utf16().collect();
    Err(ParseErr::plain(
        ErrorKind::Syntax,
        format!("unexpected {}", json_string(&units)),
    ))
}

fn illegal_tag(src: &Source<'_>, t: &TagToken) -> ParseErr {
    ParseErr::plain(
        ErrorKind::Syntax,
        format!("illegal tag: {}", slice(src.input, t.begin, t.end)),
    )
}

fn not_closed(src: &Source<'_>, t: &TagToken) -> ParseErr {
    let closer = match t.name.as_str() {
        "if" | "unless" | "case" | "for" | "tablerow" | "capture" | "comment" | "raw" | "block" => {
            format!("add `{{% end{} %}}` to close it", t.name)
        }
        _ => format!("add `end{}` to close it", t.name),
    };
    ParseErr::plain(
        ErrorKind::Unclosed,
        format!("tag {} not closed", slice(src.input, t.begin, t.end)),
    )
    .with_help(closer)
}

const BLOCKS: [&str; 9] = [
    "if", "unless", "case", "for", "tablerow", "capture", "comment", "raw", "block",
];

fn unknown_tag(name: &str) -> ParseErr {
    let err = ParseErr::plain(ErrorKind::UnknownTag, format!("tag \"{name}\" not found"));
    if let Some(opener) = name.strip_prefix("end")
        && BLOCKS.contains(&opener)
    {
        return err.with_help(format!(
            "there is no open `{{% {opener} %}}` for this `{{% {name} %}}` to close"
        ));
    }
    let inside = match name {
        "else" => Some("if, unless, case or for"),
        "elsif" => Some("if or unless"),
        "when" => Some("case"),
        _ => None,
    };
    if let Some(inside) = inside {
        return err.with_help(format!("`{{% {name} %}}` only works inside {inside}"));
    }
    let known = TAGS
        .iter()
        .copied()
        .chain(["elsif", "else", "when"])
        .chain(BLOCKS.iter().map(|b| match *b {
            "if" => "endif",
            "unless" => "endunless",
            "case" => "endcase",
            "for" => "endfor",
            "tablerow" => "endtablerow",
            "capture" => "endcapture",
            "comment" => "endcomment",
            "raw" => "endraw",
            _ => "endblock",
        }));
    match known
        .map(|k| (distance(name, k), k))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
    {
        Some((_, k)) => err.with_help(format!("did you mean `{k}`?")),
        None => err,
    }
}

/// Levenshtein distance.
pub(crate) fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = if ca == *cb {
                prev
            } else {
                1 + prev.min(row[j]).min(cur)
            };
            prev = cur;
        }
    }
    row[b.len()]
}
