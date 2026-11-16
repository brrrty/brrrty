//! JavaScript semantics that 11ty's behavior depends on.

use brrrty_js::Value;

/// True when JavaScript would treat `v` as truthy: `false`, `0`, `""` and `null` are falsy,
/// and every other value, including empty arrays and objects, is truthy.
///
/// The logic lives in [`brrrty_js::Value::js_truthy`]; this delegates.
pub fn js_truthy(v: &Value) -> bool {
    v.js_truthy()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_javascript_truthiness() {
        for v in [
            json!(true),
            json!("yes"),
            json!("false"),
            json!("0"),
            json!(1),
            json!(-1.5),
            json!([]),
            json!({}),
            json!([0]),
            json!({"a": 1}),
        ] {
            let v = Value::from_json(&v);
            assert!(js_truthy(&v), "{v:?} should be truthy");
        }
        for v in [
            json!(false),
            json!(0),
            json!(0.0),
            json!(-0.0),
            json!(""),
            json!(null),
        ] {
            let v = Value::from_json(&v);
            assert!(!js_truthy(&v), "{v:?} should be falsy");
        }
    }
}
