use brrrty_core::{BuildOptions, Severity, build, codes};
use brrrty_markdown::MarkdownIt;
use std::fs;
use std::path::{Path, PathBuf};

fn opts(root: &Path) -> BuildOptions {
    opts_with_passthrough(root, &[])
}

fn opts_with_passthrough(root: &Path, pt: &[&str]) -> BuildOptions {
    BuildOptions {
        write_to: None,
        run_mode: Default::default(),
        root: root.to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        passthrough: pt.iter().map(PathBuf::from).collect(),
        strict: false,
        config: Default::default(),
        js: Default::default(),
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
    let r = build(
        &opts_with_passthrough(root, &["src/images"]),
        &MarkdownIt::new(),
    );
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
    let r = build(&opts(root), &MarkdownIt::new());
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
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(root.join("_site/good/index.html").exists());
    let found: Vec<_> = r.problems.iter().map(|p| p.code).collect();
    assert!(
        found.contains(&codes::FRONT_MATTER_INVALID) && found.contains(&codes::TEMPLATE_NOT_UTF8)
    );
    assert!(r.problems.iter().all(|p| p.path.is_some()));
    assert!(r.has_errors());
}

#[test]
fn unsupported_templates_are_errors() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/c.11ty.js", "module.exports = 1;");
    write(root, "src/ok.md", "ok\n");
    let r = build(&opts(root), &MarkdownIt::new());
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::TEMPLATE_UNSUPPORTED)
        .unwrap();
    assert_eq!(
        p.message,
        "this JavaScript template doesn't run while JavaScript is off"
    );
    let help = p.help.as_deref().unwrap_or_default();
    assert!(help.contains("config.set_js_data(\"node\")"), "{help}");
    assert!(help.contains("--js-data=node"), "{help}");
    assert!(root.join("_site/ok/index.html").exists());
}

#[test]
fn collisions_write_nothing_and_name_both_inputs() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "---\npermalink: /same/\n---\na\n");
    write(root, "src/b.md", "---\npermalink: /same/\n---\nb\n");
    write(root, "src/c.md", "c\n");
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(!root.join("_site/same/index.html").exists());
    assert!(root.join("_site/c/index.html").exists());
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::OUTPUT_COLLISION)
        .unwrap();
    assert!(p.message.contains("src/a.md") && p.message.contains("src/b.md"));
    assert_eq!(p.path.as_deref(), Some(Path::new("src/a.md")));
    assert_eq!(
        r.problems
            .iter()
            .filter(|p| p.code == codes::OUTPUT_COLLISION)
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
    let r = build(&o, &MarkdownIt::new());
    assert!(
        r.problems
            .iter()
            .any(|p| p.code == codes::OUTPUT_OVERLAPS_SOURCE)
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
        &MarkdownIt::new(),
    );
    assert_eq!(
        r.problems
            .iter()
            .filter(|p| p.code == codes::PASSTHROUGH_ESCAPES)
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
    let r = build(
        &opts_with_passthrough(root, &["src/nope"]),
        &MarkdownIt::new(),
    );
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::PASSTHROUGH_MISSING)
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
        &MarkdownIt::new(),
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
    build(&o, &MarkdownIt::new());
    let r = build(&o, &MarkdownIt::new());
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
    let r = build(&o, &MarkdownIt::new());
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
    let r = build(
        &opts_with_passthrough(root, &["robots.txt"]),
        &MarkdownIt::new(),
    );
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::PASSTHROUGH_OVERRIDES_PAGE)
        .unwrap();
    assert!(matches!(p.severity, Severity::Warning));
    assert!(p.message.contains("src/robots.md") && p.message.contains("robots.txt"));
    assert_eq!(p.path.as_deref(), Some(Path::new("src/robots.md")));
    assert_eq!(read(root.join("_site/robots.txt")), "passthrough");
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert_eq!((r.written, r.copied), (0, 1));
}

#[test]
fn passthrough_of_input_or_root_is_refused_clearly() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    let r = build(
        &opts_with_passthrough(root, &["src", "."]),
        &MarkdownIt::new(),
    );
    assert_eq!(
        r.problems
            .iter()
            .filter(|p| p.code == codes::PASSTHROUGH_IS_INPUT)
            .count(),
        2
    );
    assert!(r.problems.iter().all(|p| p.code != codes::OUTPUT_ESCAPES));
}

