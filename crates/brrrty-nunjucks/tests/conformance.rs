//! The Nunjucks conformance cases: every template, and every include a
//! case defines, must parse like Nunjucks 3.2.4, and every case not in
//! [`EXCLUDED`] must render like it.
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

/// Cases that need something Brrrty doesn't do yet, with the reason. The
/// test checks each reason: a case excluded for a regular expression has
/// `r/` in it. None is excluded: every case renders.
const EXCLUDED: &[(&str, &str)] = &[];

/// Serves a case's `includes`.
struct MapLoader<'a>(&'a BTreeMap<String, String>);

impl brrrty_nunjucks::Loader for MapLoader<'_> {
    fn load(&self, name: &str) -> Result<(String, String), brrrty_nunjucks::LoadError> {
        self.0
            .get(name)
            .map(|s| (name.to_string(), s.clone()))
            .ok_or_else(Default::default)
    }
}

enum Outcome {
    Pass,
    Fail(String),
}

fn run_case(case: &Case) -> Outcome {
    use brrrty_nunjucks::{Engine, EngineOptions, ErrorKind, Tz};
    let expected = cases_dir().join("expected");
    let want_out = fs::read_to_string(expected.join(format!("{}.out", case.name))).ok();
    let mut engine = Engine::new(EngineOptions {
        timezone: Tz::Utc,
        autoescape: true,
    });
    engine.register_eleventy_filters();
    let mut data: serde_json::Value =
        serde_json::from_str(case.data.as_deref().unwrap_or("{}")).unwrap();
    if case.group == "eleventy" {
        // What Eleventy adds to every template's data. The references were
        // made with each case as `cases/<name>.njk`, output as
        // `/nunjucks/<name>.txt`.
        let page = serde_json::json!({
            "url": format!("/nunjucks/{}.txt", case.name),
            "fileSlug": case.name,
            "filePathStem": format!("/cases/{}", case.name),
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
        .compile(&case.name, &case.template)
        .and_then(|t| engine.render(&t, &data, &MapLoader(&case.includes)));
    // A parse error's position must match the `.error` file's.
    let want_position = fs::read_to_string(expected.join(format!("{}.error", case.name)))
        .ok()
        .and_then(|text| error_position(&text));
    if let (Some((line, col)), Err(e)) = (want_position, &got)
        && e.kind == ErrorKind::Syntax
        && (e.line, e.column) != (Some(line), Some(col))
    {
        return Outcome::Fail(format!("want an error at {line}:{col}, got {e}"));
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
fn cases_render_like_nunjucks() {
    let cases = load();
    for (name, why) in EXCLUDED {
        let case = cases
            .iter()
            .find(|c| c.name == *name)
            .unwrap_or_else(|| panic!("{name}: excluded but not a case"));
        let sources: Vec<&str> = std::iter::once(case.template.as_str())
            .chain(case.includes.values().map(String::as_str))
            .collect();
        let honest = why.starts_with("regex") && sources.iter().any(|s| s.contains("r/"));
        assert!(honest, "{name}: the reason {why:?} doesn't hold");
    }
    // file -> (passed, total, excluded)
    let mut table: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut excluded_passing = Vec::new();
    for case in &cases {
        let outcome = run_case(case);
        let row = table.entry(case.file.clone()).or_default();
        row.1 += 1;
        let excluded = EXCLUDED.iter().any(|(n, _)| *n == case.name);
        if excluded {
            row.2 += 1;
        }
        match outcome {
            Outcome::Pass => {
                row.0 += 1;
                if excluded {
                    excluded_passing.push(case.name.as_str());
                }
            }
            Outcome::Fail(why) if !excluded => {
                failures.push(format!("{} ({}): {why}", case.name, case.file))
            }
            Outcome::Fail(_) => {}
        }
    }
    println!("| file | pass | total | excluded |");
    println!("|---|---|---|---|");
    let (mut pass, mut total, mut skipped) = (0, 0, 0);
    for (file, (p, t, x)) in &table {
        println!("| {file} | {p} | {t} | {x} |");
        pass += p;
        total += t;
        skipped += x;
    }
    println!("| all | {pass} | {total} | {skipped} |");
    assert_eq!(skipped, EXCLUDED.len());
    if !excluded_passing.is_empty() {
        println!("excluded cases that already pass: {excluded_passing:?}");
    }
    assert!(
        failures.is_empty(),
        "{} cases fail:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
