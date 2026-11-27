//! `config.add_plugin(...)`: the plugins Brrrty has its own version of.
//!
//! `syntaxhighlight` is `@11ty/eleventy-plugin-syntaxhighlight`: fenced
//! code that names a language, and the `{% highlight %}` tag, highlighted
//! with Prism (tests/fixtures/highlight* compare whole sites with 11ty).
//! What Brrrty can't highlight the way 11ty does is refused, page by page.

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

/// `let a = 1;` as the plugin writes it in `js`.
const JS: &str = "<pre class=\"language-js\"><code class=\"language-js\"><span class=\"token keyword\">let</span> a <span class=\"token operator\">=</span> <span class=\"token number\">1</span><span class=\"token punctuation\">;</span></code></pre>\n";

#[test]
fn highlighted_code_builds_and_so_do_its_readers() {
    let (t, r) = site(MARKER, &pages());
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    let code = format!("<h1>Code</h1>\n<p>Text.</p>\n{JS}");
    assert_eq!(out(&t, "posts/code/index.html").unwrap(), code);
    assert_eq!(out(&t, "list/index.html").unwrap(), code);
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

/// Markdown with a fenced code block in a language: whether 11ty gives it
/// its language's class once or twice depends on whether YAML was loaded
/// before Markdown, which depends on the order pages render in.
const NESTED: &str = "````md\n```js\nx\n```\n````\n";

#[test]
fn code_that_cant_be_highlighted_exactly_is_refused_with_its_reader() {
    let mut pages = pages();
    pages.push((
        "posts/nested.md",
        "---\ntags: posts\n---\nText\n\n````md\n```js\nx\n```\n````\n",
    ));
    pages.push((
        "lists.liquid",
        "{% for p in collections.posts %}{{ p.content }}{% endfor %}",
    ));
    let (t, r) = site(MARKER, &pages);
    let errs = errors(&r);
    let nested = errs
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/posts/nested.md")))
        .expect("the page is refused");
    assert_eq!(nested.code, codes::SYNTAX_HIGHLIGHT_UNSUPPORTED);
    assert!(
        nested.message.starts_with(
            "the fenced code block in `md` can't be highlighted the way 11ty highlights it: it has Markdown with a fenced code block in a language"
        ),
        "{nested:?}"
    );
    // Line 6 of the file: three lines of front matter, then the body.
    assert_eq!((nested.line, nested.column), (Some(6), Some(1)));
    let lists = errs
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/lists.liquid")))
        .expect("the reader is refused");
    assert_eq!(lists.code, codes::CONTENT_UNAVAILABLE);
    assert_eq!(errs.len(), 2, "{errs:#?}");
    assert_eq!(out(&t, "posts/nested/index.html"), None);
    assert!(out(&t, "posts/code/index.html").is_some());
}

/// Without the plugin Brrrty can't know the 11ty config highlights code,
/// so fenced code is written plain. PORTING.md says to add it.
#[test]
fn without_the_plugin_code_is_plain() {
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
        &[("a.md", &format!("---\nx: 1\n---\n{{{{ x }}}}\n\n{NESTED}"))],
    );
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert_eq!(errs[0].code, codes::SYNTAX_HIGHLIGHT_UNSUPPORTED);
    assert_eq!(errs[0].line, None);
}

#[test]
fn languages_brrrty_lacks_are_refused() {
    let (_t, r) = site(MARKER, &[("a.md", "```haskell\nmain = pure ()\n```\n")]);
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert_eq!(errs[0].code, codes::SYNTAX_HIGHLIGHT_UNSUPPORTED);
    assert!(
        errs[0]
            .message
            .ends_with("Brrrty doesn't have Prism's `haskell` grammar yet"),
        "{:?}",
        errs[0]
    );
}

