//! `eleventyComputed` through `build()`. Every expectation was probed with
//! real 11ty 3.1.8; the fixture `tests/fixtures/computed` pins the output
//! that 11ty builds, and these tests pin what Brrrty refuses or reports.

use brrrty_core::{BuildOptions, BuildReport, Problem, Severity, build, codes};
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

fn exists(root: &Path, rel: &str) -> bool {
    root.join("_site").join(rel).exists()
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

fn errors(r: &BuildReport) -> Vec<&Problem> {
    r.problems
        .iter()
        .filter(|p| matches!(p.severity, Severity::Error))
        .collect()
}

fn only_error(r: &BuildReport) -> &Problem {
    let errs = errors(r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    errs[0]
}

/// A page with `body` as its computed data, next to a page that builds.
fn with_computed(front: &str) -> (tempfile::TempDir, BuildReport) {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "src/bad.liquid",
        &format!("---\neleventyComputed:\n{front}---\nbad"),
    );
    write(t.path(), "src/ok.liquid", "ok");
    let r = run(t.path());
    (t, r)
}

fn assert_refused(front: &str, code: &str, needle: &str) {
    let (t, r) = with_computed(front);
    let p = only_error(&r);
    assert_eq!(p.code, code, "{p:?}");
    assert_eq!(p.path.as_deref(), Some(Path::new("src/bad.liquid")));
    assert!(p.message.contains(needle), "{}", p.message);
    assert!(!exists(t.path(), "bad/index.html"));
    assert_eq!(read(t.path(), "ok/index.html"), "ok");
}

/// Probed: 11ty aborts the whole build with "Dependency Cycle Found: a -> b
/// -> c -> a". Brrrty reports the page and builds the rest. A key that reads
/// itself is not a cycle.
#[test]
fn a_cycle_between_computed_keys_refuses_the_page() {
    assert_refused(
        "  a: \"{{ b }}\"\n  b: \"{{ c }}\"\n  c: \"{{ a }}\"\n",
        codes::COMPUTED_CYCLE,
        "computed values depend on each other in a loop: a -> b -> c -> a",
    );
    // The path is the one `dependency-graph` finds, from the first key in
    // insertion order that leads into the loop (probed).
    assert_refused(
        "  z: \"{{ y }}\"\n  x: \"{{ w }}\"\n  y: \"{{ x }}\"\n  w: \"{{ y }}\"\n",
        codes::COMPUTED_CYCLE,
        "computed values depend on each other in a loop: z -> y -> x -> w -> y",
    );
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "src/self.liquid",
        "---\ntitle: T\neleventyComputed:\n  title: \"over {{ title }}\"\n---\n{{ title }}",
    );
    let r = run(t.path());
    assert!(errors(&r).is_empty(), "{:?}", r.problems);
    assert_eq!(read(t.path(), "self/index.html"), "over T");
}

/// Probed: a Liquid error in a computed value aborts 11ty ("undefined
/// filter", "invalid value expression"). Brrrty names the key.
#[test]
fn a_liquid_error_in_a_computed_value_refuses_the_page() {
    assert_refused(
        "  a: \"{{ 1 | nosuchfilter }}\"\n",
        codes::LIQUID_UNKNOWN_FILTER,
        "eleventyComputed.a",
    );
    assert_refused(
        "  a: \"{{ b }}\"\n  b: \"{% if %}\"\n",
        codes::LIQUID_SYNTAX,
        "eleventyComputed.b",
    );
}

/// Probed: 11ty honors computed `layout` and `templateEngineOverride`; they
/// change how the page is built, so Brrrty refuses them until it can follow.
/// (A computed `permalink` is followed: see `tests/permalinks.rs`.)
#[test]
fn computed_keys_that_change_the_build_are_refused() {
    assert_refused("  layout: base\n", codes::COMPUTED_UNSUPPORTED, "`layout`");
    assert_refused(
        "  templateEngineOverride: md\n",
        codes::COMPUTED_UNSUPPORTED,
        "`templateEngineOverride`",
    );
    assert_refused(
        "  layout:\n    x: 1\n",
        codes::COMPUTED_UNSUPPORTED,
        "`layout`",
    );
    assert_refused(
        "  eleventy:\n    x: 1\n",
        codes::COMPUTED_UNSUPPORTED,
        "`eleventy`",
    );
    assert_refused("  pkg:\n    x: 1\n", codes::COMPUTED_UNSUPPORTED, "`pkg`");
    assert_refused("  page:\n    x: 1\n", codes::COMPUTED_UNSUPPORTED, "`page`");
}

