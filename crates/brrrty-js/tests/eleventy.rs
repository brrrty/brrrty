//! Eleventy's filters against probes made by running the real ones
//! (`tools/gen-eleventy.mjs`, Node v26.2.0, Eleventy 3.1.8).

use brrrty_js::{FilterEnv, FilterImpl, Tz, Value, console_format, eleventy_filters};
use serde_json::Value as Json;

fn probes() -> Json {
    serde_json::from_str(include_str!("probes/eleventy.json")).unwrap()
}

/// `{"$undefined": true}` stands for `undefined`.
fn revive(j: &Json) -> Value {
    match j.as_object() {
        Some(o) if o.len() == 1 && o.contains_key("$undefined") => Value::Undefined,
        _ => Value::from_json(j),
    }
}

fn filter(name: &str) -> FilterImpl {
    eleventy_filters()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1
}

fn check(name: &str, min: usize) {
    let f = filter(name);
    let probes = probes();
    let cases = probes[name].as_array().unwrap();
    assert!(cases.len() >= min, "{name}: only {} probes", cases.len());
    let mut failures = Vec::new();
    for c in cases {
        let input = revive(&c["in"]);
        let args: Vec<Value> = c["args"].as_array().unwrap().iter().map(revive).collect();
        let got = f(&input, &args, &FilterEnv::new(Tz::Utc));
        let ok = match (c.get("out"), c.get("err")) {
            (Some(out), _) => got.as_ref().is_ok_and(|g| *g == revive(out)),
            (_, Some(err)) => got.as_ref().is_err_and(|e| e == err.as_str().unwrap()),
            _ => panic!("probe has neither out nor err"),
        };
        if !ok {
            failures.push(format!("{} {:?}: want {c}, got {got:?}", c["in"], args));
        }
    }
    assert!(failures.is_empty(), "{name}:\n{}", failures.join("\n"));
}

#[test]
fn slug_matches_eleventy() {
    check("slug", 20);
}

#[test]
fn slugify_matches_eleventy() {
    check("slugify", 20);
}

#[test]
fn url_matches_eleventy() {
    check("url", 20);
}

#[test]
fn log_formats_like_console_log() {
    let probes = probes();
    let cases = probes["log"].as_array().unwrap();
    assert!(cases.len() >= 20);
    for c in cases {
        let args: Vec<Value> = c["args"].as_array().unwrap().iter().map(revive).collect();
        assert_eq!(console_format(&args), c["out"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn log_returns_its_input() {
    let input = Value::from_json(&serde_json::json!({"a": [1, 2]}));
    let got = filter("log")(&input, &[Value::Null], &FilterEnv::new(Tz::Utc)).unwrap();
    assert_eq!(got, input);
}

fn date(ms: i64) -> Value {
    Value::Date(
        chrono::DateTime::from_timestamp_millis(ms)
            .unwrap()
            .fixed_offset(),
    )
}

/// Eleventy stringifies a `Date` as JavaScript does, with the zone's long
/// name. node, TZ=UTC, with `@11ty/eleventy/src/Filters/{Slug,Slugify,Url}.js`:
/// `slug(new Date(Date.UTC(2026, 2, 4)))` and so on. The same probes under
/// TZ=America/New_York are pinned by the CLI tests, which can set `TZ`.
#[test]
fn a_date_is_stringified_with_its_zone_name() {
    let env = FilterEnv::new(Tz::Utc);
    let cases = [
        (
            1_772_582_400_000,
            "wed-mar-04-2026-00:00:00-gmt+0000-(coordinated-universal-time)",
            "wed-mar-04-2026-00-00-00-gmt-0000-coordinated-universal-time",
        ),
        (
            1_783_168_205_000,
            "sat-jul-04-2026-12:30:05-gmt+0000-(coordinated-universal-time)",
            "sat-jul-04-2026-12-30-05-gmt-0000-coordinated-universal-time",
        ),
    ];
    for (ms, slug, slugify) in cases {
        let d = date(ms);
        assert_eq!(
            filter("slug")(&d, &[], &env),
            Ok(Value::String(slug.into()))
        );
        assert_eq!(
            filter("slugify")(&d, &[], &env),
            Ok(Value::String(slugify.into()))
        );
        assert_eq!(
            filter("url")(&d, &[], &env),
            Err("url.startsWith is not a function".to_string())
        );
    }
    // Inside an array, as `String([date, "x"])` prints it.
    let arr = Value::Array(std::sync::Arc::new(vec![
        date(1_772_582_400_000),
        Value::String("x".into()),
    ]));
    assert_eq!(
        filter("slug")(&arr, &[], &env),
        Ok(Value::String(
            "wed-mar-04-2026-00:00:00-gmt+0000-(coordinated-universal-time)x".into()
        ))
    );
    assert_eq!(
        filter("slugify")(&arr, &[], &env),
        Ok(Value::String(
            "wed-mar-04-2026-00-00-00-gmt-0000-coordinated-universal-time-x".into()
        ))
    );
}
