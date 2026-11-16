//! Templated permalinks through `build()`. Every expectation was probed with
//! real 11ty 3.1.8; the fixture `tests/fixtures/permalinks` pins the output
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

/// A site with one page `src/bad.liquid` whose front matter is `front`,
/// next to a page that builds.
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
    p
}

/// Probed: a permalink renders with the full data, from front matter,
/// directory data and global data; `permalink: false` in a page beats a
/// directory's permalink.
#[test]
fn permalinks_render_from_every_level_of_the_cascade() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/glob.json", r#"{"base": "/g"}"#);
    write(
        root,
        "src/posts/posts.json",
        r#"{"permalink": "/blog/{{ page.date | date: '%Y/%m/%d' }}/{{ page.fileSlug }}/index.html", "site": "S"}"#,
    );
    write(
        root,
        "src/posts/one.md",
        "---\ndate: 2024-03-05T12:00:00Z\n---\nx {{ page.url }} {{ page.outputPath }}\n",
    );
    write(
        root,
        "src/posts/two.md",
        "---\ndate: 2024-03-06T12:00:00Z\npermalink: false\n---\ntwo\n",
    );
    write(
        root,
        "src/posts/three.md",
        "---\npermalink: \"/over/{{ site }}.html\"\n---\nthree\n",
    );
    write(
        root,
        "src/gl.md",
        "---\npermalink: \"{{ glob.base }}/x.html\"\n---\nglobal\n",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:?}", r.problems);
    assert_eq!(
        read(root, "blog/2024/03/05/one/index.html"),
        "<p>x /blog/2024/03/05/one/ ./_site/blog/2024/03/05/one/index.html</p>\n"
    );
    assert!(!exists(root, "blog/2024/03/06/two/index.html"));
    assert_eq!(read(root, "over/S.html"), "<p>three</p>\n");
    assert_eq!(read(root, "g/x.html"), "<p>global</p>\n");
}

/// Probed: no trimming. Whitespace and the newline a block scalar leaves
/// stay in the file name.
#[test]
fn whitespace_in_the_rendered_permalink_is_kept() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/f.liquid",
        "---\npermalink: |\n  /f/{{ \"x\" }}.html\n---\nf",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:?}", r.problems);
    assert_eq!(read(root, "f/x.html\n"), "f");
}

/// Probed: 11ty crashes writing a permalink that renders to an empty
/// string (`TypeError ... Received type boolean (false)`), and an
/// extensionless one like `false` hits the no-extension error. Brrrty
/// reports the page.
#[test]
fn a_permalink_that_renders_empty_is_an_error() {
    for template in ["{{ '' }}", "{{ nothing }}", "{% if false %}/h/{% endif %}"] {
        let p = assert_refused(
            &format!("permalink: \"{template}\"\n"),
            codes::PERMALINK_INVALID,
            "empty",
        );
        assert!(p.help.as_deref().unwrap().contains("permalink: false"));
    }
    let p = assert_refused(
        "permalink: \"{{ 'false' }}\"\n",
        codes::PERMALINK_NO_EXTENSION,
        "`false`",
    );
    assert!(p.line.is_none() || p.line == Some(2));
}

/// Plan 1's rules run on the rendered path: `..` is clamped, a rendered
/// file with no extension is refused, two pages rendering to one file collide.
#[test]
fn rendered_permalinks_follow_the_permalink_rules() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/esc.liquid",
        "---\npermalink: \"/{{ '../..' }}/esc/x.html\"\n---\nesc",
    );
    write(
        root,
        "src/noext.liquid",
        "---\ntitle: noext\npermalink: \"/{{ title }}\"\n---\nx",
    );
    write(
        root,
        "src/allow.liquid",
        "---\ntitle: allow\neleventyAllowMissingExtension: true\npermalink: \"/{{ title }}\"\n---\nallowed",
    );
    write(
        root,
        "src/a.liquid",
        "---\npermalink: \"/{{ 'dup' }}/\"\n---\na",
    );
    write(root, "src/b.liquid", "---\npermalink: /dup/\n---\nb");
    let r = run(root);
    assert_eq!(read(root, "esc/x.html"), "esc");
    assert_eq!(read(root, "allow"), "allowed");
    let mut found: Vec<(&str, PathBuf)> = errors(&r)
        .iter()
        .map(|p| (p.code, p.path.clone().unwrap()))
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            (codes::OUTPUT_COLLISION, PathBuf::from("src/a.liquid")),
            (
                codes::PERMALINK_NO_EXTENSION,
                PathBuf::from("src/noext.liquid")
            ),
        ]
    );
    assert!(!exists(root, "dup/index.html"));
}