#[test]
fn the_highlight_tag_highlights() {
    let (t, r) = site(
        MARKER,
        &[
            (
                "a.liquid",
                "{% highlight js %}\nlet a = 1;\n{% endhighlight %}",
            ),
            (
                "b.liquid",
                &format!("x\n {{% highlight md %}}{NESTED}{{% endhighlight %}}"),
            ),
        ],
    );
    assert_eq!(out(&t, "a/index.html").unwrap(), JS.trim_end());
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert_eq!(errs[0].code, codes::SYNTAX_HIGHLIGHT_UNSUPPORTED);
    assert!(
        errs[0]
            .message
            .starts_with("this `{% highlight %}` can't be highlighted the way 11ty highlights it"),
        "{:?}",
        errs[0]
    );
    assert_eq!((errs[0].line, errs[0].column), (Some(2), Some(2)));
    // Without the plugin 11ty doesn't know the tag.
    let (_t, r) = site("", &[("a.liquid", "{% highlight js %}x{% endhighlight %}")]);
    assert_eq!(errors(&r)[0].code, codes::LIQUID_SYNTAX);
    // Nor when the plugin leaves Liquid out.
    let (_t, r) = site(
        r#"config.add_plugin("syntaxhighlight", #{ template_formats: "md" });"#,
        &[("a.liquid", "{% highlight js %}x{% endhighlight %}")],
    );
    assert_eq!(errors(&r)[0].code, codes::LIQUID_SYNTAX);
}

#[test]
fn where_the_plugin_throws_the_page_fails() {
    let (_t, r) = site(
        r#"config.add_plugin("syntaxhighlight", #{ error_on_invalid_language: true });"#,
        &[("a.md", "```nope\nx\n```\n"), ("b.md", "```js\nx\n```\n")],
    );
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert_eq!(errs[0].code, codes::SYNTAX_HIGHLIGHT_FAILED);
    assert!(
        errs[0].message.ends_with(
            "\"nope\" is not a valid Prism.js language for eleventy-plugin-syntaxhighlight"
        ),
        "{:?}",
        errs[0]
    );
    let (_t, r) = site(
        r#"let a = object(); a.x = (); config.add_plugin("syntaxhighlight", #{ code_attributes: a });"#,
        &[("a.md", "```js\nx\n```\n")],
    );
    assert_eq!(errors(&r)[0].code, codes::SYNTAX_HIGHLIGHT_FAILED);
    assert!(
        errors(&r)[0].message.ends_with(
            "Attribute \"x\" must have, or evaluate to, a value of type string or number, not \"undefined\"."
        ),
        "{:?}",
        errors(&r)[0]
    );
    // `{% highlight %}` fails the same way.
    let (_t, r) = site(
        r#"config.add_plugin("syntaxhighlight", #{ error_on_invalid_language: true });"#,
        &[("a.liquid", "x\n{% highlight nope %}x{% endhighlight %}")],
    );
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert_eq!(errs[0].code, codes::SYNTAX_HIGHLIGHT_FAILED);
    assert_eq!(
        errs[0].message,
        "the syntax highlighting plugin fails on this `{% highlight %}`: \"nope\" is not a valid Prism.js language for eleventy-plugin-syntaxhighlight"
    );
    assert_eq!(errs[0].line, Some(2));
}

