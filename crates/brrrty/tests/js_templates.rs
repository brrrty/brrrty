//! JavaScript templates (`.11ty.js`, `.11ty.cjs`, `.11ty.mjs`) through the
//! `brrrty` binary: off and on, what they render, what is refused, and the
//! checks they share with JavaScript data.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn has_node() -> bool {
    let ok = Command::new("node").arg("--version").output().is_ok();
    if !ok {
        assert!(std::env::var_os("CI").is_none(), "CI needs Node");
        eprintln!("skipped: no node on PATH");
    }
    ok
}

fn site(files: &[(&str, &str)]) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        let p = t.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }
    t
}

fn build(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .current_dir(dir)
        .args(["build", "--input", "src"])
        .args(args)
        .env_remove("BRRRTY_NODE")
        .env_remove("SOURCE_DATE_EPOCH")
        .output()
        .expect("run brrrty")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join("_site").join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

const ON: &str = "config.set_js_data(\"node\");\n";

#[test]
fn off_refuses_javascript_templates_and_names_the_switch() {
    let t = site(&[
        ("src/a.11ty.js", "module.exports = () => 'a';\n"),
        ("src/b.njk", "b"),
    ]);
    let o = build(t.path(), &[]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let out = text(&o);
    assert!(out.contains("src/a.11ty.js"), "{out}");
    assert!(
        out.contains("config.set_js_data(\\\"node\\\")")
            || out.contains("config.set_js_data(\"node\")"),
        "{out}"
    );
    assert!(out.contains("--js-data=node"), "{out}");
    assert_eq!(read(t.path(), "b/index.html"), "b");
    // The flag turns it off for one run.
    let t = site(&[
        ("brrrty.config.rhai", ON),
        ("src/a.11ty.js", "module.exports = () => 'a';\n"),
    ]);
    let o = build(t.path(), &["--js-data=off"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(!t.path().join("_site/a/index.html").exists());
}

#[test]
fn a_function_a_class_and_an_object_build() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/fn.11ty.js",
            "module.exports = function (d) { return `${d.title}|${this.page.url}|${d.page.fileSlug}`; };\nmodule.exports.data = { title: 'Fn' };\n",
        ),
        (
            "src/cls.11ty.cjs",
            "module.exports = class { data() { return { title: 'Cls', permalink: '/c-{{ x }}/' }; } render(d) { return d.title + ' ' + d.page.url; } };\n",
        ),
        (
            "src/obj.11ty.mjs",
            "export default { data: { title: 'Obj' }, async render(d) { return d.title; } };\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(read(t.path(), "fn/index.html"), "Fn|/fn/|fn");
    assert_eq!(read(t.path(), "c-{{ x }}/index.html"), "Cls /c-{{ x }}/");
    assert_eq!(read(t.path(), "obj/index.html"), "Obj");
}

#[test]
fn a_render_that_sorts_its_data_in_place_is_refused() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_data/list.js",
            "module.exports = { items: [3, 1, 2] };\n",
        ),
        (
            "src/sorted.11ty.js",
            "module.exports = ({ list }) => list.items.sort().join(',');\n",
        ),
        ("src/other.njk", "{{ list.items | join(',') }}"),
    ]);
    let o = build(t.path(), &[]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let out = text(&o);
    assert!(out.contains("changes the data it is given"), "{out}");
    assert!(out.contains("src/sorted.11ty.js"), "{out}");
    assert!(!t.path().join("_site/sorted/index.html").exists());
}

#[test]
fn state_kept_between_renders_is_caught_by_the_replay() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/count.11ty.js",
            "let n = 0;\nmodule.exports = class { data() { return { pagination: { data: 'items', size: 1 }, items: [1, 2, 3, 4], permalink: (d) => `/c/${d.pagination.pageNumber}/` }; } render() { n += 1; return String(n); } };\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("js-data-impure"), "{out}");
    assert!(!t.path().join("_site/c/0/index.html").exists(), "{out}");
}

#[test]
fn a_cache_that_gives_the_same_result_builds() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/cached.11ty.js",
            "const cache = new Map();\nconst slow = (x) => { if (!cache.has(x)) cache.set(x, x.toUpperCase()); return cache.get(x); };\nmodule.exports = class { data() { return { pagination: { data: 'items', size: 1 }, items: ['a', 'b', 'a'], permalink: (d) => `/k/${d.pagination.pageNumber}/` }; } render(d) { return slow(d.pagination.items[0]); } };\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(read(t.path(), "k/2/index.html"), "A");
}

