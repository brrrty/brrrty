//! The Liquid conformance cases: every template must parse like LiquidJS,
//! and the cases in [`GATED_FILES`] must render like it.
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
    /// JSON. A missing `data` means `{}`.
    data: Option<String>,
    #[serde(default)]
    includes: BTreeMap<String, String>,
    /// Which `.toml` file the case is in. Filled in by `load`.
    #[serde(skip)]
    file: String,
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
            let file = p.file_name().unwrap().to_string_lossy().into_owned();
            let mut cases = toml::from_str::<File>(&fs::read_to_string(p).unwrap())
                .unwrap()
                .case;
            for c in &mut cases {
                c.file = file.clone();
            }
            cases
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

/// The case files that must render exactly like LiquidJS. Each task that
/// adds engine features extends this list.
const GATED_FILES: &[&str] = &[
    "errors.toml",
    "eleventy.toml",
    "filters.toml",
    "includes.toml",
    "tags.toml",
    "operators.toml",
    "truthiness.toml",
    "whitespace.toml",
];

/// Cases in gated files that need a feature a later task adds. Each entry
/// names the task that removes it.
const DEFERRED: &[(&str, &str)] = &[];

/// Serves a case's `includes`.
struct MapLoader<'a>(&'a BTreeMap<String, String>);

impl brrrty_liquid::Loader for MapLoader<'_> {
    fn load(&self, name: &str) -> Option<(String, String)> {
        self.0.get(name).map(|s| (name.to_string(), s.clone()))
    }
}

enum Outcome {
    Pass,
    Fail(String),
}

fn run_case(case: &Case) -> Outcome {
    use brrrty_liquid::{Engine, EngineOptions, Tz};
    let expected = cases_dir().join("expected");
    let want_out = fs::read_to_string(expected.join(format!("{}.out", case.name))).ok();
    let mut engine = Engine::new(EngineOptions {
        strict_filters: true,
        timezone: Tz::Utc,
    });
    engine.register_eleventy_filters();
    let mut data: serde_json::Value =
        serde_json::from_str(case.data.as_deref().unwrap_or("{}")).unwrap();
    if case.group == "eleventy" {
        // What Eleventy adds to every template's data. The references were
        // made with each case as `liquid/<name>.liquid`, output as `.txt`.
        let page = serde_json::json!({
            "url": format!("/liquid/{}.txt", case.name),
            "fileSlug": case.name,
            "filePathStem": format!("/liquid/{}", case.name),
        });
        let provided = serde_json::json!({
            "page": page,
            "eleventy": {"generator": "Eleventy v3.1.8", "version": "3.1.8"},
            "collections": {"all": [{"page": page, "data": {}}]},
        });
        let (Some(data), Some(provided)) = (data.as_object_mut(), provided.as_object()) else {
            panic!("case data must be an object");
        };
        for (k, v) in provided {
            data.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    let data = brrrty_js::Value::from_json(&data);
    let got = engine
        .parse(&case.name, &case.template)
        .and_then(|t| engine.render(&t, &data, &MapLoader(&case.includes)));
    // An `.error` file with `line:N, col:M` also pins where the error is.
    // The message may differ: ours can be more helpful.
    let want_position = fs::read_to_string(expected.join(format!("{}.error", case.name)))
        .ok()
        .and_then(|text| error_position(&text));
    if let (Some((line, col)), Err(e)) = (want_position, &got)
        && (e.line, e.column) != (line, col)
    {
        return Outcome::Fail(format!(
            "want an error at {line}:{col}, got {}:{}: {e}",
            e.line, e.column
        ));
    }
    match (want_out, got) {
        (Some(want), Ok(got)) if want == got => Outcome::Pass,
        (Some(want), Ok(got)) => Outcome::Fail(format!("want {want:?}, got {got:?}")),
        (Some(want), Err(e)) => Outcome::Fail(format!("want {want:?}, got error {e}")),
        (None, Err(_)) => Outcome::Pass,
        (None, Ok(got)) => Outcome::Fail(format!("want an error, got {got:?}")),
    }
}

#[test]
fn gated_cases_render_like_liquidjs() {
    let cases = load();
    for (name, _) in DEFERRED {
        assert!(
            cases
                .iter()
                .any(|c| c.name == *name && GATED_FILES.contains(&c.file.as_str())),
            "{name}: deferred but not a case in a gated file"
        );
    }
    // (file, group) -> (passed, total)
    let mut table: BTreeMap<(String, String), (usize, usize)> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut deferred_passing = Vec::new();
    for case in &cases {
        let outcome = run_case(case);
        let row = table
            .entry((case.file.clone(), case.group.clone()))
            .or_default();
        row.1 += 1;
        let gated = GATED_FILES.contains(&case.file.as_str());
        let deferred = DEFERRED.iter().any(|(n, _)| *n == case.name);
        match outcome {
            Outcome::Pass => {
                row.0 += 1;
                if gated && deferred {
                    deferred_passing.push(case.name.as_str());
                }
            }
            Outcome::Fail(why) if gated && !deferred => {
                failures.push(format!("{} ({}): {why}", case.name, case.file))
            }
            Outcome::Fail(_) => {}
        }
    }
    println!("| file | group | gated | pass | total |");
    println!("|---|---|---|---|---|");
    let (mut pass, mut total) = (0, 0);
    for ((file, group), (p, t)) in &table {
        let gated = if GATED_FILES.contains(&file.as_str()) {
            "yes"
        } else {
            "no"
        };
        println!("| {file} | {group} | {gated} | {p} | {t} |");
        pass += p;
        total += t;
    }
    println!("| all | | | {pass} | {total} |");
    println!("deferred: {}", DEFERRED.len());
    if !deferred_passing.is_empty() {
        println!("deferred cases that already pass: {deferred_passing:?}");
    }
    assert!(
        failures.is_empty(),
        "{} gated cases fail:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
