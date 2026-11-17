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

/// `sweepInstants` in the generator.
fn sweep_instants() -> Vec<f64> {
    let mut out = Vec::new();
    let start = -3_786_825_600_000.0; // 1850-01-01
    let end = 2_366_841_600_000.0; // 2045-01-01
    let (mut t, mut i) = (start, 0u32);
    while t < end {
        out.push(t + f64::from(i % 24) * 3_600_000.0 + f64::from(i % 7) * 60_000.0);
        t += 3.0 * 86_400_000.0;
        i += 1;
    }
    for i in 0..400 {
        out.push(4_070_908_800_000.0 + f64::from(i) * 86_400_000.0 * 1.37);
    }
    for i in 0..100 {
        out.push(8.64e15 - f64::from(i) * 86_400_000.0 * 3.3);
        out.push(-8.64e15 + f64::from(i) * 86_400_000.0 * 3.3);
    }
    out.into_iter().map(f64::trunc).collect()
}

/// Every zone Node knows, against the bundled tz data, by the hash of
/// `getTimezoneOffset()` over ~24,000 instants from 1850 to 2045, around
/// 2100 and at the ends of the date range.
#[test]
fn every_zone_matches_node_s_tz_data() {
    let p: Json = serde_json::from_str(include_str!("probes/dates.json")).unwrap();
    assert_eq!(p["tz"], tz::TZDATA_VERSION);
    let instants = sweep_instants();
    let sweep = p["sweep"].as_object().unwrap();
    assert!(sweep.len() > 400);
    let mut bad = Vec::new();
    for (name, want) in sweep {
        let Some(zone) = tz::Zone::named(name) else {
            bad.push(format!("{name}: not in the bundled data"));
            continue;
        };
        let mut h: u32 = 0x811c_9dc5;
        for &t in &instants {
            // getTimezoneOffset: minutes west, seconds dropped as V8 drops them.
            let west = (-(zone.offset_seconds(t) as f64) / 60.0).trunc() + 0.0;
            for b in brrrty_js::js_number_to_string(west).bytes().chain(*b"\n") {
                h ^= u32::from(b);
                h = h.wrapping_mul(0x0100_0193);
            }
        }
        if u64::from(h) != want.as_u64().unwrap() {
            bad.push(format!("{name}: offsets differ from Node's"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
