//! Nunjucks templates, layouts, includes, permalinks and computed data
//! through `build()`. Expected outputs were probed with real 11ty 3.1.8
//! (nunjucks 3.2.4).

use brrrty_core::{
    BuildOptions, BuildReport, ConfigOptions, Problem, Severity, TemplateEngines, TemplateLanguage,
    build, codes,
};
use brrrty_markdown::MarkdownIt;
use std::fs;
use std::path::{Path, PathBuf};

fn opts(root: &Path, config: ConfigOptions) -> BuildOptions {
    BuildOptions {
        root: root.to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        passthrough: Vec::new(),
        strict: false,
        config,
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
    build(&opts(root, Default::default()), &MarkdownIt::new())
}

/// A build whose config sets `markdownTemplateEngine` and
/// `htmlTemplateEngine` to `njk`.
fn run_njk(root: &Path) -> BuildReport {
    let config = ConfigOptions {
        template_engines: TemplateEngines {
            markdown: Some(TemplateLanguage::Nunjucks),
            html: Some(TemplateLanguage::Nunjucks),
        },
        ..Default::default()
    };
    build(&opts(root, config), &MarkdownIt::new())
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

/// Probed: includes are looked up in `_includes`, then the project folder
/// (not the input folder); `./` is relative to the page. An included file
/// of any type is a Nunjucks template. `title` is escaped; `range`,
/// `cycler` and `joiner` are Nunjucks's globals, and 11ty adds none.
#[test]
fn pages_include_from_includes_then_the_project_folder() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "rootonly.txt", "ROOT-FILE {{ 1 + 2 }}\n");
    write(root, "src/posts/rel.txt", "REL {{ page.url }}\n");
    write(root, "src/_includes/sub/x.njk", "INC-SUB\n");
    write(root, "src/_includes/lib.js", "var a = {b: 1};\n");
    write(
        root,
        "src/a.njk",
        concat!(
            "---\ntitle: A <b>\n---\n",
            "{% include \"rootonly.txt\" %}\n",
            "{% include \"sub/x.njk\" %}\n",
            "{% include \"./posts/rel.txt\" %}\n",
            "{% include \"lib.js\" %}\n",
            "{{ title }}\n",
            "{{ range(3) | join(\",\") }}\n",
            "{{ cycler is defined }} {{ joiner is defined }} {{ lipsum is defined }}\n",
            "{{ ctx is defined }}{{ getContext is defined }}\n",
            "{{ page.templateSyntax }}\n",
        ),
    );
    let r = run(root);
    assert_clean(&r);
    assert_eq!(
        read(root, "a/index.html"),
        concat!(
            "ROOT-FILE 3\n\nINC-SUB\n\nREL /a/\n\nvar a = {b: 1};\n\n",
            "A &lt;b&gt;\n0,1,2\ntrue true false\nfalsefalse\nnjk\n"
        )
    );
}

/// Probed: a file only in the input folder is not found, with Nunjucks's
/// message.
#[test]
fn includes_do_not_look_in_the_input_folder() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/inputonly.txt", "X");
    write(root, "src/a.njk", "{% include \"inputonly.txt\" %}\n");
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::NUNJUCKS_RENDER);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/a.njk")));
    assert!(
        p.message
            .contains("Error: template not found: inputonly.txt"),
        "{p:?}"
    );
    assert!(!root.join("_site/a/index.html").exists());
}

/// Probed: Nunjucks layouts escape `content` unless it is marked `safe`,
/// and layout chains mix languages freely.
#[test]
fn layouts_mix_languages() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/_includes/base.njk",
        "---\nlayoutkey: fromlayout\n---\n<html>{{ content }}|{{ content | safe }}|{{ title }}|{{ layoutkey }}</html>\n",
    );
    write(
        root,
        "src/_includes/mid.liquid",
        "---\nlayout: base.njk\n---\n<inner>{{ content }}</inner>\n",
    );
    write(
        root,
        "src/d.liquid",
        "---\nlayout: base.njk\ntitle: D & D\n---\nliquid {{ title }}\n",
    );
    write(
        root,
        "src/e.njk",
        "---\nlayout: mid.liquid\ntitle: E\n---\nnjk in liquid {{ title }}\n",
    );
    let r = run(root);
    assert_clean(&r);
    assert_eq!(
        read(root, "d/index.html"),
        "<html>liquid D &amp; D\n|liquid D & D\n|D &amp; D|fromlayout</html>\n"
    );
    assert_eq!(
        read(root, "e/index.html"),
        "<html>&lt;inner&gt;njk in liquid E\n&lt;/inner&gt;\n|<inner>njk in liquid E\n</inner>\n|E|fromlayout</html>\n"
    );
}