#[test]
fn duplicate_passthrough_entries_count_once_and_warn_once() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/games/a.html", "<p>page</p>\n");
    write(root, "src/games/index.html", "<p>page</p>\n");
    let o = opts_with_passthrough(root, &["src/games", "src/games", "src/games/a.html"]);
    let r = build(&o, &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    // games/index.html is overwritten by its identical passthrough copy, so that page is not "written".
    assert_eq!((r.written, r.copied), (1, 2));
    let warnings = r
        .problems
        .iter()
        .filter(|p| p.code == codes::PASSTHROUGH_OVERRIDES_PAGE)
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
    let r = build(&o, &MarkdownIt::new());
    let warnings = r
        .problems
        .iter()
        .filter(|p| p.code == codes::PASSTHROUGH_OVERRIDES_PAGE)
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
    let r = build(&o, &MarkdownIt::new());
    assert_eq!(r.written, 0);
    assert_eq!(r.copied, 2);
    assert_eq!(
        r.problems
            .iter()
            .filter(|p| p.code == codes::PASSTHROUGH_OVERRIDES_PAGE)
            .count(),
        1
    );
}

#[test]
fn missing_input_folder_is_one_clear_error() {
    let t = tempfile::tempdir().unwrap();
    let r = build(&opts(t.path()), &MarkdownIt::new());
    assert_eq!(r.problems.len(), 1, "{:?}", r.problems);
    let p = &r.problems[0];
    assert_eq!(p.code, codes::INPUT_MISSING);
    assert_eq!(p.path.as_deref(), Some(Path::new("src")));
    assert!(!p.message.contains(t.path().to_str().unwrap()), "{p:?}");
}

#[test]
fn input_that_is_a_file_is_a_clear_error() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "src", "not a folder");
    let r = build(&opts(t.path()), &MarkdownIt::new());
    assert_eq!(r.problems.len(), 1, "{:?}", r.problems);
    assert_eq!(r.problems[0].code, codes::INPUT_NOT_A_DIRECTORY);
    assert_eq!(r.problems[0].path.as_deref(), Some(Path::new("src")));
}

/// Probed: pagination from a page or from a layout's front matter.
#[test]
fn pages_and_layouts_paginate() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/b.md",
        "---\ns: [1, 2]\npagination:\n  data: s\n  size: 1\n---\nb{{ pagination.items[0] }}\n",
    );
    // A layout's front matter is page data too.
    write(
        root,
        "src/_includes/paged.liquid",
        "---\npagination:\n  data: s\n  size: 1\n---\n[{{ content }}]",
    );
    write(
        root,
        "src/d.md",
        "---\nlayout: paged\ns: [x, y]\n---\n{{ pagination.items[0] }} {{ page.url }}\n",
    );
    write(root, "src/c.md", "---\ntitle: ok\ntags: [x]\n---\nbody\n");
    let r = build(&opts(root), &MarkdownIt::new());
    assert_eq!(error_count(&r), 0, "{:?}", r.problems);
    let read = |rel: &str| fs::read_to_string(root.join("_site").join(rel)).unwrap();
    assert_eq!(read("b/index.html"), "<p>b1</p>\n");
    assert_eq!(read("b/1/index.html"), "<p>b2</p>\n");
    assert_eq!(read("d/index.html"), "[<p>x /d/</p>\n]");
    assert_eq!(read("d/1/index.html"), "[<p>y /d/1/</p>\n]");
    assert!(root.join("_site/c/index.html").exists());
}

#[test]
fn eleventy_config_files_warn_once_each() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    write(root, "eleventy.config.js", "module.exports = () => {};");
    write(root, ".eleventy.js", "module.exports = () => {};");
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    let warned: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::ELEVENTY_CONFIG_IGNORED)
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
    assert!(
        build(&opts(t2.path()), &MarkdownIt::new())
            .problems
            .is_empty()
    );
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
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert_eq!(r.written, 4);
}

