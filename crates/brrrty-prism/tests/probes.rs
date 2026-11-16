//! Replays tests/probes/regex.json: what Node's `exec` gave for Prism's
//! regular expressions on code-like text, a sample of the fuzz in
//! tests/fuzz.rs (which wrote the file). Positions are UTF-16 units. A
//! case `exec` refuses (its backtracking budget runs out) is counted as a
//! refusal, not a difference: the page would be refused, never written.

use brrrty_prism::regex::{JsText, translate};
use serde_json::Value;

/// A case's text: a string, or (when it has a lone surrogate, which JSON
/// can't carry to a Rust string) its UTF-16 units.
fn text(case: &Value) -> JsText {
    match &case["t"] {
        Value::String(s) => JsText::new(s),
        Value::Array(units) => {
            let units: Vec<u16> = units.iter().map(|u| u.as_u64().unwrap() as u16).collect();
            JsText::from_units(&units)
        }
        other => panic!("bad text {other}"),
    }
}

#[test]
fn prism_regexes_match_node() {
    let file: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/probes/regex.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut refused = Vec::new();
    let regexes = file["regexes"].as_array().unwrap();
    for r in regexes {
        let (source, flags) = (r["source"].as_str().unwrap(), r["flags"].as_str().unwrap());
        let re = match translate(source, flags) {
            Ok(re) => re,
            Err(e) => {
                failures.push(format!("/{source}/{flags}: not translated: {}", e.0));
                continue;
            }
        };
        for case in r["cases"].as_array().unwrap() {
            let t = text(case);
            let last = case["i"].as_u64().unwrap() as usize;
            let got = match re.exec(&t, last) {
                Ok(got) => got,
                Err(e) => {
                    refused.push(format!("/{source}/{flags} at {last}: {e:?}"));
                    continue;
                }
            };
            let got = got.map(|m| {
                let mut out = vec![m.index as i64, m.end as i64];
                for g in &m.groups {
                    let (a, b) = g.map_or((-1, -1), |(a, b)| (a as i64, b as i64));
                    out.extend([a, b]);
                }
                out
            });
            let want: Option<Vec<i64>> = case["m"]
                .as_array()
                .map(|xs| xs.iter().map(|x| x.as_i64().unwrap()).collect());
            compared += 1;
            if got != want {
                failures.push(format!(
                    "/{source}/{flags} at {last} on {:?}: got {got:?}, node gave {want:?}",
                    case["t"]
                ));
            }
        }
    }
    if !refused.is_empty() {
        eprintln!("{} refused:\n{}", refused.len(), refused.join("\n"));
    }
    assert!(
        failures.is_empty(),
        "{} of {compared} differ:\n{}",
        failures.len(),
        failures[..failures.len().min(30)].join("\n")
    );
    assert!(regexes.len() > 300, "only {} regexes", regexes.len());
    assert!(compared > 8_000, "only {compared} cases");
}