#[test]
fn undefined_writes_nothing_and_warns() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        ("src/none.11ty.js", "module.exports = () => undefined;\n"),
        (
            "src/wrapped.11ty.js",
            "module.exports = { data: { layout: 'w.njk' }, render: () => undefined };\n",
        ),
        ("src/_includes/w.njk", "[{{ content }}]"),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    assert!(out.contains("js-template-empty"), "{out}");
    assert!(!t.path().join("_site/none/index.html").exists());
    // A layout prints it as 11ty's does.
    assert_eq!(read(t.path(), "wrapped/index.html"), "[]");
}

#[test]
fn a_number_is_refused_alone_and_printed_by_a_layout() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        ("src/n.11ty.js", "module.exports = () => 42;\n"),
        (
            "src/w.11ty.js",
            "module.exports = { data: { layout: 'w.njk' }, render: () => [1, 'b'] };\n",
        ),
        ("src/_includes/w.njk", "[{{ content }}]"),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("js-template-value"), "{out}");
    assert!(out.contains("a number (42)"), "{out}");
    assert_eq!(read(t.path(), "w/index.html"), "[1,b]");
}

#[test]
fn errors_name_the_file_and_line() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/boom.11ty.js",
            "module.exports = function (d) {\n  return d.missing.value;\n};\n",
        ),
        ("src/empty.11ty.js", "module.exports = {};\n"),
        (
            "src/str.11ty.js",
            "module.exports = { data: () => 'x', render: () => 'y' };\n",
        ),
        ("src/ok.njk", "ok"),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("src/boom.11ty.js:2:"), "{out}");
    assert!(out.contains("TypeError"), "{out}");
    assert!(
        out.contains("No JavaScript template returned from src/empty.11ty.js"),
        "{out}"
    );
    assert!(
        out.contains("Invalid data format returned from src/str.11ty.js: typeof string"),
        "{out}"
    );
    assert!(out.contains("js-template-invalid"), "{out}");
    assert_eq!(read(t.path(), "ok/index.html"), "ok");
}

#[test]
fn what_brrrty_cannot_follow_is_refused_by_page() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/slug.11ty.js",
            "module.exports = function () { return this.getCollectionItem([]); };\n",
        ),
        ("src/raw.njk", "{{ collections.all | length }}"),
        (
            "src/reads-raw.njk",
            "{% for p in collections.all %}{{ p.rawInput }}{% endfor %}",
        ),
        (
            "src/_includes/l.11ty.js",
            "module.exports = { render(d) { return this.page.url + d.content; } };\n",
        ),
        ("src/uses.njk", "---\nlayout: l.11ty.js\n---\nx"),
        ("src/ok.11ty.js", "module.exports = () => 'ok';\n"),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("calls `this.getCollectionItem`"), "{out}");
    assert!(out.contains("`rawInput`"), "{out}");
    assert!(
        out.contains("reads `this.page` in a JavaScript layout"),
        "{out}"
    );
    assert_eq!(read(t.path(), "ok/index.html"), "ok");
    assert_eq!(read(t.path(), "raw/index.html"), "5");
    for gone in ["slug", "reads-raw", "uses"] {
        assert!(
            !t.path().join(format!("_site/{gone}/index.html")).exists(),
            "{gone}: {out}"
        );
    }
}

#[test]
fn layouts_go_both_ways() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_includes/js.11ty.js",
            "module.exports = class { data() { return { layout: 'outer.njk', from: 'js' }; } render({ content, from, title }) { return `<js ${from} ${title}>${content}</js>`; } };\n",
        ),
        ("src/_includes/outer.njk", "<o>{{ content | safe }}</o>"),
        ("src/a.md", "---\ntitle: A\nlayout: js\n---\n*{{ from }}*"),
        (
            "src/b.11ty.js",
            "module.exports = { data: { title: 'B', layout: 'outer.njk' }, render: (d) => d.title };\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        read(t.path(), "a/index.html"),
        "<o><js js A><p><em>js</em></p>\n</js></o>"
    );
    assert_eq!(read(t.path(), "b/index.html"), "<o>B</o>");
}

#[test]
fn the_clock_in_a_render_is_read_once_and_replayed() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/now.11ty.js",
            "module.exports = () => String(new Date().getFullYear() > 2000);\n",
        ),
    ]);
    for _ in 0..3 {
        let o = build(t.path(), &[]);
        assert!(o.status.success(), "{}", text(&o));
        assert_eq!(read(t.path(), "now/index.html"), "true");
    }
}

