//! Liquid templates, page data and layouts through `build()`. Expected
//! outputs and behaviors were probed with real 11ty 3.1.8 (liquidjs 10.30.1).

use brrrty_core::{BuildOptions, BuildReport, Problem, Severity, build, codes};
use brrrty_markdown::MarkdownIt;
use std::fs;
use std::path::{Path, PathBuf};

fn opts(root: &Path) -> BuildOptions {
    BuildOptions {
        root: root.to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        passthrough: Vec::new(),
        strict: false,
    }
}

fn write(root: &Path, rel: &str, body: impl AsRef<[u8]>) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join("_site").join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn run(root: &Path) -> BuildReport {
    build(&opts(root), &MarkdownIt::new())
}

fn errors(r: &BuildReport) -> Vec<&Problem> {
    r.problems
        .iter()
        .filter(|p| matches!(p.severity, Severity::Error))
        .collect()
}

fn only_error(r: &BuildReport) -> &Problem {
    let all = errors(r);
    assert_eq!(all.len(), 1, "{:#?}", r.problems);
    all[0]
}

fn assert_clean(r: &BuildReport) {
    assert!(errors(r).is_empty(), "{:#?}", r.problems);
}

/// 11ty's data is a JavaScript object: integer-like keys come first in
/// ascending order, then the rest in insertion order. Probed with
/// `{% for kv in obj %}` over front matter keys `b`, `2`, `a`, `1`.
#[test]
fn page_data_follows_javascript_key_order() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/k.liquid",
        "---\nobj:\n  b: B\n  2: two\n  a: A\n  1: one\n---\n{% for kv in obj %}{{ kv[0] }}={{ kv[1] }};{% endfor %} {{ obj | json }}",
    );
    let r = run(root);
    assert_clean(&r);
    assert_eq!(
        read(root, "k/index.html"),
        r#"1=one;2=two;b=B;a=A; {"1":"one","2":"two","b":"B","a":"A"}"#
    );
}

/// `page` and `eleventy` as 11ty 3.1.8 prints them with `| json`.
#[test]
fn page_and_eleventy_objects_match_11ty() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/posts/2026-03-04-hello.liquid",
        "---\ndate: 2026-03-04\n---\n{{ page | json }}|{{ eleventy | json }}\n",
    );
    let r = run(root);
    assert_clean(&r);
    let real_root = root.canonicalize().unwrap();
    assert_eq!(
        read(root, "posts/2026-03-04-hello/index.html"),
        format!(
            concat!(
                r#"{{"inputPath":"./src/posts/2026-03-04-hello.liquid","fileSlug":"hello","filePathStem":"/posts/hello","#,
                r#""outputFileExtension":"html","templateSyntax":"liquid","date":"2026-03-04T00:00:00.000Z","#,
                r#""rawInput":"{{{{ page | json }}}}|{{{{ eleventy | json }}}}\n","url":"/posts/2026-03-04-hello/","#,
                r#""outputPath":"./_site/posts/2026-03-04-hello/index.html"}}|"#,
                r#"{{"version":"3.1.8","generator":"Eleventy v3.1.8","env":{{"source":"cli","runMode":"build","root":{root}}},"#,
                r#""directories":{{"input":"./src/","data":"./src/_data/","includes":"./src/_includes/","output":"./_site/"}}}}"#,
                "\n"
            ),
            root = serde_json::to_string(&real_root.display().to_string()).unwrap()
        )
    );
}

/// An absolute output folder is printed as given, as 11ty does.
#[test]
fn output_path_keeps_an_absolute_output_folder() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    let out = root.join("elsewhere");
    write(root, "src/a.liquid", "{{ page.outputPath }}");
    let mut o = opts(root);
    o.output = out.clone();
    let r = build(&o, &MarkdownIt::new());
    assert_clean(&r);
    assert_eq!(
        fs::read_to_string(out.join("a/index.html")).unwrap(),
        format!("{}/a/index.html", out.display())
    );
}

