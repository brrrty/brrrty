//! markdown-it renderer rules in Rhai: `config.amend_library("md", ...)` on 11ty's Markdown and
//! `markdown_it(options)` instances, against markdown-it 14.3.3 (11ty's) and 15.0.3 with the
//! same rules in JavaScript (`crates/brrrty-markdown/tools/markdown-rules.cjs`).
//!
//! `MARKDOWN_RULES_FUZZ=corpus.json cargo test -p brrrty-config --test markdown_rules -- --ignored`
//! replays a corpus made by `gen-markdown-fuzz.cjs` with `MARKDOWN_RULES=1` (and the same
//! `MARKDOWN_MODE` or `MARKDOWN_OPTIONS`).

use std::fs;
use std::path::{Path, PathBuf};

use brrrty_config::{CONFIG_FILE, Clock, SiteConfig, load_with};
use brrrty_core::{BuildOptions, BuildReport, Problem, Severity, build, codes};
use brrrty_js::Value;
use brrrty_markdown::{
    ConfiguredMarkdownIt, DefaultMarkdownIt, MarkdownIt, MarkdownRenderer, Options, RenderError,
};

const NOON_MS: i64 = 1_699_963_200_000;
const RULES: &str = include_str!("probes/markdown_rules.rhai");

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn load_in(root: &Path, script: &str) -> Result<SiteConfig, Vec<Problem>> {
    write(root, CONFIG_FILE, script);
    load_with(root, Clock::Fixed(NOON_MS)).map(|c| c.unwrap())
}

fn load(script: &str) -> SiteConfig {
    let t = tempfile::tempdir().unwrap();
    load_in(t.path(), script).unwrap_or_else(|p| panic!("{p:#?}"))
}

fn load_err(script: &str) -> String {
    let t = tempfile::tempdir().unwrap();
    match load_in(t.path(), script) {
        Err(p) => p
            .iter()
            .map(|p| p.message.clone())
            .collect::<Vec<_>>()
            .join("\n"),
        Ok(_) => panic!("expected a problem"),
    }
}

/// A wrapper for the docs' idiom.
const TAGGED: &str =
    "fn tagged(base, t, i, o, e, r) { t[i].attr_set(\"rel\", \"x\"); base.call(t, i, o, e, r) }";

/// What `expr` evaluates to, after the differential's rules are defined.
fn eval(expr: &str) -> Value {
    let cfg = load(&format!(
        "{RULES}\n{TAGGED}\nconfig.add_global_data(\"r\", {expr});"
    ));
    cfg.global_data["r"].clone()
}

fn eval_err(expr: &str) -> String {
    load_err(&format!("{RULES}\nconfig.add_global_data(\"r\", {expr});"))
}

fn s(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_string(),
        other => panic!("not a string: {other:?}"),
    }
}

const INPUTS: [&str; 2] = [
    "[a](/x) [b](http://shop.example/y \"T\") <http://go.to/z> ![i *j*](/p.png \"t\") `c` *e* **s** ~~d~~ a\\*b &amp; <b>h</b>\nline  \nend http://www.example.com",
    "# [h](/h)\n\n- [x](http://WWW.Shop.example)\n- y\n\n| a | [b](/b) |\n|---|---|\n| `c` | d |\n\n> q *e*",
];

