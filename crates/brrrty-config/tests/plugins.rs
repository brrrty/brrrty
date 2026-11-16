//! `config.add_plugin(...)`: the plugins Brrrty has its own version of.
//!
//! `syntaxhighlight` stands for `@11ty/eleventy-plugin-syntaxhighlight`.
//! Brrrty doesn't highlight yet, so the marker refuses every page 11ty would
//! highlight: a fenced code block that names a language (the plugin hands a
//! fence with no language back to markdown-it, so that one is plain), and
//! the plugin's `{% highlight %}` tag. Pages without them build.

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

const MARKER: &str = r#"config.add_plugin("syntaxhighlight");"#;

/// The pages of a small blog: code in a language, code without one, no
/// code, and a page that reads the first page's content.
fn pages() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "posts/code.md",
            "---\ntags: posts\n---\n# Code\n\nText.\n\n```js\nlet a = 1;\n```\n",
        ),
        ("posts/plain.md", "---\ntags: posts\n---\n```\nplain\n```\n"),
        (
            "posts/indented.md",
            "---\ntags: posts\n---\nText\n\n    not code in 11ty\n",
        ),
        ("about.md", "# About\n"),
        (
            "list.liquid",
            "{% for p in collections.posts %}{% if p.url == '/posts/code/' %}{{ p.templateContent }}{% endif %}{% endfor %}",
        ),
    ]
}

#[test]
fn the_marker_refuses_highlighted_code_and_nothing_else() {
    let (t, r) = site(MARKER, &pages());
    let errs = errors(&r);
    let code = errs
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/posts/code.md")))
        .expect("the page with code is refused");
    assert_eq!(code.code, codes::SYNTAX_HIGHLIGHT_UNSUPPORTED);
    assert_eq!(
        code.message,
        "this page has a fenced code block in `js`, which 11ty's syntax highlighting plugin highlights"
    );
    assert!(
        code.help
            .as_deref()
            .unwrap()
            .starts_with("syntax highlighting is coming soon"),
        "{code:?}"
    );
    // Line 8 of the file: three lines of front matter, then the body.
    assert_eq!((code.line, code.column), (Some(8), Some(1)));
    // The page that reads its content can't be built either.
    let list = errs
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/list.liquid")))
        .expect("the reader is refused");
    assert_eq!(list.code, codes::CONTENT_UNAVAILABLE);
    assert_eq!(errs.len(), 2, "{errs:#?}");
    assert_eq!(out(&t, "posts/code/index.html"), None);
    assert_eq!(out(&t, "list/index.html"), None);
    // A fence with no language is plain in 11ty too; indented lines are
    // a paragraph (11ty turns markdown-it's `code` rule off).
    assert_eq!(
        out(&t, "posts/plain/index.html").unwrap(),
        "<pre><code>plain\n</code></pre>\n"
    );
    assert_eq!(
        out(&t, "posts/indented/index.html").unwrap(),
        "<p>Text</p>\n<p>not code in 11ty</p>\n"
    );
    assert_eq!(out(&t, "about/index.html").unwrap(), "<h1>About</h1>\n");
}

/// Without the marker Brrrty can't know the 11ty config highlights code,
/// so fenced code is written plain. PORTING.md says to add the marker.
#[test]
fn without_the_marker_code_is_plain() {
    let (t, r) = site("", &pages());
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        out(&t, "posts/code/index.html").unwrap(),
        "<h1>Code</h1>\n<p>Text.</p>\n<pre><code class=\"language-js\">let a = 1;\n</code></pre>\n"
    );
}

#[test]
fn a_line_is_given_only_when_liquid_changed_nothing() {
    let (_t, r) = site(
        MARKER,
        &[("a.md", "---\nx: 1\n---\n{{ x }}\n\n~~~ruby\nputs 1\n~~~\n")],
    );
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert_eq!(errs[0].code, codes::SYNTAX_HIGHLIGHT_UNSUPPORTED);
    assert!(errs[0].message.contains("`ruby`"), "{:?}", errs[0]);
    assert_eq!(errs[0].line, None);
}

#[test]
fn the_highlight_tag_is_refused_neutrally() {
    let (t, r) = site(
        MARKER,
        &[
            ("a.liquid", "{% highlight js %}let a;{% endhighlight %}"),
            ("b.liquid", "fine"),
        ],
    );
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert_eq!(errs[0].code, codes::SYNTAX_HIGHLIGHT_UNSUPPORTED);
    assert_eq!(
        errs[0].message,
        "this page uses `{% highlight %}`, the syntax highlighting plugin's tag"
    );
    assert_eq!(out(&t, "b/index.html").unwrap(), "fine");
    // Without the plugin 11ty doesn't know the tag either.
    let (_t, r) = site("", &[("a.liquid", "{% highlight js %}x{% endhighlight %}")]);
    assert_eq!(errors(&r)[0].code, codes::LIQUID_SYNTAX);
}

#[test]
fn the_marker_takes_the_plugins_options() {
    let (_t, r) = site(
        r#"config.add_plugin("syntaxhighlight", #{ preAttributes: #{ tabindex: 0 } });"#,
        &[("a.md", "```js\nx\n```\n")],
    );
    assert_eq!(errors(&r)[0].code, codes::SYNTAX_HIGHLIGHT_UNSUPPORTED);
}

#[test]
fn an_unknown_plugin_is_refused_where_it_is_added() {
    let p = load_err("let x = 1;\nconfig.add_plugin(\"pwa\");\n");
    assert_eq!(p.code, codes::CONFIG_INVALID);
    assert!(
        p.message.starts_with(
            "add_plugin(\"pwa\", ...): Brrrty doesn't have this plugin. It has `syntaxhighlight`"
        ),
        "{p:?}"
    );
    assert_eq!(p.line, Some(2));
}