/// 11ty 3.1.8 dates `Last Modified` by the file's change time (`ctimeMs`,
/// `Template.js`), not its modification time: a file whose modification
/// time is set back to 2001 is still dated by when it last changed (now).
#[test]
fn last_modified_is_the_change_time() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/p.liquid",
        "---\ndate: Last Modified\n---\n{{ page.date | date: \"%Y\" }}",
    );
    let file = fs::File::options()
        .write(true)
        .open(root.join("src/p.liquid"))
        .unwrap();
    let y2001 = std::time::UNIX_EPOCH + std::time::Duration::from_secs(978_307_200 + 86_400 * 180);
    file.set_modified(y2001).unwrap();
    drop(file);
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    let year = read(root.join("_site/p/index.html"));
    assert_ne!(year, "2001");
    assert!(year.starts_with("20"), "{year}");
}

/// A page that uses data from a `_data` file that failed says to fix the
/// error "reported above": the data file's error comes first, though
/// `src/About.md` sorts before `src/_data/site.json`, and in strict mode
/// it is the one error kept.
#[test]
fn a_data_file_error_comes_before_the_pages_it_stops() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/site.json", "{ bad json\n");
    write(root, "src/About.md", "{{ site.title }}\n");
    write(root, "src/2024-01-02-post.md", "{{ site.title }}\n");
    let r = build(&opts(root), &MarkdownIt::new());
    let paths: Vec<_> = r
        .problems
        .iter()
        .map(|p| (p.path.clone().unwrap(), p.follows))
        .collect();
    assert_eq!(
        paths,
        [
            (PathBuf::from("src/_data/site.json"), false),
            (PathBuf::from("src/2024-01-02-post.md"), true),
            (PathBuf::from("src/About.md"), true),
        ]
    );
    assert_eq!(r.problems[0].code, codes::DATA_FILE_INVALID);
    assert!(
        r.problems[1]
            .help
            .as_deref()
            .unwrap()
            .contains("reported above")
    );

    let mut strict = opts(root);
    strict.strict = true;
    let r = build(&strict, &MarkdownIt::new());
    assert_eq!(r.problems.len(), 1, "{:?}", r.problems);
    assert_eq!(r.problems[0].code, codes::DATA_FILE_INVALID);
}

#[test]
fn problem_paths_are_root_relative() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/bad.md", "---\ntitle: [unclosed\n---\n");
    write(root, "src/posts/raw.md", "{{ x | nope }}\n");
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(!r.problems.is_empty());
    for p in &r.problems {
        let path = p.path.as_deref().expect("every problem has a path");
        assert!(path.starts_with("src"), "{path:?}");
        assert!(path.is_relative(), "{path:?}");
    }
    assert!(
        r.problems
            .iter()
            .any(|p| p.path.as_deref() == Some(Path::new("src/bad.md"))
                && p.code == codes::FRONT_MATTER_INVALID)
    );
    assert!(
        r.problems
            .iter()
            .any(|p| p.path.as_deref() == Some(Path::new("src/posts/raw.md"))
                && p.code == codes::LIQUID_UNKNOWN_FILTER)
    );
}

#[test]
fn passthrough_and_discover_problem_paths_are_root_relative() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/index.md", "hi\n");
    write(root, "src/old.11ty.js", "x\n");
    write(root, "src/a.html", "<p>a</p>\n");
    write(root, "src/a/index.md", "x\n");
    write(root, "src/images/a.html", "<p>other</p>\n");
    let r = build(
        &opts_with_passthrough(root, &["./src/gone", "../x", "src/images"]),
        &MarkdownIt::new(),
    );
    let path_of = |code: &str| {
        r.problems
            .iter()
            .find(|p| p.code == code)
            .and_then(|p| p.path.clone())
    };
    assert_eq!(
        path_of(codes::TEMPLATE_UNSUPPORTED),
        Some("src/old.11ty.js".into())
    );
    assert_eq!(path_of(codes::PASSTHROUGH_MISSING), Some("src/gone".into()));
    assert_eq!(path_of(codes::PASSTHROUGH_ESCAPES), Some("../x".into()));
    for p in &r.problems {
        assert!(!p.path.as_ref().unwrap().is_absolute(), "{p:?}");
    }
}

