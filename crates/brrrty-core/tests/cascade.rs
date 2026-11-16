//! The data cascade through `build()`: directory and template data files,
//! layout front matter, and front matter. Every expected output was probed
//! with real 11ty 3.1.8.

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

/// Probed: global data < layout front matter < `src/src.json` <
/// `src/src.11tydata.json` < `posts/posts.json` < `posts/posts.11tydata.json`
/// < `posts/deep/deep.json` < `x.json` < `x.11tydata.json` < front matter.
/// Arrays join in that order; objects merge key by key.
#[test]
fn the_cascade_follows_11ty_order() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/arr.json", r#"["g"]"#);
    write(root, "src/_data/lvl.json", r#""global""#);
    write(
        root,
        "src/_includes/base.liquid",
        "---\nlvl: layoutfm\narr: [layoutfm]\nlayoutonly: 1\n---\n{{ content }}",
    );
    write(
        root,
        "src/src.json",
        r#"{"lvl": "src.json", "arr": ["srcjson"], "obj": {"src": 1, "shared": "src"}}"#,
    );
    write(
        root,
        "src/src.11tydata.json",
        r#"{"lvl": "src.11tydata.json"}"#,
    );
    write(
        root,
        "src/posts/posts.json",
        r#"{"lvl": "posts.json", "arr": ["postsjson"], "obj": {"posts": 1, "shared": "posts"}, "nested": {"a": [1]}}"#,
    );
    write(
        root,
        "src/posts/posts.11tydata.json",
        r#"{"lvl": "posts.11tydata.json", "arr": ["posts11"]}"#,
    );
    write(
        root,
        "src/posts/deep/deep.json",
        r#"{"lvl": "deep.json", "arr": ["deep"], "nested": {"a": [2], "b": 1}}"#,
    );
    write(
        root,
        "src/posts/deep/x.json",
        r#"{"lvl": "x.json", "arr": ["xjson"]}"#,
    );
    write(
        root,
        "src/posts/deep/x.11tydata.json",
        r#"{"lvl": "x.11tydata.json", "arr": ["x11"]}"#,
    );
    write(
        root,
        "src/posts/deep/x.liquid",
        "---\nlayout: base.liquid\narr: [fm]\n---\nlvl={{ lvl }} arr={{ arr | join: \",\" }} obj={{ obj | json }} nested={{ nested | json }} {{ layoutonly }}",
    );
    write(
        root,
        "src/posts/deep/y.liquid",
        "lvl={{ lvl }} arr={{ arr | join: \",\" }}",
    );
    write(
        root,
        "src/top.liquid",
        "lvl={{ lvl }} arr={{ arr | join: \",\" }}",
    );
    write(
        root,
        "src/withlayout.liquid",
        "---\nlayout: base.liquid\n---\nlvl={{ lvl }} arr={{ arr | join: \",\" }}",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        read(root, "posts/deep/x/index.html"),
        r#"lvl=x.11tydata.json arr=g,layoutfm,srcjson,postsjson,posts11,deep,xjson,x11,fm obj={"src":1,"shared":"posts","posts":1} nested={"a":[1,2],"b":1} 1"#
    );
    assert_eq!(
        read(root, "posts/deep/y/index.html"),
        "lvl=deep.json arr=g,srcjson,postsjson,posts11,deep"
    );
    assert_eq!(
        read(root, "top/index.html"),
        "lvl=src.11tydata.json arr=g,srcjson"
    );
    // Layout front matter sits over global data and under the data files.
    assert_eq!(
        read(root, "withlayout/index.html"),
        "lvl=src.11tydata.json arr=g,layoutfm,srcjson"
    );
}

#[test]
fn directory_data_applies_only_to_its_folder() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a/a.json", r#"{"v": "a"}"#);
    write(root, "src/a/deeper/p.liquid", "{{ v }}");
    write(root, "src/a/p.liquid", "{{ v }}");
    write(root, "src/b/p.liquid", "[{{ v }}]");
    write(root, "src/p.liquid", "[{{ v }}]");
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(root, "a/p/index.html"), "a");
    assert_eq!(read(root, "a/deeper/p/index.html"), "a");
    assert_eq!(read(root, "b/p/index.html"), "[]");
    assert_eq!(read(root, "p/index.html"), "[]");
}

