//! The standard filters against LiquidJS 10.30.1.
//!
//! `probes/filters.json` holds templates run through LiquidJS with
//! `strictFilters: true`, with what each printed or the error it raised.
//!
//! - `plain` and `dates_utc` ran with `TZ=UTC` and are checked with both
//!   `Tz::Utc` and `Tz::Local` (in a child process with `TZ=UTC`).
//! - `zones` holds probes that depend on the time zone, each run with the
//!   `TZ` named by its key. They are checked with `Tz::Local` in a child
//!   process that has that `TZ`, because the zone can only be chosen
//!   before a process reads it.
//!
//! Data objects of the form `{"$date": "..."}` are `Date`s.

use std::collections::BTreeMap;
use std::process::Command;

use brrrty_js::Value;
use brrrty_liquid::{Engine, EngineOptions, Loader, Tz};
use serde::Deserialize;

#[derive(Deserialize)]
struct Probe {
    src: String,
    data: Option<serde_json::Value>,
    out: Option<String>,
    err: Option<String>,
}

#[derive(Deserialize)]
struct Zone {
    zonal: Vec<Probe>,
    #[serde(default)]
    zonal_parse: Vec<Probe>,
}

#[derive(Deserialize)]
struct File {
    data: serde_json::Value,
    plain: Vec<Probe>,
    dates_utc: Vec<Probe>,
    zones: BTreeMap<String, Zone>,
}

fn file() -> File {
    serde_json::from_str(include_str!("probes/filters.json")).unwrap()
}

fn revive(v: &serde_json::Value) -> Value {
    use std::sync::Arc;
    match v {
        serde_json::Value::Object(o) if o.len() == 1 && o.contains_key("$date") => {
            let text = o["$date"].as_str().unwrap();
            Value::Date(chrono::DateTime::parse_from_rfc3339(text).unwrap())
        }
        serde_json::Value::Object(o) => Value::Object(Arc::new(
            o.iter().map(|(k, v)| (k.clone(), revive(v))).collect(),
        )),
        serde_json::Value::Array(a) => Value::Array(Arc::new(a.iter().map(revive).collect())),
        other => Value::from_json(other),
    }
}

struct NoPartials;

impl Loader for NoPartials {
    fn load(&self, _: &str) -> Option<(String, String)> {
        None
    }
}

/// `message, line:N, col:M` into the message and position.
fn split_position(text: &str) -> (&str, Option<(usize, usize)>) {
    let Some(at) = text.rfind(", line:") else {
        return (text, None);
    };
    let (line, col) = text[at + 7..].split_once(", col:").unwrap();
    (
        &text[..at],
        Some((line.parse().unwrap(), col.parse().unwrap())),
    )
}

/// Where Brrrty knowingly differs from LiquidJS.
///
/// Rust strings cannot hold a lone surrogate. JavaScript's `split('')` cuts
/// an emoji into its two UTF-16 halves, and printing the array joins them
/// back into the emoji; `first` of an emoji is a lone half, which
/// `JSON.stringify` writes as `\ud83d`. Brrrty gives each half as U+FFFD,
/// which is also what writing a lone half to a file produces, so the
/// difference shows only when the halves are put back together.
///
/// A `nil` group key is a drop in LiquidJS and `JSON.stringify` writes it as
/// `{}`; Brrrty's is null.
fn known_deviation(src: &str, zone: &str) -> bool {
    // Far outside the years the system's zone rules cover (before 1883 and
    // after 2037), a zone other than UTC has the offsets ICU gives and the
    // system does not.
    let extreme = !zone.to_lowercase().contains("utc")
        && ["275760", "271821", "8640000000000"]
            .iter()
            .any(|y| src.contains(y));
    let split_print = (src.starts_with("{{ emoji | split") || src.starts_with("{{ mixed | split"))
        && !src.contains("join")
        && !src.contains("size");
    let replace_all_units = (src.starts_with("{{ emoji | replace:")
        || src.starts_with("{{ mixed | replace:"))
        && src.contains("nul");
    extreme
        || split_print
        || replace_all_units
        || (src.contains("first | json") && (src.contains("emoji") || src.contains("😀")))
        || src == "{{ people | group_by_exp: 'p', 'nil' | json }}"
}