/// Probed: with `markdownTemplateEngine: "njk"`, Nunjucks runs before
/// Markdown, on pages and on `.md` layouts; `htmlTemplateEngine: "njk"`
/// runs Nunjucks on `.html` pages and layouts.
#[test]
fn markdown_and_html_can_run_nunjucks() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/_includes/l.md",
        "---\nk: md-layout\n---\n# MDL {{ k }} {{ content | safe }}\n",
    );
    write(
        root,
        "src/_includes/h.html",
        "---\nlayout: l.md\n---\n<h>{{ content | safe }}{% raw %}{{x}}{% endraw %}</h>\n",
    );
    write(
        root,
        "src/a.njk",
        "---\ntitle: A\npermalink: \"/{{ title | lower }}/{# c #}x.html\"\nlayout: h.html\n---\nA {{ page.url }}\n",
    );
    write(
        root,
        "src/b.md",
        "---\ntitle: B\n---\n# Hello {{ title }} {% if true %}*yes*{% endif %}\n\n{{ page.templateSyntax }}\n",
    );
    write(
        root,
        "src/c.html",
        "---\ntitle: C\n---\n{{ page.templateSyntax }} {{ title | upper }}\n",
    );
    let r = run_njk(root);
    assert_clean(&r);
    assert_eq!(
        read(root, "a/x.html"),
        "<h1>MDL md-layout <h>A /a/x.html</h1>\n<p>{{x}}</h></p>\n"
    );
    assert_eq!(
        read(root, "b/index.html"),
        "<h1>Hello B <em>yes</em></h1>\n<p>njk,md</p>\n"
    );
    assert_eq!(read(root, "c/index.html"), "njk C\n");
}

/// Probed: a permalink renders with the language 11ty compiles it with.
/// For a Markdown page with a `templateEngineOverride` that includes `md`,
/// that is the config's `markdownTemplateEngine`, not the override's
/// language. A `.liquid` page checks for Liquid's delimiters first, so
/// a Nunjucks comment alone is written as it is.
#[test]
fn permalinks_render_with_the_page_language() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/b.md",
        "---\ntitle: B\ntemplateEngineOverride: liquid,md\npermalink: \"/{{ title | lower }}/\"\n---\n*{{ title | upcase }}* {{ page.templateSyntax }}\n",
    );
    write(
        root,
        "src/c.md",
        "---\ntitle: C\ntemplateEngineOverride: njk\n---\n*{{ title | upper }}* {{ page.templateSyntax }}\n",
    );
    write(
        root,
        "src/d.liquid",
        "---\ntitle: D\ntemplateEngineOverride: njk,md\npermalink: \"/d{# x #}/\"\n---\n*{{ title | upper }}* {{ page.templateSyntax }}\n",
    );
    write(
        root,
        "src/e.njk",
        "---\ntitle: E\ntemplateEngineOverride: md\n---\n*{{ title }}* {{ page.templateSyntax }}\n",
    );
    write(
        root,
        "src/f.njk",
        "---\ntitle: F\ndynamicPermalink: false\npermalink: \"/f{{x}}/index.html\"\n---\nF\n",
    );
    write(
        root,
        "src/g.njk",
        "---\ntitle: G\neleventyComputed:\n  permalink: \"/g-{{ title | lower }}/\"\n  x: \"{{ page.url }}!\"\n---\n{{ x }}\n",
    );
    let r = run_njk(root);
    assert_clean(&r);
    assert_eq!(read(root, "b/index.html"), "<p><em>B</em> liquid,md</p>\n");
    assert_eq!(read(root, "c/index.html"), "*C* njk\n");
    assert_eq!(
        read(root, "d{# x #}/index.html"),
        "<p><em>D</em> njk,md</p>\n"
    );
    assert_eq!(
        read(root, "e/index.html"),
        "<p><em>{{ title }}</em> {{ page.templateSyntax }}</p>\n"
    );
    assert_eq!(read(root, "f{{x}}/index.html"), "F\n");
    assert_eq!(read(root, "g-g/index.html"), "/g-g/!\n");
}

