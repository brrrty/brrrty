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
