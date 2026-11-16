//! The JavaScript the built-in filters lean on: `nunjucks/src/lib.js`
//! (`map`, `keys`, `getAttrGetter`, `repeat`), `copySafeness`, method calls
//! with V8's error messages, and `JSON.stringify`.

use std::rc::Rc;

use brrrty_js::{Value, js_number_to_string};

use crate::jsops::{self, BinOp, Ops, Prop};
use crate::render::{R, grow, too_many};
use crate::value::{Obj, Val};

pub(super) fn arg(args: &[Val], i: usize) -> Val {
    args.get(i).cloned().unwrap_or_else(Val::undef)
}

pub(super) fn is_string(v: &Val) -> bool {
    matches!(v, Val::V(Value::String(_)))
}

pub(super) fn text(v: &Val) -> Option<&str> {
    match v {
        Val::V(Value::String(s)) => Some(s),
        Val::Safe(s) => Some(&s.0),
        _ => None,
    }
}

/// `value === null || value === undefined || value === false ? default :
/// value`.
pub(super) fn normalize(v: Val, default: &str) -> Val {
    if v.is_nullish() || matches!(v, Val::V(Value::Bool(false))) {
        Val::str(default)
    } else {
        v
    }
}

/// `runtime.copySafeness(dest, target)` for a string `target`.
pub(super) fn copy_safeness(dest: &Val, target: String) -> Val {
    if matches!(dest, Val::Safe(_)) {
        Val::safe(target)
    } else {
        Val::str(target)
    }
}

pub(super) fn fit(s: String, ops: Ops) -> R<String> {
    if s.len() > ops.limit {
        Err(jsops::too_big(ops))
    } else {
        Ok(s)
    }
}

/// What V8 says when `recv.name` is read or called and can't be.
pub(super) fn need_method(recv: &Val, expr: &str, name: &str) -> R<()> {
    if recv.is_nullish() {
        let what = if recv.is_undefined() {
            "undefined"
        } else {
            "null"
        };
        return Err(jsops::type_error(format!(
            "Cannot read properties of {what} (reading '{name}')"
        )));
    }
    match jsops::get_property(recv, name)? {
        Prop::Function => Ok(()),
        Prop::Value(_) => Err(jsops::type_error(format!(
            "{expr}.{name} is not a function"
        ))),
    }
}

/// `recv.name(...args)`, written in the filter as `expr.name(...)`.
pub(super) fn call(recv: &Val, expr: &str, name: &str, args: &[Val], ops: Ops) -> R<Val> {
    need_method(recv, expr, name)?;
    jsops::call_method(recv, name, args, ops)
}

/// The text of a string or `SafeString` after `need_method`; anything else
/// would have had its own method, which Brrrty doesn't run.
pub(super) fn string_receiver<'a>(recv: &'a Val, name: &str) -> R<&'a str> {
    text(recv).ok_or_else(|| {
        jsops::unsupported(format!(
            "Brrrty doesn't run the JavaScript method `{name}()` on this value"
        ))
    })
}

/// `obj[key]` for a filter's own code (no `memberLookup` wrapper).
pub(super) fn get(obj: &Val, key: &Val, ops: Ops) -> R<Val> {
    jsops::raw_index(obj, key, ops)
}

pub(super) fn get_str(obj: &Val, key: &str, ops: Ops) -> R<Val> {
    jsops::raw_index(obj, &Val::str(key), ops)
}

pub(super) fn lt(a: &Val, b: &Val, ops: Ops) -> R<bool> {
    Ok(jsops::binary(BinOp::Lt, a, b, ops)?.truthy())
}

pub(super) fn is_zero(v: &Val) -> bool {
    matches!(v, Val::V(Value::Number(n)) if *n == 0.0)
}

/// How many times `for (i = 0; i < n; i++)` runs.
pub(super) fn loop_count(n: &Val, ops: Ops) -> R<usize> {
    let n = jsops::to_number(n, ops)?;
    count_f(n, ops)
}

pub(super) fn count_f(n: f64, ops: Ops) -> R<usize> {
    if n.is_nan() || n <= 0.0 {
        return Ok(0);
    }
    let c = n.ceil();
    if c > ops.limit as f64 {
        return Err(jsops::too_big(ops));
    }
    Ok(c as usize)
}

/// `lib.repeat(' ', n)`.
pub(super) fn spaces(n: &Val, ops: Ops) -> R<String> {
    Ok(" ".repeat(loop_count(n, ops)?))
}