#[test]
fn a_site_builds_the_same_bytes_every_time() {
    if !has_node() {
        return;
    }
    let mut files: Vec<(String, String)> = vec![
        ("brrrty.config.rhai".into(), ON.into()),
        ("src/_includes/base.njk".into(), "<main>{{ content | safe }}</main>".into()),
        (
            "src/list.11ty.js".into(),
            "module.exports = class { data() { return { layout: 'base.njk', pagination: { data: 'collections.post', size: 3 }, permalink: (d) => `/list/${d.pagination.pageNumber}/` }; } render({ pagination }) { return pagination.items.map((p) => `${p.data.title}@${p.url}`).join(';'); } };\n".into(),
        ),
        (
            "src/all.11ty.js".into(),
            "module.exports = ({ collections }) => collections.all.map((p) => p.url).join('\\n');\n".into(),
        ),
    ];
    for i in 0..40 {
        files.push((
            format!("src/posts/p{i:02}.md"),
            format!(
                "---\ntitle: P{i}\ntags: post\ndate: 2024-01-{:02}\n---\nBody {i}\n",
                i % 28 + 1
            ),
        ));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let t = site(&refs);
    let mut first: Option<Vec<(String, String)>> = None;
    for _ in 0..5 {
        let o = build(t.path(), &[]);
        assert!(o.status.success(), "{}", text(&o));
        let mut pages = Vec::new();
        for dir in ["list/0", "list/5", "list/13", "all"] {
            pages.push((
                dir.to_string(),
                read(t.path(), &format!("{dir}/index.html")),
            ));
        }
        match &first {
            None => first = Some(pages),
            Some(f) => assert_eq!(f, &pages),
        }
    }
}

#[test]
fn an_engine_override_on_a_javascript_template_is_refused() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/over.11ty.js",
            "module.exports = { data: { templateEngineOverride: 'njk' }, render: () => 'x' };\n",
        ),
        (
            "src/fine.njk",
            "---\ntemplateEngineOverride: 11ty.js\n---\nx",
        ),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.contains("`templateEngineOverride` on a JavaScript template"),
        "{out}"
    );
    assert!(
        out.contains("templateEngineOverride `11ty.js` uses 11ty.js"),
        "{out}"
    );
}

#[test]
fn collections_build_and_only_their_order_is_refused() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/job.11ty.js",
            "module.exports = { data: { tags: 'job', date: '2024-02-01', title: 'Js' }, render: () => 'js job' };\n",
        ),
        (
            "src/a.md",
            "---\ntags: job\ndate: 2024-01-01\ntitle: A\n---\na\n",
        ),
        (
            "src/sitemap.njk",
            "---\neleventyExcludeFromCollections: true\n---\n{% for p in collections.all %}{{ p.url }};{% endfor %}",
        ),
        (
            "src/keys.njk",
            "---\neleventyExcludeFromCollections: true\n---\n{% for k, v in collections %}{{ k }};{% endfor %}",
        ),
        (
            "src/keys-js.11ty.js",
            "module.exports = { data: { eleventyExcludeFromCollections: true }, render: ({ collections }) => Object.keys(collections).join(';') };\n",
        ),
        (
            "src/jobs-js.11ty.js",
            "module.exports = { data: { eleventyExcludeFromCollections: true }, render: ({ collections }) => collections.job.map((p) => p.data.title).join(';') };\n",
        ),
        (
            "src/paged.njk",
            "---\neleventyExcludeFromCollections: true\npagination:\n  data: collections.job\n  size: 1\npermalink: \"/paged/{{ pagination.pageNumber }}/\"\n---\n{{ pagination.items[0].url }}",
        ),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert_eq!(read(t.path(), "sitemap/index.html"), "/a/;/job/;");
    assert_eq!(read(t.path(), "jobs-js/index.html"), "A;Js");
    assert!(out.contains("src/keys.njk: this page lists the names in `collections`, whose order 11ty decides by file-read timing"), "{out}");
    assert!(out.contains("src/keys-js.11ty.js"), "{out}");
    assert_eq!(read(t.path(), "paged/1/index.html"), "/job/");
    for gone in ["keys/index.html", "keys-js/index.html"] {
        assert!(!t.path().join("_site").join(gone).exists(), "{gone}: {out}");
    }
}

