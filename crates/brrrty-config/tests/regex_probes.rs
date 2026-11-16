//! `regex_replace`, `regex_split` and `regex_test` against what Node gave
//! for the same calls (tests/probes/regex.json, made by
//! tools/gen-regex-probes.cjs), on text with emoji, CJK and combining
//! marks. A pattern marked `astral: false` must refuse text with a
//! character outside the BMP, since JavaScript sees that character as two;
//! cases with `out` null are those Brrrty refuses, for the reason `why`.

use std::fs;

use brrrty_config::{CONFIG_FILE, Clock, load_with};
use brrrty_js::Value;
use brrrty_liquid::{FilterCtx, Tz};
use serde_json::Value as Json;

const SCRIPT: &str = r#"
config.add_filter("rx", |op, s, p, f, r| {
    switch op {
        "replace" => regex_replace(s, p, r, f),
        "split" => regex_split(s, p, f),
        _ => regex_test(s, p, f),
    }
});
"#;

#[test]
fn regex_calls_match_node_on_text_outside_ascii() {
    let probes: Json = serde_json::from_str(
        &fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/probes/regex.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join(CONFIG_FILE), SCRIPT).unwrap();
    let cfg = load_with(t.path(), Clock::Fixed(0)).unwrap().unwrap();
    let rx = &cfg.filters[0].run;
    let ctx = FilterCtx::new(Tz::Utc);
    let patterns = probes["patterns"].as_array().unwrap();
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut refused = 0;
    // Cases with text outside the BMP that a random pattern accepted.
    let mut random_astral = 0;
    for case in probes["cases"].as_array().unwrap() {
        let pat = &patterns[case["i"].as_u64().unwrap() as usize];
        let (p, f) = (pat["p"].as_str().unwrap(), pat["f"].as_str().unwrap());
        let s = case["s"].as_str().unwrap();
        let op = case["op"].as_str().unwrap();
        let args = [
            Value::String(s.into()),
            Value::String(p.into()),
            Value::String(f.into()),
            Value::String(case["r"].as_str().unwrap_or("").into()),
        ];
        let got = rx(&Value::String(op.into()), &args, &ctx);
        if case["out"].is_null() {
            let why = case["why"].as_str().unwrap();
            match got {
                Err(e) if e.contains(why) => refused += 1,
                // A random pattern may be refused outright.
                Err(e) if pat["astral"].is_null() && e.contains("can match nothing") => {}
                other => failures.push(format!("/{p}/{f} {op} {s:?}: not refused: {other:?}")),
            }
            continue;
        }
        // JSON has no `undefined`: a group that took no part in a split is
        // null in the file.
        let want = match Value::from_json(&case["out"]) {
            Value::Array(xs) if op == "split" => Value::Array(std::sync::Arc::new(
                xs.iter()
                    .map(|x| {
                        if *x == Value::Null {
                            Value::Undefined
                        } else {
                            x.clone()
                        }
                    })
                    .collect(),
            )),
            v => v,
        };
        if pat["astral"].is_null() {
            // A random pattern: refusing is fine, a wrong answer isn't.
            if matches!(&got, Err(e) if e.contains("outside the BMP") || e.contains("can match nothing"))
            {
                continue;
            }
            if s.chars().any(|c| c as u32 > 0xFFFF) {
                random_astral += 1;
            }
        }
        compared += 1;
        match got {
            Ok(v) if v == want => {}
            other => failures.push(format!(
                "/{p}/{f} {op} {s:?}: got {other:?}, node gave {want:?}"
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        compared,
        failures[..failures.len().min(40)].join("\n")
    );
    eprintln!(
        "{compared} compared, {refused} refused, {random_astral} random accepted outside the BMP"
    );
    assert!(compared > 7000, "only {compared} cases compared");
    assert!(refused > 900, "only {refused} cases refused");
    assert!(
        random_astral > 1000,
        "only {random_astral} random cases accepted"
    );
}
