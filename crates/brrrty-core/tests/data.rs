//! Global data through `build()`. Behaviors were probed with real 11ty 3.1.8.

use brrrty_core::{BuildOptions, BuildReport, Severity, build, codes};
use brrrty_markdown::MarkdownIt;
use std::fs;
use std::path::{Path, PathBuf};

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join("_site").join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn run(root: &Path) -> BuildReport {
    build(
        &BuildOptions {
            root: root.to_path_buf(),
            input: PathBuf::from("src"),
            output: PathBuf::from("_site"),
            passthrough: Vec::new(),
            strict: false,
        },
        &MarkdownIt::new(),
    )
}

fn errors(r: &BuildReport) -> Vec<&brrrty_core::Problem> {
    r.problems
        .iter()
        .filter(|p| matches!(p.severity, Severity::Error))
        .collect()
}

#[test]
fn json_data_is_in_page_data_under_front_matter() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/_data/site.json",
        r#"{"title": "S", "n": {"a": 1}}"#,
    );
    write(root, "src/_data/site/extra.json", r#"{"k": "v"}"#);
    write(root, "src/_data/title.json", "\"Global\"");
    write(
        root,
        "src/a.liquid",
        "{{ site.title }} {{ site.n.a }} {{ site.extra.k }} {{ title }}\n",
    );
    write(
        root,
        "src/b.liquid",
        "---\ntitle: Own\nsite:\n  title: Mine\n---\n{{ title }} {{ site.title }} {{ site.n.a }}\n",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(root, "a/index.html"), "S 1 v Global\n");
    assert_eq!(read(root, "b/index.html"), "Own Mine 1\n");
}

#[test]
fn global_layout_and_permalink_apply() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/layout.json", "\"base.liquid\"");
    write(root, "src/_includes/base.liquid", "[{{ content }}]");
    write(root, "src/a.liquid", "hi\n");
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(root, "a/index.html"), "[hi\n]");
}

#[test]
fn invalid_json_is_reported_and_the_rest_builds() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/bad.json", "{\n \"a\": }");
    write(root, "src/_data/good.json", r#"{"v": 1}"#);
    write(root, "src/uses-bad.liquid", "{{ bad.a }}");
    write(root, "src/uses-good.liquid", "{{ good.v }}");
    write(root, "src/plain.liquid", "plain");
    let r = run(root);
    let errs = errors(&r);
    assert_eq!(errs.len(), 2, "{:#?}", r.problems);
    let file = errs
        .iter()
        .find(|p| p.code == codes::DATA_FILE_INVALID)
        .unwrap();
    assert_eq!(file.path.as_deref(), Some(Path::new("src/_data/bad.json")));
    assert_eq!((file.line, file.column), (Some(2), Some(7)));
    let page = errs
        .iter()
        .find(|p| p.code == codes::DATA_UNSUPPORTED)
        .unwrap();
    assert_eq!(page.path.as_deref(), Some(Path::new("src/uses-bad.liquid")));
    assert_eq!(read(root, "uses-good/index.html"), "1");
    assert_eq!(read(root, "plain/index.html"), "plain");
}

#[test]
fn javascript_data_refuses_only_pages_that_read_it() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/js.js", "module.exports = {v: 1};");
    write(root, "src/_data/json.json", r#"{"v": 2}"#);
    write(root, "src/reads-js.liquid", "{{ js.v }}");
    write(root, "src/reads-json.liquid", "{{ json.v }}");
    // The page's own data would merge with the JavaScript file's.
    write(
        root,
        "src/merges.liquid",
        "---\njs:\n  w: 1\n---\n{{ js.w }}",
    );
    let r = run(root);
    let mut bad: Vec<_> = errors(&r).iter().map(|p| p.path.clone().unwrap()).collect();
    bad.sort();
    assert_eq!(
        bad,
        [
            PathBuf::from("src/merges.liquid"),
            PathBuf::from("src/reads-js.liquid")
        ]
    );
    assert_eq!(read(root, "reads-json/index.html"), "2");
}

#[test]
fn a_javascript_file_for_a_build_key_refuses_every_page() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/layout.js", "module.exports = 'x';");
    write(root, "src/a.liquid", "a");
    let r = run(root);
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert!(errs[0].message.contains("`layout`"), "{}", errs[0].message);
}

#[test]
fn reserved_names_in_global_data_are_reported_once() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/content.json", "\"x\"");
    write(root, "src/a.liquid", "a");
    let r = run(root);
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert_eq!(errs[0].code, codes::DATA_RESERVED);
    assert_eq!(read(root, "a/index.html"), "a");
}
