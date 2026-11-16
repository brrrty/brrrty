//! Collections written in Rhai: `config.add_collection("name", |api| ...)`.
//!
//! What 11ty 3.1.8 does was probed with real 11ty; the fixture
//! `tests/fixtures/collections-config` pins the whole shape (`posts`,
//! `postsNewest` and a nested `meta`) byte for byte. These tests pin the
//! rest: the API, what is refused, and that items stay the same items.

use std::fs;
use std::path::{Path, PathBuf};

use brrrty_config::{CONFIG_FILE, Clock, load_with};
use brrrty_core::{BuildOptions, BuildReport, Problem, Severity, build, codes};

const NOON_MS: i64 = 1_699_963_200_000;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn site(script: &str, pages: &[(&str, &str)]) -> (tempfile::TempDir, BuildReport) {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        CONFIG_FILE,
        &format!("{script}\n#{{ dir: #{{ input: \"src\" }} }}\n"),
    );
    for (name, body) in pages {
        write(t.path(), &format!("src/{name}"), body);
    }
    let cfg = load_with(t.path(), Clock::Fixed(NOON_MS)).unwrap().unwrap();
    let opts = BuildOptions {
        root: t.path().to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        config: cfg.build_options(),
        ..Default::default()
    };
    let report = build(&opts, &brrrty_markdown::MarkdownIt::new());
    (t, report)
}

fn load_err(script: &str) -> Problem {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), CONFIG_FILE, script);
    match load_with(t.path(), Clock::Fixed(NOON_MS)) {
        Err(mut p) => {
            assert_eq!(p.len(), 1, "{p:#?}");
            p.remove(0)
        }
        Ok(_) => panic!("expected a problem"),
    }
}

fn out(t: &tempfile::TempDir, rel: &str) -> Option<String> {
    fs::read_to_string(t.path().join("_site").join(rel)).ok()
}

fn errors(r: &BuildReport) -> Vec<&Problem> {
    r.problems
        .iter()
        .filter(|p| p.severity == Severity::Error)
        .collect()
}

/// Three dated posts in two folders, and an undated-free page set.
fn posts() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "posts/2023-01-01-a.md",
            "---\ntitle: A\ncat: x\ntags: [t1]\n---\nBody *A*.\n",
        ),
        (
            "posts/2023-01-02-b.md",
            "---\ntitle: B\ncat: y\ntags: [t1, t2]\n---\nBody B.\n",
        ),
        (
            "posts/sub/2023-01-03-c.md",
            "---\ntitle: C\ncat: x\n---\nBody C.\n",
        ),
    ]
}

fn with(
    mut pages: Vec<(&'static str, &'static str)>,
    more: &[(&'static str, &'static str)],
) -> Vec<(&'static str, &'static str)> {
    pages.extend_from_slice(more);
    pages
}

const LIST: &str = "---\npermalink: list.txt\neleventyExcludeFromCollections: true\n---\n";

#[test]
fn a_collection_of_items_renders_their_content() {
    let (t, r) = site(
        r#"config.add_collection("newest", |api| api.get_filtered_by_glob("./src/posts/**/*.md").reverse());"#,
        &with(
            posts(),
            &[(
                "list.liquid",
                "---\npermalink: list.txt\neleventyExcludeFromCollections: true\n---\n{% for p in collections.newest %}{{ p.data.title }}={{ p.templateContent | strip }};{% endfor %}",
            )],
        ),
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        out(&t, "list.txt").unwrap(),
        "C=<p>Body C.</p>;B=<p>Body B.</p>;A=<p>Body <em>A</em>.</p>;"
    );
}