/// `templateSyntax` and the engines used, for each template type and
/// `templateEngineOverride` value.
#[test]
fn template_engines_follow_extension_and_override() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "*{{ page.templateSyntax }}*\n");
    write(root, "src/b.html", "*{{ page.templateSyntax }}*\n");
    write(root, "src/c.liquid", "*{{ page.templateSyntax }}*\n");
    write(
        root,
        "src/d.md",
        "---\ntemplateEngineOverride: md\n---\n*{{ x }}*\n",
    );
    write(
        root,
        "src/e.md",
        "---\ntemplateEngineOverride: liquid\n---\n*{{ page.templateSyntax }}*\n",
    );
    write(
        root,
        "src/f.html",
        "---\ntemplateEngineOverride: liquid,md\n---\n*{{ page.templateSyntax }}*\n",
    );
    write(
        root,
        "src/g.liquid",
        "---\ntemplateEngineOverride: md,liquid\n---\n*{{ page.templateSyntax }}*\n",
    );
    write(
        root,
        "src/h.md",
        "---\ntemplateEngineOverride: html\n---\n*{{ x }}*\n",
    );
    write(
        root,
        "src/i.md",
        "---\ntemplateEngineOverride: false\n---\n*{{ x }}*\n",
    );
    let r = run(root);
    assert_clean(&r);
    assert_eq!(read(root, "a/index.html"), "<p><em>liquid,md</em></p>\n");
    assert_eq!(read(root, "b/index.html"), "*liquid*\n");
    assert_eq!(read(root, "c/index.html"), "*liquid*\n");
    assert_eq!(read(root, "d/index.html"), "<p><em>{{ x }}</em></p>\n");
    assert_eq!(read(root, "e/index.html"), "*liquid*\n");
    assert_eq!(read(root, "f/index.html"), "<p><em>liquid,md</em></p>\n");
    assert_eq!(read(root, "g/index.html"), "<p><em>liquid,md</em></p>\n");
    assert_eq!(read(root, "h/index.html"), "*{{ x }}*\n");
    assert_eq!(read(root, "i/index.html"), "*{{ x }}*\n");
}

#[test]
fn unsupported_engine_overrides_are_errors() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/a.md",
        "---\ntemplateEngineOverride: njk\n---\nx\n",
    );
    write(
        root,
        "src/b.md",
        "---\ntemplateEngineOverride: liquid,njk\n---\nx\n",
    );
    write(root, "src/c.md", "---\ntemplateEngineOverride: 5\n---\nx\n");
    write(root, "src/ok.md", "ok\n");
    let r = run(root);
    let code_of = |rel: &str| {
        r.problems
            .iter()
            .find(|p| p.path.as_deref() == Some(Path::new(rel)))
            .map(|p| p.code)
    };
    assert_eq!(
        code_of("src/a.md"),
        Some(codes::TEMPLATE_ENGINE_UNSUPPORTED)
    );
    assert_eq!(
        code_of("src/b.md"),
        Some(codes::TEMPLATE_ENGINE_OVERRIDE_INVALID)
    );
    assert_eq!(
        code_of("src/c.md"),
        Some(codes::TEMPLATE_ENGINE_OVERRIDE_INVALID)
    );
    assert_eq!(errors(&r).len(), 3, "{:#?}", r.problems);
    assert!(root.join("_site/ok/index.html").exists());
}

/// Layout lookup: the name as written first, then with each template
/// extension in 11ty's order (`liquid`, `md`, `njk`, `html`, `11ty.js`).
/// `.md` layouts run Markdown after Liquid.
#[test]
fn layouts_resolve_like_11ty() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_includes/both.html", "HTML[{{ content }}]");
    write(root, "src/_includes/both.liquid", "LIQUID[{{ content }}]");
    write(root, "src/_includes/both.md", "MD *em* [{{ content }}]");
    write(root, "src/_includes/hm.html", "HTML[{{ content }}]");
    write(root, "src/_includes/hm.md", "MD[{{ content }}]");
    write(root, "src/_includes/sub/deep.liquid", "DEEP[{{ content }}]");
    write(root, "src/p1.md", "---\nlayout: both\n---\nx\n");
    write(root, "src/p2.md", "---\nlayout: hm\n---\nx\n");
    write(root, "src/p3.html", "---\nlayout: hm.html\n---\nx\n");
    write(root, "src/p4.html", "---\nlayout: ./hm.html\n---\nx\n");
    write(root, "src/p5.html", "---\nlayout: sub/deep\n---\nx");
    write(root, "src/p6.md", "---\nlayout: both.md\n---\nbody\n");
    write(root, "src/p7.md", "---\nlayout: false\n---\nnone\n");
    write(root, "src/p8.md", "---\nlayout: \"\"\n---\nnone\n");
    let r = run(root);
    assert_clean(&r);
    assert_eq!(read(root, "p1/index.html"), "LIQUID[<p>x</p>\n]");
    assert_eq!(read(root, "p2/index.html"), "<p>MD[<p>x</p>\n]</p>\n");
    assert_eq!(read(root, "p3/index.html"), "HTML[x\n]");
    assert_eq!(read(root, "p4/index.html"), "HTML[x\n]");
    assert_eq!(read(root, "p5/index.html"), "DEEP[x]");
    assert_eq!(
        read(root, "p6/index.html"),
        "<p>MD <em>em</em> [<p>body</p>\n]</p>\n"
    );
    assert_eq!(read(root, "p7/index.html"), "<p>none</p>\n");
    assert_eq!(read(root, "p8/index.html"), "<p>none</p>\n");
}

