//! Front matter splitting and YAML parsing.

use std::sync::Arc;

use brrrty_js::Value;
use indexmap::IndexMap;

use crate::problem::{Problem, codes};

/// Page, layout and data-file data: a JavaScript object's own properties, in insertion order.
pub type Data = IndexMap<String, Value>;

/// Splits `source` into its front matter map and the body that follows.
///
/// Front matter is a first line of `---` (optionally followed by whitespace)
/// through the next `---` line. Both `\n` and `\r\n` are accepted; the body is
/// returned untouched. Without front matter the map is empty and the body is
/// the whole source.
pub fn split_front_matter(source: &str) -> Result<(Data, &str), Problem> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let Some((first, rest)) = next_line(source) else {
        return Ok((Data::new(), source));
    };
    if !is_delimiter(first) {
        return Ok((Data::new(), source));
    }

    let mut offset = 0;
    let mut remaining = rest;
    while let Some((line, after)) = next_line(remaining) {
        if is_delimiter(line) {
            let yaml = &rest[..offset];
            return Ok((parse_yaml(yaml)?, after));
        }
        offset += remaining.len() - after.len();
        remaining = after;
    }
    Err(Problem::error(
        codes::FRONT_MATTER_INVALID,
        "front matter is missing its closing `---` line",
    )
    .at(1, 1)
    .with_help("add a line containing only `---` after the front matter"))
}

/// Returns the next line (without its line ending) and the text after it.
fn next_line(text: &str) -> Option<(&str, &str)> {
    if text.is_empty() {
        return None;
    }
    match text.find('\n') {
        Some(i) => Some((text[..i].trim_end_matches('\r'), &text[i + 1..])),
        None => Some((text, "")),
    }
}

fn is_delimiter(line: &str) -> bool {
    line.strip_prefix("---")
        .is_some_and(|tail| tail.chars().all(char::is_whitespace))
}