/// Probed: a Liquid error in a permalink aborts 11ty. Brrrty reports the
/// page, at the `permalink:` line.
#[test]
fn a_liquid_error_in_a_permalink_names_the_line() {
    let p = assert_refused(
        "title: x\npermalink: \"/{% if %}/\"\n",
        codes::LIQUID_SYNTAX,
        "permalink",
    );
    assert_eq!(p.line, Some(3));
    let p = assert_refused(
        "title: x\npermalink: \"/{{ title | nosuchfilter }}.html\"\n",
        codes::LIQUID_UNKNOWN_FILTER,
        "permalink",
    );
    assert_eq!(p.line, Some(3));
}

/// A permalink from a data file names the file; the line is the file's, so
/// none is given.
#[test]
fn a_liquid_error_in_a_data_file_permalink_names_the_file() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/p/p.json",
        r#"{"permalink": "/{{ x | nosuchfilter }}/"}"#,
    );
    write(root, "src/p/a.liquid", "a");
    write(root, "src/ok.liquid", "ok");
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::LIQUID_UNKNOWN_FILTER);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/p/a.liquid")));
    assert!(p.message.contains("src/p/p.json"), "{}", p.message);
    assert_eq!(p.line, None);
}

/// Probed: 11ty's own filters work in a permalink. A filter only a config
/// file adds (here `utcDate`) is refused: 11ty aborts on an unknown filter,
/// and Brrrty can't know the config's filters until the Rhai config.
#[test]
fn config_filters_in_a_permalink_are_refused_with_a_pointer_to_the_config() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/s.liquid",
        "---\ntitle: Some Title!\npermalink: \"{{ title | slugify }}/{{ title | url }}/u.html\"\n---\ns",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:?}", r.problems);
    assert_eq!(read(root, "some-title/Some Title!/u.html"), "s");

    let p = assert_refused(
        "date: 2024-03-05T12:00:00Z\npermalink: \"/{{ page.date | utcDate: 'yyyy' }}/x.html\"\n",
        codes::LIQUID_UNKNOWN_FILTER,
        "utcDate",
    );
    let help = p.help.unwrap();
    assert!(help.contains("Rhai config"), "{help}");
    assert!(help.contains("custom filters"), "{help}");
}

/// Probed: `templateEngineOverride` of `html`, `false` or `""` writes the
/// permalink as written; `md`, `liquid` and the pairs render it;
/// `dynamicPermalink: false` also writes it as written.
#[test]
fn engine_overrides_and_dynamic_permalink_decide_whether_it_renders() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    for (name, over) in [
        ("raw1", "html"),
        ("raw2", "false"),
        ("raw3", "\"\""),
        ("md1", "md"),
        ("lq1", "liquid"),
        ("lq2", "\"liquid,md\""),
        ("lq3", "\"md,liquid\""),
    ] {
        write(
            root,
            &format!("src/{name}.md"),
            &format!(
                "---\ntitle: T\ntemplateEngineOverride: {over}\npermalink: \"/{name}/{{{{ title }}}}.html\"\n---\nx"
            ),
        );
    }
    write(
        root,
        "src/dyn.md",
        "---\ntitle: T\ndynamicPermalink: false\npermalink: \"/dyn/{{ title }}.html\"\n---\nx",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:?}", r.problems);
    for raw in ["raw1", "raw2", "raw3", "dyn"] {
        assert!(
            exists(root, &format!("{raw}/{{{{ title }}}}.html")),
            "{raw}"
        );
    }
    for rendered in ["md1", "lq1", "lq2", "lq3"] {
        assert!(exists(root, &format!("{rendered}/T.html")), "{rendered}");
    }
}

