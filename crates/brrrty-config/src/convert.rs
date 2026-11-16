//! Rhai values to Brrrty data and back.
//!
//! Rhai's object maps keep their keys sorted, not in the order they were
//! written, so a map made in a script comes out with its keys sorted. An
//! `object()` (see `object.rs`) keeps insertion order, and every object that
//! comes in from Brrrty is one. Everything else keeps its shape: integers and floats become JavaScript
//! numbers, `()` is `undefined`, and a date made with `now()` or `date()`
//! is a JavaScript `Date`.

use std::sync::Arc;

use brrrty_core::frontmatter::Data;
use brrrty_js::Value;
use chrono::{DateTime, FixedOffset};
use rhai::{Array, Dynamic, Map};

use crate::api::JsDate;
use crate::object::JsObject;

/// How deeply data may nest. Deeper data is refused rather than walked, so
/// converting it can't overflow the stack.
pub const MAX_DEPTH: usize = 256;

/// `v` as Brrrty data, or why it can't be.
pub fn to_value(v: &Dynamic) -> Result<Value, String> {
    to_value_at(v, 0)
}

fn to_value_at(v: &Dynamic, depth: usize) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err(format!("data nests deeper than {MAX_DEPTH} levels"));
    }
    if v.is_shared() {
        return to_value_at(&v.flatten_clone(), depth);
    }
    if v.is_unit() {
        return Ok(Value::Undefined);
    }
    if let Some(b) = v.clone().try_cast::<bool>() {
        return Ok(Value::Bool(b));
    }
    if let Ok(i) = v.as_int() {
        return Ok(Value::Number(i as f64));
    }
    if let Ok(f) = v.as_float() {
        return Ok(Value::Number(f));
    }
    if let Ok(c) = v.as_char() {
        return Ok(Value::String(c.to_string().into()));
    }
    if let Some(s) = v.read_lock::<rhai::ImmutableString>() {
        return Ok(Value::String(s.as_str().into()));
    }
    if let Some(a) = v.read_lock::<Array>() {
        let items = a
            .iter()
            .map(|x| to_value_at(x, depth + 1))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Value::Array(Arc::new(items)));
    }
    if let Some(m) = v.read_lock::<Map>() {
        let mut out = Data::new();
        for (k, x) in m.iter() {
            out.insert(k.to_string(), to_value_at(x, depth + 1)?);
        }
        return Ok(Value::Object(Arc::new(out)));
    }
    if let Some(o) = v.read_lock::<JsObject>() {
        // Insertion order: that is how a `Value` object is kept, and its
        // readers list the keys in JavaScript's order.
        let mut out = Data::new();
        for (k, x) in o.0.iter() {
            out.insert(k.clone(), to_value_at(x, depth + 1)?);
        }
        return Ok(Value::Object(Arc::new(out)));
    }
    if let Some(d) = v.read_lock::<JsDate>() {
        return date_value(d.0)
            .ok_or_else(|| "an invalid date can't be data in Brrrty yet".to_string());
    }
    Err(format!("a value of type `{}` can't be data", v.type_name()))
}

/// A JavaScript date at `ms`, in UTC like `Date` itself.
pub fn date_value(ms: f64) -> Option<Value> {
    if !ms.is_finite() {
        return None;
    }
    let utc = DateTime::from_timestamp_millis(ms as i64)?;
    Some(Value::Date(utc.with_timezone(&FixedOffset::east_opt(0)?)))
}

/// `v` as a Rhai value. `null` and `undefined` both become `()`.
pub fn to_dynamic(v: &Value) -> Dynamic {
    match v {
        Value::Undefined | Value::Null => Dynamic::UNIT,
        Value::Bool(b) => Dynamic::from_bool(*b),
        Value::Number(n) => {
            if n.fract() == 0.0
                && n.abs() < 9.007_199_254_740_992e15
                && !(*n == 0.0 && n.is_sign_negative())
            {
                Dynamic::from_int(*n as rhai::INT)
            } else {
                Dynamic::from_float(*n)
            }
        }
        Value::String(s) => Dynamic::from(s.to_string()),
        Value::Array(a) => Dynamic::from_array(a.iter().map(to_dynamic).collect()),
        Value::Object(o) => Dynamic::from(JsObject(
            o.iter().map(|(k, x)| (k.clone(), to_dynamic(x))).collect(),
        )),
        Value::Date(d) => Dynamic::from(JsDate(d.timestamp_millis() as f64)),
    }
}

/// 11ty's `DeepCopy` (`Merge` without `override:` keys): arrays join, plain
/// objects merge key by key, an object target stays when the source is not
/// an object, and anything else is replaced by the source.
pub fn deep_copy(target: Option<&Value>, source: &Value) -> Value {
    match (target, source) {
        (Some(Value::Array(t)), Value::Array(s)) => {
            Value::Array(Arc::new(t.iter().chain(s.iter()).cloned().collect()))
        }
        (Some(Value::Object(t)), Value::Object(s)) => {
            let mut out = (**t).clone();
            for (k, x) in s.iter() {
                let merged = deep_copy(out.get(k), x);
                out.insert(k.clone(), merged);
            }
            Value::Object(Arc::new(out))
        }
        (Some(t @ Value::Object(_)), _) => t.clone(),
        (_, s) => s.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(pairs: &[(&str, Value)]) -> Value {
        Value::Object(Arc::new(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        ))
    }

    fn n(x: f64) -> Value {
        Value::Number(x)
    }

    #[test]
    fn deep_copy_merges_like_11ty() {
        let a = obj(&[
            ("x", n(1.0)),
            ("o", obj(&[("a", n(1.0))])),
            ("l", Value::Array(Arc::new(vec![n(1.0)]))),
        ]);
        let b = obj(&[
            ("o", obj(&[("b", n(2.0))])),
            ("x", n(2.0)),
            ("l", Value::Array(Arc::new(vec![n(2.0)]))),
            ("y", n(3.0)),
        ]);
        let m = deep_copy(Some(&a), &b);
        let Value::Object(m) = m else { panic!() };
        let keys: Vec<&str> = m.keys().map(String::as_str).collect();
        assert_eq!(keys, ["x", "o", "l", "y"]);
        assert_eq!(m["x"], n(2.0));
        assert_eq!(m["l"], Value::Array(Arc::new(vec![n(1.0), n(2.0)])));
        // An object target keeps its value when the source is not an object.
        assert_eq!(deep_copy(Some(&m["o"]), &n(5.0)), m["o"]);
    }

    #[test]
    fn values_round_trip() {
        for v in [
            n(1.0),
            n(1.5),
            n(-0.0),
            Value::Bool(true),
            Value::String("s".into()),
            Value::Array(Arc::new(vec![n(1.0), Value::String("x".into())])),
            obj(&[("a", n(1.0)), ("b", obj(&[("c", Value::Bool(false))]))]),
            date_value(1_700_000_000_123.0).unwrap(),
        ] {
            let back = to_value(&to_dynamic(&v)).unwrap();
            match (&v, &back) {
                (Value::Number(a), Value::Number(b)) => {
                    assert!(a == b && a.is_sign_negative() == b.is_sign_negative())
                }
                _ => assert_eq!(back, v),
            }
        }
        assert_eq!(
            to_value(&to_dynamic(&Value::Null)).unwrap(),
            Value::Undefined
        );
    }

    #[test]
    fn deep_data_is_refused_not_walked() {
        let mut d = Dynamic::from_array(vec![]);
        for _ in 0..MAX_DEPTH + 5 {
            d = Dynamic::from_array(vec![d]);
        }
        assert!(to_value(&d).unwrap_err().contains("deeper"));
    }
}
