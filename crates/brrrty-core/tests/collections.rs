//! Collections through `build()`. Every expectation was probed with real
//! 11ty 3.1.8; the fixture `tests/fixtures/collections` pins the output 11ty
//! writes, and these tests pin what Brrrty refuses, and the cases that need
//! a site with an 11ty config file or a template that fails.

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

/// A site with two dated pages and `src/report.liquid` set to `body`.
fn site(body: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "src/a.md",
        "---\ndate: 2020-01-01\ntags: x\ntitle: A\n---\na\n",
    );
    write(
        t.path(),
        "src/b.md",
        "---\ndate: 2020-01-02\ntitle: B\n---\nb\n",
    );
    write(
        t.path(),
        "src/report.liquid",
        &format!("---\npermalink: report.txt\neleventyExcludeFromCollections: true\n---\n{body}"),
    );
    t
}

/// Probed: `collections.all` is every page by date, then input path; a tag
/// collection is the pages with the tag.
#[test]
fn collections_hold_the_pages() {
    let t = site(
        "{% for p in collections.all %}{{ p.url }} {% endfor %}|{% for p in collections.x %}{{ p.data.title }} {% endfor %}",
    );
    let r = run(t.path());
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(t.path(), "report.txt"), "/a/ /b/ |A ");
}

/// Probed: a collection that nothing publishes to is empty, and `collections`
/// has no key for it, in a site with no 11ty config file. Tags that are
/// empty strings make no collection.
#[test]
fn an_unknown_collection_is_empty_without_a_config_file() {
    let t = site(
        "{{ collections.nope | size }}|{{ collections.nope | json }}|{{ collections[\"\"] | json }}|{% for c in collections %}{{ c[0] }} {% endfor %}",
    );
    write(
        t.path(),
        "src/e.md",
        "---\ndate: 2020-01-03\ntags: [\"\"]\n---\ne\n",
    );
    let r = run(t.path());
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(t.path(), "report.txt"), "0|||x all ");
}

/// With an 11ty config file, a name that is not a tag may be a collection
/// the config adds, and Brrrty doesn't read the config: the page is refused
/// instead of printing an empty collection. Tags still work.
#[test]
fn an_unknown_collection_is_refused_with_a_config_file() {
    let t = site("{% for p in collections.posts %}{{ p.url }}{% endfor %}");
    write(
        t.path(),
        "src/ok.liquid",
        "{% for p in collections.x %}{{ p.url }}{% endfor %}",
    );
    write(
        t.path(),
        "eleventy.config.js",
        "module.exports = () => ({});",
    );
    let r = run(t.path());
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert_eq!(errs[0].code, codes::DATA_UNSUPPORTED);
    assert_eq!(
        errs[0].path.as_deref(),
        Some(Path::new("src/report.liquid"))
    );
    assert!(
        errs[0].message.starts_with(
            "this page reads `collections.posts`, which is not a tag collection, and this site has an 11ty config file"
        ),
        "{}",
        errs[0].message
    );
    assert_eq!(read(t.path(), "ok/index.html"), "/a/");
}

/// Listing every collection can't be right when a config may add more.
#[test]
fn listing_collections_is_refused_with_a_config_file() {
    let t = site("{% for c in collections %}{{ c[0] }}{% endfor %}");
    write(t.path(), ".eleventy.js", "module.exports = () => ({});");
    let r = run(t.path());
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert!(
        errs[0]
            .message
            .starts_with("this page lists every collection"),
        "{}",
        errs[0].message
    );
}

/// Probed: an item's `templateContent` and `content` are the page's
/// content without its layout, read directly or through a filter. Listing
/// an item's keys also lists `template`, which Brrrty has no value for, so
/// that is still refused.
#[test]
fn item_content_reads_like_11ty() {
    let t = site(
        "{% for p in collections.all %}{{ p.templateContent }}{% endfor %}|{% for p in collections.all %}{{ p.content }}{% endfor %}|{{ collections.all | map: \"templateContent\" | join: \",\" }}",
    );
    let r = run(t.path());
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        read(t.path(), "report.txt"),
        "<p>a</p>\n<p>b</p>\n|<p>a</p>\n<p>b</p>\n|<p>a</p>\n,<p>b</p>\n"
    );
    for body in [
        "{% for p in collections.all %}{{ p | json }}{% endfor %}",
        "{% for p in collections.all %}{% for k in p %}{{ k[0] }}{% endfor %}{% endfor %}",
    ] {
        let t = site(body);
        let r = run(t.path());
        let errs = errors(&r);
        assert_eq!(errs.len(), 1, "{body}: {:#?}", r.problems);
        assert_eq!(errs[0].code, codes::DATA_UNSUPPORTED, "{body}");
        assert!(
            errs[0].message.contains("`template` of a collection item"),
            "{body}: {}",
            errs[0].message
        );
        assert!(!t.path().join("_site/report.txt").exists(), "{body}");
        assert_eq!(read(t.path(), "a/index.html"), "<p>a</p>\n");
    }
}