/// Layout front matter merges under the page's, deeply: arrays join
/// (farthest layout first), objects merge, and an object beats a string.
/// The layout's `permalink`, `date` and `templateEngineOverride` apply to
/// the page. `content` is the rendered child; `layoutContent` is gone.
#[test]
fn layout_data_merges_like_11ty() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/_includes/base.liquid",
        "---\nx: base-x\ny: base-y\nlist: [b]\n---\n<html>{{ content }}|x={{ x }}|y={{ y }}|url={{ page.url }}</html>",
    );
    write(
        root,
        "src/_includes/post.liquid",
        "---\nlayout: base\nx: post-x\nlist: [p]\nobj: {a: 1, b: 2}\nauthor: {name: lay}\n---\n<article>{{ content }}|lc={{ layoutContent }}|x={{ x }}|title={{ title }}|layout={{ layout }}</article>",
    );
    write(
        root,
        "src/chain.md",
        "---\nlayout: post\ntitle: T\nlist: [c]\nobj: {b: 3, c: 4}\nauthor: Bob\n---\n{{ list | join: \",\" }} {{ obj | json }} {{ author | json }}\n",
    );
    write(
        root,
        "src/_includes/moved.liquid",
        "---\npermalink: /from-layout/\ndate: 2001-02-03T12:00:00Z\n---\nM[{{ content }}]",
    );
    write(
        root,
        "src/moved.md",
        "---\nlayout: moved\n---\n{{ page.date | date: \"%Y-%m-%d\" }} {{ page.url }}\n",
    );
    write(
        root,
        "src/_includes/mdonly.liquid",
        "---\ntemplateEngineOverride: md\n---\nO[{{ content }}]",
    );
    write(
        root,
        "src/over.md",
        "---\nlayout: mdonly\n---\n# {{ \"x\" }}\n",
    );
    let r = run(root);
    assert_clean(&r);
    assert_eq!(
        read(root, "chain/index.html"),
        "<html><article><p>b,p,c {&quot;a&quot;:1,&quot;b&quot;:3,&quot;c&quot;:4} {&quot;name&quot;:&quot;lay&quot;}</p>\n|lc=|x=post-x|title=T|layout=post</article>|x=post-x|y=base-y|url=/chain/</html>"
    );
    assert_eq!(
        read(root, "from-layout/index.html"),
        "M[<p>2001-02-03 /from-layout/</p>\n]"
    );
    assert!(!root.join("_site/moved/index.html").exists());
    assert_eq!(
        read(root, "over/index.html"),
        "O[<h1>{{ &quot;x&quot; }}</h1>\n]"
    );
}

