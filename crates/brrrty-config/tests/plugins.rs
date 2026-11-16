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
            "add_plugin(\"pwa\", ...): Brrrty doesn't have this plugin. It has `rss`, `syntaxhighlight`"
        ),
        "{p:?}"
    );
    assert_eq!(p.line, Some(2));
}

// ---- rss: `@11ty/eleventy-plugin-rss`'s `feedPlugin` ----
// Byte-for-byte output is pinned by `tests/fixtures/rss`, built with real
// 11ty; these pin the options and what is refused.

const FEED: &str = r#"config.add_plugin("rss", #{
    type: "atom",
    output_path: "/feed.xml",
    collection: #{ name: "posts", limit: 2 },
    metadata: #{ title: "T", base: "https://example.com/" },
});"#;

fn feed_err(options: &str) -> Problem {
    load_err(&format!("config.add_plugin(\"rss\", {options});\n"))
}

#[test]
fn feed_options_are_checked_like_11ty() {
    let cases = [
        (
            "#{}",
            "Missing `collection.name` option in feedPlugin from @11ty/eleventy-plugin-rss.",
        ),
        (
            "#{ collection: #{ name: 5 } }",
            "Only string is supported in `collection.name` option in feedPlugin from @11ty/eleventy-plugin-rss. Received: number",
        ),
        (
            "#{ collection: #{ name: \"p\", sort: \"up\" } }",
            "Invalid `collection.sort` option in feedPlugin from @11ty/eleventy-plugin-rss. Expected \"auto\", \"ascending\", or \"descending\", received: up",
        ),
        (
            "#{ type: \"xml\", collection: #{ name: \"p\" } }",
            "Missing or invalid feed type. Received: xml",
        ),
    ];
    for (options, want) in cases {
        let p = feed_err(options);
        assert_eq!(p.code, codes::CONFIG_INVALID, "{options}");
        assert_eq!(p.message, want, "{options}");
        assert_eq!(p.line, Some(1));
    }
}

#[test]
fn feed_options_brrrty_cant_follow_are_refused() {
    for (options, part) in [
        (
            "#{ collection: #{ name: \"p\" }, html_base_plugin_options: #{} }",
            "doesn't support the option `html_base_plugin_options`",
        ),
        (
            "#{ collection: #{ name: \"p\", limit: 2.5 } }",
            "`collection.limit` is 2.5",
        ),
        (
            "#{ type: \"rss\", collection: #{ name: \"my-posts\" } }",
            "the collection name `my-posts`",
        ),
        (
            "#{ collection: #{ name: \"it's\" } }",
            "the collection name `it's`",
        ),
        (
            "#{ collection: #{ name: \"p\" }, stylesheet: \"{{ x }}\" }",
            "`stylesheet` has Nunjucks tags",
        ),
        (
            "#{ collection: #{ name: \"p\" }, input_path: \"feed.liquid\" }",
            "`input_path` is `feed.liquid`",
        ),
        (
            "#{ collection: #{ name: \"p\" }, input_path: \"../feed.njk\" }",
            "`input_path` is `../feed.njk`",
        ),
    ] {
        let p = feed_err(options);
        assert_eq!(p.code, codes::CONFIG_INVALID, "{options}");
        assert!(p.message.contains(part), "{options}: {}", p.message);
    }
    // Two feeds with one virtual path: 11ty's conflict.
    let p = load_err(&format!("{FEED}\n{FEED}\n"));
    assert!(p.message.starts_with("Virtual template conflict"), "{p:?}");
    assert_eq!(p.line, Some(7));
}

fn posts(more: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    let mut v = vec![
        (
            "posts/2024-01-01-a.md",
            "---\ntags: posts\ntitle: A\n---\n[x](x/)\n",
        ),
        (
            "posts/2024-01-02-b.md",
            "---\ntags: posts\ntitle: B\n---\nB\n",
        ),
    ];
    v.extend_from_slice(more);
    v
}

#[test]
fn a_feed_builds_with_full_urls() {
    let (t, r) = site(FEED, &posts(&[]));
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    let feed = out(&t, "feed.xml").unwrap();
    assert!(
        feed.contains("<link href=\"https://example.com/feed.xml\" rel=\"self\" />"),
        "{feed}"
    );
    assert!(
        feed.contains("&lt;a href=&quot;https://example.com/posts/2024-01-01-a/x/&quot;&gt;"),
        "{feed}"
    );
    assert!(
        feed.find("<title>B</title>") < feed.find("<title>A</title>"),
        "{feed}"
    );
}

#[test]
fn a_feed_of_a_refused_post_is_refused() {
    let script = format!("{MARKER}\n{FEED}");
    let (t, r) = site(
        &script,
        &posts(&[(
            "posts/2024-01-03-c.md",
            "---\ntags: posts\n---\n```js\nx\n```\n",
        )]),
    );
    let errs = errors(&r);
    let feed = errs
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/eleventy-plugin-feed-t-atom.njk")))
        .expect("the feed is refused");
    assert_eq!(feed.code, codes::CONTENT_UNAVAILABLE);
    assert_eq!(out(&t, "feed.xml"), None);
    assert!(out(&t, "posts/2024-01-01-a/index.html").is_some());
}

#[test]
fn a_feed_of_a_missing_collection_is_refused() {
    let (t, r) = site(
        r#"config.add_plugin("rss", #{ collection: #{ name: "nope" } });"#,
        &posts(&[]),
    );
    let errs = errors(&r);
    // 11ty stops the build here (`undefined.filter`); a name no tag or
    // config collection has may be one the 11ty config adds, so Brrrty
    // can't build the collections at all.
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert_eq!(errs[0].code, codes::DATA_UNSUPPORTED);
    assert!(errs[0].message.contains("imports `nope`"), "{:?}", errs[0]);
    assert_eq!(out(&t, "feed.xml"), None);
}

#[test]
fn a_feed_without_a_full_base_is_refused() {
    let (_t, r) = site(
        r#"config.add_plugin("rss", #{ collection: #{ name: "posts" }, metadata: #{ base: "/" } });"#,
        &posts(&[]),
    );
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert_eq!(errs[0].code, codes::FEED_FAILED);
    assert!(errs[0].message.contains("`metadata.base`"), "{:?}", errs[0]);
}

#[test]
fn a_feed_on_a_files_path_is_refused() {
    let (t, r) = site(FEED, &posts(&[("eleventy-plugin-feed-t-atom.njk", "x")]));
    assert!(
        errors(&r).iter().any(|p| p.code == codes::FEED_UNSUPPORTED),
        "{:#?}",
        r.problems
    );
    assert_eq!(out(&t, "feed.xml"), None);
}

/// `dateToRfc3339` and the like are Nunjucks filters: Liquid doesn't have
/// them in 11ty either. The universal ones work.
#[test]
fn only_the_universal_filters_reach_liquid() {
    let (t, r) = site(
        FEED,
        &posts(&[
            ("a.liquid", "{{ page.date | dateToRfc3339 }}"),
            (
                "b.liquid",
                "{{ '/x/' | htmlBaseUrl: 'https://example.com/s/' }} {{ collections.posts | eleventyFeedHead: -1 | size }}",
            ),
        ]),
    );
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert_eq!(errs[0].code, codes::LIQUID_UNKNOWN_FILTER);
    assert_eq!(out(&t, "b/index.html").unwrap(), "https://example.com/x/ 1");
}