#[test]
fn write_failures_report_root_relative_paths() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    write(root, "src/b.md", "b\n");
    // A folder where a.md's output file belongs makes the write fail.
    fs::create_dir_all(root.join("_site/a/index.html")).unwrap();
    // A file where b.md's output folder belongs makes the folder creation fail.
    write(root, "_site/b", "in the way");
    let r = build(&opts(root), &MarkdownIt::new());
    let paths: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::WRITE_FAILED)
        .map(|p| p.path.clone().unwrap())
        .collect();
    assert_eq!(paths.len(), 2, "{:?}", r.problems);
    assert!(paths.contains(&PathBuf::from("src/a.md")), "{paths:?}");
    assert!(paths.contains(&PathBuf::from("src/b.md")), "{paths:?}");
}

#[test]
fn ignore_rules_match_11ty_probe() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    for f in [
        "src/deep.md",
        "src/top.md",
        "src/sub/deep.md",
        "src/sub/top.md",
        "src/sub/keep.md",
        "src/sub/deeper/x.md",
        "src/drafts/a.md",
        "src/sub/drafts/b.md",
        "src/index.md",
    ] {
        write(root, f, "x\n");
    }
    write(root, ".gitignore", "deep.md\ndrafts\n");
    write(root, "src/.eleventyignore", "*.md\n");
    write(root, ".eleventyignore", "sub/deeper\n");
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    let mut pages = Vec::new();
    collect(&root.join("_site"), &root.join("_site"), &mut pages);
    pages.sort();
    assert_eq!(
        pages,
        [
            "drafts/a/index.html",
            "sub/deep/index.html",
            "sub/deeper/x/index.html",
            "sub/drafts/b/index.html",
            "sub/keep/index.html",
            "sub/top/index.html",
        ]
    );

    let mut warns: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::IGNORE_LINE_ANCHORED)
        .collect();
    warns.sort_by_key(|p| p.line);
    assert_eq!(warns.len(), 2, "{warns:?}");
    assert_eq!(warns[0].path.as_deref(), Some(Path::new(".gitignore")));
    assert_eq!(warns[0].line, Some(1));
    assert!(
        warns[0].message.contains("src/sub/deep.md"),
        "{}",
        warns[0].message
    );
    assert!(!warns[0].message.contains("more"));
    assert_eq!(warns[1].line, Some(2));
    assert!(matches!(warns[1].severity, Severity::Warning));
    assert_eq!(
        warns[1].message,
        ".gitignore line 2 `drafts` only ignores `drafts` at the project root (11ty reads ignore lines as paths from the file's folder); it does not ignore src/drafts/a.md and 1 more"
    );
    assert_eq!(
        warns[1].help.as_deref(),
        Some("to ignore it, write the full path, like src/drafts")
    );
}

fn collect(base: &Path, dir: &Path, out: &mut Vec<String>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            collect(base, &p, out);
        } else {
            out.push(
                p.strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn symlinked_file_builds_at_link_path() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/real/target.md", "hi\n");
    fs::create_dir_all(root.join("src/posts")).unwrap();
    std::os::unix::fs::symlink("../real/target.md", root.join("src/posts/link.md")).unwrap();
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert!(root.join("_site/real/target/index.html").exists());
    assert!(root.join("_site/posts/link/index.html").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_dir_is_followed() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/real/target.md", "hi\n");
    std::os::unix::fs::symlink("real", root.join("src/linkdir")).unwrap();
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert!(root.join("_site/linkdir/target/index.html").exists());
}

#[cfg(unix)]
#[test]
fn symlink_loop_warns_and_finishes() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/index.md", "hi\n");
    write(root, "src/sub/page.md", "hi\n");
    std::os::unix::fs::symlink(".", root.join("src/loop")).unwrap();
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    let loops: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::SYMLINK_LOOP)
        .collect();
    assert_eq!(loops.len(), 1, "{:?}", r.problems);
    assert!(loops[0].path.as_ref().unwrap().starts_with("src"));
    assert!(matches!(loops[0].severity, Severity::Warning));
    assert!(root.join("_site/index.html").exists());
    assert!(root.join("_site/sub/page/index.html").exists());
}