#[test]
fn a_config_collection_in_added_order_is_refused_with_javascript_templates() {
    if !has_node() {
        return;
    }
    let t = site(&[
        (
            "brrrty.config.rhai",
            "config.set_js_data(\"node\");\nconfig.add_collection(\"added\", |api| api.get_all());\nconfig.add_collection(\"sorted\", |api| api.get_all_sorted());\n",
        ),
        (
            "src/j.11ty.js",
            "module.exports = { data: { date: '2024-01-02' }, render: () => 'j' };\n",
        ),
        ("src/a.md", "---\ndate: 2024-01-01\n---\na\n"),
        (
            "src/added.njk",
            "---\neleventyExcludeFromCollections: true\n---\n{{ collections.added | length }}",
        ),
        (
            "src/sorted.njk",
            "---\neleventyExcludeFromCollections: true\n---\n{% for p in collections.sorted %}{{ p.url }};{% endfor %}",
        ),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.contains("`get_all()` gives pages in the order 11ty added them"),
        "{out}"
    );
    assert_eq!(read(t.path(), "sorted/index.html"), "/a/;/j/;");
    assert!(!t.path().join("_site/added/index.html").exists(), "{out}");
}

#[test]
fn local_javascript_data_that_ran_keeps_collections_readable() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/posts/posts.11tydata.js",
            "module.exports = () => ({ tags: ['post'] });\n",
        ),
        ("src/posts/a.md", "---\ndate: 2024-01-01\n---\na\n"),
        ("src/posts/b.md", "---\ndate: 2024-01-02\n---\nb\n"),
        (
            "src/list.njk",
            "{% for p in collections.post %}{{ p.url }};{% endfor %}",
        ),
        (
            "src/keys.njk",
            "{% for k, v in collections %}{{ k }};{% endfor %}",
        ),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert_eq!(read(t.path(), "list/index.html"), "/posts/a/;/posts/b/;");
    assert!(out.contains("JavaScript data file (.11tydata.js)"), "{out}");
    assert!(!t.path().join("_site/keys/index.html").exists(), "{out}");
}

