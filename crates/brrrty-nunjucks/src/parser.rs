//! A port of Nunjucks 3.2.4's `parser.js`, method for method.
//!
//! The parser pulls tokens from the lexer one at a time with one token of
//! lookahead, as Nunjucks does, so errors come out in the same order and
//! with the same positions. Where Nunjucks's parser throws a JavaScript
//! error instead of a template error, such as reading `.type` of the
//! `null` it gets at the end of the source, this returns the same message.
//!
//! Two bounds Nunjucks does not have: tags may nest at most
//! [`MAX_DEPTH`] deep, and so may expressions, counting each operator in a
//! chain such as `a + b + c` as a level, because the tree nests that deep.

use crate::ast::{
    Args, BinaryOp, Block, Branch, Capture, Case, CompareOp, CompareOperand, Expr, Extends, For,
    ForKind, FromImport, If, Import, ImportName, Include, Literal, Macro, Node, Output, Pair, Set,
    SetValue, Span, Switch, Text, UnaryOp,
};
use crate::error::{ErrorKind, Fail};
use crate::lexer::{Kind, Lexer, Token};

/// The deepest tags or expressions may nest.
pub(crate) const MAX_DEPTH: usize = 256;

/// The tags 11ty 3.1.8 adds to Nunjucks by default: the `setAsync` paired
/// shortcode and the bundle plugin's `getBundle` and `getBundleFileUrl`.
const ELEVENTY_TAGS: [&str; 3] = ["setAsync", "getBundle", "getBundleFileUrl"];

type Res<T> = Result<T, Fail>;

/// JavaScript's `\s`, for trimming text.
fn js_space(c: char) -> bool {
    brrrty_js::is_js_whitespace(c)
}

fn grow<R>(f: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(256 * 1024, 2 * 1024 * 1024, f)
}

/// An expression and how deep its tree is.
struct P {
    expr: Expr,
    height: usize,
}

fn too_deep(span: Span, what: &str) -> Fail {
    Fail {
        kind: ErrorKind::TooDeep,
        message: format!("{what} nest too deeply (more than {MAX_DEPTH} levels)"),
        at: Some((span.line, span.column)),
        help: None,
    }
}

/// A node over `children`, refused when the tree gets too deep.
fn mk(expr: Expr, children: &[usize]) -> Res<P> {
    let height = 1 + children.iter().copied().max().unwrap_or(0);
    if height > MAX_DEPTH {
        return Err(too_deep(expr.span(), "expressions"));
    }
    Ok(P { expr, height })
}

fn leaf(expr: Expr) -> P {
    P { expr, height: 1 }
}

pub(crate) fn parse(source: &str, shortcodes: &Shortcodes) -> Res<Vec<Node>> {
    let mut parser = Parser {
        shortcodes,
        tokens: Lexer::new(source),
        peeked: None,
        break_on_blocks: None,
        drop_leading_whitespace: false,
        tag_depth: 0,
        expr_depth: 0,
        last: Span::default(),
    };
    parser.parse_nodes()
}

/// The shortcodes the config added, by name: `None` for a plain one, the names that end the
/// body (`end<name>`) for a paired one.
pub type Shortcodes = std::collections::HashMap<String, Option<&'static [&'static str]>>;

/// What a shortcode's call is named in the tree: a filter of this prefix
/// and the shortcode's name. The engine registers each shortcode under it.
pub(crate) const SHORTCODE_PREFIX: &str = "\u{0}shortcode:";

struct Parser<'a> {
    shortcodes: &'a Shortcodes,
    tokens: Lexer<'a>,
    peeked: Option<Token>,
    break_on_blocks: Option<&'static [&'static str]>,
    drop_leading_whitespace: bool,
    tag_depth: usize,
    expr_depth: usize,
    /// The last token read, for positioning depth errors.
    last: Span,
}

