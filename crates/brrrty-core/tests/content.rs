//! `templateContent` and `content` of collection items, through `build()`.
//!
//! 11ty 3.1.8 renders every page's content (without its layout) in the
//! order of its dependency graph, one page at a time. A page that reads the
//! content of a page not rendered yet is tried again once, after all the
//! others; if the content is still missing, 11ty stops the whole build with
//! "contains a circular reference (using collections) to its own
//! templateContent". Layouts render after every page's content is known.
//!
//! The fixture `tests/fixtures/content` pins the cases 11ty builds. These
//! tests pin what Brrrty does where 11ty stops: the page is refused with a
//! clear problem, and the rest of the site builds. Every case was probed
//! with real 11ty.

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
            write_to: None,
            run_mode: Default::default(),
            root: root.to_path_buf(),
            input: PathBuf::from("src"),
            output: PathBuf::from("_site"),
            passthrough: Vec::new(),
            strict: false,
            config: Default::default(),
            js: Default::default(),
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

fn error_for<'a>(r: &'a BuildReport, path: &str) -> &'a Problem {
    errors(r)
        .into_iter()
        .find(|p| p.path.as_deref() == Some(Path::new(path)))
        .unwrap_or_else(|| panic!("no error for {path}: {:#?}", r.problems))
}

fn site(files: &[(&str, &str)]) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        write(t.path(), &format!("src/{rel}"), body);
    }
    t
}

/// The simple case: an item's content, as the fixture shows it too.
#[test]
fn item_content_is_the_page_without_its_layout() {
    let t = site(&[
        ("_includes/l.liquid", "<main>{{ content }}</main>"),
        (
            "a.md",
            "---\ntags: t\nlayout: l.liquid\ntitle: A\n---\n# {{ title }}\n",
        ),
        (
            "list.liquid",
            "{% for p in collections.t %}{{ p.templateContent }}|{{ p.content }}{% endfor %}",
        ),
    ]);
    let r = run(t.path());
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        read(t.path(), "list/index.html"),
        "<h1>A</h1>\n|<h1>A</h1>\n"
    );
    assert_eq!(read(t.path(), "a/index.html"), "<main><h1>A</h1>\n</main>");
}

/// Probed: 11ty stops ("./all.liquid contains a circular reference (using
/// collections) to its own templateContent"). Brrrty refuses the page.
#[test]
fn a_page_reading_its_own_content_is_a_cycle() {
    let t = site(&[
        (
            "all.liquid",
            "{% for p in collections.all %}[{{ p.url }}={{ p.templateContent | size }}]{% endfor %}",
        ),
        ("x.liquid", "x"),
    ]);
    let r = run(t.path());
    assert_eq!(errors(&r).len(), 1, "{:#?}", r.problems);
    let p = error_for(&r, "src/all.liquid");
    assert_eq!(p.code, codes::CONTENT_CYCLE);
    assert_eq!(
        p.message,
        "this page reads its own `templateContent` through a collection"
    );
    assert_eq!(
        p.help.as_deref(),
        Some("Skip the page itself when you loop, for example with `{% if p.url != page.url %}`.")
    );
    assert!(!exists(t.path(), "all/index.html"));
    assert_eq!(read(t.path(), "x/index.html"), "x");
}

/// `content` is the same as `templateContent`, so reading it is the same
/// cycle.
#[test]
fn reading_its_own_content_alias_is_a_cycle() {
    let t = site(&[(
        "all.liquid",
        "{% for p in collections.all %}{{ p.content }}{% endfor %}",
    )]);
    let r = run(t.path());
    assert_eq!(error_for(&r, "src/all.liquid").code, codes::CONTENT_CYCLE);
}

/// Probed: two pages that read each other's content stop 11ty. Both are
/// refused, and each problem names the circle the same way.
#[test]
fn two_pages_reading_each_other_is_a_cycle() {
    let body = "{% for p in collections.t %}{% if p.url != page.url %}({{ p.templateContent }}){% endif %}{% endfor %}";
    let t = site(&[
        ("a.liquid", &format!("---\ntags: t\n---\nA{body}")),
        ("b.liquid", &format!("---\ntags: t\n---\nB{body}")),
        ("c.liquid", "c"),
    ]);
    let r = run(t.path());
    assert_eq!(errors(&r).len(), 2, "{:#?}", r.problems);
    let a = error_for(&r, "src/a.liquid");
    assert_eq!(a.code, codes::CONTENT_CYCLE);
    assert_eq!(
        a.message,
        "`templateContent` reads go in a circle: src/a.liquid -> src/b.liquid -> src/a.liquid"
    );
    let b = error_for(&r, "src/b.liquid");
    assert_eq!(
        b.message,
        "`templateContent` reads go in a circle: src/a.liquid -> src/b.liquid -> src/a.liquid"
    );
    assert!(!exists(t.path(), "a/index.html"));
    assert!(!exists(t.path(), "b/index.html"));
    assert_eq!(read(t.path(), "c/index.html"), "c");
}

