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

/// The report problem of a site, which must be the only one besides the
/// unsupported templates and layouts.
fn report_problem(r: &BuildReport) -> &Problem {
    r.problems
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/report.liquid")))
        .unwrap_or_else(|| panic!("{:#?}", r.problems))
}

/// Probed: Nunjucks templates, and pages with a Nunjucks layout, are
/// collection items with their front matter, data files, layout data,
/// computed values, permalinks, pagination and dates. Brrrty places them
/// without running Nunjucks and writes the pages that read them exactly as
/// 11ty does; it only refuses to write the Nunjucks pages themselves.
#[test]
fn nunjucks_templates_are_collection_items() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    let files: &[(&str, &str)] = &[
        (
            "_includes/l.njk",
            "---\ntags: fromlayout\nwho: layout\n---\n<l>{{ content | safe }}</l>\n",
        ),
        (
            "_includes/m.liquid",
            "---\nlayout: l.njk\n---\n<m>{{ content }}</m>\n",
        ),
        (
            "a.njk",
            "---\ndate: 2020-01-03\ntags: post\ntitle: A\n---\n{% set x = 1 %}A {{ title }}\n",
        ),
        (
            "b.njk",
            "---\ndate: 2020-01-02\ntags: [post, other]\ntitle: B\nlayout: l.njk\n---\nB\n",
        ),
        (
            "c.md",
            "---\ndate: 2020-01-04\ntags: post\ntitle: C\nlayout: l.njk\n---\n# C {{ title }}\n",
        ),
        (
            "c2.md",
            "---\ndate: 2020-01-05\ntitle: C2\nlayout: m.liquid\n---\nC2\n",
        ),
        (
            "d.njk",
            "---\ndate: 2020-01-01\ntags: post\ntitle: D\npermalink: /dee/\n---\nD\n",
        ),
        (
            "e.njk",
            "---\ndate: 2020-01-06\ntags: post\ntitle: E\npermalink: false\n---\nE\n",
        ),
        (
            "f.njk",
            "---\ndate: 2020-01-07\ntags: post\ntitle: F\neleventyComputed:\n  title: Fixed\n  n: 5\n---\nF\n",
        ),
        (
            "g.njk",
            "---\ndate: 2020-01-08\ntags: pg\nitems: [p, q, r]\npagination:\n  data: items\n  size: 1\n  addAllPagesToCollections: true\n---\nG\n",
        ),
        (
            "h.njk",
            "---\ndate: 2020-01-09\ntags: post\ntitle: H\npermalink: /h.json\ndynamicPermalink: false\n---\nH\n",
        ),
        (
            "sub/i.njk",
            "---\ntags: post\ntitle: I\ndate: 2019-12-31\n---\nI\n",
        ),
        (
            "list.liquid",
            "---\ndate: 2020-02-01\neleventyExcludeFromCollections: true\n---\n{% for p in collections.post %}[{{ p.url }}|{{ p.outputPath }}|{{ p.inputPath }}|{{ p.fileSlug }}|{{ p.filePathStem }}|{{ p.date | date: '%F' }}|{{ p.data.title }}|{{ p.data.n }}|{{ p.data.who }}|{{ p.data.tags | join: ',' }}|{{ p.data.page.templateSyntax }}|{{ p.data.page.outputFileExtension }}]\n{% endfor %}{% for p in collections.pg %}({{ p.url }} {{ p.data.pagination.items | join: ',' }} {{ p.pageNumber }})\n{% endfor %}fl={% for p in collections.fromlayout %}{{ p.inputPath }},{% endfor %}\nall={% for p in collections.all %}{{ p.inputPath }},{% endfor %}\nc={% for p in collections.post %}{% if p.fileSlug == 'c' %}{{ p.templateContent }}{% endif %}{% endfor %}\n",
        ),
        (
            "keys.liquid",
            "---\npermalink: keys.txt\ndate: 2020-02-01\neleventyExcludeFromCollections: true\n---\n{% for c in collections %}{{ c[0] }}={{ c[1].size }},{% endfor %}",
        ),
        (
            "raw.liquid",
            "---\ndate: 2020-02-01\neleventyExcludeFromCollections: true\n---\n{% for p in collections.all %}{% if p.fileSlug == 'a' %}{{ p.data.page.rawInput }}|{{ p.data.page | json }}{% endif %}{% endfor %}",
        ),
    ];
    for (rel, text) in files {
        write(root, &format!("src/{rel}"), text);
    }
    let r = run(root);
    for p in errors(&r) {
        assert!(
            [codes::TEMPLATE_UNSUPPORTED, codes::LAYOUT_UNSUPPORTED].contains(&p.code),
            "{p:#?}"
        );
    }
    assert_eq!(
        read(root, "list/index.html"),
        "[/sub/i/|./_site/sub/i/index.html|./src/sub/i.njk|i|/sub/i|%F|I|||post|njk|html]
[/dee/|./_site/dee/index.html|./src/d.njk|d|/d|%F|D|||post|njk|html]
[/b/|./_site/b/index.html|./src/b.njk|b|/b|%F|B||layout|fromlayout,post,other|njk|html]
[/a/|./_site/a/index.html|./src/a.njk|a|/a|%F|A|||post|njk|html]
[/c/|./_site/c/index.html|./src/c.md|c|/c|%F|C||layout|fromlayout,post|liquid,md|html]
[false|false|./src/e.njk|e|/e|%F|E|||post|njk|html]
[/f/|./_site/f/index.html|./src/f.njk|f|/f|%F|Fixed|5||post|njk|html]
[/h.json|./_site/h.json|./src/h.njk|h|/h|%F|H|||post|njk|html]
(/g/ p 0)
(/g/1/ q 1)
(/g/2/ r 2)
fl=./src/b.njk,./src/c.md,./src/c2.md,
all=./src/sub/i.njk,./src/d.njk,./src/b.njk,./src/a.njk,./src/c.md,./src/c2.md,./src/e.njk,./src/f.njk,./src/g.njk,./src/g.njk,./src/g.njk,./src/h.njk,
c=<h1>C C</h1>

"
    );
    assert_eq!(
        read(root, "keys.txt"),
        "post=8,pg=3,fromlayout=3,other=1,all=12,"
    );
    assert_eq!(
        read(root, "raw/index.html"),
        r#"{% set x = 1 %}A {{ title }}
|{"inputPath":"./src/a.njk","fileSlug":"a","filePathStem":"/a","outputFileExtension":"html","templateSyntax":"njk","date":"2020-01-03T00:00:00.000Z","rawInput":"{% set x = 1 %}A {{ title }}\n","url":"/a/","outputPath":"./_site/a/index.html"}"#
    );
    // Only the pages Brrrty can render are written; the Nunjucks layout is
    // reported once for the two pages that use it.
    assert_eq!(r.written, 3);
    let layout = errors(&r)
        .into_iter()
        .find(|p| p.code == codes::LAYOUT_UNSUPPORTED)
        .unwrap();
    assert_eq!(
        layout.path.as_deref(),
        Some(Path::new("src/_includes/l.njk"))
    );
    assert!(
        layout
            .help
            .as_deref()
            .unwrap()
            .ends_with("2 pages use this layout and were not written."),
        "{layout:?}"
    );
    assert!(!root.join("_site/a/index.html").exists());
    assert!(!root.join("_site/c/index.html").exists());
}

