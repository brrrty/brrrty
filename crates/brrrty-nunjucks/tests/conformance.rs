//! The Nunjucks conformance cases: every template, and every include a
//! case defines, must parse like Nunjucks 3.2.4.
//!
//! `probes/conformance_parse_errors.json` lists every case template and
//! include that Nunjucks's parser rejects, with the message, line and
//! column it reports (made by `tools/gen-conformance-probes.cjs`). The
//! errors Nunjucks raises later, when it compiles or renders, are not in
//! it: unknown filters, missing includes and the like.

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
    template: String,
    #[serde(default)]
    includes: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Pinned {
    name: String,
    msg: String,
    line: Option<usize>,
    col: Option<usize>,
}

fn cases_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/nunjucks")
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

/// `[Line N, Column M]` from the first line of an `.error` file, which is
/// `(./cases/name.njk) [Line N, Column M]` or just the path.
fn error_position(text: &str) -> Option<(usize, usize)> {
    let at = text.find("[Line ")?;
    let (line, rest) = text[at + 6..].split_once(", Column ")?;
    let col = rest.split_once(']')?.0;
    Some((line.parse().ok()?, col.parse().ok()?))
}

#[test]
fn every_conformance_template_parses_like_nunjucks() {
    let cases = load();
    assert!(cases.len() >= 390, "{} cases", cases.len());
    let pinned = pinned();
    let expected = cases_dir().join("expected");
    let (mut parsed, mut rejected, mut includes, mut positions) = (0, 0, 0, 0);
    let mut failures = Vec::new();

    for case in &cases {
        let out = expected.join(format!("{}.out", case.name));
        let err = expected.join(format!("{}.error", case.name));
        assert!(
            out.exists() != err.exists(),
            "{}: needs one of .out and .error",
            case.name
        );
        let mut sources = vec![(case.name.clone(), case.template.as_str())];
        for (file, text) in &case.includes {
            sources.push((format!("{}::{file}", case.name), text.as_str()));
            includes += 1;
        }
        for (name, source) in sources {
            let result = brrrty_nunjucks::parse(&name, source);
            match (pinned.get(&name), &result) {
                (None, Ok(_)) => parsed += 1,
                (None, Err(e)) => {
                    failures.push(format!("{name}: Nunjucks parses this, we fail: {e}"))
                }
                (Some(p), Ok(_)) => {
                    failures.push(format!("{name}: Nunjucks fails ({}), we parse", p.msg))
                }
                (Some(p), Err(e)) => {
                    rejected += 1;
                    let want = (p.msg.as_str(), p.line, p.col);
                    if (e.message.as_str(), e.line, e.column) != want {
                        failures.push(format!(
                            "{name}: want {want:?}, got {:?}",
                            (&e.message, e.line, e.column)
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
            // A position in the .error file belongs to a parse error only
            // when Nunjucks's parser rejects the template; the others come
            // from rendering.
            if let (Some(want), Some(_)) = (error_position(&text), pinned.get(&name)) {
                positions += 1;
                match &result {
                    Err(e) if (e.line, e.column) == (Some(want.0), Some(want.1)) => {}
                    Err(e) => failures.push(format!(
                        "{name}: .error says {want:?}, we say {:?}",
                        (e.line, e.column)
                    )),
                    Ok(_) => failures.push(format!("{name}: .error says {want:?}, we parse")),
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    // Each parse-time .error with a position was checked against it.
    let with_position = pinned
        .values()
        .filter(|p| p.line.is_some() && !p.name.contains("::"))
        .count();
    assert_eq!(positions, with_position);
    eprintln!(
        "{parsed} parsed, {rejected} rejected as Nunjucks does, {includes} includes, {positions} .error positions"
    );
}