/// Review focus 1: a layout cycle is one error naming the cycle, however
/// many pages use it, and the other pages still build.
#[test]
fn a_layout_cycle_is_one_error_and_the_build_goes_on() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/_includes/main.liquid",
        "---\nlayout: post\n---\nM{{ content }}",
    );
    write(
        root,
        "src/_includes/post.liquid",
        "---\ntitle: p\nlayout: main\n---\nP{{ content }}",
    );
    write(
        root,
        "src/_includes/self.liquid",
        "---\nlayout: self\n---\nS{{ content }}",
    );
    write(root, "src/a.md", "---\nlayout: main\n---\na\n");
    write(root, "src/b.md", "---\nlayout: main\n---\nb\n");
    write(root, "src/c.md", "---\nlayout: post\n---\nc\n");
    write(root, "src/s.md", "---\nlayout: self\n---\ns\n");
    write(root, "src/ok.md", "ok\n");
    let r = run(root);
    let cycles: Vec<_> = errors(&r);
    assert_eq!(cycles.len(), 2, "{:#?}", r.problems);
    let main = cycles
        .iter()
        .find(|p| p.message == "layout cycle: main -> post -> main")
        .unwrap_or_else(|| panic!("{:#?}", r.problems));
    assert_eq!(main.code, codes::LAYOUT_CYCLE);
    assert_eq!(
        main.path.as_deref(),
        Some(Path::new("src/_includes/post.liquid"))
    );
    assert_eq!((main.line, main.column), (Some(3), Some(1)));
    assert!(
        main.help.as_deref().unwrap().contains("3 pages"),
        "{:?}",
        main.help
    );
    let s = cycles
        .iter()
        .find(|p| p.message == "layout cycle: self -> self")
        .unwrap();
    assert_eq!(
        s.path.as_deref(),
        Some(Path::new("src/_includes/self.liquid"))
    );
    assert!(root.join("_site/ok/index.html").exists());
    for page in ["a", "b", "c", "s"] {
        assert!(!root.join(format!("_site/{page}/index.html")).exists());
    }
}

#[test]
fn layout_problems_are_clear_errors() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_includes/n.njk", "{{ content | safe }}");
    write(root, "src/_includes/folder/x.liquid", "x");
    write(root, "src/_includes/noext", "x");
    write(
        root,
        "src/_includes/endsbad.liquid",
        "---\nlayout: false\n---\n{{ content }}",
    );
    write(root, "src/missing.md", "---\nlayout: nope\n---\nx\n");
    write(root, "src/njk.md", "---\nlayout: n\n---\nx\n");
    write(root, "src/folder.md", "---\nlayout: folder\n---\nx\n");
    write(root, "src/noext.md", "---\nlayout: noext\n---\nx\n");
    write(root, "src/endsbad.md", "---\nlayout: endsbad\n---\nx\n");
    write(root, "src/number.md", "---\nlayout: 5\n---\nx\n");
    write(root, "src/ok.md", "ok\n");
    let r = run(root);
    let find = |code: &str| {
        r.problems
            .iter()
            .find(|p| p.code == code)
            .unwrap_or_else(|| panic!("no {code}: {:#?}", r.problems))
    };
    let missing = find(codes::LAYOUT_MISSING);
    assert_eq!(missing.path.as_deref(), Some(Path::new("src/missing.md")));
    assert_eq!(missing.line, Some(2));
    assert_eq!(missing.message, "layout `nope` not found");
    assert!(
        missing
            .help
            .as_deref()
            .unwrap()
            .contains("src/_includes/nope.liquid"),
        "{:?}",
        missing.help
    );
    let njk = find(codes::LAYOUT_UNSUPPORTED);
    assert_eq!(njk.path.as_deref(), Some(Path::new("src/_includes/n.njk")));
    assert_eq!(
        njk.help.as_deref(),
        Some(
            "Nunjucks support is in progress. Liquid, Markdown and HTML layouts work. 1 page uses this layout and was not written."
        )
    );
    let invalid: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::LAYOUT_INVALID)
        .map(|p| p.path.clone().unwrap())
        .collect();
    assert_eq!(
        invalid,
        [
            PathBuf::from("src/_includes/endsbad.liquid"),
            PathBuf::from("src/_includes/folder"),
            PathBuf::from("src/_includes/noext"),
            PathBuf::from("src/number.md"),
        ]
    );
    assert_eq!(errors(&r).len(), 6, "{:#?}", r.problems);
    assert!(root.join("_site/ok/index.html").exists());
}

/// Review focus 3: an error is reported in the file it came from, with the
/// line and column in that file, and names the page when that differs.
#[test]
fn an_error_in_an_include_is_reported_at_the_include() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_includes/inc.liquid", "l1\nl2 {% if %}\n");
    write(
        root,
        "src/a.md",
        "---\ntitle: x\n---\ntop\n{% include \"inc\" %}\n",
    );
    write(root, "src/ok.md", "ok\n");
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::LIQUID_SYNTAX);
    assert_eq!(
        p.path.as_deref(),
        Some(Path::new("src/_includes/inc.liquid"))
    );
    assert_eq!((p.line, p.column), (Some(2), Some(9)));
    assert_eq!(
        p.message,
        "invalid value expression: \"\" while rendering src/a.md"
    );
    assert!(root.join("_site/ok/index.html").exists());
    assert!(!root.join("_site/a/index.html").exists());
}

