//! Helpers a port author calls where Rhai's operators differ from
//! JavaScript's. Rhai's own operators are not changed (see PORTING.md).

use std::cmp::Ordering;

use brrrty_js::{Tz, Value, tz};
use rhai::{Array, Dynamic, Engine, EvalAltResult, FLOAT, FnPtr, NativeCallContext};

use crate::api::{JsDate, invalid};
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

fn err<T>(msg: String) -> Res<T> {
    Err(invalid(msg, rhai::Position::NONE))
}

/// A number for a method only numbers have in JavaScript (`toFixed`,
/// `toLocaleString`): an integer or a float, nothing converted.
fn number_this(v: &Dynamic, what: &str) -> Res<FLOAT> {
    if let Some(i) = v.clone().try_cast::<rhai::INT>() {
        return Ok(i as FLOAT);
    }
    if let Some(f) = v.clone().try_cast::<FLOAT>() {
        return Ok(f);
    }
    err(format!(
        "{what}: a {} isn't a number; in JavaScript only numbers have this method. Convert it first, as the JavaScript does with Number(x) or parseFloat(x)",
        v.type_name()
    ))
}

/// `ToIntegerOrInfinity`: `NaN` is 0, fractions are dropped.
fn to_integer(n: FLOAT) -> FLOAT {
    if n.is_nan() { 0.0 } else { n.trunc() }
}

/// `x.toFixed(digits)`.
fn to_fixed(x: &Dynamic, digits: &Dynamic) -> Res<String> {
    let n = number_this(x, "to_fixed")?;
    let d = if digits.is_unit() {
        0.0
    } else {
        to_integer(float(digits, "to_fixed")?)
    };
    if !(0.0..=100.0).contains(&d) {
        return err("to_fixed: toFixed() digits argument must be between 0 and 100 (a RangeError in JavaScript)".into());
    }
    brrrty_js::js_to_fixed(n, d as u32)
        .map_err(|why| invalid(format!("to_fixed: {why}"), rhai::Position::NONE))
}

/// The locales the English helpers take: none, `"en-US"` or `"en"`.
fn english(locale: &Dynamic, what: &str) -> Res<()> {
    if locale.is_unit() {
        return Ok(());
    }
    match locale.clone().into_immutable_string() {
        Ok(s) if s == "en-US" || s == "en" => Ok(()),
        Ok(s) => err(format!(
            "{what}: the locale \"{s}\" isn't ported; Brrrty matches Node only for \"en-US\" (and \"en\", the same here)"
        )),
        Err(t) => err(format!(
            "{what}: the locale must be a string like \"en-US\", not a {t}"
        )),
    }
}

/// `x.toLocaleString('en-US')`.
fn to_locale_string(x: &Dynamic, locale: &Dynamic) -> Res<String> {
    english(locale, "to_locale_string")?;
    Ok(brrrty_js::js_to_locale_string(number_this(
        x,
        "to_locale_string",
    )?))
}

/// `a.localeCompare(b)` in an English locale: -1, 0 or 1.
fn locale_compare(a: &Dynamic, b: &Dynamic, locale: &Dynamic) -> Res<rhai::INT> {
    english(locale, "locale_compare")?;
    let Ok(a) = a.clone().into_immutable_string() else {
        return err(format!(
            "locale_compare: a {} isn't a string; in JavaScript only strings have localeCompare",
            a.type_name()
        ));
    };
    let b = js_string(b)?;
    match brrrty_js::js_locale_compare(&a, &b) {
        Ok(o) => Ok(o as rhai::INT),
        Err(c) => err(format!(
            "locale_compare: U+{:04X} is new in Unicode 18, which Node's ICU doesn't have yet, so its order there can't be matched",
            c as u32
        )),
    }
}

/// `new Date(y, m, ...)` in the local zone.
fn local_date(args: &[Dynamic]) -> Res<JsDate> {
    let mut n = Vec::with_capacity(args.len());
    for a in args {
        n.push(float(a, "date")?);
    }
    Ok(JsDate(tz::make_date(Tz::Local, &n)))
}

