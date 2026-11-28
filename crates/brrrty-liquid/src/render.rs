//! Renders a parsed template, as LiquidJS's `Render.renderTemplates` and
//! the tag `render` methods do.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use brrrty_js::Value;

use crate::ast::{
    BinaryOp, Expr, FilterArg, FilteredValue, HashArg, Literal, Node, Segment, Span, Template,
};
use crate::context::{Context, Val};
use crate::engine::{Engine, Filter, FilterCtx, Loader, Rendered, Tz};
use crate::error::{ErrorKind, LiquidError};
use crate::lexer::grow;
use crate::ops::{self, Cmp};

/// The most items a range or loop may have.
pub(crate) const MAX_ITEMS: usize = 1_000_000;

/// A render error. `Plain` errors take the position of the innermost node
/// being rendered, as LiquidJS's `RenderError` does.
#[derive(Debug)]
pub(crate) enum RenderErr {
    Plain {
        kind: ErrorKind,
        message: String,
        help: Option<String>,
    },
    Located(Box<LiquidError>),
}

impl RenderErr {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> RenderErr {
        RenderErr::Plain {
            kind,
            message: message.into(),
            help: None,
        }
    }

    pub fn with_help(mut self, text: impl Into<String>) -> RenderErr {
        if let RenderErr::Plain { help, .. } = &mut self {
            *help = Some(text.into());
        }
        self
    }

    pub fn locate(self, span: Span, template: &str) -> RenderErr {
        match self {
            RenderErr::Plain {
                kind,
                message,
                help,
            } => RenderErr::Located(Box::new(LiquidError {
                kind,
                message,
                template: template.to_string(),
                line: span.line,
                column: span.column,
                help,
            })),
            located => located,
        }
    }
}

pub(crate) type R<T> = Result<T, RenderErr>;

/// A limit error with the shared hint.
pub(crate) fn too_many(message: String) -> RenderErr {
    RenderErr::new(ErrorKind::Limit, message).with_help(format!(
        "Brrrty stops loops and ranges at {MAX_ITEMS} items so a typo cannot use up all memory"
    ))
}

pub(crate) struct Renderer<'a> {
    pub engine: &'a Engine,
    /// The template being rendered, for error positions. `include` and
    /// `render` swap it for the partial's name.
    pub template: Rc<str>,
    pub loader: &'a dyn Loader,
    pub ctx: Context<'a>,
    /// Parsed partials by the path the loader resolved, so a partial in a
    /// loop is parsed once.
    pub partials: HashMap<String, Rc<Template>>,
    /// The partials being rendered: `(resolved path, name as written)`.
    pub stack: Vec<(String, String)>,
}