#[test]
fn plugin_only_extensions_warn_once_per_extension() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.webc", "x");
    write(root, "src/b.webc", "x");
    write(root, "src/c.pug", "x");
    write(root, "src/d.mdx", "x");
    write(root, "src/ok.md", "ok\n");
    let r = build(&opts(root), &MarkdownIt::new());
    assert_eq!(error_count(&r), 0, "{:?}", r.problems);
    let w: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::TEMPLATE_PLUGIN_ONLY)
        .collect();
    assert_eq!(w.len(), 3, "{:?}", r.problems);
    let pug = w.iter().find(|p| p.message.contains(".pug")).unwrap();
    assert!(
        pug.message.starts_with("1 `.pug` file skipped:"),
        "{}",
        pug.message
    );
    let webc = w.iter().find(|p| p.message.contains(".webc")).unwrap();
    assert_eq!(
        webc.message,
        "2 `.webc` files skipped: in 11ty these are templates only when a plugin adds them"
    );
    assert_eq!(webc.path.as_deref(), Some(Path::new("src/a.webc")));
    assert!(root.join("_site/ok/index.html").exists());
}

#[cfg(unix)]
#[test]
fn dangling_symlink_warns_and_finishes() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/index.md", "hi\n");
    std::os::unix::fs::symlink("nowhere.md", root.join("src/dangling.md")).unwrap();
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    let w: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::SYMLINK_BROKEN)
        .collect();
    assert_eq!(w.len(), 1, "{:?}", r.problems);
    assert_eq!(w[0].path.as_deref(), Some(Path::new("src/dangling.md")));
    assert!(w[0].message.contains("nowhere.md"));
    assert!(root.join("_site/index.html").exists());
}

#[test]
fn duplicate_passthrough_destination_is_an_error() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    write(root, "src/assets/same.txt", "from src");
    write(root, "src/assets/one.txt", "one");
    write(root, "assets/same.txt", "from root");
    write(root, "assets/two.txt", "two");
    // Both entries land in _site/assets: one input-relative, one root-relative.
    let r = build(
        &opts_with_passthrough(root, &["src/assets/", "assets/"]),
        &MarkdownIt::new(),
    );
    let errs: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::PASSTHROUGH_COLLISION)
        .collect();
    assert_eq!(errs.len(), 1, "{:?}", r.problems);
    assert!(matches!(errs[0].severity, Severity::Error));
    let m = &errs[0].message;
    assert!(m.contains("src/assets/same.txt"), "{m}");
    assert!(m.contains("assets/same.txt"), "{m}");
    assert!(m.contains("_site/assets/same.txt"), "{m}");
    assert_eq!(
        errs[0].help.as_deref(),
        Some("remove one of the passthrough entries, or rename one of the files")
    );
    assert!(!root.join("_site/assets/same.txt").exists());
    assert_eq!(read(root.join("_site/assets/one.txt")), "one");
    assert_eq!(read(root.join("_site/assets/two.txt")), "two");
    assert_eq!(error_count(&r), 1);
}

/// Lays out a site whose input folder is `<t>/real`, with a root `.gitignore` line that
/// matches nothing at the root and an input `.eleventyignore` that skips `drafts`.
fn outside_input_site(t: &Path) -> PathBuf {
    let root = t.join("site");
    write(&root, ".gitignore", "drafts\n");
    write(t, "real/index.md", "hi\n");
    write(t, "real/drafts/a.md", "draft\n");
    write(t, "real/posts/2026-01-02-hello.md", "hi\n");
    write(t, "real/.eleventyignore", "drafts\n");
    root
}

