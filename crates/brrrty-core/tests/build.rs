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

#[test]
fn passthrough_overrides_page_with_warning() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/robots.md",
        "---\npermalink: /robots.txt\n---\npage\n",
    );
    write(root, "robots.txt", "passthrough");
    let r = build(&opts_with_passthrough(root, &["robots.txt"]), &Provisional);
    let p = r
        .problems
        .iter()
        .find(|p| p.code == "passthrough-overrides-page")
        .unwrap();
    assert!(matches!(p.severity, Severity::Warning));
    assert!(p.message.contains("robots.md") && p.message.contains("robots.txt"));
    assert_eq!(read(root.join("_site/robots.txt")), "passthrough");
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert_eq!((r.written, r.copied), (0, 1));
}

#[test]
fn passthrough_of_input_or_root_is_refused_clearly() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    let r = build(&opts_with_passthrough(root, &["src", "."]), &Provisional);
    assert_eq!(
        r.problems
            .iter()
            .filter(|p| p.code == "passthrough-is-input")
            .count(),
        2
    );
    assert!(r.problems.iter().all(|p| p.code != "output-escapes"));
}

#[test]
fn duplicate_passthrough_entries_count_once_and_warn_once() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/games/a.html", "<p>page</p>\n");
    write(root, "src/games/index.html", "<p>page</p>\n");
    let o = opts_with_passthrough(root, &["src/games", "src/games", "src/games/a.html"]);
    let r = build(&o, &Provisional);
    assert!(!r.has_errors(), "{:?}", r.problems);
    // games/index.html is overwritten by its identical passthrough copy, so that page is not "written".
    assert_eq!((r.written, r.copied), (1, 2));
    let warnings = r
        .problems
        .iter()
        .filter(|p| p.code == "passthrough-overrides-page")
        .count();
    assert_eq!(warnings, 0, "identical bytes do not warn");
}

#[test]
fn overriding_passthrough_with_different_bytes_warns_once_per_page() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/games/a.html", "<p>page</p>\n");
    write(root, "src/other.md", "hi\n");
    let o = opts_with_passthrough(root, &["src/games", "src/games/a.html", "src/games"]);
    write(
        root,
        "src/games/b.md",
        "---\npermalink: /games/b.txt\n---\n# b\n",
    );
    write(root, "src/games/b.txt", "different");
    let r = build(&o, &Provisional);
    let warnings = r
        .problems
        .iter()
        .filter(|p| p.code == "passthrough-overrides-page")
        .count();
    assert_eq!(warnings, 1, "{:?}", r.problems);
    assert_eq!(read(root.join("_site/games/b.txt")), "different");
    // Three pages render; games/b.txt is replaced by the copy. Copied: a.html, b.md, b.txt.
    assert_eq!(r.written, 2);
    assert_eq!(r.copied, 3);
}

#[test]
fn repeated_overriding_entry_does_not_underflow_written() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/games/b.md",
        "---\npermalink: /games/b.txt\n---\n# b\n",
    );
    write(root, "src/games/b.txt", "different");
    let o = opts_with_passthrough(root, &["src/games", "src/games"]);
    let r = build(&o, &Provisional);
    assert_eq!(r.written, 0);
    assert_eq!(r.copied, 2);
    assert_eq!(
        r.problems
            .iter()
            .filter(|p| p.code == "passthrough-overrides-page")
            .count(),
        1
    );
}

#[test]
fn missing_input_folder_is_one_clear_error() {
    let t = tempfile::tempdir().unwrap();
    let r = build(&opts(t.path()), &Provisional);
    assert_eq!(r.problems.len(), 1, "{:?}", r.problems);
    let p = &r.problems[0];
    assert_eq!(p.code, "input-missing");
    assert_eq!(p.path.as_deref(), Some(Path::new("src")));
    assert!(!p.message.contains(t.path().to_str().unwrap()), "{p:?}");
}

