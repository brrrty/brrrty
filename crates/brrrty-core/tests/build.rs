use brrrty_core::{BuildOptions, Severity, build, codes};
use brrrty_markdown::MarkdownIt;
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
        config: Default::default(),
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
    write(root, "src/c.njk", "{{ x }}");
    write(root, "src/ok.md", "ok\n");
    let r = build(&opts(root), &MarkdownIt::new());
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::TEMPLATE_UNSUPPORTED)
        .unwrap();
    assert_eq!(
        p.help.as_deref(),
        Some("Nunjucks support is coming soon. Liquid, Markdown and HTML templates work now.")
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
    write(root, "src/old.njk", "x\n");
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
        Some("src/old.njk".into())
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