fn check_outside_input_build(root: &Path, input: &str) {
    let o = BuildOptions {
        input: PathBuf::from(input),
        ..opts(root)
    };
    let r = build(&o, &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert!(root.join("_site/index.html").exists());
    assert!(
        root.join("_site/posts/2026-01-02-hello/index.html")
            .exists()
    );
    // `<input>/.eleventyignore` is honored, so the draft is not built.
    assert!(!root.join("_site/drafts").exists(), "{:?}", r.problems);
    // The root `.gitignore` line can't reach the input folder, so there is nothing to warn about.
    assert!(
        r.problems
            .iter()
            .all(|p| p.code != codes::IGNORE_LINE_ANCHORED),
        "{:?}",
        r.problems
    );
}

#[test]
fn input_outside_the_root_builds_and_honors_its_eleventyignore() {
    let t = tempfile::tempdir().unwrap();
    let root = outside_input_site(t.path());
    check_outside_input_build(&root, "../real");
}

#[cfg(unix)]
#[test]
fn symlinked_input_builds_and_honors_its_eleventyignore() {
    let t = tempfile::tempdir().unwrap();
    let root = outside_input_site(t.path());
    std::os::unix::fs::symlink("../real", root.join("src")).unwrap();
    check_outside_input_build(&root, "src");
}

#[cfg(unix)]
#[test]
fn symlinked_input_reports_paths_and_dates_through_the_link() {
    let t = tempfile::tempdir().unwrap();
    // A date in a folder above the root is not part of the page path, so this impossible
    // one must not turn into a `date-invalid` error.
    let base = t.path().join("2026-13-45-archive");
    let root = base.join("site");
    write(&base, "real/bad.md", "---\npermalink: \"{{ x }}\"\n---\n");
    write(&base, "real/plain.md", "hi\n");
    fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink("../real", root.join("src")).unwrap();
    let r = build(&opts(&root), &MarkdownIt::new());
    let paths: Vec<_> = r.problems.iter().filter_map(|p| p.path.clone()).collect();
    assert_eq!(paths, [PathBuf::from("src/bad.md")], "{:?}", r.problems);
    assert!(root.join("_site/plain/index.html").exists());
}

#[test]
fn ignore_line_matching_a_root_file_outside_the_input_does_not_warn() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "README.md", "readme\n");
    write(root, "src/a.md", "a\n");
    write(root, ".gitignore", "*.md\n");
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert!(
        r.problems
            .iter()
            .all(|p| p.code != codes::IGNORE_LINE_ANCHORED),
        "{:?}",
        r.problems
    );
    assert!(root.join("_site/a/index.html").exists());
}

#[test]
fn invalid_ignore_line_warns_and_the_build_goes_on() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    write(root, "src/skip.md", "skip\n");
    write(root, "src/.eleventyignore", "{a,b\nskip.md\n");
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    let w: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::IGNORE_LINE_INVALID)
        .collect();
    assert_eq!(w.len(), 1, "{:?}", r.problems);
    assert_eq!(w[0].path.as_deref(), Some(Path::new("src/.eleventyignore")));
    assert_eq!(w[0].line, Some(1));
    assert!(w[0].message.contains("`{a,b`"), "{}", w[0].message);
    assert!(root.join("_site/a/index.html").exists());
    assert!(!root.join("_site/skip").exists());
}

/// A strikethrough that spans a CRLF line break inside a list item used to overflow an integer
/// in the markdown-it port (a panic in debug builds). It renders as markdown-it 14 does
/// (`markdownIt().render(...)` printed by Node) in every build.
#[test]
fn a_strikethrough_across_crlf_in_a_list_renders() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "src/ok.md", "# fine\n");
    write(t.path(), "src/bad.md", "2) `~~中\r\n---~~(![i](u)");
    let r = build(&opts(t.path()), &MarkdownIt::new());
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(
        read(t.path().join("_site/bad/index.html")),
        "<ol start=\"2\">\n<li>`<s>中\n---</s>(<img src=\"u\" alt=\"i\"></li>\n</ol>\n"
    );
    assert_eq!(
        read(t.path().join("_site/ok/index.html")),
        "<h1>fine</h1>\n"
    );
}

/// A renderer that panics on any page containing `PANIC`, in every build profile.
struct PanickyMarkdown(MarkdownIt);

impl brrrty_markdown::MarkdownRenderer for PanickyMarkdown {
    fn render(&self, source: &str) -> String {
        if source.contains("PANIC") {
            panic!("deliberate renderer panic");
        }
        self.0.render(source)
    }
}