impl<'a> Renderer<'a> {
    pub fn new(
        engine: &'a Engine,
        template: &'a str,
        data: &'a Value,
        loader: &'a dyn Loader,
    ) -> Renderer<'a> {
        Renderer {
            engine,
            template: Rc::from(template),
            loader,
            ctx: Context::new(data),
            partials: HashMap::new(),
            stack: Vec::new(),
        }
    }

    pub fn render(self, nodes: &[Node]) -> Result<String, LiquidError> {
        self.render_tracked(nodes).map(|r| r.output)
    }

    pub fn render_tracked(mut self, nodes: &[Node]) -> Result<Rendered, LiquidError> {
        let mut out = String::new();
        brrrty_js::traps::clear();
        let result = self.render_nodes(nodes, &mut out);
        let traps = brrrty_js::traps::take();
        match result {
            Ok(()) => Ok(Rendered {
                output: out,
                undefined: {
                    let mut all = self.ctx.undefined.into_inner();
                    all.extend(traps);
                    all.into_iter().collect()
                },
            }),
            Err(RenderErr::Located(e)) => Err(*e),
            Err(RenderErr::Plain { .. }) => unreachable!("render_nodes locates every error"),
        }
    }

    pub fn tz(&self) -> Tz {
        self.engine.options.timezone
    }

    /// LiquidJS's `renderTemplates`: stops after any node that calls
    /// `break` or `continue`, leaving the flag for the enclosing loop.
    pub fn render_nodes(&mut self, nodes: &[Node], out: &mut String) -> R<()> {
        for node in nodes {
            grow(|| self.render_node(node, out))
                .map_err(|e| e.locate(node.span(), &self.template))?;
            if self.ctx.interrupted() {
                break;
            }
        }
        Ok(())
    }

    fn render_node(&mut self, node: &Node, out: &mut String) -> R<()> {
        match node {
            Node::Text(t) => self.write(out, &t.text),
            Node::Output(o) => {
                let v = self.eval_filtered(&o.value)?;
                self.write_value(out, &v)
            }
            Node::Tag(tag) => crate::tags::render(self, tag, out),
        }
    }

    /// Appends `s`, keeping the whole render under the output limit.
    pub fn write(&mut self, out: &mut String, s: &str) -> R<()> {
        let limit = self.engine.output_limit;
        if self.ctx.written + s.len() > limit {
            return Err(RenderErr::new(
                ErrorKind::Limit,
                format!("the render wrote more than {} MiB", limit / (1024 * 1024)),
            )
            .with_help("look for a loop or capture that keeps doubling its output"));
        }
        self.ctx.written += s.len();
        out.push_str(s);
        Ok(())
    }

    /// Writes `v` as an output prints it.
    pub fn write_value(&mut self, out: &mut String, v: &Val) -> R<()> {
        let s = ops::stringify(&v.to_value(), self.tz());
        self.write(out, &s)
    }

    /// A value and its filters. Drops are kept, as LiquidJS's
    /// `Value.value` keeps them.
    pub fn eval_filtered(&mut self, fv: &FilteredValue) -> R<Val> {
        if self.engine.options.strict_filters {
            // LiquidJS checks every filter when it builds the `Value`,
            // before evaluating anything.
            for f in &fv.filters {
                if !self.engine.filters.contains_key(&f.name) {
                    return Err(self.unknown_filter(&f.name));
                }
            }
        }
        let engine = self.engine;
        let mut v = self.eval(&fv.expr)?;
        for f in &fv.filters {
            let filter = engine.filters.get(&f.name);
            let out = match filter {
                Some(Filter::Builtin(run)) => {
                    let mut argv = Vec::with_capacity(f.args.len());
                    for arg in &f.args {
                        argv.push(match arg {
                            FilterArg::Positional(e) => self.eval(e)?,
                            FilterArg::Keyword { name, value } => {
                                let value = match value {
                                    Some(e) => match self.eval(e)? {
                                        // A drop is an object in JavaScript, so
                                        // it is truthy inside the pair.
                                        Val::Nil | Val::Empty | Val::Blank | Val::Block => {
                                            Value::Object(Default::default())
                                        }
                                        other => other.to_value(),
                                    },
                                    None => Value::Undefined,
                                };
                                Val::V(Value::Array(Arc::new(vec![
                                    Value::String(name.as_str().into()),
                                    value,
                                ])))
                            }
                        });
                    }
                    brrrty_js::functions::check_filter_args(
                        &f.name,
                        std::iter::once(&v).chain(&argv).filter_map(|x| match x {
                            Val::V(v) => Some(v),
                            _ => None,
                        }),
                    );
                    let out = run(self, &v, &argv)?;
                    self.check_size(&out)?;
                    out
                }
                Some(Filter::User(run)) => {
                    let mut args = Vec::new();
                    let mut kwargs = Vec::new();
                    for arg in &f.args {
                        match arg {
                            FilterArg::Positional(e) => args.push(self.eval(e)?.to_value()),
                            FilterArg::Keyword { name, value } => {
                                let value = match value {
                                    Some(e) => self.eval(e)?.to_value(),
                                    None => Value::Undefined,
                                };
                                kwargs.push((name.clone(), value));
                            }
                        }
                    }
                    let input = v.to_value();
                    brrrty_js::functions::check_filter_args(
                        &f.name,
                        std::iter::once(&input)
                            .chain(&args)
                            .chain(kwargs.iter().map(|(_, v)| v)),
                    );
                    let ctx = self.filter_ctx();
                    let out = run(&input, &args, &kwargs, &ctx)
                        .map_err(|message| RenderErr::new(ErrorKind::Render, message))?;
                    self.check_size(&out)?;
                    out
                }
                Some(Filter::Args(run)) => {
                    let name = &f.name;
                    drop_arg(name, "its input", &v)?;
                    let mut argv = Vec::new();
                    for arg in &f.args {
                        argv.push(match arg {
                            FilterArg::Positional(e) => {
                                let a = self.eval(e)?;
                                drop_arg(name, "an argument", &a)?;
                                a.to_value()
                            }
                            FilterArg::Keyword { name: key, value } => {
                                let value = match value {
                                    Some(e) => {
                                        let a = self.eval(e)?;
                                        drop_arg(name, &format!("its `{key}` argument"), &a)?;
                                        a.to_value()
                                    }
                                    None => Value::Undefined,
                                };
                                Value::Array(Arc::new(vec![
                                    Value::String(key.as_str().into()),
                                    value,
                                ]))
                            }
                        });
                    }
                    let input = v.to_value();
                    brrrty_js::functions::check_filter_args(
                        name,
                        std::iter::once(&input)
                            .chain(&argv)
                            .chain(argv.iter().filter_map(|a| match a {
                                Value::Array(pair) => pair.get(1),
                                _ => None,
                            })),
                    );
                    let ctx = self.filter_ctx();
                    let out = run(&input, &argv, &ctx)
                        .map_err(|message| RenderErr::new(ErrorKind::Render, message))?;
                    self.check_size(&out)?;
                    out
                }
                None if self.engine.options.strict_filters => {
                    return Err(self.unknown_filter(&f.name));
                }
                // LiquidJS's `identify`: the input passes through.
                None => continue,
            };
            v = Val::V(out);
        }
        Ok(v)
    }

    /// What a filter or shortcode sees of the render: the time zone, and the data's `page` and
    /// `eleventy`.
    pub fn filter_ctx(&self) -> FilterCtx {
        let mut ctx = FilterCtx::new(self.tz());
        ctx.page = self.ctx.data_get("page");
        ctx.eleventy = self.ctx.data_get("eleventy");
        ctx
    }

    /// Keeps a filter's result under the output limit, so a loop that
    /// doubles a string through filters stops with an error instead of
    /// using up the machine's memory.
    pub fn check_size(&self, v: &Value) -> R<()> {
        let size = match v {
            Value::String(s) => s.len(),
            Value::Array(a) => a.len().saturating_mul(std::mem::size_of::<Value>()),
            _ => return Ok(()),
        };
        self.check_bytes(size)
    }

    /// Errors when a filter is about to build something of `bytes` bytes
    /// that is over the output limit.
    pub fn check_bytes(&self, bytes: usize) -> R<()> {
        let limit = self.engine.output_limit;
        if bytes > limit {
            return Err(RenderErr::new(
                ErrorKind::Limit,
                format!("a filter produced more than {} MiB", limit / (1024 * 1024)),
            )
            .with_help("look for a loop that keeps doubling a string or an array"));
        }
        Ok(())
    }

    pub fn unknown_filter(&self, name: &str) -> RenderErr {
        let (message, help) = self.engine.unknown_filter(name);
        RenderErr::Plain {
            kind: ErrorKind::UnknownFilter,
            message,
            help,
        }
    }

    /// Evaluates a tag's `name: value` arguments, as LiquidJS's `Hash`
    /// does: a repeated name keeps its first place and its last value, and
    /// a name with no value is `true`.
    pub fn eval_hash(&mut self, args: &[HashArg]) -> R<Vec<(String, Val)>> {
        let mut last: Vec<(&str, Option<&Expr>)> = Vec::new();
        for a in args {
            match last.iter_mut().find(|(n, _)| *n == a.name) {
                Some(slot) => slot.1 = a.value.as_ref(),
                None => last.push((&a.name, a.value.as_ref())),
            }
        }
        last.into_iter()
            .map(|(name, value)| {
                let v = match value {
                    Some(e) => self.eval(e)?,
                    None => Val::bool(true),
                };
                Ok((name.to_string(), v))
            })
            .collect()
    }

    pub fn eval(&mut self, e: &Expr) -> R<Val> {
        grow(|| self.eval_inner(e))
    }

    fn eval_inner(&mut self, e: &Expr) -> R<Val> {
        Ok(match e {
            Expr::Literal { value, .. } => match value {
                Literal::True => Val::bool(true),
                Literal::False => Val::bool(false),
                Literal::Nil => Val::Nil,
                Literal::Empty => Val::Empty,
                Literal::Blank => Val::Blank,
                Literal::Number(n) => Val::number(*n),
                Literal::String(s) => Val::string(s.as_str()),
            },
            Expr::Path { root, segments, .. } => {
                let mut props = Vec::with_capacity(segments.len());
                for s in segments {
                    props.push(match s {
                        Segment::Key { name, .. } => Value::String(name.as_str().into()),
                        Segment::Index(e) => self.eval(e)?.to_value(),
                    });
                }
                let (mut v, rest) = match root {
                    Some(root) => (self.eval(root)?, &props[..]),
                    None => match props.split_first() {
                        Some((first, rest)) => (self.ctx.variable(first), rest),
                        None => (Val::undefined(), &props[..]),
                    },
                };
                for p in rest {
                    v = ops::read_property(&v, p);
                }
                v
            }
            Expr::Range { start, end, .. } => {
                let low = self.eval(start)?.to_value().to_js_number();
                let high = self.eval(end)?.to_value().to_js_number();
                range(low, high)?
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                // LiquidJS evaluates both operands before applying any
                // operator, so `and` and `or` do not short-circuit.
                let l = self.eval(lhs)?;
                let r = self.eval(rhs)?;
                let tz = self.tz();
                Val::bool(match op {
                    BinaryOp::Eq => ops::equals(&l, &r),
                    BinaryOp::Ne => !ops::equals(&l, &r),
                    BinaryOp::Gt => ops::compare(Cmp::Gt, &l, &r, tz),
                    BinaryOp::Lt => ops::compare(Cmp::Lt, &l, &r, tz),
                    BinaryOp::Ge => ops::compare(Cmp::Ge, &l, &r, tz),
                    BinaryOp::Le => ops::compare(Cmp::Le, &l, &r, tz),
                    BinaryOp::Contains => ops::contains(&l, &r, tz),
                    BinaryOp::And => l.truthy() && r.truthy(),
                    BinaryOp::Or => l.truthy() || r.truthy(),
                })
            }
            Expr::Not { operand, .. } => Val::bool(!self.eval(operand)?.truthy()),
            Expr::Missing { .. } => Val::undefined(),
        })
    }
}

