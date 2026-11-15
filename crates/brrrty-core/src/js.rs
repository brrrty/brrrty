//! JavaScript semantics that 11ty's behavior depends on.

use serde_json::Value;

/// True when JavaScript would treat `v` as truthy: `false`, `0`, `""` and `null` are falsy,
/// and every other value, including empty arrays and objects, is truthy.
pub fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
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
            assert!(js_truthy(&v), "{v} should be truthy");
        }
        for v in [
            json!(false),
            json!(0),
            json!(0.0),
            json!(-0.0),
            json!(""),
            json!(null),
        ] {
            assert!(!js_truthy(&v), "{v} should be falsy");
        }
    }
}