/// Probed: a permalink may be a map. `build` is the path; a map without
/// `build` writes nothing; `{}` is the default. Every value renders.
#[test]
fn permalink_maps_use_build() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/o1.md",
        "---\npermalink:\n  build: \"/obj/{{ page.fileSlug }}.html\"\n  serve: \"/z/{{ page.fileSlug }}\"\n---\no1",
    );
    write(
        root,
        "src/o2.md",
        "---\npermalink:\n  serve: /s.html\n---\no2",
    );
    write(root, "src/o3.md", "---\npermalink: {}\n---\no3");
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:?}", r.problems);
    assert_eq!(read(root, "obj/o1.html"), "<p>o1</p>\n");
    assert!(!exists(root, "s.html"));
    assert!(!exists(root, "o2/index.html"));
    assert_eq!(read(root, "o3/index.html"), "<p>o3</p>\n");
}

/// Probed: `build: false` crashes 11ty (it writes a file named `false`).
/// Other shapes of map Brrrty refuses rather than guess.
#[test]
fn permalink_maps_of_other_shapes_are_refused() {
    assert_refused(
        "permalink:\n  build: false\n",
        codes::PERMALINK_UNSUPPORTED,
        "map",
    );
    assert_refused(
        "permalink:\n  build:\n    - /a.html\n",
        codes::PERMALINK_UNSUPPORTED,
        "map",
    );
    assert_refused(
        "permalink:\n  build: \"{{ '' }}\"\n",
        codes::PERMALINK_INVALID,
        "empty",
    );
}

/// Probed: `permalinkBypassOutputDir` makes 11ty write outside the output
/// folder (`Writing /byp/T.html`). Brrrty never does.
#[test]
fn bypassing_the_output_folder_is_refused() {
    let p = assert_refused(
        "permalinkBypassOutputDir: true\npermalink: \"/byp/x.html\"\n",
        codes::PERMALINK_UNSUPPORTED,
        "permalinkBypassOutputDir",
    );
    assert!(p.help.unwrap().contains("output folder"));
}

/// Probed: a template that reads its own permalink gets the raw template,
/// and `page.url` and `page.outputPath` are empty while it renders.
#[test]
fn a_permalink_reads_its_own_template_and_no_url_yet() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    let template = "/s/{{ page.url }}|{{ permalink | size }}|{{ page.outputPath }}|{{ page.filePathStem }}-x.html";
    write(
        root,
        "src/self.liquid",
        &format!("---\npermalink: \"{template}\"\n---\nbody [{{{{ permalink }}}}]"),
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:?}", r.problems);
    assert_eq!(
        read(root, &format!("s/|{}||/self-x.html", template.len())),
        format!("body [{template}]")
    );
}