#[test]
fn an_error_in_a_page_counts_its_front_matter_lines() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/a.md",
        "---\ntitle: x\n---\nline one\n  {{ title | upcasee }}\n",
    );
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::LIQUID_UNKNOWN_FILTER);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/a.md")));
    assert_eq!((p.line, p.column), (Some(5), Some(3)));
    assert_eq!(p.message, "undefined filter: upcasee");
    assert_eq!(p.help.as_deref(), Some("did you mean `upcase`?"));
}

/// A layout that does not parse is one error, at the layout's own line,
/// for every page that uses it. A render error in a layout names the page.
#[test]
fn errors_in_layouts_point_at_the_layout() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/_includes/bad.liquid",
        "---\nx: 1\n---\nline\n  {{ content | nofilter }}\n",
    );
    write(root, "src/a.md", "---\nlayout: bad\n---\na\n");
    write(root, "src/b.md", "---\nlayout: bad\n---\nb\n");
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::LIQUID_UNKNOWN_FILTER);
    assert_eq!(
        p.path.as_deref(),
        Some(Path::new("src/_includes/bad.liquid"))
    );
    assert_eq!((p.line, p.column), (Some(5), Some(3)));
    assert_eq!(p.message, "undefined filter: nofilter");
    assert!(
        p.help.as_deref().unwrap().contains("2 pages"),
        "{:?}",
        p.help
    );

    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/_includes/l.liquid",
        "---\nx: 1\n---\n{{ content }}\n{% include 'gone' %}",
    );
    write(root, "src/a.md", "---\nlayout: l\n---\na\n");
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::LIQUID_RENDER);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/_includes/l.liquid")));
    assert_eq!((p.line, p.column), (Some(5), Some(1)));
    assert_eq!(
        p.message,
        "include file not found: \"gone.liquid\" while rendering src/a.md"
    );
}

/// Ruling 4: LiquidJS's own `{% layout %}` tag stays unsupported.
#[test]
fn the_liquid_layout_tag_is_a_page_error() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_includes/l.liquid", "L{{ content }}");
    write(
        root,
        "src/p.liquid",
        "---\nt: 1\n---\n{% layout \"l\" %}x\n",
    );
    write(root, "src/ok.liquid", "{% layout none %}fine");
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::LIQUID_RENDER);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/p.liquid")));
    assert_eq!((p.line, p.column), (Some(4), Some(1)));
    assert_eq!(
        p.message,
        "the `{% layout %}` tag with a file isn't supported"
    );
    assert!(p.help.as_deref().unwrap().contains("layout: name"));
    assert_eq!(read(root, "ok/index.html"), "fine");
}

/// Ruling 1: the loader serves only files inside `_includes` and the input
/// folder, and says so when a name points elsewhere.
#[cfg(unix)]
#[test]
fn includes_cannot_leave_the_project() {
    let t = tempfile::tempdir().unwrap();
    let outside = t.path().join("outside");
    write(&outside, "secret.liquid", "SECRET");
    let root = &t.path().join("proj");
    write(root, "src/_includes/ok.liquid", "OK");
    std::os::unix::fs::symlink(
        outside.join("secret.liquid"),
        root.join("src/_includes/link.liquid"),
    )
    .unwrap();
    let secret = outside.join("secret.liquid");
    write(root, "src/a.liquid", "{% include '../../etc/passwd' %}");
    write(root, "src/b.liquid", "{% include '/etc/passwd' %}");
    write(root, "src/c.liquid", "{% include 'link' %}");
    write(
        root,
        "src/d.liquid",
        format!("{{% include '{}' %}}", secret.display()),
    );
    write(
        root,
        "src/e.liquid",
        "{% include '../../../outside/secret.liquid' %}",
    );
    write(
        root,
        "src/f.liquid",
        "{% include 'sub/../ok' %}|{% include './ok.liquid' %}",
    );
    let r = run(root);
    let errs = errors(&r);
    assert_eq!(errs.len(), 5, "{:#?}", r.problems);
    for p in &errs {
        assert_eq!(p.code, codes::LIQUID_RENDER, "{p:?}");
        assert!(p.message.starts_with("cannot read include file"), "{p:?}");
        assert!(p.message.contains("outside"), "{p:?}");
    }
    assert_eq!(read(root, "f/index.html"), "OK|OK");
    for page in ["a", "b", "c", "d", "e"] {
        assert!(!root.join(format!("_site/{page}/index.html")).exists());
    }
}

