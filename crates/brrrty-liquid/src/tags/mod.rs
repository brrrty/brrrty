//! Tag rendering, one module per tag family.

mod branch;
mod iteration;
mod layout;
mod variable;

use crate::ast::{Tag, TagKind};
use crate::error::ErrorKind;
use crate::render::{R, RenderErr, Renderer};

/// Renders one tag into `out`.
pub(crate) fn render(r: &mut Renderer<'_>, tag: &Tag, out: &mut String) -> R<()> {
    match &tag.kind {
        TagKind::If {
            branches,
            else_body,
        } => branch::if_tag(r, branches, else_body.as_deref(), out),
        TagKind::Unless {
            branches,
            else_body,
        } => branch::unless(r, branches, else_body, out),
        TagKind::Case {
            value,
            whens,
            else_body,
        } => branch::case(r, value, whens, else_body, out),
        TagKind::For(f) => iteration::for_tag(r, f, out),
        TagKind::Tablerow(t) => iteration::tablerow(r, t, out),
        TagKind::Break => {
            r.ctx.break_called = true;
            Ok(())
        }
        TagKind::Continue => {
            r.ctx.continue_called = true;
            Ok(())
        }
        TagKind::Cycle { group, candidates } => {
            iteration::cycle(r, group.as_ref(), candidates, out)
        }
        TagKind::Assign { name, value } => variable::assign(r, name, value),
        TagKind::Capture { name, body } => variable::capture(r, name, body),
        TagKind::Increment { name } => variable::increment(r, name, 1.0, out),
        TagKind::Decrement { name } => variable::increment(r, name, -1.0, out),
        TagKind::Echo { value } => variable::echo(r, value.as_ref(), out),
        TagKind::Raw { text } => r.write(out, text),
        TagKind::Comment | TagKind::InlineComment => Ok(()),
        TagKind::Liquid { body } => r.render_nodes(body, out),
        TagKind::Layout(l) => layout::layout(r, l, out),
        TagKind::Block { body, .. } => layout::block(r, body, out),
        TagKind::Include(_) | TagKind::Render(_) => Err(RenderErr::new(
            ErrorKind::Render,
            format!("`{{% {} %}}` is not supported yet", tag.name),
        )),
    }
}