#[test]
fn filters_on_this_are_brrrtys_own() {
    if !has_node() {
        return;
    }
    let t = site(&[
        (
            "brrrty.config.rhai",
            "config.set_js_data(\"node\");\nconfig.add_filter(\"shout\", |s| s.to_upper() + \"!\");\n",
        ),
        (
            "src/f.11ty.js",
            "module.exports = function () { return [this.slugify('A B'), this.url('/x/'), this.shout('hi')].join(' '); };\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(read(t.path(), "f/index.html"), "a-b /x/ HI!");
}

/// A random filter called from a JavaScript template's `render` and its
/// permalink function runs, notes its pages, and passes the replay check:
/// the replays get the first run's answers.
#[test]
fn a_random_filter_on_this_runs_and_is_replayed() {
    if !has_node() {
        return;
    }
    let t = site(&[
        (
            "brrrty.config.rhai",
            "config.set_js_data(\"node\");\nconfig.add_random_filter(\"pick\", |a| a[(random() * a.len()).floor().to_int()]);\n",
        ),
        (
            "src/r.11ty.js",
            "module.exports = function () { return [1, 2, 3, 4, 5, 6].map(() => this.pick(['x', 'y', 'z'])).join(''); };\n",
        ),
        (
            "src/p.11ty.js",
            "module.exports = { data: { permalink() { return `/p-${this.pick(['a', 'b'])}/`; } }, render() { return 'p'; } };\n",
        ),
    ]);
    let run = |seed: &str| {
        let o = Command::new(env!("CARGO_BIN_EXE_brrrty"))
            .current_dir(t.path())
            .args(["build", "--input", "src"])
            .env_remove("BRRRTY_NODE")
            .env_remove("SOURCE_DATE_EPOCH")
            .env("BRRRTY_RANDOM_SEED", seed)
            .output()
            .expect("run brrrty");
        assert!(o.status.success(), "{}", text(&o));
        let out = text(&o);
        assert!(out.contains("0 errors, 0 warnings, 2 notes"), "{out}");
        assert!(out.contains("src/r.11ty.js"), "{out}");
        assert!(out.contains("src/p.11ty.js"), "{out}");
        let r = read(t.path(), "r/index.html");
        assert!(r.len() == 6 && r.chars().all(|c| "xyz".contains(c)), "{r}");
        let p = if t.path().join("_site/p-a/index.html").exists() {
            "a"
        } else {
            "b"
        };
        assert_eq!(read(t.path(), &format!("p-{p}/index.html")), "p");
        let _ = fs::remove_dir_all(t.path().join("_site"));
        format!("{r}|{p}")
    };
    assert_eq!(run("1"), run("1"));
}

/// When the collections can't be built, a JavaScript template or layout
/// that reads them is refused with why, never handed `undefined`.
#[test]
fn a_javascript_read_of_collections_under_a_gap_is_refused() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        // Refused by Brrrty, so the collections would lack its page.
        (
            "src/pk.njk",
            "---\npagination:\n  data: collections\n  size: 1\n---\nx",
        ),
        (
            "src/read.11ty.js",
            "module.exports = (d) => `n=${d.collections ? d.collections.all.length : 'none'}`;\n",
        ),
        (
            "src/keys.11ty.js",
            "module.exports = (d) => Object.keys(d).join(',');\n",
        ),
        (
            "src/_includes/l.11ty.js",
            "module.exports = (d) => `${d.content}${d.collections ? 'yes' : 'no'}`;\n",
        ),
        ("src/in-layout.njk", "---\nlayout: l.11ty.js\n---\nx"),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("src/read.11ty.js"), "{out}");
    assert!(
        out.contains("reads `collections`, but 1 template could not be built"),
        "{out}"
    );
    assert!(
        !out.contains("11ty stops the whole build here. Fix the template."),
        "{out}"
    );
    assert!(!t.path().join("_site/read/index.html").exists(), "{out}");
    assert!(
        !t.path().join("_site/in-layout/index.html").exists(),
        "{out}"
    );
    // Listing the data's names lists `collections` where 11ty has it.
    assert_eq!(
        read(t.path(), "keys/index.html"),
        "eleventy,pkg,page,collections"
    );
}

/// 11ty keeps one write flag per template: a permalink function giving
/// `false` (or `{ build: false }`) for any page writes none of them.
#[test]
fn a_permalink_function_false_for_one_page_writes_none() {
    if !has_node() {
        return;
    }
    let paged = |n: usize, other: &str| {
        format!(
            "module.exports = {{ data: {{ pagination: {{ data: 'list', size: 1 }}, list: [1, 2, 3], permalink: (d) => d.pagination.pageNumber === {n} ? {other} : `/p{n}/${{d.pagination.pageNumber}}/` }}, render: () => 'x' }};\n"
        )
    };
    let f0 = paged(0, "false");
    let f2 = paged(2, "false");
    let t = site(&[
        ("brrrty.config.rhai", ON),
        ("src/f0.11ty.js", &f0),
        ("src/f2.11ty.js", &f2),
        (
            "src/dd/dd.11tydata.js",
            "module.exports = { permalink: (d) => d.pagination.pageNumber === 1 ? false : `/dd/${d.pagination.pageNumber}/` };\n",
        ),
        (
            "src/dd/p.njk",
            "---\npagination:\n  data: list\n  size: 1\nlist: [1, 2, 3]\n---\nx",
        ),
        (
            "src/ctl.11ty.js",
            "module.exports = { data: { pagination: { data: 'list', size: 1 }, list: [1, 2], permalink: (d) => `/ctl/${d.pagination.pageNumber}/` }, render: () => 'c' };\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    assert!(out.contains("permalink-false-page"), "{out}");
    let mut written: Vec<String> = Vec::new();
    let mut stack = vec![t.path().join("_site")];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap().flatten() {
            if e.path().is_dir() {
                stack.push(e.path());
            } else {
                written.push(
                    e.path()
                        .strip_prefix(t.path().join("_site"))
                        .unwrap()
                        .display()
                        .to_string(),
                );
            }
        }
    }
    written.sort();
    assert_eq!(written, ["ctl/0/index.html", "ctl/1/index.html"], "{out}");
}

/// A computed permalink giving `false` for some pages makes 11ty write
/// `false` as a path and stop.
#[test]
fn a_computed_permalink_false_for_one_page_is_refused() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/cf.11ty.js",
            "module.exports = { data: { pagination: { data: 'list', size: 1 }, list: [1, 2], eleventyComputed: { permalink: (d) => d.pagination.pageNumber === 1 ? false : '/cf/' } }, render: () => 'x' };\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.contains("the computed `permalink` gives `false` for some pages"),
        "{out}"
    );
    assert!(!t.path().join("_site/cf/index.html").exists(), "{out}");
}

/// A paginated page's data lists `pagination` first, as 11ty's
/// `DeepCopy({ pagination }, data)` does; a JavaScript layout sees
/// `content` then the same (expected text from 11ty 3.1.8).
#[test]
fn a_paginated_pages_data_keys_are_in_11tys_order() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        ("src/_data/list.json", "[1, 2, 3]"),
        (
            "src/k3.11ty.js",
            "module.exports = { data: { pagination: { data: 'list', size: 1, alias: 'it' }, permalink: (d) => `/k3/${d.pagination.pageNumber}/` }, render: (d) => Object.keys(d).join(',') };\n",
        ),
        (
            "src/_includes/jl.11ty.js",
            "module.exports = { data: { a: 1 }, render(d) { return Object.keys(d).join(',') + '\\n' + d.content; } };\n",
        ),
        (
            "src/k4.njk",
            "---\nlayout: jl.11ty.js\npagination:\n  data: list\n  size: 1\npermalink: /k4/{{ pagination.pageNumber }}/\n---\nx\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        read(t.path(), "k3/1/index.html"),
        "pagination,list,eleventy,pkg,permalink,page,collections,it"
    );
    assert_eq!(
        read(t.path(), "k4/2/index.html"),
        "content,pagination,list,eleventy,pkg,a,layout,permalink,page,collections\nx\n"
    );
}

