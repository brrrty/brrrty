//! The worker's wire forms (see `worker/encode.mjs`), decoded into Brrrty
//! values. Anything not exactly one of the forms is an error: the worker
//! and this file are one protocol, so a mismatch is a bug, never data.

use std::sync::Arc;

use brrrty_js::Value;
use brrrty_js::runtime::{Decoded, FnHandle, PathPart};
use serde_json::Value as Json;

/// Decode one encoded value.
pub fn decode(json: &Json) -> Result<Decoded, String> {
    let mut d = Decoded {
        value: Value::Undefined,
        functions: Vec::new(),
        refused: Vec::new(),
    };
    let mut path = Vec::new();
    d.value = value(json, &mut path, &mut d, false)?;
    Ok(d)
}

fn value(
    json: &Json,
    path: &mut Vec<PathPart>,
    d: &mut Decoded,
    in_array: bool,
) -> Result<Value, String> {
    Ok(match json {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::Number(
            n.as_f64()
                .ok_or_else(|| format!("number out of range: {n}"))?,
        ),
        Json::String(s) => Value::String(s.as_str().into()),
        Json::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                path.push(PathPart::Index(i));
                out.push(value(item, path, d, true)?);
                path.pop();
            }
            Value::Array(Arc::new(out))
        }
        Json::Object(o) => {
            let mut it = o.iter();
            let (Some((tag, body)), None) = (it.next(), it.next()) else {
                return Err(format!("not a wire form: {json}"));
            };
            match (tag.as_str(), body) {
                ("$o", Json::Array(entries)) => {
                    let mut out = indexmap::IndexMap::with_capacity(entries.len());
                    for entry in entries {
                        let Some([Json::String(k), v]) = entry.as_array().map(Vec::as_slice) else {
                            return Err(format!("not an object entry: {entry}"));
                        };
                        path.push(PathPart::Key(k.clone()));
                        let v = value(v, path, d, false)?;
                        path.pop();
                        out.insert(k.clone(), v);
                    }
                    Value::Object(Arc::new(out))
                }
                ("$u", _) => Value::Undefined,
                ("$hole", _) if in_array => Value::Undefined,
                ("$n", Json::String(s)) => Value::Number(match s.as_str() {
                    "-0" => -0.0,
                    "NaN" => f64::NAN,
                    "Infinity" => f64::INFINITY,
                    "-Infinity" => f64::NEG_INFINITY,
                    other => return Err(format!("not a special number: {other}")),
                }),
                ("$d", Json::Number(ms)) => {
                    let ms = ms
                        .as_f64()
                        .filter(|m| m.fract() == 0.0)
                        .ok_or_else(|| format!("not a time: {ms}"))?;
                    match chrono::DateTime::from_timestamp_millis(ms as i64) {
                        Some(t) => Value::Date(t.fixed_offset()),
                        None => {
                            d.refused.push((
                                path.clone(),
                                "a Date too far from 1970 for Brrrty to print".to_string(),
                            ));
                            Value::Undefined
                        }
                    }
                }
                ("$f", Json::Number(h)) => {
                    let h = h
                        .as_u64()
                        .and_then(|h| u32::try_from(h).ok())
                        .ok_or_else(|| format!("not a function handle: {h}"))?;
                    d.functions.push((path.clone(), FnHandle(h)));
                    Value::Undefined
                }
                ("$x", Json::String(what)) => {
                    d.refused.push((path.clone(), what.clone()));
                    Value::Undefined
                }
                _ => return Err(format!("not a wire form: {json}")),
            }
        }
    })
}