/// markdown-it's output for [`INPUTS`] with the rules (`markdown-rules.cjs`): 11ty's instance
/// and `markdownIt()` write the first the same way.
const FIRST: &str = "<p><a href=\"https://example.org/x\" title=\"a\" class=\"l0 plain\">a</a> <a href=\"http://shop.example/y\" title=\"T\" rel=\"sponsored nofollow\" target=\"_blank\" class=\"l0 plain\">b</a> <a href=\"http://go.to/z\" rel=\"sponsored nofollow\" target=\"_blank\" title=\"http://go.to/z\" class=\"l0 autolink\">http://go.to/z</a><span class=\"auto\"></span> <span class=\"img\" data-label=\"i *j*\"><img src=\"/p.png\" alt=\"i j\" title=\"t\" loading=\"lazy\"></span> <code class=\"a b&quot;&amp;\">c</code> <i data-m=\"*\">e</i> <strong data-level=\"0\">s</strong> <s>d</del> a*b &amp; &lt;b&gt;h&lt;/b&gt;<wbr>\nline<br>\nend http://www.example.com</p>\n";
const FIRST_LINKIFY: &str = "<p><a href=\"https://example.org/x\" title=\"a\" class=\"l0 plain\">a</a> <a href=\"http://shop.example/y\" title=\"T\" rel=\"sponsored nofollow\" target=\"_blank\" class=\"l0 plain\">b</a> <a href=\"http://go.to/z\" rel=\"sponsored nofollow\" target=\"_blank\" title=\"http://go.to/z\" class=\"l0 autolink\">http://go.to/z</a><span class=\"auto\"></span> <span class=\"img\" data-label=\"i *j*\"><img src=\"/p.png\" alt=\"i j\" title=\"t\" loading=\"lazy\"></span> <code class=\"a b&quot;&amp;\">c</code> <i data-m=\"*\">e</i> <strong data-level=\"0\">s</strong> <s>d</del> a*b &amp; &lt;b&gt;h&lt;/b&gt;<wbr>\nline<br>\nend <a href=\"http://www.example.com\" title=\"http://www.example.com\" class=\"l0 linkify\">http://www.example.com</a><span class=\"auto\"></span></p>\n";
const SECOND: &str = "<h1><a href=\"https://example.org/h\" title=\"h\" class=\"l0 plain\">h</a></h1>\n<ul>\n<li><a href=\"http://WWW.Shop.example\" rel=\"sponsored nofollow\" target=\"_blank\" title=\"x\" class=\"l0 plain\">x</a></li>\n<li>y</li>\n</ul>\n<table>\n<thead>\n<tr>\n<th>a</th>\n<th><a href=\"https://example.org/b\" title=\"b\" class=\"l0 plain\">b</a></th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td><code class=\"a b&quot;&amp;\">c</code></td>\n<td>d</td>\n</tr>\n</tbody>\n</table>\n<blockquote>\n<p>q <i data-m=\"*\">e</i></p>\n</blockquote>\n";

