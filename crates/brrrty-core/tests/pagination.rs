//! Pagination through `build()`. Every expectation was probed with real
//! 11ty 3.1.8; the fixture `tests/fixtures/pagination` pins the pages 11ty
//! builds, and these tests pin the object shape, what Brrrty refuses, and
//! where it reports a page error that stops 11ty's whole build.

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

/// `src/bad.liquid` with `front`, next to a page that builds.
fn with_front(front: &str) -> (tempfile::TempDir, BuildReport) {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "src/bad.liquid", &format!("---\n{front}---\nbad"));
    write(t.path(), "src/ok.liquid", "ok");
    let r = run(t.path());
    (t, r)
}

fn assert_refused(front: &str, code: &str, needle: &str) -> Problem {
    let (t, r) = with_front(front);
    let p = only_error(&r).clone();
    assert_eq!(p.code, code, "{p:?}");
    assert_eq!(p.path.as_deref(), Some(Path::new("src/bad.liquid")));
    assert!(p.message.contains(needle), "{}", p.message);
    assert_eq!(read(t.path(), "ok/index.html"), "ok");
    assert!(!exists(t.path(), "bad/index.html"));
    p
}

/// Probed: the whole `pagination` object, size 2, with default permalinks.
#[test]
fn the_pagination_object_matches_11ty() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/nums.json", r#"["a","b","c","d","e"]"#);
    write(
        root,
        "src/list.liquid",
        "---\npagination:\n  data: nums\n  size: 2\n---\n{{ pagination | json }}\nURL={{ page.url }}",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        read(root, "list/1/index.html"),
        r#"{"data":"nums","size":2,"pages":[["a","b"],["c","d"],["e"]],"items":["c","d"],"page":{"previous":["a","b"],"next":["e"],"first":["a","b"],"last":["e"]},"pageNumber":1,"previousPageLink":"/list/index.html","previous":"/list/index.html","nextPageLink":"/list/2/index.html","next":"/list/2/index.html","firstPageLink":"/list/index.html","lastPageLink":"/list/2/index.html","links":["/list/index.html","/list/1/index.html","/list/2/index.html"],"pageLinks":["/list/index.html","/list/1/index.html","/list/2/index.html"],"previousPageHref":"/list/","nextPageHref":"/list/2/","firstPageHref":"/list/","lastPageHref":"/list/2/","hrefs":["/list/","/list/1/","/list/2/"],"href":{"previous":"/list/","next":"/list/2/","first":"/list/","last":"/list/2/"}}
URL=/list/1/"#
    );
    assert_eq!(
        read(root, "list/index.html"),
        r#"{"data":"nums","size":2,"pages":[["a","b"],["c","d"],["e"]],"items":["a","b"],"page":{"previous":null,"next":["c","d"],"first":["a","b"],"last":["e"]},"pageNumber":0,"previousPageLink":null,"previous":null,"nextPageLink":"/list/1/index.html","next":"/list/1/index.html","firstPageLink":"/list/index.html","lastPageLink":"/list/2/index.html","links":["/list/index.html","/list/1/index.html","/list/2/index.html"],"pageLinks":["/list/index.html","/list/1/index.html","/list/2/index.html"],"previousPageHref":null,"nextPageHref":"/list/1/","firstPageHref":"/list/","lastPageHref":"/list/2/","hrefs":["/list/","/list/1/","/list/2/"],"href":{"previous":null,"next":"/list/1/","first":"/list/","last":"/list/2/"}}
URL=/list/"#
    );
    assert_eq!(r.written, 3);
}

/// Probed: size 1 unwraps `pages`, `page.*` and the alias to the item;
/// `page.outputPath` follows the extra page folder.
#[test]
fn size_one_unwraps_items_and_sets_the_alias() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/nums.json", r#"["a","b","c"]"#);
    write(
        root,
        "src/one.liquid",
        "---\npagination:\n  data: nums\n  size: 1\n  alias: n\n---\nn={{ n | json }} items={{ pagination.items | json }} pages={{ pagination.pages | json }} first={{ pagination.page.first }} next={{ pagination.page.next }}|{{ page.outputPath }}",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        read(root, "one/index.html"),
        r#"n="a" items=["a"] pages=["a","b","c"] first=a next=b|./_site/one/index.html"#
    );
    assert_eq!(
        read(root, "one/2/index.html"),
        r#"n="c" items=["c"] pages=["a","b","c"] first=a next=|./_site/one/2/index.html"#
    );
}