/// Probed: data files in `_includes` apply to nothing, layouts included.
#[test]
fn data_files_in_includes_are_not_read() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_includes/_includes.json", r#"{"m": "inc"}"#);
    write(root, "src/_includes/base.json", r#"{"m": "basejson"}"#);
    write(
        root,
        "src/_includes/base.11tydata.json",
        r#"{"m": "base11"}"#,
    );
    write(root, "src/_includes/base.liquid", "L[{{ m }}]{{ content }}");
    write(
        root,
        "src/p.liquid",
        "---\nlayout: base.liquid\n---\n[{{ m }}]",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(root, "p/index.html"), "L[][]");
}

/// Probed: a data file whose value isn't an object adds nothing.
#[test]
fn data_files_that_are_not_objects_add_nothing() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a/a.json", r#"["x", "y"]"#);
    write(root, "src/b/b.json", "null");
    write(root, "src/c/c.json", r#""str""#);
    write(root, "src/d/d.json", "");
    write(root, "src/e/e.json", "\u{feff}{\"bom\": 1}");
    for d in ["a", "b", "c", "d"] {
        write(root, &format!("src/{d}/p.liquid"), "[{{ 0 }}{{ x }}]");
    }
    write(root, "src/e/p.liquid", "{{ bom }}");
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    for d in ["a", "b", "c", "d"] {
        assert_eq!(read(root, &format!("{d}/p/index.html")), "[0]", "{d}");
    }
    assert_eq!(read(root, "e/p/index.html"), "1");
}

