//! `parse_csv` against csv-parse 7.0.3: `tests/probes/csv.json` (made by
//! `tools/gen-csv-probes.mjs` in Node) has hand-written edge cases and seeded
//! fuzz inputs under 18 option sets, with csv-parse's records or its error
//! message for each. The port has to give the same records, keys in the same
//! order, and the same message.

use brrrty_config::csv;
use brrrty_js::Value;
use serde_json::Value as Json;

fn record_json(v: &Value) -> Json {
    match v {
        Value::Array(a) => Json::Array(
            a.iter()
                .map(|f| match f {
                    Value::String(s) => Json::String(s.to_string()),
                    other => panic!("field {other:?}"),
                })
                .collect(),
        ),
        Value::Object(o) => {
            // `Object.keys` order: integer-like keys first.
            let mut index: Vec<(u32, &String)> = Vec::new();
            let mut rest = Vec::new();
            for k in o.keys() {
                match k.parse::<u32>() {
                    Ok(i) if i != u32::MAX && i.to_string() == *k => index.push((i, k)),
                    _ => rest.push(k),
                }
            }
            index.sort_by_key(|(i, _)| *i);
            Json::Array(
                index
                    .into_iter()
                    .map(|(_, k)| k)
                    .chain(rest)
                    .map(|k| {
                        let Value::String(s) = &o[k] else {
                            panic!("field {:?}", o[k])
                        };
                        Json::Array(vec![Json::String(k.clone()), Json::String(s.to_string())])
                    })
                    .collect(),
            )
        }
        other => panic!("record {other:?}"),
    }
}

#[test]
fn parse_csv_matches_csv_parse() {
    let text = include_str!("probes/csv.json");
    let probes: Json = serde_json::from_str(text).expect("csv.json");
    let options: Vec<csv::CsvOptions> = probes["options"]
        .as_array()
        .expect("options")
        .iter()
        .map(|o| {
            let given: Vec<(String, Value)> = o
                .as_object()
                .expect("an options object")
                .iter()
                .map(|(k, v)| (k.clone(), Value::from_json(v)))
                .collect();
            csv::options(&given).expect("supported options")
        })
        .collect();
    let mut failures = Vec::new();
    let cases = probes["cases"].as_array().expect("cases");
    for case in cases {
        let o = case["o"].as_u64().expect("o") as usize;
        let s = case["s"].as_str().expect("s");
        let got = csv::parse(s, &options[o]);
        let ok = match (&got, &case["out"], &case["err"]) {
            (Ok(records), Json::Array(want), Json::Null) => {
                records.iter().map(record_json).collect::<Vec<_>>() == *want
            }
            (Err(message), Json::Null, Json::String(want)) => message == want,
            _ => false,
        };
        if !ok {
            failures.push(format!(
                "options {} on {s:?}: got {got:?}, want {} / {}",
                probes["options"][o], case["out"], case["err"]
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn options_are_normalized_as_csv_parse_does() {
    let opts = |pairs: &[(&str, Value)]| {
        csv::options(
            &pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<Vec<_>>(),
        )
    };
    let o = opts(&[("trim", Value::Bool(true)), ("rtrim", Value::Bool(false))]).unwrap();
    assert!(o.ltrim && !o.rtrim);
    let o = opts(&[("skipEmptyLines", Value::Bool(true))]).unwrap();
    assert!(o.skip_empty_lines);
    assert_eq!(
        opts(&[("trim", Value::String("yes".into()))]).unwrap_err(),
        "Invalid Option: trim must be a boolean, got \"yes\""
    );
    assert!(
        opts(&[("cast", Value::Bool(true))])
            .unwrap_err()
            .contains("doesn't support csv-parse's `cast` option")
    );
    assert!(
        opts(&[("colums", Value::Bool(true))])
            .unwrap_err()
            .contains("`colums` isn't a csv-parse option")
    );
}
