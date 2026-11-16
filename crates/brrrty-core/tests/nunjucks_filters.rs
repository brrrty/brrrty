//! The filters Brrrty registers in the Nunjucks engine: Eleventy's, the
//! config's, and the RSS plugin's.
//!
//! `probes/nunjucks_filters.json` (made by `tools/gen-nunjucks-filter-probes.cjs`)
//! holds what Nunjucks 3.2.4 prints for the filters Eleventy and
//! `@11ty/eleventy-plugin-rss` add to it.

use std::sync::Arc;

use brrrty_core::{ConfigFilter, register_nunjucks_filters};
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
}

struct NoIncludes;

impl Loader for NoIncludes {
    fn load(&self, _: &str) -> Result<(String, String), LoadError> {
        Err(Default::default())
    }
}

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

fn engine(config: &[ConfigFilter], rss: bool) -> Engine {
    let mut e = Engine::new(EngineOptions {
        timezone: Tz::Utc,
        autoescape: true,
    });
    register_nunjucks_filters(&mut e, config, rss);
    e
}

fn render(
    e: &Engine,
    template: &str,
    data: &Value,
) -> Result<String, brrrty_nunjucks::NunjucksError> {
    e.compile("t", template)
        .and_then(|t| e.render(&t, data, &NoIncludes))
}

const REFUSED: &[&str] = &["`emptyFallbackDate`", "in `dateToRfc822`"];

#[test]
fn eleventy_and_rss_filters_match_nunjucks() {
    let file: File = serde_json::from_str(include_str!("probes/nunjucks_filters.json")).unwrap();
    let data = value(&file.data);
    let e = engine(&[], true);
    let mut failures = Vec::new();
    for p in &file.probes {
        match (&p.out, &p.err, render(&e, &p.t, &data)) {
            (Some(want), _, Ok(got)) if *want == got => {}
            // Precise refusals: a SafeString fallback argument that would
            // have to stay the same object, and a date string for RFC 822.
            (_, _, Err(err))
                if err.kind == ErrorKind::Unsupported
                    && REFUSED.iter().any(|r| err.message.contains(r)) => {}
            (None, Some(want), Err(err)) => {
                // Eleventy wraps what its filters throw.
                let want = want
                    .strip_prefix("TypeError: ")
                    .or_else(|| want.strip_prefix("Error: "))
                    .unwrap_or(want);
                if !err.message.contains(want) {
                    failures.push(format!("{:?}: want error {want:?}, got {err}", p.t));
                }
            }
            (want, werr, got) => {
                failures.push(format!("{:?}: want {want:?} / {werr:?}, got {got:?}", p.t))
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn rss_filters_are_in_nunjucks_only_when_the_plugin_is_on() {
    let data = value(&serde_json::json!({"d": "__DATE__2020-05-05T10:20:30.123Z"}));
    let off = engine(&[], false);
    let err = render(&off, "{{ d | dateToRfc3339 }}", &data).unwrap_err();
    assert!(
        err.message.contains("filter not found: dateToRfc3339"),
        "{err}"
    );
    let on = engine(&[], true);
    assert_eq!(
        render(&on, "{{ d | dateToRfc3339 }}", &data).unwrap(),
        "2020-05-05T10:20:30Z"
    );
}

#[test]
fn html_to_absolute_urls_is_refused_as_an_async_filter() {
    let e = engine(&[], true);
    let err = render(
        &e,
        "{{ '<a href=\"/x\">y</a>' | htmlToAbsoluteUrls('https://e.com') }}",
        &Value::Null,
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported, "{err}");
    assert!(err.message.contains("async filter"), "{err}");
}

#[test]
fn config_filters_work_and_replace_nunjucks_ones() {
    let upcase: brrrty_liquid::ArgsFilterFn = Arc::new(|input, args, _| {
        Ok(Value::String(
            format!("<{}:{}>", input.to_js_string(), args.len()).into(),
        ))
    });
    let cfg = [
        ConfigFilter {
            name: "mine".into(),
            run: upcase.clone(),
        },
        ConfigFilter {
            name: "upper".into(),
            run: upcase,
        },
    ];
    let e = engine(&cfg, false);
    let data = value(&serde_json::json!({"s": "a"}));
    // Keyword arguments arrive as a last object argument, so they count.
    assert_eq!(render(&e, "{{ s | mine }}", &data).unwrap(), "&lt;a:0&gt;");
    assert_eq!(
        render(&e, "{{ s | mine(1, 2) }}", &data).unwrap(),
        "&lt;a:2&gt;"
    );
    assert_eq!(
        render(&e, "{{ s | mine(k=1) }}", &data).unwrap(),
        "&lt;a:1&gt;"
    );
    // `addFilter` replaces Nunjucks's own `upper`.
    assert_eq!(render(&e, "{{ s | upper }}", &data).unwrap(), "&lt;a:0&gt;");
    // A SafeString can't be told from a string by such a filter: refused.
    let err = render(&e, "{{ (s | safe) | mine }}", &data).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported, "{err}");
    let err = render(&e, "{{ s | mine(s | safe) }}", &data).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported, "{err}");
    // Eleventy's own are there.
    assert_eq!(render(&e, "{{ 'A B' | slugify }}", &data).unwrap(), "a-b");
}
