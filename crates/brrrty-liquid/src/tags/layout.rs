//! `layout` and `block`.

use std::collections::HashMap;

use crate::ast::{FilePath, Layout, Node};
use crate::context::Val;
use crate::error::ErrorKind;
use crate::render::{R, RenderErr, Renderer};

/// `{% layout none %}` renders what follows it. A layout file needs the
/// partial loader.
pub(super) fn layout(r: &mut Renderer<'_>, l: &Layout, out: &mut String) -> R<()> {
    match l.file {
        FilePath::None => r.render_nodes(&l.body, out),
        _ => Err(RenderErr::new(
            ErrorKind::Render,
            "`{% layout %}` with a file is not supported yet",
        )),
    }
}

/// Outside a layout, a block renders its body with `block` in scope.
pub(super) fn block(r: &mut Renderer<'_>, body: &[Node], out: &mut String) -> R<()> {
    r.ctx
        .scopes
        .push(HashMap::from([("block".to_string(), Val::Block)]));
    let result = r.render_nodes(body, out);
    r.ctx.scopes.pop();
    result
}