/// Probed: with `eleventyComputed`, `page.url` and `page.outputPath` are
/// entries that run first, so a plain permalink reading a computed key sees
/// the key's original value; a computed `permalink` runs in dependency
/// order, and a computed value may read `page.url`.
#[test]
fn computed_data_and_permalinks_run_in_11tys_order() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    let page = |name: &str, front: &str, body: &str| {
        write(
            root,
            &format!("src/{name}.liquid"),
            &format!("---\n{front}---\n{body}"),
        );
    };
    page(
        "c1",
        "title: A B\npermalink: \"/c1/{{ slug }}-x.html\"\neleventyComputed:\n  slug: \"{{ title | slug }}\"\n",
        "c1 {{ slug }}",
    );
    page(
        "c1b",
        "title: A B\nslug: orig\npermalink: \"/c1b/{{ slug }}-x.html\"\neleventyComputed:\n  slug: \"{{ title | slug }}\"\n",
        "c1b {{ slug }}",
    );
    page(
        "c2",
        "permalink: /c2/x.html\neleventyComputed:\n  u: \"{{ page.url }}\"\n  o: \"{{ page.outputPath }}\"\n",
        "c2 {{ u }} {{ o }}",
    );
    page(
        "c3",
        "title: T3\neleventyComputed:\n  permalink: \"/c3/{{ title }}.html\"\n",
        "c3 {{ permalink }} {{ page.url }}",
    );
    page(
        "c5",
        "title: A B\nslug: orig\neleventyComputed:\n  permalink: \"/c5/{{ slug }}.html\"\n  slug: \"{{ title | slug }}\"\n",
        "c5 {{ permalink }} {{ slug }}",
    );
    page(
        "c6",
        "title: C6\npermalink: \"/c6/{{ title }}.html\"\neleventyComputed:\n  permalink: \"/c6/comp-{{ title }}.html\"\n",
        "c6",
    );
    page(
        "c7",
        "title: C7\neleventyComputed:\n  permalink: false\n",
        "c7",
    );
    page(
        "c9",
        "title: C9\npermalink: \"/c9/{{ u }}-x.html\"\neleventyComputed:\n  u: \"{{ page.url }}\"\n",
        "c9 {{ u }}",
    );
    page(
        "c10",
        "title: A B\neleventyComputed:\n  u: \"{{ page.url }}\"\n  permalink: \"/c10/{{ slug }}-x.html\"\n  slug: \"{{ title | slug }}\"\n",
        "c10 {{ u }} {{ permalink }}",
    );
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:?}", r.problems);
    assert_eq!(read(root, "c1/-x.html"), "c1 a-b");
    assert_eq!(read(root, "c1b/orig-x.html"), "c1b a-b");
    assert_eq!(read(root, "c2/x.html"), "c2 /c2/x.html ./_site/c2/x.html");
    assert_eq!(read(root, "c3/T3.html"), "c3 /c3/T3.html /c3/T3.html");
    assert_eq!(read(root, "c5/a-b.html"), "c5 /c5/a-b.html a-b");
    assert_eq!(read(root, "c6/comp-C6.html"), "c6");
    assert!(!exists(root, "c7/index.html"));
    assert_eq!(read(root, "c9/-x.html"), "c9 /c9/-x.html");
    assert_eq!(
        read(root, "c10/a-b-x.html"),
        "c10 /c10/a-b-x.html /c10/a-b-x.html"
    );
}

/// Probed: a computed `permalink` that reads `page.url` aborts 11ty with
/// "Dependency Cycle Found: page.url -> permalink -> page.url".
#[test]
fn a_computed_permalink_reading_page_url_is_a_cycle() {
    let p = assert_refused(
        "eleventyComputed:\n  permalink: \"/c8/{{ page.url }}.html\"\n",
        codes::COMPUTED_CYCLE,
        "Dependency Cycle Found: page.url -> permalink -> page.url",
    );
    assert!(p.help.is_some());
}

/// A computed `permalink` map isn't followed.
#[test]
fn a_computed_permalink_map_is_refused() {
    assert_refused(
        "eleventyComputed:\n  permalink:\n    build: /x.html\n",
        codes::COMPUTED_UNSUPPORTED,
        "permalink",
    );
}

/// A computed value that errors on a page with `permalink: false` still
/// stops that page, as in 11ty.
#[test]
fn computed_errors_surface_on_pages_that_are_not_written() {
    assert_refused(
        "permalink: false\neleventyComputed:\n  a: \"{{ 1 | nosuchfilter }}\"\n",
        codes::LIQUID_UNKNOWN_FILTER,
        "eleventyComputed.a",
    );
}

/// A layout's data and a permalink from a layout's front matter render too.
#[test]
fn a_layouts_permalink_renders_for_its_pages() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/_includes/l.liquid",
        "---\npermalink: \"/lay/{{ page.fileSlug }}.html\"\n---\n[{{ content }}]",
    );
    write(root, "src/a.liquid", "---\nlayout: l\n---\nA");
    let r = run(root);
    assert!(errors(&r).is_empty(), "{:?}", r.problems);
    assert_eq!(read(root, "lay/a.html"), "[A]");
}

/// Reads of a JavaScript data key in a permalink are caught like any read.
#[test]
fn a_permalink_reading_a_js_data_key_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_data/site.js", "module.exports = { slug: 's' };");
    write(
        root,
        "src/a.liquid",
        "---\npermalink: \"/{{ site.slug }}/\"\n---\nA",
    );
    write(root, "src/ok.liquid", "ok");
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::DATA_JS_UNSUPPORTED);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/a.liquid")));
    assert!(!exists(root, "s/index.html"));
    assert_eq!(read(root, "ok/index.html"), "ok");
}
