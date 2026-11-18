//! The data cascade: how 11ty 3.1.8's `Template.getData` builds a page's
//! data, probed with real 11ty.
//!
//! Lowest priority first:
//!
//! 1. global data (`_data`, then `eleventy` and `pkg`);
//! 2. the layout chain's front matter, from the layout chosen by the first
//!    truthy `layout` of front matter, data files, then global data;
//! 3. directory data files, from the input folder down, then the template's
//!    own data files: `<name>.json` under `<name>.11tydata.json` (and the
//!    `.11tydata` JavaScript files over both);
//! 4. front matter.
//!
//! Every level merges deeply ([`merge`]). Front matter, layout front matter
//! and each data file have their `tags` cleaned first ([`clean_tags`]);
//! global data keeps its `tags` as written.

use std::collections::HashSet;
use std::sync::Arc;

use brrrty_js::Value;
use brrrty_liquid::Tz;

use super::merge;
use crate::frontmatter::Data;

/// The page's data: `global`, then `layout`, then `local`, then `front`.
pub(crate) fn page_data(global: &Data, layout: &Data, local: &Data, front: &Data) -> Data {
    let mut data = Data::new();
    for level in [global, layout, local, front] {
        merge(&mut data, level);
    }
    data
}

/// 11ty's `TemplateData.cleanupData`: a string `tags` splits at commas, a
/// list stays, and any other falsy value is an empty list; repeats go
/// (`new Set`) and entries become strings. `Err` holds the value 11ty
/// refuses: anything else truthy.
pub(crate) fn clean_tags(data: &mut Data) -> Result<(), Value> {
    let Some(tags) = data.get("tags") else {
        return Ok(());
    };
    let list: Vec<Value> = match tags {
        v if !v.js_truthy() => Vec::new(),
        Value::String(s) => s.split(',').map(|t| Value::String(t.into())).collect(),
        Value::Array(a) => a.as_ref().clone(),
        other => return Err(other.clone()),
    };
    let mut seen: HashSet<SetKey> = HashSet::new();
    let mut cleaned = Vec::new();
    for v in list {
        if let Some(key) = set_key(&v)
            && !seen.insert(key)
        {
            continue;
        }
        cleaned.push(Value::String(v.to_js_string_in(Tz::Local).into()));
    }
    data.insert("tags".to_string(), Value::Array(Arc::new(cleaned)));
    Ok(())
}

/// What `new Set` compares primitives by (SameValueZero). Objects, lists
/// and dates are distinct objects in parsed data, so they never repeat.
#[derive(Hash, PartialEq, Eq)]
enum SetKey {
    Undefined,
    Null,
    Bool(bool),
    Number(u64),
    String(Arc<str>),
}

fn set_key(v: &Value) -> Option<SetKey> {
    Some(match v {
        Value::Undefined => SetKey::Undefined,
        Value::Null => SetKey::Null,
        Value::Bool(b) => SetKey::Bool(*b),
        // SameValueZero: -0 is 0 and NaN is NaN.
        Value::Number(n) if n.is_nan() => SetKey::Number(f64::NAN.to_bits()),
        Value::Number(n) => SetKey::Number((n + 0.0).to_bits()),
        Value::String(s) => SetKey::String(s.clone()),
        Value::Array(_) | Value::Object(_) | Value::Date(_) | Value::Function(_) => return None,
    })
}

/// The 11ty-supplied keys `data` would add to. 11ty freezes `eleventy`
/// and `pkg`, so outside global data an object with keys under either name
/// stops the build (probed), while any other value is ignored by the merge.
pub(crate) fn frozen_keys(data: &Data) -> Vec<String> {
    ["eleventy", "pkg"]
        .into_iter()
        .filter(|k| matches!(data.get(*k), Some(Value::Object(o)) if !o.is_empty()))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(v: serde_json::Value) -> Data {
        match Value::from_json(&v) {
            Value::Object(m) => Arc::unwrap_or_clone(m),
            _ => unreachable!(),
        }
    }

    fn tags(v: serde_json::Value) -> Result<Value, Value> {
        let mut d = map(json!({ "tags": v }));
        clean_tags(&mut d).map(|()| d["tags"].clone())
    }

    /// Probed values.
    #[test]
    fn tags_are_cleaned_like_11ty() {
        assert_eq!(
            tags(json!("a,b, c,a")),
            Ok(Value::from_json(&json!(["a", "b", " c"])))
        );
        assert_eq!(
            tags(json!([1, "1", true, null, ["x", "y"], {"k": "v"}, 1, null])),
            Ok(Value::from_json(&json!([
                "1",
                "1",
                "true",
                "null",
                "x,y",
                "[object Object]"
            ])))
        );
        assert_eq!(tags(json!("")), Ok(Value::from_json(&json!([]))));
        assert_eq!(tags(json!(null)), Ok(Value::from_json(&json!([]))));
        assert_eq!(tags(json!(0)), Ok(Value::from_json(&json!([]))));
        assert_eq!(tags(json!(5)), Err(Value::Number(5.0)));
        assert!(tags(json!({"a": 1})).is_err());
        let mut none = map(json!({"x": 1}));
        clean_tags(&mut none).unwrap();
        assert!(!none.contains_key("tags"));
    }

    /// Probed: global < layout front matter < data files < front matter,
    /// with lists joined in that order.
    #[test]
    fn levels_merge_in_order() {
        let d = page_data(
            &map(json!({"arr": ["g"], "lvl": "g", "o": {"g": 1}})),
            &map(json!({"arr": ["layout"], "lvl": "layout"})),
            &map(json!({"arr": ["local"], "lvl": "local", "o": {"l": 1}})),
            &map(json!({"arr": ["fm"]})),
        );
        assert_eq!(
            Value::Object(Arc::new(d)),
            Value::from_json(&json!({
                "arr": ["g", "layout", "local", "fm"],
                "lvl": "local",
                "o": {"g": 1, "l": 1},
            }))
        );
    }

    #[test]
    fn frozen_keys_are_objects_with_keys() {
        assert_eq!(
            frozen_keys(&map(json!({"pkg": {"a": 1}, "eleventy": {}}))),
            ["pkg"]
        );
        assert!(frozen_keys(&map(json!({"pkg": 5, "eleventy": "s"}))).is_empty());
    }
}