/// Reading what only Nunjucks can make is refused: the content of a
/// Nunjucks page, and collections holding a Nunjucks page whose permalink
/// or computed data is a Nunjucks template.
#[test]
fn reads_that_need_nunjucks_are_refused() {
    let t = site("{% for p in collections.all %}{{ p.templateContent }}{% endfor %}");
    write(t.path(), "src/n.njk", "---\ndate: 2020-01-03\n---\nN");
    let r = run(t.path());
    let p = report_problem(&r);
    assert_eq!(p.code, codes::CONTENT_UNAVAILABLE);
    assert_eq!(
        p.message,
        "this page reads `templateContent` of src/n.njk, a Nunjucks template Brrrty can't render yet"
    );
    assert_eq!(read(t.path(), "a/index.html"), "<p>a</p>\n");
    assert_eq!(errors(&r).len(), 2, "{:#?}", r.problems);

    for (front, why) in [
        (
            "permalink: \"/{{ title }}/\"\n",
            "its `permalink` is a Nunjucks template",
        ),
        (
            "permalink: \"/n/{# note #}\"\n",
            "its `permalink` is a Nunjucks template",
        ),
        (
            "eleventyComputed:\n  t: \"{{ title }}\"\n",
            "its `eleventyComputed` values are Nunjucks templates",
        ),
        (
            "templateEngineOverride: md\n",
            "it sets `templateEngineOverride`",
        ),
    ] {
        let t = site("{{ collections.all | size }}");
        write(t.path(), "src/n.njk", &format!("---\n{front}---\nN"));
        let r = run(t.path());
        assert_eq!(
            report_problem(&r).message,
            format!(
                "this page uses `collections`, but Brrrty can't tell where src/n.njk goes in the collections: {why}"
            ),
            "{front}"
        );
        // Only the Nunjucks template itself is reported besides.
        assert_eq!(errors(&r).len(), 2, "{:#?}", r.problems);
    }
}

/// A JavaScript template's data comes from running it, so collections
/// stay refused while one is in the site.
#[test]
fn collections_are_refused_with_a_javascript_template() {
    let t = site("{{ collections.all | size }}");
    write(t.path(), "src/j.11ty.js", "module.exports = () => 'x';");
    let r = run(t.path());
    assert_eq!(
        report_problem(&r).message,
        "this page uses `collections`, but 1 JavaScript template (.11ty.js) is in the site, whose data Brrrty can't read yet, so the collections would be missing its pages"
    );
}

