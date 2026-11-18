//! LiquidJS's operators, property reads and printing, with the JavaScript
//! coercions they rely on.

use std::cmp::Ordering;
use std::sync::Arc;

use brrrty_js::{Value, array_index, is_js_whitespace, js_number_to_string};

use crate::context::Val;
use crate::engine::Tz;

/// LiquidJS's `equals`: a comparable drop on either side decides; arrays
/// compare item by item; everything else is `===`.
pub(crate) fn equals(l: &Val, r: &Val) -> bool {
    if l.is_comparable() {
        return drop_equals(l, r);
    }
    if r.is_comparable() {
        return drop_equals(r, l);
    }
    values_equal(&l.to_value(), &r.to_value())
}

fn values_equal(l: &Value, r: &Value) -> bool {
    match (l, r) {
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| values_equal(x, y))
        }
        (Value::Array(_), _) => false,
        _ => strict_equals(l, r),
    }
}

/// JavaScript's `===`. Objects are equal only to themselves.
///
/// Dates differ: Brrrty's dates have no identity, so two dates are equal
/// when they are the same moment. In LiquidJS two `Date` objects for the
/// same moment are unequal.
fn strict_equals(l: &Value, r: &Value) -> bool {
    match (l, r) {
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Array(a), Value::Array(b)) => Arc::ptr_eq(a, b),
        (Value::Object(a), Value::Object(b)) => Arc::ptr_eq(a, b),
        (Value::Date(a), Value::Date(b)) => a == b,
        // One function from the data is one handle.
        (Value::Function(a), Value::Function(b)) => a == b,
        _ => false,
    }
}

/// `NullDrop`, `EmptyDrop` and `BlankDrop`'s `equals`.
fn drop_equals(d: &Val, other: &Val) -> bool {
    match d {
        Val::Nil => matches!(other.to_value(), Value::Null | Value::Undefined),
        Val::Empty => empty_equals(other),
        Val::Blank => {
            if matches!(other, Val::V(Value::Bool(false))) {
                return true;
            }
            if matches!(other.to_value(), Value::Null | Value::Undefined) {
                return true;
            }
            if let Val::V(Value::String(s)) = other {
                return s.chars().all(is_js_whitespace);
            }
            empty_equals(other)
        }
        _ => unreachable!("only nil, empty and blank are comparable"),
    }
}