/// Items hold more than the fields Brrrty can fill: the template object and
/// the item's own `data.collections` are 11ty's, and are refused too.
#[test]
fn reading_the_template_object_or_item_collections_is_refused() {
    for (body, needle) in [
        (
            "{% for p in collections.all %}{{ p.template }}{% endfor %}",
            "`template` of a collection item",
        ),
        (
            "{% for p in collections.all %}{{ p.data.collections }}{% endfor %}",
            "`data.collections` of a collection item",
        ),
    ] {
        let t = site(body);
        let r = run(t.path());
        let errs = errors(&r);
        assert_eq!(errs.len(), 1, "{body}: {:#?}", r.problems);
        assert!(errs[0].message.contains(needle), "{}", errs[0].message);
    }
}

/// Every other field of an item reads fine, alongside the refused ones that
/// are not read.
#[test]
fn the_other_item_fields_are_exact() {
    let t = site(
        "{% for p in collections.all %}{{ p.inputPath }} {{ p.fileSlug }} {{ p.filePathStem }} {{ p.date | json }} {{ p.url }} {{ p.groupNumber }} {{ p.page.url }} {{ p.data.title }} {{ p.rawInput | json }}\n{% endfor %}",
    );
    let r = run(t.path());
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        read(t.path(), "report.txt"),
        "./src/a.md a /a \"2020-01-01T00:00:00.000Z\" /a/ 0 /a/ A \"a\\n\"\n./src/b.md b /b \"2020-01-02T00:00:00.000Z\" /b/ 0 /b/ B \"b\\n\"\n"
    );
}

/// 11ty puts every page in the collections, so a collection made while a
/// template can't be built would be wrong: pages that read `collections`
/// are refused and say why. Pages that don't read them still build.
#[test]
fn collections_are_refused_when_a_template_failed() {
    let t = site("{{ collections.all | size }}");
    write(t.path(), "src/bad.md", "---\nlayout: missing\n---\nbad\n");
    let r = run(t.path());
    let report = r
        .problems
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/report.liquid")))
        .unwrap_or_else(|| panic!("{:#?}", r.problems));
    assert_eq!(
        report.message,
        "this page uses `collections`, but 1 template could not be built, so the collections would be missing its page"
    );
    assert_eq!(read(t.path(), "a/index.html"), "<p>a</p>\n");
}

/// A template language Brrrty doesn't render is a template 11ty puts in the
/// collections.
#[test]
fn collections_are_refused_when_a_template_language_is_unsupported() {
    let t = site("{{ collections.all | size }}");
    write(t.path(), "src/n.njk", "x");
    let r = run(t.path());
    let report = r
        .problems
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/report.liquid")))
        .unwrap_or_else(|| panic!("{:#?}", r.problems));
    assert!(
        report
            .message
            .starts_with("this page uses `collections`, but 1 template uses a template language"),
        "{}",
        report.message
    );
}

/// Pagination over a collection that nothing publishes to makes no pages, as
/// 11ty does (it makes the collection empty first), unless the site has a
/// config that may define it.
#[test]
fn pagination_over_an_unknown_collection_makes_no_pages() {
    let t = site("x");
    write(
        t.path(),
        "src/none.liquid",
        "---\npagination:\n  data: collections.nothing\n  size: 1\n---\nx",
    );
    let r = run(t.path());
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert!(!t.path().join("_site/none").exists());
}

/// 11ty stops on a template that paginates a collection it also publishes
/// to (dotted spelling, the only one it checks).
#[test]
fn a_circular_pagination_reference_is_an_error() {
    let t = site("x");
    write(
        t.path(),
        "src/loop.liquid",
        "---\ntags: x\npagination:\n  data: collections.x\n  size: 1\n---\nx",
    );
    let r = run(t.path());
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert_eq!(errs[0].code, codes::PAGINATION_INVALID);
    assert!(
        errs[0].message.contains("pagination circular reference"),
        "{}",
        errs[0].message
    );
    assert_eq!(read(t.path(), "a/index.html"), "<p>a</p>\n");
}

