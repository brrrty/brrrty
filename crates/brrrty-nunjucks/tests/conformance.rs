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

/// Cases that need something a later task adds, with the reason. The test
/// checks each reason: a case excluded for a filter uses that Nunjucks
/// filter, and one excluded for a regular expression has `r/` in it.
const EXCLUDED: &[(&str, &str)] = &[
    ("e_collections_var", "filter length (Task 3)"),
    ("e_getCollectionItem_absent", "filter length (Task 3)"),
    ("err_filter_arg_missing", "filter replace (Task 3)"),
    ("expr_js_split", "filter join (Task 3)"),
    (
        "expr_js_replace_regex",
        "regex literal (Task 4, controller ruling)",
    ),
    ("expr_js_array_slice", "filter join (Task 3)"),
    ("expr_range_func", "filter join (Task 3)"),
    (
        "expr_regex_literal",
        "regex literal (Task 4, controller ruling)",
    ),
    ("expr_filter_in_expr", "filter length (Task 3)"),
    ("expr_filter_precedence", "filter string (Task 3)"),
    ("expr_filter_on_literal", "filter upper (Task 3)"),
    ("expr_object_in_for_unpack_len", "filter length (Task 3)"),
    ("f_abs", "filter abs (Task 3)"),
    ("f_batch", "filter batch (Task 3)"),
    ("f_batch_fill", "filter batch (Task 3)"),
    ("f_capitalize", "filter capitalize (Task 3)"),
    ("f_center", "filter center (Task 3)"),
    ("f_default", "filter default (Task 3)"),
    ("f_dictsort", "filter dictsort (Task 3)"),
    ("f_dump", "filter dump (Task 3)"),
    ("f_dump_indent", "filter dump (Task 3)"),
    ("f_dump_string", "filter dump (Task 3)"),
    ("f_first", "filter first (Task 3)"),
    ("f_first_string", "filter first (Task 3)"),
    ("f_first_empty", "filter first (Task 3)"),
    ("f_float", "filter float (Task 3)"),
    ("f_float_bad", "filter float (Task 3)"),
    ("f_float_default", "filter float (Task 3)"),
    ("f_groupby", "filter groupby (Task 3)"),
    ("f_indent", "filter indent (Task 3)"),
    ("f_indent_first", "filter indent (Task 3)"),
    ("f_int", "filter int (Task 3)"),
    ("f_int_float_string", "filter int (Task 3)"),
    ("f_int_bad", "filter int (Task 3)"),
    ("f_int_default", "filter int (Task 3)"),
    ("f_join", "filter join (Task 3)"),
    ("f_join_no_sep", "filter join (Task 3)"),
    ("f_join_attr", "filter join (Task 3)"),
    ("f_last", "filter last (Task 3)"),
    ("f_length_array", "filter length (Task 3)"),
    ("f_length_string", "filter length (Task 3)"),
    ("f_length_object", "filter length (Task 3)"),
    ("f_length_undefined", "filter length (Task 3)"),
    ("f_list_string", "filter join (Task 3)"),
    ("f_list_object", "filter list (Task 3)"),
    ("f_lower", "filter lower (Task 3)"),
    ("f_nl2br", "filter nl2br (Task 3)"),
    ("f_reject", "filter join (Task 3)"),
    ("f_rejectattr", "filter rejectattr (Task 3)"),
    ("f_replace", "filter replace (Task 3)"),
    ("f_replace_count", "filter replace (Task 3)"),
    ("f_replace_empty", "filter replace (Task 3)"),
    ("f_replace_number", "filter replace (Task 3)"),
    ("f_replace_special", "filter replace (Task 3)"),
    ("f_reverse_array", "filter join (Task 3)"),
    ("f_reverse_string", "filter reverse (Task 3)"),
    ("f_round", "filter round (Task 3)"),
    ("f_round_precision", "filter round (Task 3)"),
    ("f_round_floor", "filter round (Task 3)"),
    ("f_round_ceil", "filter round (Task 3)"),
    ("f_round_integer", "filter round (Task 3)"),
    ("f_select", "filter join (Task 3)"),
    ("f_select_truthy", "filter join (Task 3)"),
    ("f_selectattr", "filter selectattr (Task 3)"),
    ("f_slice", "filter slice (Task 3)"),
    ("f_sort", "filter join (Task 3)"),
    ("f_sort_strings", "filter join (Task 3)"),
    ("f_sort_reverse", "filter join (Task 3)"),
    ("f_sort_attr", "filter join (Task 3)"),
    ("f_string", "filter string (Task 3)"),
    ("f_striptags", "filter striptags (Task 3)"),
    ("f_striptags_whitespace", "filter striptags (Task 3)"),
    ("f_striptags_preserve", "filter striptags (Task 3)"),
    ("f_striptags_entities", "filter striptags (Task 3)"),
    ("f_sum", "filter sum (Task 3)"),
    ("f_sum_attr", "filter sum (Task 3)"),
    ("f_sum_start", "filter sum (Task 3)"),
    ("f_title", "filter title (Task 3)"),
    ("f_title_mixed", "filter title (Task 3)"),
    ("f_trim", "filter trim (Task 3)"),
    ("f_truncate", "filter truncate (Task 3)"),
    ("f_truncate_killwords", "filter truncate (Task 3)"),
    ("f_truncate_end", "filter truncate (Task 3)"),
    ("f_truncate_short", "filter truncate (Task 3)"),
    ("f_upper", "filter upper (Task 3)"),
    ("f_urlencode", "filter urlencode (Task 3)"),
    ("f_urlencode_object", "filter urlencode (Task 3)"),
    ("f_urlize", "filter urlize (Task 3)"),
    ("f_wordcount", "filter wordcount (Task 3)"),
    ("f_wordcount_empty", "filter wordcount (Task 3)"),
    ("f_chain", "filter truncate (Task 3)"),
    ("f_filter_on_undefined", "filter upper (Task 3)"),
    ("f_filter_on_null", "filter lower (Task 3)"),
    ("f_filter_arg_expression", "filter truncate (Task 3)"),
    ("f_filter_arg_variable", "filter join (Task 3)"),
    ("f_filter_kwargs", "filter truncate (Task 3)"),
    ("f_capitalize_empty", "filter capitalize (Task 3)"),
    ("f_length_number", "filter length (Task 3)"),
    ("inc_with_filter_inside", "filter upper (Task 3)"),
    ("set_array_literal", "filter length (Task 3)"),
    ("filter_block", "filter upper (Task 3)"),
    ("filter_block_title", "filter title (Task 3)"),
    ("do_tag_absent", "filter length (Task 3)"),
    ("truth_and_empty_array", "filter dump (Task 3)"),
    ("default_undefined", "filter default (Task 3)"),
    ("default_false", "filter default (Task 3)"),
    ("default_false_true", "filter default (Task 3)"),
    ("default_empty_string", "filter default (Task 3)"),
    ("default_empty_string_true", "filter default (Task 3)"),
    ("default_zero", "filter default (Task 3)"),
    ("default_null", "filter default (Task 3)"),
    ("default_null_true", "filter default (Task 3)"),
    ("default_nested_undefined", "filter default (Task 3)"),
    ("default_no_args", "filter default (Task 3)"),
    ("default_chain", "filter default (Task 3)"),
    ("auto_join_escapes", "filter join (Task 3)"),
    ("auto_default_escapes", "filter default (Task 3)"),
];