/// Probed: 11ty turns `tags` into a list in front matter, layout front
/// matter and data files (not global data): a string splits at commas,
/// repeats go, and entries become strings. The levels then join.
#[test]
fn tags_become_lists_and_join_across_levels() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/_includes/b.liquid",
        "---\ntags: lay\n---\n{{ content }}",
    );
    write(
        root,
        "src/t/t.json",
        r#"{"tags": "dir,dir2", "layout": "b.liquid"}"#,
    );
    write(
        root,
        "src/t/p.liquid",
        "---\ntags: fm\n---\n{{ tags | json }}",
    );
    write(
        root,
        "src/q.liquid",
        "---\nlayout: b.liquid\n---\n{{ tags | json }}",
    );
    write(
        root,
        "src/r.liquid",
        "---\nlayout: b.liquid\ntags: [fm2]\n---\n{{ tags | json }}",
    );
    write(
        root,
        "src/split.liquid",
        "---\ntags: \"a,b, c,a\"\n---\n{{ tags | json }}",
    );
    write(
        root,
        "src/coerce.liquid",
        "---\ntags: [1, \"1\", true, null, [x, y], {k: v}]\n---\n{{ tags | json }}",
    );
    write(
        root,
        "src/empty.liquid",
        "---\ntags: \"\"\n---\n{{ tags | json }}",
    );
    write(
        root,
        "src/null.liquid",
        "---\ntags:\n---\n{{ tags | json }}",
    );
    write(root, "src/_data/tags.json", r#""t-global""#);
    write(root, "src/global.liquid", "{{ tags | json }}");
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(root, "t/p/index.html"), r#"["lay","dir","dir2","fm"]"#);
    assert_eq!(read(root, "q/index.html"), r#"["lay"]"#);
    assert_eq!(read(root, "r/index.html"), r#"["lay","fm2"]"#);
    assert_eq!(read(root, "split/index.html"), r#"["a","b"," c"]"#);
    assert_eq!(
        read(root, "coerce/index.html"),
        r#"["1","1","true","null","x,y","[object Object]"]"#
    );
    assert_eq!(read(root, "empty/index.html"), "[]");
    assert_eq!(read(root, "null/index.html"), "[]");
    // Global data keeps its own `tags` as written.
    assert_eq!(read(root, "global/index.html"), r#""t-global""#);
}

/// 11ty stops the build on `tags` that are neither a string nor a list;
/// Brrrty refuses the pages it applies to.
#[test]
fn tags_that_are_not_strings_or_lists_are_errors() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/fm.liquid", "---\ntitle: x\ntags: 5\n---\nx");
    write(root, "src/d/d.json", r#"{"tags": {"a": 1}}"#);
    write(root, "src/d/p.liquid", "x");
    write(root, "src/d/q.liquid", "x");
    write(root, "src/ok.liquid", "ok");
    let r = run(root);
    let errs = errors(&r);
    assert_eq!(errs.len(), 2, "{:#?}", r.problems);
    let fm = errs
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/fm.liquid")))
        .unwrap();
    assert_eq!(fm.code, codes::TAGS_INVALID);
    assert_eq!(fm.line, Some(3));
    let d = errs
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/d/d.json")))
        .unwrap();
    assert_eq!(d.code, codes::TAGS_INVALID);
    assert!(d.help.as_deref().unwrap().contains("2 pages"), "{d:?}");
    assert!(!exists(root, "d/p/index.html"));
    assert_eq!(read(root, "ok/index.html"), "ok");
}

/// Probed: the layout whose front matter joins the data is the first truthy
/// `layout` of front matter, data files, then global data; the layout that
/// wraps the page is the merged `layout`. So `layout: false` over a data
/// file's layout keeps that layout's data but not its markup.
#[test]
fn a_falsy_layout_keeps_the_data_files_layout_data() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/layout.json", r#""base.liquid""#);
    write(
        root,
        "src/_includes/base.liquid",
        "---\nx: from-base\n---\nBASE[{{ content }}]",
    );
    write(
        root,
        "src/_includes/other.liquid",
        "---\nx: from-other\n---\nOTHER[{{ content }}]",
    );
    write(
        root,
        "src/fmfalse.liquid",
        "---\nlayout: false\n---\nx={{ x }}",
    );
    write(
        root,
        "src/fmempty.liquid",
        "---\nlayout: \"\"\n---\nx={{ x }}",
    );
    write(root, "src/d/d.json", r#"{"layout": "other.liquid"}"#);
    write(
        root,
        "src/d/fmfalse.liquid",
        "---\nlayout: false\n---\nx={{ x }}",
    );
    write(root, "src/d/plain.liquid", "x={{ x }}");
    write(root, "src/n/n.json", r#"{"layout": null}"#);
    write(root, "src/n/p.liquid", "x={{ x }}");
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(read(root, "fmfalse/index.html"), "x=from-base");
    assert_eq!(read(root, "fmempty/index.html"), "x=from-base");
    assert_eq!(read(root, "d/fmfalse/index.html"), "x=from-other");
    assert_eq!(read(root, "d/plain/index.html"), "OTHER[x=from-other]");
    assert_eq!(read(root, "n/p/index.html"), "x=from-base");
}

/// 11ty stops the whole build on a bad data file; Brrrty reports it once,
/// with its position, and builds the pages it doesn't apply to.
#[test]
fn a_bad_json_data_file_refuses_only_its_pages() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/bad/bad.json", "{\n  \"a\": 1,\n}\n");
    write(root, "src/bad/p.liquid", "x");
    write(root, "src/bad/sub/q.liquid", "x");
    write(root, "src/t.json", "{");
    write(root, "src/t.liquid", "x");
    write(root, "src/ok/p.liquid", "ok");
    let r = run(root);
    let errs = errors(&r);
    assert_eq!(errs.len(), 2, "{:#?}", r.problems);
    let bad = errs
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/bad/bad.json")))
        .unwrap();
    assert_eq!(bad.code, codes::DATA_FILE_INVALID);
    assert_eq!((bad.line, bad.column), (Some(3), Some(1)));
    assert!(bad.help.as_deref().unwrap().contains("2 pages"), "{bad:?}");
    let tmpl = errs
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/t.json")))
        .unwrap();
    assert!(tmpl.help.as_deref().unwrap().contains("1 page"), "{tmpl:?}");
    assert!(!exists(root, "bad/p/index.html"));
    assert!(!exists(root, "bad/sub/q/index.html"));
    assert!(!exists(root, "t/index.html"));
    assert_eq!(read(root, "ok/p/index.html"), "ok");
}

/// Probed: reserved names in a data file stop 11ty, as in front matter.
#[test]
fn reserved_names_in_data_files_refuse_their_pages() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/r/r.json", r#"{"content": "x"}"#);
    write(root, "src/r/p.liquid", "x");
    write(root, "src/m/m.json", r#"{"page": {"mine": 1}}"#);
    write(
        root,
        "src/m/p.liquid",
        "{{ page.mine }} {{ page.fileSlug }}",
    );
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::DATA_RESERVED);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/r/r.json")));
    assert!(!exists(root, "r/p/index.html"));
    assert_eq!(read(root, "m/p/index.html"), "1 p");
}

/// Probed: `eleventy` and `pkg` are frozen; 11ty stops the build when a
/// data file, layout or front matter adds to them, and ignores a value
/// that isn't an object with keys.
#[test]
fn adding_to_eleventy_or_pkg_outside_global_data_is_an_error() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/d/d.json", r#"{"pkg": {"extra": 1}}"#);
    write(root, "src/d/p.liquid", "x");
    write(root, "src/fm.liquid", "---\neleventy:\n  extra: 2\n---\nx");
    write(root, "src/s/s.json", r#"{"pkg": 5, "eleventy": "s"}"#);
    write(
        root,
        "src/s/a.liquid",
        "x={{ pkg | json }} v={{ eleventy.version }}",
    );
    let r = run(root);
    let errs = errors(&r);
    assert_eq!(errs.len(), 2, "{:#?}", r.problems);
    assert!(errs.iter().all(|p| p.code == codes::DATA_RESERVED));
    let mut paths: Vec<_> = errs.iter().map(|p| p.path.clone().unwrap()).collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            PathBuf::from("src/d/d.json"),
            PathBuf::from("src/fm.liquid")
        ]
    );
    assert_eq!(read(root, "s/a/index.html"), "x={} v=3.1.8");
}

/// Templated permalinks are refused, wherever they come from, until
/// Brrrty renders them.
#[test]
fn a_templated_permalink_from_a_data_file_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/posts/posts.json",
        r#"{"permalink": "/{{ page.fileSlug }}/", "kind": "post"}"#,
    );
    write(root, "src/posts/one.liquid", "{{ kind }}");
    write(
        root,
        "src/posts/own.liquid",
        "---\npermalink: /mine/\n---\n{{ kind }}",
    );
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::PERMALINK_TEMPLATED);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/posts/one.liquid")));
    assert!(p.message.contains("src/posts/posts.json"), "{}", p.message);
    assert!(!exists(root, "posts/one/index.html"));
    assert_eq!(read(root, "mine/index.html"), "post");
}