/// Review Focus 2: empty data gives zero pages and no problem, as in 11ty.
#[test]
fn empty_data_gives_no_pages() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/a.liquid",
        "---\ns: []\npagination:\n  data: s\n  size: 2\n---\nA",
    );
    write(
        root,
        "src/o.liquid",
        "---\ns: {}\npagination:\n  data: s\n  size: 2\n---\nO",
    );
    for size in ["0", "-1", "null"] {
        write(
            root,
            &format!("src/z{size}.liquid"),
            &format!("---\ns: [1, 2]\npagination:\n  data: s\n  size: {size}\n---\nZ"),
        );
    }
    write(root, "src/ok.liquid", "ok");
    let r = run(root);
    assert!(r.problems.is_empty(), "{:#?}", r.problems);
    assert_eq!(r.written, 1);
    assert_eq!(read(root, "ok/index.html"), "ok");
}

/// Review Focus 2: object keys paginate in JavaScript order.
#[test]
fn object_keys_paginate_in_javascript_order() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/_data/o.json",
        r#"{"zeta":"Z","2":"two","alpha":"A","1":"one"}"#,
    );
    write(
        root,
        "src/k.liquid",
        "---\npagination:\n  data: o\n  size: 4\n---\n{{ pagination.items | json }}",
    );
    write(
        root,
        "src/v.liquid",
        "---\npagination:\n  data: o\n  size: 4\n  resolve: values\n---\n{{ pagination.items | json }}",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(root, "k/index.html"), r#"["1","2","zeta","alpha"]"#);
    assert_eq!(read(root, "v/index.html"), r#"["one","two","Z","A"]"#);
}

/// Review Focus 2: a missing key stops 11ty's build ("Could not find
/// pagination data"); Brrrty refuses that page and builds the rest.
#[test]
fn a_missing_key_is_one_page_error() {
    let p = assert_refused(
        "pagination:\n  data: nope.deeper\n  size: 1\n",
        codes::PAGINATION_INVALID,
        "`nope.deeper`",
    );
    assert_eq!(p.line, Some(2));
    assert!(
        p.help.as_deref().unwrap_or("").contains("11ty stops"),
        "{p:?}"
    );
    assert_refused(
        "pagination:\n  size: 1\n",
        codes::PAGINATION_INVALID,
        "`pagination.data`",
    );
}

/// Probed: 11ty stops on a target that is not a list or an object.
#[test]
fn a_target_that_is_not_a_list_or_object_is_one_page_error() {
    for value in ["hello", "5", "null", "2020-01-01", "true"] {
        let p = assert_refused(
            &format!("s: {value}\npagination:\n  data: s\n  size: 1\n"),
            codes::PAGINATION_INVALID,
            "a list or an object",
        );
        assert!(p.message.contains("`s`"), "{}", p.message);
    }
}

/// Probed: 11ty stops on a missing `size`, and on `pagination` that is not
/// an object.
#[test]
fn misconfigured_pagination_is_one_page_error() {
    assert_refused(
        "s: [1]\npagination:\n  data: s\n",
        codes::PAGINATION_INVALID,
        "`pagination.size`",
    );
    assert_refused(
        "pagination: {}\n",
        codes::PAGINATION_INVALID,
        "`pagination.size`",
    );
    for v in ["", " null", " false", " hello", " [1]", " 0"] {
        assert_refused(
            &format!("pagination:{v}\n"),
            codes::PAGINATION_INVALID,
            "`pagination`",
        );
    }
}

/// Sizes 11ty reads through lodash's `toInteger`, which Brrrty doesn't
/// copy, are refused.
#[test]
fn a_size_that_is_not_a_number_is_refused() {
    for v in ["\"2\"", "true", "[1]"] {
        assert_refused(
            &format!("s: [1, 2]\npagination:\n  data: s\n  size: {v}\n"),
            codes::PAGINATION_UNSUPPORTED,
            "`pagination.size`",
        );
    }
    assert_refused(
        "s: [1, 2]\npagination:\n  data: [s]\n  size: 1\n",
        codes::PAGINATION_UNSUPPORTED,
        "`pagination.data`",
    );
}

/// A page paginating JavaScript data Brrrty can't run is refused, naming
/// the file, like any other page that reads it.
#[test]
fn pagination_over_javascript_data_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/_data/site.js",
        "module.exports = { list: [1, 2] };\n",
    );
    write(
        root,
        "src/p.liquid",
        "---\npagination:\n  data: site.list\n  size: 1\n---\nP",
    );
    write(root, "src/ok.liquid", "ok");
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::DATA_JS_UNSUPPORTED, "{p:?}");
    assert_eq!(p.path.as_deref(), Some(Path::new("src/p.liquid")));
    assert!(p.message.contains("src/_data/site.js"), "{}", p.message);
    assert_eq!(read(root, "ok/index.html"), "ok");
}

