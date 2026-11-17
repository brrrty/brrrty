//! JavaScript semantics for compiled template code: conversions,
//! operators, `runtime.memberLookup`, `lib.inOperator`, and the built-in
//! methods a template may call on strings, arrays, numbers, booleans,
//! objects and dates.
//!
//! Methods are emulated only where Brrrty matches V8 exactly; every other
//! method of those prototypes is still a function (so `typeof` and
//! truthiness are right) but calling it is refused.

use std::rc::Rc;
use std::sync::Arc;

use brrrty_js::{Tz, Value, js_number_to_string, js_string_to_number};

use crate::error::ErrorKind;
use crate::render::{R, RenderErr};
use crate::value::{Func, Obj, Val};

/// `ToPrimitive`'s hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hint {
    Default,
    Number,
    String,
}

/// What the operations need from the render.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ops {
    pub tz: Tz,
    /// The most bytes a string a method builds may have.
    pub limit: usize,
}

pub(crate) fn js_err(name: &str, message: impl AsRef<str>) -> RenderErr {
    RenderErr::new(ErrorKind::Render, format!("{name}: {}", message.as_ref()))
}

pub(crate) fn type_error(message: impl AsRef<str>) -> RenderErr {
    js_err("TypeError", message)
}

pub(crate) fn unsupported(message: impl Into<String>) -> RenderErr {
    RenderErr::new(ErrorKind::Unsupported, message)
}

// ---- conversions ----

/// `ToPrimitive`, giving a primitive [`Value`].
pub(crate) fn to_primitive(v: &Val, hint: Hint, ops: Ops) -> R<Value> {
    let mut seen = Vec::new();
    to_primitive_in(v, hint, ops, &mut seen)
}

fn to_primitive_in(v: &Val, hint: Hint, ops: Ops, seen: &mut Vec<usize>) -> R<Value> {
    Ok(match v {
        Val::V(Value::Array(a)) => {
            no_own_conversion_data(None)?;
            let items: Vec<Val> = a.iter().cloned().map(Val::V).collect();
            Value::String(join(&items, ",", ops, seen)?.into())
        }
        Val::V(Value::Object(o)) => {
            no_own_conversion_data(Some(o))?;
            Value::String("[object Object]".into())
        }
        Val::V(Value::Date(d)) => {
            let ms = d.timestamp_millis() as f64;
            if hint == Hint::Number {
                Value::Number(ms)
            } else {
                Value::String(brrrty_js::tz::date_to_string(ops.tz, ms).into())
            }
        }
        Val::V(p) => p.clone(),
        Val::Safe(s) => Value::String(s.0.clone()),
        Val::Arr(a) => {
            let ptr = Rc::as_ptr(a) as usize;
            if seen.contains(&ptr) {
                return Err(unsupported(
                    "an array that contains itself can't be turned into a string",
                ));
            }
            seen.push(ptr);
            let items = a.borrow().clone();
            let s = join(&items, ",", ops, seen)?;
            seen.pop();
            Value::String(s.into())
        }
        Val::Obj(o) => {
            for key in ["toString", "valueOf"] {
                if o.has(key) {
                    return Err(unsupported(format!(
                        "an object with its own `{key}` can't be turned into a string or number here"
                    )));
                }
            }
            Value::String("[object Object]".into())
        }
        // `valueOf` gives the object itself, so both hints use `toString`.
        Val::Re(r) => Value::String(r.lit.to_js_string().into()),
        Val::Func(_) => {
            return Err(unsupported(
                "a function can't be printed or turned into a string or number",
            )
            .with_help(
                "Nunjucks would print the function's JavaScript source; call it with `()` instead",
            ));
        }
    })
}

fn no_own_conversion_data(o: Option<&indexmap::IndexMap<String, Value>>) -> R<()> {
    if let Some(o) = o {
        for key in ["toString", "valueOf"] {
            if o.contains_key(key) {
                return Err(unsupported(format!(
                    "an object with its own `{key}` property can't be turned into a string or number"
                )));
            }
        }
    }
    Ok(())
}

/// `ToString` of a primitive.
pub(crate) fn prim_to_string(p: &Value) -> Arc<str> {
    match p {
        Value::String(s) => s.clone(),
        Value::Undefined => "undefined".into(),
        Value::Null => "null".into(),
        Value::Bool(b) => if *b { "true" } else { "false" }.into(),
        Value::Number(n) => js_number_to_string(*n).into(),
        other => other.to_js_string().into(),
    }
}

/// `ToNumber` of a primitive.
pub(crate) fn prim_to_number(p: &Value) -> f64 {
    match p {
        Value::Undefined => f64::NAN,
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => *n,
        Value::String(s) => js_string_to_number(s),
        other => other.to_js_number(),
    }
}

pub(crate) fn to_string(v: &Val, ops: Ops) -> R<Arc<str>> {
    if let Val::V(Value::String(s)) = v {
        return Ok(s.clone());
    }
    Ok(prim_to_string(&to_primitive(v, Hint::String, ops)?))
}

pub(crate) fn to_number(v: &Val, ops: Ops) -> R<f64> {
    if let Val::V(Value::Number(n)) = v {
        return Ok(*n);
    }
    Ok(prim_to_number(&to_primitive(v, Hint::Number, ops)?))
}

/// `ToIntegerOrInfinity`.
pub(crate) fn to_integer(v: &Val, ops: Ops) -> R<f64> {
    let n = to_number(v, ops)?;
    Ok(if n.is_nan() { 0.0 } else { n.trunc() + 0.0 })
}

/// `ToPropertyKey`.
pub(crate) fn property_key(v: &Val, ops: Ops) -> R<Arc<str>> {
    to_string(v, ops)
}

fn join(items: &[Val], sep: &str, ops: Ops, seen: &mut Vec<usize>) -> R<String> {
    let mut out = String::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(sep);
        }
        if !item.is_nullish() {
            let p = crate::render::grow(|| to_primitive_in(item, Hint::String, ops, seen))?;
            out.push_str(&prim_to_string(&p));
        }
        if out.len() > ops.limit {
            return Err(too_big(ops));
        }
    }
    Ok(out)
}

pub(crate) fn too_big(ops: Ops) -> RenderErr {
    RenderErr::new(
        ErrorKind::Limit,
        format!(
            "a string or array grew past {} MiB",
            ops.limit / (1024 * 1024)
        ),
    )
    .with_help("look for a loop that keeps doubling a string or an array")
}

// ---- output ----

/// Nunjucks's `lib.escape`.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\\' => out.push_str("&#92;"),
            c => out.push(c),
        }
    }
    out
}

/// `val.toString()`, as output and the `escape` and `safe` filters call it.
pub(crate) fn to_string_method(v: &Val, ops: Ops) -> R<Arc<str>> {
    if let Val::V(Value::Object(o)) = v
        && o.contains_key("toString")
    {
        return Err(type_error("val.toString is not a function"));
    }
    to_string(v, ops)
}