/// Runs `probes` and returns one line per mismatch.
fn check(probes: &[Probe], shared: &serde_json::Value, tz: Tz, label: &str) -> Vec<String> {
    let engine = Engine::new(EngineOptions {
        strict_filters: true,
        timezone: tz,
    });
    let shared = revive(shared);
    let mut failures = Vec::new();
    for p in probes {
        if known_deviation(&p.src, label) {
            continue;
        }
        let data = p.data.as_ref().map_or_else(|| shared.clone(), revive);
        let got = engine
            .parse("probe", &p.src)
            .and_then(|t| engine.render(&t, &data, &NoPartials));
        match (&got, &p.out, &p.err) {
            (Ok(got), Some(want), None) => {
                if got != want {
                    failures.push(format!("[{label}] {:?}: want {want:?}, got {got:?}", p.src));
                }
            }
            (Err(e), None, Some(err)) => {
                let (msg, pos) = split_position(err);
                let pos = pos.unwrap_or((0, 0));
                if (e.message.as_str(), e.line, e.column) != (msg, pos.0, pos.1) {
                    failures.push(format!(
                        "[{label}] {:?}: want {msg:?} at {pos:?}, got {:?} at {}:{}",
                        p.src, e.message, e.line, e.column
                    ));
                }
            }
            (Ok(got), _, Some(err)) => failures.push(format!(
                "[{label}] {:?}: LiquidJS fails ({err}), we print {got:?}",
                p.src
            )),
            (Err(e), Some(want), _) => failures.push(format!(
                "[{label}] {:?}: LiquidJS prints {want:?}, we fail: {e}",
                p.src
            )),
            _ => unreachable!("a probe has an out or an err"),
        }
    }
    failures
}

