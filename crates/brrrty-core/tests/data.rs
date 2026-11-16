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
            config: Default::default(),
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

/// Probed: `size`, `first` and `last` from `_data` print their values, and
/// from a JavaScript data file they are refused like any other key.
#[test]
fn size_first_and_last_are_ordinary_data_keys() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/size.json", "[5, 6, 7]");
    write(root, "src/_data/first.json", "\"F\"");
    write(root, "src/_data/last.json", "\"L\"");
    write(root, "src/a.liquid", "{{ size }} {{ first }} {{ last }}");
    write(root, "src/b.liquid", "{{ size | json }} [{{ nope }}]");
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(root, "a/index.html"), "567 F L");
    assert_eq!(read(root, "b/index.html"), "[5,6,7] []");
}

#[test]
fn size_first_and_last_from_javascript_are_refused() {
    for key in ["size", "first", "last"] {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        write(root, &format!("src/_data/{key}.js"), "module.exports = 7;");
        write(root, "src/a.liquid", &format!("{{{{ {key} }}}}"));
        write(root, "src/b.liquid", "plain");
        let r = run(root);
        let errs = errors(&r);
        assert_eq!(errs.len(), 1, "{key}: {:#?}", r.problems);
        assert_eq!(errs[0].code, codes::DATA_JS_UNSUPPORTED, "{key}");
        assert_eq!(errs[0].path.as_deref(), Some(Path::new("src/a.liquid")));
        assert!(!root.join("_site/a/index.html").exists(), "{key}");
        assert_eq!(read(root, "b/index.html"), "plain");
    }
}

/// Probed: `eleventy` is deep-merged over `_data/eleventy.json`, and `pkg`
/// replaces `_data/pkg.json` outright.
#[test]
fn eleventy_and_pkg_win_over_data_files() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "package.json",
        r#"{"name": "real", "version": "1.0.0"}"#,
    );
    write(
        root,
        "src/_data/pkg.json",
        r#"{"version": "fake", "extra": 1}"#,
    );
    write(
        root,
        "src/_data/eleventy.json",
        r#"{"version": "fake", "extra": 2}"#,
    );
    write(
        root,
        "src/a.liquid",
        "{{ pkg | json }} {{ eleventy.version }} {{ eleventy.extra }} {{ eleventy.generator }}",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        read(root, "a/index.html"),
        r#"{"name":"real","version":"1.0.0"} 3.1.8 2 Eleventy v3.1.8"#
    );
}

const COUNTERS: &str = "{% increment counter %}|{% increment counter %}|{% decrement size %}";

/// Probed: counters start from numbers in the data, so JSON `counter: 5` and
/// `size: 7` give `5|6|6`.
#[test]
fn counters_start_from_json_data() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/counter.json", "5");
    write(root, "src/_data/size.json", "7");
    write(root, "src/a.liquid", COUNTERS);
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(root, "a/index.html"), "5|6|6");
}

#[test]
fn counters_from_javascript_data_are_refused() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/counter.js", "module.exports = 5;");
    write(root, "src/_data/size.js", "module.exports = 7;");
    write(root, "src/a.liquid", COUNTERS);
    write(root, "src/b.liquid", "plain");
    let r = run(root);
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert_eq!(errs[0].code, codes::DATA_JS_UNSUPPORTED);
    assert!(!root.join("_site/a/index.html").exists());
    assert_eq!(read(root, "b/index.html"), "plain");
}

/// Runs a build on another thread, failing the test if it takes longer
/// than `secs` instead of hanging.
fn run_within(root: &Path, secs: u64) -> BuildReport {
    let root = root.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run(&root));
    });
    rx.recv_timeout(std::time::Duration::from_secs(secs))
        .unwrap_or_else(|_| panic!("the build took longer than {secs} s"))
}

/// Front matter whose `l{n}` repeats `l{n-1}` ten times through aliases.
fn laughs(levels: usize) -> String {
    let mut t = String::from("---\nl0: &l0 [x, x, x, x, x, x, x, x, x, x]\n");
    for i in 1..levels {
        let p = format!("*l{}", i - 1);
        t.push_str(&format!("l{i}: &l{i} [{}]\n", vec![p; 10].join(", ")));
    }
    t.push_str("---\n");
    t
}

/// 11ty builds a page whose aliases expand to 10^9 values (it only walks
/// what the page prints). Brrrty copies the data into each page, so it
/// refuses front matter that expands past a million values, at the alias
/// that goes over, and builds the rest of the site at once. Below that the
/// page builds.
#[test]
fn nested_yaml_aliases_are_refused_without_hanging() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/laughs.liquid",
        &format!("{}{{{{ l8 | size }}}}", laughs(9)),
    );
    write(
        root,
        "src/fine.liquid",
        &format!(
            "{}{{{{ l4 | size }}}}{{{{ l4[9][9][9][9][9] }}}}{{{{ l4 | json | size }}}}",
            laughs(5)
        ),
    );
    write(root, "src/other.md", "other\n");
    let r = run_within(root, 30);
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert_eq!(errs[0].code, codes::FRONT_MATTER_INVALID);
    assert_eq!(
        errs[0].path.as_deref(),
        Some(Path::new("src/laughs.liquid"))
    );
    assert!(
        errs[0]
            .message
            .contains("expands to more than 1000000 values"),
        "{}",
        errs[0].message
    );
    // `l5: &l5 [*l4, ...` on line 7; the tenth `*l4` goes over.
    assert_eq!((errs[0].line, errs[0].column), (Some(7), Some(55)));
    assert_eq!(read(root, "fine/index.html"), "10x422221");
    assert_eq!(read(root, "other/index.html"), "<p>other</p>\n");
}