/// A page that reads a page caught in a circle can't be rendered either.
#[test]
fn reading_a_page_in_a_cycle_is_refused() {
    let body = "{% for p in collections.t %}{% if p.url != page.url %}({{ p.templateContent }}){% endif %}{% endfor %}";
    let t = site(&[
        ("a.liquid", &format!("---\ntags: t\n---\nA{body}")),
        ("b.liquid", &format!("---\ntags: t\n---\nB{body}")),
        (
            "z.liquid",
            "{% for p in collections.t %}{{ p.templateContent }}{% endfor %}",
        ),
    ]);
    let r = run(t.path());
    let z = error_for(&r, "src/z.liquid");
    assert_eq!(z.code, codes::CONTENT_UNAVAILABLE);
    assert_eq!(
        z.message,
        "this page reads `templateContent` of src/a.liquid, which could not be rendered"
    );
    assert!(!exists(t.path(), "z/index.html"));
}

/// Probed: a reads b, b reads c, in that file order. 11ty renders c in the
/// first pass, and a and b in the second, in order: when a runs, b is not
/// rendered yet, and 11ty stops ("./a.liquid contains a circular
/// reference"). There is no real circle, so Brrrty refuses only a, and
/// builds b and c.
#[test]
fn a_chain_longer_than_11tys_second_pass_is_refused() {
    let t = site(&[
        (
            "a.liquid",
            "---\ntags: ta\n---\nA({% for p in collections.tb %}{{ p.templateContent }}{% endfor %})",
        ),
        (
            "b.liquid",
            "---\ntags: tb\n---\nB({% for p in collections.tc %}{{ p.templateContent }}{% endfor %})",
        ),
        ("c.liquid", "---\ntags: tc\n---\nC"),
    ]);
    let r = run(t.path());
    assert_eq!(errors(&r).len(), 1, "{:#?}", r.problems);
    let a = error_for(&r, "src/a.liquid");
    assert_eq!(a.code, codes::CONTENT_UNAVAILABLE);
    assert_eq!(
        a.message,
        "this page reads `templateContent` of src/b.liquid before 11ty has rendered it"
    );
    assert!(
        a.help
            .as_deref()
            .unwrap()
            .contains("11ty stops the whole build"),
        "{:?}",
        a.help
    );
    assert!(!exists(t.path(), "a/index.html"));
    assert_eq!(read(t.path(), "b/index.html"), "B(C)");
    assert_eq!(read(t.path(), "c/index.html"), "C");
}

/// Probed: page 0 of a paginated template that reads page 1 stops 11ty
/// (page 1 is never rendered before page 0's second try). Page 1 builds.
#[test]
fn an_earlier_page_reading_a_later_page_of_its_template_is_refused() {
    let t = site(&[
        ("_data/letters.json", "[\"x\", \"y\"]"),
        (
            "gen.liquid",
            "---\npagination:\n  data: letters\n  size: 1\n  addAllPagesToCollections: true\ntags: g\npermalink: \"gen/{{ pagination.pageNumber }}/\"\n---\nG{{ pagination.items[0] }}{% if pagination.pageNumber == 0 %}{% for p in collections.g %}{% if p.pageNumber == 1 %}({{ p.templateContent }}){% endif %}{% endfor %}{% endif %}",
        ),
    ]);
    let r = run(t.path());
    assert_eq!(errors(&r).len(), 1, "{:#?}", r.problems);
    let p = error_for(&r, "src/gen.liquid");
    assert_eq!(p.code, codes::CONTENT_UNAVAILABLE);
    assert_eq!(
        p.message,
        "this page reads `templateContent` of src/gen.liquid (pagination page 1) before 11ty has rendered it (pagination page 0)"
    );
    assert!(!exists(t.path(), "gen/0/index.html"));
    assert_eq!(read(t.path(), "gen/1/index.html"), "Gy");
}

/// Probed: an item whose content fails to render stops 11ty. Brrrty reports
/// that page's own problem, and refuses the page that reads it.
#[test]
fn reading_a_page_that_fails_is_refused() {
    let t = site(&[
        ("bad.liquid", "---\ntags: t\n---\n{{ 'x' | nosuchfilter }}"),
        (
            "reader.liquid",
            "{% for p in collections.t %}{{ p.templateContent }}{% endfor %}",
        ),
    ]);
    let r = run(t.path());
    assert_eq!(
        error_for(&r, "src/bad.liquid").code,
        codes::LIQUID_UNKNOWN_FILTER
    );
    let p = error_for(&r, "src/reader.liquid");
    assert_eq!(p.code, codes::CONTENT_UNAVAILABLE);
    assert_eq!(
        p.message,
        "this page reads `templateContent` of src/bad.liquid, which could not be rendered"
    );
    assert!(!exists(t.path(), "reader/index.html"));
}

