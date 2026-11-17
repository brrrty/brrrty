//! Local dates against V8: `tests/probes/dates.json`, made by
//! `tools/gen-date-probes.mjs` (Node v26.2.0) in five zones. Each zone runs
//! in a child process with `TZ` set, since the local zone is read once per
//! process.

use brrrty_js::{Tz, tz};
use serde_json::Value as Json;

const ZONES: [&str; 5] = [
    "America/Chicago",
    "Europe/London",
    "Australia/Lord_Howe",
    "Asia/Kolkata",
    "UTC",
];

fn num(j: &Json) -> f64 {
    j.as_f64().unwrap_or(f64::NAN)
}

fn same(a: f64, b: f64) -> bool {
    a == b || (a.is_nan() && b.is_nan())
}

/// [getTime, getFullYear, getMonth, getDate, getDay, getHours, getMinutes,
/// getSeconds, getMilliseconds, getTimezoneOffset] of `ms` in the local zone.
fn getters(ms: f64) -> Vec<f64> {
    match tz::fields(Tz::Local, ms) {
        None => vec![f64::NAN; 10],
        Some(f) => vec![
            ms,
            f.year as f64,
            f64::from(f.month0),
            f64::from(f.day),
            f64::from(f.weekday),
            f64::from(f.hour),
            f64::from(f.minute),
            f64::from(f.second),
            f64::from(f.millis),
            f.offset_west,
        ],
    }
}

fn check_zone(zone: &str) -> Vec<String> {
    let p: Json = serde_json::from_str(include_str!("probes/dates.json")).unwrap();
    let z = &p["zones"][zone];
    let mut bad = Vec::new();
    let mut compare = |what: String, want: &Json, got_ms: f64| {
        let want_get: Vec<f64> = want[1].as_array().unwrap().iter().map(num).collect();
        let got_get = getters(got_ms);
        let want_iso = want[2].as_str().map(str::to_string);
        let got_iso = tz::iso_string(got_ms);
        if want_get.len() != got_get.len()
            || want_get.iter().zip(&got_get).any(|(a, b)| !same(*a, *b))
            || want_iso != got_iso
        {
            bad.push(format!(
                "{zone} {what}: want {want_get:?} {want_iso:?}, got {got_get:?} {got_iso:?}"
            ));
        }
    };
    let ctor = z["ctor"].as_array().unwrap();
    assert!(ctor.len() > 3000);
    for c in ctor {
        let args: Vec<f64> = c[0].as_array().unwrap().iter().map(num).collect();
        compare(
            format!("new Date{args:?}"),
            c,
            tz::make_date(Tz::Local, &args),
        );
    }
    for c in z["instants"].as_array().unwrap() {
        let ms = num(&c[0]);
        compare(format!("new Date({ms})"), c, ms);
    }
    bad
}

#[test]
fn local_dates_match_v8_in_five_zones() {
    if let Ok(zone) = std::env::var("BRRRTY_DATE_PROBE_ZONE") {
        let bad = check_zone(&zone);
        assert!(
            bad.is_empty(),
            "{} differ:\n{}",
            bad.len(),
            bad[..bad.len().min(20)].join("\n")
        );
        return;
    }
    for zone in ZONES {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_dates_match_v8_in_five_zones",
                "--nocapture",
            ])
            .env("TZ", zone)
            .env("BRRRTY_DATE_PROBE_ZONE", zone)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{zone}:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