/// Probed: 11ty stops when generated pages share an output. Brrrty reports
/// the collision and builds the other pages.
#[test]
fn generated_pages_with_one_permalink_collide() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/p.liquid",
        "---\ns: [1, 2, 3]\npagination:\n  data: s\n  size: 1\npermalink: /same/\n---\nX",
    );
    write(
        root,
        "src/q.liquid",
        "---\ns: [1, 2]\npagination:\n  data: s\n  size: 1\npermalink: \"/q/{% if pagination.pageNumber == 1 %}0{% else %}{{ pagination.pageNumber }}{% endif %}/\"\n---\nQ{{ pagination.pageNumber }}",
    );
    write(root, "src/ok.liquid", "ok");
    let r = run(root);
    let errs = errors(&r);
    assert_eq!(errs.len(), 2, "{:#?}", r.problems);
    let same = errs
        .iter()
        .find(|p| p.message.contains("same/index.html"))
        .unwrap();
    assert_eq!(same.code, codes::OUTPUT_COLLISION);
    assert!(
        same.message.starts_with("3 pages write to same/index.html"),
        "{}",
        same.message
    );
    assert!(
        same.message.contains("src/p.liquid (pagination page 2)"),
        "{}",
        same.message
    );
    let q = errs
        .iter()
        .find(|p| p.message.contains("q/0/index.html"))
        .unwrap();
    assert_eq!(q.code, codes::OUTPUT_COLLISION);
    assert!(!exists(root, "same/index.html"));
    assert!(!exists(root, "q/0/index.html"));
    assert_eq!(read(root, "ok/index.html"), "ok");
}

/// Probed: `permalink: {}` with pagination stops 11ty (`templateRender has
/// not yet initialized`).
#[test]
fn an_empty_permalink_map_with_pagination_is_refused() {
    assert_refused(
        "s: [1]\npagination:\n  data: s\n  size: 1\npermalink: {}\n",
        codes::PERMALINK_INVALID,
        "`permalink: {}`",
    );
}

/// An error in one generated page's permalink refuses the template: every
/// page's `pagination.hrefs` lists it.
#[test]
fn a_permalink_error_on_one_page_refuses_the_template() {
    let p = assert_refused(
        "s: [a.html, \"\"]\npagination:\n  data: s\n  size: 1\n  alias: x\npermalink: \"{{ x }}\"\n",
        codes::PERMALINK_INVALID,
        "empty string",
    );
    assert!(p.message.contains("pagination page 1"), "{}", p.message);
}

/// Probed: `before` from front matter is a string, not a function, and
/// 11ty ignores it (it stays on the object). `after` is not an option.
#[test]
fn before_and_after_from_front_matter_are_plain_data() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/p.liquid",
        "---\ns: [x, y]\npagination:\n  data: s\n  size: 1\n  before: nope\n  after: nope\n---\n{{ pagination.items | json }}{{ pagination.before }}{{ pagination.after }}",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(root, "p/1/index.html"), r#"["y"]nopenope"#);
}

/// Review Focus 4: a size-1 pagination over 5,000 items builds every page,
/// in time linear in the pages.
#[test]
fn five_thousand_pages() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    let items: Vec<String> = (0..5000).map(|i| format!("\"item-{i}\"")).collect();
    write(
        root,
        "src/_data/items.json",
        &format!("[{}]", items.join(",")),
    );
    write(root, "src/_includes/l.liquid", "<main>{{ content }}</main>");
    write(
        root,
        "src/i.liquid",
        "---\nlayout: l.liquid\npagination:\n  data: items\n  size: 1\n  alias: item\npermalink: \"/i/{{ item }}/\"\n---\n{{ item }} {{ pagination.pageNumber }}/{{ pagination.hrefs.size }} {{ pagination.href.next }}",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(r.written, 5000);
    assert_eq!(
        read(root, "i/item-0/index.html"),
        "<main>item-0 0/5000 /i/item-1/</main>"
    );
    assert_eq!(
        read(root, "i/item-4999/index.html"),
        "<main>item-4999 4999/5000 </main>"
    );
}