/// A page whose content Brrrty can't render (here it reads JavaScript
/// data) is never handed to a reader as empty content.
#[test]
fn reading_a_page_brrrty_refuses_is_refused() {
    let t = site(&[
        ("_data/site.js", "module.exports = { name: 'x' };"),
        ("a.liquid", "---\ntags: t\n---\n{{ site.name }}"),
        (
            "reader.liquid",
            "R{% for p in collections.t %}{{ p.templateContent }}{% endfor %}",
        ),
    ]);
    let r = run(t.path());
    assert_eq!(
        error_for(&r, "src/a.liquid").code,
        codes::DATA_JS_UNSUPPORTED
    );
    assert_eq!(
        error_for(&r, "src/reader.liquid").code,
        codes::CONTENT_UNAVAILABLE
    );
    assert!(!exists(t.path(), "reader/index.html"));
}

/// Probed: a `permalink` map without `build` makes a page 11ty doesn't
/// render. Its `url` reads `false`; reading its content stops 11ty ("Tried
/// to use templateContent on unrendered template").
#[test]
fn reading_a_page_11ty_does_not_render_is_refused() {
    let t = site(&[
        ("u.liquid", "---\ntags: t\npermalink:\n  other: /x/\n---\nU"),
        (
            "urls.liquid",
            "{% for p in collections.t %}[{{ p.url }}]{% endfor %}",
        ),
        (
            "reader.liquid",
            "{% for p in collections.t %}{{ p.templateContent }}{% endfor %}",
        ),
    ]);
    let r = run(t.path());
    assert_eq!(errors(&r).len(), 1, "{:#?}", r.problems);
    let p = error_for(&r, "src/reader.liquid");
    assert_eq!(p.code, codes::CONTENT_UNAVAILABLE);
    assert_eq!(
        p.message,
        "this page reads `templateContent` of src/u.liquid, which 11ty doesn't render: its `permalink` map has no `build` key"
    );
    assert_eq!(read(t.path(), "urls/index.html"), "[false]");
}

/// Probed: 11ty places pages before it renders any content, so a permalink
/// or computed value that reads an item's content stops it ("Tried to use
/// templateContent too early").
#[test]
fn a_permalink_or_computed_value_reading_item_content_is_refused() {
    for front in [
        "permalink: \"c/{{ pagination.items[0].templateContent | slugify }}/\"",
        "eleventyComputed:\n  t: \"{{ pagination.items[0].templateContent }}\"",
    ] {
        let t = site(&[
            ("a.liquid", "---\ntags: t\n---\nHello"),
            (
                "cons.liquid",
                &format!(
                    "---\npagination:\n  data: collections.t\n  size: 1\n{front}\n---\n[{{{{ t }}}}]"
                ),
            ),
        ]);
        let r = run(t.path());
        let p = error_for(&r, "src/cons.liquid");
        assert_eq!(p.code, codes::CONTENT_UNAVAILABLE, "{front}");
        assert!(
            p.message.contains("before 11ty renders any content"),
            "{front}: {}",
            p.message
        );
        assert_eq!(read(t.path(), "a/index.html"), "Hello");
    }
}

/// Probed: a layout renders after every page's content exists, so a page's
/// layout can read the page's own item.
#[test]
fn a_layout_can_read_its_own_pages_content() {
    let t = site(&[
        (
            "_includes/side.liquid",
            "{{ content }}|{% for p in collections.t %}[{{ p.content }}]{% endfor %}",
        ),
        ("a.md", "---\ntags: t\nlayout: side.liquid\n---\nA"),
    ]);
    let r = run(t.path());
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(t.path(), "a/index.html"), "<p>A</p>\n|[<p>A</p>\n]");
}

/// A layout that reads the content of a page Brrrty refused is refused too.
#[test]
fn a_layout_reading_a_refused_page_is_refused() {
    let t = site(&[
        (
            "_includes/side.liquid",
            "{{ content }}|{% for p in collections.t %}[{{ p.content }}]{% endfor %}",
        ),
        ("bad.liquid", "---\ntags: t\n---\n{{ 'x' | nosuchfilter }}"),
        ("a.md", "---\nlayout: side.liquid\n---\nA"),
    ]);
    let r = run(t.path());
    let p = error_for(&r, "src/a.md");
    assert_eq!(p.code, codes::CONTENT_UNAVAILABLE);
    assert!(!exists(t.path(), "a/index.html"));
}

