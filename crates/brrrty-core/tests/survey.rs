//! Survey builds, which `brrrty doctor` runs: every refusal per template,
//! including the ones a plain build stops before reaching.

use brrrty_core::{BuildOptions, BuildReport, build, codes, survey};
use brrrty_markdown::MarkdownIt;
use std::fs;
use std::path::{Path, PathBuf};

fn opts(root: &Path) -> BuildOptions {
    BuildOptions {
        root: root.to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        ..BuildOptions::default()
    }
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

/// A layout with a filter Brrrty doesn't have, used by a page that reads a
/// JavaScript data file and one that paginates it.
fn masked_site() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "src/_includes/base.liquid",
        "<title>{{ title | shout }}</title>\n{{ content }}\n",
    );
    write(
        root,
        "src/_data/stuff.js",
        "module.exports = { items: [1, 2] };\n",
    );
    write(
        root,
        "src/reads.liquid",
        "---\nlayout: base.liquid\ntitle: Reads\n---\n{{ stuff.items[0] }}\n",
    );
    write(
        root,
        "src/pages.liquid",
        "---\nlayout: base.liquid\npagination:\n  data: stuff.items\n  size: 1\n---\n{{ pagination.items[0] }}\n",
    );
    write(root, "src/plain.liquid", "plain\n");
    dir
}

fn codes_of(report: &BuildReport, template: &str) -> Vec<&'static str> {
    let survey = report.survey.as_ref().expect("a survey");
    let t = &survey.templates[Path::new(template)];
    t.problems.iter().map(|p| p.code).collect()
}

#[test]
fn a_plain_build_reports_only_the_layout() {
    let dir = masked_site();
    let report = build(&opts(dir.path()), &MarkdownIt::new());
    assert!(report.survey.is_none());
    let codes: Vec<&str> = report.problems.iter().map(|p| p.code).collect();
    assert_eq!(codes, [codes::LIQUID_UNKNOWN_FILTER]);
}

#[test]
fn a_survey_records_what_the_layout_hid() {
    let dir = masked_site();
    let report = survey(&opts(dir.path()), &MarkdownIt::new(), true);
    // What is reported is unchanged.
    let codes: Vec<&str> = report.problems.iter().map(|p| p.code).collect();
    assert_eq!(codes, [codes::LIQUID_UNKNOWN_FILTER]);
    assert_eq!(
        codes_of(&report, "src/reads.liquid"),
        [codes::LIQUID_UNKNOWN_FILTER, codes::DATA_JS_UNSUPPORTED]
    );
    let pages = codes_of(&report, "src/pages.liquid");
    assert!(pages.contains(&codes::LIQUID_UNKNOWN_FILTER), "{pages:?}");
    assert!(pages.contains(&codes::DATA_JS_UNSUPPORTED), "{pages:?}");
    let s = report.survey.as_ref().unwrap();
    assert_eq!(
        s.templates[Path::new("src/reads.liquid")].layouts,
        [PathBuf::from("src/_includes/base.liquid")]
    );
    assert_eq!(s.templates[Path::new("src/plain.liquid")].written, 1);
    assert_eq!(s.templates[Path::new("src/reads.liquid")].refused, 1);
    assert_eq!(s.templates[Path::new("src/pages.liquid")].refused, 1);
    assert_eq!(
        s.js_keys.get("stuff"),
        Some(&PathBuf::from("src/_data/stuff.js"))
    );
}

#[test]
fn a_dry_run_writes_nothing() {
    let dir = masked_site();
    write(dir.path(), "static/a.txt", "a");
    let mut o = opts(dir.path());
    o.passthrough = vec![PathBuf::from("static")];
    let report = survey(&o, &MarkdownIt::new(), true);
    assert_eq!(report.written, 1);
    assert_eq!(report.copied, 1);
    assert!(!dir.path().join("_site").exists());
}

#[test]
fn a_survey_that_writes_matches_a_build() {
    let dir = masked_site();
    let mut o = opts(dir.path());
    o.output = dir.path().join("out");
    let report = survey(&o, &MarkdownIt::new(), false);
    assert_eq!(report.written, 1);
    assert_eq!(
        fs::read_to_string(dir.path().join("out/plain/index.html")).unwrap(),
        "plain\n"
    );
}