pub(super) fn too_long(what: &str, limit: usize) -> crate::render::RenderErr {
    too_many(format!("{what} would have more than {limit} items"))
}

/// `lib.map(obj, f)`.
pub(super) fn lib_map(
    obj: &Val,
    ops: Ops,
    item_limit: usize,
    mut f: impl FnMut(Val) -> R<Val>,
) -> R<Vec<Val>> {
    if obj.is_nullish() {
        return Ok(Vec::new());
    }
    if let Some(items) = obj.array_items() {
        return items.into_iter().map(f).collect();
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    loop {
        let len = get_str(obj, "length", ops)?;
        if !lt(&Val::num(i as f64), &len, ops)? {
            break;
        }
        if out.len() >= item_limit {
            return Err(too_long("this list", item_limit));
        }
        out.push(f(get(obj, &Val::num(i as f64), ops)?)?);
        i += 1;
    }
    let len = get_str(obj, "length", ops)?;
    if let Val::V(Value::Number(n)) = len
        && n != out.len() as f64
        && !n.is_nan()
    {
        return Err(jsops::unsupported(
            "a list whose length isn't a whole number can't be mapped here",
        ));
    }
    Ok(out)
}

/// `Array.prototype.slice.call(obj)`, with the holes left out (every use
/// goes on to `filter`).
pub(super) fn to_array(obj: &Val, ops: Ops, item_limit: usize) -> R<Vec<Val>> {
    if obj.is_nullish() {
        return Err(jsops::type_error(
            "Cannot convert undefined or null to object",
        ));
    }
    if let Some(items) = obj.array_items() {
        return Ok(items);
    }
    let len = get_str(obj, "length", ops)?;
    let n = jsops::to_integer(&len, ops)?.max(0.0);
    if n > item_limit as f64 {
        return Err(too_long("this list", item_limit));
    }
    let mut out = Vec::new();
    for i in 0..n as usize {
        if jsops::has_own(obj, &i.to_string())? {
            out.push(get(obj, &Val::num(i as f64), ops)?);
        }
    }
    Ok(out)
}

/// Own enumerable properties, in `Object.keys` order, of what `lib.keys`
/// and `for ... in` see. A `SafeString` has `val` and `length`.
pub(super) fn entries(v: &Val) -> Vec<(String, Val)> {
    match v {
        Val::Obj(o) => o
            .keys()
            .into_iter()
            .map(|k| {
                let val = o.get(&k).unwrap_or_else(Val::undef);
                (k, val)
            })
            .collect(),
        // A listing of a collection item, or of the open `collections`,
        // is a trap, as `for k, v in` is; each value is read as a property
        // read reads it.
        Val::V(Value::Object(o)) => Value::js_entries(o)
            .into_iter()
            .map(|(k, v)| {
                let val =
                    brrrty_js::traps::read(o, k, Some(v.clone())).map_or_else(Val::undef, Val::V);
                (k.clone(), val)
            })
            .collect(),
        Val::Safe(s) => vec![
            ("val".to_string(), Val::str(s.0.clone())),
            (
                "length".to_string(),
                Val::num(s.0.encode_utf16().count() as f64),
            ),
        ],
        _ => Vec::new(),
    }
}

/// `Object.prototype.toString.call(v) === '[object Object]'`, which is
/// what `lib.isObject` asks. A `SafeString` counts.
pub(super) fn is_object(v: &Val) -> bool {
    matches!(v, Val::Obj(_) | Val::Safe(_) | Val::V(Value::Object(_)))
}

/// `lib.getAttrGetter(attr)`.
pub(super) struct AttrGetter(Vec<Val>);

impl AttrGetter {
    pub fn new(attr: &Val, ops: Ops) -> R<AttrGetter> {
        if !attr.truthy() {
            return Ok(AttrGetter(Vec::new()));
        }
        if let Some(s) = attr.as_str() {
            return Ok(AttrGetter(s.split('.').map(Val::str).collect()));
        }
        let _ = ops;
        Ok(AttrGetter(vec![attr.clone()]))
    }

    pub fn get(&self, item: &Val, ops: Ops) -> R<Val> {
        let mut cur = item.clone();
        for part in &self.0 {
            if cur.is_nullish() {
                return Err(jsops::type_error(
                    "Cannot convert undefined or null to object",
                ));
            }
            let key = jsops::property_key(part, ops)?;
            if jsops::has_own(&cur, &key)? {
                cur = get(&cur, part, ops)?;
            } else {
                return Ok(Val::undef());
            }
        }
        Ok(cur)
    }
}

/// `makeMacro(argNames, [], func)`'s argument shuffling for a filter whose
/// named arguments are `names`: the arguments `func` gets.
pub(super) fn macro_args_named(argv: &[Val], names: &[&str]) -> Vec<Val> {
    let n = names.len();
    let kw: Option<Rc<Obj>> = match argv.last() {
        Some(Val::Obj(o)) if o.has("__keywords") => Some(o.clone()),
        _ => None,
    };
    let count = argv.len() - usize::from(kw.is_some());
    if count > n {
        argv[..n].to_vec()
    } else if count < n {
        let mut args = argv[..count].to_vec();
        for name in &names[count..] {
            args.push(
                kw.as_ref()
                    .and_then(|o| o.get(name))
                    .unwrap_or_else(Val::undef),
            );
        }
        args
    } else {
        argv[..n].to_vec()
    }
}

// ---- JSON.stringify ----

/// `JSON.stringify(v, null, space)`: `None` is `undefined`.
pub(super) fn json_stringify(v: &Val, space: &Val, ops: Ops) -> R<Option<String>> {
    let gap = match space {
        Val::V(Value::Number(n)) => {
            let k = if n.is_nan() { 0.0 } else { n.trunc() }.min(10.0);
            " ".repeat(if k >= 1.0 { k as usize } else { 0 })
        }
        Val::V(Value::String(s)) => {
            let units: Vec<u16> = s.encode_utf16().take(10).collect();
            jsops::from_utf16(&units)?
        }
        _ => String::new(),
    };
    let mut seen = Vec::new();
    let mut out = String::new();
    if json_value(v, &gap, "", &mut seen, &mut out, ops)? {
        Ok(Some(out))
    } else {
        Ok(None)
    }
}

pub(super) fn json_quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Writes `v` if it has a JSON form. Returns whether it did.
fn json_value(
    v: &Val,
    gap: &str,
    indent: &str,
    seen: &mut Vec<usize>,
    out: &mut String,
    ops: Ops,
) -> R<bool> {
    if out.len() > ops.limit {
        return Err(jsops::too_big(ops));
    }
    match v {
        Val::V(Value::Undefined) | Val::Func(_) => Ok(false),
        // No own enumerable properties: `lastIndex` isn't enumerable.
        Val::Re(_) => {
            out.push_str("{}");
            Ok(true)
        }
        Val::V(Value::Null) => {
            out.push_str("null");
            Ok(true)
        }
        Val::V(Value::Bool(b)) => {
            out.push_str(if *b { "true" } else { "false" });
            Ok(true)
        }
        Val::V(Value::Number(n)) => {
            out.push_str(&if n.is_finite() {
                js_number_to_string(*n)
            } else {
                "null".to_string()
            });
            Ok(true)
        }
        Val::V(Value::String(s)) => {
            json_quote(s, out);
            Ok(true)
        }
        Val::V(Value::Date(_)) => {
            let iso = jsops::call_method(v, "toJSON", &[], ops)?;
            match iso.as_str() {
                Some(s) => json_quote(s, out),
                None => out.push_str("null"),
            }
            Ok(true)
        }
        Val::V(Value::Array(_)) | Val::Arr(_) => {
            let items = v.array_items().unwrap_or_default();
            let id = match v {
                Val::Arr(a) => Rc::as_ptr(a) as usize,
                _ => 0,
            };
            guard(id, seen)?;
            let inner = format!("{indent}{gap}");
            if items.is_empty() {
                out.push_str("[]");
            } else {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    if !gap.is_empty() {
                        out.push('\n');
                        out.push_str(&inner);
                    }
                    if !grow(|| json_value(item, gap, &inner, seen, out, ops))? {
                        out.push_str("null");
                    }
                }
                if !gap.is_empty() {
                    out.push('\n');
                    out.push_str(indent);
                }
                out.push(']');
            }
            unguard(id, seen);
            Ok(true)
        }
        Val::V(Value::Object(_)) | Val::Obj(_) | Val::Safe(_) => {
            if let Val::Obj(o) = v
                && matches!(o.get("toJSON"), Some(Val::Func(_)))
            {
                return Err(jsops::unsupported(
                    "Brrrty doesn't call a `toJSON` function in `dump`",
                ));
            }
            let id = match v {
                Val::Obj(o) => Rc::as_ptr(o) as usize,
                _ => 0,
            };
            guard(id, seen)?;
            let inner = format!("{indent}{gap}");
            let mut first = true;
            out.push('{');
            for (k, val) in entries(v) {
                let mark = out.len();
                if !first {
                    out.push(',');
                }
                if !gap.is_empty() {
                    out.push('\n');
                    out.push_str(&inner);
                }
                json_quote(&k, out);
                out.push(':');
                if !gap.is_empty() {
                    out.push(' ');
                }
                if grow(|| json_value(&val, gap, &inner, seen, out, ops))? {
                    first = false;
                } else {
                    out.truncate(mark);
                }
            }
            if !first && !gap.is_empty() {
                out.push('\n');
                out.push_str(indent);
            }
            out.push('}');
            unguard(id, seen);
            Ok(true)
        }
    }
}

