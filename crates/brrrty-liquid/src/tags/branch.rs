//! `if`, `unless` and `case`.

use crate::ast::{Branch, FilteredValue, Node, When};
use crate::context::Val;
use crate::ops;
use crate::render::{R, Renderer};

/// The first branch whose condition is truthy renders, else the `else`.
pub(super) fn if_tag(
    r: &mut Renderer<'_>,
    branches: &[Branch],
    else_body: Option<&[Node]>,
    out: &mut String,
) -> R<()> {
    for b in branches {
        if r.eval_filtered(&b.condition)?.truthy() {
            return r.render_nodes(&b.body, out);
        }
    }
    r.render_nodes(else_body.unwrap_or_default(), out)
}

/// The first branch renders when its condition is falsy; `elsif` branches
/// when theirs is truthy.
pub(super) fn unless(
    r: &mut Renderer<'_>,
    branches: &[Branch],
    else_body: &[Node],
    out: &mut String,
) -> R<()> {
    for (i, b) in branches.iter().enumerate() {
        let truthy = r.eval_filtered(&b.condition)?.truthy();
        if truthy != (i == 0) {
            return r.render_nodes(&b.body, out);
        }
    }
    r.render_nodes(else_body, out)
}

/// Every `when` with a matching value renders, in order. Within one
/// `when`, the first matching value is enough.
pub(super) fn case(
    r: &mut Renderer<'_>,
    value: &FilteredValue,
    whens: &[When],
    else_body: &[Node],
    out: &mut String,
) -> R<()> {
    let target = Val::V(r.eval_filtered(value)?.to_value());
    let mut hit = false;
    for w in whens {
        for v in &w.values {
            let v = r.eval(v)?;
            if ops::equals(&target, &v) {
                r.render_nodes(&w.body, out)?;
                hit = true;
                break;
            }
        }
    }
    if !hit {
        r.render_nodes(else_body, out)?;
    }
    Ok(())
}