/// Includes come from `_includes`, then the input folder.
#[test]
fn includes_search_includes_then_input() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_includes/x.liquid", "INC");
    write(root, "src/x.liquid", "INPUT");
    write(root, "src/only.html", "<b>ONLY-INPUT</b>");
    write(
        root,
        "src/p.md",
        "---\npermalink: /p/\n---\n{% include 'x' %} {% include 'only.html' %}\n",
    );
    write(root, "src/q.liquid", "{% include 'missing' %}");
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.message, "include file not found: \"missing.liquid\"");
    assert_eq!(
        p.help.as_deref(),
        Some(concat!(
            "tried: src/_includes/missing.liquid, src/missing.liquid; ",
            "\".liquid\" was added because \"missing\" has no extension; write the whole file name to include another kind of file"
        ))
    );
    assert_eq!(read(root, "p/index.html"), "<p>INC <b>ONLY-INPUT</b></p>\n");
}

/// Ruling 3: rendering runs on threads with room for deep documents.
#[test]
fn deep_documents_do_not_overflow_the_stack() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/quotes.md", format!("{}x\n", ">".repeat(10_000)));
    let ifs = 250;
    write(
        root,
        "src/ifs.liquid",
        format!(
            "{}deep{}",
            "{% if true %}".repeat(ifs),
            "{% endif %}".repeat(ifs)
        ),
    );
    let too_deep = 5_000;
    write(
        root,
        "src/too-deep.liquid",
        format!(
            "{}x{}",
            "{% if true %}".repeat(too_deep),
            "{% endif %}".repeat(too_deep)
        ),
    );
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/too-deep.liquid")));
    assert!(root.join("_site/quotes/index.html").exists());
    assert_eq!(read(root, "ifs/index.html"), "deep");
}

/// Data Brrrty can't read yet is an error on the pages that use it, not
/// output that silently differs.
#[test]
fn pages_that_need_unsupported_data_are_errors() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/site.js", "module.exports = {title: 'S'};");
    write(root, "src/_data/nested/x.js", "module.exports = {};");
    write(root, "src/uses-data.md", "{{ site.title }}\n");
    write(root, "src/uses-dir.md", "{% if nested %}{% endif %}\n");
    write(root, "src/_data/plain.json", "{\"n\": 1}");
    write(root, "src/uses-json.md", "{{ plain.n }}\n");
    write(
        root,
        "src/uses-collections.liquid",
        "{% for p in collections.all %}{% endfor %}",
    );
    write(root, "src/posts/posts.json", "{\"layout\": \"x\"}");
    write(root, "src/posts/one.md", "plain\n");
    write(root, "src/other/thing.11tydata.js", "module.exports = {};");
    write(root, "src/other/thing.md", "plain\n");
    write(root, "src/fine.md", "a{{ title }}{{ unknown }}b\n");
    let r = run(root);
    let by_path = |rel: &str| {
        r.problems
            .iter()
            .find(|p| p.path.as_deref() == Some(Path::new(rel)))
            .unwrap_or_else(|| panic!("no problem for {rel}: {:#?}", r.problems))
    };
    let d = by_path("src/uses-data.md");
    assert_eq!(d.code, codes::DATA_UNSUPPORTED);
    assert_eq!(
        d.message,
        "this page uses `site` from src/_data, which Brrrty doesn't read yet"
    );
    assert_eq!(by_path("src/uses-dir.md").code, codes::DATA_UNSUPPORTED);
    let c = by_path("src/uses-collections.liquid");
    assert_eq!(
        c.message,
        "this page uses `collections`, which Brrrty doesn't build yet"
    );
    let posts = by_path("src/posts/one.md");
    assert_eq!(
        posts.message,
        "data file src/posts/posts.json applies to this page, and Brrrty doesn't read data files yet"
    );
    assert_eq!(by_path("src/other/thing.md").code, codes::DATA_UNSUPPORTED);
    assert_eq!(errors(&r).len(), 5, "{:#?}", r.problems);
    assert_eq!(read(root, "fine/index.html"), "<p>ab</p>\n");
    // JSON data is read, so a page using only it builds.
    assert_eq!(read(root, "uses-json/index.html"), "<p>1</p>\n");
    assert!(!root.join("_site/uses-data/index.html").exists());
}