#[test]
fn input_that_is_a_file_is_a_clear_error() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "src", "not a folder");
    let r = build(&opts(t.path()), &Provisional);
    assert_eq!(r.problems.len(), 1, "{:?}", r.problems);
    assert_eq!(r.problems[0].code, "input-not-a-directory");
    assert_eq!(r.problems[0].path.as_deref(), Some(Path::new("src")));
}

#[test]
fn template_syntax_in_body_is_an_error_with_file_position() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/a.md",
        "---\ntitle: x\n---\nhello\n  {{ name }}\n",
    );
    write(root, "src/b.html", "<p>{% if x %}</p>\n");
    write(root, "src/ok.md", "fine { not syntax }\n");
    let r = build(&opts(root), &Provisional);
    let all: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == "template-syntax-unsupported")
        .collect();
    assert_eq!(all.len(), 2, "{:?}", r.problems);
    let a = all
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("a.md")))
        .unwrap();
    assert_eq!((a.line, a.column), (Some(5), Some(3)));
    assert_eq!(a.message, "Template syntax ({{ or {%) isn't supported yet");
    assert_eq!(
        a.help.as_deref(),
        Some("Liquid and Nunjucks support is in progress.")
    );
    let b = all
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("b.html")))
        .unwrap();
    assert_eq!((b.line, b.column), (Some(1), Some(4)));
    assert!(!root.join("_site/a/index.html").exists());
    assert!(!root.join("_site/b/index.html").exists());
    assert!(root.join("_site/ok/index.html").exists());
}

#[test]
fn unsupported_front_matter_keys_are_errors() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/a.md",
        "---\nlayout: base.njk\ntags: [x]\n---\nbody\n",
    );
    write(
        root,
        "src/b.md",
        "---\npagination:\n  data: x\neleventyComputed:\n  y: 1\n---\nbody\n",
    );
    write(root, "src/c.md", "---\ntitle: ok\ntags: [x]\n---\nbody\n");
    let r = build(&opts(root), &Provisional);
    let a = r
        .problems
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("a.md")))
        .unwrap();
    assert_eq!(a.code, "front-matter-unsupported");
    assert!(a.message.contains("`layout`"), "{}", a.message);
    assert_eq!(
        a.help.as_deref(),
        Some("Layouts, pagination, and computed data are in progress.")
    );
    let b = r
        .problems
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("b.md")))
        .unwrap();
    assert!(b.message.contains("`pagination`") && b.message.contains("`eleventyComputed`"));
    assert!(!root.join("_site/a/index.html").exists());
    assert!(root.join("_site/c/index.html").exists());
    assert_eq!(error_count(&r), 2);
}

#[test]
fn eleventy_config_files_warn_once_each() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    write(root, "eleventy.config.js", "module.exports = () => {};");
    write(root, ".eleventy.js", "module.exports = () => {};");
    let r = build(&opts(root), &Provisional);
    assert!(!r.has_errors(), "{:?}", r.problems);
    let warned: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == "eleventy-config-ignored")
        .collect();
    assert_eq!(warned.len(), 2);
    assert!(matches!(warned[0].severity, Severity::Warning));
    assert_eq!(
        warned[0].message,
        "Brrrty doesn't read your 11ty config yet"
    );
    assert_eq!(
        warned[0].help.as_deref(),
        Some(
            "Pass your input folder and passthrough paths as flags, like --input src --passthrough src/images."
        )
    );
    assert!(
        warned
            .iter()
            .any(|p| p.path.as_deref() == Some(Path::new("eleventy.config.js")))
    );
    assert!(root.join("_site/a/index.html").exists());
    // no config, no warning
    let t2 = tempfile::tempdir().unwrap();
    write(t2.path(), "src/a.md", "a\n");
    assert!(build(&opts(t2.path()), &Provisional).problems.is_empty());
}

#[test]
fn date_keywords_build_without_errors() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    for (i, kw) in [
        "Last Modified",
        "Created",
        "git Last Modified",
        "git Created",
    ]
    .iter()
    .enumerate()
    {
        write(
            root,
            &format!("src/p{i}.md"),
            format!("---\ndate: {kw}\n---\nx\n"),
        );
    }
    let r = build(&opts(root), &Provisional);
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert_eq!(r.written, 4);
}