/// A layout whose Liquid doesn't parse refuses only the pages it wraps:
/// their data and content are known, so the collections still hold them.
#[test]
fn a_broken_layout_keeps_its_pages_in_the_collections() {
    let t = site(
        "{% for p in collections.all %}{{ p.url }}={{ p.templateContent | strip }} {% endfor %}",
    );
    write(t.path(), "src/_includes/bad.liquid", "{% if %}");
    write(
        t.path(),
        "src/c.md",
        "---\ndate: 2020-01-03\nlayout: bad\n---\nc\n",
    );
    let r = run(t.path());
    assert_eq!(
        read(t.path(), "report.txt"),
        "/a/=<p>a</p> /b/=<p>b</p> /c/=<p>c</p> "
    );
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert_eq!(
        errs[0].path.as_deref(),
        Some(Path::new("src/_includes/bad.liquid"))
    );
    assert!(!t.path().join("_site/c/index.html").exists());
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

/// A page that fails for another reason too, under a layout Brrrty can't
/// render, reports only the layout, once with its page count, as before
/// such layouts were placed.
#[test]
fn a_failing_page_under_an_unrenderable_layout_reports_the_layout() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "src/_includes/l.njk", "{{ content | safe }}");
    for name in ["a", "b"] {
        write(
            t.path(),
            &format!("src/{name}.md"),
            "---\nlayout: l.njk\npermalink: \"/{{ title | nope }}/\"\n---\nx\n",
        );
    }
    let r = run(t.path());
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert_eq!(errs[0].code, codes::LAYOUT_UNSUPPORTED);
    assert!(
        errs[0]
            .help
            .as_deref()
            .unwrap()
            .ends_with("2 pages use this layout and were not written."),
        "{:?}",
        errs[0]
    );
}

/// `eleventyImport.collections` reorders 11ty's collections graph (fixture
/// `collections-import`). An entry 11ty names in the local time zone, or one
/// that may be a collection the 11ty config adds, can't be placed, so reads
/// of `collections` are refused.
#[test]
fn imports_brrrty_cannot_place_refuse_collections() {
    let t = site("{% for c in collections %}{{ c[0] }} {% endfor %}");
    write(
        t.path(),
        "src/d.md",
        "---\neleventyImport:\n  collections: [x, 2020-01-01]\n---\nd\n",
    );
    let r = run(t.path());
    let p = report_problem(&r);
    assert_eq!(p.code, codes::DATA_UNSUPPORTED);
    assert!(
        p.message.contains(
            "src/d.md lists a date in `eleventyImport.collections`, which 11ty names in the local time zone"
        ),
        "{}",
        p.message
    );

    let t = site("{{ collections.x | size }}");
    write(
        t.path(),
        "src/d.md",
        "---\neleventyImport:\n  collections: [x, posts]\n---\nd\n",
    );
    write(
        t.path(),
        "eleventy.config.js",
        "module.exports = () => ({});",
    );
    let r = run(t.path());
    let p = report_problem(&r);
    assert!(
        p.message.contains("src/d.md imports `posts` with `eleventyImport.collections`, which is not a tag collection and may be one your 11ty config adds"),
        "{}",
        p.message
    );
    assert!(t.path().join("_site/d/index.html").is_file());
}

/// Probed (e5b): 11ty fails at once on `json` of collection items
/// ("Converting circular structure to JSON"). The items share each other's
/// data through `pagination`, so writing them out walks a graph that grows
/// exponentially with each level of collection pagination; Brrrty stops at
/// the first item and refuses the page at once. 5,000 items, two levels.
#[test]
fn json_of_collection_items_stops_at_the_first_item() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    let list: Vec<String> = (0..5000).map(|i| i.to_string()).collect();
    write(
        root,
        "src/_data/list.json",
        &format!("[{}]", list.join(",")),
    );
    write(
        root,
        "src/items.liquid",
        "---\npagination:\n  data: list\n  size: 1\n  addAllPagesToCollections: true\ntags: l0\npermalink: \"i/{{ pagination.pageNumber }}/\"\n---\n{{ pagination.items[0] }}",
    );
    write(
        root,
        "src/lev1.liquid",
        "---\npagination:\n  data: collections.l0\n  size: 1\n  addAllPagesToCollections: true\ntags: l1\npermalink: \"l/{{ pagination.pageNumber }}/\"\n---\n{{ pagination.items[0].url }}",
    );
    write(
        root,
        "src/out.liquid",
        "---\npermalink: out.txt\n---\n{{ collections.l1 | json }}{{ collections.l1 | inspect }}",
    );
    let root_buf = root.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run(&root_buf));
    });
    let r = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the build took longer than 60 s");
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{:#?}", r.problems);
    assert_eq!(errs[0].path.as_deref(), Some(Path::new("src/out.liquid")));
    assert_eq!(errs[0].code, codes::DATA_UNSUPPORTED);
    assert!(!root.join("_site/out.txt").exists());
    assert_eq!(read(root, "l/4999/index.html"), "/i/4999/");
}