/// 11ty refuses to build when data sets one of its reserved names.
#[test]
fn reserved_data_names_are_errors() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "---\ncontent: mine\n---\nx\n");
    write(root, "src/b.md", "---\npage:\n  url: /x/\n---\nx\n");
    write(
        root,
        "src/c.md",
        "---\npage:\n  mine: 1\n---\n{{ page.mine }} {{ page.fileSlug }}\n",
    );
    let r = run(root);
    let codes_found: Vec<_> = errors(&r)
        .iter()
        .map(|p| (p.path.clone().unwrap(), p.code))
        .collect();
    assert_eq!(
        codes_found,
        [
            (PathBuf::from("src/a.md"), codes::DATA_RESERVED),
            (PathBuf::from("src/b.md"), codes::DATA_RESERVED),
        ]
    );
    assert_eq!(read(root, "c/index.html"), "<p>1 c</p>\n");
}

/// Plan 4 renders templated permalinks; until then they stay refused, as
/// Plan 1 did, rather than written to the wrong place.
#[test]
fn templated_permalinks_are_still_refused() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/p.md",
        "---\ntitle: T\npermalink: \"/{{ title | slugify }}/\"\n---\nx\n",
    );
    let r = run(root);
    assert_eq!(only_error(&r).code, codes::PERMALINK_TEMPLATED);
}

#[test]
fn package_json_is_pkg() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.liquid", "[{{ pkg | json }}]");
    let r = run(root);
    assert_clean(&r);
    assert_eq!(read(root, "a/index.html"), "[{}]");
    write(
        root,
        "package.json",
        "{\"name\":\"n\",\"version\":\"1.2.3\"}",
    );
    let r = run(root);
    assert_clean(&r);
    assert_eq!(
        read(root, "a/index.html"),
        r#"[{"name":"n","version":"1.2.3"}]"#
    );
}

/// Front matter scalars resolve as js-yaml resolves them (probed with 11ty 3.1.8,
/// whose gray-matter calls js-yaml 4.3.2): `0123` is 123, `.inf` is `Infinity`,
/// and an unquoted timestamp is a `Date` (its `date: "%s"` is the epoch second).
#[test]
fn front_matter_scalars_resolve_like_js_yaml() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/index.liquid",
        "---
a: 0123
b: 08
c: 02134
d: 0x1F
e: 0o17
f: 1_000
g: .inf
h: -.inf
i: .nan
j: ~
k: null
l: yes
m: no
n: on
o: off
p: 1e3
q: 2020-01-02
r: 2020-01-02T03:04:05+02:00
s: 2020-01-02 03:04:05
t: '0123'
u: 2020-01-02T03:04:05.123456Z
w: 2020-13-45
x: 0b101
y: -0x1F
z: 0777
---
a={{ a }} b={{ b }} c={{ c }} d={{ d }} e={{ e }} f={{ f }} g={{ g }} h={{ h }} i={{ i }} j=[{{ j }}] k=[{{ k }}] l={{ l }} m={{ m }} n={{ n }} o={{ o }} p={{ p }}
q={{ q | date: \"%s\" }}
r={{ r | date: \"%s\" }}
s={{ s | date: \"%s\" }}
t={{ t }} u={{ u | date: \"%s %L\" }} w={{ w | date: \"%s\" }} x={{ x }} y={{ y }} z={{ z }}
a_size={{ a | size }} t_size={{ t | size }} q_size={{ q | size }}
",
    );
    assert_clean(&run(root));
    assert_eq!(
        read(root, "index.html"),
        "a=123 b=8 c=2134 d=31 e=15 f=1_000 g=Infinity h=-Infinity i=NaN j=[] k=[] l=yes m=no n=on o=off p=1000
q=1577923200
r=1577927045
s=1577934245
t=0123 u=1577934245 123 w=1613260800 x=5 y=-31 z=777
a_size=0 t_size=4 q_size=0
"
    );
}