/// Probed: `lodash.set` reads a dotted key as a path, so Brrrty refuses it.
#[test]
fn a_dotted_computed_key_is_refused() {
    assert_refused("  \"a.b\": x\n", codes::COMPUTED_UNSUPPORTED, "`a.b`");
    assert_refused("  \"a[0]\": x\n", codes::COMPUTED_UNSUPPORTED, "`a[0]`");
    assert_refused(
        "  a:\n    \"b.c\": x\n",
        codes::COMPUTED_UNSUPPORTED,
        "`a.b.c`",
    );
}

/// Probed: `content`, `collections` and `page.url` in computed data abort
/// 11ty ("Cannot override reserved Eleventy properties").
#[test]
fn reserved_names_in_computed_data_are_refused() {
    assert_refused("  content: x\n", codes::DATA_RESERVED, "`content`");
    assert_refused("  collections: x\n", codes::DATA_RESERVED, "`collections`");
    assert_refused("  page:\n    url: x\n", codes::DATA_RESERVED, "`page.url`");
}

/// Probed: a computed value that reads `collections` works in 11ty, which
/// computes it after collections exist. Brrrty places pages before it builds
/// collections, so the value is refused.
#[test]
fn a_computed_value_reading_collections_is_refused() {
    assert_refused(
        "  n: \"{{ collections.all | size }}\"\n",
        codes::DATA_UNSUPPORTED,
        "collections",
    );
}

/// Each refusal's help says what to do instead.
#[test]
fn refusals_say_what_to_do() {
    for (front, help) in [
        (
            "  layout: base\n",
            "Set `layout` in front matter or a data file instead of computing it.",
        ),
        (
            "  permalink:\n    build: /x/\n",
            "Set the `permalink` map in front matter or a data file, or compute `permalink` as a string.",
        ),
        (
            "  \"a.b\": x\n",
            "Rename the key so it isn't empty and has no `.`, `[` or `]`.",
        ),
        (
            "  n: \"{{ collections.all | size }}\"\n",
            "Read `collections` in the template itself, for example `{% assign posts = collections.post %}`.",
        ),
        (
            "  a: \"{{ b }}\"\n  b: \"{{ a }}\"\n",
            "Change one of these values so it doesn't read the next one.",
        ),
        (
            "  fresh:\n    \"0\": a\n",
            "Compute the whole `fresh` value instead, or set it in front matter.",
        ),
    ] {
        let (_t, r) = with_computed(front);
        let p = only_error(&r);
        assert_eq!(p.help.as_deref(), Some(help), "{front}");
        assert!(!p.message.contains("11ty"), "{}", p.message);
    }
}

/// Probed with the `lodash.set` rules: an index-like key under a missing
/// parent makes an array, and a key under an existing array sets a property
/// on it. Neither is a plain object, so Brrrty refuses both.
#[test]
fn computed_paths_that_lodash_turns_into_arrays_are_refused() {
    assert_refused(
        "  fresh:\n    \"0\": a\n    \"1\": b\n",
        codes::COMPUTED_UNSUPPORTED,
        "`fresh.0`",
    );
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "src/bad.liquid",
        "---\narr: [1, 2]\neleventyComputed:\n  arr:\n    k: v\n---\nbad",
    );
    let r = run(t.path());
    let p = only_error(&r);
    assert_eq!(p.code, codes::COMPUTED_UNSUPPORTED);
    assert!(p.message.contains("`arr.k`"), "{}", p.message);
    // An index key under an object that exists is a plain property.
    write(
        t.path(),
        "src/good.liquid",
        "---\nord:\n  b: 1\neleventyComputed:\n  ord:\n    \"9\": nine\n---\n{{ ord | json }}",
    );
    let r = run(t.path());
    assert_eq!(errors(&r).len(), 1, "{:?}", r.problems);
    assert_eq!(read(t.path(), "good/index.html"), r#"{"9":"nine","b":1}"#);
}