fn rhai_string(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

#[test]
fn markdown_it_instances_run_rules_as_markdown_it_does() {
    for (options, first) in [
        ("", FIRST),
        (
            "#{ linkify: true, typographer: true, fuzzy_link: true }",
            FIRST_LINKIFY,
        ),
    ] {
        let expr = format!(
            "{{ let md = install(markdown_it({options})); [md.render({}), md.render({})] }}",
            rhai_string(INPUTS[0]),
            rhai_string(INPUTS[1])
        );
        let Value::Array(got) = eval(&expr) else {
            panic!()
        };
        assert_eq!(s(&got[0]), first, "{options}");
        assert_eq!(s(&got[1]), SECOND, "{options}");
    }
}

#[test]
fn amend_library_rules_render_like_11tys_markdown_it() {
    let cfg = load(&format!(
        "{RULES}\nconfig.amend_library(\"md\", |md| install(md));"
    ));
    let rules = cfg.markdown_rules.expect("rules").0;
    let md = MarkdownIt::new();
    assert_eq!(
        md.try_render_with_rules(INPUTS[0], None, rules.as_ref())
            .unwrap(),
        FIRST
    );
    assert_eq!(
        md.try_render_with_rules(INPUTS[1], None, rules.as_ref())
            .unwrap(),
        SECOND
    );
}

/// The site rule's shape: wrap the link rule, tag matching links, call the base.
const AFFILIATE: &str = r#"
fn affiliate_rel(href) {
    if regex_test(href, "(?:^|\\/\\/)(?:go\\.to|(?:www\\.)?shop\\.)", "i") { "sponsored nofollow noopener" } else { "nofollow noopener" }
}
fn link_rule(base, tokens, idx, options, env, renderer) {
    let href = tokens[idx].attr_get("href") ?? "";
    if regex_test(href, "(?:^|\\/\\/)(?:go\\.to|(?:www\\.)?shop\\.)", "i") {
        tokens[idx].attr_set("rel", affiliate_rel(href));
        tokens[idx].attr_set("target", "_blank");
    }
    if type_of(base) == "()" { renderer.render_token(tokens, idx, options) } else { base.call(tokens, idx, options, env, renderer) }
}
fn affiliate_link_rule(md) {
    md.renderer.rules.link_open = Fn("link_rule").curry(md.renderer.rules.link_open);
    md
}
"#;

fn site(script: &str, pages: &[(&str, &str)]) -> (tempfile::TempDir, BuildReport) {
    let t = tempfile::tempdir().unwrap();
    for (name, body) in pages {
        write(t.path(), &format!("src/{name}"), body);
    }
    let cfg = load_in(
        t.path(),
        &format!("{script}\n#{{ dir: #{{ input: \"src\" }} }}\n"),
    )
    .unwrap_or_else(|p| panic!("{p:#?}"));
    let opts = BuildOptions {
        root: t.path().to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        config: cfg.build_options(),
        ..Default::default()
    };
    let report = build(&opts, &MarkdownIt::new());
    (t, report)
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

#[test]
fn amend_library_applies_to_markdown_pages_and_layouts() {
    let (t, r) = site(
        &format!("{AFFILIATE}\nconfig.amend_library(\"md\", |md| affiliate_link_rule(md));"),
        &[
            ("a.md", "[x](https://shop.example/p) and [y](/y)\n"),
            ("_includes/l.md", "{{ content }}\n[z](http://go.to/q)\n"),
            ("b.md", "---\nlayout: l.md\n---\nhi\n"),
            ("c.liquid", "[x](https://shop.example/p)"),
        ],
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(
        out(&t, "a/index.html").unwrap(),
        "<p><a href=\"https://shop.example/p\" rel=\"sponsored nofollow noopener\" target=\"_blank\">x</a> and <a href=\"/y\">y</a></p>\n"
    );
    assert_eq!(
        out(&t, "b/index.html").unwrap(),
        "<p>hi</p>\n<p><a href=\"http://go.to/q\" rel=\"sponsored nofollow noopener\" target=\"_blank\">z</a></p>\n"
    );
    // Not Markdown: untouched.
    assert_eq!(
        out(&t, "c/index.html").unwrap(),
        "[x](https://shop.example/p)"
    );
}

#[test]
fn a_rule_that_fails_refuses_only_its_page() {
    let (t, r) = site(
        "config.amend_library(\"md\", |md| { md.renderer.rules.link_open = |tokens, idx, options, env, renderer| env.page.url; });",
        &[("a.md", "[x](/p)\n"), ("b.md", "no links\n")],
    );
    let e = errors(&r);
    assert_eq!(e.len(), 1, "{e:#?}");
    assert_eq!(e[0].code, codes::MARKDOWN_RULE_FAILED);
    assert!(
        e[0].message.contains("renderer rule `link_open` failed")
            && e[0]
                .message
                .contains("env.page: 11ty renders Markdown with the page's data"),
        "{}",
        e[0].message
    );
    assert!(out(&t, "a/index.html").is_none());
    assert_eq!(out(&t, "b/index.html").unwrap(), "<p>no links</p>\n");
}

#[test]
fn rules_for_block_tokens_are_refused() {
    let e = load_err(
        "config.amend_library(\"md\", |md| { md.renderer.rules.paragraph_open = |t, i, o, e, r| \"<p>\"; });",
    );
    assert!(e.contains("`paragraph_open` is a block token"), "{e}");
    let e =
        eval_err("{ let md = markdown_it(); md.renderer.rules.fence = |t, i, o, e, r| \"\"; 1 }");
    assert!(e.contains("`fence` is a block token"), "{e}");
}

#[test]
fn other_libraries_and_set_library_are_refused() {
    let e = load_err("config.amend_library(\"njk\", |env| env);");
    assert!(e.contains("only has 11ty's Markdown library"), "{e}");
    let e = load_err("config.set_library(\"md\", markdown_it());");
    assert!(e.contains("set_library(\"md\", ...) isn't ported"), "{e}");
}

#[test]
fn what_a_rule_reads_and_changes() {
    // `inline` rules exist in markdown-it but are never called; types it never makes are fine.
    let v = eval(
        "{ let md = markdown_it(); md.renderer.rules.inline = |t, i, o, e, r| \"X\"; md.renderer.rules.footnote_ref = |t, i, o, e, r| \"X\"; md.render(\"a\") }",
    );
    assert_eq!(s(&v), "<p>a</p>\n");
    // An unset rule reads as (); markdown-it's own rules can be read and called.
    let v = eval(
        "{ let md = markdown_it(); [type_of(md.renderer.rules.link_open), type_of(md.renderer.rules.text)] }",
    );
    assert_eq!(format!("{v:?}"), format!("{:?}", eval("[\"()\", \"Fn\"]")));
    // A rule on a markdown_it() instance reads env as `{}` and options as markdown-it's.
    let v = eval(
        "{ let md = markdown_it(#{ html: true }); md.renderer.rules.text = |t, i, o, e, r| `${type_of(e.anything)} ${o.html} ${o.xhtmlOut} ${o.langPrefix} ${t.len} ${t[5] == ()}`; md.render(\"a\") }",
    );
    assert_eq!(s(&v), "<p>() true false language- 1 true</p>\n");
    // Setting a rule back to () removes it; setting the default back is the default.
    let v = eval(
        "{ let md = markdown_it(); let base = md.renderer.rules.text; md.renderer.rules.text = |t, i, o, e, r| \"X\"; md.renderer.rules.text = base; md.renderer.rules.em_open = |t, i, o, e, r| \"Y\"; md.renderer.rules.em_open = (); md.render(\"a *b*\") }",
    );
    assert_eq!(s(&v), "<p>a <em>b</em></p>\n");
    // attrs read and written whole.
    let v = eval(
        "{ let md = markdown_it(); md.renderer.rules.link_open = |t, i, o, e, r| { let tok = t[i]; tok.attrs.push([\"data-x\", \"1\"]); tok.attrs = tok.attrs + [[\"b\", \"<\"]]; r.render_token(t, i, o) }; md.render(\"[a](u \\\"t\\\")\") }",
    );
    assert_eq!(
        s(&v),
        "<p><a href=\"u\" title=\"t\" data-x=\"1\" b=\"&lt;\">a</a></p>\n"
    );
    // A plugin is a function that takes `md`.
    let v = eval("{ let md = markdown_it(); install(md); md.render(\"`c`\") }");
    assert_eq!(s(&v), "<p><code class=\"a b&quot;&amp;\">c</code></p>\n");
}

#[test]
fn what_brrrty_cant_give_a_rule_is_an_error() {
    let cases = [
        // JS would write `undefined`.
        ("|t, i, o, e, r| ()", "returned (), not a string"),
        (
            "|t, i, o, e, r| { e.x = 1; \"\" }",
            "`env` is shared by every rule",
        ),
        (
            "|t, i, o, e, r| { o.xhtmlOut = true; \"\" }",
            "Brrrty doesn't let a rule change them",
        ),
        ("|t, i, o, e, r| { o.highlight; \"\" }", "options.highlight"),
        (
            "|t, i, o, e, r| { t[i].attr_set(\"a\", 1); \"\" }",
            "attr_set's value must be a string",
        ),
        (
            "|t, i, o, e, r| { t[i].map = 1; \"\" }",
            "token.map can't be set",
        ),
        ("|t, i, o, e, r| { t[i] = t[0]; \"\" }", ""),
        ("|t, i, o, e, r| { t.push(1); \"\" }", "push"),
    ];
    for (rule, want) in cases {
        let e = eval_err(&format!(
            "{{ let md = markdown_it(); md.renderer.rules.link_open = {rule}; md.render(\"[a](b)\") }}"
        ));
        assert!(e.contains(want), "{rule}: {e}");
    }
    // An image's children aren't given.
    let e = eval_err(
        "{ let md = markdown_it(); md.renderer.rules.image = |t, i, o, e, r| { t[i].children; \"\" }; md.render(\"![a](b)\") }",
    );
    assert!(e.contains("an image's children"), "{e}");
}

#[test]
fn removing_alt_before_the_image_rule_is_an_error() {
    let cfg = load(
        "fn drop_alt(base, t, i, o, e, r) { t[i].attrs = [[\"src\", \"x\"]]; base.call(t, i, o, e, r) }\nconfig.amend_library(\"md\", |md| { md.renderer.rules.image = Fn(\"drop_alt\").curry(md.renderer.rules.image); });",
    );
    let rules = cfg.markdown_rules.unwrap().0;
    let e = MarkdownIt::new()
        .try_render_with_rules("![a](b)", None, rules.as_ref())
        .unwrap_err();
    assert!(
        matches!(&e, RenderError::Rule(m) if m.contains("needs an `alt` attribute")),
        "{e:?}"
    );
}

#[test]
#[ignore = "needs MARKDOWN_RULES_FUZZ=<corpus.json>"]
fn rules_fuzz_corpus() {
    let path = std::env::var("MARKDOWN_RULES_FUZZ").expect("MARKDOWN_RULES_FUZZ");
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let eleventy = std::env::var("MARKDOWN_MODE").as_deref() == Ok("eleventy");
    let options = std::env::var("MARKDOWN_OPTIONS").ok().map(|o| {
        let o: serde_json::Value = serde_json::from_str(&o).expect("MARKDOWN_OPTIONS is JSON");
        let flag = |k: &str| o[k].as_bool().unwrap_or(false);
        Options {
            html: flag("html"),
            linkify: flag("linkify"),
            typographer: flag("typographer"),
            fuzzy_link: flag("fuzzyLink"),
        }
    });
    let cfg = load(&format!(
        "{RULES}\nconfig.amend_library(\"md\", |md| install(md));"
    ));
    let rules = cfg.markdown_rules.unwrap().0;
    let configured = options.map(ConfiguredMarkdownIt::new);
    let default = DefaultMarkdownIt::new();
    let eleventy_md = MarkdownIt::new();
    let (mut bad, mut refused, mut n) = (0, 0, 0);
    for c in v.as_array().unwrap() {
        let s = c["s"].as_str().unwrap();
        let Some(want) = c["ruled"].as_str() else {
            continue;
        };
        n += 1;
        let got = match (&configured, eleventy) {
            (Some(m), _) => m.render_with_rules(s, rules.as_ref()),
            (None, true) => eleventy_md.try_render_with_rules(s, None, rules.as_ref()),
            (None, false) => default.render_with_rules(s, rules.as_ref()),
        };
        match got {
            Err(RenderError::Refused(_)) => refused += 1,
            Err(RenderError::Rule(e)) => {
                bad += 1;
                println!("rule failed {s:?}: {e}");
            }
            Ok(got) if got != want => {
                bad += 1;
                println!("{s:?}\n  want {want:?}\n  got  {got:?}");
            }
            Ok(_) => {}
        }
    }
    println!("{n} cases, {bad} differences, {refused} refused");
    assert_eq!(bad, 0);
}

/// Review round 1, C1: `renderToken` differs between markdown-it 14.3.3 (11ty's instance) and
/// 15.0.3 (`markdown_it()`) around hidden tokens. Expected output from both, in Node.
const HIDING: &str = r#"
fn hide_prev(t, i, o, e, s) { t[i - 1].hidden = true; t[i].block = true; s.render_token(t, i, o) }
fn hide_next(t, i, o, e, s) { t[i].block = true; t[i + 1].hidden = true; s.render_token(t, i, o) }
fn hiding(md) {
    md.renderer.rules.strong_open = Fn("hide_prev");
    md.renderer.rules.em_open = Fn("hide_next");
    md
}
"#;

#[test]
fn render_token_follows_each_markdown_it_version() {
    let cfg = load(&format!(
        "{HIDING}\nconfig.amend_library(\"md\", |md| hiding(md));\nconfig.add_global_data(\"r\", hiding(markdown_it()).render(\"a **b** *c* d\"));"
    ));
    assert_eq!(
        s(&cfg.global_data["r"]),
        "<p>a <strong>\nb</strong> <em>c</em> d</p>\n",
        "15.0.3"
    );
    let rules = cfg.markdown_rules.unwrap().0;
    assert_eq!(
        MarkdownIt::new()
            .try_render_with_rules("a **b** *c* d", None, rules.as_ref())
            .unwrap(),
        "<p>a \n<strong>\nb</strong> <em>c</em> d</p>\n",
        "14.3.3"
    );
}

/// Review round 1, C3: a token's type can change to an inline or unknown type (each checked
/// against markdown-it 14.3.3 and 15.0.3, which agree), never to a block type; and the image
/// rule on a token that isn't an image is an error, as markdown-it throws.
#[test]
fn changing_a_tokens_type() {
    let cases = [
        ("code_inline", "<p><em><code>a&lt;b</code></em></p>\n"),
        ("html_inline", "<p><em>a<b</em></p>\n"),
        ("softbreak", "<p><em>\n</em></p>\n"),
        ("hardbreak", "<p><em><br>\n</em></p>\n"),
        ("foo", "<p><em><x></em></p>\n"),
        ("inline", "<p><em><x></em></p>\n"),
        ("text", "<p><em>a&lt;b</em></p>\n"),
        ("link_close", "<p><em><x></em></p>\n"),
    ];
    for (kind, want) in cases {
        let rule = format!(
            "|t, i, o, e, s| {{ t[i + 1].type = \"{kind}\"; t[i + 1].tag = \"x\"; s.render_token(t, i, o) }}"
        );
        let v = eval(&format!(
            "{{ let md = markdown_it(); md.renderer.rules.em_open = {rule}; md.render(\"*a<b*\") }}"
        ));
        assert_eq!(s(&v), want, "{kind}, 15");
        let cfg = load(&format!(
            "config.amend_library(\"md\", |md| {{ md.renderer.rules.em_open = {rule}; }});"
        ));
        let got = MarkdownIt::new()
            .try_render_with_rules("*a<b*", None, cfg.markdown_rules.unwrap().0.as_ref())
            .unwrap();
        assert_eq!(got, want, "{kind}, 14.3.3");
    }
    for kind in [
        "fence",
        "code_block",
        "html_block",
        "paragraph_open",
        "table_open",
    ] {
        let e = eval_err(&format!(
            "{{ let md = markdown_it(); md.renderer.rules.em_open = |t, i, o, e, s| {{ t[i + 1].type = \"{kind}\"; \"\" }}; md.render(\"*a*\") }}"
        ));
        assert!(
            e.contains(&format!("`{kind}` is a block token type")),
            "{e}"
        );
    }
    let e = eval_err(
        "{ let md = markdown_it(); md.renderer.rules.em_open = |t, i, o, e, s| { t[i + 1].attr_push([\"alt\", \"x\"]); t[i + 1].type = \"image\"; s.render_token(t, i, o) }; md.render(\"*a<b*\") }",
    );
    assert!(e.contains("only an image token has"), "{e}");
}

/// Review round 1, C2: libraries can't change once pages render, so a rule that tries to is
/// an error on its page, and every build writes the same pages.
#[test]
fn libraries_are_frozen_while_pages_render() {
    let script = r#"
fn stamp(t, i, o, e, s) { "MUTATED" }
fn lo(inner, t, i, o, e, s) {
    if t[i].attr_get("href") == "/flip" { inner.renderer.rules.text = Fn("stamp"); }
    s.render_token(t, i, o) + inner.render("x")
}
fn install(md) { md.renderer.rules.link_open = Fn("lo").curry(markdown_it()); md }
fn mutate_own(md, t, i, o, e, s) { md.renderer.rules.text = Fn("stamp"); s.render_token(t, i, o) }
config.amend_library("md", |md| install(md));
config.add_filter("tweak", Fn("tweak_filter").curry(markdown_it()));
fn tweak_filter(md, v) { md.renderer.rules.text = Fn("stamp"); v }
"#;
    let mut pages: Vec<(String, String)> = (0..40)
        .map(|i| (format!("p{i}.md"), format!("[a](/p{i}) b\n")))
        .collect();
    pages.push(("flip.md".into(), "[a](/flip)\n".into()));
    pages.push(("f.liquid".into(), "{{ 'x' | tweak }}".into()));
    let pages: Vec<(&str, &str)> = pages
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let mut first: Option<Vec<String>> = None;
    for _ in 0..3 {
        let (t, r) = site(script, &pages);
        let e = errors(&r);
        assert_eq!(e.len(), 2, "{e:#?}");
        assert!(
            e.iter().any(|p| p.code == codes::MARKDOWN_RULE_FAILED
                && p.message
                    .contains("can't be changed from inside a renderer rule")),
            "{e:#?}"
        );
        assert!(
            e.iter()
                .any(|p| p.message.contains("can't be changed while pages render")),
            "{e:#?}"
        );
        let outs: Vec<String> = (0..40)
            .map(|i| out(&t, &format!("p{i}/index.html")).unwrap())
            .collect();
        assert!(outs.iter().all(|o| !o.contains("MUTATED")));
        assert_eq!(outs[0], "<p><a href=\"/p0\"><p>x</p>\na</a> b</p>\n");
        if let Some(f) = &first {
            assert_eq!(f, &outs);
        }
        first = Some(outs);
    }
}

#[test]
fn review_minors() {
    // M1: markdown-it's render throws on anything but a string.
    let e = eval_err("markdown_it().render(())");
    assert!(e.contains("Input data should be a String"), "{e}");
    // M2: a block rule can't be read either.
    let e = eval_err("{ let md = markdown_it(); md.renderer.rules.fence }");
    assert!(e.contains("`fence` is a block token"), "{e}");
    // M5: 11ty lowercases the library's name.
    let cfg = load(&format!(
        "{AFFILIATE}\nconfig.amend_library(\"MD\", |md| affiliate_link_rule(md));"
    ));
    assert!(cfg.markdown_rules.is_some());
    // M6: a closure that captures nothing stands in for `|| renderToken`.
    let v = eval(
        "{ let md = markdown_it(); let base = md.renderer.rules.link_open ?? |t, i, o, e, r| r.render_token(t, i, o); md.renderer.rules.link_open = Fn(\"tagged\").curry(base); md.render(\"[a](b)\") }",
    );
    assert_eq!(s(&v), "<p><a href=\"b\" rel=\"x\">a</a></p>\n");
}