/// A page with no output (`permalink: false`) still renders its content for
/// readers. One that fails is reported only when a page reads it, as
/// before: nothing else depends on it.
#[test]
fn pages_without_output_render_for_readers_only() {
    let t = site(&[
        (
            "hidden.liquid",
            "---\ntags: h\npermalink: false\n---\nH{{ 1 | plus: 1 }}",
        ),
        (
            "broken.liquid",
            "---\npermalink: false\n---\n{{ 'x' | nope }}",
        ),
        (
            "reader.liquid",
            "{% for p in collections.h %}{{ p.templateContent }}{% endfor %}",
        ),
    ]);
    let r = run(t.path());
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(t.path(), "reader/index.html"), "H2");
}

/// Many readers of the same items share one rendering of each, and a long
/// chain of pages that each read the one before resolves in one pass of
/// 11ty's order. Release builds run 2,000 posts and 200 readers.
#[test]
fn content_is_rendered_once_and_shared() {
    let (posts, readers) = if cfg!(debug_assertions) {
        (300, 30)
    } else {
        (2_000, 200)
    };
    let t = tempfile::tempdir().unwrap();
    for i in 0..posts {
        let prev = if i == 0 {
            String::new()
        } else {
            format!(
                "{{% for p in collections.post %}}{{% if p.fileSlug == \"p{:05}\" %}}{{{{ p.templateContent | size }}}}{{% endif %}}{{% endfor %}}",
                i - 1
            )
        };
        write(
            t.path(),
            &format!("src/posts/p{i:05}.md"),
            &format!("---\ntags: post\ndate: 2020-01-01\n---\nPost {i} {prev}\n"),
        );
    }
    for i in 0..readers {
        write(
            t.path(),
            &format!("src/r/r{i}.liquid"),
            "---\neleventyExcludeFromCollections: true\n---\n{% for p in collections.post %}{{ p.templateContent | size }},{% endfor %}",
        );
    }
    let start = std::time::Instant::now();
    let r = run(t.path());
    let elapsed = start.elapsed();
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        read(t.path(), "posts/p00001/index.html"),
        "<p>Post 1 14</p>\n"
    );
    let first = read(t.path(), "r/r0/index.html");
    assert!(first.starts_with("14,17,17,"), "{first}");
    if !cfg!(debug_assertions) {
        assert!(elapsed.as_secs_f64() < 5.0, "{elapsed:?}");
    }
}

/// A circle of many pages: each page names it the same way, without the
/// pagination page twice, and a long circle is shortened.
#[test]
fn a_long_circle_is_named_once_and_shortened() {
    let t = site(&[
        ("_data/n.json", "[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]"),
        (
            "ring.liquid",
            "---\npagination:\n  data: n\n  size: 1\n  addAllPagesToCollections: true\ntags: r\npermalink: \"r/{{ pagination.pageNumber }}/\"\n---\n{% assign next = pagination.pageNumber | plus: 1 | modulo: 10 %}{% for p in collections.r %}{% if p.pageNumber == next %}{{ p.templateContent }}{% endif %}{% endfor %}",
        ),
    ]);
    let r = run(t.path());
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert_eq!(errs[0].code, codes::CONTENT_CYCLE);
    assert_eq!(
        errs[0].message,
        "`templateContent` reads go in a circle: src/ring.liquid (pagination page 0) -> src/ring.liquid (pagination page 1) -> src/ring.liquid (pagination page 2) -> src/ring.liquid (pagination page 3) -> src/ring.liquid (pagination page 4) -> ... 5 more -> src/ring.liquid (pagination page 0)"
    );
    assert!(!exists(t.path(), "r/0/index.html"));
}

/// Probed: 11ty loads `.liquid` templates before `.html` before `.md`, so
/// here the reader (`.liquid`) comes first and the page it reads (`.html`)
/// waits for a `.md` page: 11ty stops on the reader ("./chain/c_r.liquid
/// contains a circular reference"). In path order the chain would build;
/// the fixture `tests/fixtures/order` has the same chain the other way
/// round, which 11ty builds.
#[test]
fn the_order_11ty_renders_in_groups_by_extension() {
    let t = site(&[
        (
            "chain/c_r.liquid",
            "---\ntags: cr\n---\nR{% for p in collections.cp %}({{ p.templateContent }}){% endfor %}",
        ),
        (
            "chain/b_p.html",
            "---\ntags: cp\n---\nP{% for p in collections.cq %}({{ p.templateContent }}){% endfor %}",
        ),
        ("chain/a_q.md", "---\ntags: cq\n---\nQ"),
    ]);
    let r = run(t.path());
    assert_eq!(errors(&r).len(), 1, "{:#?}", r.problems);
    let p = error_for(&r, "src/chain/c_r.liquid");
    assert_eq!(p.code, codes::CONTENT_UNAVAILABLE);
    assert_eq!(read(t.path(), "chain/b_p/index.html"), "P(<p>Q</p>\n)");
    assert_eq!(read(t.path(), "chain/a_q/index.html"), "<p>Q</p>\n");
}