#[test]
fn unsupported_keys_from_data_files_name_the_file() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/p/p.json", r#"{"pagination": {"data": "x"}}"#);
    write(root, "src/p/a.liquid", "x");
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::FRONT_MATTER_UNSUPPORTED);
    assert!(p.message.contains("src/p/p.json"), "{}", p.message);
}

/// A JavaScript data file next to pages: Brrrty can't run it, so only the
/// pages that read one of its keys are refused. Probed outputs for the rest.
#[test]
fn a_javascript_data_file_refuses_only_pages_that_read_its_keys() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/s/s.11tydata.js",
        "// Keys: greeting, nested, fn\nmodule.exports = {\n  greeting: \"hi\", // a comment\n  'nested': { a: [1, 2], b: `t` },\n  fn(x) { return { y: x }; },\n};\n",
    );
    write(root, "src/s/reads.liquid", "{{ greeting }}");
    write(root, "src/s/nested.liquid", "{% if nested.a %}y{% endif %}");
    write(root, "src/s/other.liquid", "plain {{ title }}");
    write(root, "src/s/sets.liquid", "---\ngreeting: own\n---\nok");
    write(root, "src/out.liquid", "[{{ greeting }}]");
    let r = run(root);
    let errs = errors(&r);
    assert_eq!(errs.len(), 2, "{:#?}", r.problems);
    for p in &errs {
        assert_eq!(p.code, codes::DATA_JS_UNSUPPORTED);
        assert!(p.message.contains("src/s/s.11tydata.js"), "{}", p.message);
        assert!(p.help.as_deref().unwrap().contains(".rhai"), "{p:?}");
    }
    let mut paths: Vec<_> = errs.iter().map(|p| p.path.clone().unwrap()).collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            PathBuf::from("src/s/nested.liquid"),
            PathBuf::from("src/s/reads.liquid")
        ]
    );
    assert_eq!(read(root, "s/other/index.html"), "plain ");
    assert_eq!(read(root, "s/sets/index.html"), "ok");
    assert_eq!(read(root, "out/index.html"), "[]");
}