impl Parser<'_> {
    // Token handling.

    fn next_token(&mut self) -> Res<Option<Token>> {
        let tok = match self.peeked.take() {
            Some(tok) => Some(tok),
            None => loop {
                match self.tokens.next_token()? {
                    Some(t) if t.kind == Kind::Whitespace => continue,
                    other => break other,
                }
            },
        };
        if let Some(t) = &tok {
            self.last = t.span;
        }
        Ok(tok)
    }

    fn peek_token(&mut self) -> Res<Option<&Token>> {
        if self.peeked.is_none() {
            self.peeked = self.next_token()?;
        }
        Ok(self.peeked.as_ref())
    }

    /// `peekToken().type`, which throws at the end of the source.
    fn peek_kind(&mut self) -> Res<Kind> {
        match self.peek_token()? {
            Some(t) => Ok(t.kind),
            None => Err(Fail::null_read("type")),
        }
    }

    fn push_token(&mut self, tok: Option<Token>) -> Res<()> {
        if self.peeked.is_some() {
            return Err(Fail::js(
                "Error",
                "pushToken: can only push one token on between reads",
            ));
        }
        self.peeked = tok;
        Ok(())
    }

    /// `parser.error`: without a position, the next token's, if any.
    fn error(&mut self, msg: impl Into<String>, at: Option<(usize, usize)>) -> Fail {
        let msg = msg.into();
        let at = match at {
            Some(at) => Some(at),
            None => match self.peek_token() {
                Ok(tok) => tok.map(Token::at),
                Err(e) => return e,
            },
        };
        Fail::template(msg, at)
    }

    fn fail<T>(&mut self, msg: impl Into<String>, at: Option<(usize, usize)>) -> Res<T> {
        Err(self.error(msg, at))
    }

    fn skip(&mut self, kind: Kind) -> Res<bool> {
        let tok = self.next_token()?;
        if tok.as_ref().is_some_and(|t| t.kind == kind) {
            return Ok(true);
        }
        self.push_token(tok)?;
        Ok(false)
    }

    fn expect(&mut self, kind: Kind) -> Res<Token> {
        let Some(tok) = self.next_token()? else {
            return Err(Fail::null_read("type"));
        };
        if tok.kind != kind {
            let msg = format!("expected {}, got {}", kind.name(), tok.kind.name());
            return self.fail(msg, Some(tok.at()));
        }
        Ok(tok)
    }

    fn skip_value(&mut self, kind: Kind, value: &str) -> Res<bool> {
        let tok = self.next_token()?;
        if tok
            .as_ref()
            .is_some_and(|t| t.kind == kind && t.value == value)
        {
            return Ok(true);
        }
        self.push_token(tok)?;
        Ok(false)
    }

    fn skip_symbol(&mut self, value: &str) -> Res<bool> {
        self.skip_value(Kind::Symbol, value)
    }

    /// The next token, which the caller has peeked.
    fn take(&mut self) -> Res<Token> {
        Ok(self.next_token()?.expect("a peeked token"))
    }

    /// The peeked token, cloned, or `None` at the end.
    fn peek_cloned(&mut self) -> Res<Option<Token>> {
        Ok(self.peek_token()?.cloned())
    }

    /// The peeked token, which the caller knows is there.
    fn peek_known(&mut self) -> Res<Token> {
        Ok(self.peek_cloned()?.expect("a token"))
    }

    fn advance_after_block_end(&mut self, name: Option<&str>) -> Res<Token> {
        let name = match name {
            Some(n) => n.to_string(),
            None => {
                let Some(tok) = self.peek_token()? else {
                    return self.fail("unexpected end of file", None);
                };
                if tok.kind != Kind::Symbol {
                    return self.fail(
                        "advanceAfterBlockEnd: expected symbol token or explicit name to be passed",
                        None,
                    );
                }
                self.take()?.value
            }
        };
        match self.next_token()? {
            Some(tok) if tok.kind == Kind::BlockEnd => {
                if tok.value.starts_with('-') {
                    self.drop_leading_whitespace = true;
                }
                Ok(tok)
            }
            _ => self.fail(format!("expected block end in {name} statement"), None),
        }
    }

    fn advance_after_variable_end(&mut self) -> Res<()> {
        match self.next_token()? {
            Some(tok) if tok.kind == Kind::VariableEnd => {
                self.drop_leading_whitespace = tok.value.starts_with('-');
                Ok(())
            }
            tok => {
                self.push_token(tok)?;
                self.fail("expected variable end", None)
            }
        }
    }

    // Statements.

    fn parse_for(&mut self) -> Res<Node> {
        let for_tok = self.peek_known()?;
        let (kind, end_block) = if self.skip_symbol("for")? {
            (ForKind::For, "endfor")
        } else if self.skip_symbol("asyncEach")? {
            (ForKind::AsyncEach, "endeach")
        } else if self.skip_symbol("asyncAll")? {
            (ForKind::AsyncAll, "endall")
        } else {
            return self.fail("parseFor: expected for{Async}", Some(for_tok.at()));
        };
        let first = self.parse_primary(false)?.expr;
        if !matches!(first, Expr::Symbol { .. }) {
            return self.fail("parseFor: variable name expected for loop", None);
        }
        let mut targets = vec![first];
        if self.peek_kind()? == Kind::Comma {
            while self.skip(Kind::Comma)? {
                targets.push(self.parse_primary(false)?.expr);
            }
        }
        if !self.skip_symbol("in")? {
            return self.fail(
                "parseFor: expected \"in\" keyword for loop",
                Some(for_tok.at()),
            );
        }
        let iter = self.parse_expression()?.expr;
        self.advance_after_block_end(Some(&for_tok.value))?;
        let (body_end, else_end): (&'static [&'static str], &'static [&'static str]) =
            match end_block {
                "endfor" => (&["endfor", "else"], &["endfor"]),
                "endeach" => (&["endeach", "else"], &["endeach"]),
                _ => (&["endall", "else"], &["endall"]),
            };
        let body = self.parse_until_blocks(body_end, for_tok.span)?;
        let mut else_body = None;
        if self.skip_symbol("else")? {
            self.advance_after_block_end(Some("else"))?;
            else_body = Some(self.parse_until_blocks(else_end, for_tok.span)?);
        }
        self.advance_after_block_end(None)?;
        Ok(Node::For(For {
            span: for_tok.span,
            kind,
            targets,
            iter,
            body,
            else_body,
        }))
    }

    fn parse_macro(&mut self) -> Res<Node> {
        let macro_tok = self.peek_known()?;
        if !self.skip_symbol("macro")? {
            return self.fail("expected macro", None);
        }
        let name = self.parse_primary(true)?.expr;
        let args = self
            .parse_signature(false)?
            .map(|a| a.0)
            .unwrap_or_default();
        self.advance_after_block_end(Some(&macro_tok.value))?;
        let body = self.parse_until_blocks(&["endmacro"], macro_tok.span)?;
        self.advance_after_block_end(None)?;
        Ok(Node::Macro(Macro {
            span: macro_tok.span,
            name,
            args,
            body,
        }))
    }

    fn parse_call(&mut self) -> Res<Node> {
        let call_tok = self.peek_known()?;
        if !self.skip_symbol("call")? {
            return self.fail("expected call", None);
        }
        let caller_args = self.parse_signature(true)?.map(|a| a.0).unwrap_or_default();
        let macro_call = self.parse_primary(false)?.expr;
        self.advance_after_block_end(Some(&call_tok.value))?;
        let body = self.parse_until_blocks(&["endcall"], call_tok.span)?;
        self.advance_after_block_end(None)?;
        let span = call_tok.span;
        let caller = Expr::Caller {
            span,
            args: caller_args,
            body,
        };
        // `macroCall.args.children`: only a call has arguments.
        let Expr::Call {
            span: call_span,
            callee,
            mut args,
        } = macro_call
        else {
            return Err(Fail::undefined_read("children"));
        };
        args.keyword.push(Pair {
            span,
            key: Expr::Symbol {
                span,
                name: "caller".into(),
            },
            value: caller,
        });
        Ok(Node::Output(Output {
            span,
            expr: Expr::Call {
                span: call_span,
                callee,
                args,
            },
        }))
    }

    fn parse_with_context(&mut self) -> Res<Option<bool>> {
        let tok = self.peek_cloned()?;
        let with_context = if self.skip_symbol("with")? {
            Some(true)
        } else if self.skip_symbol("without")? {
            Some(false)
        } else {
            None
        };
        if with_context.is_some() && !self.skip_symbol("context")? {
            let at = tok.map(|t| t.at());
            return self.fail("parseFrom: expected context after with/without", at);
        }
        Ok(with_context)
    }

    fn parse_import(&mut self) -> Res<Node> {
        let import_tok = self.peek_known()?;
        if !self.skip_symbol("import")? {
            return self.fail("parseImport: expected import", Some(import_tok.at()));
        }
        let template = self.parse_expression()?.expr;
        if !self.skip_symbol("as")? {
            return self.fail(
                "parseImport: expected \"as\" keyword",
                Some(import_tok.at()),
            );
        }
        let target = self.parse_expression()?.expr;
        let with_context = self.parse_with_context()?;
        self.advance_after_block_end(Some(&import_tok.value))?;
        Ok(Node::Import(Import {
            span: import_tok.span,
            template,
            target,
            with_context,
        }))
    }

    fn parse_from(&mut self) -> Res<Node> {
        let from_tok = self.peek_known()?;
        if !self.skip_symbol("from")? {
            return self.fail("parseFrom: expected from", None);
        }
        let template = self.parse_expression()?.expr;
        if !self.skip_symbol("import")? {
            return self.fail("parseFrom: expected import", Some(from_tok.at()));
        }
        let mut names = Vec::new();
        let mut with_context = None;
        loop {
            let Some(next) = self.peek_cloned()? else {
                return Err(Fail::null_read("type"));
            };
            if next.kind == Kind::BlockEnd {
                if names.is_empty() {
                    return self.fail(
                        "parseFrom: Expected at least one import name",
                        Some(from_tok.at()),
                    );
                }
                if next.value.starts_with('-') {
                    self.drop_leading_whitespace = true;
                }
                self.next_token()?;
                break;
            }
            if !names.is_empty() && !self.skip(Kind::Comma)? {
                return self.fail("parseFrom: expected comma", Some(from_tok.at()));
            }
            let name = self.parse_primary(false)?.expr;
            // `name.value.charAt(0) === '_'`.
            let first = match &name {
                Expr::Symbol { name, .. }
                | Expr::Literal {
                    value: Literal::String(name),
                    ..
                } => name.chars().next(),
                Expr::Literal {
                    value: Literal::Null,
                    ..
                } => return Err(Fail::null_read("charAt")),
                Expr::Literal { .. } => {
                    return Err(Fail::js("TypeError", "name.value.charAt is not a function"));
                }
                _ => return Err(Fail::undefined_read("charAt")),
            };
            let span = name.span();
            if first == Some('_') {
                return self.fail(
                    "parseFrom: names starting with an underscore cannot be imported",
                    Some((span.line, span.column)),
                );
            }
            let alias = if self.skip_symbol("as")? {
                Some(self.parse_primary(false)?.expr)
            } else {
                None
            };
            names.push(ImportName { span, name, alias });
            with_context = self.parse_with_context()?;
        }
        Ok(Node::FromImport(FromImport {
            span: from_tok.span,
            template,
            names,
            with_context,
        }))
    }

    fn parse_block(&mut self) -> Res<Node> {
        let tag = self.peek_known()?;
        if !self.skip_symbol("block")? {
            return self.fail("parseBlock: expected block", Some(tag.at()));
        }
        let name = self.parse_primary(false)?.expr;
        let Expr::Symbol {
            span: name_span,
            name,
        } = name
        else {
            return self.fail("parseBlock: variable name expected", Some(tag.at()));
        };
        self.advance_after_block_end(Some(&tag.value))?;
        let body = self.parse_until_blocks(&["endblock"], tag.span)?;
        self.skip_symbol("endblock")?;
        self.skip_symbol(&name)?;
        let Some(tok) = self.peek_cloned()? else {
            return self.fail("parseBlock: expected endblock, got end of file", None);
        };
        self.advance_after_block_end(Some(tok.shown()))?;
        Ok(Node::Block(Block {
            span: tag.span,
            name,
            name_span,
            body,
        }))
    }

    fn parse_extends(&mut self) -> Res<Node> {
        let tag = self.peek_known()?;
        if !self.skip_symbol("extends")? {
            return self.fail("parseTemplateRef: expected extends", None);
        }
        let template = self.parse_expression()?.expr;
        self.advance_after_block_end(Some(&tag.value))?;
        Ok(Node::Extends(Extends {
            span: tag.span,
            template,
        }))
    }

    fn parse_include(&mut self) -> Res<Node> {
        let tag = self.peek_known()?;
        if !self.skip_symbol("include")? {
            return self.fail("parseInclude: expected include", None);
        }
        let template = self.parse_expression()?.expr;
        let ignore_missing = self.skip_symbol("ignore")? && self.skip_symbol("missing")?;
        self.advance_after_block_end(Some(&tag.value))?;
        Ok(Node::Include(Include {
            span: tag.span,
            template,
            ignore_missing,
        }))
    }

    /// `parseIf`. Nunjucks parses each `elif` by calling itself; this
    /// loops instead, with the same steps, and keeps the branches flat.
    fn parse_if(&mut self) -> Res<Node> {
        let first = self.peek_known()?;
        let is_async = if self.skip_symbol("if")?
            || self.skip_symbol("elif")?
            || self.skip_symbol("elseif")?
        {
            false
        } else if self.skip_symbol("ifAsync")? {
            true
        } else {
            return self.fail("parseIf: expected if, elif, or elseif", Some(first.at()));
        };
        let mut tag = first.clone();
        let mut branches = Vec::new();
        let else_body = loop {
            let cond = self.parse_expression()?.expr;
            self.advance_after_block_end(Some(&tag.value))?;
            let body = self.parse_until_blocks(&["elif", "elseif", "else", "endif"], first.span)?;
            branches.push(Branch {
                span: tag.span,
                cond,
                body,
            });
            let tok = self.peek_cloned()?;
            match tok.as_ref().map(|t| t.shown()) {
                Some("elseif" | "elif") => {
                    tag = tok.expect("peeked");
                    // The nested `parseIf` skips the tag name, which is
                    // `elif` or `elseif` here.
                    self.take()?;
                }
                Some("else") => {
                    self.advance_after_block_end(None)?;
                    let body = self.parse_until_blocks(&["endif"], first.span)?;
                    self.advance_after_block_end(None)?;
                    break Some(body);
                }
                Some("endif") => {
                    self.advance_after_block_end(None)?;
                    break None;
                }
                _ => {
                    return self.fail(
                        "parseIf: expected elif, else, or endif, got end of file",
                        None,
                    );
                }
            }
        };
        Ok(Node::If(If {
            span: first.span,
            is_async,
            branches,
            else_body,
        }))
    }

    fn parse_set(&mut self) -> Res<Node> {
        let tag = self.peek_known()?;
        if !self.skip_symbol("set")? {
            return self.fail("parseSet: expected set", Some(tag.at()));
        }
        let mut targets = Vec::new();
        loop {
            targets.push(self.parse_primary(false)?.expr);
            if !self.skip(Kind::Comma)? {
                break;
            }
        }
        let value = if !self.skip_value(Kind::Operator, "=")? {
            if !self.skip(Kind::BlockEnd)? {
                return self.fail(
                    "parseSet: expected = or block end in set tag",
                    Some(tag.at()),
                );
            }
            let body = self.parse_until_blocks(&["endset"], tag.span)?;
            self.advance_after_block_end(None)?;
            SetValue::Capture(Capture {
                span: tag.span,
                body,
            })
        } else {
            let value = self.parse_expression()?.expr;
            self.advance_after_block_end(Some(&tag.value))?;
            SetValue::Expr(value)
        };
        Ok(Node::Set(Set {
            span: tag.span,
            targets,
            value,
        }))
    }

    fn parse_switch(&mut self) -> Res<Node> {
        let tag = self.peek_known()?;
        if !self.skip_symbol("switch")?
            && !self.skip_symbol("case")?
            && !self.skip_symbol("default")?
        {
            return self.fail(
                "parseSwitch: expected \"switch,\" \"case\" or \"default\"",
                Some(tag.at()),
            );
        }
        let expr = self.parse_expression()?.expr;
        self.advance_after_block_end(Some("switch"))?;
        // Text before the first case is parsed and dropped.
        self.parse_until_blocks(&["case", "default", "endswitch"], tag.span)?;
        let mut cases = Vec::new();
        // A do-while: the first case is parsed even when the next tag is
        // `default` or `endswitch`, which then reads as an expression.
        let tok = loop {
            self.skip_symbol("case")?;
            let cond = self.parse_expression()?.expr;
            self.advance_after_block_end(Some("switch"))?;
            let body = self.parse_until_blocks(&["case", "default", "endswitch"], tag.span)?;
            cases.push(Case { cond, body });
            let tok = self.peek_cloned()?;
            if !tok.as_ref().is_some_and(|t| t.shown() == "case") {
                break tok;
            }
        };
        let Some(tok) = tok else {
            return Err(Fail::null_read("value"));
        };
        let default = match tok.shown() {
            "default" => {
                self.advance_after_block_end(None)?;
                let body = self.parse_until_blocks(&["endswitch"], tag.span)?;
                self.advance_after_block_end(None)?;
                Some(body)
            }
            "endswitch" => {
                self.advance_after_block_end(None)?;
                None
            }
            _ => {
                return self.fail(
                    "parseSwitch: expected \"case,\" \"default\" or \"endswitch,\" got EOF.",
                    None,
                );
            }
        };
        Ok(Node::Switch(Switch {
            span: tag.span,
            expr,
            cases,
            default,
        }))
    }

    /// `parseStatement`. `None` when the tag ends the body being parsed.
    fn parse_statement(&mut self) -> Res<Option<Node>> {
        let tok = match self.peek_cloned()? {
            Some(t) => t,
            None => return Err(Fail::null_read("type")),
        };
        if tok.kind != Kind::Symbol {
            return self.fail("tag name expected", Some(tok.at()));
        }
        if self
            .break_on_blocks
            .is_some_and(|b| b.contains(&tok.value.as_str()))
        {
            return Ok(None);
        }
        let node = match tok.value.as_str() {
            "raw" => self.parse_raw("raw")?,
            "verbatim" => self.parse_raw("verbatim")?,
            "if" | "ifAsync" => self.parse_if()?,
            "for" | "asyncEach" | "asyncAll" => self.parse_for()?,
            "block" => self.parse_block()?,
            "extends" => self.parse_extends()?,
            "include" => self.parse_include()?,
            "set" => self.parse_set()?,
            "macro" => self.parse_macro()?,
            "call" => self.parse_call()?,
            "import" => self.parse_import()?,
            "from" => self.parse_from()?,
            "filter" => self.parse_filter_statement()?,
            "switch" => self.parse_switch()?,
            name if ELEVENTY_TAGS.contains(&name) && !self.shortcodes.contains_key(name) => {
                let mut e = Fail::template(
                    format!("Brrrty does not support the {name} tag"),
                    Some(tok.at()),
                );
                e.kind = ErrorKind::Unsupported;
                return Err(e.with_help(format!(
                    "`{name}` is a tag 11ty adds to Nunjucks; Brrrty does not run it"
                )));
            }
            name if self.shortcodes.contains_key(name) => self.parse_shortcode(name)?,
            name => {
                let msg = format!("unknown block tag: {name}");
                return self.fail(msg, Some(tok.at()));
            }
        };
        Ok(Some(node))
    }

    /// 11ty's `addShortcode` and `addPairedShortcode` tags (`src/Engines/Nunjucks.js`,
    /// `_getShortcodeFn` and `_getPairedShortcodeFn`): `parseSignature(true, true)`, then for a
    /// plain one an empty string argument when there are none, for a paired one the body up
    /// to `end<name>` as the first argument.
    fn parse_shortcode(&mut self, name: &str) -> Res<Node> {
        let paired = self.shortcodes[name];
        let tok = self.peek_known()?;
        self.take()?;
        let mut args = self.parse_signature_no_parens()?;
        let span = tok.span;
        if paired.is_none() && args.positional.is_empty() && args.keyword.is_empty() {
            args.positional.push(Expr::Literal {
                span,
                value: Literal::String(String::new()),
            });
        }
        self.advance_after_block_end(Some(&tok.value))?;
        if let Some(end) = paired {
            let body = self.parse_until_blocks(end, span)?;
            self.advance_after_block_end(None)?;
            args.positional
                .insert(0, Expr::Capture(Capture { span, body }));
        }
        let expr = Expr::Filter {
            span,
            name: format!("{SHORTCODE_PREFIX}{name}"),
            args,
        };
        Ok(Node::Output(Output { span, expr }))
    }

    /// `parseSignature(true, true)`: arguments up to the block end, an opening `(` skipped.
    fn parse_signature_no_parens(&mut self) -> Res<Args> {
        let Some(tok) = self.peek_cloned()? else {
            return Err(Fail::null_read("type"));
        };
        if tok.kind == Kind::LeftParen {
            self.take()?;
        }
        let mut args = Args::default();
        let mut check_comma = false;
        loop {
            let Some(tok) = self.peek_cloned()? else {
                return Err(Fail::null_read("type"));
            };
            if tok.kind == Kind::BlockEnd {
                break;
            }
            if check_comma && !self.skip(Kind::Comma)? {
                return self.fail(
                    "parseSignature: expected comma after expression",
                    Some(tok.at()),
                );
            }
            let arg = self.parse_expression()?;
            if self.skip_value(Kind::Operator, "=")? {
                let value = self.parse_expression()?;
                args.keyword.push(Pair {
                    span: arg.expr.span(),
                    key: arg.expr,
                    value: value.expr,
                });
            } else {
                args.positional.push(arg.expr);
            }
            check_comma = true;
        }
        Ok(args)
    }

    fn parse_raw(&mut self, tag_name: &str) -> Res<Node> {
        let begun = self.advance_after_block_end(None)?;
        let mut level = 1;
        let mut text = String::new();
        while let Some((pre, tag, opens)) = self.tokens.extract_raw_tag(tag_name) {
            if level == 0 {
                break;
            }
            if opens {
                level += 1;
            } else {
                level -= 1;
            }
            text.push_str(pre);
            if level == 0 {
                // Nunjucks steps back to the start of the end tag, then
                // its loop reads that tag again and stops.
                self.tokens.back_over(tag)?;
            } else {
                text.push_str(tag);
            }
        }
        Ok(Node::Text(Text {
            span: begun.span,
            text,
        }))
    }

    fn parse_filter_statement(&mut self) -> Res<Node> {
        let filter_tok = self.peek_known()?;
        if !self.skip_symbol("filter")? {
            return self.fail("parseFilterStatement: expected filter", None);
        }
        let (name, span) = self.parse_filter_name()?;
        let (args, _) = self.parse_filter_args()?;
        self.advance_after_block_end(Some(&filter_tok.value))?;
        let body = self.parse_until_blocks(&["endfilter"], filter_tok.span)?;
        self.advance_after_block_end(None)?;
        let mut positional = vec![Expr::Capture(Capture { span, body })];
        positional.extend(args.positional);
        let filter = Expr::Filter {
            span,
            name,
            args: Args {
                positional,
                keyword: args.keyword,
            },
        };
        Ok(Node::Output(Output { span, expr: filter }))
    }

    fn parse_until_blocks(&mut self, names: &'static [&'static str], at: Span) -> Res<Vec<Node>> {
        self.tag_depth += 1;
        if self.tag_depth > MAX_DEPTH {
            return Err(too_deep(at, "tags"));
        }
        let prev = self.break_on_blocks.replace(names);
        let nodes = grow(|| self.parse_nodes())?;
        self.break_on_blocks = prev;
        self.tag_depth -= 1;
        Ok(nodes)
    }

    fn parse_nodes(&mut self) -> Res<Vec<Node>> {
        let mut buf = Vec::new();
        while let Some(tok) = self.next_token()? {
            match tok.kind {
                Kind::Data => {
                    let next = self.peek_cloned()?;
                    let mut data = tok.value.as_str();
                    if self.drop_leading_whitespace {
                        data = data.trim_start_matches(js_space);
                        self.drop_leading_whitespace = false;
                    }
                    let trims = next.is_some_and(|n| match n.kind {
                        Kind::BlockStart => n.value.ends_with('-'),
                        Kind::VariableStart | Kind::Comment => {
                            n.value.as_bytes().get(2) == Some(&b'-')
                        }
                        _ => false,
                    });
                    if trims {
                        data = data.trim_end_matches(js_space);
                    }
                    buf.push(Node::Text(Text {
                        span: tok.span,
                        text: data.to_string(),
                    }));
                }
                Kind::BlockStart => {
                    self.drop_leading_whitespace = false;
                    match self.parse_statement()? {
                        Some(n) => buf.push(n),
                        None => break,
                    }
                }
                Kind::VariableStart => {
                    let e = self.parse_expression()?;
                    self.drop_leading_whitespace = false;
                    self.advance_after_variable_end()?;
                    buf.push(Node::Output(Output {
                        span: tok.span,
                        expr: e.expr,
                    }));
                }
                Kind::Comment => {
                    let body = &tok.value[..tok.value.len() - 2];
                    self.drop_leading_whitespace = body.ends_with('-');
                }
                kind => {
                    let msg = format!("Unexpected token at top-level: {}", kind.name());
                    return self.fail(msg, Some(tok.at()));
                }
            }
        }
        Ok(buf)
    }

    // Expressions.

    /// Counts one level of recursion, refusing too many.
    fn enter(&mut self) -> Res<()> {
        self.expr_depth += 1;
        if self.expr_depth > MAX_DEPTH {
            return Err(too_deep(self.last, "expressions"));
        }
        Ok(())
    }

    fn parse_expression(&mut self) -> Res<P> {
        self.enter()?;
        let p = grow(|| self.parse_inline_if())?;
        self.expr_depth -= 1;
        Ok(p)
    }

    fn parse_postfix(&mut self, mut node: Option<P>, missing: Span) -> Res<Option<P>> {
        loop {
            let Some(tok) = self.peek_cloned()? else {
                return Ok(node);
            };
            let target =
                |node: Option<P>| node.unwrap_or_else(|| leaf(Expr::Missing { span: missing }));
            node = Some(match tok.kind {
                Kind::LeftParen => {
                    let (args, args_height) = self.parse_signature(false)?.expect("a paren");
                    let callee = target(node);
                    let call = Expr::Call {
                        span: tok.span,
                        callee: Box::new(callee.expr),
                        args,
                    };
                    mk(call, &[callee.height, args_height])?
                }
                Kind::LeftBracket => {
                    let lookup = self.parse_aggregate()?.expect("a bracket");
                    let Expr::Array { items, .. } = lookup.expr else {
                        unreachable!("`[` starts an array")
                    };
                    if items.len() > 1 {
                        return self.fail("invalid index", None);
                    }
                    let key = items
                        .into_iter()
                        .next()
                        .unwrap_or(Expr::Missing { span: tok.span });
                    let target = target(node);
                    let lookup_val = Expr::Lookup {
                        span: tok.span,
                        target: Box::new(target.expr),
                        key: Box::new(key),
                    };
                    mk(lookup_val, &[target.height, lookup.height])?
                }
                Kind::Operator if tok.value == "." => {
                    self.take()?;
                    let Some(val) = self.next_token()? else {
                        return Err(Fail::null_read("type"));
                    };
                    if val.kind != Kind::Symbol {
                        let msg = format!("expected name as lookup value, got {}", val.shown());
                        return self.fail(msg, Some(val.at()));
                    }
                    let key = Expr::Literal {
                        span: val.span,
                        value: Literal::String(val.value),
                    };
                    let target = target(node);
                    let lookup_val = Expr::Lookup {
                        span: tok.span,
                        target: Box::new(target.expr),
                        key: Box::new(key),
                    };
                    mk(lookup_val, &[target.height, 1])?
                }
                _ => return Ok(node),
            });
        }
    }

    fn parse_inline_if(&mut self) -> Res<P> {
        let body = self.parse_or()?;
        if !self.skip_symbol("if")? {
            return Ok(body);
        }
        let cond = self.parse_or()?;
        let else_ = if self.skip_symbol("else")? {
            Some(self.parse_or()?)
        } else {
            None
        };
        let heights = [
            body.height,
            cond.height,
            else_.as_ref().map_or(0, |e| e.height),
        ];
        let span = body.expr.span();
        mk(
            Expr::InlineIf {
                span,
                cond: Box::new(cond.expr),
                body: Box::new(body.expr),
                else_: else_.map(|e| Box::new(e.expr)),
            },
            &heights,
        )
    }

    /// A left-associative chain of one binary operator, such as
    /// `parseOr` or `parseAdd`.
    fn chain(
        &mut self,
        op: BinaryOp,
        kind: Kind,
        value: &str,
        next: fn(&mut Self) -> Res<P>,
    ) -> Res<P> {
        let mut node = next(self)?;
        while self.skip_value(kind, value)? {
            let rhs = next(self)?;
            let span = node.expr.span();
            node = mk(
                Expr::Binary {
                    span,
                    op,
                    lhs: Box::new(node.expr),
                    rhs: Box::new(rhs.expr),
                },
                &[node.height, rhs.height],
            )?;
        }
        Ok(node)
    }

    fn parse_or(&mut self) -> Res<P> {
        self.chain(BinaryOp::Or, Kind::Symbol, "or", Self::parse_and)
    }

    fn parse_and(&mut self) -> Res<P> {
        self.chain(BinaryOp::And, Kind::Symbol, "and", Self::parse_not)
    }

    fn parse_not(&mut self) -> Res<P> {
        let tok = self.peek_cloned()?;
        if !self.skip_symbol("not")? {
            return self.parse_in();
        }
        self.enter()?;
        let operand = grow(|| self.parse_not())?;
        self.expr_depth -= 1;
        let span = tok.expect("peeked").span;
        unary(UnaryOp::Not, span, operand)
    }

    fn parse_in(&mut self) -> Res<P> {
        let mut node = self.parse_is()?;
        while let Some(tok) = self.next_token()? {
            let invert = tok.is_symbol("not");
            if !invert {
                self.push_token(Some(tok.clone()))?;
            }
            if self.skip_symbol("in")? {
                let rhs = self.parse_is()?;
                let span = node.expr.span();
                node = mk(
                    Expr::Binary {
                        span,
                        op: BinaryOp::In,
                        lhs: Box::new(node.expr),
                        rhs: Box::new(rhs.expr),
                    },
                    &[node.height, rhs.height],
                )?;
                if invert {
                    node = unary(UnaryOp::Not, span, node)?;
                }
            } else {
                if invert {
                    self.push_token(Some(tok))?;
                }
                break;
            }
        }
        Ok(node)
    }

    fn parse_is(&mut self) -> Res<P> {
        let node = self.parse_compare()?;
        if !self.skip_symbol("is")? {
            return Ok(node);
        }
        let not = self.skip_symbol("not")?;
        let rhs = self.parse_compare()?;
        let span = node.expr.span();
        let is = mk(
            Expr::Binary {
                span,
                op: BinaryOp::Is,
                lhs: Box::new(node.expr),
                rhs: Box::new(rhs.expr),
            },
            &[node.height, rhs.height],
        )?;
        if not {
            unary(UnaryOp::Not, span, is)
        } else {
            Ok(is)
        }
    }

    fn parse_compare(&mut self) -> Res<P> {
        let expr = self.parse_concat()?;
        let mut ops = Vec::new();
        let mut height = expr.height;
        while let Some(tok) = self.next_token()? {
            // Nunjucks compares the token's value, whatever its type, so
            // a string "==" works as an operator too.
            let op = match tok.shown() {
                "==" => CompareOp::Eq,
                "===" => CompareOp::StrictEq,
                "!=" => CompareOp::Ne,
                "!==" => CompareOp::StrictNe,
                "<" => CompareOp::Lt,
                ">" => CompareOp::Gt,
                "<=" => CompareOp::Le,
                ">=" => CompareOp::Ge,
                _ => {
                    self.push_token(Some(tok))?;
                    break;
                }
            };
            let rhs = self.parse_concat()?;
            height = height.max(rhs.height + 1);
            ops.push(CompareOperand {
                span: tok.span,
                op,
                expr: rhs.expr,
            });
        }
        if ops.is_empty() {
            return Ok(expr);
        }
        let span = ops[0].span;
        mk(
            Expr::Compare {
                span,
                expr: Box::new(expr.expr),
                ops,
            },
            &[height],
        )
    }

    fn parse_concat(&mut self) -> Res<P> {
        self.chain(BinaryOp::Concat, Kind::Tilde, "~", Self::parse_add)
    }

    fn parse_add(&mut self) -> Res<P> {
        self.chain(BinaryOp::Add, Kind::Operator, "+", Self::parse_sub)
    }

    fn parse_sub(&mut self) -> Res<P> {
        self.chain(BinaryOp::Sub, Kind::Operator, "-", Self::parse_mul)
    }

    fn parse_mul(&mut self) -> Res<P> {
        self.chain(BinaryOp::Mul, Kind::Operator, "*", Self::parse_div)
    }

    fn parse_div(&mut self) -> Res<P> {
        self.chain(BinaryOp::Div, Kind::Operator, "/", Self::parse_floor_div)
    }

    fn parse_floor_div(&mut self) -> Res<P> {
        self.chain(BinaryOp::FloorDiv, Kind::Operator, "//", Self::parse_mod)
    }

    fn parse_mod(&mut self) -> Res<P> {
        self.chain(BinaryOp::Mod, Kind::Operator, "%", Self::parse_pow)
    }

    fn parse_pow(&mut self) -> Res<P> {
        self.chain(BinaryOp::Pow, Kind::Operator, "**", |p| {
            p.parse_unary(false)
        })
    }

    fn parse_unary(&mut self, no_filters: bool) -> Res<P> {
        let tok = self.peek_cloned()?;
        let op = if self.skip_value(Kind::Operator, "-")? {
            Some(UnaryOp::Neg)
        } else if self.skip_value(Kind::Operator, "+")? {
            Some(UnaryOp::Pos)
        } else {
            None
        };
        let node = match op {
            Some(op) => {
                self.enter()?;
                let operand = grow(|| self.parse_unary(true))?;
                self.expr_depth -= 1;
                unary(op, tok.expect("peeked").span, operand)?
            }
            None => self.parse_primary(false)?,
        };
        if no_filters {
            Ok(node)
        } else {
            self.parse_filter(node)
        }
    }

    fn parse_primary(&mut self, no_postfix: bool) -> Res<P> {
        let Some(tok) = self.next_token()? else {
            return self.fail("expected expression, got end of file", None);
        };
        let value = match tok.kind {
            Kind::String => Some(Literal::String(tok.value.clone())),
            Kind::Int => Some(Literal::Number(tok.value.parse().unwrap_or(f64::NAN))),
            Kind::Float => Some(Literal::Number(parse_float(&tok.value))),
            Kind::Boolean => Some(Literal::Bool(tok.value == "true")),
            Kind::None => Some(Literal::Null),
            Kind::Regex => Some(regex(&tok)?),
            _ => None,
        };
        let mut node = if let Some(value) = value {
            Some(leaf(Expr::Literal {
                span: tok.span,
                value,
            }))
        } else if tok.kind == Kind::Symbol {
            Some(leaf(Expr::Symbol {
                span: tok.span,
                name: tok.value.clone(),
            }))
        } else {
            self.push_token(Some(tok.clone()))?;
            self.parse_aggregate()?
        };
        if !no_postfix {
            node = self.parse_postfix(node, tok.span)?;
        }
        match node {
            Some(p) => Ok(p),
            None => {
                let msg = format!("unexpected token: {}", tok.shown());
                self.fail(msg, Some(tok.at()))
            }
        }
    }

    fn parse_filter_name(&mut self) -> Res<(String, Span)> {
        let tok = self.expect(Kind::Symbol)?;
        let mut name = tok.value;
        while self.skip_value(Kind::Operator, ".")? {
            name.push('.');
            name.push_str(&self.expect(Kind::Symbol)?.value);
        }
        Ok((name, tok.span))
    }

    /// `parseFilterArgs`: the arguments of the call the postfix parse
    /// ends with. Anything else after `(...)`, such as `.x`, ends with a
    /// lookup, which has no arguments for Nunjucks to read.
    fn parse_filter_args(&mut self) -> Res<(Args, usize)> {
        if self.peek_kind()? != Kind::LeftParen {
            return Ok((Args::default(), 0));
        }
        let placeholder = leaf(Expr::Missing {
            span: Span::default(),
        });
        match self.parse_postfix(Some(placeholder), Span::default())? {
            Some(P {
                expr: Expr::Call { args, .. },
                height,
            }) => Ok((args, height)),
            _ => Err(Fail::undefined_read("children")),
        }
    }

    fn parse_filter(&mut self, mut node: P) -> Res<P> {
        while self.skip(Kind::Pipe)? {
            let (name, span) = self.parse_filter_name()?;
            let (args, args_height) = self.parse_filter_args()?;
            let mut positional = vec![node.expr];
            positional.extend(args.positional);
            node = mk(
                Expr::Filter {
                    span,
                    name,
                    args: Args {
                        positional,
                        keyword: args.keyword,
                    },
                },
                &[node.height, args_height],
            )?;
        }
        Ok(node)
    }

    /// `parseAggregate`: a group, array or dict, or `None` when the next
    /// token starts none of them (it is consumed all the same).
    fn parse_aggregate(&mut self) -> Res<Option<P>> {
        let Some(tok) = self.next_token()? else {
            return Err(Fail::null_read("type"));
        };
        let kind = tok.kind;
        if !matches!(kind, Kind::LeftParen | Kind::LeftBracket | Kind::LeftCurly) {
            return Ok(None);
        }
        let mut items = Vec::new();
        let mut pairs = Vec::new();
        let mut height = 0;
        loop {
            let next = self.peek_kind()?;
            if matches!(
                next,
                Kind::RightParen | Kind::RightBracket | Kind::RightCurly
            ) {
                self.take()?;
                break;
            }
            if (!items.is_empty() || !pairs.is_empty()) && !self.skip(Kind::Comma)? {
                return self.fail(
                    "parseAggregate: expected comma after expression",
                    Some(tok.at()),
                );
            }
            if kind == Kind::LeftCurly {
                // A key is a primary, not an expression, so this is the
                // one recursion `parse_expression` does not count.
                self.enter()?;
                let key = grow(|| self.parse_primary(false))?;
                self.expr_depth -= 1;
                if !self.skip(Kind::Colon)? {
                    return self.fail(
                        "parseAggregate: expected colon after dict key",
                        Some(tok.at()),
                    );
                }
                let value = self.parse_expression()?;
                height = height.max(1 + key.height.max(value.height));
                pairs.push(Pair {
                    span: key.expr.span(),
                    key: key.expr,
                    value: value.expr,
                });
            } else {
                let item = self.parse_expression()?;
                height = height.max(item.height);
                items.push(item.expr);
            }
        }
        let span = tok.span;
        let expr = match kind {
            Kind::LeftParen => Expr::Group { span, items },
            Kind::LeftBracket => Expr::Array { span, items },
            _ => Expr::Dict { span, pairs },
        };
        mk(expr, &[height]).map(Some)
    }

    /// `parseSignature`: `(args)`. `None` when `tolerant` and there is no
    /// `(`. Keyword arguments are gathered after the positional ones.
    fn parse_signature(&mut self, tolerant: bool) -> Res<Option<(Args, usize)>> {
        let Some(tok) = self.peek_cloned()? else {
            return Err(Fail::null_read("type"));
        };
        if tok.kind != Kind::LeftParen {
            if tolerant {
                return Ok(None);
            }
            return self.fail("expected arguments", Some(tok.at()));
        }
        self.take()?;
        let mut args = Args::default();
        let mut height = 0;
        let mut check_comma = false;
        loop {
            let Some(tok) = self.peek_cloned()? else {
                return Err(Fail::null_read("type"));
            };
            if tok.kind == Kind::RightParen {
                self.take()?;
                break;
            }
            if check_comma && !self.skip(Kind::Comma)? {
                return self.fail(
                    "parseSignature: expected comma after expression",
                    Some(tok.at()),
                );
            }
            let arg = self.parse_expression()?;
            if self.skip_value(Kind::Operator, "=")? {
                let value = self.parse_expression()?;
                height = height.max(1 + arg.height.max(value.height));
                args.keyword.push(Pair {
                    span: arg.expr.span(),
                    key: arg.expr,
                    value: value.expr,
                });
            } else {
                height = height.max(arg.height);
                args.positional.push(arg.expr);
            }
            check_comma = true;
        }
        Ok(Some((args, height + 1)))
    }
}

