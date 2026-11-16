//! The `Value` type.

use std::sync::Arc;

use chrono::{DateTime, FixedOffset};
use indexmap::IndexMap;

use crate::number::{js_number_to_string, js_string_to_number};
use crate::tz::Tz;

/// A JavaScript-like value. Objects keep their insertion order.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(Arc<str>),
    Array(Arc<Vec<Value>>),
    Object(Arc<IndexMap<String, Value>>),
    Date(DateTime<FixedOffset>),
}

impl Value {
    /// Converts JSON, keeping object key order (relies on `serde_json`'s `preserve_order`).
    pub fn from_json(v: &serde_json::Value) -> Value {
        match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(n) => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
            serde_json::Value::String(s) => Value::String(s.as_str().into()),
            serde_json::Value::Array(a) => {
                Value::Array(Arc::new(a.iter().map(Value::from_json).collect()))
            }
            serde_json::Value::Object(o) => Value::Object(Arc::new(
                o.iter()
                    .map(|(k, v)| (k.clone(), Value::from_json(v)))
                    .collect(),
            )),
        }
    }

    /// Liquid truthiness: only `false`, `null` and `undefined` are falsy.
    pub fn liquid_truthy(&self) -> bool {
        !matches!(self, Value::Undefined | Value::Null | Value::Bool(false))
    }

    /// JavaScript truthiness: `false`, `0`, `-0`, `NaN`, `""`, `null` and `undefined` are falsy.
    pub fn js_truthy(&self) -> bool {
        match self {
            Value::Undefined | Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0 && !n.is_nan(),
            Value::String(s) => !s.is_empty(),
            Value::Array(_) | Value::Object(_) | Value::Date(_) => true,
        }
    }

    /// JavaScript's `String(v)`, printing a `Date` in UTC. Use
    /// [`Value::to_js_string_in`] to print it in another zone.
    pub fn to_js_string(&self) -> String {
        self.to_js_string_in(Tz::Utc)
    }

    /// JavaScript's `String(v)`, printing a `Date` as
    /// `Date.prototype.toString` does in `tz`, with the zone's long name:
    /// `Tue Mar 03 2026 19:00:00 GMT-0500 (Eastern Standard Time)`.
    pub fn to_js_string_in(&self, tz: Tz) -> String {
        match self {
            Value::Undefined => "undefined".into(),
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => js_number_to_string(*n),
            Value::String(s) => s.to_string(),
            // Array.prototype.toString: join(","), with null and undefined as empty strings.
            Value::Array(a) => a
                .iter()
                .map(|v| match v {
                    Value::Null | Value::Undefined => String::new(),
                    v => v.to_js_string_in(tz),
                })
                .collect::<Vec<_>>()
                .join(","),
            Value::Object(_) => "[object Object]".into(),
            Value::Date(d) => crate::tz::date_to_string(tz, d.timestamp_millis() as f64),
        }
    }

    /// JavaScript's `Number(v)`. Arrays and objects go through their
    /// string form, as `ToPrimitive` does; a `Date` is its time in
    /// milliseconds.
    pub fn to_js_number(&self) -> f64 {
        match self {
            Value::Undefined => f64::NAN,
            Value::Null => 0.0,
            Value::Bool(b) => f64::from(u8::from(*b)),
            Value::Number(n) => *n,
            Value::String(s) => js_string_to_number(s),
            Value::Array(_) | Value::Object(_) => js_string_to_number(&self.to_js_string()),
            Value::Date(d) => d.timestamp_millis() as f64,
        }
    }

    /// An object's entries in JavaScript's own-key order: keys that are
    /// array indexes (`"0"` to `"4294967294"`, written canonically) come
    /// first in numeric order, then the other keys in insertion order. This
    /// is the order `Object.keys` and `for...in` give.
    pub fn js_entries(o: &IndexMap<String, Value>) -> Vec<(&String, &Value)> {
        crate::traps::entries(o);
        Self::js_order(o)
    }

    /// [`Value::js_entries`], or `None` when listing `o` is a trap (see
    /// [`crate::traps::entries`]): a walk that meets one can stop, since the
    /// render will be refused.
    pub fn js_entries_or_trap(o: &IndexMap<String, Value>) -> Option<Vec<(&String, &Value)>> {
        if crate::traps::entries(o) {
            return None;
        }
        Some(Self::js_order(o))
    }

    fn js_order(o: &IndexMap<String, Value>) -> Vec<(&String, &Value)> {
        let mut index: Vec<(u32, (&String, &Value))> = Vec::new();
        let mut rest = Vec::new();
        for (k, v) in o {
            match array_index(k) {
                Some(i) => index.push((i, (k, v))),
                None => rest.push((k, v)),
            }
        }
        index.sort_by_key(|(i, _)| *i);
        index.into_iter().map(|(_, e)| e).chain(rest).collect()
    }

    /// Property lookup. Only `.length` is special-cased (UTF-16 length for strings); Liquid's
    /// `.size`, `.first` and `.last` belong to `brrrty-liquid`.
    pub fn get(&self, key: &str) -> Value {
        match self {
            Value::String(s) if key == "length" => Value::Number(s.encode_utf16().count() as f64),
            Value::Array(a) if key == "length" => Value::Number(a.len() as f64),
            Value::Object(o) => o.get(key).cloned().unwrap_or(Value::Undefined),
            _ => Value::Undefined,
        }
    }

    /// Bracket lookup as LiquidJS does it: negative array indexes count from the end, and
    /// numeric strings index arrays like property names do.
    pub fn index(&self, i: &Value) -> Value {
        match self {
            Value::Array(a) => match i {
                Value::Number(n) => {
                    let len = a.len() as f64;
                    let idx = if *n < 0.0 { len + n } else { *n };
                    if idx.fract() == 0.0 && idx >= 0.0 && idx < len {
                        a[idx as usize].clone()
                    } else {
                        Value::Undefined
                    }
                }
                Value::String(s) => match s.parse::<usize>() {
                    Ok(n) if n.to_string() == **s => a.get(n).cloned().unwrap_or(Value::Undefined),
                    _ => self.get(s),
                },
                _ => Value::Undefined,
            },
            Value::String(s) => match i {
                Value::Number(n) if *n >= 0.0 && n.fract() == 0.0 => s
                    .encode_utf16()
                    .nth(*n as usize)
                    .map(|u| Value::String(String::from_utf16_lossy(&[u]).into()))
                    .unwrap_or(Value::Undefined),
                Value::String(k) => self.get(k),
                _ => Value::Undefined,
            },
            Value::Object(_) => match i {
                Value::String(_) | Value::Number(_) | Value::Bool(_) => self.get(&i.to_js_string()),
                _ => Value::Undefined,
            },
            _ => Value::Undefined,
        }
    }
}