/// A JavaScript data file that sets a key changing how pages are built, or
/// whose keys Brrrty can't read off, refuses every page under it, with one
/// problem on the file.
#[test]
fn a_javascript_data_file_with_build_keys_refuses_every_page_under_it() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/c/c.11tydata.mjs",
        "export default {\n  eleventyComputed: { title: (d) => d.x },\n};\n",
    );
    write(root, "src/c/a.liquid", "a");
    write(root, "src/c/b.liquid", "b");
    write(
        root,
        "src/f/f.11tydata.cjs",
        "module.exports = function () { return { layout: 'x' }; };\n",
    );
    write(root, "src/f/a.liquid", "a");
    write(root, "src/ok.liquid", "ok");
    let r = run(root);
    let errs = errors(&r);
    assert_eq!(errs.len(), 2, "{:#?}", r.problems);
    let c = errs
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/c/c.11tydata.mjs")))
        .unwrap();
    assert_eq!(c.code, codes::DATA_JS_UNSUPPORTED);
    assert!(c.message.contains("`eleventyComputed`"), "{}", c.message);
    assert!(c.help.as_deref().unwrap().contains("2 pages"), "{c:?}");
    let f = errs
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/f/f.11tydata.cjs")))
        .unwrap();
    assert_eq!(f.code, codes::DATA_JS_UNSUPPORTED);
    assert!(f.help.as_deref().unwrap().contains("1 page"), "{f:?}");
    assert!(!exists(root, "c/a/index.html"));
    assert!(!exists(root, "f/a/index.html"));
    assert_eq!(read(root, "ok/index.html"), "ok");
}

/// A JSON data file next to a page can't stand in for a key a JavaScript
/// global data file sets: the merged value depends on the JavaScript.
#[test]
fn json_over_a_javascript_global_key_is_still_refused_when_read() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/site.js", "module.exports = {a: 1};");
    write(root, "src/d/d.json", r#"{"site": {"b": 2}}"#);
    write(root, "src/d/reads.liquid", "{{ site.b }}");
    write(root, "src/d/plain.liquid", "plain");
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::DATA_JS_UNSUPPORTED);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/d/reads.liquid")));
    assert!(p.message.contains("src/_data/site.js"), "{}", p.message);
    assert_eq!(read(root, "d/plain/index.html"), "plain");
}

/// Fix round 1: JavaScript files that only look like a plain export. 11ty
/// writes each of the first three somewhere else (probed), and stops on
/// the syntax errors, so every page under them is refused, never built.
#[test]
fn javascript_files_brrrty_cant_be_sure_of_refuse_every_page() {
    for (name, src) in [
        ("and", "module.exports = {x:1} && {permalink:\"/a/\"};"),
        (
            "bracket",
            "module.exports = {x:1}; module[\"exports\"] = {permalink:\"/m/\"};",
        ),
        (
            "define",
            "Object.defineProperty(module,\"exports\",{value:{permalink:\"/d/\"}});",
        ),
        ("comma", "module.exports = {a: 1 b: 2};"),
        ("brackets", "module.exports = { a: (1] };"),
        (
            "schema",
            "module.exports = { eleventyDataSchema: function () { throw new Error('x'); } };",
        ),
    ] {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        write(root, "src/j/j.11tydata.js", src);
        write(root, "src/j/p.liquid", "hi");
        write(root, "src/ok.liquid", "ok");
        let r = run(root);
        let p = only_error(&r);
        assert_eq!(p.code, codes::DATA_JS_UNSUPPORTED, "{name}");
        assert_eq!(
            p.path.as_deref(),
            Some(Path::new("src/j/j.11tydata.js")),
            "{name}"
        );
        assert!(
            fs::read_dir(root.join("_site"))
                .unwrap()
                .flatten()
                .all(|e| e.file_name() == "ok"),
            "{name}: only ok/ is written"
        );
        assert_eq!(read(root, "ok/index.html"), "ok", "{name}");
    }
}