/// Paginating the whole `collections` object isn't supported yet.
#[test]
fn pagination_over_the_collections_object_is_refused() {
    let t = site("x");
    write(
        t.path(),
        "src/all.liquid",
        "---\npagination:\n  data: collections\n  size: 1\n---\nx",
    );
    let r = run(t.path());
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert_eq!(errs[0].code, codes::PAGINATION_UNSUPPORTED);
    assert!(errs[0].message.contains("whole `collections` object"));
}

/// A page whose pagination over a collection fails is reported, and the
/// collections are then refused rather than built without its pages.
#[test]
fn a_failed_collection_pagination_is_reported_and_blocks_collections() {
    let t = site("{{ collections.all | size }}");
    write(
        t.path(),
        "src/bad.liquid",
        "---\npagination:\n  data: collections.x\n  size: 1\npermalink: \"{{ collections.all | size }}/\"\n---\nx",
    );
    let r = run(t.path());
    let errs = errors(&r);
    assert!(
        errs.iter()
            .any(|p| p.path.as_deref() == Some(Path::new("src/bad.liquid"))),
        "{:#?}",
        r.problems
    );
    assert!(
        errs.iter().any(
            |p| p.path.as_deref() == Some(Path::new("src/report.liquid"))
                && p.message.contains("this page uses `collections`")
        ),
        "{:#?}",
        r.problems
    );
}

/// A permalink or computed value that reads `collections` can't be placed
/// before the collections exist.
#[test]
fn a_permalink_reading_collections_is_refused() {
    let t = site("x");
    write(
        t.path(),
        "src/perm.liquid",
        "---\npermalink: \"n{{ collections.all | size }}/\"\n---\nx",
    );
    let r = run(t.path());
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert_eq!(errs[0].path.as_deref(), Some(Path::new("src/perm.liquid")));
    assert!(
        errs[0]
            .message
            .starts_with("a permalink, `pagination` or computed value reads `collections`"),
        "{}",
        errs[0].message
    );
}

/// Computed `tags` decide membership in a way Brrrty doesn't follow yet.
#[test]
fn computed_tags_block_collections() {
    let t = site("{{ collections.all | size }}");
    write(
        t.path(),
        "src/c.md",
        "---\ndate: 2020-01-03\neleventyComputed:\n  tags: \"{{ title }}\"\n---\nc\n",
    );
    let r = run(t.path());
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert!(
        errs[0].message.contains("computed data sets `tags`"),
        "{}",
        errs[0].message
    );
    assert_eq!(read(t.path(), "c/index.html"), "<p>c</p>\n");
}

/// Layouts and includes see `collections` like the page does.
#[test]
fn layouts_read_collections() {
    let t = site("x");
    write(
        t.path(),
        "src/_includes/l.liquid",
        "{{ collections.all | size }}:{{ content }}",
    );
    write(
        t.path(),
        "src/withl.md",
        "---\nlayout: l\ndate: 2020-01-04\n---\nw\n",
    );
    let r = run(t.path());
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(t.path(), "withl/index.html"), "3:<p>w</p>\n");
}

