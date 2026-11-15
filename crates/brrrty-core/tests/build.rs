use brrrty_core::{BuildOptions, Severity, build};
use brrrty_markdown::Provisional;
use std::fs;
use std::path::{Path, PathBuf};

fn opts(root: &Path) -> BuildOptions {
    opts_with_passthrough(root, &[])
}

fn opts_with_passthrough(root: &Path, pt: &[&str]) -> BuildOptions {
    BuildOptions {
        root: root.to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        passthrough: pt.iter().map(PathBuf::from).collect(),
        strict: false,
    }
}

fn write(root: &Path, rel: &str, body: impl AsRef<[u8]>) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn read(p: PathBuf) -> String {
    fs::read_to_string(p).unwrap()
}

fn error_count(r: &brrrty_core::BuildReport) -> usize {
    r.problems
        .iter()
        .filter(|p| matches!(p.severity, Severity::Error))
        .count()
}

#[test]
fn builds_a_plain_site() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/index.md", "# Hello\n\nWorld\n");
    write(root, "src/page.html", "<p>html page</p>\n");
    write(root, "src/images/a.png", "x");
    let r = build(&opts_with_passthrough(root, &["src/images"]), &Provisional);
    assert!(!r.has_errors(), "{:?}", r.problems);
    let out = root.join("_site");
    assert_eq!(
        read(out.join("index.html")),
        "<h1>Hello</h1>\n<p>World</p>\n"
    );
    assert_eq!(read(out.join("page/index.html")), "<p>html page</p>\n");
    assert_eq!(read(out.join("images/a.png")), "x");
    assert_eq!((r.written, r.copied), (2, 1));
}

#[test]
fn permalink_false_writes_nothing() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/hidden.md", "---\npermalink: false\n---\nx\n");
    let r = build(&opts(root), &Provisional);
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert_eq!(r.written, 0);
}

#[test]
fn bad_page_does_not_stop_the_build() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/good.md", "ok\n");
    write(root, "src/bad.md", "---\ntitle: [unclosed\n---\n");
    write(root, "src/bin.md", [0xffu8, 0xfe]);
    let r = build(&opts(root), &Provisional);
    assert!(root.join("_site/good/index.html").exists());
    let codes: Vec<_> = r.problems.iter().map(|p| p.code).collect();
    assert!(codes.contains(&"front-matter-invalid") && codes.contains(&"template-not-utf8"));
    assert!(r.problems.iter().all(|p| p.path.is_some()));
    assert!(r.has_errors());
}

#[test]
fn unsupported_templates_are_errors() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/c.njk", "{{ x }}");
    write(root, "src/ok.md", "ok\n");
    let r = build(&opts(root), &Provisional);
    let p = r
        .problems
        .iter()
        .find(|p| p.code == "template-unsupported")
        .unwrap();
    assert_eq!(
        p.help.as_deref(),
        Some("Liquid and Nunjucks support is in progress.")
    );
    assert!(root.join("_site/ok/index.html").exists());
}

#[test]
fn collisions_write_nothing_and_name_both_inputs() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "---\npermalink: /same/\n---\na\n");
    write(root, "src/b.md", "---\npermalink: /same/\n---\nb\n");
    write(root, "src/c.md", "c\n");
    let r = build(&opts(root), &Provisional);
    assert!(!root.join("_site/same/index.html").exists());
    assert!(root.join("_site/c/index.html").exists());
    let p = r
        .problems
        .iter()
        .find(|p| p.code == "output-collision")
        .unwrap();
    assert!(p.message.contains("a.md") && p.message.contains("b.md"));
    assert_eq!(
        r.problems
            .iter()
            .filter(|p| p.code == "output-collision")
            .count(),
        1
    );
}

#[test]
fn overlapping_output_writes_nothing() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    let mut o = opts(root);
    o.output = PathBuf::from("src");
    let r = build(&o, &Provisional);
    assert!(
        r.problems
            .iter()
            .any(|p| p.code == "output-overlaps-source")
    );
    assert!(!root.join("src/a/index.html").exists());
}

#[test]
fn passthrough_cannot_escape_output() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    let r = build(
        &opts_with_passthrough(root, &["../outside", "/etc"]),
        &Provisional,
    );
    assert_eq!(
        r.problems
            .iter()
            .filter(|p| p.code == "passthrough-escapes")
            .count(),
        2
    );
    assert_eq!(r.copied, 0);
}

#[test]
fn passthrough_missing_is_a_warning() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    let r = build(&opts_with_passthrough(root, &["src/nope"]), &Provisional);
    let p = r
        .problems
        .iter()
        .find(|p| p.code == "passthrough-missing")
        .unwrap();
    assert!(matches!(p.severity, Severity::Warning));
    assert!(!r.has_errors());
}

#[test]
fn passthrough_outside_input_and_root_file_map_to_output_root() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    write(root, "assets/css/site.css", "body{}");
    write(root, "robots.txt", "User-agent: *");
    let r = build(
        &opts_with_passthrough(root, &["assets", "robots.txt"]),
        &Provisional,
    );
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert_eq!(read(root.join("_site/assets/css/site.css")), "body{}");
    assert_eq!(read(root.join("_site/robots.txt")), "User-agent: *");
    assert_eq!(r.copied, 2);
}

#[test]
fn second_build_skips_unchanged_passthrough() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    write(root, "src/images/a.png", "x");
    let o = opts_with_passthrough(root, &["src/images"]);
    build(&o, &Provisional);
    let r = build(&o, &Provisional);
    assert_eq!((r.unchanged, r.copied), (1, 0));
}

#[test]
fn strict_stops_at_first_error() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/bad1.md", "---\ntitle: [unclosed\n---\n");
    write(root, "src/bad2.md", "---\ntitle: [unclosed\n---\n");
    write(root, "src/good.md", "ok\n");
    let mut o = opts(root);
    o.strict = true;
    let r = build(&o, &Provisional);
    assert_eq!(error_count(&r), 1);
    assert!(!root.join("_site").exists());
}
