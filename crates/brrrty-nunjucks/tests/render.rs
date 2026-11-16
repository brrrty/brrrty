//! Renders pinned against Nunjucks 3.2.4.
//!
//! `probes/render.json` holds templates rendered by Nunjucks as 11ty runs
//! it (made by `tools/gen-render-probes.cjs`): the output, or the message
//! of the error. Brrrty must print the same output. Where Nunjucks fails,
//! Brrrty must fail too, with the same message unless it refuses
//! (`ErrorKind::Unsupported` or `Limit`). Where Nunjucks succeeds and
//! Brrrty refuses, the template must be listed in [`REFUSED`]; and an
//! output that splits a character in two must be refused.

use std::collections::BTreeMap;
use std::sync::Arc;

use brrrty_js::Value;
use brrrty_nunjucks::{Engine, EngineOptions, ErrorKind, LoadError, Loader, Tz};
use serde::Deserialize;

#[derive(Deserialize)]
struct Probe {
    template: String,
    data: serde_json::Value,
    includes: BTreeMap<String, String>,
    out: Option<String>,
    err: Option<String>,
    /// Nunjucks's output splits a character. Brrrty refuses those.
    #[serde(default)]
    lone_surrogate: bool,
}

/// Templates Nunjucks renders that Brrrty refuses on purpose, and why.
const REFUSED: &[(&str, &str)] = &[
    ("{{ a.push(1) }}", "push would change the build's data"),
    (
        "{{ a.map }}",
        "Nunjucks prints the function's JavaScript source",
    ),
    ("{{ a.sort() }}", "sort is not emulated"),
    (
        "{{ range }}",
        "Nunjucks prints the function's JavaScript source",
    ),
    (
        "{% macro m() %}M{% endmacro %}{{ m }}",
        "Nunjucks prints the function's JavaScript source",
    ),
    (
        "{% macro outer() %}{% macro inner() %}I{% endmacro %}{{ inner() }}{% endmacro %}{{ outer() }}[{{ inner }}]",
        "Nunjucks prints the function's JavaScript source",
    ),
    (
        "{% import \"m.njk\" as m %}{{ m }}|{{ m.f }}",
        "Nunjucks prints the function's JavaScript source",
    ),
    (
        "{{ constructor }}",
        "Object.prototype's functions as variables",
    ),
    (
        "{{ toString }}",
        "Object.prototype's functions as variables",
    ),
    (
        "{{ x.constructor }}",
        "Nunjucks prints the function's JavaScript source",
    ),
    ("{{ x.__proto__ }}", "__proto__ is not emulated"),
    (
        "{{ s.toUpperCase }}",
        "Nunjucks prints the function's JavaScript source",
    ),
    ("{{ d == d }}", "a date's identity is not known"),
    (
        "{% set re = r/b/g %}{{ re.test(\"abc\") }}|{{ re.lastIndex }}",
        "lastIndex isn't tracked after a global test() matched",
    ),
    (
        "{% for k in r/x/ %}{{ k }}{% else %}E{% endfor %}",
        "looping over a regular expression",
    ),
    ("{{ \"abc\".match(r/b/) }}", "match is not emulated"),
    (
        "{{ [r/b/, r/a/] | sort | join(\",\") }}",
        "sort of values that aren't all numbers or all strings",
    ),
    (
        "{{ s.replace(r/(?=o)/g, \"-\") }}",
        "the regex crate has no lookaround",
    ),
];

struct MapLoader<'a>(&'a BTreeMap<String, String>);

impl Loader for MapLoader<'_> {
    fn load(&self, name: &str) -> Result<(String, String), LoadError> {
        self.0
            .get(name)
            .map(|s| (name.to_string(), s.clone()))
            .ok_or_else(Default::default)
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

#[test]
fn renders_match_nunjucks() {
    check(include_str!("probes/render.json"));
}

/// The same check on a file of probes made elsewhere, such as a large
/// generated run: `NUNJUCKS_RENDER_PROBES=path cargo test -- --ignored`.
#[test]
#[ignore]
fn render_probes_from_a_file_match_nunjucks() {
    let path = std::env::var("NUNJUCKS_RENDER_PROBES").expect("set NUNJUCKS_RENDER_PROBES");
    check(&std::fs::read_to_string(path).unwrap());
}

fn check(json: &str) {
    let file = !json.is_empty() && std::env::var("NUNJUCKS_RENDER_PROBES").is_ok();
    let probes: Vec<Probe> = serde_json::from_str(json).unwrap();
    let engine = Engine::new(EngineOptions {
        timezone: Tz::Utc,
        autoescape: true,
    });
    let mut failures = Vec::new();
    let mut refused_seen = Vec::new();
    for p in &probes {
        let data = value(&p.data);
        let got = engine
            .compile("t", &p.template)
            .and_then(|t| engine.render(&t, &data, &MapLoader(&p.includes)));
        let refused = REFUSED.iter().find(|(t, _)| *t == p.template);
        if p.lone_surrogate {
            if !matches!(&got, Err(e) if e.kind == ErrorKind::Unsupported) {
                failures.push(format!(
                    "{:?}: splits a character; want a refusal",
                    p.template
                ));
            }
            continue;
        }
        match (&p.out, &p.err, got) {
            (Some(want), _, Ok(got)) if *want == got => {}
            (Some(want), _, Ok(got)) => {
                failures.push(format!("{:?}: want {want:?}, got {got:?}", p.template))
            }
            (Some(_), _, Err(e)) if refused.is_some() && e.kind == ErrorKind::Unsupported => {
                refused_seen.push(p.template.clone());
            }
            (Some(want), _, Err(e)) => {
                failures.push(format!("{:?}: want {want:?}, got error {e}", p.template))
            }
            (None, Some(want), Err(e)) => {
                let refusal = matches!(e.kind, ErrorKind::Unsupported | ErrorKind::Limit);
                if !refusal && e.message != *want {
                    failures.push(format!(
                        "{:?}: want error {want:?}, got {:?}",
                        p.template, e.message
                    ));
                }
            }
            (None, _, Ok(got)) => failures.push(format!(
                "{:?}: want error {:?}, got {got:?}",
                p.template, p.err
            )),
            (None, None, Err(_)) => unreachable!("a probe has out or err"),
        }
    }
    for (t, _) in REFUSED.iter().filter(|_| !file) {
        assert!(
            refused_seen.iter().any(|s| s == t),
            "{t:?} is listed as refused but isn't"
        );
    }
    eprintln!("{} probes, {} refused", probes.len(), refused_seen.len());
    assert!(
        failures.is_empty(),
        "{} of {} probes differ:\n{}",
        failures.len(),
        probes.len(),
        failures.join("\n")
    );
}
