//! `layout` and `block`.

use std::collections::HashMap;

use crate::ast::{FilePath, Layout, Node};
use crate::context::Val;
use crate::error::ErrorKind;
use crate::render::{R, RenderErr, Renderer};

/// `{% layout none %}` renders what follows it. LiquidJS's layout files
/// (`{% layout "name" %}` with `{% block %}`s) are not supported: 11ty sites
/// use the front matter `layout` key instead.
pub(super) fn layout(r: &mut Renderer<'_>, l: &Layout, out: &mut String) -> R<()> {
    match l.file {
        FilePath::None => r.render_nodes(&l.body, out),
        _ => Err(RenderErr::new(
            ErrorKind::Render,
            "the `{% layout %}` tag with a file isn't supported",
        )
        .with_help(
            "use 11ty's front matter instead: put `layout: name` between the `---` lines and print `{{ content }}` in the layout",
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