/// Probed: computed values in `.html` with `htmlTemplateEngine: "njk"`
/// run as Nunjucks, in the order their symbols give.
#[test]
fn computed_values_run_as_nunjucks() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/c.html",
        concat!(
            "---\ntitle: C\npermalink: \"/c/{{ title | slugify }}/\"\n",
            "eleventyComputed:\n  bar: \"{{ foo }}-y\"\n  foo: \"{{ title }}-x\"\n---\n",
            "{{ page.templateSyntax }} {{ foo }} {{ bar }} {{ page.url }}\n"
        ),
    );
    let r = run_njk(root);
    assert_clean(&r);
    assert_eq!(read(root, "c/c/index.html"), "njk C-x C-x-y /c/c/\n");
}

/// A Nunjucks page is a collection item like any other, and reads the
/// collections.
#[test]
fn nunjucks_pages_read_and_join_collections() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/a.njk",
        "---\ntitle: A <b>\ntags: post\ndate: 2026-01-01\n---\n{{ collections.post | length }}|{% for p in collections.all %}{{ p.url }} {{ p.data.title }};{% endfor %}\n",
    );
    write(
        root,
        "src/b.md",
        "---\ntitle: B\ntags: post\ndate: 2026-01-02\n---\nB\n",
    );
    let r = run(root);
    assert_clean(&r);
    assert_eq!(read(root, "a/index.html"), "2|/a/ A &lt;b&gt;;/b/ B;\n");
}

/// An error in an included template names that file, with its line.
#[test]
fn errors_in_includes_name_the_include() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/_includes/bad.njk", "ok\n{{ x | nope }}\n");
    write(
        root,
        "src/a.njk",
        "---\ntitle: A\n---\n{% include \"bad.njk\" %}\n",
    );
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/_includes/bad.njk")));
    assert_eq!(p.line, Some(2));
    assert!(p.message.contains("filter not found: nope"), "{p:?}");
    assert!(p.message.contains("while rendering src/a.njk"), "{p:?}");
}

/// An error in a page's own text is placed at its line in the file.
#[test]
fn errors_in_pages_are_placed_in_the_file() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.njk", "---\ntitle: A\n---\nfine\n{% if %}\n");
    let r = run(root);
    let p = only_error(&r);
    assert_eq!(p.code, codes::NUNJUCKS_SYNTAX);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/a.njk")));
    assert_eq!(p.line, Some(5));
}

/// With 11ty's syntax highlighting plugin, `{% highlight %}` is its
/// paired shortcode, which Brrrty refuses as such until it highlights.
#[test]
fn the_highlight_tag_is_refused_as_the_plugin_tag() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/a.njk",
        "{% highlight \"js\" %}x{% endhighlight %}\n",
    );
    use brrrty_core::plugins::syntaxhighlight::{Options, SyntaxHighlight};
    let with = |formats: Option<Vec<String>>| ConfigOptions {
        syntax_highlight: Some(
            SyntaxHighlight::new(Options {
                template_formats: formats,
                ..Default::default()
            })
            .unwrap(),
        ),
        ..Default::default()
    };
    let r = build(&opts(root, with(None)), &MarkdownIt::new());
    let p = only_error(&r);
    assert_eq!(p.code, codes::SYNTAX_HIGHLIGHT_UNSUPPORTED);
    assert!(p.message.contains("in Nunjucks"), "{p:?}");
    // Without `njk` in its formats the plugin adds no Nunjucks tag, and
    // 11ty fails on the unknown tag too.
    let r = build(
        &opts(
            root,
            with(Some(vec!["md".to_string(), "liquid".to_string()])),
        ),
        &MarkdownIt::new(),
    );
    assert_eq!(only_error(&r).code, codes::NUNJUCKS_SYNTAX);
    let r = run(root);
    assert_eq!(only_error(&r).code, codes::NUNJUCKS_SYNTAX);
}

/// A config filter runs in Nunjucks as in Liquid.
#[test]
fn config_filters_run_in_nunjucks() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/a.njk",
        "{{ \"x\" | twice }}|{{ \"<\" | twice }}\n",
    );
    let config = ConfigOptions {
        filters: vec![brrrty_core::ConfigFilter {
            name: "twice".to_string(),
            run: std::sync::Arc::new(|v, _args, _ctx| match v {
                brrrty_js::Value::String(s) => {
                    Ok(brrrty_js::Value::String(format!("{s}{s}").into()))
                }
                other => Ok(other.clone()),
            }),
        }],
        ..Default::default()
    };
    let r = build(&opts(root, config), &MarkdownIt::new());
    assert_clean(&r);
    assert_eq!(read(root, "a/index.html"), "xx|&lt;&lt;\n");
}