fn guard(id: usize, seen: &mut Vec<usize>) -> R<()> {
    if id != 0 {
        if seen.contains(&id) {
            return Err(jsops::unsupported(
                "`dump` of a value that contains itself (Nunjucks says \"Converting circular structure to JSON\")",
            ));
        }
        seen.push(id);
    }
    Ok(())
}

fn unguard(id: usize, seen: &mut Vec<usize>) {
    if id != 0 {
        seen.pop();
    }
}

// ---- parseFloat, parseInt, encodeURIComponent ----

/// `parseFloat(s)`.
pub(super) fn parse_float(s: &str) -> f64 {
    let t = s.trim_start_matches(brrrty_js::is_js_whitespace);
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let sign = if neg { -1.0 } else { 1.0 };
    if rest.starts_with("Infinity") {
        return sign * f64::INFINITY;
    }
    let b = rest.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - start
    };
    let int_digits = digits(&mut i);
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        let mut j = i + 1;
        frac_digits = digits(&mut j);
        if frac_digits > 0 || int_digits > 0 {
            i = j;
        }
    }
    if int_digits == 0 && frac_digits == 0 {
        return f64::NAN;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if digits(&mut j) > 0 {
            i = j;
        }
    }
    let mut lit = rest[..i].to_string();
    if lit.starts_with('.') {
        lit.insert(0, '0');
    }
    let lit = lit.replace(".e", ".0e").replace(".E", ".0E");
    let lit = if lit.ends_with('.') {
        format!("{lit}0")
    } else {
        lit
    };
    sign * lit.parse::<f64>().unwrap_or(f64::NAN)
}