/// 11ty imports the layout a template's data names first (front matter,
/// then template and directory data, then global data): a JavaScript
/// layout lower in that order, overridden on every page, never runs.
#[test]
fn an_overridden_javascript_layout_never_runs() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/posts/posts.11tydata.json",
            "{\"layout\": \"post.11ty.js\"}",
        ),
        ("src/posts/a.md", "---\nlayout: plain.njk\n---\na"),
        (
            "src/_includes/post.11ty.js",
            "globalThis.__ran = true;\nmodule.exports = (d) => d.content;\n",
        ),
        ("src/_includes/plain.njk", "[{{ content | safe }}]"),
        (
            "src/probe.11ty.js",
            "module.exports = () => `unused=${globalThis.__ran ? 'ran' : 'no'}`;\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(read(t.path(), "probe/index.html"), "unused=no");
    assert_eq!(read(t.path(), "posts/a/index.html"), "[<p>a</p>\n]");
}

/// 11ty imports a JavaScript layout only when a page uses it: one in the
/// layouts folder that nothing names never runs, whatever it does.
#[test]
fn an_unused_javascript_layout_never_runs() {
    if !has_node() {
        return;
    }
    for body in [
        "globalThis.__unusedRan = true;\nmodule.exports = (d) => d.content;\n",
        "process.exit(0);\n",
        "while (true) {}\n",
        "await new Promise(() => {});\nexport default (d) => d.content;\n",
    ] {
        let name = if body.contains("await") {
            "src/_includes/unused.11ty.mjs"
        } else {
            "src/_includes/unused.11ty.js"
        };
        let t = site(&[
            ("brrrty.config.rhai", ON),
            (name, body),
            (
                "src/_includes/used.11ty.js",
                "module.exports = { data: { layout: 'outer.njk' }, render: (d) => `[${d.content}]` };\n",
            ),
            ("src/_includes/outer.njk", "<{{ content | safe }}>"),
            ("src/a.njk", "---\nlayout: used.11ty.js\n---\na"),
            (
                "src/probe.11ty.js",
                "module.exports = () => `unused=${globalThis.__unusedRan ? 'ran' : 'no'}`;\n",
            ),
        ]);
        let o = build(t.path(), &[]);
        assert!(o.status.success(), "{body}: {}", text(&o));
        assert_eq!(read(t.path(), "a/index.html"), "<[a]>", "{body}");
        assert_eq!(read(t.path(), "probe/index.html"), "unused=no", "{body}");
    }
}