fn register_dates(e: &mut Engine) {
    e.register_fn("date", |y: Dynamic, m: Dynamic| local_date(&[y, m]));
    e.register_fn("date", |y: Dynamic, m: Dynamic, d: Dynamic| {
        local_date(&[y, m, d])
    });
    e.register_fn("date", |y: Dynamic, m: Dynamic, d: Dynamic, h: Dynamic| {
        local_date(&[y, m, d, h])
    });
    e.register_fn(
        "date",
        |y: Dynamic, m: Dynamic, d: Dynamic, h: Dynamic, mi: Dynamic| local_date(&[y, m, d, h, mi]),
    );
    e.register_fn(
        "date",
        |y: Dynamic, m: Dynamic, d: Dynamic, h: Dynamic, mi: Dynamic, s: Dynamic| {
            local_date(&[y, m, d, h, mi, s])
        },
    );
    e.register_fn(
        "date",
        |y: Dynamic, m: Dynamic, d: Dynamic, h: Dynamic, mi: Dynamic, s: Dynamic, ms: Dynamic| {
            local_date(&[y, m, d, h, mi, s, ms])
        },
    );
    type Getter = fn(&tz::Fields) -> FLOAT;
    let getters: [(&str, Getter); 9] = [
        ("get_full_year", |f| f.year as FLOAT),
        ("get_month", |f| FLOAT::from(f.month0)),
        ("get_date", |f| FLOAT::from(f.day)),
        ("get_day", |f| FLOAT::from(f.weekday)),
        ("get_hours", |f| FLOAT::from(f.hour)),
        ("get_minutes", |f| FLOAT::from(f.minute)),
        ("get_seconds", |f| FLOAT::from(f.second)),
        ("get_milliseconds", |f| FLOAT::from(f.millis)),
        ("get_timezone_offset", |f| f.offset_west),
    ];
    for (name, get) in getters {
        e.register_fn(name, move |d: &mut JsDate| {
            tz::fields(Tz::Local, d.0).map_or(FLOAT::NAN, |f| get(&f))
        });
    }
    e.register_fn("to_iso_string", |d: &mut JsDate| -> Res<String> {
        tz::iso_string(d.0).map_or_else(
            || err("to_iso_string: Invalid time value (a RangeError in JavaScript)".into()),
            Ok,
        )
    });
}

pub fn register(e: &mut Engine) {
    register_dates(e);
    e.register_fn("to_fixed", |x: Dynamic| to_fixed(&x, &Dynamic::UNIT));
    e.register_fn("to_fixed", |x: Dynamic, d: Dynamic| to_fixed(&x, &d));
    e.register_fn("to_locale_string", |x: Dynamic| {
        to_locale_string(&x, &Dynamic::UNIT)
    });
    e.register_fn("to_locale_string", |x: Dynamic, l: Dynamic| {
        to_locale_string(&x, &l)
    });
    e.register_fn(
        "to_locale_string",
        |_: Dynamic, _: Dynamic, _: Dynamic| -> Res<String> {
            err("to_locale_string: options aren't ported; Brrrty matches only the default format (up to 3 decimals, grouped by thousands)".into())
        },
    );
    e.register_fn("locale_compare", |a: Dynamic, b: Dynamic| {
        locale_compare(&a, &b, &Dynamic::UNIT)
    });
    e.register_fn("locale_compare", |a: Dynamic, b: Dynamic, l: Dynamic| {
        locale_compare(&a, &b, &l)
    });
    e.register_fn(
        "locale_compare",
        |_: Dynamic, _: Dynamic, _: Dynamic, _: Dynamic| -> Res<rhai::INT> {
            err(
                "locale_compare: options aren't ported; Brrrty matches only the default order"
                    .into(),
            )
        },
    );
    // Math.random() can't give 11ty's output: each run differs, so a page
    // built from it could never be the same file. Refused with the reason,
    // like timestamp().
    e.register_fn("random", || -> Res<FLOAT> {
        err("random() is not available: Math.random gives a different result on every build, so a page made from it can never match 11ty's. For a filter that shuffles or picks at random, use config.add_nondeterministic_filter(name), which refuses only the pages that use it".into())
    });
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