/// `parseInt(s, radix)` where `radix` is already `ToInt32`.
pub(super) fn parse_int(s: &str, radix: i32) -> R<f64> {
    let t = s.trim_start_matches(brrrty_js::is_js_whitespace);
    let (neg, mut rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut r = radix;
    let mut strip = true;
    if r != 0 {
        if !(2..=36).contains(&r) {
            return Ok(f64::NAN);
        }
        if r != 16 {
            strip = false;
        }
    } else {
        r = 10;
    }
    if strip && (rest.starts_with("0x") || rest.starts_with("0X")) {
        rest = &rest[2..];
        r = 16;
    }
    let r = r as u32;
    let digits: Vec<u32> = rest.chars().map_while(|c| c.to_digit(r)).collect();
    // `to_digit` takes only ASCII digits and letters.
    if digits.is_empty() {
        return Ok(f64::NAN);
    }
    let sign = if neg { -1.0 } else { 1.0 };
    if r == 10 {
        let lit: String = digits.iter().map(|d| char::from(b'0' + *d as u8)).collect();
        return Ok(sign * lit.parse::<f64>().unwrap_or(f64::NAN));
    }
    let mut acc: u128 = 0;
    for d in &digits {
        acc = match acc
            .checked_mul(u128::from(r))
            .and_then(|a| a.checked_add(u128::from(*d)))
        {
            Some(a) => a,
            None => {
                return Err(jsops::unsupported(
                    "`int` of a digit string too long for Brrrty to convert exactly",
                ));
            }
        };
    }
    if !r.is_power_of_two() && acc > (1u128 << 53) {
        return Err(jsops::unsupported(
            "`int` of a large number in a base that isn't 2, 4, 8, 10, 16 or 32",
        ));
    }
    Ok(sign * acc as f64)
}

/// `encodeURIComponent`.
pub(super) fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
        {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