/// Review Focus 3: a nested object of items, built with `object()`, keeps
/// its key order, and the items in it are the pages themselves.
#[test]
fn nested_objects_keep_order_and_items() {
    let script = r#"
config.add_collection("meta", |api| {
    let by = object();
    for post in api.get_filtered_by_glob("./src/posts/**/*.md") {
        let k = post.data.cat;
        if !by.contains(k) { by[k] = []; }
        by[k].push(post);
    }
    let m = object();
    m.zz = object();
    m.zz.by = by;
    m.aa = 1;
    m
});
"#;
    let (t, r) = site(
        script,
        &with(
            posts(),
            &[(
                "list.liquid",
                &*Box::leak(format!("{LIST}{{% for k in collections.meta %}}{{{{ k[0] }}}} {{% endfor %}}|{{% for c in collections.meta.zz.by %}}{{{{ c[0] }}}}:{{% for p in c[1] %}}{{{{ p.url }}}}={{{{ p.content | strip }}}},{{% endfor %}} {{% endfor %}}").into_boxed_str()),
            )],
        ),
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        out(&t, "list.txt").unwrap(),
        "zz aa |x:/posts/2023-01-01-a/=<p>Body <em>A</em>.</p>,/posts/sub/2023-01-03-c/=<p>Body C.</p>, y:/posts/2023-01-02-b/=<p>Body B.</p>, "
    );
}