/// Parses front matter YAML as js-yaml does (see [`crate::yaml`]), so scalars such as
/// `0123`, `.inf` and `2020-01-02` resolve to the numbers and dates 11ty sees.
fn parse_yaml(yaml: &str) -> Result<Data, Problem> {
    match crate::yaml::load(yaml) {
        Ok(Value::Object(map)) => Ok(Arc::unwrap_or_clone(map)),
        // Lists and scalars are not data; 11ty renders the page with none.
        Ok(_) => Ok(Data::new()),
        // +1 for the opening `---` line.
        Err(e) => Err(Problem::error(
            codes::FRONT_MATTER_INVALID,
            format!("invalid YAML front matter: {}", e.message),
        )
        .at(e.line + 1, e.column.max(1))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

    fn s(x: &str) -> Value {
        Value::String(x.into())
    }
    fn n(x: f64) -> Value {
        Value::Number(x)
    }
    fn date(rfc3339: &str) -> Value {
        Value::Date(DateTime::parse_from_rfc3339(rfc3339).unwrap())
    }
    /// The value of `v:` in front matter.
    fn v(text: &str) -> Value {
        let (m, _) = split_front_matter(&format!("---\nv: {text}\n---\n")).unwrap();
        m["v"].clone()
    }

    #[test]
    fn invalid_yaml_message_has_no_relative_positions() {
        let p = split_front_matter("---\ntitle: [oops\n---\nbody\n").unwrap_err();
        assert!(!p.message.contains("at line"), "{}", p.message);
        assert!(p.line.is_some() && p.column.is_some());
    }

    #[test]
    fn splits_lf_and_crlf_identically() {
        let (a, body_a) = split_front_matter("---\ntitle: Hi\n---\nBody\n").unwrap();
        let (b, body_b) = split_front_matter("---\r\ntitle: Hi\r\n---\r\nBody\r\n").unwrap();
        assert_eq!(a, b);
        assert_eq!(a["title"], s("Hi"));
        assert_eq!(body_a, "Body\n");
        assert_eq!(body_b, "Body\r\n");
    }

    #[test]
    fn no_front_matter_returns_whole_source() {
        let (m, body) = split_front_matter("no front matter\n").unwrap();
        assert!(m.is_empty());
        assert_eq!(body, "no front matter\n");
    }

    #[test]
    fn invalid_yaml_reports_file_line() {
        let p = split_front_matter("---\ntitle: [unclosed\n---\nbad\n").unwrap_err();
        assert_eq!(p.code, codes::FRONT_MATTER_INVALID);
        assert_eq!(p.line, Some(3));
    }

    #[test]
    fn ignores_unknown_keys_like_disqus() {
        let (m, _) = split_front_matter("---\ndisqus: true\ntitle: X\n---\n").unwrap();
        assert_eq!(m["disqus"], Value::Bool(true));
    }

    /// Probed with 11ty 3.1.8: `date: 2020-01-02` is a `Date` (it prints as
    /// `Wed Jan 01 2020 ...` and `| date: "%s"` is 1577923200), not a string.
    #[test]
    fn unquoted_timestamps_are_dates() {
        assert_eq!(v("2020-01-02"), date("2020-01-02T00:00:00Z"));
        assert_eq!(v("2020-01-02T03:04:05+02:00"), date("2020-01-02T01:04:05Z"));
        assert_eq!(v("2020-01-02 03:04:05"), date("2020-01-02T03:04:05Z"));
        assert_eq!(v("2020-01-02T03:04:05Z"), date("2020-01-02T03:04:05Z"));
        assert_eq!(v("2020-01-02t03:04:05Z"), date("2020-01-02T03:04:05Z"));
        assert_eq!(
            v("2020-01-02 03:04:05 +02:00"),
            date("2020-01-02T01:04:05Z")
        );
        assert_eq!(v("2020-01-02 03:04:05 Z"), date("2020-01-02T03:04:05Z"));
        assert_eq!(v("2020-1-2 3:04:05"), date("2020-01-02T03:04:05Z"));
        assert_eq!(v("2020-01-02T03:04:05-5"), date("2020-01-02T08:04:05Z"));
        // Fractions keep three digits, truncated.
        assert_eq!(
            v("2020-01-02T03:04:05.123456Z"),
            date("2020-01-02T03:04:05.123Z")
        );
        assert_eq!(v("2020-01-02 03:04:05.5"), date("2020-01-02T03:04:05.500Z"));
        // `Date.UTC` rolls fields over.
        assert_eq!(v("2020-13-45"), date("2021-02-14T00:00:00Z"));
        assert_eq!(v("2020-02-30"), date("2020-03-01T00:00:00Z"));
        assert_eq!(v("2020-01-02T25:61:61"), date("2020-01-03T02:02:01Z"));
        // Quoted, or not shaped like a timestamp: strings.
        assert_eq!(v("'2020-01-02'"), s("2020-01-02"));
        assert_eq!(v("\"2020-01-02\""), s("2020-01-02"));
        assert_eq!(v("2020-1-2"), s("2020-1-2"));
        assert_eq!(v("2020-01-02T03:04:05+0200"), s("2020-01-02T03:04:05+0200"));
        assert_eq!(v("2020-01-02t03:04:05z"), s("2020-01-02t03:04:05z"));
        // Nested values resolve too.
        assert_eq!(
            split_front_matter("---\nk: [1, 2020-01-02]\n---\n")
                .unwrap()
                .0["k"],
            Value::Array(Arc::new(vec![n(1.0), date("2020-01-02T00:00:00Z")]))
        );
    }

    /// Probed with 11ty 3.1.8: `0123`, `08` and `02134` print as `123`, `8`
    /// and `2134`; `0777` is decimal 777; `0x1F` is 31; `0o17` is 15.
    #[test]
    fn integers_like_js_yaml() {
        assert_eq!(v("0123"), n(123.0));
        assert_eq!(v("08"), n(8.0));
        assert_eq!(v("02134"), n(2134.0));
        assert_eq!(v("0777"), n(777.0));
        assert_eq!(v("0"), n(0.0));
        assert_eq!(v("00"), n(0.0));
        assert_eq!(v("0x1F"), n(31.0));
        assert_eq!(v("-0x1F"), n(-31.0));
        assert_eq!(v("0o17"), n(15.0));
        assert_eq!(v("0b101"), n(5.0));
        assert_eq!(v("+12"), n(12.0));
        assert_eq!(v("-08"), n(-8.0));
        assert_eq!(v("99999999999999999999"), n(1e20));
        // Not integers.
        assert_eq!(v("1_000"), s("1_000"));
        assert_eq!(v("1,000"), s("1,000"));
        assert_eq!(v("0b2"), s("0b2"));
        assert_eq!(v("0x"), s("0x"));
        assert_eq!(v("12:30:45"), s("12:30:45"));
        assert_eq!(v("'0123'"), s("0123"));
        // `-0` is read as plain 0.
        assert!(matches!(v("-0"), Value::Number(x) if x == 0.0 && x.is_sign_positive()));
    }

    /// Probed with 11ty 3.1.8: `.inf` prints `Infinity`, `.nan` prints `NaN`.
    #[test]
    fn floats_like_js_yaml() {
        assert_eq!(v(".inf"), n(f64::INFINITY));
        assert_eq!(v(".Inf"), n(f64::INFINITY));
        assert_eq!(v("+.INF"), n(f64::INFINITY));
        assert_eq!(v("-.inf"), n(f64::NEG_INFINITY));
        assert_eq!(v("-.INF"), n(f64::NEG_INFINITY));
        assert!(matches!(v(".nan"), Value::Number(x) if x.is_nan()));
        assert!(matches!(v(".NaN"), Value::Number(x) if x.is_nan()));
        assert_eq!(v("1e3"), n(1000.0));
        assert_eq!(v("1.5e3"), n(1500.0));
        assert_eq!(v(".5"), n(0.5));
        assert_eq!(v("5."), n(5.0));
        assert_eq!(v("09.5"), n(9.5));
        // Not floats.
        assert_eq!(v("+.5"), s("+.5"));
        assert_eq!(v("-.nan"), s("-.nan"));
        assert_eq!(v("1e400"), s("1e400"));
    }

    /// Probed with 11ty 3.1.8: only `~`, `null`, `Null` and `NULL` are null and
    /// only `true`/`false` in three spellings are booleans. `yes`, `no`, `on`
    /// and `off` are strings.
    #[test]
    fn null_and_booleans_like_js_yaml() {
        for text in ["~", "null", "Null", "NULL", ""] {
            assert_eq!(v(text), Value::Null, "{text:?}");
        }
        assert_eq!(v("true"), Value::Bool(true));
        assert_eq!(v("True"), Value::Bool(true));
        assert_eq!(v("FALSE"), Value::Bool(false));
        for text in ["yes", "no", "on", "off", "Yes", "y", "tRue", "nULL"] {
            assert_eq!(v(text), s(text), "{text}");
        }
        assert_eq!(v("'null'"), s("null"));
        assert_eq!(v("\"true\""), s("true"));
    }

    #[test]
    fn structure_like_js_yaml() {
        let load = |t: &str| split_front_matter(&format!("---\n{t}\n---\n")).map(|(m, _)| m);
        // Duplicate keys are an error.
        assert!(load("a: 1\na: 2").is_err());
        // Merge keys: explicit keys win, in either order.
        let m = load("base: &b {x: 1, y: 2}\nm:\n  <<: *b\n  y: 3").unwrap();
        let Value::Object(o) = &m["m"] else { panic!() };
        assert_eq!(o.keys().collect::<Vec<_>>(), ["x", "y"]);
        assert_eq!(o["y"], n(3.0));
        let m = load("base: &b {x: 1, y: 2}\nm:\n  y: 3\n  <<: *b").unwrap();
        let Value::Object(o) = &m["m"] else { panic!() };
        assert_eq!(o.keys().collect::<Vec<_>>(), ["y", "x"]);
        assert_eq!(o["y"], n(3.0));
        // Aliases copy the anchored value; non-string keys become strings.
        let m = load("a: &x [1, 2]\nb: *x\n1: one\ntrue: t").unwrap();
        assert_eq!(m["a"], m["b"]);
        assert_eq!(m["1"], s("one"));
        assert_eq!(m["true"], s("t"));
        // Block scalars and `!!str` are strings.
        let m = load("a: |\n  x\nb: !!str 123").unwrap();
        assert_eq!(m["a"], s("x\n"));
        assert_eq!(m["b"], s("123"));
        // Unknown tags are refused.
        assert!(load("a: !custom x").is_err());
        // A second document is an error, as in js-yaml.
        assert!(crate::yaml::load("a: 1\n---\nb: 2").is_err());
    }

    #[test]
    fn empty_front_matter_and_trailing_whitespace_delimiter() {
        let (m, body) = split_front_matter("---  \n---\nx").unwrap();
        assert!(m.is_empty());
        assert_eq!(body, "x");
    }

    #[test]
    fn unclosed_front_matter_is_an_error() {
        let p = split_front_matter("---\ntitle: x\n").unwrap_err();
        assert_eq!(p.code, codes::FRONT_MATTER_INVALID);
        assert_eq!(p.line, Some(1));
        assert!(p.message.contains("closing"));
    }

    #[test]
    fn non_mapping_yaml_is_empty_data() {
        let (m, body) = split_front_matter("---\n- a\n- b\n---\nlist fm\n").unwrap();
        assert!(m.is_empty());
        assert_eq!(body, "list fm\n");
        let (m, body) = split_front_matter("---\njust a scalar\n---\nx").unwrap();
        assert!(m.is_empty());
        assert_eq!(body, "x");
    }

    #[test]
    fn bom_is_stripped() {
        let (m, body) = split_front_matter("\u{feff}---\ntitle: bom\n---\nbom body").unwrap();
        assert_eq!(m["title"], s("bom"));
        assert_eq!(body, "bom body");
        let (m, body) = split_front_matter("\u{feff}plain").unwrap();
        assert!(m.is_empty());
        assert_eq!(body, "plain");
    }

    #[test]
    fn closing_delimiter_without_body_newline() {
        let (m, body) = split_front_matter("---\na: 1\n---").unwrap();
        assert_eq!(m["a"], n(1.0));
        assert_eq!(body, "");
    }

    /// Nesting is capped so a hostile file is a Problem, not a stack overflow.
    #[test]
    fn deep_nesting_is_a_problem_not_an_abort() {
        let run = || {
            let mut block = String::from("---\n");
            for i in 0..10_000 {
                block.push_str(&"  ".repeat(i));
                block.push_str("k:\n");
            }
            let mut seq = String::from("---\nk:\n");
            for i in 0..10_000 {
                seq.push_str(&" ".repeat(i + 1));
                seq.push_str("-\n");
            }
            let flow = format!("---\nk: {}{}\n", "[".repeat(10_000), "]".repeat(10_000));
            for text in [block, seq, flow] {
                let p = split_front_matter(&format!("{text}---\n")).unwrap_err();
                assert_eq!(p.code, codes::FRONT_MATTER_INVALID);
                assert!(p.line.is_some());
            }
        };
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(run)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn depth_cap_allows_128_levels() {
        let nested = |n: usize| {
            let mut t = String::from("---\n");
            for i in 0..n {
                t.push_str(&"  ".repeat(i));
                t.push_str("k:\n");
            }
            t.push_str(&"  ".repeat(n));
            t.push_str("v: 1\n---\n");
            split_front_matter(&t).map(|_| ())
        };
        assert!(nested(127).is_ok());
        assert!(nested(128).is_err());
        let flow = |n: usize| {
            split_front_matter(&format!(
                "---\nk: {}{}\n---\n",
                "[".repeat(n),
                "]".repeat(n)
            ))
            .map(|_| ())
        };
        assert!(flow(127).is_ok());
        assert!(flow(128).is_err());
    }

    /// js-yaml hangs on `? *alias` (11ty never finishes); Brrrty refuses it.
    #[test]
    fn alias_as_mapping_key_is_refused_without_hanging() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = split_front_matter("---\na: &a x\n? *a\n: y\n---\n");
            let _ = tx.send(r.map(|_| ()));
        });
        let r = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("timed out");
        let p = r.unwrap_err();
        assert_eq!(p.code, codes::FRONT_MATTER_INVALID);
        assert!(p.message.contains("alias"), "{}", p.message);
    }
}