/// Containment works in release builds too, and on every Rayon worker: each panicking page is
/// refused with the panic's message, and every other page is written.
#[test]
fn renderer_panics_refuse_their_pages_on_every_thread() {
    let t = tempfile::tempdir().unwrap();
    for i in 0..40 {
        write(t.path(), &format!("src/ok{i}.md"), "# fine\n");
        write(t.path(), &format!("src/bad{i}.md"), "PANIC\n");
    }
    let r = build(&opts(t.path()), &PanickyMarkdown(MarkdownIt::new()));
    let bad: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::MARKDOWN_RENDER_FAILED)
        .collect();
    assert_eq!(bad.len(), 40, "{:?}", r.problems);
    assert_eq!(r.problems.len(), 40, "{:?}", r.problems);
    assert!(bad[0].message.contains("deliberate renderer panic"));
    for i in 0..40 {
        assert_eq!(
            read(t.path().join(format!("_site/ok{i}/index.html"))),
            "<h1>fine</h1>\n"
        );
        assert!(!t.path().join(format!("_site/bad{i}/index.html")).exists());
    }
}

#[test]
fn write_to_writes_elsewhere_but_keeps_the_output_spelling() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "index.md",
        "{{ page.outputPath }} {{ eleventy.directories.output }} {{ eleventy.env.runMode }}",
    );
    write(root, "img/a.png", "png");
    // An old build in the real output folder must not be read as input.
    write(root, "_site/old/index.html", "old");
    let stage = root.join("_site/.stage/1");
    let o = BuildOptions {
        input: PathBuf::from("."),
        write_to: Some(stage.clone()),
        run_mode: brrrty_core::RunMode::Serve,
        ..opts_with_passthrough(root, &["img"])
    };
    let r = build(&o, &MarkdownIt::new());
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(
        read(stage.join("index.html")),
        "<p>./_site/index.html ./_site/ serve</p>\n"
    );
    assert_eq!(read(stage.join("img/a.png")), "png");
    assert!(!stage.join("old").exists());
    assert!(!stage.join("_site").exists());
    assert!(!root.join("_site/index.html").exists());
}

#[test]
fn a_build_removes_a_stale_serve_staging_folder_and_nothing_else() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/index.md", "hi");
    write(root, "_site/.brrrty-serve/build-3/index.html", "stale");
    // Look-alikes that are not Brrrty's: other names, other depths, outside the output.
    write(root, "_site/.brrrty-serve-other/x.html", "keep");
    write(root, "_site/sub/.brrrty-serve/y.html", "keep");
    write(root, ".brrrty-serve/z.html", "keep");
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert!(!root.join("_site/.brrrty-serve").exists());
    for kept in [
        "_site/.brrrty-serve-other/x.html",
        "_site/sub/.brrrty-serve/y.html",
        ".brrrty-serve/z.html",
    ] {
        assert_eq!(read(root.join(kept)), "keep", "{kept}");
    }
    assert_eq!(read(root.join("_site/index.html")), "<p>hi</p>\n");
}

#[cfg(unix)]
#[test]
fn a_stale_serve_staging_symlink_is_removed_not_followed() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/index.md", "hi");
    write(root, "elsewhere/keep.txt", "keep");
    fs::create_dir_all(root.join("_site")).unwrap();
    std::os::unix::fs::symlink(root.join("elsewhere"), root.join("_site/.brrrty-serve")).unwrap();
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert!(fs::symlink_metadata(root.join("_site/.brrrty-serve")).is_err());
    assert_eq!(read(root.join("elsewhere/keep.txt")), "keep");
}

