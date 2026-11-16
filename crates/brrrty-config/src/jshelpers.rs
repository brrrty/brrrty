//! Helpers a port author calls where Rhai's operators differ from
//! JavaScript's. Rhai's own operators are not changed (see PORTING.md).

use std::cmp::Ordering;

use brrrty_js::{Tz, Value};
use rhai::{Array, Dynamic, Engine, EvalAltResult, FLOAT, FnPtr, NativeCallContext};

use crate::api::invalid;
use crate::convert::to_value;

type Res<T> = Result<T, Box<EvalAltResult>>;

fn val(v: &Dynamic, what: &str) -> Res<Value> {
    to_value(v).map_err(|why| invalid(format!("{what}: {why}"), rhai::Position::NONE))
}

/// JavaScript's `String(v)`.
fn js_string(v: &Dynamic) -> Res<String> {
    Ok(val(v, "js_string")?.to_js_string_in(Tz::Local))
}

/// JavaScript's `JSON.stringify(v)`: a string, or `()` for undefined.
fn json_stringify(v: &Dynamic) -> Res<Dynamic> {
    let value = val(v, "json_stringify")?;
    if value == Value::Undefined {
        return Ok(Dynamic::UNIT);
    }
    brrrty_liquid::json_stringify(&value)
        .map(Dynamic::from)
        .ok_or_else(|| {
            invalid(
                "json_stringify: a collection item can't be turned into JSON whole; pick the fields you need".into(),
                rhai::Position::NONE,
            )
        })
}

/// JavaScript's `a + b`: a string when either side is a string, array,
/// object or date, else the sum of the two as numbers.
fn js_concat(a: &Dynamic, b: &Dynamic) -> Res<Dynamic> {
    let (x, y) = (val(a, "js_concat")?, val(b, "js_concat")?);
    let texty = |v: &Value| {
        matches!(
            v,
            Value::String(_) | Value::Array(_) | Value::Object(_) | Value::Date(_)
        )
    };
    if texty(&x) || texty(&y) {
        Ok(Dynamic::from(format!(
            "{}{}",
            x.to_js_string_in(Tz::Local),
            y.to_js_string_in(Tz::Local)
        )))
    } else {
        Ok(Dynamic::from_float(x.to_js_number() + y.to_js_number()))
    }
}

/// `undefined` items go last and are never compared, as in JavaScript.
fn sort_with(arr: Array, cmp: &mut dyn FnMut(&Dynamic, &Dynamic) -> Res<Ordering>) -> Res<Array> {
    let (undefined, defined): (Vec<_>, Vec<_>) = arr.into_iter().partition(Dynamic::is_unit);
    let mut sorted = defined;
    crate::timsort::sort(&mut sorted, cmp)?;
    sorted.extend(undefined);
    Ok(sorted)
}

fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `arr.sort()`: by `String(item)`, in UTF-16 order. Returns a new array.
fn js_sort(arr: Array) -> Res<Array> {
    sort_with(arr, &mut |a, b| {
        Ok(utf16_cmp(&js_string(a)?, &js_string(b)?))
    })
}

/// `arr.sort(fn)`: the comparator's result is a number in JavaScript's
/// sense (negative, zero, positive, any float; `NaN` is zero; `true` is 1).
fn js_sort_by(ctx: NativeCallContext, arr: Array, f: FnPtr) -> Res<Array> {
    sort_with(arr, &mut |a, b| {
        let r: Dynamic = f.call_within_context(&ctx, (a.clone(), b.clone()))?;
        let n = val(&r, "the comparator's result")?.to_js_number();
        Ok(if n > 0.0 {
            Ordering::Greater
        } else if n < 0.0 {
            Ordering::Less
        } else {
            Ordering::Equal
        })
    })
}

fn float(v: &Dynamic, what: &str) -> Res<FLOAT> {
    Ok(val(v, what)?.to_js_number())
}

/// `Math.round`: halves round up (`-4.5` is `-4`), and `-0` stays `-0`.
fn js_round(x: FLOAT) -> FLOAT {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    let r = if x - f >= 0.5 { f + 1.0 } else { f };
    if r == 0.0 && (x < 0.0 || (x == 0.0 && x.is_sign_negative())) {
        -0.0
    } else {
        r
    }
}

pub fn register(e: &mut Engine) {
    e.register_fn("js_string", |v: Dynamic| js_string(&v));
    e.register_fn("json_stringify", |v: Dynamic| json_stringify(&v));
    e.register_fn("js_concat", |a: Dynamic, b: Dynamic| js_concat(&a, &b));
    e.register_fn("js_sort", js_sort);
    e.register_fn("js_sort_by", js_sort_by);
    // `arr.sort()` and `arr.sort(fn)` are JavaScript's, in place and
    // returning the array, instead of Rhai's: Rhai's skips a comparator
    // that fails and wants an integer result, so a port could sort
    // differently from JavaScript without a word.
    e.register_fn(
        "sort",
        |ctx: NativeCallContext, arr: &mut Array, f: FnPtr| -> Res<Array> {
            let sorted = js_sort_by(ctx, arr.clone(), f)?;
            *arr = sorted.clone();
            Ok(sorted)
        },
    );
    e.register_fn("sort", |arr: &mut Array| -> Res<Array> {
        let sorted = js_sort(arr.clone())?;
        *arr = sorted.clone();
        Ok(sorted)
    });
    e.register_fn("js_div", |a: Dynamic, b: Dynamic| -> Res<FLOAT> {
        Ok(float(&a, "js_div")? / float(&b, "js_div")?)
    });
    e.register_fn("js_round", |x: Dynamic| -> Res<FLOAT> {
        Ok(js_round(float(&x, "js_round")?))
    });
}