/// Probed: computed values run on each generated page, after its
/// `pagination` and `page.url` are set.
#[test]
fn computed_data_runs_on_each_paginated_page() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "src/p.liquid",
        "---\ns: [a, b]\npagination:\n  data: s\n  size: 1\n  alias: l\neleventyComputed:\n  y: \"{{ l }}{{ pagination.pageNumber }}{{ page.url }}\"\n---\n{{ y }}",
    );
    let r = run(t.path());
    assert!(errors(&r).is_empty(), "{:?}", r.problems);
    assert_eq!(read(t.path(), "p/index.html"), "a0/p/");
    assert_eq!(read(t.path(), "p/1/index.html"), "b1/p/1/");
}

/// A computed value that reads a key from a JavaScript data file Brrrty
/// can't run is refused, naming the file, like any other page that reads it.
#[test]
fn a_computed_value_reading_javascript_data_is_refused() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "src/_data/site.js", "module.exports = {a: 1};");
    write(
        t.path(),
        "src/bad.liquid",
        "---\neleventyComputed:\n  x: \"{{ site.a }}\"\n---\nbad",
    );
    write(
        t.path(),
        "src/ok.liquid",
        "---\neleventyComputed:\n  x: \"fine\"\n---\n{{ x }}",
    );
    let r = run(t.path());
    let p = only_error(&r);
    assert_eq!(p.code, codes::DATA_JS_UNSUPPORTED);
    assert!(p.message.contains("`site`"), "{}", p.message);
    assert_eq!(read(t.path(), "ok/index.html"), "fine");
}

/// Computed data in a JavaScript data file holds functions: still refused,
/// for every page under it.
#[test]
fn computed_data_from_a_javascript_file_is_still_refused() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "src/c/c.11tydata.cjs",
        "module.exports = {\n  eleventyComputed: { title: (d) => d.x },\n};\n",
    );
    write(t.path(), "src/c/a.liquid", "a");
    write(t.path(), "src/ok.liquid", "ok");
    let r = run(t.path());
    let p = only_error(&r);
    assert_eq!(p.code, codes::DATA_JS_UNSUPPORTED);
    assert!(p.message.contains("`eleventyComputed`"), "{}", p.message);
    assert!(!exists(t.path(), "c/a/index.html"));
}

/// Computed values of a Nunjucks page run as Nunjucks (probed).
#[test]
fn nunjucks_pages_compute_with_nunjucks() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "src/n.njk",
        "---\ntitle: T\neleventyComputed:\n  x: \"{{ title | lower }}{# c #}\"\n---\n{{ x }}",
    );
    let r = run(t.path());
    assert!(r.problems.is_empty(), "{:#?}", r.problems);
    assert_eq!(read(t.path(), "n/index.html"), "t");
}

/// Probed: a computed value renders with the template's engine, so a
/// layout and a data file read it too; an empty `eleventyComputed` object
/// and a non-object one add nothing.
#[test]
fn computed_data_reaches_layouts_and_empty_or_odd_values_add_nothing() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "src/_includes/lay.liquid",
        "<{{ content }}|{{ c }}>",
    );
    write(
        t.path(),
        "src/a.liquid",
        "---\nlayout: lay\ntitle: T\neleventyComputed:\n  c: \"c-{{ title }}\"\n---\nbody",
    );
    write(
        t.path(),
        "src/e.liquid",
        "---\neleventyComputed: {}\n---\nempty",
    );
    write(
        t.path(),
        "src/s.liquid",
        "---\neleventyComputed: text\n---\nstring",
    );
    let r = run(t.path());
    assert!(errors(&r).is_empty(), "{:?}", r.problems);
    assert_eq!(read(t.path(), "a/index.html"), "<body|c-T>");
    assert_eq!(read(t.path(), "e/index.html"), "empty");
    assert_eq!(read(t.path(), "s/index.html"), "string");
}
