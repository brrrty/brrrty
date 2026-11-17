//! `luxon_format` against Luxon 3.7.2, case by case. The cases are in `tests/probes/luxon.json`,
//! made by `tools/gen-luxon-probes.cjs`.

use brrrty_core::helpers::luxon::luxon_format;
use serde_json::Value;

#[test]
fn matches_luxon() {
    let v: Value = serde_json::from_str(include_str!("probes/luxon.json")).expect("probe file");
    let cases = v["cases"].as_array().expect("cases");
    assert!(cases.len() >= 200);
    let mut bad = Vec::new();
    let mut refused: Vec<(f64, String)> = Vec::new();
    for c in cases {
        let ms = c["ms"].as_f64().unwrap_or(f64::NAN);
        let (zone, fmt, want) = (
            c["zone"].as_str().unwrap(),
            c["fmt"].as_str().unwrap(),
            c["out"].as_str().unwrap(),
        );
        match luxon_format(ms, fmt, zone, "en-US") {
            Ok(got) if got == want => {}
            Ok(got) => bad.push(format!(
                "{ms} {zone} {fmt:?}\n   want {want:?}\n   got  {got:?}"
            )),
            Err(e) => refused.push((ms, format!("{ms} {zone} {fmt:?}: {e}"))),
        }
    }
    let n = bad.len();
    eprintln!(
        "{} cases, {} differ, {} refused",
        cases.len(),
        n,
        refused.len()
    );
    // Only what the module docs list is refused: a zone name before 1970.
    let unexpected: Vec<_> = refused
        .iter()
        .filter(|(ms, e)| !(e.contains("before 1970") && *ms < 0.0))
        .collect();
    assert!(unexpected.is_empty(), "unexpected refusals: {unexpected:?}");
    assert!(
        bad.is_empty(),
        "{n} differ:\n{}",
        bad.iter().take(60).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn refuses_what_it_cannot_reproduce() {
    let e = luxon_format(0.0, "yyyy", "utc", "fr-FR").unwrap_err();
    assert!(e.contains("fr-FR") && e.contains("en-US"), "{e}");
    for z in ["local", "system", "default", "LOCAL"] {
        let e = luxon_format(0.0, "yyyy", z, "en-US").unwrap_err();
        assert!(e.contains("depends on the machine"), "{e}");
    }
    let e = luxon_format(-1.0e12, "ZZZZ", "America/New_York", "en-US").unwrap_err();
    assert!(e.contains("before 1970"), "{e}");
    // Not a zone name: fine in any year.
    assert_eq!(
        luxon_format(-1.0e12, "ZZ", "America/New_York", "en-US").unwrap(),
        "-04:00"
    );
}

#[test]
fn invalid_input_is_invalid_datetime() {
    for (ms, zone) in [(f64::NAN, "utc"), (0.0, "Fantasia/Castle"), (0.0, "UTC+")] {
        assert_eq!(
            luxon_format(ms, "yyyy", zone, "en-US").unwrap(),
            "Invalid DateTime"
        );
    }
    assert_eq!(luxon_format(0.0, "", "utc", "en").unwrap(), "");
}

/// The bundled tz data has each zone's rules to the end of JavaScript's
/// range. Node: `DateTime.fromMillis(4120000000000, { zone }).toFormat("HH:mm ZZ")`.
#[test]
fn zones_with_dst_work_after_2099() {
    let july_2100 = 4_120_000_000_000.0;
    assert_eq!(
        luxon_format(july_2100, "HH:mm ZZ", "America/New_York", "en-US").unwrap(),
        "00:26 -04:00"
    );
    assert_eq!(
        luxon_format(4_088_000_000_000.0, "HH:mm ZZ", "America/New_York", "en-US").unwrap(),
        "15:33 -04:00"
    );
    assert_eq!(
        luxon_format(july_2100, "HH:mm ZZ", "Asia/Tokyo", "en-US").unwrap(),
        "13:26 +09:00"
    );
    assert_eq!(
        luxon_format(july_2100, "HH:mm ZZ", "UTC+5", "en-US").unwrap(),
        "09:26 +05:00"
    );
}
