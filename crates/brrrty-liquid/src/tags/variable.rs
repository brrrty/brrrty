//! `assign`, `capture`, `increment`, `decrement` and `echo`.

use brrrty_js::Value;

use crate::ast::{FilteredValue, Node};
use crate::context::Val;
use crate::ops;
use crate::render::{R, Renderer};

/// Sets `name` in the outermost scope, so it outlives loops and blocks.
/// Drops are kept: `{% assign e = empty %}` still compares as `empty`.
pub(super) fn assign(r: &mut Renderer<'_>, name: &str, value: &FilteredValue) -> R<()> {
    let v = r.eval_filtered(value)?;
    r.ctx.scopes[0].insert(name.to_string(), v);
    Ok(())
}

pub(super) fn capture(r: &mut Renderer<'_>, name: &str, body: &[Node]) -> R<()> {
    let mut buf = String::new();
    r.render_nodes(body, &mut buf)?;
    r.ctx.scopes[0].insert(name.to_string(), Val::string(buf));
    Ok(())
}

/// `increment` prints the counter and then adds one; `decrement`
/// subtracts one and then prints. Counters live in the data, as in
/// LiquidJS, so `assign` shadows them and they start from a number already
/// in the data.
pub(super) fn increment(r: &mut Renderer<'_>, name: &str, step: f64, out: &mut String) -> R<()> {
    let found = r.ctx.data_get(name);
    if found.is_none() {
        // The counter starts from a number in the data, so a missing one is
        // a lookup the caller must hear about, as for any other variable.
        r.ctx.undefined.borrow_mut().insert(name.to_string());
    }
    let n = match found {
        Some(Value::Number(n)) => n,
        _ => 0.0,
    };
    let next = n + step;
    r.ctx.env.insert(name.to_string(), Value::Number(next));
    let shown = if step > 0.0 { n } else { next };
    r.write(out, &ops::num(shown))
}

pub(super) fn echo(r: &mut Renderer<'_>, value: Option<&FilteredValue>, out: &mut String) -> R<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let v = r.eval_filtered(value)?;
    r.write_value(out, &v)
}

/// A raw shortcode: the arguments are evaluated, the body goes as written, and what the function
/// returns is written as it is.
pub(super) fn raw_shortcode(
    r: &mut Renderer<'_>,
    name: &str,
    args: &[crate::ast::Expr],
    body: &str,
    out: &mut String,
) -> R<()> {
    let f = r
        .engine
        .raw_shortcodes
        .get(name)
        .expect("a raw shortcode is parsed only when the engine has it")
        .clone();
    let mut argv = Vec::with_capacity(args.len());
    for (i, e) in args.iter().enumerate() {
        let a = r.eval(e)?;
        crate::render::drop_arg(name, &format!("argument {}", i + 1), &a)?;
        argv.push(a.to_value());
    }
    brrrty_js::functions::check_filter_args(name, argv.iter());
    let ctx = r.filter_ctx();
    let text = f(body, &argv, &ctx)
        .map_err(|m| crate::render::RenderErr::new(crate::ErrorKind::Render, m))?;
    r.write(out, &text)
}

/// A shortcode: the arguments are evaluated, a paired one's body is rendered and goes first,
/// and what the function returns is written as it is.
pub(super) fn shortcode(
    r: &mut Renderer<'_>,
    name: &str,
    args: &[crate::ast::Expr],
    body: Option<&[Node]>,
    out: &mut String,
) -> R<()> {
    let f = r
        .engine
        .shortcodes
        .get(name)
        .expect("a shortcode is parsed only when the engine has it")
        .1
        .clone();
    // The arguments are evaluated before the body is rendered.
    let mut argv = Vec::with_capacity(args.len() + 1);
    for (i, e) in args.iter().enumerate() {
        let a = r.eval(e)?;
        crate::render::drop_arg(name, &format!("argument {}", i + 1), &a)?;
        argv.push(a.to_value());
    }
    if let Some(body) = body {
        let mut buf = String::new();
        r.render_nodes(body, &mut buf)?;
        argv.insert(0, Value::String(buf.as_str().into()));
    }
    brrrty_js::functions::check_filter_args(name, argv.iter());
    let ctx = r.filter_ctx();
    let v =
        f(&argv, &ctx).map_err(|m| crate::render::RenderErr::new(crate::ErrorKind::Render, m))?;
    // LiquidJS writes the result only when it is truthy: `html && emitter.write(html)`.
    let falsy = match &v {
        Value::Bool(b) => !*b,
        Value::Number(n) => *n == 0.0 || n.is_nan(),
        Value::String(s) => s.is_empty(),
        Value::Null | Value::Undefined => true,
        _ => false,
    };
    if falsy {
        return Ok(());
    }
    r.write_value(out, &Val::V(v))
}
