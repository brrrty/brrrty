//! Filters pinned against Nunjucks 3.2.4.
//!
//! `probes/filters.json` (made by `tools/gen-filter-probes.cjs`) holds
//! templates that apply every built-in filter, and Eleventy's own, to
//! values of every type, rendered by Nunjucks as 11ty configures it. Each
//! is rendered twice, printed and through `dump`, so the file shows both
//! the escaping and whether a result is a `SafeString`.
//!
//! Brrrty must print the same output. Where Nunjucks fails, Brrrty must
//! fail with the same message, unless it refuses (`ErrorKind::Unsupported`
//! or `Limit`). A refusal of something Nunjucks renders must match one of
//! [`REFUSALS`], and an output that splits a character must be refused.

use std::collections::BTreeMap;
use std::sync::Arc;

use brrrty_js::Value;
use brrrty_nunjucks::{Engine, EngineOptions, ErrorKind, LoadError, Loader, Tz};
use serde::Deserialize;

#[derive(Deserialize)]
struct File {
    data: serde_json::Value,
    probes: Vec<Probe>,
}

#[derive(Deserialize)]
struct Probe {
    t: String,
    out: Option<String>,
    err: Option<String>,
    #[serde(default)]
    lone_surrogate: bool,
}

/// Why Brrrty refuses what Nunjucks renders: a part of the refusal's
/// message, and the reason.
const REFUSALS: &[(&str, &str)] = &[
    (
        "the `random` filter picks at random",
        "output would differ from run to run",
    ),
    (
        "the order would depend on how V8 sorts",
        "comparisons that aren't a consistent order depend on V8's TimSort",
    ),
    (
        "outside the Basic Multilingual Plane",
        "JavaScript splits such a character in two; Rust strings can't hold half of one",
    ),
    (
        "split a character",
        "the result would hold half of a character",
    ),
    (
        "which Brrrty doesn't print yet",
        "a date in a year the plugin doesn't print",
    ),
    (
        "inside its input",
        "a SafeString inside an array or object given to a user filter",
    ),
    (
        "Brrrty doesn't support options for the",
        "slug and slugify with an options object",
    ),
    (
        "can't be printed or turned into a string or number",
        "Nunjucks prints a function's source",
    ),
    (
        "a function can't be printed",
        "Nunjucks prints a function's source",
    ),
    (
        "dump` of a value that contains itself",
        "Nunjucks fails with V8's circular structure message",
    ),
    (
        "`dictsort` of a SafeString",
        "Nunjucks lists a SafeString's inherited properties",
    ),
    ("`toJSON` function", "Nunjucks would call the function"),
    ("items aren't pairs", "V8's destructuring message"),
    (
        "digit string too long",
        "parseInt of a long digit string in an uncommon way",
    ),
    (
        "base that isn't 2, 4, 8, 10, 16 or 32",
        "parseInt rounds large numbers in odd bases its own way",
    ),
    (
        "`replace` with an empty search on text with characters outside",
        "JavaScript splits such a character in two",
    ),
    ("isn't a whole number", "a list whose length is fractional"),
];

struct NoIncludes;

impl Loader for NoIncludes {
    fn load(&self, _name: &str) -> Result<(String, String), LoadError> {
        Err(Default::default())
    }
}

/// JSON with `__DATE__...` strings made dates, as the generator does.
fn value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::String(s) if s.starts_with("__DATE__") => {
            Value::Date(chrono::DateTime::parse_from_rfc3339(&s[8..]).expect("a probe date"))
        }
        serde_json::Value::Array(a) => Value::Array(Arc::new(a.iter().map(value).collect())),
        serde_json::Value::Object(o) => Value::Object(Arc::new(
            o.iter().map(|(k, v)| (k.clone(), value(v))).collect(),
        )),
        other => Value::from_json(other),
    }
}

fn engine() -> Engine {
    let mut engine = Engine::new(EngineOptions {
        timezone: Tz::Utc,
        autoescape: true,
    });
    engine.register_eleventy_filters();
    engine
}

#[test]
fn filters_match_nunjucks() {
    check(include_str!("probes/filters.json"));
}

/// The same check on a file made elsewhere, such as a large generated
/// run: `NUNJUCKS_FILTER_PROBES=path cargo test -- --ignored`.
#[test]
#[ignore]
fn filter_probes_from_a_file_match_nunjucks() {
    let path = std::env::var("NUNJUCKS_FILTER_PROBES").expect("set NUNJUCKS_FILTER_PROBES");
    check(&std::fs::read_to_string(path).unwrap());
}