/// 11ty runs each template's module code as its file finishes loading, in
/// an order of its own: state the modules share (a counter in a helper)
/// follows it, and Brrrty refuses rather than guess.
#[test]
fn module_state_shared_at_load_is_refused() {
    if !has_node() {
        return;
    }
    let counter = (
        "src/lib/counter.js",
        "let n = 0;\nmodule.exports = () => ++n;\n",
    );
    // In module code, printed by render.
    let t = site(&[
        ("brrrty.config.rhai", ON),
        counter,
        (
            "src/a.11ty.js",
            "const id = require('./lib/counter.js')();\nmodule.exports = () => `a${id}`;\n",
        ),
        (
            "src/b.11ty.js",
            "const id = require('./lib/counter.js')();\nmodule.exports = () => `b${id}`;\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("js-data-impure"), "{out}");
    assert!(!t.path().join("_site/a/index.html").exists(), "{out}");
    // In `data()`.
    let t = site(&[
        ("brrrty.config.rhai", ON),
        counter,
        (
            "src/a.11ty.js",
            "const next = require('./lib/counter.js');\nmodule.exports = { data: () => ({ id: next() }), render: (d) => `a${d.id}` };\n",
        ),
        (
            "src/b.11ty.js",
            "const next = require('./lib/counter.js');\nmodule.exports = { data: () => ({ id: next() }), render: (d) => `b${d.id}` };\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("in another order"), "{out}");
    // In directory data, read by a page with no function in sight.
    let t = site(&[
        ("brrrty.config.rhai", ON),
        counter,
        (
            "src/x/x.11tydata.js",
            "module.exports = { id: require('../lib/counter.js')() };\n",
        ),
        (
            "src/y/y.11tydata.js",
            "module.exports = { id: require('../lib/counter.js')() };\n",
        ),
        ("src/x/p.njk", "{{ id }}"),
        ("src/y/p.njk", "{{ id }}"),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("in another order"), "{out}");
    // Only the middle one shows its place, which the reverse keeps: the
    // rotation moves it.
    let t = site(&[
        ("brrrty.config.rhai", ON),
        counter,
        (
            "src/a.11ty.js",
            "require('./lib/counter.js')();\nmodule.exports = () => 'a';\n",
        ),
        (
            "src/b.11ty.js",
            "const id = require('./lib/counter.js')();\nmodule.exports = () => `b id=${id}`;\n",
        ),
        (
            "src/c.11ty.js",
            "require('./lib/counter.js')();\nmodule.exports = () => 'c';\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("js-data-impure"), "{out}");
    assert!(!t.path().join("_site/b/index.html").exists(), "{out}");
    // Module code that shares nothing builds.
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/a.11ty.js",
            "const when = new Date().getFullYear() > 2000;\nmodule.exports = () => `a${when}`;\n",
        ),
        (
            "src/b.11ty.js",
            "const r = Math.random() < 2;\nmodule.exports = () => `b${r}`;\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(read(t.path(), "b/index.html"), "btrue");
}

/// A permalink or computed function's `this` is 11ty's filters
/// (`_renderFunction`), which run Brrrty's own.
#[test]
fn permalink_and_computed_functions_call_filters_on_this() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/p.11ty.js",
            "module.exports = { data: { title: 'Hello World', permalink(d) { return `/${this.slugify(d.title)}/`; }, eleventyComputed: { s: function (d) { return this.slug(d.title) + '|' + typeof this.page + '|' + Object.keys(this).join(','); } } }, render: (d) => d.s };\n",
        ),
        (
            "src/plain.11ty.js",
            "module.exports = { data: { permalink() { return '/' + ('tmpl' in this ? 'has-tmpl' : 'no-tmpl') + '/'; } }, render: () => 'p' };\n",
        ),
        (
            "src/t.11ty.js",
            "module.exports = { data: { eleventyComputed: { s: function () { return typeof this.tmpl; } } }, render: (d) => d.s };\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    // Text from 11ty 3.1.8: a computed function's `this` ends with `tmpl`.
    assert_eq!(
        read(t.path(), "hello-world/index.html"),
        "hello-world|undefined|getBundle,getBundleFileUrl,inputPathToUrl,slug,slugify,url,log,getCollectionItemIndex,getCollectionItem,getPreviousCollectionItem,getNextCollectionItem,renderTransforms,tmpl"
    );
    assert!(out.contains("reads `this.tmpl`"), "{out}");
    // A plain permalink function's `this` has no `tmpl` in 11ty.
    assert_eq!(read(t.path(), "no-tmpl/index.html"), "p");
}

/// A filter a render calls from a timer that outlives it is named where
/// it comes from, never counted against another page.
#[test]
fn a_filter_called_after_its_render_names_its_template() {
    if !has_node() {
        return;
    }
    let mut files: Vec<(String, String)> = vec![
        ("brrrty.config.rhai".into(), ON.into()),
        (
            "src/timer.11ty.js".into(),
            "module.exports = function () { setTimeout(() => this.slugify('late'), 0); return 'timer'; };\n".into(),
        ),
    ];
    for i in 0..30 {
        files.push((
            format!("src/p{i}.11ty.js"),
            "module.exports = async function () { await new Promise((r) => setTimeout(r, 2)); return this.slugify('A B'); };\n".into(),
        ));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let t = site(&refs);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.contains("in src/timer.11ty.js after its render ended"),
        "{out}"
    );
}

/// `undefined` from a render reaches a JavaScript layout as `content`, as
/// in 11ty, which prints it as JavaScript does.
#[test]
fn undefined_content_reaches_a_javascript_layout() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_includes/l.11ty.js",
            "module.exports = (d) => `[${d.content}]`;\n",
        ),
        (
            "src/u.11ty.js",
            "module.exports = { data: { layout: 'l.11ty.js' }, render: () => undefined };\n",
        ),
    ]);
    let o = build(t.path(), &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(read(t.path(), "u/index.html"), "[undefined]");
}

/// 11ty stops computing a page's data at the function that throws: the
/// ones after it never run (here the third logs when it does). Brrrty
/// sends a page's functions together, and the worker stops at the same
/// place. The only runs left are the look each worker takes at what the
/// functions read, as 11ty takes one.
#[test]
fn computed_functions_after_one_that_throws_do_not_run() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/a.11ty.js",
            "module.exports = class {\n  data() { return { title: 't', eleventyComputed: {\n    one: (d) => `${d.title}1`,\n    two: (d) => { if (d.one === 't1') throw new Error('boom two'); return 'x'; },\n    three: (d) => { console.error('THREE RAN with two=' + JSON.stringify(d.two)); return `${d.two}3`; },\n    four: (d) => d.three + '4',\n  } }; }\n  render(d) { return d.four; }\n};\n",
        ),
        ("src/b.njk", "b"),
    ]);
    let o = build(t.path(), &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("boom two"), "{out}");
    // With the real value of `two` it never runs.
    assert!(!out.contains("THREE RAN with two=\"x\""), "{out}");
    let looks = out.matches("THREE RAN with two=\"").count();
    assert_eq!(looks, 3, "{out}");
    assert_eq!(read(t.path(), "b/index.html"), "b");
}

/// What Brrrty sends together comes back right: a page's data too large
/// for one write of the pipe, a function that waits behind a quick one
/// (its reply held, then sent by the timer), and a page whose function
/// changes its data, after which the workers are sent the shared values
/// again and the replay check steps through the calls one at a time.
#[test]
fn requests_sent_together_come_back_in_order() {
    if !has_node() {
        return;
    }
    let big = "x".repeat(300_000);
    let page = format!(
        "exports.data = {{ big: '{big}', eleventyComputed: {{\n  a: (d) => d.big.length,\n  b: async (d) => {{ await new Promise((r) => setTimeout(r, 150)); return d.a + 1; }},\n  c: (d) => d.b + 1,\n}} }};\nexports.render = (d) => [d.a, d.b, d.c].join();\n"
    );
    let mut files: Vec<(String, String)> = vec![
        ("brrrty.config.rhai".into(), ON.into()),
        ("src/_data/site.json".into(), r#"{"list":[{"n":1},{"n":2}],"o":{"k":1}}"#.into()),
        (
            "src/_data/h.js".into(),
            "module.exports = { poke(o) { o.k = 2; return ''; }, count(l) { return l.length; }, key(o) { return o.k; } };".into(),
        ),
        ("src/big.11ty.js".into(), page),
        ("src/poke.njk".into(), "{{ h.poke(site.o) }}".into()),
    ];
    for i in 0..8 {
        files.push((
            format!("src/p{i}.njk"),
            "{{ h.count(site.list) }}/{{ h.key(site.o) }}".into(),
        ));
    }
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let t = site(&files);
    let o = build(t.path(), &[]);
    let out = text(&o);
    // Only the page that changes its data is refused.
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("src/poke.njk"), "{out}");
    assert!(out.contains("changes the data it is given"), "{out}");
    assert!(!out.contains("different result"), "{out}");
    assert_eq!(read(t.path(), "big/index.html"), "300000,300001,300002");
    for i in 0..8 {
        assert_eq!(read(t.path(), &format!("p{i}/index.html")), "2/1");
    }
}
