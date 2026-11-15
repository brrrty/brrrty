//! Parses every template and include in the Liquid conformance cases.
//!
//! `probes/conformance_parse_errors.json` lists every case template that
//! LiquidJS 10.30.1 rejects at parse time, with the message, line and
//! column it reports. It was made by running each case through
//! `new Liquid().parse()`. Without `strictFilters`, so unknown-filter errors
//! are not in it: filters are checked by a later stage.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    case: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    group: String,
    template: String,
    #[serde(default)]
    includes: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Pinned {
    name: String,
    msg: String,
    line: usize,
    col: usize,
}

fn cases_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/liquid")
}

fn load() -> Vec<Case> {
    let mut paths: Vec<_> = fs::read_dir(cases_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();
    paths
        .iter()
        .flat_map(|p| {
            toml::from_str::<File>(&fs::read_to_string(p).unwrap())
                .unwrap()
                .case
        })
        .collect()
}

fn pinned() -> HashMap<String, Pinned> {
    let text = include_str!("probes/conformance_parse_errors.json");
    serde_json::from_str::<Vec<Pinned>>(text)
        .unwrap()
        .into_iter()
        .map(|p| (p.name.clone(), p))
        .collect()
}

/// `line:N, col:M` from an `.error` file.
fn error_position(text: &str) -> Option<(usize, usize)> {
    let at = text.rfind(", line:")?;
    let (line, col) = text[at + 7..].split_once(", col:")?;
    Some((line.trim().parse().ok()?, col.trim().parse().ok()?))
}

/// The `.error` message without its `, file:..., line:..., col:...` suffix.
fn error_message(text: &str) -> &str {
    match text.find(", file:") {
        Some(at) => &text[..at],
        None => text,
    }
}

#[test]
fn every_conformance_template_parses_like_liquidjs() {
    let cases = load();
    let pinned = pinned();
    let expected = cases_dir().join("expected");
    let (mut parsed, mut rejected, mut includes) = (0, 0, 0);
    let mut failures = Vec::new();
    let mut matched_positions = Vec::new();

    for case in &cases {
        let out = expected.join(format!("{}.out", case.name));
        let err = expected.join(format!("{}.error", case.name));
        let mut sources = vec![(case.name.clone(), case.template.as_str())];
        for (file, text) in &case.includes {
            sources.push((format!("{}::{file}", case.name), text.as_str()));
            includes += 1;
        }
        for (name, source) in sources {
            let result = brrrty_liquid::parse(&name, source);
            match (pinned.get(&name), &result) {
                (None, Ok(_)) => parsed += 1,
                (None, Err(e)) => {
                    failures.push(format!("{name}: LiquidJS parses this, we fail: {e}"))
                }
                (Some(p), Ok(_)) => {
                    failures.push(format!("{name}: LiquidJS fails ({}), we parse", p.msg))
                }
                (Some(p), Err(e)) => {
                    rejected += 1;
                    if (e.message.as_str(), e.line, e.column) != (p.msg.as_str(), p.line, p.col) {
                        failures.push(format!(
                            "{name}: want {:?} at {}:{}, got {:?} at {}:{}",
                            p.msg, p.line, p.col, e.message, e.line, e.column
                        ));
                    }
                }
            }
            if name != case.name {
                continue;
            }
            if out.exists() && result.is_err() {
                failures.push(format!("{name}: has a .out but does not parse"));
            }
            let Ok(text) = fs::read_to_string(&err) else {
                continue;
            };
            let Some((line, col)) = error_position(&text) else {
                continue;
            };
            match &result {
                Err(e)
                    if (e.line, e.column) == (line, col) && e.message == error_message(&text) =>
                {
                    matched_positions.push(format!("{name} {line}:{col}"));
                }
                Err(e) => failures.push(format!(
                    "{name}: .error says {:?} at {line}:{col}, we say {:?} at {}:{}",
                    error_message(&text),
                    e.message,
                    e.line,
                    e.column
                )),
                Ok(_) => failures.push(format!("{name}: .error has a position but we parse")),
            }
        }
        assert!(
            ["core", "filters", "eleventy", "errors"].contains(&case.group.as_str()),
            "{}: unknown group",
            case.name
        );
    }

    println!(
        "{} cases, {includes} includes: {parsed} parse, {rejected} rejected like LiquidJS",
        cases.len()
    );
    println!("positions matching .error files: {matched_positions:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(rejected, pinned.len(), "every pinned error was seen");
    assert_eq!(matched_positions.len(), 9, "every .error with a position");
}