#[test]
fn options_brrrty_cant_reproduce_are_refused() {
    let cases = [
        (
            r#"#{ preAttributes: #{ tabindex: 0 } }"#,
            "Brrrty doesn't support the option `preAttributes` (`preAttributes` is `pre_attributes` here)",
        ),
        (
            r#"#{ pre_attributes: #{ a: 1, b: 2 } }"#,
            "which sorts its keys",
        ),
        (r#"#{ pre_attributes: #{ a: || "x" } }"#, "has a function"),
        (r#"#{ code_attributes: "x" }"#, "must be an object"),
        (r#"#{ init: || 1 }"#, "`init` gets Prism itself"),
        (r#"#{ trim: 1 }"#, "`trim` must be true or false"),
        (
            r#"#{ languages: ["haskell"] }"#,
            "`languages` has `haskell`: Brrrty doesn't have Prism's `haskell` grammar yet",
        ),
        (
            r#"#{ languages: "js" }"#,
            "`languages` must be a list of strings",
        ),
    ];
    for (options, want) in cases {
        let p = load_err(&format!(
            "config.add_plugin(\"syntaxhighlight\", {options});\n#{{}}"
        ));
        assert_eq!(p.code, codes::CONFIG_INVALID, "{options}");
        assert!(p.message.contains(want), "{options}: {}", p.message);
    }
    let p = load_err(
        "config.add_plugin(\"syntaxhighlight\");\nconfig.add_plugin(\"syntaxhighlight\");\n#{}",
    );
    assert!(p.message.contains("called twice"), "{}", p.message);
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
            "---\ntags: posts\n---\n````md\n```js\nx\n```\n````\n",
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

/// A language that reaches the highlighter only as pages render (here a
/// fence built by a Rhai data file, which no file names) still refuses the
/// pages whose grammar it changes, and nothing is written with the
/// unchanged grammar.
#[test]
fn a_language_from_data_that_changes_a_grammar_refuses_pages() {
    let (t, r) = site(
        MARKER,
        &[
            (
                "_data/snippet.json",
                r#""\u0060\u0060\u0060regex\n^a+$\n\u0060\u0060\u0060""#,
            ),
            ("a.md", "---\ntitle: a\n---\n{{ snippet }}\n"),
            ("b.md", "```js\nconst r = /a+[b-c]/g;\n```\n"),
            ("c.md", "```bash\necho a\n```\n"),
        ],
    );
    let errs = errors(&r);
    assert!(out(&t, "b/index.html").is_none(), "{errs:#?}");
    assert!(out(&t, "a/index.html").is_none());
    assert!(out(&t, "c/index.html").is_some());
    let b = errs
        .iter()
        .find(|p| p.path.as_deref() == Some(Path::new("src/b.md")))
        .expect("b is refused");
    assert_eq!(b.code, codes::SYNTAX_HIGHLIGHT_UNSUPPORTED);
    assert!(
        b.message.contains(
            "src/a.md highlights code in the language `regex`. Loading it changes Prism's `javascript` grammar"
        ),
        "{}",
        b.message
    );
}

/// LiquidJS hands `{% highlight %}` its arguments as written, so
/// `{{ lang }}` there is the language `{{` (and `lang }}` marks lines),
/// as 11ty 3.1.8 writes it; `regex` is never loaded. The scan doesn't know
/// which engine reads a file, and in Nunjucks the shortcode's arguments
/// are expressions, so it counts `{{ lang }}` as any language: the js page
/// and bash pages are refused, never written wrong.
#[test]
fn a_highlight_tag_with_a_variable_language_counts_as_any_language() {
    let (t, r) = site(
        MARKER,
        &[
            (
                "a.liquid",
                "{% assign lang = 'regex' %}{% highlight {{ lang }} %}^a+${% endhighlight %}",
            ),
            ("b.md", "```js\nlet a = 1;\n```\n"),
            ("c.md", "```bash\necho a\n```\n"),
        ],
    );
    assert_eq!(
        out(&t, "a/index.html").as_deref(),
        Some(
            r#"<pre class="language-{{"><code class="language-{{"><span class="highlight-line">^a+$</span></code></pre>"#
        )
    );
    assert!(out(&t, "b/index.html").is_none(), "{:#?}", errors(&r));
    // Any language includes `naniscript`, whose hook acts on every grammar.
    assert!(out(&t, "c/index.html").is_none());
    let errs = errors(&r);
    assert_eq!(errs.len(), 2, "{errs:#?}");
    assert!(
        errs[0]
            .message
            .contains("src/a.liquid:1 names the language `{{`"),
        "{}",
        errs[0].message
    );
}

/// `addPlugin(pluginRss)`, the plugin's default export, adds its filters
/// and no feed: `#{ feed: false }`.
#[test]
fn the_rss_filters_come_without_a_feed() {
    let (t, r) = site(
        r#"config.add_plugin("rss", #{ feed: false });"#,
        &posts(&[
            (
                "feed.njk",
                "---\npermalink: feed.xml\n---\n{{ collections.posts | getNewestCollectionItemDate | dateToRfc3339 }} {{ '/a/' | absoluteUrl('https://example.com/') }} {{ '/b/' | htmlBaseUrl('https://example.com/') }}",
            ),
            (
                "a.liquid",
                "{{ '/x/' | htmlBaseUrl: 'https://example.com/s/' }}",
            ),
        ]),
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        out(&t, "feed.xml").unwrap(),
        "2024-01-02T00:00:00Z https://example.com/a/ https://example.com/b/"
    );
    assert_eq!(out(&t, "a/index.html").unwrap(), "https://example.com/x/");
    // No feed of its own: the two posts and the two pages.
    assert_eq!(r.written, 4);
}

#[test]
fn the_rss_filters_take_no_feed_options() {
    for (options, part) in [
        ("#{ feed: true }", "`feed` can only be `false`"),
        (
            "#{ feed: false, collection: #{ name: \"p\" } }",
            "takes no other options",
        ),
    ] {
        let p = feed_err(options);
        assert_eq!(p.code, codes::CONFIG_INVALID, "{options}");
        assert!(p.message.contains(part), "{options}: {}", p.message);
    }
}

/// Brrrty's `rss` plugin is version 3's: a site that installs another
/// version is refused, and a version that changed since `brrrty init`
/// recorded it warns.
#[test]
fn the_rss_plugin_version_is_checked_when_the_config_loads() {
    let t = tempfile::tempdir().unwrap();
    let pkg = |v: &str| {
        write(
            t.path(),
            "package.json",
            &format!(
                "{{\"devDependencies\":{{\"@11ty/eleventy\":\"3.1.8\",\"@11ty/eleventy-plugin-rss\":\"{v}\"}}}}"
            ),
        )
    };
    for script in [
        "config.add_plugin(\"rss\", #{ feed: false });\n",
        &format!("{FEED}\n"),
    ] {
        write(
            t.path(),
            CONFIG_FILE,
            &format!("// RSS plugin: ^3.1.0\n{script}"),
        );
        pkg("^3.1.0");
        let c = load_with(t.path(), Clock::Fixed(NOON_MS)).unwrap().unwrap();
        assert!(c.problems.is_empty(), "{:?}", c.problems);
        pkg("~3.0.0");
        let c = load_with(t.path(), Clock::Fixed(NOON_MS)).unwrap().unwrap();
        assert_eq!(c.problems.len(), 1, "{:?}", c.problems);
        assert_eq!(c.problems[0].code, codes::CONFIG_STALE);
        pkg("^2.0.4");
        let p = load_with(t.path(), Clock::Fixed(NOON_MS)).unwrap_err();
        assert_eq!(p[0].code, codes::CONFIG_UNSUPPORTED, "{p:?}");
        assert!(p[0].message.contains("version ^2.0.4"), "{}", p[0].message);
    }
}

/// A feed's template can't be in the includes or layouts folder, wherever
/// those are.
#[test]
fn a_feed_in_the_includes_folder_is_refused() {
    let p = load_err(
        "config.add_plugin(\"rss\", #{ input_path: \"parts/feed.njk\", collection: #{ name: \"p\" } });\n#{ dir: #{ includes: \"parts\" } }\n",
    );
    assert_eq!(p.code, codes::CONFIG_INVALID);
    assert!(p.message.contains("`parts/feed.njk`"), "{}", p.message);
    // `_includes` is an ordinary folder then.
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        CONFIG_FILE,
        "config.add_plugin(\"rss\", #{ input_path: \"_includes/feed.njk\", collection: #{ name: \"p\" } });\n#{ dir: #{ includes: \"parts\" } }\n",
    );
    assert!(load_with(t.path(), Clock::Fixed(NOON_MS)).is_ok());
}

/// `addPlugin(EleventyHtmlBasePlugin)` with no options: 11ty's `pathPrefix`
/// is `/` in Brrrty, so the plugin rewrites no URL in a page (its transform
/// is off for the default base); what it adds is two filters.
#[test]
fn the_html_base_plugin_adds_its_filters() {
    let (t, r) = site(
        r#"config.add_plugin("htmlbase");"#,
        &[
            (
                "a.liquid",
                "{{ '/x/' | htmlBaseUrl: 'https://example.com/s/' }} {{ '/y/' | htmlBaseUrl }}",
            ),
            (
                "b.njk",
                "{{ '/x/' | htmlBaseUrl('https://example.com/s/') }} {{ 'https://example.com' | addPathPrefixToFullUrl }}",
            ),
            ("c.md", "[l](/c/) <a href=\"d/\">d</a>\n"),
        ],
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        out(&t, "a/index.html").unwrap(),
        "https://example.com/x/ /y/"
    );
    assert_eq!(
        out(&t, "b/index.html").unwrap(),
        "https://example.com/x/ https://example.com/"
    );
    assert_eq!(
        out(&t, "c/index.html").unwrap(),
        "<p><a href=\"/c/\">l</a> <a href=\"d/\">d</a></p>\n"
    );
}

/// Without the plugin the filters don't exist.
#[test]
fn the_html_base_filters_need_the_plugin() {
    let (_t, r) = site("", &[("a.liquid", "{{ '/x/' | htmlBaseUrl }}")]);
    assert!(!errors(&r).is_empty());
}

#[test]
fn the_html_base_plugin_refuses_options_it_does_not_have() {
    let p = load_err(r#"config.add_plugin("htmlbase", #{ baseHref: "https://example.com/" });"#);
    assert_eq!(p.code, codes::CONFIG_INVALID);
    assert!(p.message.contains("htmlbase"), "{}", p.message);
    assert!(p.message.contains("baseHref"), "{}", p.message);
}