/// LiquidJS's `range(+low, +high + 1)`: counts up from `low` in steps of
/// one while below `high + 1`.
fn range(low: f64, high: f64) -> R<Val> {
    let stop = high + 1.0;
    let count = if low < stop { (stop - low).ceil() } else { 0.0 };
    if count > MAX_ITEMS as f64 {
        return Err(too_many(format!(
            "the range ({}..{}) has {} items, more than the limit of {MAX_ITEMS}",
            ops::num(low),
            ops::num(high),
            ops::num(count)
        )));
    }
    let mut items = Vec::with_capacity(count as usize);
    let mut i = low;
    // The count also stops numbers too large to step by one, where
    // LiquidJS would loop forever.
    while i < stop && items.len() < count as usize {
        items.push(Value::Number(i));
        i += 1.0;
    }
    Ok(Val::V(Value::Array(std::sync::Arc::new(items))))
}

/// 11ty hands a JavaScript filter LiquidJS's drop objects as they are: `nil` and `null` are a
/// `NullDrop`, `empty` an `EmptyDrop`, `blank` a `BlankDrop`, and `forloop` and `block` their
/// own drops, also when they were assigned to a variable first. A drop is an object
/// (`JSON.stringify` gives `{}`, `String` gives `[object Object]`, and it is truthy), but `+`
/// gives `"null"` or `""` and a `forloop` stringifies its fields, so no Brrrty value acts the
/// same. A filter written in Rhai (or one of the RSS plugin's) given one refuses the page.
pub(crate) fn drop_arg(filter: &str, what: &str, v: &Val) -> R<()> {
    let drop = match v {
        Val::V(_) => return Ok(()),
        Val::Nil => "`nil` or `null`, LiquidJS's NullDrop",
        Val::Empty => "`empty`, LiquidJS's EmptyDrop",
        Val::Blank => "`blank`, LiquidJS's BlankDrop",
        Val::Loop(_) => "`forloop`, LiquidJS's ForloopDrop",
        Val::Block => "`block`, LiquidJS's BlockDrop",
    };
    Err(RenderErr::new(
        ErrorKind::Render,
        format!(
            "filter `{filter}` would get {drop}, as {what}. 11ty passes a filter that object as it is, and Brrrty can't give a script the same value; pass a string, number or variable that holds one instead"
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineOptions;

    struct NoPartials;

    impl Loader for NoPartials {
        fn load(&self, _: &str) -> Result<(String, String), crate::LoadError> {
            Err(Default::default())
        }
    }

    #[test]
    fn output_over_the_limit_is_an_error() {
        let mut engine = Engine::new(EngineOptions {
            strict_filters: true,
            timezone: Tz::Utc,
        });
        engine.output_limit = 1000;
        let data = Value::Object(Default::default());
        let ok = engine.parse("t", &"x".repeat(1000)).unwrap();
        assert_eq!(engine.render(&ok, &data, &NoPartials).unwrap().len(), 1000);
        let big = engine.parse("t", &"x".repeat(1001)).unwrap();
        let e = engine.render(&big, &data, &NoPartials).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Limit);

        // Captures count too: this doubles a string forever without
        // printing it.
        let src = "{% capture s %}x{% endcapture %}{% for i in (1..64) %}\
                   {% capture s %}{{ s }}{{ s }}{% endcapture %}{% endfor %}";
        let t = engine.parse("t", src).unwrap();
        let e = engine.render(&t, &data, &NoPartials).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Limit);
        // At the node whose write crossed the limit: the second `{{ s }}`.
        let at = src.find("{{ s }}{{ s }}").unwrap() + "{{ s }}".len();
        assert_eq!((e.line, e.column), (1, at + 1));
    }

    #[test]
    fn the_real_limit_is_512_mib() {
        let engine = Engine::new(EngineOptions::default());
        assert_eq!(engine.output_limit, 512 * 1024 * 1024);
    }

    #[test]
    fn ranges_stop_where_steps_of_one_do_not_move() {
        let big = 9_007_199_254_740_990.0; // 2^53 - 2
        let Val::V(Value::Array(a)) = range(big, big + 10.0).unwrap() else {
            panic!()
        };
        assert!(a.len() <= 12);
    }
}