fn check(json: &str) {
    let file: File = serde_json::from_str(json).unwrap();
    let data = value(&file.data);
    let engine = engine();
    let mut failures = Vec::new();
    let mut refused: BTreeMap<&str, usize> = BTreeMap::new();
    for p in &file.probes {
        let got = engine
            .compile("t", &p.t)
            .and_then(|t| engine.render(&t, &data, &NoIncludes));
        if p.lone_surrogate {
            if !matches!(&got, Err(e) if e.kind == ErrorKind::Unsupported) {
                failures.push(format!("{:?}: splits a character; want a refusal", p.t));
            }
            continue;
        }
        match (&p.out, &p.err, got) {
            (Some(want), _, Ok(got)) if *want == got => {}
            (Some(want), _, Ok(got)) => {
                failures.push(format!("{:?}: want {want:?}, got {got:?}", p.t))
            }
            (Some(want), _, Err(e)) => {
                let reason = REFUSALS
                    .iter()
                    .find(|(m, _)| e.message.contains(m))
                    .filter(|_| matches!(e.kind, ErrorKind::Unsupported | ErrorKind::Limit));
                match reason {
                    Some((m, _)) => *refused.entry(m).or_default() += 1,
                    None => failures.push(format!("{:?}: want {want:?}, got error {e}", p.t)),
                }
            }
            (None, Some(want), Err(e)) => {
                let refusal = matches!(e.kind, ErrorKind::Unsupported | ErrorKind::Limit);
                // Eleventy wraps what its filters throw.
                let wrapped = e.message.starts_with("Error in Nunjucks Filter `")
                    && e.message
                        .ends_with(want.strip_prefix("TypeError: ").unwrap_or(want));
                if !refusal && !wrapped && e.message != *want {
                    failures.push(format!(
                        "{:?}: want error {want:?}, got {:?}",
                        p.t, e.message
                    ));
                }
                if refusal {
                    *refused.entry("(refused where Nunjucks fails)").or_default() += 1;
                }
            }
            (None, _, Ok(got)) => {
                failures.push(format!("{:?}: want error {:?}, got {got:?}", p.t, p.err))
            }
            (None, None, Err(_)) => unreachable!("a probe has out or err"),
        }
    }
    eprintln!("{} probes; refusals by reason:", file.probes.len());
    for (reason, n) in &refused {
        eprintln!("  {n:5}  {reason}");
    }
    assert!(
        failures.is_empty(),
        "{} of {} probes differ:\n{}",
        failures.len(),
        file.probes.len(),
        failures
            .iter()
            .take(150)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Escapes and parses the probe corpus doesn't reach. Expected values are what Nunjucks 3.2.4
/// printed with `autoescape: true` for the same templates and data.
#[test]
fn dump_escapes_and_int_signs_match_nunjucks() {
    let engine = engine();
    let o = obj(&[
        ("a", Value::Undefined),
        (
            "b",
            Value::Array(Arc::new(vec![Value::Undefined, Value::Null])),
        ),
        ("c", Value::Number(1.0)),
    ]);
    let s = Value::String("a\u{8}b\u{c}c\td\ne\"f\\g\u{1}".into());
    let data = obj(&[("s", s), ("o", o)]);
    let render = |t: &str| {
        let t = engine.compile("t", t).unwrap();
        engine.render(&t, &data, &NoIncludes)
    };
    for (t, want) in [
        ("{{ s | dump | safe }}", r#""a\bb\fc\td\ne\"f\\g\u0001""#),
        ("{{ o | dump | safe }}", r#"{"b":[null,null],"c":1}"#),
        ("{{ \"+12\" | int }}", "12"),
        ("{{ \"0x1F\" | int }}", "0"),
        ("{{ \"-0x1F\" | int }}", "0"),
        ("{{ \" 42abc\" | int }}", "42"),
        ("{{ \"+3.9\" | float }}", "3.9"),
    ] {
        assert_eq!(render(t).unwrap(), want, "{t}");
    }
}

fn obj(pairs: &[(&str, Value)]) -> Value {
    Value::Object(Arc::new(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    ))
}

/// Nunjucks 3.2.4 ends its message with "Error: filter not found: nope"; Brrrty gives that line,
/// and suggests the closest name when one is near.
#[test]
fn an_unknown_filter_is_nunjucks_s_error_with_a_suggestion() {
    let engine = engine();
    let fail = |t: &str| {
        engine
            .compile("t", t)
            .and_then(|t| engine.render(&t, &Value::Undefined, &NoIncludes))
            .unwrap_err()
    };
    let err = fail("{{ x | nope }}");
    assert_eq!(err.message, "Error: filter not found: nope");
    let err = fail("{{ x | uppr }}");
    assert_eq!(err.message, "Error: filter not found: uppr");
    assert_eq!(err.help.as_deref(), Some("did you mean `upper`?"));
}