/// Every filter that reads a property of the items sees the item's content
/// (probed: where, reject, find, find_index, has, group_by, map, sort, sum
/// and their `_exp` forms, each printed with `size`), and the fields Brrrty
/// can't fill stay refused, with or without a config file.
#[test]
fn filters_reading_item_fields() {
    let t = "templateContent";
    let filters = [
        (format!("where: \"{t}\""), "2"),
        (format!("where: \"{t}\", \"x\""), "0"),
        (format!("reject: \"{t}\""), "0"),
        (format!("find: \"{t}\", \"x\""), "0"),
        (format!("find_index: \"{t}\", \"x\""), "0"),
        (format!("has: \"{t}\", \"x\""), "0"),
        (format!("group_by: \"{t}\""), "2"),
        (format!("map: \"{t}\""), "2"),
        (format!("sort: \"{t}\""), "2"),
        (format!("sort_natural: \"{t}\""), "2"),
        (format!("sum: \"{t}\""), "0"),
        (format!("where_exp: \"i\", \"i.{t} == 1\""), "0"),
        (format!("reject_exp: \"i\", \"i.{t} == 1\""), "2"),
        (format!("find_exp: \"i\", \"i.{t} == 1\""), "0"),
        (format!("find_index_exp: \"i\", \"i.{t} == 1\""), "0"),
        (format!("has_exp: \"i\", \"i.{t} == 1\""), "0"),
        (format!("group_by_exp: \"i\", \"i.{t}\""), "2"),
        ("where: \"content\"".to_string(), "2"),
    ];
    let refused = ["where: \"data.collections\"", "map: \"template\""];
    for config in [false, true] {
        let with_config = |t: &tempfile::TempDir| {
            if config {
                write(
                    t.path(),
                    "eleventy.config.js",
                    "module.exports = () => ({});",
                );
            }
        };
        for (f, size) in &filters {
            let t = site(&format!("{{{{ collections.all | {f} | size }}}}"));
            with_config(&t);
            let r = run(t.path());
            assert!(
                errors(&r).is_empty(),
                "{f} (config {config}): {:#?}",
                r.problems
            );
            assert_eq!(read(t.path(), "report.txt"), *size, "{f} (config {config})");
        }
        for f in refused {
            let t = site(&format!("{{{{ collections.all | {f} | size }}}}"));
            with_config(&t);
            let r = run(t.path());
            let errs = errors(&r);
            assert_eq!(errs.len(), 1, "{f} (config {config}): {:#?}", r.problems);
            assert!(
                errs[0].message.contains("collection item"),
                "{f}: {}",
                errs[0].message
            );
            assert!(!t.path().join("_site/report.txt").exists(), "{f}");
        }
    }
}

/// `first` and `last` hand back an item, which prints as JSON.
#[test]
fn printing_a_whole_item_is_refused_without_a_config_file() {
    for body in [
        "{{ collections.all | first | json }}",
        "{{ collections.all.first | json }}",
        "{{ collections.x | last | json }}",
    ] {
        let t = site(body);
        let r = run(t.path());
        assert_eq!(errors(&r).len(), 1, "{body}: {:#?}", r.problems);
    }
}

/// A template that paginates `collections.post` with
/// `addAllPagesToCollections` puts every page in the collections it reads
/// from: the pages hold each other's data, which must be shared, not copied
/// (it once took 12 GB for 2,000 posts). Release builds run 10,000 posts.
#[test]
fn add_all_pages_to_collections_stays_linear() {
    let posts = if cfg!(debug_assertions) {
        1_000
    } else {
        10_000
    };
    let t = tempfile::tempdir().unwrap();
    for i in 0..posts {
        write(
            t.path(),
            &format!("src/posts/p{i}.md"),
            &format!(
                "---\ntitle: P{i}\ndate: 2020-01-01\ntags: [post, t{}]\n---\nb\n",
                i % 50
            ),
        );
    }
    write(
        t.path(),
        "src/list.liquid",
        "---\npagination:\n  data: collections.post\n  size: 1\n  addAllPagesToCollections: true\npermalink: \"list/{{ pagination.pageNumber }}/\"\ndate: 2040-01-01\n---\n{{ pagination.items[0].url }}\n",
    );
    write(
        t.path(),
        "src/count.liquid",
        "---\ndate: 2040-01-02\neleventyExcludeFromCollections: true\n---\n{{ collections.all | size }}\n",
    );
    let start = std::time::Instant::now();
    let r = run(t.path());
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        read(t.path(), "count/index.html"),
        format!("{}\n", posts * 2)
    );
    assert_eq!(read(t.path(), "list/0/index.html"), "/posts/p0/\n");
    if !cfg!(debug_assertions) {
        assert!(start.elapsed().as_secs_f64() < 5.0, "{:?}", start.elapsed());
    }
}

/// 11ty loads a JavaScript data file with `import()`, from the disk, while
/// it decides the order of templates; Brrrty can't know when that finishes,
/// so the order of the collections is unknown and reads of them are
/// refused. Pages that don't read collections still build.
#[test]
fn a_javascript_data_file_refuses_collections() {
    let t = site("{{ collections.all | size }}");
    write(
        t.path(),
        "src/js/js.11tydata.js",
        "module.exports = { k: 1 };",
    );
    write(t.path(), "src/js/p.md", "---\ndate: 2020-01-03\n---\np\n");
    let r = run(t.path());
    let report = r
        .problems
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/report.liquid")))
        .unwrap_or_else(|| panic!("{:#?}", r.problems));
    assert!(
        report
            .message
            .contains("1 template uses a JavaScript data file"),
        "{}",
        report.message
    );
    assert_eq!(read(t.path(), "a/index.html"), "<p>a</p>\n");
    assert_eq!(read(t.path(), "js/p/index.html"), "<p>p</p>\n");
}
