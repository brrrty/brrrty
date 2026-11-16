//! The variables a template reads, as 11ty 3.1.8's Nunjucks engine finds
//! them for computed data (`Nunjucks.parseForSymbols`).
//!
//! 11ty parses the template, lists every `Symbol` node and every `Value`
//! node (symbols, literals and text) the way Nunjucks's `findAll` walks the
//! tree, and extends each symbol into a dotted name: while the source
//! character just past the name so far is a `.`, the next value in the list
//! is appended. The walk follows each node's fields in Nunjucks's order,
//! and like `findAll` it skips what Nunjucks keeps in plain arrays rather
//! than nodes, or outside its field list: the targets and the block body
//! of `{% set %}`, the operands of a comparison
//! after the first, and the cases of `{% switch %}`.

use crate::ast::{Args, Expr, Literal, Node, Span};
use crate::error::NunjucksError;

/// What a `Value` node's `value.length` is.
#[derive(Clone, Copy, PartialEq)]
enum Length {
    /// A string's, in UTF-16 code units.
    Of(usize),
    /// A number, boolean or regular expression has none: `undefined`,
    /// which makes the index `NaN`.
    Undefined,
    /// `null`: reading it throws.
    Throws,
}

/// A `Value` node: a symbol, or a literal or text.
struct Val {
    symbol: bool,
    /// The value as `Array.prototype.join` writes it.
    text: String,
    length: Length,
    /// 0-based, as Nunjucks counts.
    line: usize,
    column: usize,
}

struct Walk {
    values: Vec<Val>,
}

impl Walk {
    fn push(&mut self, symbol: bool, text: String, span: Span) {
        let length = Length::Of(text.encode_utf16().count());
        self.push_value(symbol, text, length, span);
    }

    fn push_value(&mut self, symbol: bool, text: String, length: Length, span: Span) {
        self.values.push(Val {
            symbol,
            text,
            length,
            line: span.line.saturating_sub(1),
            column: span.column.saturating_sub(1),
        });
    }

    fn nodes(&mut self, nodes: &[Node]) {
        for n in nodes {
            self.node(n);
        }
    }

    fn node(&mut self, node: &Node) {
        match node {
            Node::Text(t) => self.push(false, t.text.clone(), t.span),
            Node::Output(o) => self.expr(&o.expr),
            Node::If(n) => {
                for b in &n.branches {
                    self.expr(&b.cond);
                    self.nodes(&b.body);
                }
                if let Some(e) = &n.else_body {
                    self.nodes(e);
                }
            }
            Node::For(n) => {
                self.expr(&n.iter);
                for t in &n.targets {
                    self.expr(t);
                }
                self.nodes(&n.body);
                if let Some(e) = &n.else_body {
                    self.nodes(e);
                }
            }
            // `body` isn't one of `Set`'s fields, so a block set's body is
            // never walked.
            Node::Set(n) => match &n.value {
                crate::ast::SetValue::Expr(e) => self.expr(e),
                crate::ast::SetValue::Capture(_) => {}
            },
            Node::Block(n) => {
                self.push(true, n.name.clone(), n.name_span);
                self.nodes(&n.body);
            }
            Node::Extends(n) => self.expr(&n.template),
            Node::Include(n) => self.expr(&n.template),
            Node::Macro(n) => {
                self.expr(&n.name);
                self.args(&n.args);
                self.nodes(&n.body);
            }
            Node::Import(n) => {
                self.expr(&n.template);
                self.expr(&n.target);
            }
            Node::FromImport(n) => {
                self.expr(&n.template);
                for name in &n.names {
                    self.expr(&name.name);
                    if let Some(a) = &name.alias {
                        self.expr(a);
                    }
                }
            }
            Node::Switch(n) => {
                self.expr(&n.expr);
                if let Some(d) = &n.default {
                    self.nodes(d);
                }
            }
        }
    }

