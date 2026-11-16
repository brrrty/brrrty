//! `default`, `json` and the other filters that fit nowhere else.

use brrrty_js::Value;
use indexmap::IndexMap;

use super::{Args, Out, json, string, throw};
use crate::context::{LoopDrop, Val};
use crate::render::Renderer;

/// `default(value, defaultValue, ...args)`.
pub(super) fn default(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let value = v.to_value();
    let fallback = Args(a).value(0);
    match &value {
        Value::Array(items) => {
            return Ok(if items.is_empty() { fallback } else { value });
        }
        Value::String(s) => {
            return Ok(if s.is_empty() { fallback } else { value });
        }
        _ => {}
    }
    if value == Value::Bool(false) && allow_false(r, a.get(1..).unwrap_or(&[]))? {
        return Ok(Value::Bool(false));
    }
    Ok(
        if matches!(value, Value::Bool(false) | Value::Null | Value::Undefined) {
            fallback
        } else {
            value
        },
    )
}

/// `new Map(args).get('allow_false')`, which throws for an argument that is
/// not a `[key, value]` pair.
fn allow_false(r: &Renderer<'_>, args: &[Val]) -> Result<bool, crate::render::RenderErr> {
    let mut found = Value::Undefined;
    for arg in args {
        let entry = arg.to_value();
        let (key, value) = match &entry {
            Value::Array(pair) => (
                pair.first().cloned().unwrap_or(Value::Undefined),
                pair.get(1).cloned().unwrap_or(Value::Undefined),
            ),
            Value::Object(o) => (
                o.get("0").cloned().unwrap_or(Value::Undefined),
                o.get("1").cloned().unwrap_or(Value::Undefined),
            ),
            Value::Date(_) => (Value::Undefined, Value::Undefined),
            other => {
                return Err(throw(format!(
                    "Iterator value {} is not an entry object",
                    super::js_string(r, other)
                )));
            }
        };
        if matches!(&key, Value::String(k) if &**k == "allow_false") {
            found = value;
        }
    }
    Ok(found.js_truthy())
}

pub(super) fn raw(_: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    Ok(v.to_value())
}

/// A drop as `JSON.stringify` sees it.
fn json_value(v: &Val) -> Value {
    match v {
        Val::V(v) => v.clone(),
        Val::Loop(d) => loop_object(d),
        Val::Nil | Val::Empty | Val::Blank | Val::Block => Value::Object(Default::default()),
    }
}

fn loop_object(d: &LoopDrop) -> Value {
    let mut o = IndexMap::new();
    o.insert("i".to_string(), Value::Number(d.i.get() as f64));
    o.insert("length".to_string(), Value::Number(d.length as f64));
    o.insert("name".to_string(), string(d.name.as_str()));
    if let Some(cols) = &d.cols {
        o.insert("cols".to_string(), cols.to_value());
    }
    Value::Object(std::sync::Arc::new(o))
}

pub(super) fn json(_: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let indent = json::indent_of(&Args(a).or(0, Value::Number(0.0)));
    let value = json_value(v);
    Ok(json::stringify(&value, &indent).map_or(Value::Undefined, string))
}

/// `inspect` is `JSON.stringify` that writes `[Circular]` for a cycle. Data
/// here has none.
pub(super) fn inspect(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    json(r, v, a)
}

/// `Number(value)`, without any rounding to an integer.
pub(super) fn to_integer(_: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    Ok(Value::Number(v.to_value().to_js_number()))
}