/// The API: `get_all` is in the order pages were added, the others are
/// sorted by date then input path. Probed with 11ty 3.1.8.
#[test]
fn the_api_filters_like_11ty() {
    let script = r#"
fn urls(items) { items.map(|i| i.url) }
config.add_collection("probe", |api| {
    let o = object();
    o.sorted = urls(api.get_all_sorted());
    o.t1 = urls(api.get_filtered_by_tag("t1"));
    o.t1t2 = urls(api.get_filtered_by_tags("t1", "t2"));
    o.none = urls(api.get_filtered_by_tags());
    o.unit = urls(api.get_filtered_by_tag(()));
    o.missing = urls(api.get_filtered_by_tag("nope"));
    o.globs = urls(api.get_filtered_by_glob(["./src/posts/*.md", "src/posts/sub/*"]));
    o.rel = urls(api.get_filtered_by_glob("posts/**"));
    o.count = api.get_all().len();
    o
});
"#;
    let (t, r) = site(
        script,
        &with(
            posts(),
            &[(
                "list.liquid",
                &*Box::leak(format!("{LIST}{{% for k in collections.probe %}}{{{{ k[0] }}}}={{{{ k[1] | json }}}}\n{{% endfor %}}").into_boxed_str()),
            )],
        ),
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    let a = "\"/posts/2023-01-01-a/\"";
    let b = "\"/posts/2023-01-02-b/\"";
    let c = "\"/posts/sub/2023-01-03-c/\"";
    assert_eq!(
        out(&t, "list.txt").unwrap(),
        format!(
            "sorted=[{a},{b},{c}]\nt1=[{a},{b}]\nt1t2=[{b}]\nnone=[{a},{b},{c}]\nunit=[{a},{b},{c}]\nmissing=[]\nglobs=[{a},{b},{c}]\nrel=[]\ncount=3\n"
        )
    );
}

/// `reverse()` returns the array, as JavaScript's does, and `slice` is
/// JavaScript's.
#[test]
fn array_helpers_return_arrays() {
    let script = r#"
config.add_collection("r", |api| {
    let o = object();
    let a = [1, 2, 3, 4];
    o.rev = a.reverse();
    o.same = a;
    o.s1 = [1, 2, 3, 4].slice(1, 3);
    o.s2 = [1, 2, 3, 4].slice(-2);
    o.s3 = [1, 2, 3, 4].slice(3, 1);
    o.s4 = [1, 2, 3, 4].slice(0, -1);
    o.s5 = [1, 2, 3, 4].slice(-10, 10);
    o
});
"#;
    let (t, r) = site(
        script,
        &[(
            "list.liquid",
            &*Box::leak(format!("{LIST}{{{{ collections.r | json }}}}").into_boxed_str()),
        )],
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    // Node: [1,2,3,4].reverse(), .slice(1,3), .slice(-2), .slice(3,1),
    // .slice(0,-1), .slice(-10,10).
    assert_eq!(
        out(&t, "list.txt").unwrap(),
        r#"{"rev":[4,3,2,1],"same":[4,3,2,1],"s1":[2,3],"s2":[3,4],"s3":[],"s4":[1,2,3],"s5":[1,2,3,4]}"#
    );
}

/// Items are shared with every page in 11ty, so a change to one would show
/// everywhere. Brrrty can't follow that, so changing one is an error.
#[test]
fn items_are_read_only() {
    for (body, what) in [
        ("post.data.title = \"x\";", "title"),
        ("post.url = \"x\";", "url"),
        ("post.data.tags.push(\"x\");", "push"),
        ("let t = post.data.tags; t.push(\"x\");", "push"),
        ("let d = post.data; d.title = \"x\";", "title"),
        ("post.data.remove(\"title\");", "remove"),
    ] {
        let script = format!(
            "config.add_collection(\"c\", |api| {{\n    for post in api.get_all() {{ {body} }}\n    []\n}});"
        );
        let (t, r) = site(
            &script,
            &with(
                posts(),
                &[(
                    "list.liquid",
                    &*Box::leak(format!("{LIST}{{{{ collections.c | size }}}}").into_boxed_str()),
                )],
            ),
        );
        let errs = errors(&r);
        assert!(
            errs.iter().any(|p| p.code == codes::CONFIG_ERROR
                && p.message.contains("collection `c`")
                && p.path.as_deref() == Some(Path::new(CONFIG_FILE))
                && p.line == Some(2)),
            "{body}: {errs:#?}"
        );
        assert!(
            errs.iter().any(|p| p.message.contains(what)),
            "{body}: {errs:#?}"
        );
        // The page that reads it is refused; the posts still build.
        assert!(out(&t, "list.txt").is_none(), "{body}");
        assert!(out(&t, "posts/2023-01-01-a/index.html").is_some());
    }
}

/// Reading an item without changing it is fine, even through a chain that
/// Rhai writes back.
#[test]
fn reading_items_through_chains_is_fine() {
    let script = r#"
config.add_collection("c", |api| {
    let o = object();
    for post in api.get_all() {
        o[post.data.title] = post.data.tags.len() + post.data.title.len();
        let tags = post.data.tags.to_array();
        tags.push("mine");
        o[post.data.title + "!"] = tags;
    }
    o
});
"#;
    let (t, r) = site(
        script,
        &[
            ("a.md", "---\ntitle: A\ndate: 2020-01-01\ntags: [x, y]\n---\n"),
            (
                "list.liquid",
                &*Box::leak(format!("{LIST}{{{{ collections.c | json }}}} {{{{ collections.all[0].data.tags | json }}}}").into_boxed_str()),
            ),
        ],
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        out(&t, "list.txt").unwrap(),
        r#"{"A":3,"A!":["x","y","mine"]} ["x","y"]"#
    );
}

/// 11ty renders content after it builds the collections, and stops the
/// whole build with "Tried to use templateContent too early" (probed).
/// Brrrty refuses the collection, and only the pages that read it.
#[test]
fn reading_content_in_a_collection_is_refused() {
    for key in ["templateContent", "content", "template"] {
        let script = format!("config.add_collection(\"c\", |api| api.get_all().map(|p| p.{key}));");
        let (t, r) = site(
            &script,
            &with(
                posts(),
                &[(
                    "list.liquid",
                    &*Box::leak(format!("{LIST}{{{{ collections.c | size }}}}").into_boxed_str()),
                )],
            ),
        );
        let errs = errors(&r);
        assert!(
            errs.iter()
                .any(|p| p.message.contains("collection `c`") && p.message.contains(key)),
            "{key}: {errs:#?}"
        );
        assert!(out(&t, "list.txt").is_none());
        assert!(out(&t, "posts/2023-01-01-a/index.html").is_some());
    }
}

/// A collection that throws is a problem at its line, with Rhai's message;
/// pages that read it are refused and the rest build.
#[test]
fn a_failing_collection_refuses_only_its_readers() {
    let (t, r) = site(
        "config.add_collection(\"ok\", |api| api.get_all());\nconfig.add_collection(\"bad\", |api| {\n    throw \"no luck\";\n});",
        &with(
            posts(),
            &[
                (
                    "reads-bad.liquid",
                    "---\npermalink: bad.txt\neleventyExcludeFromCollections: true\n---\n{{ collections.bad | size }}",
                ),
                (
                    "reads-ok.liquid",
                    "---\npermalink: ok.txt\neleventyExcludeFromCollections: true\n---\n{{ collections.ok | size }}",
                ),
            ],
        ),
    );
    let errs = errors(&r);
    let own = errs
        .iter()
        .find(|p| p.message.contains("collection `bad`") && p.message.contains("no luck"))
        .unwrap_or_else(|| panic!("{errs:#?}"));
    assert_eq!(own.path.as_deref(), Some(Path::new(CONFIG_FILE)));
    assert_eq!((own.line, own.column), (Some(3), Some(5)));
    assert!(
        errs.iter().any(
            |p| p.path.as_deref() == Some(Path::new("src/reads-bad.liquid"))
                && p.message.contains("collections.bad")
        ),
        "{errs:#?}"
    );
    assert!(out(&t, "bad.txt").is_none());
    assert_eq!(out(&t, "ok.txt").unwrap(), "3");
}

/// A result that isn't data, or a map that isn't an item but has an
/// `inputPath` (11ty would give it the content of that page), is refused.
#[test]
fn results_brrrty_cannot_reproduce_are_refused() {
    for (body, why) in [
        ("|api| || 1", "can't be data"),
        (
            "|api| { let o = object(); o.inputPath = \"./src/posts/2023-01-01-a.md\"; [o] }",
            "inputPath",
        ),
    ] {
        let (t, r) = site(
            &format!("config.add_collection(\"c\", {body});"),
            &with(
                posts(),
                &[(
                    "list.liquid",
                    &*Box::leak(format!("{LIST}{{{{ collections.c | size }}}}").into_boxed_str()),
                )],
            ),
        );
        let errs = errors(&r);
        assert!(
            errs.iter()
                .any(|p| p.message.contains("collection `c`") && p.message.contains(why)),
            "{body}: {errs:#?}"
        );
        assert!(out(&t, "list.txt").is_none());
    }
}

/// On a site with a config file, a name the config doesn't define and no
/// page tags is still refused: the JavaScript config may define it.
#[test]
fn unknown_names_stay_refused() {
    let (t, r) = site(
        "config.add_collection(\"known\", |api| api.get_all());",
        &with(
            posts(),
            &[
                (
                    "known.liquid",
                    "---\npermalink: known.txt\neleventyExcludeFromCollections: true\n---\n{{ collections.known | size }} {{ collections.t1 | size }}",
                ),
                (
                    "unknown.liquid",
                    "---\npermalink: unknown.txt\neleventyExcludeFromCollections: true\n---\n{{ collections.mystery | size }}",
                ),
            ],
        ),
    );
    assert_eq!(out(&t, "known.txt").unwrap(), "3 2");
    assert!(out(&t, "unknown.txt").is_none());
    assert!(
        errors(&r)
            .iter()
            .any(|p| p.message.contains("collections.mystery")),
        "{:#?}",
        r.problems
    );
}

/// A page that paginates a name that is neither a tag nor defined in the
/// config would be built from an empty collection; it is refused.
#[test]
fn paginating_an_unknown_name_is_refused() {
    let (t, r) = site(
        "config.add_collection(\"known\", |api| api.get_all());",
        &with(
            posts(),
            &[(
                "p.liquid",
                "---\npagination:\n  data: collections.mystery\n  size: 1\npermalink: \"p/{{ pagination.pageNumber }}/\"\ndate: 2020-01-01\n---\nx",
            )],
        ),
    );
    assert!(out(&t, "p/0/index.html").is_none());
    assert!(
        errors(&r).iter().any(|p| p.message.contains("mystery")),
        "{:#?}",
        r.problems
    );
}

/// A page Brrrty can't date has no place in a sorted collection. A result
/// that would hold it is refused; one that leaves it out is fine.
#[test]
fn undated_pages_poison_only_the_results_that_hold_them() {
    let (t, r) = site(
        "config.add_collection(\"every\", |api| api.get_all_sorted());\nconfig.add_collection(\"some\", |api| api.get_filtered_by_glob(\"./src/posts/**\"));",
        &with(
            posts(),
            &[
                (
                    "undated.md",
                    "---\ntitle: U\ndate: someday\n---\nA date Brrrty can't read.\n",
                ),
                (
                    "every.liquid",
                    "---\npermalink: every.txt\neleventyExcludeFromCollections: true\n---\n{{ collections.every | size }}",
                ),
                (
                    "some.liquid",
                    "---\npermalink: some.txt\neleventyExcludeFromCollections: true\n---\n{{ collections.some | size }}",
                ),
            ],
        ),
    );
    assert_eq!(out(&t, "some.txt").unwrap(), "3");
    assert!(out(&t, "every.txt").is_none());
    assert!(
        errors(&r)
            .iter()
            .any(|p| p.message.contains("collections.every") && p.message.contains("undated.md")),
        "{:#?}",
        r.problems
    );
}

#[test]
fn bad_definitions_are_config_problems() {
    let p = load_err(
        "config.add_collection(\"x\", |api| []);\nconfig.add_collection(\"x\", |api| []);",
    );
    assert_eq!(p.code, codes::CONFIG_INVALID);
    assert!(p.message.contains("already exists"), "{p:#?}");
    assert_eq!(p.line, Some(2));
    for name in ["all", "", "[keys]"] {
        let p = load_err(&format!("config.add_collection(\"{name}\", |api| []);"));
        assert_eq!(p.code, codes::CONFIG_INVALID, "{name}: {p:#?}");
    }
}

/// Unsupported glob syntax is refused rather than guessed.
#[test]
fn unsupported_globs_are_refused() {
    for glob in [
        "./src/{a,b}.md",
        "./src/[ab].md",
        "./src/!(a).md",
        "!./src/a.md",
        "/src/**",
        "../src/**",
        "./src//a.md",
        "./src/**.md",
        "./src/é.md",
    ] {
        let (_t, r) = site(
            &format!("config.add_collection(\"c\", |api| api.get_filtered_by_glob(\"{glob}\"));"),
            &with(
                posts(),
                &[(
                    "list.liquid",
                    &*Box::leak(format!("{LIST}{{{{ collections.c | size }}}}").into_boxed_str()),
                )],
            ),
        );
        assert!(
            errors(&r)
                .iter()
                .any(|p| p.message.contains("collection `c`") && p.message.contains("glob")),
            "{glob}: {:#?}",
            r.problems
        );
    }
}

/// The graph, end to end (probed with 11ty 3.1.8): a config collection
/// holds the pages added before its place, a template that paginates a
/// path into one comes after it, and a filter given an item reads its
/// content.
#[test]
fn config_collections_take_their_place_in_the_graph() {
    let script = r#"
config.add_collection("meta", |api| { let o = object(); o.x = api.get_all().map(|i| i.url); o });
config.add_collection("posts", |api| api.get_all());
config.add_collection("other", |api| api.get_all());
config.add_filter("tc", |p| p.templateContent);
"#;
    let (t, r) = site(
        script,
        &[
            ("a.md", "---\ndate: 2020-01-01\ntags: [t1]\n---\nA\n"),
            ("b.md", "---\ndate: 2020-01-02\ntags: [posts]\n---\nB\n"),
            (
                "c.md",
                "---\ndate: 2020-01-03\ntags: [listing]\npagination:\n  data: collections.posts\n  size: 1\npermalink: \"c/{{ pagination.pageNumber }}/\"\n---\nC{{ pagination.items[0].url }}\n",
            ),
            (
                "d.md",
                "---\ndate: 2020-01-04\npagination:\n  data: collections.meta.x\n  size: 1\npermalink: \"d/{{ pagination.pageNumber }}/\"\n---\nD{{ pagination.items[0] }}\n",
            ),
            (
                "e.md",
                "---\ndate: 2020-01-05\neleventyImport:\n  collections: [other]\ntags: [t5]\n---\nE\n",
            ),
            (
                "keys.liquid",
                "---\npermalink: keys.txt\neleventyExcludeFromCollections: true\n---\n{% for c in collections %}{{ c[0] }} {% endfor %}|{{ collections.meta.x | join: \",\" }}|{% for p in collections.other %}{{ p.url }},{% endfor %}|{{ collections.t1[0] | tc }}\n",
            ),
        ],
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        out(&t, "keys.txt").unwrap(),
        "t1 posts listing other t5 meta all meta.x |/a/,/b/,/c/0/,/e/|/a/,/b/,/c/0/,|<p>A</p>\n\n"
    );
    for (page, body) in [("c/1", "<p>C/b/</p>\n"), ("d/3", "<p>D/e/</p>\n")] {
        assert_eq!(out(&t, &format!("{page}/index.html")).unwrap(), body);
    }
}