fn unary(op: UnaryOp, span: Span, operand: P) -> Res<P> {
    let height = operand.height;
    mk(
        Expr::Unary {
            span,
            op,
            operand: Box::new(operand.expr),
        },
        &[height],
    )
}

/// JavaScript's `parseFloat` of a float token: `D.D`, `D.` or `D.null`.
fn parse_float(text: &str) -> f64 {
    let (int, dec) = text.split_once('.').unwrap_or((text, ""));
    let number = if !dec.is_empty() && dec.bytes().all(|b| b.is_ascii_digit()) {
        format!("{int}.{dec}")
    } else {
        int.to_string()
    };
    number.parse().unwrap_or(f64::NAN)
}

/// `new RegExp(body, flags)`. The lexer only takes `g`, `i`, `m` and `y`
/// as flags, so the only flags JavaScript refuses here are repeated ones.
/// The pattern is checked where it is compiled.
fn regex(tok: &Token) -> Res<Literal> {
    let flags = &tok.flags;
    if flags
        .char_indices()
        .any(|(i, c)| flags[i + c.len_utf8()..].contains(c))
    {
        return Err(Fail::js(
            "SyntaxError",
            format!("Invalid flags supplied to RegExp constructor '{flags}'"),
        ));
    }
    Ok(Literal::Regex {
        pattern: tok.value.clone(),
        flags: flags.clone(),
    })
}