/// Nunjucks's built-in filters that Task 2 does not have yet.
const LATER_FILTERS: &[&str] = &[
    "abs",
    "batch",
    "capitalize",
    "center",
    "default",
    "dictsort",
    "dump",
    "first",
    "groupby",
    "indent",
    "join",
    "last",
    "length",
    "list",
    "lower",
    "nl2br",
    "random",
    "reject",
    "rejectattr",
    "select",
    "selectattr",
    "replace",
    "reverse",
    "round",
    "slice",
    "sum",
    "sort",
    "string",
    "striptags",
    "title",
    "trim",
    "truncate",
    "upper",
    "urlencode",
    "urlize",
    "wordcount",
    "float",
    "int",
    "d",
];

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

/// Whether `text` uses the filter `name`: `| name` or `{% filter name`.
fn uses_filter(text: &str, name: &str) -> bool {
    let starts_word = |rest: &str| {
        rest.trim_start()
            .strip_prefix(name)
            .is_some_and(|after| !after.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    };
    text.split('|').skip(1).any(starts_word)
        || text.split("{% filter").skip(1).any(starts_word)
        || text.split("{%- filter").skip(1).any(starts_word)
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
        let honest = if let Some(filter) = why.strip_prefix("filter ") {
            let filter = filter.split(' ').next().unwrap();
            LATER_FILTERS.contains(&filter) && sources.iter().any(|s| uses_filter(s, filter))
        } else if why.starts_with("regex") {
            sources.iter().any(|s| s.contains("r/"))
        } else {
            false
        };
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