fn report(failures: Vec<String>, total: usize) {
    assert!(
        failures.is_empty(),
        "{} of {total} differ from LiquidJS:\n{}",
        failures.len(),
        failures
            .iter()
            .take(60)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn plain_probes_match_liquidjs() {
    let f = file();
    report(check(&f.plain, &f.data, Tz::Utc, "utc"), f.plain.len());
}

#[test]
fn date_probes_match_liquidjs_in_utc() {
    let f = file();
    report(
        check(&f.dates_utc, &f.data, Tz::Utc, "utc"),
        f.dates_utc.len(),
    );
}

/// The probes that depend on the zone, with `Tz::Local`. The child runs with
/// the `TZ` the parent chose.
#[test]
fn zone_child() {
    let Ok(zone) = std::env::var("BRRRTY_PROBE_ZONE") else {
        return;
    };
    let f = file();
    let z = &f.zones[&zone];
    let mut failures = check(&z.zonal, &f.data, Tz::Local, &zone);
    failures.extend(check(&z.zonal_parse, &f.data, Tz::Local, &zone));
    if zone == "UTC" {
        failures.extend(check(&f.plain, &f.data, Tz::Local, "local utc"));
        failures.extend(check(&f.dates_utc, &f.data, Tz::Local, "local utc"));
    }
    report(failures, z.zonal.len() + z.zonal_parse.len());
}

#[test]
fn date_probes_match_liquidjs_in_other_zones() {
    let f = file();
    let exe = std::env::current_exe().unwrap();
    let mut failed = Vec::new();
    for zone in f.zones.keys() {
        let out = Command::new(&exe)
            .args(["--exact", "zone_child", "--nocapture", "--test-threads=1"])
            .env("TZ", zone)
            .env("BRRRTY_PROBE_ZONE", zone)
            .output()
            .unwrap();
        if !out.status.success() {
            failed.push(format!(
                "== {zone}\n{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

// ---- non-ASCII input ---------------------------------------------------------------

fn render_utc(src: &str) -> String {
    let engine = Engine::new(EngineOptions {
        strict_filters: true,
        timezone: Tz::Utc,
    });
    let t = engine.parse("t", src).unwrap();
    engine
        .render(&t, &Value::Object(Default::default()), &NoPartials)
        .unwrap()
}

/// Every expected value here was printed by LiquidJS 10.30.1:
/// `truncate` counts code points for the length it keeps but UTF-16 units
/// for the length it compares with, `slice` counts code points, and `size`
/// is JavaScript's `.length`.
#[test]
fn truncate_counts_the_way_liquidjs_does() {
    assert_eq!(
        render_utc(
            "{{ '😀abc' | truncate: 3 }}|{{ '😀abc' | truncate: 4 }}|{{ '😀abc' | truncate: 5 }}|\
             {{ '😀😀😀' | truncate: 2, '' }}|{{ '日本語テキスト' | truncate: 5 }}|\
             {{ '日本語テキスト' | truncate: 5, '…' }}"
        ),
        "...|😀abc|😀abc|😀😀|日本...|日本語テ…"
    );
}

#[test]
fn truncatewords_splits_on_javascript_whitespace() {
    assert_eq!(
        render_utc(
            "{{ '😀 日本 abc def' | truncatewords: 2 }}|{{ '日本語 テキスト' | truncatewords: 1, '…' }}|\
             {{ '  😀  b ' | truncatewords: 5 }}"
        ),
        "😀 日本...|日本語…|😀 b "
    );
}

#[test]
fn slice_counts_code_points() {
    assert_eq!(
        render_utc(
            "{{ '😀abc' | slice: 0 }}|{{ '😀abc' | slice: 1, 2 }}|{{ '😀abc' | slice: -2, 2 }}|\
             {{ '日本語' | slice: 1, 1 }}|{{ '日本語' | slice: -1 }}|{{ '😀😀' | slice: 1 }}"
        ),
        "😀|ab|bc|本|語|😀"
    );
}

#[test]
fn size_counts_utf16_units() {
    assert_eq!(
        render_utc(
            "{{ '😀abc' | size }}|{{ '日本語テキスト' | size }}|{{ '😀' | size }}|{{ 'é' | size }}|\
             {{ '👨‍👩‍👧' | size }}"
        ),
        "5|7|2|1|8"
    );
}

// ---- the filter set ---------------------------------------------------------------------

/// `Object.keys(new Liquid().filters)` in LiquidJS 10.30.1.
const LIQUIDJS_FILTERS: [&str; 88] = [
    "abs",
    "append",
    "array_to_sentence_string",
    "at_least",
    "at_most",
    "base64_decode",
    "base64_encode",
    "capitalize",
    "ceil",
    "cgi_escape",
    "compact",
    "concat",
    "date",
    "date_to_long_string",
    "date_to_rfc822",
    "date_to_string",
    "date_to_xmlschema",
    "default",
    "divided_by",
    "downcase",
    "escape",
    "escape_once",
    "find",
    "find_exp",
    "find_index",
    "find_index_exp",
    "first",
    "floor",
    "group_by",
    "group_by_exp",
    "has",
    "has_exp",
    "hmac_sha256",
    "inspect",
    "join",
    "json",
    "jsonify",
    "last",
    "lstrip",
    "map",
    "minus",
    "modulo",
    "newline_to_br",
    "normalize_whitespace",
    "number_of_words",
    "plus",
    "pop",
    "prepend",
    "push",
    "raw",
    "reject",
    "reject_exp",
    "remove",
    "remove_first",
    "remove_last",
    "replace",
    "replace_first",
    "replace_last",
    "reverse",
    "round",
    "rstrip",
    "sample",
    "sha256",
    "shift",
    "size",
    "slice",
    "slugify",
    "sort",
    "sort_natural",
    "split",
    "squish",
    "strip",
    "strip_html",
    "strip_newlines",
    "sum",
    "times",
    "to_integer",
    "truncate",
    "truncatewords",
    "uniq",
    "unshift",
    "upcase",
    "uri_escape",
    "url_decode",
    "url_encode",
    "where",
    "where_exp",
    "xml_escape",
];

#[test]
fn the_standard_set_is_exactly_liquidjs_s() {
    let engine = Engine::new(EngineOptions::default());
    for name in LIQUIDJS_FILTERS {
        engine
            .parse("t", &format!("{{{{ x | {name} }}}}"))
            .unwrap_or_else(|e| panic!("{name} should exist: {e}"));
    }
    // Names that other Liquid engines have and LiquidJS 10.30.1 does not.
    for name in [
        "md5",
        "sha1",
        "shuffle",
        "unescape",
        "truncate_words",
        "money",
        "t",
    ] {
        let e = engine
            .parse("t", &format!("{{{{ x | {name} }}}}"))
            .expect_err(name);
        assert_eq!(e.message, format!("undefined filter: {name}"));
    }
}

#[test]
fn an_unknown_filter_suggests_the_closest_name() {
    let engine = Engine::new(EngineOptions::default());
    let e = engine.parse("t", "{{ x | upcas }}").unwrap_err();
    assert_eq!(e.help.as_deref(), Some("did you mean `upcase`?"));
    let e = engine.parse("t", "{{ x | trunkate: 3 }}").unwrap_err();
    assert_eq!(e.help.as_deref(), Some("did you mean `truncate`?"));
}

// ---- the output limit ----------------------------------------------------------------------

/// A doubling loop with no output at all must stop at the limit, not at
/// the machine's memory. This one runs for real, to 512 MiB, so it is not
/// part of the normal run: `cargo test -- --ignored doubling`.
#[test]
#[ignore = "builds strings of 512 MiB"]
fn doubling_append_stops_at_the_real_limit() {
    let engine = Engine::new(EngineOptions::default());
    let t = engine
        .parse(
            "t",
            "{% assign s = 'x' %}{% for i in (1..64) %}{% assign s = s | append: s %}{% endfor %}",
        )
        .unwrap();
    let e = engine
        .render(&t, &Value::Object(Default::default()), &NoPartials)
        .unwrap_err();
    assert_eq!(e.kind, brrrty_liquid::ErrorKind::Limit);
    assert!(e.message.contains("512 MiB"), "{e}");
    assert!(e.help.is_some());
}