/// A panic while Nunjucks renders a page refuses that page as a crash in
/// Brrrty's engine; the rest of the site still builds.
#[test]
fn a_panic_in_a_render_refuses_only_that_page() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.njk", "{{ \"x\" | boom }}\n");
    write(root, "src/b.njk", "fine\n");
    let config = ConfigOptions {
        filters: vec![brrrty_core::ConfigFilter {
            name: "boom".to_string(),
            run: std::sync::Arc::new(|_v, _args, _ctx| panic!("deliberate test panic")),
        }],
        ..Default::default()
    };
    let r = build(&opts(root, config), &MarkdownIt::new());
    let p = only_error(&r);
    assert_eq!(p.code, codes::NUNJUCKS_RENDER);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/a.njk")));
    assert!(
        p.message
            .contains("Brrrty's Nunjucks engine crashed: deliberate test panic"),
        "{p:?}"
    );
    assert!(p.help.as_deref().unwrap_or("").contains("bug in Brrrty"));
    assert_eq!(read(root, "b/index.html"), "fine\n");
    assert!(!root.join("_site/a/index.html").exists());
}

/// Probed with 11ty 3.1.8: `urlencode`, `dictsort` and `list` on a
/// collection item print its `template` as `[object Object]` and its
/// rendered content, and `dump` stops the build ("Converting circular
/// structure to JSON"). Brrrty doesn't have `template`, so each is refused
/// like `for k, v in item`. A count of the keys (13) and `in` need no
/// values, so they render.
#[test]
fn listing_a_collection_item_through_a_filter_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "---\ntitle: A\n---\n# A\n");
    let each = |body: &str| {
        format!(
            "{{% for p in collections.all %}}{{% if p.url == \"/a/\" %}}{body}{{% endif %}}{{% endfor %}}\n"
        )
    };
    write(root, "src/u.njk", each("{{ p | urlencode }}"));
    write(
        root,
        "src/d.njk",
        each("{% for k, v in p | dictsort %}{{ k }}={{ v }};{% endfor %}"),
    );
    write(root, "src/l.njk", each("{{ p | list | length }}"));
    write(root, "src/j.njk", each("{{ p | dump }}"));
    write(
        root,
        "src/k.njk",
        each("{{ p | length }};{{ \"content\" in p }};{{ \"nope\" in p }}"),
    );
    let r = run(root);
    let refused: Vec<_> = errors(&r).iter().map(|p| p.path.clone().unwrap()).collect();
    let want: Vec<PathBuf> = ["d", "j", "l", "u"]
        .iter()
        .map(|n| PathBuf::from(format!("src/{n}.njk")))
        .collect();
    assert_eq!(refused, want, "{:#?}", r.problems);
    for p in errors(&r) {
        assert!(
            p.message.contains("`template` of a collection item"),
            "{p:?}"
        );
    }
    for n in ["d", "j", "l", "u"] {
        assert!(!root.join(format!("_site/{n}/index.html")).exists(), "{n}");
    }
    assert_eq!(read(root, "k/index.html"), "13;true;false\n");
}

/// Probed with 11ty 3.1.8 and a config that adds the collection `foo`:
/// `"foo" in collections` is true, `collections | length` is 2, and
/// `dictsort` and `urlencode` list `foo`. Brrrty can't see the config's
/// collections, so each is refused instead of leaving `foo` out.
#[test]
fn counting_or_listing_collections_under_an_11ty_config_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "eleventy.config.js",
        "module.exports = c => { c.addCollection('foo', () => []); };\n",
    );
    write(root, "src/a.md", "# A\n");
    write(root, "src/in.njk", "{{ \"foo\" in collections }}\n");
    write(root, "src/len.njk", "{{ collections | length }}\n");
    write(
        root,
        "src/ds.njk",
        "{% for k, v in collections | dictsort %}{{ k }};{% endfor %}\n",
    );
    write(root, "src/ue.njk", "{{ collections | urlencode }}\n");
    write(root, "src/all.njk", "{{ \"all\" in collections }}\n");
    let r = run(root);
    let mut refused: Vec<_> = errors(&r).iter().map(|p| p.path.clone().unwrap()).collect();
    refused.sort();
    let want: Vec<PathBuf> = ["ds", "in", "len", "ue"]
        .iter()
        .map(|n| PathBuf::from(format!("src/{n}.njk")))
        .collect();
    assert_eq!(refused, want, "{:#?}", r.problems);
    assert_eq!(read(root, "all/index.html"), "true\n");
}