/// `key` as an array index, if it is one written canonically: `"7"` but not
/// `"07"`, `"-1"` or `"4294967295"`.
pub fn array_index(key: &str) -> Option<u32> {
    let n: u32 = key.parse().ok()?;
    (n != u32::MAX && n.to_string() == key).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn s(x: &str) -> Value {
        Value::String(x.into())
    }
    fn arr(v: Vec<Value>) -> Value {
        Value::Array(Arc::new(v))
    }
    fn obj(pairs: &[(&str, Value)]) -> Value {
        Value::Object(Arc::new(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        ))
    }
    fn date() -> Value {
        Value::Date(DateTime::parse_from_rfc3339("1970-01-01T00:00:00+00:00").unwrap())
    }

    #[test]
    fn truthiness_tables() {
        let all = [
            (Value::Undefined, false, false),
            (Value::Null, false, false),
            (Value::Bool(false), false, false),
            (Value::Bool(true), true, true),
            (Value::Number(0.0), true, false),
            (Value::Number(-0.0), true, false),
            (Value::Number(f64::NAN), true, false),
            (Value::Number(2.0), true, true),
            (s(""), true, false),
            (s("0"), true, true),
            (arr(vec![]), true, true),
            (obj(&[]), true, true),
            (date(), true, true),
        ];
        for (v, liquid, js) in all {
            assert_eq!(v.liquid_truthy(), liquid, "liquid {v:?}");
            assert_eq!(v.js_truthy(), js, "js {v:?}");
        }
    }

    #[test]
    fn from_json_keeps_key_order() {
        let j: serde_json::Value =
            serde_json::from_str(r#"{"b":1,"a":{"z":null,"y":[true,"x"]}}"#).unwrap();
        let v = Value::from_json(&j);
        let Value::Object(o) = &v else { panic!() };
        assert_eq!(o.keys().collect::<Vec<_>>(), ["b", "a"]);
        let Value::Object(inner) = &o["a"] else {
            panic!()
        };
        assert_eq!(inner.keys().collect::<Vec<_>>(), ["z", "y"]);
        assert_eq!(inner["z"], Value::Null);
        assert_eq!(Value::from_json(&json!(1.5)), Value::Number(1.5));
    }

    #[test]
    fn to_js_string_matches_node() {
        // Verified with: node -e 'String([1,[2,3]]), String([null,undefined,1]), String({})'
        assert_eq!(
            arr(vec![
                Value::Number(1.0),
                arr(vec![Value::Number(2.0), Value::Number(3.0)])
            ])
            .to_js_string(),
            "1,2,3"
        );
        assert_eq!(
            arr(vec![Value::Null, Value::Undefined, Value::Number(1.0)]).to_js_string(),
            ",,1"
        );
        assert_eq!(obj(&[("a", Value::Null)]).to_js_string(), "[object Object]");
        assert_eq!(Value::Null.to_js_string(), "null");
        assert_eq!(Value::Undefined.to_js_string(), "undefined");
        assert_eq!(Value::Bool(true).to_js_string(), "true");
        assert_eq!(Value::Number(1e21).to_js_string(), "1e+21");
        assert_eq!(s("hi").to_js_string(), "hi");
        assert_eq!(arr(vec![]).to_js_string(), "");
    }

    #[test]
    fn to_js_number_matches_node() {
        // node: Number(undefined), Number(null), Number(true), Number([2]),
        // Number([1,2]), Number([]), Number({}), Number(new Date(0)), Number(' 0x10 ')
        assert!(Value::Undefined.to_js_number().is_nan());
        assert_eq!(Value::Null.to_js_number(), 0.0);
        assert_eq!(Value::Bool(true).to_js_number(), 1.0);
        assert_eq!(arr(vec![Value::Number(2.0)]).to_js_number(), 2.0);
        assert!(
            arr(vec![Value::Number(1.0), Value::Number(2.0)])
                .to_js_number()
                .is_nan()
        );
        assert_eq!(arr(vec![]).to_js_number(), 0.0);
        assert!(obj(&[]).to_js_number().is_nan());
        assert_eq!(date().to_js_number(), 0.0);
        assert_eq!(s(" 0x10 ").to_js_number(), 16.0);
    }

    #[test]
    fn js_entries_put_index_keys_first() {
        // node: Object.keys({b:1,"1":2,a:3,"0":4,"4294967295":5,"4294967294":6,"-1":7,"01":8,"1.5":9})
        let keys = [
            "b",
            "1",
            "a",
            "0",
            "4294967295",
            "4294967294",
            "-1",
            "01",
            "1.5",
        ];
        let o: IndexMap<String, Value> =
            keys.iter().map(|k| (k.to_string(), Value::Null)).collect();
        let got: Vec<&str> = Value::js_entries(&o)
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(
            got,
            [
                "0",
                "1",
                "4294967294",
                "b",
                "a",
                "4294967295",
                "-1",
                "01",
                "1.5"
            ]
        );
    }

    #[test]
    fn get_length_is_utf16() {
        assert_eq!(s("😀").get("length"), Value::Number(2.0));
        assert_eq!(s("héllo😀").get("length"), Value::Number(7.0));
        assert_eq!(arr(vec![Value::Null; 3]).get("length"), Value::Number(3.0));
        assert_eq!(
            obj(&[("k", Value::Number(1.0))]).get("k"),
            Value::Number(1.0)
        );
        assert_eq!(
            obj(&[("length", Value::Number(7.0))]).get("length"),
            Value::Number(7.0)
        );
        assert_eq!(obj(&[]).get("nope"), Value::Undefined);
        assert_eq!(Value::Null.get("x"), Value::Undefined);
        assert_eq!(Value::Number(5.0).get("length"), Value::Undefined);
        assert_eq!(date().get("length"), Value::Undefined);
    }

    #[test]
    fn index_follows_liquidjs() {
        // Probed against liquidjs 10.30.1: a[-1] is the last item, a[-4] and a[5] are empty,
        // a['1'] and a['length'] work, s[0] is a char, s[-1] is empty.
        let a = arr(vec![
            Value::Number(10.0),
            Value::Number(20.0),
            Value::Number(30.0),
        ]);
        assert_eq!(a.index(&Value::Number(0.0)), Value::Number(10.0));
        assert_eq!(a.index(&Value::Number(-1.0)), Value::Number(30.0));
        assert_eq!(a.index(&Value::Number(-4.0)), Value::Undefined);
        assert_eq!(a.index(&Value::Number(5.0)), Value::Undefined);
        assert_eq!(a.index(&Value::Number(1.5)), Value::Undefined);
        assert_eq!(a.index(&s("1")), Value::Number(20.0));
        assert_eq!(a.index(&s("length")), Value::Number(3.0));
        assert_eq!(a.index(&Value::Bool(true)), Value::Undefined);
        assert_eq!(a.index(&Value::Null), Value::Undefined);
        assert_eq!(s("héllo").index(&Value::Number(1.0)), s("é"));
        assert_eq!(s("hi").index(&Value::Number(-1.0)), Value::Undefined);
        assert_eq!(s("hi").index(&Value::Number(9.0)), Value::Undefined);
        let o = obj(&[("k", Value::Number(1.0)), ("0", Value::Bool(true))]);
        assert_eq!(o.index(&s("k")), Value::Number(1.0));
        assert_eq!(o.index(&Value::Number(0.0)), Value::Bool(true));
        assert_eq!(
            Value::Number(5.0).index(&Value::Number(0.0)),
            Value::Undefined
        );
    }
}