#[test]
fn the_report_lists_written_pages_and_passthrough_files() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/index.md", "hi");
    write(root, "src/about.md", "---\npermalink: /about/me/\n---\nme");
    write(root, "src/draft.md", "---\npermalink: false\n---\nno");
    write(root, "src/css/site.css", "p{}");
    write(root, "assets/logo.svg", "<svg/>");
    let r = build(
        &opts_with_passthrough(root, &["src/css", "assets"]),
        &MarkdownIt::new(),
    );
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    let mut pages: Vec<(String, String, String)> = r
        .pages
        .iter()
        .map(|p| {
            (
                p.source.display().to_string(),
                p.url.clone(),
                p.output.display().to_string(),
            )
        })
        .collect();
    pages.sort();
    assert_eq!(
        pages,
        [
            (
                "src/about.md".into(),
                "/about/me/".into(),
                "about/me/index.html".into()
            ),
            ("src/index.md".into(), "/".into(), "index.html".into()),
        ]
    );
    let mut files = r.passthrough.clone();
    files.sort();
    assert_eq!(
        files,
        [
            (
                PathBuf::from("assets/logo.svg"),
                PathBuf::from("assets/logo.svg")
            ),
            (
                PathBuf::from("src/css/site.css"),
                PathBuf::from("css/site.css")
            ),
        ]
    );
    // An unchanged file is still listed on the next build.
    let again = build(
        &opts_with_passthrough(root, &["src/css", "assets"]),
        &MarkdownIt::new(),
    );
    assert_eq!(again.unchanged, 2);
    assert_eq!(again.passthrough.len(), 2);
}

#[test]
fn the_report_lists_refused_templates_and_hashes_what_it_wrote() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/index.md", "hi");
    write(root, "src/broken.liquid", "{% if %}\n");
    write(root, "src/bad-front.md", "---\n: [\n---\nx");
    write(
        root,
        "src/missing-layout.md",
        "---\nlayout: nope.liquid\n---\nx",
    );
    write(
        root,
        "src/draft.liquid",
        "---\npermalink: false\n---\n{% if %}",
    );
    let r = build(&opts(root), &MarkdownIt::new());
    assert!(r.complete);
    assert!(r.has_errors());
    let mut refused: Vec<String> = r.refused.iter().map(|p| p.display().to_string()).collect();
    refused.sort();
    assert_eq!(
        refused,
        [
            "src/bad-front.md",
            "src/broken.liquid",
            "src/missing-layout.md"
        ]
    );
    assert_eq!(r.pages.len(), 1);
    let written = fs::read(root.join("_site/index.html")).unwrap();
    assert_eq!(r.pages[0].hash, brrrty_core::content_hash(&written));

    // Stopping before any page is written is not a complete build.
    let mut strict = opts(root);
    strict.strict = true;
    write(root, "src/x.njk", "x");
    assert!(!build(&strict, &MarkdownIt::new()).complete);
}

#[test]
fn many_passthrough_files_and_pages_land_intact_and_report_in_walk_order() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    let mut expected = Vec::new();
    for d in 0..12 {
        for f in 0..25 {
            let rel = format!("assets/d{d:02}/sub/f{f:02}.bin");
            write(root, &rel, format!("{d}-{f}").repeat(f + 1));
            expected.push((PathBuf::from(&rel), PathBuf::from(&rel)));
        }
        for p in 0..10 {
            write(
                root,
                &format!("src/s{d}/p{p}.md"),
                format!("---\npermalink: /s{d}/p{p}/deep/\n---\npage {d} {p}\n"),
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let run = root.join("assets/d00/sub/f00.bin");
        fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let o = opts_with_passthrough(root, &["assets"]);
    let r = build(&o, &MarkdownIt::new());
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!((r.written, r.copied, r.unchanged), (120, 300, 0));
    // Copies run in parallel, but the report keeps the order of the walk.
    assert_eq!(r.passthrough, expected);
    for (src, dest) in &expected {
        assert_eq!(
            fs::read(root.join(src)).unwrap(),
            fs::read(root.join("_site").join(dest)).unwrap()
        );
    }
    for d in 0..12 {
        for p in 0..10 {
            let page = read(root.join(format!("_site/s{d}/p{p}/deep/index.html")));
            assert!(page.contains(&format!("page {d} {p}")), "{page}");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &str| fs::metadata(root.join(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode("_site/assets/d00/sub/f00.bin"), 0o755);
        assert_eq!(
            mode("_site/assets/d00/sub/f01.bin"),
            mode("assets/d00/sub/f01.bin")
        );
    }
    let again = build(&o, &MarkdownIt::new());
    assert_eq!((again.copied, again.unchanged), (0, 300));
    assert_eq!(again.passthrough, expected);
}