/// `runtime.suppressValue(val, autoescape)` as a string.
pub(crate) fn suppress(v: &Val, autoescape: bool, ops: Ops) -> R<String> {
    match v {
        Val::V(Value::Undefined | Value::Null) => Ok(String::new()),
        Val::Safe(s) => Ok(s.0.to_string()),
        _ if autoescape => Ok(escape(&to_string_method(v, ops)?)),
        // `output += val`.
        _ => Ok(prim_to_string(&to_primitive(v, Hint::Default, ops)?).to_string()),
    }
}

// ---- operators ----

pub(crate) fn add(a: &Val, b: &Val, ops: Ops) -> R<Val> {
    if let (Val::V(Value::Number(x)), Val::V(Value::Number(y))) = (a, b) {
        return Ok(Val::num(x + y));
    }
    let pa = to_primitive(a, Hint::Default, ops)?;
    let pb = to_primitive(b, Hint::Default, ops)?;
    if matches!(pa, Value::String(_)) || matches!(pb, Value::String(_)) {
        let (sa, sb) = (prim_to_string(&pa), prim_to_string(&pb));
        if sa.len() + sb.len() > ops.limit {
            return Err(too_big(ops));
        }
        // `a ~ b` is `a + "" + b`: adding "" gives the same string, so it
        // is shared instead of copied.
        if sb.is_empty() {
            return Ok(Val::V(Value::String(sa)));
        }
        if sa.is_empty() {
            return Ok(Val::V(Value::String(sb)));
        }
        let mut s = String::with_capacity(sa.len() + sb.len());
        s.push_str(&sa);
        s.push_str(&sb);
        return Ok(Val::str(s));
    }
    Ok(Val::num(prim_to_number(&pa) + prim_to_number(&pb)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    StrictEq,
    Ne,
    StrictNe,
    Lt,
    Gt,
    Le,
    Ge,
}

impl BinOp {
    /// JavaScript precedence.
    pub fn precedence(self) -> u8 {
        match self {
            BinOp::Eq | BinOp::StrictEq | BinOp::Ne | BinOp::StrictNe => 8,
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => 9,
            BinOp::Add | BinOp::Sub => 11,
            BinOp::Mul | BinOp::Div | BinOp::Mod => 12,
        }
    }
}

pub(crate) fn binary(op: BinOp, a: &Val, b: &Val, ops: Ops) -> R<Val> {
    let num = |f: fn(f64, f64) -> f64| -> R<Val> {
        let x = to_number(a, ops)?;
        let y = to_number(b, ops)?;
        Ok(Val::num(f(x, y)))
    };
    match op {
        BinOp::Add => add(a, b, ops),
        BinOp::Sub => num(|x, y| x - y),
        BinOp::Mul => num(|x, y| x * y),
        BinOp::Div => num(|x, y| x / y),
        BinOp::Mod => num(|x, y| x % y),
        BinOp::Eq => Ok(Val::bool(loose_eq(a, b, ops)?)),
        BinOp::Ne => Ok(Val::bool(!loose_eq(a, b, ops)?)),
        BinOp::StrictEq => Ok(Val::bool(strict_eq(a, b)?)),
        BinOp::StrictNe => Ok(Val::bool(!strict_eq(a, b)?)),
        BinOp::Lt => Ok(Val::bool(less_than(a, b, true, ops)? == Some(true))),
        BinOp::Gt => Ok(Val::bool(less_than(b, a, false, ops)? == Some(true))),
        BinOp::Le => Ok(Val::bool(less_than(b, a, false, ops)? == Some(false))),
        BinOp::Ge => Ok(Val::bool(less_than(a, b, true, ops)? == Some(false))),
    }
}

/// `Math.pow`.
pub(crate) fn pow(x: f64, y: f64) -> f64 {
    if y.is_nan() || (x.abs() == 1.0 && y.is_infinite()) {
        return f64::NAN;
    }
    x.powf(y)
}

fn is_object(v: &Val) -> bool {
    !matches!(
        v,
        Val::V(
            Value::Undefined | Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
        )
    )
}

fn dates_refused() -> RenderErr {
    unsupported("comparing two dates with `==` or `===` compares which object each is, which Brrrty can't tell")
        .with_help("compare their times instead: `a.getTime() == b.getTime()`")
}

/// `===`.
pub(crate) fn strict_eq(a: &Val, b: &Val) -> R<bool> {
    Ok(match (a, b) {
        (Val::V(Value::Date(_)), Val::V(Value::Date(_))) => return Err(dates_refused()),
        (Val::V(x), Val::V(y)) => match (x, y) {
            (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
            (Value::Bool(p), Value::Bool(q)) => p == q,
            (Value::Number(p), Value::Number(q)) => p == q,
            (Value::String(p), Value::String(q)) => p == q,
            _ => a.same_object(b),
        },
        _ => a.same_object(b),
    })
}

/// `SameValueZero`, for `includes`.
fn same_value_zero(a: &Val, b: &Val) -> R<bool> {
    if let (Val::V(Value::Number(x)), Val::V(Value::Number(y))) = (a, b)
        && x.is_nan()
        && y.is_nan()
    {
        return Ok(true);
    }
    strict_eq(a, b)
}

/// `==`.
pub(crate) fn loose_eq(a: &Val, b: &Val, ops: Ops) -> R<bool> {
    let (oa, ob) = (is_object(a), is_object(b));
    if oa && ob {
        return strict_eq(a, b);
    }
    if !oa && !ob {
        let (Val::V(x), Val::V(y)) = (a, b) else {
            unreachable!("primitives are plain values")
        };
        return Ok(prim_loose_eq(x, y));
    }
    // One object, one primitive.
    let (obj, prim) = if oa { (a, b) } else { (b, a) };
    let Val::V(p) = prim else {
        unreachable!("a primitive is a plain value")
    };
    if matches!(p, Value::Undefined | Value::Null) {
        return Ok(false);
    }
    let op = to_primitive(obj, Hint::Default, ops)?;
    Ok(prim_loose_eq(&op, p))
}

fn prim_loose_eq(x: &Value, y: &Value) -> bool {
    match (x, y) {
        (Value::Undefined | Value::Null, Value::Undefined | Value::Null) => true,
        (Value::Undefined | Value::Null, _) | (_, Value::Undefined | Value::Null) => false,
        (Value::Number(p), Value::Number(q)) => p == q,
        (Value::String(p), Value::String(q)) => p == q,
        (Value::Bool(p), Value::Bool(q)) => p == q,
        _ => prim_to_number(x) == prim_to_number(y),
    }
}

/// `IsLessThan(x, y)`: `None` when either side is NaN.
fn less_than(x: &Val, y: &Val, left_first: bool, ops: Ops) -> R<Option<bool>> {
    let (px, py) = if left_first {
        let px = to_primitive(x, Hint::Number, ops)?;
        (px, to_primitive(y, Hint::Number, ops)?)
    } else {
        let py = to_primitive(y, Hint::Number, ops)?;
        (to_primitive(x, Hint::Number, ops)?, py)
    };
    if let (Value::String(a), Value::String(b)) = (&px, &py) {
        return Ok(Some(a.encode_utf16().lt(b.encode_utf16())));
    }
    let (nx, ny) = (prim_to_number(&px), prim_to_number(&py));
    if nx.is_nan() || ny.is_nan() {
        return Ok(None);
    }
    Ok(Some(nx < ny))
}

// ---- strings as UTF-16 ----

pub(crate) fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// A string from UTF-16 code units. JavaScript can hold a lone surrogate,
/// which Rust can't; such a result is refused.
pub(crate) fn from_utf16(units: &[u16]) -> R<String> {
    String::from_utf16(units).map_err(|_| {
        unsupported("the result would split a character outside the Basic Multilingual Plane (such as an emoji) in half")
            .with_help("JavaScript strings count such a character as two units; take whole characters")
    })
}

fn find(hay: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return (from <= hay.len()).then_some(from);
    }
    if needle.len() > hay.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

fn rfind(hay: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.len() > hay.len() {
        return None;
    }
    let start = from.min(hay.len() - needle.len());
    (0..=start)
        .rev()
        .find(|&i| &hay[i..i + needle.len()] == needle)
}

/// A relative index (`slice`, `at`): negative counts from the end, then
/// clamped to `0..=len`.
fn relative(n: f64, len: usize) -> usize {
    let len = len as f64;
    let i = if n < 0.0 {
        (len + n).max(0.0)
    } else {
        n.min(len)
    };
    i as usize
}

fn clamp(n: f64, len: usize) -> usize {
    n.max(0.0).min(len as f64) as usize
}

fn arg(args: &[Val], i: usize) -> Val {
    args.get(i).cloned().unwrap_or_else(Val::undef)
}

// ---- properties ----

/// Function-valued properties of each prototype, as V8 has them.
const STRING_PROTO: &[&str] = &[
    "constructor",
    "anchor",
    "at",
    "big",
    "blink",
    "bold",
    "charAt",
    "charCodeAt",
    "codePointAt",
    "concat",
    "endsWith",
    "fontcolor",
    "fontsize",
    "fixed",
    "includes",
    "indexOf",
    "isWellFormed",
    "italics",
    "lastIndexOf",
    "link",
    "localeCompare",
    "match",
    "matchAll",
    "normalize",
    "padEnd",
    "padStart",
    "repeat",
    "replace",
    "replaceAll",
    "search",
    "slice",
    "small",
    "split",
    "strike",
    "sub",
    "substr",
    "substring",
    "sup",
    "startsWith",
    "toString",
    "toWellFormed",
    "trim",
    "trimStart",
    "trimLeft",
    "trimEnd",
    "trimRight",
    "toLocaleLowerCase",
    "toLocaleUpperCase",
    "toLowerCase",
    "toUpperCase",
    "valueOf",
];
const ARRAY_PROTO: &[&str] = &[
    "constructor",
    "at",
    "concat",
    "copyWithin",
    "fill",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "lastIndexOf",
    "pop",
    "push",
    "reverse",
    "shift",
    "unshift",
    "slice",
    "sort",
    "splice",
    "includes",
    "indexOf",
    "join",
    "keys",
    "entries",
    "values",
    "forEach",
    "filter",
    "flat",
    "flatMap",
    "map",
    "every",
    "some",
    "reduce",
    "reduceRight",
    "toReversed",
    "toSorted",
    "toSpliced",
    "with",
    "toLocaleString",
    "toString",
];
const NUMBER_PROTO: &[&str] = &[
    "constructor",
    "toExponential",
    "toFixed",
    "toPrecision",
    "toString",
    "valueOf",
    "toLocaleString",
];
const BOOLEAN_PROTO: &[&str] = &["constructor", "toString", "valueOf"];
pub(crate) const OBJECT_PROTO: &[&str] = &[
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "toLocaleString",
];
const DATE_PROTO: &[&str] = &[
    "constructor",
    "toString",
    "toDateString",
    "toTimeString",
    "toISOString",
    "toUTCString",
    "toGMTString",
    "getDate",
    "setDate",
    "getDay",
    "setFullYear",
    "getFullYear",
    "getHours",
    "setHours",
    "getMilliseconds",
    "setMilliseconds",
    "getMinutes",
    "setMinutes",
    "getMonth",
    "setMonth",
    "getSeconds",
    "setSeconds",
    "getTime",
    "setTime",
    "getTimezoneOffset",
    "getUTCDate",
    "setUTCDate",
    "getUTCDay",
    "getUTCFullYear",
    "setUTCFullYear",
    "getUTCHours",
    "setUTCHours",
    "getUTCMilliseconds",
    "setUTCMilliseconds",
    "getUTCMinutes",
    "setUTCMinutes",
    "getUTCMonth",
    "setUTCMonth",
    "getUTCSeconds",
    "setUTCSeconds",
    "valueOf",
    "getYear",
    "setYear",
    "toJSON",
    "toLocaleString",
    "toLocaleDateString",
    "toLocaleTimeString",
    "toTemporalInstant",
];

/// Whether `name` is a function on `v`'s prototype chain.
fn proto_function(v: &Val, name: &str) -> bool {
    let own: &[&str] = match v {
        Val::V(Value::String(_)) | Val::Safe(_) => STRING_PROTO,
        Val::V(Value::Number(_)) => NUMBER_PROTO,
        Val::V(Value::Bool(_)) => BOOLEAN_PROTO,
        Val::V(Value::Array(_)) | Val::Arr(_) => ARRAY_PROTO,
        Val::V(Value::Date(_)) => DATE_PROTO,
        Val::Re(_) => REGEXP_PROTO,
        _ => &[],
    };
    own.contains(&name) || OBJECT_PROTO.contains(&name)
}

/// What reading a property gives.
pub(crate) enum Prop {
    Value(Val),
    /// A function: own (a macro in a namespace) or on the prototype.
    Function,
}

/// `obj[key]` for an object or primitive that isn't null or undefined.
pub(crate) fn get_property(obj: &Val, key: &str) -> R<Prop> {
    let own = match obj {
        Val::V(Value::String(s)) => string_own(s, key)?,
        Val::V(Value::Array(a)) => {
            if key == "length" {
                Some(Val::num(a.len() as f64))
            } else {
                brrrty_js::array_index(key)
                    .and_then(|i| a.get(i as usize).cloned())
                    .map(Val::V)
            }
        }
        Val::V(Value::Object(o)) => brrrty_js::traps::read(o, key, o.get(key).cloned()).map(Val::V),
        Val::V(_) => None,
        Val::Safe(s) => match key {
            "val" => Some(Val::str(s.0.clone())),
            "length" => Some(Val::num(s.0.encode_utf16().count() as f64)),
            _ => None,
        },
        Val::Arr(a) => {
            let a = a.borrow();
            if key == "length" {
                Some(Val::num(a.len() as f64))
            } else {
                brrrty_js::array_index(key).and_then(|i| a.get(i as usize).cloned())
            }
        }
        Val::Obj(o) => o.get(key),
        Val::Func(_) => {
            return Err(unsupported(format!(
                "Brrrty can't read `.{key}` of a function"
            )));
        }
        Val::Re(r) => regex_property(r, key)?,
    };
    if let Some(v) = own {
        return Ok(match v {
            Val::Func(_) => Prop::Function,
            v => Prop::Value(v),
        });
    }
    if key == "__proto__" {
        return Err(unsupported("Brrrty can't read `__proto__`"));
    }
    if proto_function(obj, key) {
        return Ok(Prop::Function);
    }
    Ok(Prop::Value(Val::undef()))
}

fn string_own(s: &str, key: &str) -> R<Option<Val>> {
    if key == "length" {
        return Ok(Some(Val::num(s.encode_utf16().count() as f64)));
    }
    let Some(i) = brrrty_js::array_index(key) else {
        return Ok(None);
    };
    match s.encode_utf16().nth(i as usize) {
        Some(unit) => Ok(Some(Val::str(from_utf16(&[unit])?))),
        None => Ok(None),
    }
}

/// `runtime.memberLookup(obj, val)`.
pub(crate) fn member(obj: &Val, key: &Val, ops: Ops) -> R<Val> {
    if obj.is_nullish() {
        return Ok(Val::undef());
    }
    let key = property_key(key, ops)?;
    Ok(match get_property(obj, &key)? {
        Prop::Value(v) => v,
        Prop::Function => Val::func(Func::Method {
            this: obj.clone(),
            name: key,
        }),
    })
}

/// A raw property read `obj[key]`, as the compiled `for` loop does it: a
/// function comes back unwrapped, which only matters if it is then printed.
pub(crate) fn raw_index(obj: &Val, key: &Val, ops: Ops) -> R<Val> {
    if obj.is_nullish() {
        let what = if obj.is_undefined() {
            "undefined"
        } else {
            "null"
        };
        // V8 shows a plain object key as `#<Object>`, and leaves the key out
        // for an array, a date, a `SafeString` and an object with its own
        // `toString`.
        let reading = match key {
            Val::Obj(_) | Val::V(Value::Object(_)) if !has_own(key, "toString")? => {
                " (reading '#<Object>')".to_string()
            }
            Val::Obj(_)
            | Val::V(Value::Object(_) | Value::Array(_) | Value::Date(_))
            | Val::Arr(_)
            | Val::Safe(_) => String::new(),
            _ => format!(" (reading '{}')", property_key(key, ops)?),
        };
        return Err(type_error(format!(
            "Cannot read properties of {what}{reading}"
        )));
    }
    member(obj, key, ops)
}

/// Whether `obj` has `key` as an own property (`hasOwnProperty`).
pub(crate) fn has_own(obj: &Val, key: &str) -> R<bool> {
    Ok(match obj {
        Val::V(Value::String(s)) => string_own(s, key)?.is_some(),
        Val::V(Value::Array(a)) => {
            key == "length" || brrrty_js::array_index(key).is_some_and(|i| (i as usize) < a.len())
        }
        Val::V(Value::Object(o)) => brrrty_js::traps::has(o, key),
        Val::Safe(_) => key == "val" || key == "length",
        Val::Arr(a) => {
            key == "length"
                || brrrty_js::array_index(key).is_some_and(|i| (i as usize) < a.borrow().len())
        }
        Val::Obj(o) => o.has(key),
        Val::Re(_) => key == "lastIndex",
        _ => false,
    })
}

/// The methods on `RegExp.prototype`.
const REGEXP_PROTO: &[&str] = &["exec", "test", "toString", "compile"];

/// A regular expression's `lastIndex`, and the accessors on its prototype.
fn regex_property(r: &crate::value::Re, key: &str) -> R<Option<Val>> {
    let flag = |f: char| Some(Val::bool(r.lit.flags.contains(f)));
    Ok(match key {
        "lastIndex" if r.moved.get() => {
            return Err(unsupported(
                "Brrrty doesn't track `lastIndex` after a global regular expression's `test()` matched",
            ));
        }
        "lastIndex" => Some(Val::num(0.0)),
        "source" => Some(Val::str(r.lit.source.as_str())),
        "flags" => Some(Val::str(r.lit.flags.as_str())),
        "global" => flag('g'),
        "ignoreCase" => flag('i'),
        "multiline" => flag('m'),
        "sticky" => flag('y'),
        "unicode" | "dotAll" | "hasIndices" | "unicodeSets" => Some(Val::bool(false)),
        _ => None,
    })
}

/// `test`, `toString` and `exec` on a regular expression.
fn regex_method(r: &crate::value::Re, name: &str, args: &[Val], ops: Ops) -> R<Val> {
    match name {
        "toString" => Ok(Val::str(r.lit.to_js_string())),
        "test" => {
            let global = r.lit.flags.contains('g');
            if global && r.moved.get() {
                return Err(unsupported(
                    "Brrrty doesn't run `test()` again on a global regular expression after it matched: JavaScript starts it at `lastIndex`",
                ));
            }
            let s = to_string(&arg(args, 0), ops)?;
            let hit = r
                .lit
                .matcher()
                .map_err(unsupported)?
                .is_match(&s)
                .map_err(unsupported)?;
            if global && hit {
                r.moved.set(true);
            }
            Ok(Val::bool(hit))
        }
        _ => Err(unsupported(format!(
            "Brrrty doesn't run the JavaScript method `{name}()` on a regular expression"
        ))),
    }
}

/// `lib.inOperator(key, val)`.
pub(crate) fn in_operator(key: &Val, val: &Val, ops: Ops) -> R<bool> {
    match val {
        Val::V(Value::Array(_)) | Val::Arr(_) => {
            let items = val.array_items().unwrap_or_default();
            for item in &items {
                if strict_eq(item, key)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Val::V(Value::String(s)) => {
            let needle = to_string(key, ops)?;
            Ok(find(&utf16(s), &utf16(&needle), 0).is_some())
        }
        Val::V(Value::Object(_)) | Val::Obj(_) => {
            let k = property_key(key, ops)?;
            Ok(has_own(val, &k)? || OBJECT_PROTO.contains(&&*k) || &*k == "__proto__")
        }
        // A SafeString is an object whose prototype is String.prototype.
        Val::Safe(_) => {
            let k = property_key(key, ops)?;
            Ok(has_own(val, &k)?
                || STRING_PROTO.contains(&&*k)
                || OBJECT_PROTO.contains(&&*k)
                || &*k == "length"
                || &*k == "__proto__")
        }
        _ => {
            let shown = prim_to_string(&to_primitive(key, Hint::Default, ops)?);
            Err(js_err(
                "Error",
                format!(
                    "Cannot use \"in\" operator to search for \"{shown}\" in unexpected types."
                ),
            ))
        }
    }
}

// ---- methods ----

fn no_method(this: &Val, name: &str) -> RenderErr {
    let on = match this {
        Val::V(Value::String(_)) => "a string",
        Val::Safe(_) => "a SafeString",
        Val::V(Value::Number(_)) => "a number",
        Val::V(Value::Bool(_)) => "a boolean",
        Val::V(Value::Array(_)) | Val::Arr(_) => "an array",
        Val::V(Value::Date(_)) => "a date",
        _ => "an object",
    };
    unsupported(format!(
        "Brrrty doesn't run the JavaScript method `{name}()` on {on}"
    ))
    .with_help("Brrrty emulates only the methods it can match exactly; use a Nunjucks filter, or do this in the data")
}

/// Calls the built-in method `name` on `this`.
pub(crate) fn call_method(this: &Val, name: &str, args: &[Val], ops: Ops) -> R<Val> {
    match this {
        Val::V(Value::String(s)) => string_method(this, s, name, args, ops),
        Val::Safe(s) => match name {
            // SafeString's own `toString` and `valueOf` give the string.
            "toString" | "valueOf" => Ok(Val::str(s.0.clone())),
            _ => string_method(this, &s.0, name, args, ops),
        },
        Val::V(Value::Number(n)) => number_method(this, *n, name, args, ops),
        Val::V(Value::Bool(b)) => match name {
            "toString" => Ok(Val::str(if *b { "true" } else { "false" })),
            "valueOf" => Ok(Val::bool(*b)),
            _ => object_method(this, name, args, ops),
        },
        Val::V(Value::Array(_)) | Val::Arr(_) => array_method(this, name, args, ops),
        Val::V(Value::Date(d)) => date_method(this, d.timestamp_millis() as f64, name, ops),
        Val::Re(r) => match name {
            "toString" | "test" | "exec" | "compile" => regex_method(r, name, args, ops),
            _ => object_method(this, name, args, ops),
        },
        _ => object_method(this, name, args, ops),
    }
}

fn object_method(this: &Val, name: &str, args: &[Val], ops: Ops) -> R<Val> {
    match name {
        "hasOwnProperty" => {
            let k = property_key(&arg(args, 0), ops)?;
            Ok(Val::bool(has_own(this, &k)?))
        }
        "propertyIsEnumerable" => {
            let k = property_key(&arg(args, 0), ops)?;
            let length = matches!(
                this,
                Val::V(Value::String(_) | Value::Array(_)) | Val::Arr(_) | Val::Safe(_)
            ) && &*k == "length";
            Ok(Val::bool(has_own(this, &k)? && !length))
        }
        "toString" => match this {
            Val::V(Value::Number(_) | Value::Bool(_) | Value::String(_)) => {
                Ok(Val::str(to_string(this, ops)?))
            }
            _ => Ok(Val::str("[object Object]")),
        },
        "valueOf" => Ok(this.clone()),
        _ => Err(no_method(this, name)),
    }
}

fn string_method(this: &Val, s: &str, name: &str, args: &[Val], ops: Ops) -> R<Val> {
    let units = || utf16(s);
    let len = || s.encode_utf16().count();
    let ret_units = |u: &[u16]| -> R<Val> { Ok(Val::str(from_utf16(u)?)) };
    match name {
        "toString" | "valueOf" => Ok(Val::str(s)),
        "localeCompare" => {
            let that = to_string(&arg(args, 0), ops)?;
            english_only(name, &args[args.len().min(1)..])?;
            match brrrty_js::js_locale_compare(s, &that) {
                Ok(o) => Ok(Val::num(o as i8 as f64)),
                Err(c) => Err(unsupported(format!(
                    "`localeCompare()`: U+{:04X} is new in Unicode 18, which Node's ICU doesn't have yet, so its order there can't be matched",
                    c as u32
                ))),
            }
        }
        "toUpperCase" => Ok(Val::str(s.to_uppercase())),
        "toLowerCase" => Ok(Val::str(s.to_lowercase())),
        "trim" => Ok(Val::str(s.trim_matches(brrrty_js::is_js_whitespace))),
        "trimStart" | "trimLeft" => Ok(Val::str(s.trim_start_matches(brrrty_js::is_js_whitespace))),
        "trimEnd" | "trimRight" => Ok(Val::str(s.trim_end_matches(brrrty_js::is_js_whitespace))),
        "isWellFormed" => Ok(Val::bool(true)),
        "toWellFormed" => Ok(Val::str(s)),
        "at" => {
            let u = units();
            let n = to_integer(&arg(args, 0), ops)?;
            let k = if n >= 0.0 { n } else { u.len() as f64 + n };
            if k < 0.0 || k >= u.len() as f64 {
                return Ok(Val::undef());
            }
            ret_units(&u[k as usize..k as usize + 1])
        }
        "charAt" => {
            let u = units();
            let p = to_integer(&arg(args, 0), ops)?;
            if p < 0.0 || p >= u.len() as f64 {
                return Ok(Val::str(""));
            }
            ret_units(&u[p as usize..p as usize + 1])
        }
        "charCodeAt" => {
            let u = units();
            let p = to_integer(&arg(args, 0), ops)?;
            if p < 0.0 || p >= u.len() as f64 {
                return Ok(Val::num(f64::NAN));
            }
            Ok(Val::num(f64::from(u[p as usize])))
        }
        "codePointAt" => {
            let u = units();
            let p = to_integer(&arg(args, 0), ops)?;
            if p < 0.0 || p >= u.len() as f64 {
                return Ok(Val::undef());
            }
            let i = p as usize;
            let first = u[i];
            if (0xD800..0xDC00).contains(&first)
                && let Some(&second) = u.get(i + 1)
                && (0xDC00..0xE000).contains(&second)
            {
                let cp =
                    0x10000 + ((u32::from(first) - 0xD800) << 10) + (u32::from(second) - 0xDC00);
                return Ok(Val::num(f64::from(cp)));
            }
            Ok(Val::num(f64::from(first)))
        }
        "concat" => {
            let mut out = s.to_string();
            for a in args {
                out.push_str(&to_string(a, ops)?);
                if out.len() > ops.limit {
                    return Err(too_big(ops));
                }
            }
            Ok(Val::str(out))
        }
        "includes" | "indexOf" | "startsWith" | "endsWith" | "lastIndexOf" => {
            if matches!(args.first(), Some(Val::Re(_)))
                && name != "indexOf"
                && name != "lastIndexOf"
            {
                return Err(type_error(format!(
                    "First argument to String.prototype.{name} must not be a regular expression"
                )));
            }
            let u = units();
            let needle = utf16(&to_string(&arg(args, 0), ops)?);
            let pos = arg(args, 1);
            match name {
                "includes" | "indexOf" => {
                    let start = clamp(to_integer(&pos, ops)?, u.len());
                    let at = find(&u, &needle, start);
                    Ok(if name == "includes" {
                        Val::bool(at.is_some())
                    } else {
                        Val::num(at.map_or(-1.0, |i| i as f64))
                    })
                }
                "startsWith" => {
                    let start = clamp(to_integer(&pos, ops)?, u.len());
                    Ok(Val::bool(u[start..].starts_with(&needle)))
                }
                "endsWith" => {
                    let end = if pos.is_undefined() {
                        u.len()
                    } else {
                        clamp(to_integer(&pos, ops)?, u.len())
                    };
                    Ok(Val::bool(u[..end].ends_with(&needle)))
                }
                _ => {
                    let n = to_number(&pos, ops)?;
                    let p = if n.is_nan() { f64::INFINITY } else { n.trunc() };
                    let start = clamp(p, u.len());
                    Ok(Val::num(
                        rfind(&u, &needle, start).map_or(-1.0, |i| i as f64),
                    ))
                }
            }
        }
        "slice" => {
            let u = units();
            let from = relative(to_integer(&arg(args, 0), ops)?, u.len());
            let end = arg(args, 1);
            let to = if end.is_undefined() {
                u.len()
            } else {
                relative(to_integer(&end, ops)?, u.len())
            };
            if from >= to {
                return Ok(Val::str(""));
            }
            ret_units(&u[from..to])
        }
        "substring" => {
            let u = units();
            let a = clamp(to_integer(&arg(args, 0), ops)?, u.len());
            let end = arg(args, 1);
            let b = if end.is_undefined() {
                u.len()
            } else {
                clamp(to_integer(&end, ops)?, u.len())
            };
            ret_units(&u[a.min(b)..a.max(b)])
        }
        "substr" => {
            let u = units();
            let size = u.len();
            let start = to_integer(&arg(args, 0), ops)?;
            let start = if start == f64::NEG_INFINITY {
                0
            } else if start < 0.0 {
                (size as f64 + start).max(0.0) as usize
            } else {
                start.min(size as f64) as usize
            };
            let length = arg(args, 1);
            let length = if length.is_undefined() {
                size as f64
            } else {
                to_integer(&length, ops)?
            };
            let length = length.max(0.0).min((size - start) as f64);
            if length <= 0.0 {
                return Ok(Val::str(""));
            }
            ret_units(&u[start..start + length as usize])
        }
        "padStart" | "padEnd" => {
            let u = units();
            let max = to_integer(&arg(args, 0), ops)?.max(0.0);
            if max <= u.len() as f64 {
                return Ok(Val::str(s));
            }
            let fill = arg(args, 1);
            let fill = if fill.is_undefined() {
                utf16(" ")
            } else {
                utf16(&to_string(&fill, ops)?)
            };
            if fill.is_empty() {
                return Ok(Val::str(s));
            }
            let pad_len = max - u.len() as f64;
            if pad_len * 2.0 > ops.limit as f64 {
                return Err(too_big(ops));
            }
            let pad: Vec<u16> = fill
                .iter()
                .copied()
                .cycle()
                .take(pad_len as usize)
                .collect();
            let all = if name == "padStart" {
                [pad, u].concat()
            } else {
                [u, pad].concat()
            };
            ret_units(&all)
        }
        "repeat" => {
            let count = arg(args, 0);
            let n = to_number(&count, ops)?;
            let k = if n.is_nan() { 0.0 } else { n.trunc() };
            if k < 0.0 || k.is_infinite() {
                return Err(js_err(
                    "RangeError",
                    format!("Invalid count value: {}", js_number_to_string(n)),
                ));
            }
            if s.is_empty() || k == 0.0 {
                return Ok(Val::str(""));
            }
            if s.len() as f64 * k > ops.limit as f64 {
                return Err(too_big(ops));
            }
            Ok(Val::str(s.repeat(k as usize)))
        }
        "replace" | "replaceAll" => {
            let pattern = arg(args, 0);
            let replacement = arg(args, 1);
            if matches!(replacement, Val::Func(_)) {
                return Err(unsupported(format!(
                    "Brrrty doesn't run `{name}()` with a function as the replacement"
                )));
            }
            if let Val::Re(r) = &pattern {
                let global = r.lit.flags.contains('g');
                if name == "replaceAll" && !global {
                    return Err(type_error(
                        "String.prototype.replaceAll called with a non-global RegExp argument",
                    ));
                }
                let repl = to_string(&replacement, ops)?;
                let out = r
                    .lit
                    .matcher()
                    .map_err(unsupported)?
                    .replace(s, &repl)
                    .map_err(unsupported)?;
                if out.len() > ops.limit {
                    return Err(too_big(ops));
                }
                // A global replace starts and leaves `lastIndex` at 0.
                if global {
                    r.moved.set(false);
                }
                return Ok(Val::str(out));
            }
            let u = units();
            let needle = utf16(&to_string(&pattern, ops)?);
            let repl = utf16(&to_string(&replacement, ops)?);
            let mut positions = Vec::new();
            if name == "replace" {
                positions.extend(find(&u, &needle, 0));
            } else {
                let step = needle.len().max(1);
                let mut at = find(&u, &needle, 0);
                while let Some(p) = at {
                    positions.push(p);
                    at = find(&u, &needle, p + step);
                }
            }
            let mut out: Vec<u16> = Vec::new();
            let mut end = 0;
            for p in positions {
                out.extend_from_slice(&u[end..p]);
                substitute(&mut out, &u, p, needle.len(), &repl);
                end = p + needle.len();
                if out.len() * 2 > ops.limit {
                    return Err(too_big(ops));
                }
            }
            out.extend_from_slice(&u[end..]);
            ret_units(&out)
        }
        "split" => {
            let sep = arg(args, 0);
            let limit = arg(args, 1);
            let lim: u32 = if limit.is_undefined() {
                u32::MAX
            } else {
                to_uint32(to_number(&limit, ops)?)
            };
            if let Val::Re(r) = &sep {
                if lim == 0 {
                    return Ok(Val::arr(vec![]));
                }
                let pieces = r
                    .lit
                    .matcher()
                    .map_err(unsupported)?
                    .split(s)
                    .map_err(unsupported)?;
                return Ok(Val::arr(
                    pieces
                        .into_iter()
                        .take(lim as usize)
                        .map(|p| p.map_or_else(Val::undef, Val::str))
                        .collect(),
                ));
            }
            if sep.is_undefined() {
                return Ok(Val::arr(if lim == 0 { vec![] } else { vec![Val::str(s)] }));
            }
            let r = utf16(&to_string(&sep, ops)?);
            if lim == 0 {
                return Ok(Val::arr(vec![]));
            }
            let u = units();
            let lim = lim as usize;
            if r.is_empty() {
                let mut out = Vec::new();
                for unit in u.iter().take(lim) {
                    out.push(Val::str(from_utf16(std::slice::from_ref(unit))?));
                }
                return Ok(Val::arr(out));
            }
            if u.is_empty() {
                return Ok(Val::arr(vec![Val::str("")]));
            }
            let mut out = Vec::new();
            let mut i = 0;
            let mut at = find(&u, &r, 0);
            while let Some(j) = at {
                out.push(Val::str(from_utf16(&u[i..j])?));
                if out.len() == lim {
                    return Ok(Val::arr(out));
                }
                i = j + r.len();
                at = find(&u, &r, i);
            }
            out.push(Val::str(from_utf16(&u[i..])?));
            Ok(Val::arr(out))
        }
        _ => {
            let _ = len;
            object_method(this, name, args, ops)
        }
    }
}

/// `GetSubstitution` with no capture groups.
fn substitute(out: &mut Vec<u16>, s: &[u16], pos: usize, matched: usize, repl: &[u16]) {
    let dollar = u16::from(b'$');
    let mut i = 0;
    while i < repl.len() {
        let c = repl[i];
        if c == dollar && i + 1 < repl.len() {
            let n = repl[i + 1];
            if n == dollar {
                out.push(dollar);
                i += 2;
                continue;
            } else if n == u16::from(b'&') {
                out.extend_from_slice(&s[pos..pos + matched]);
                i += 2;
                continue;
            } else if n == u16::from(b'`') {
                out.extend_from_slice(&s[..pos]);
                i += 2;
                continue;
            } else if n == u16::from(b'\'') {
                out.extend_from_slice(&s[(pos + matched).min(s.len())..]);
                i += 2;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
}

fn to_uint32(n: f64) -> u32 {
    if !n.is_finite() {
        return 0;
    }
    n.trunc().rem_euclid(4_294_967_296.0) as u32
}

fn number_method(this: &Val, n: f64, name: &str, args: &[Val], ops: Ops) -> R<Val> {
    match name {
        "valueOf" => Ok(Val::num(n)),
        "toString" => {
            let radix = arg(args, 0);
            if radix.is_undefined() {
                return Ok(Val::str(js_number_to_string(n)));
            }
            let r = to_integer(&radix, ops)?;
            if !(2.0..=36.0).contains(&r) {
                return Err(js_err(
                    "RangeError",
                    "toString() radix must be between 2 and 36",
                ));
            }
            if r == 10.0 || !n.is_finite() {
                return Ok(Val::str(js_number_to_string(n)));
            }
            if n.fract() != 0.0 || n.abs() > 9_007_199_254_740_991.0 {
                return Err(unsupported(
                    "Brrrty runs `toString(radix)` only on whole numbers up to 2^53",
                ));
            }
            Ok(Val::str(radix_string(n, r as u32)))
        }
        "toFixed" => {
            let f = to_integer(&arg(args, 0), ops)?;
            if !(0.0..=100.0).contains(&f) {
                return Err(js_err(
                    "RangeError",
                    "toFixed() digits argument must be between 0 and 100",
                ));
            }
            Ok(Val::str(to_fixed(n, f as usize)))
        }
        "toLocaleString" => {
            english_only(name, args)?;
            Ok(Val::str(brrrty_js::js_to_locale_string(n)))
        }
        _ => object_method(this, name, args, ops),
    }
}

/// The arguments of a locale-dependent method (`toLocaleString`,
/// `localeCompare`): Brrrty runs them as Node does in `en-US`, so a locale
/// other than that (or `"en"`) and any options are refused.
fn english_only(name: &str, args: &[Val]) -> R<()> {
    let locale = arg(args, 0);
    let english = locale.is_undefined()
        || matches!(&locale, Val::V(Value::String(l)) if &**l == "en-US" || &**l == "en");
    if !english {
        return Err(unsupported(format!(
            "`{name}()` with a locale other than \"en-US\" isn't emulated"
        ))
        .with_help("Brrrty matches Node's en-US results only; format the value in a filter, or in the data"));
    }
    if !arg(args, 1).is_undefined() {
        return Err(unsupported(format!("`{name}()` with options isn't emulated"))
            .with_help("Brrrty matches Node's default en-US results only; format the value in a filter, or in the data"));
    }
    Ok(())
}

fn radix_string(n: f64, radix: u32) -> String {
    let mut x = n.abs() as u64;
    if x == 0 {
        return "0".into();
    }
    let mut digits = Vec::new();
    while x > 0 {
        let d = (x % u64::from(radix)) as u32;
        digits.push(std::char::from_digit(d, radix).unwrap_or('0'));
        x /= u64::from(radix);
    }
    if n < 0.0 {
        digits.push('-');
    }
    digits.iter().rev().collect()
}

/// `Number.prototype.toFixed(f)`, V8's: see [`brrrty_js::js_to_fixed`].
pub(crate) fn to_fixed(x: f64, f: usize) -> String {
    brrrty_js::js_to_fixed(x, f as u32).unwrap_or_default()
}

fn array_method(this: &Val, name: &str, args: &[Val], ops: Ops) -> R<Val> {
    // Changing methods work on the array in place; the rest read a copy.
    let reads = !matches!(
        name,
        "push" | "unshift" | "pop" | "shift" | "reverse" | "valueOf"
    );
    let items = if reads {
        this.array_items().unwrap_or_default()
    } else {
        Vec::new()
    };
    let len = match this {
        Val::Arr(a) => a.borrow().len(),
        Val::V(Value::Array(a)) => a.len(),
        _ => 0,
    };
    let own = || -> R<&Rc<std::cell::RefCell<Vec<Val>>>> {
        match this {
            Val::Arr(a) => Ok(a),
            _ => Err(unsupported(format!(
                "`{name}()` would change an array from the data"
            ))
            .with_help("Brrrty never changes the build's data; copy it first, for example with `.slice()`")),
        }
    };
    match name {
        "toString" => Ok(Val::str(to_string(this, ops)?)),
        "join" => {
            let sep = arg(args, 0);
            let sep = if sep.is_undefined() {
                ",".into()
            } else {
                to_string(&sep, ops)?
            };
            let mut seen = Vec::new();
            if let Val::Arr(a) = this {
                seen.push(Rc::as_ptr(a) as usize);
            }
            Ok(Val::str(join(&items, &sep, ops, &mut seen)?))
        }
        "at" => {
            let n = to_integer(&arg(args, 0), ops)?;
            let k = if n >= 0.0 { n } else { len as f64 + n };
            Ok(if k < 0.0 || k >= len as f64 {
                Val::undef()
            } else {
                items[k as usize].clone()
            })
        }
        "includes" | "indexOf" => {
            let target = arg(args, 0);
            let n = to_integer(&arg(args, 1), ops)?;
            let start = if n >= 0.0 {
                n.min(len as f64) as usize
            } else {
                (len as f64 + n).max(0.0) as usize
            };
            for (i, item) in items.iter().enumerate().skip(start) {
                let hit = if name == "includes" {
                    same_value_zero(item, &target)?
                } else {
                    strict_eq(item, &target)?
                };
                if hit {
                    return Ok(if name == "includes" {
                        Val::bool(true)
                    } else {
                        Val::num(i as f64)
                    });
                }
            }
            Ok(if name == "includes" {
                Val::bool(false)
            } else {
                Val::num(-1.0)
            })
        }
        "lastIndexOf" => {
            if len == 0 {
                return Ok(Val::num(-1.0));
            }
            let target = arg(args, 0);
            let n = if args.len() > 1 {
                to_integer(&args[1], ops)?
            } else {
                len as f64 - 1.0
            };
            let k = if n >= 0.0 {
                n.min(len as f64 - 1.0)
            } else {
                len as f64 + n
            };
            if k < 0.0 {
                return Ok(Val::num(-1.0));
            }
            for i in (0..=k as usize).rev() {
                if strict_eq(&items[i], &target)? {
                    return Ok(Val::num(i as f64));
                }
            }
            Ok(Val::num(-1.0))
        }
        "slice" => {
            let from = relative(to_integer(&arg(args, 0), ops)?, len);
            let end = arg(args, 1);
            let to = if end.is_undefined() {
                len
            } else {
                relative(to_integer(&end, ops)?, len)
            };
            Ok(Val::arr(if from < to {
                items[from..to].to_vec()
            } else {
                vec![]
            }))
        }
        "concat" => {
            // The size is checked before anything is copied, so a doubling
            // array stops at the limit instead of several times past it.
            let total = args.iter().fold(items.len(), |n, a| {
                n.saturating_add(a.array_len().unwrap_or(1))
            });
            check_len(total, ops)?;
            let mut out = items;
            out.reserve_exact(total - out.len());
            for a in args {
                match a.array_items() {
                    Some(more) => out.extend(more),
                    None => out.push(a.clone()),
                }
                check_len(out.len(), ops)?;
            }
            Ok(Val::arr(out))
        }
        "toReversed" => {
            let mut out = items;
            out.reverse();
            Ok(Val::arr(out))
        }
        "push" | "unshift" => {
            let a = own()?;
            check_len(len + args.len(), ops)?;
            if name == "push" {
                a.borrow_mut().extend(args.iter().cloned());
            } else {
                let mut v = a.borrow_mut();
                let rest = std::mem::take(&mut *v);
                v.extend(args.iter().cloned());
                v.extend(rest);
            }
            crate::render::note_mutation(this);
            Ok(Val::num(a.borrow().len() as f64))
        }
        "pop" | "shift" => {
            let a = own()?;
            let mut v = a.borrow_mut();
            Ok(if v.is_empty() {
                Val::undef()
            } else if name == "pop" {
                v.pop().unwrap_or_else(Val::undef)
            } else {
                v.remove(0)
            })
        }
        "reverse" => {
            own()?.borrow_mut().reverse();
            Ok(this.clone())
        }
        "valueOf" => Ok(this.clone()),
        _ => object_method(this, name, args, ops),
    }
}

fn check_len(len: usize, ops: Ops) -> R<()> {
    if len.saturating_mul(std::mem::size_of::<Val>()) > ops.limit {
        return Err(too_big(ops));
    }
    Ok(())
}

fn date_method(this: &Val, ms: f64, name: &str, ops: Ops) -> R<Val> {
    use brrrty_js::tz::fields;
    let local = || fields(ops.tz, ms);
    let utc = || fields(Tz::Utc, ms);
    let num = |f: Option<brrrty_js::tz::Fields>, g: fn(&brrrty_js::tz::Fields) -> f64| {
        Ok(Val::num(f.as_ref().map_or(f64::NAN, g)))
    };
    match name {
        "getTime" | "valueOf" => Ok(Val::num(ms)),
        "getFullYear" => num(local(), |f| f.year as f64),
        "getYear" => num(local(), |f| f.year as f64 - 1900.0),
        "getMonth" => num(local(), |f| f64::from(f.month0)),
        "getDate" => num(local(), |f| f64::from(f.day)),
        "getDay" => num(local(), |f| f64::from(f.weekday)),
        "getHours" => num(local(), |f| f64::from(f.hour)),
        "getMinutes" => num(local(), |f| f64::from(f.minute)),
        "getSeconds" => num(local(), |f| f64::from(f.second)),
        "getMilliseconds" => num(local(), |f| f64::from(f.millis)),
        "getTimezoneOffset" => num(local(), |f| f.offset_west),
        "getUTCFullYear" => num(utc(), |f| f.year as f64),
        "getUTCMonth" => num(utc(), |f| f64::from(f.month0)),
        "getUTCDate" => num(utc(), |f| f64::from(f.day)),
        "getUTCDay" => num(utc(), |f| f64::from(f.weekday)),
        "getUTCHours" => num(utc(), |f| f64::from(f.hour)),
        "getUTCMinutes" => num(utc(), |f| f64::from(f.minute)),
        "getUTCSeconds" => num(utc(), |f| f64::from(f.second)),
        "getUTCMilliseconds" => num(utc(), |f| f64::from(f.millis)),
        "toString" => Ok(Val::str(brrrty_js::tz::date_to_string(ops.tz, ms))),
        "toISOString" | "toJSON" => brrrty_js::tz::iso_string(ms)
            .map(Val::str)
            .ok_or_else(|| js_err("RangeError", "Invalid time value")),
        _ if name.starts_with("set") => Err(unsupported(format!(
            "`{name}()` would change a date from the data"
        ))
        .with_help("Brrrty never changes the build's data")),
        _ => match name {
            "hasOwnProperty" | "propertyIsEnumerable" => object_method(this, name, &[], ops),
            _ => Err(no_method(this, name)),
        },
    }
}

/// Wraps an object's own properties into a fresh object, for `{a: 1}`.
pub(crate) fn new_object(pairs: Vec<(String, Val)>) -> Val {
    let mut props = indexmap::IndexMap::new();
    for (k, v) in pairs {
        props.insert(k, v);
    }
    Val::Obj(Obj::new(props))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_matches_v8() {
        // node: [0.5,1.5,2.5,1.25,1.005,-1.5,-0.001,123.456,1e21,0,-0].map(...)
        assert_eq!(to_fixed(0.5, 0), "1");
        assert_eq!(to_fixed(1.5, 0), "2");
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(1.25, 1), "1.3");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(-1.5, 0), "-2");
        assert_eq!(to_fixed(-0.001, 2), "-0.00");
        assert_eq!(to_fixed(123.456, 2), "123.46");
        assert_eq!(to_fixed(1e21, 2), "1e+21");
        assert_eq!(to_fixed(0.0, 2), "0.00");
        assert_eq!(to_fixed(-0.0, 2), "0.00");
        assert_eq!(to_fixed(5.4321, 3), "5.432");
        assert_eq!(to_fixed(9.995, 2), "9.99");
        assert_eq!(to_fixed(99.5, 0), "100");
    }

    #[test]
    fn escape_matches_nunjucks() {
        assert_eq!(
            escape(r#"<a href="x">&'\</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;&#92;&lt;/a&gt;"
        );
    }
}