    fn args(&mut self, args: &Args) {
        for a in &args.positional {
            self.expr(a);
        }
        for p in &args.keyword {
            self.expr(&p.key);
            self.expr(&p.value);
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Literal { span, value } => match value {
                Literal::String(s) => self.push(false, s.clone(), *span),
                Literal::Number(n) => self.push_value(
                    false,
                    brrrty_js::js_number_to_string(*n),
                    Length::Undefined,
                    *span,
                ),
                Literal::Bool(b) => self.push_value(false, b.to_string(), Length::Undefined, *span),
                // `join` writes `null` as nothing.
                Literal::Null => self.push_value(false, String::new(), Length::Throws, *span),
                Literal::Regex { pattern, flags } => self.push_value(
                    false,
                    format!("/{pattern}/{flags}"),
                    Length::Undefined,
                    *span,
                ),
            },
            Expr::Symbol { span, name } => self.push(true, name.clone(), *span),
            Expr::Group { items, .. } | Expr::Array { items, .. } => {
                for i in items {
                    self.expr(i);
                }
            }
            Expr::Dict { pairs, .. } => {
                for p in pairs {
                    self.expr(&p.key);
                    self.expr(&p.value);
                }
            }
            Expr::Lookup { target, key, .. } => {
                self.expr(target);
                self.expr(key);
            }
            Expr::Call { callee, args, .. } => {
                self.expr(callee);
                self.args(args);
            }
            Expr::Filter { span, name, args } => {
                self.push(true, name.clone(), *span);
                self.args(args);
            }
            Expr::InlineIf {
                cond, body, else_, ..
            } => {
                self.expr(cond);
                self.expr(body);
                if let Some(e) = else_ {
                    self.expr(e);
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::Unary { operand, .. } => self.expr(operand),
            // The operands after the first are in a plain array.
            Expr::Compare { expr, .. } => self.expr(expr),
            Expr::Caller { span, args, body } => {
                self.push(true, "caller".to_string(), *span);
                self.args(args);
                self.nodes(body);
            }
            Expr::Capture(c) => self.nodes(&c.body),
            Expr::Missing { .. } => {}
        }
    }
}

/// The dotted names `source` reads, each once, in the order 11ty's
/// `parseForSymbols` gives them. A template Nunjucks can't parse is an
/// error, as it is in 11ty.
pub fn symbols(source: &str) -> Result<Vec<String>, NunjucksError> {
    let t = crate::parse("", source)?;
    let mut walk = Walk { values: Vec::new() };
    walk.nodes(&t.nodes);
    // `str.split("\n")`, as UTF-16 for `charAt`.
    let lines: Vec<Vec<u16>> = source
        .split('\n')
        .map(|l| l.encode_utf16().collect())
        .collect();
    // `charAt(NaN)` is `charAt(0)`.
    let char_at = |line: usize, i: Option<usize>| {
        lines.get(line).and_then(|l| l.get(i.unwrap_or(0))).copied()
    };
    let mut out: Vec<String> = Vec::new();
    for entry in walk.values.iter().filter(|v| v.symbol) {
        let mut name = vec![entry.text.as_str()];
        // The index of the character after the name so far: `None` while
        // not extending a name, `Some(None)` once it is `NaN`.
        let mut nested: Option<Option<usize>> = None;
        for val in &walk.values {
            if let Some(at) = nested {
                if char_at(val.line, at) == Some(u16::from(b'.')) {
                    name.push(&val.text);
                    nested = Some(match (at, val.length) {
                        (Some(at), Length::Of(n)) => Some(at + n + 1),
                        (_, Length::Throws) => {
                            return Err(NunjucksError {
                                kind: crate::ErrorKind::Render,
                                message:
                                    "TypeError: Cannot read properties of null (reading 'length')"
                                        .to_string(),
                                template: String::new(),
                                line: None,
                                column: None,
                                help: None,
                            });
                        }
                        _ => None,
                    });
                } else {
                    nested = None;
                }
            } else if val.line == entry.line
                && val.column == entry.column
                && val.text == entry.text
                && val.length == entry.length
            {
                let Length::Of(n) = entry.length else {
                    unreachable!("a symbol is a string")
                };
                nested = Some(Some(entry.column + n));
            }
        }
        let joined = name.join(".");
        if !out.contains(&joined) {
            out.push(joined);
        }
    }
    Ok(out)
}