fn empty_equals(other: &Val) -> bool {
    if matches!(other, Val::Empty | Val::Blank) {
        return false;
    }
    match other.to_value() {
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        // `Object.keys(date)` is empty, and so is a function's.
        Value::Date(_) | Value::Function(_) => true,
        _ => false,
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Cmp {
    Gt,
    Lt,
    Ge,
    Le,
}

/// `>`, `<`, `>=` and `<=`. Comparable drops say no to everything;
/// anything else compares as JavaScript does.
pub(crate) fn compare(op: Cmp, l: &Val, r: &Val, tz: Tz) -> bool {
    if l.is_comparable() || r.is_comparable() {
        return false;
    }
    let (l, r) = (l.to_value(), r.to_value());
    match op {
        Cmp::Lt => js_less(&l, &r, tz) == Some(true),
        Cmp::Gt => js_less(&r, &l, tz) == Some(true),
        Cmp::Le => js_less(&r, &l, tz) == Some(false),
        Cmp::Ge => js_less(&l, &r, tz) == Some(false),
    }
}

/// JavaScript's `a < b`, with `None` where the answer is undefined (a NaN).
fn js_less(a: &Value, b: &Value, tz: Tz) -> Option<bool> {
    let (a, b) = (to_primitive_number(a, tz), to_primitive_number(b, tz));
    if let (Value::String(x), Value::String(y)) = (&a, &b) {
        return Some(x.encode_utf16().cmp(y.encode_utf16()) == Ordering::Less);
    }
    let (x, y) = (a.to_js_number(), b.to_js_number());
    if x.is_nan() || y.is_nan() {
        return None;
    }
    Some(x < y)
}

/// `ToPrimitive(v, number)`.
fn to_primitive_number(v: &Value, tz: Tz) -> Value {
    match v {
        Value::Array(_) | Value::Object(_) => Value::String(to_js_string(v, tz).into()),
        Value::Date(d) => Value::Number(d.timestamp_millis() as f64),
        other => other.clone(),
    }
}

/// `ToPrimitive(v)` with the default hint, as `+` uses it.
pub(crate) fn to_primitive(v: &Val, tz: Tz) -> Value {
    match v {
        Val::V(v @ (Value::Array(_) | Value::Object(_) | Value::Date(_))) => {
            Value::String(to_js_string(v, tz).into())
        }
        Val::Block => Value::String("[object Object]".into()),
        other => other.to_value(),
    }
}

/// JavaScript's `a + b` on primitives.
pub(crate) fn js_add(a: &Value, b: &Value, tz: Tz) -> Value {
    if matches!(a, Value::String(_)) || matches!(b, Value::String(_)) {
        let mut s = to_js_string(a, tz);
        s.push_str(&to_js_string(b, tz));
        return Value::String(s.into());
    }
    Value::Number(a.to_js_number() + b.to_js_number())
}

/// The `[start, end)` that `array.slice(start, end)` takes from `len`
/// items. `end: None` is `slice(start)`.
pub(crate) fn slice_bounds(len: usize, start: f64, end: Option<f64>) -> (usize, usize) {
    let len_f = len as f64;
    let clamp = |n: f64| {
        let n = if n.is_nan() { 0.0 } else { n.trunc() };
        if n < 0.0 {
            (len_f + n).max(0.0)
        } else {
            n.min(len_f)
        }
    };
    let from = clamp(start) as usize;
    let to = end.map_or(len, |e| clamp(e) as usize);
    (from, to.max(from))
}

/// The `contains` operator.
pub(crate) fn contains(l: &Val, r: &Val, tz: Tz) -> bool {
    match l.to_value() {
        Value::Array(a) => a.iter().any(|item| equals(&Val::V(item.clone()), r)),
        // `String.prototype.indexOf` converts its argument to a string.
        Value::String(s) => s.contains(to_js_string(&r.to_value(), tz).as_str()),
        _ => false,
    }
}

/// `String(v)`, printing dates in `tz`.
pub(crate) fn to_js_string(v: &Value, tz: Tz) -> String {
    v.to_js_string_in(tz)
}

/// `String(val)` with a template literal, as `cycle` names its group: a
/// drop has no `toString` of its own.
pub(crate) fn template_string(v: &Val, tz: Tz) -> String {
    match v {
        Val::V(v) => to_js_string(v, tz),
        _ => "[object Object]".into(),
    }
}

/// LiquidJS's `stringify`, which outputs use: strings as they are, null
/// and undefined as nothing, arrays as their items run together.
pub(crate) fn stringify(v: &Value, tz: Tz) -> String {
    let mut out = String::new();
    stringify_into(v, tz, &mut out);
    out
}

fn stringify_into(v: &Value, tz: Tz, out: &mut String) {
    match v {
        Value::String(s) => out.push_str(s),
        Value::Null | Value::Undefined => {}
        Value::Array(a) => {
            for x in a.iter() {
                crate::lexer::grow(|| stringify_into(x, tz, out));
            }
        }
        other => out.push_str(&to_js_string(other, tz)),
    }
}

/// LiquidJS's `toEnumerable`: what `for` and `tablerow` loop over.
pub(crate) fn to_enumerable(v: &Val) -> Vec<Value> {
    match v.to_value() {
        Value::Array(a) => a.as_ref().clone(),
        Value::String(s) if !s.is_empty() => vec![Value::String(s)],
        Value::Object(o) => Value::js_entries(&o)
            .into_iter()
            .map(|(k, v)| Value::Array(Arc::new(vec![Value::String(k.as_str().into()), v.clone()])))
            .collect(),
        _ => Vec::new(),
    }
}

/// How many items [`to_enumerable`] gives, without building them.
pub(crate) fn enumerable_len(v: &Val) -> usize {
    match v.to_value() {
        Value::Array(a) => a.len(),
        Value::String(s) if !s.is_empty() => 1,
        Value::Object(o) => o.len(),
        _ => 0,
    }
}

/// LiquidJS's `readProperty` with `ownPropertyOnly`: own properties only,
/// negative array indexes, and `size`, `first` and `last`.
pub(crate) fn read_property(obj: &Val, key: &Value) -> Val {
    let v = match obj {
        Val::V(v) => v,
        Val::Loop(d) => return d.get(&key.to_js_string()),
        Val::Block => {
            return if key.to_js_string() == "super" {
                // The block's parent content. Without a layout there is none.
                Val::string("")
            } else {
                Val::undefined()
            };
        }
        Val::Nil | Val::Empty | Val::Blank => return Val::undefined(),
    };
    if matches!(v, Value::Null | Value::Undefined) {
        return Val::V(v.clone());
    }
    if let (Value::Array(a), Value::Number(n)) = (v, key) {
        let len = a.len() as f64;
        let i = if *n < 0.0 { len + n } else { *n };
        return Val::V(if i.fract() == 0.0 && i >= 0.0 && i < len {
            a[i as usize].clone()
        } else {
            Value::Undefined
        });
    }
    let name = key.to_js_string();
    let mut own = own_property(v, &name);
    if let Value::Object(o) = v {
        own = brrrty_js::traps::read(o, &name, own);
    }
    // LiquidJS calls a function it reads, with the object as `this`, and
    // awaits what it returns.
    if let Some(Value::Function(h)) = own {
        return Val::V(call_data_fn(h, brrrty_js::functions::This::Value(v)));
    }
    let special = matches!(key, Value::String(_));
    Val::V(match name.as_str() {
        "size" if special => own.unwrap_or_else(|| size_of(v)),
        "first" if special => match v {
            Value::Array(a) => a.first().cloned().unwrap_or(Value::Undefined),
            _ => own.unwrap_or(Value::Undefined),
        },
        "last" if special => match v {
            Value::Array(a) => a.last().cloned().unwrap_or(Value::Undefined),
            _ => own.unwrap_or(Value::Undefined),
        },
        _ => own.unwrap_or(Value::Undefined),
    })
}

/// Call a function from JavaScript data as LiquidJS does when it reads one:
/// no arguments, the result awaited.
pub(crate) fn call_data_fn(h: brrrty_js::FnHandle, this: brrrty_js::functions::This<'_>) -> Value {
    brrrty_js::functions::call(h, this, &[], brrrty_js::functions::Settle::Await)
}

pub(crate) fn own_property(v: &Value, key: &str) -> Option<Value> {
    match v {
        Value::Array(a) => match key {
            "length" => Some(Value::Number(a.len() as f64)),
            _ => array_index(key).and_then(|i| a.get(i as usize).cloned()),
        },
        Value::String(s) => match key {
            "length" => Some(Value::Number(s.encode_utf16().count() as f64)),
            _ => {
                let i = array_index(key)? as usize;
                let unit = s.encode_utf16().nth(i)?;
                Some(Value::String(String::from_utf16_lossy(&[unit]).into()))
            }
        },
        Value::Object(o) => o.get(key).cloned(),
        // A function's own `name` and `length`: JavaScript's to say.
        Value::Function(_) => {
            brrrty_js::functions::used_as_value("reads a property of");
            None
        }
        _ => None,
    }
}

/// LiquidJS's `readSize` for a value with no own `size`.
fn size_of(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Number(a.len() as f64),
        Value::String(s) => Value::Number(s.encode_utf16().count() as f64),
        Value::Object(o) => Value::Number(o.len() as f64),
        Value::Date(_) => Value::Number(0.0),
        _ => Value::Undefined,
    }
}

/// Formats a number as JavaScript prints it in a template literal.
pub(crate) fn num(n: f64) -> String {
    js_number_to_string(n)
}
