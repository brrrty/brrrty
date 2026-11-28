//! `config.set_js_config("node")`: the site's 11ty config runs in Node and what
//! it recorded is applied to the build.

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

fn run(dir: &Path, cmd: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .current_dir(dir)
        .arg(cmd)
        .args(args)
        .env_remove("BRRRTY_NODE")
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

const CONFIG: &str = r#"module.exports = function (eleventyConfig) {
  eleventyConfig.addPassthroughCopy("a").addPassthroughCopy("b");
  eleventyConfig.addGlobalData("who", "world");
  eleventyConfig.setQuietMode(true);
  return { dir: { input: "src" } };
};
"#;

fn fixture(rhai: &str) -> tempfile::TempDir {
    site(&[
        ("brrrty.config.rhai", rhai),
        ("eleventy.config.js", CONFIG),
        ("src/index.njk", "hello {{ who }}"),
        ("a/x.txt", "x"),
        ("b/y.txt", "y"),
    ])
}

#[test]
fn the_recorded_config_is_applied_to_the_build() {
    if !has_node() {
        return;
    }
    let t = fixture("config.set_js_config(\"node\");\n");
    let o = run(t.path(), "build", &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(!text(&o).contains("config-unverified"), "{}", text(&o));
    let out = t.path().join("_site");
    assert_eq!(
        fs::read_to_string(out.join("index.html")).unwrap(),
        "hello world"
    );
    assert_eq!(fs::read_to_string(out.join("a/x.txt")).unwrap(), "x");
    assert_eq!(fs::read_to_string(out.join("b/y.txt")).unwrap(), "y");
}

#[test]
fn the_flag_turns_it_on_and_off() {
    if !has_node() {
        return;
    }
    let t = fixture("");
    let o = run(t.path(), "build", &["--js-config=node"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(t.path().join("_site/a/x.txt").exists());
    let t = fixture("config.set_js_config(\"node\");\n");
    run(t.path(), "build", &["--js-config=off"]);
    assert!(!t.path().join("_site/a/x.txt").exists());
}

#[test]
fn off_is_the_default_and_changes_nothing() {
    let t = fixture("");
    // Without the line the 11ty config is not run: the site is read from
    // the project folder, so nothing is copied.
    run(t.path(), "build", &[]);
    assert!(!t.path().join("_site/a/x.txt").exists());
}

#[test]
fn rhai_lines_win_over_the_js_config() {
    if !has_node() {
        return;
    }
    let t =
        fixture("config.set_js_config(\"node\");\nconfig.add_global_data(\"who\", \"rhai\");\n");
    let o = run(t.path(), "build", &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "hello rhai"
    );
}

#[test]
fn doctor_lists_unsupported_methods_with_file_and_line() {
    if !has_node() {
        return;
    }
    let t = fixture("config.set_js_config(\"node\");\n");
    let o = run(t.path(), "doctor", &[]);
    let s = text(&o);
    assert!(s.contains("setQuietMode"), "{s}");
    assert!(s.contains("eleventy.config.js:4"), "{s}");
}

fn build_with(files: &[(&str, &str)], args: &[&str]) -> (tempfile::TempDir, Output) {
    let mut all = vec![("brrrty.config.rhai", "config.set_js_config(\"node\");\n")];
    all.extend_from_slice(files);
    let t = site(&all);
    let o = run(t.path(), "build", args);
    (t, o)
}

#[test]
fn returned_formats_and_engines_are_applied() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            (
                "eleventy.config.js",
                r#"module.exports = () => ({ dir: { input: "src" }, markdownTemplateEngine: "njk", templateFormats: [" MD "] });"#,
            ),
            ("src/index.md", "{% set x = \"nj\" %}{{ x }}"),
            ("src/other.html", "other"),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o));
    let out = t.path().join("_site");
    assert!(
        fs::read_to_string(out.join("index.html"))
            .unwrap()
            .contains("nj")
    );
    assert!(!out.join("other.html").exists() && !out.join("other/index.html").exists());
}

#[test]
fn a_returned_key_brrrty_does_not_apply_is_a_warning() {
    if !has_node() {
        return;
    }
    let (_t, o) = build_with(
        &[
            (
                "eleventy.config.js",
                r#"module.exports = () => ({ dir: { input: "src" }, pathPrefix: "/x/", somethingNew: 1 });"#,
            ),
            ("src/index.html", "hi"),
        ],
        &[],
    );
    let s = text(&o);
    assert!(
        s.contains("pathPrefix") && s.contains("somethingNew"),
        "{s}"
    );
}

#[test]
fn the_dot_eleventy_js_file_is_looked_up_first() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            (
                "eleventy.config.js",
                r#"module.exports = () => ({ dir: { input: "src" } });"#,
            ),
            (
                ".eleventy.js",
                r#"module.exports = () => ({ dir: { input: "other" } });"#,
            ),
            ("src/a.html", "src"),
            ("other/b.html", "other"),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o));
    assert!(t.path().join("_site/b/index.html").exists() || t.path().join("_site/b.html").exists());
    assert!(!t.path().join("_site/a/index.html").exists());
}

#[test]
fn an_esm_config_export_is_merged_into_the_return_value() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            (
                "eleventy.config.mjs",
                "export default function () {}\nexport const config = { dir: { input: \"src\" } };\n",
            ),
            ("src/index.html", "hi"),
            ("index.html", "wrong"),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "hi"
    );
}

#[test]
fn js_format_names_are_checked_like_rhai_ones() {
    if !has_node() {
        return;
    }
    let (_t, o) = build_with(
        &[
            (
                "eleventy.config.js",
                r#"module.exports = (c) => { c.setTemplateFormats([".md"]); };"#,
            ),
            ("index.md", "x"),
        ],
        &[],
    );
    assert!(!o.status.success(), "{}", text(&o));
    assert!(text(&o).contains(".md"), "{}", text(&o));
}

#[test]
fn rhai_added_formats_merge_with_js_set_formats() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            (
                "eleventy.config.js",
                r#"module.exports = (c) => { c.setTemplateFormats(["md"]); };"#,
            ),
            ("index.md", "md"),
            ("p.njk", "njk"),
            ("q.html", "html"),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o));
    assert!(!t.path().join("_site/q/index.html").exists());
    // Rhai adds njk on top of the JS set.
    let t = site(&[
        (
            "brrrty.config.rhai",
            "config.set_js_config(\"node\");\nconfig.add_template_formats(\"njk\");\n",
        ),
        (
            "eleventy.config.js",
            r#"module.exports = (c) => { c.setTemplateFormats(["md"]); };"#,
        ),
        ("index.md", "md"),
        ("p.njk", "njk"),
        ("q.html", "html"),
    ]);
    let o = run(t.path(), "build", &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(t.path().join("_site/p/index.html").exists());
    assert!(!t.path().join("_site/q/index.html").exists());
}

#[test]
fn passthrough_options_are_a_warning() {
    if !has_node() {
        return;
    }
    let (_t, o) = build_with(
        &[
            (
                "eleventy.config.js",
                r#"module.exports = (c) => { c.addPassthroughCopy("a", { filter: "*.txt" }); };"#,
            ),
            ("a/x.txt", "x"),
            ("index.html", "hi"),
        ],
        &[],
    );
    let s = text(&o);
    assert!(s.contains("filter") || s.contains("options"), "{s}");
}

#[test]
fn an_esm_config_export_is_deep_merged_into_the_return_value() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            ("package.json", r#"{"type":"module"}"#),
            (
                "eleventy.config.js",
                "export default function () { return { dir: { output: \"out\" } }; }\nexport const config = { dir: { input: \"src\" } };\n",
            ),
            ("src/index.md", "src"),
            ("other/index.md", "other"),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o));
    assert!(t.path().join("out/index.html").is_file(), "{}", text(&o));
    assert!(!t.path().join("out/other/index.html").exists());
}

#[test]
fn an_esm_config_export_wins_over_the_return_value() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            ("package.json", r#"{"type":"module"}"#),
            (
                "eleventy.config.js",
                "export default function () { return { dir: { input: \"other\" } }; }\nexport const config = { dir: { input: \"src\" } };\n",
            ),
            ("src/index.md", "src"),
            ("other/index.md", "other"),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o));
    let html = fs::read_to_string(t.path().join("_site/index.html")).unwrap();
    assert!(html.contains("src"), "{html}");
}

#[test]
fn the_missing_config_help_lists_the_files_in_lookup_order() {
    let t = site(&[("brrrty.config.rhai", "config.set_js_config(\"node\");\n")]);
    let o = run(t.path(), "build", &[]);
    let out = text(&o);
    let a = out.find(".eleventy.js").expect(&out);
    let b = out.find("eleventy.config.js").expect(&out);
    assert!(a < b, "{out}");
}

const FILTER_CONFIG: &str = r#"module.exports = function (eleventyConfig) {
  eleventyConfig.addFilter("shout", (s) => String(s).toUpperCase() + "!");
  eleventyConfig.addAsyncFilter("later", async (s) => { await null; return s + "?"; });
  eleventyConfig.addFilter("pageUrl", function () { return this.page.url; });
  eleventyConfig.addNunjucksFilter("njkOnly", (s) => "njk:" + s);
  eleventyConfig.addLiquidFilter("liqOnly", (s) => "liq:" + s);
  eleventyConfig.addNunjucksFilter("kw", (s, n, o) => s + n + o.sep);
  eleventyConfig.addFilter("boom", (s) => { throw new Error("nope " + s); });
  return { dir: { input: "src" } };
};
"#;

#[test]
fn config_filters_run_in_node_for_each_engine() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
            ("eleventy.config.js", FILTER_CONFIG),
            (
                "src/a.njk",
                "{{ 'a' | shout }}|{{ 'b' | later }}|{{ '' | pageUrl }}|{{ 'c' | njkOnly }}|{{ 'x' | kw(1, sep='-') }}",
            ),
            (
                "src/b.liquid",
                "{{ 'a' | shout }}|{{ 'b' | later }}|{{ '' | pageUrl }}|{{ 'c' | liqOnly }}",
            ),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o));
    let a = fs::read_to_string(t.path().join("_site/a/index.html")).unwrap();
    assert_eq!(a, "A!|b?|/a/|njk:c|x1-");
    let b = fs::read_to_string(t.path().join("_site/b/index.html")).unwrap();
    assert_eq!(b, "A!|b?|/b/|liq:c");
}

#[test]
fn an_engine_only_filter_is_not_in_the_other_engine() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
            ("eleventy.config.js", FILTER_CONFIG),
            ("src/b.liquid", "{{ 'c' | njkOnly }}"),
        ],
        &[],
    );
    assert!(!o.status.success(), "{}", text(&o));
    assert!(!t.path().join("_site/b/index.html").exists());
}

#[test]
fn a_rhai_filter_of_the_same_name_wins() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            (
                "brrrty.config.rhai",
                "config.set_js_config(\"node\");\nconfig.add_filter(\"shout\", |s| `rhai ${s}`);\n",
            ),
            ("eleventy.config.js", FILTER_CONFIG),
            ("src/a.njk", "{{ 'a' | shout }}"),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/a/index.html")).unwrap(),
        "rhai a"
    );
}

#[test]
fn a_throwing_filter_refuses_its_page_naming_filter_and_place() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
            ("eleventy.config.js", FILTER_CONFIG),
            ("src/a.njk", "{{ 'a' | boom }}"),
            ("src/ok.njk", "fine"),
        ],
        &[],
    );
    let s = text(&o);
    assert!(s.contains("boom"), "{s}");
    assert!(s.contains("nope a"), "{s}");
    assert!(s.contains("eleventy.config.js:8"), "{s}");
    assert!(!t.path().join("_site/a/index.html").exists());
}

const RUN_MODE_CONFIG: &str = r#"module.exports = function (eleventyConfig) {
  eleventyConfig.addFilter("mode", function () { return this.eleventy.env.runMode; });
  return { dir: { input: "src" } };
};
"#;

#[test]
fn an_explicit_js_data_off_is_kept_with_a_js_config() {
    if !has_node() {
        return;
    }
    let files = [
        ("eleventy.config.js", RUN_MODE_CONFIG),
        ("src/_data/env.js", "module.exports = { v: 'ran' };"),
        ("src/a.njk", "[{{ env.v }}]"),
    ];
    // Without a say, the config's Node runs data files too.
    let (t, o) = build_with(&files, &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/a/index.html")).unwrap(),
        "[ran]"
    );
    // Said off, in the flag or the config: they stay off.
    let (t, o) = build_with(&files, &["--js-data=off"]);
    let html = fs::read_to_string(t.path().join("_site/a/index.html")).unwrap_or_default();
    assert!(!html.contains("ran"), "flag: {html}\n{}", text(&o));
    let mut all = files.to_vec();
    all.push((
        "brrrty.config.rhai",
        "config.set_js_config(\"node\");\nconfig.set_js_data(\"off\");\n",
    ));
    let t = site(&all);
    let o = run(t.path(), "build", &[]);
    let html = fs::read_to_string(t.path().join("_site/a/index.html")).unwrap_or_default();
    assert!(!html.contains("ran"), "config: {html}\n{}", text(&o));
}

#[test]
fn the_kept_node_runs_in_the_builds_run_mode() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
        ("eleventy.config.js", RUN_MODE_CONFIG),
        (
            "src/_data/env.js",
            "module.exports = { v: process.env.ELEVENTY_RUN_MODE };",
        ),
        ("src/a.njk", "{{ env.v }}|{{ '' | mode }}"),
    ]);
    let flags = brrrty::options::Flags {
        serve: true,
        ..Default::default()
    };
    let mut opts = brrrty::options::build_options(t.path(), &flags).expect("options");
    opts.run_mode = brrrty_core::RunMode::Serve;
    opts.write_to = Some(t.path().join("_serve"));
    let report = brrrty_core::build(&opts, &brrrty_markdown::MarkdownIt::new());
    assert!(!report.has_errors(), "{:?}", report.problems);
    assert_eq!(
        fs::read_to_string(t.path().join("_serve/a/index.html")).unwrap(),
        "serve|serve"
    );
}

const THIS_CONFIG: &str = r#"module.exports = function (eleventyConfig) {
  eleventyConfig.addFilter("shout", (s) => String(s).toUpperCase());
  eleventyConfig.addFilter("readCtx", function () { return this.ctx; });
  eleventyConfig.addFilter("readEnv", function () { return this.env; });
  return { dir: { input: "src" } };
};
"#;

#[test]
fn this_ctx_and_this_env_refuse_naming_the_filter() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            ("eleventy.config.js", THIS_CONFIG),
            ("src/a.njk", "{{ '' | readCtx }}"),
            ("src/b.njk", "{{ '' | readEnv }}"),
            ("src/ok.njk", "fine"),
        ],
        &[],
    );
    let s = text(&o);
    assert!(s.contains("this.ctx"), "{s}");
    assert!(s.contains("this.env"), "{s}");
    assert!(s.contains("readCtx"), "{s}");
    assert!(!t.path().join("_site/a/index.html").exists());
    assert!(!t.path().join("_site/b/index.html").exists());
}

#[test]
fn a_safestring_given_to_a_config_filter_refuses_naming_it() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            ("eleventy.config.js", THIS_CONFIG),
            ("src/a.njk", "{{ 'x' | safe | shout }}"),
            ("src/ok.njk", "{{ 'x' | shout }}"),
        ],
        &[],
    );
    let s = text(&o);
    assert!(s.contains("SafeString") && s.contains("shout"), "{s}");
    assert!(!t.path().join("_site/a/index.html").exists());
    assert_eq!(
        fs::read_to_string(t.path().join("_site/ok/index.html")).unwrap(),
        "X"
    );
}

const SHORTCODE_CONFIG: &str = r#"module.exports = function (eleventyConfig) {
  eleventyConfig.addShortcode("sum", (a, b) => a + b);
  eleventyConfig.addShortcode("greet", function (name, o) {
    return `hi ${name}${o && o.punct ? o.punct : ""}|${this.page.url}`;
  });
  eleventyConfig.addShortcode("blank", (x) => `[${JSON.stringify(x)}]`);
  eleventyConfig.addShortcode("html", () => "<b>");
  eleventyConfig.addAsyncShortcode("later", async (s) => { await null; return s + "?"; });
  eleventyConfig.addPairedShortcode("box", (body, cls) => `<div class="${cls}">${body}</div>`);
  eleventyConfig.addPairedAsyncShortcode("abox", async (body, cls) => { await null; return `<p ${cls}>${body}</p>`; });
  eleventyConfig.addNunjucksShortcode("njkOnly", () => "njk");
  eleventyConfig.addShortcode("boom", () => { throw new Error("nope"); });
  return { dir: { input: "src" } };
};
"#;

#[test]
fn nunjucks_shortcodes_run_in_node_sync_async_and_paired() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
            ("eleventy.config.js", SHORTCODE_CONFIG),
            (
                "src/a.njk",
                concat!(
                    "{% sum 1, 2 %}|{% greet \"A, B\", punct=\"!\" %}|{% blank %}|{% later \"b\" %}|{% html %}|{% njkOnly %}|",
                    "{% box \"c\" %}{% set v = \"a&b\" %}{{ v }}{% if v %}!{% endif %}{% endbox %}|{% abox \"d\" %}x{% endabox %}"
                ),
            ),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o));
    let a = fs::read_to_string(t.path().join("_site/a/index.html")).unwrap();
    assert_eq!(
        a,
        "3|hi A, B!|/a/|[\"\"]|b?|<b>|njk|<div class=\"c\">a&amp;b!</div>|<p d>x</p>"
    );
}

#[test]
fn a_throwing_shortcode_refuses_its_page_naming_shortcode_and_place() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
            ("eleventy.config.js", SHORTCODE_CONFIG),
            ("src/a.njk", "{% boom %}"),
            ("src/ok.njk", "fine"),
        ],
        &[],
    );
    let s = text(&o);
    assert!(s.contains("the shortcode `boom`"), "{s}");
    assert!(s.contains("nope"), "{s}");
    assert!(s.contains("eleventy.config.js:12"), "{s}");
    assert!(!t.path().join("_site/a/index.html").exists());
}

#[test]
fn liquid_shortcodes_run_in_node_sync_async_and_paired() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
            ("eleventy.config.js", SHORTCODE_CONFIG),
            (
                "src/a.liquid",
                concat!(
                    "{% sum 1, 2 %}|{% greet \"A, B\" v %}|{% blank %}|{% later \"b\" %}|{% html %}|",
                    "{% box \"c\" %}{% assign w = \"a&b\" %}{{ w }}{% if w %}!{% endif %}{% endbox %}|{% abox v %}x{% endabox %}"
                ),
            ),
            ("src/_data/v.json", "\"V\""),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o));
    let a = fs::read_to_string(t.path().join("_site/a/index.html")).unwrap();
    assert_eq!(
        a,
        "3|hi A, B|/a/|[undefined]|b?|<b>|<div class=\"c\">a&b!</div>|<p V>x</p>"
    );
}

#[test]
fn a_throwing_liquid_shortcode_refuses_its_page() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
            ("eleventy.config.js", SHORTCODE_CONFIG),
            ("src/a.liquid", "{% boom %}"),
            ("src/ok.liquid", "fine"),
        ],
        &[],
    );
    let s = text(&o);
    assert!(s.contains("the shortcode `boom`"), "{s}");
    assert!(s.contains("nope"), "{s}");
    assert!(!t.path().join("_site/a/index.html").exists());
}

const SHORTCODE_EDGE_CONFIG: &str = r#"module.exports = function (eleventyConfig) {
  eleventyConfig.addShortcode("num", () => 0);
  eleventyConfig.addShortcode("f", () => false);
  eleventyConfig.addPairedShortcode("show", (b, a) => `[${b}|${a}]`);
  eleventyConfig.addPairedShortcode("pnum", () => 3);
  eleventyConfig.addPairedShortcode("parr", () => ["<b>", 2]);
  eleventyConfig.addPairedShortcode("pobj", () => ({a: 1}));
  eleventyConfig.addPairedShortcode("pnull", () => null);
  eleventyConfig.addPairedAsyncShortcode("pasync", async () => [1, 2]);
  return { dir: { input: "src" } };
};
"#;

fn edge_build(file: &str, body: &str) -> String {
    let (t, o) = build_with(
        &[
            ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
            ("eleventy.config.js", SHORTCODE_EDGE_CONFIG),
            (file, body),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o));
    fs::read_to_string(t.path().join("_site/a/index.html")).unwrap()
}

#[test]
fn liquid_shortcode_falsy_return_writes_nothing() {
    if !has_node() {
        return;
    }
    assert_eq!(edge_build("src/a.liquid", "{% num %}|{% f %}"), "|");
}

#[test]
fn paired_shortcode_args_are_evaluated_before_the_body() {
    if !has_node() {
        return;
    }
    assert_eq!(
        edge_build("src/a.njk", "{% show w %}{% set w = \"in\" %}{% endshow %}"),
        "[|undefined]"
    );
    assert_eq!(
        edge_build(
            "src/a.liquid",
            "{% show w %}{% assign w = \"in\" %}{% endshow %}"
        ),
        "[|undefined]"
    );
}

#[test]
fn nunjucks_paired_shortcode_non_string_return_prints_undefined() {
    if !has_node() {
        return;
    }
    assert_eq!(
        edge_build("src/a.njk", "{% pnum %}x{% endpnum %}"),
        "undefined"
    );
}

#[test]
fn nunjucks_paired_shortcode_array_and_object_return_print_escaped_to_string() {
    if !has_node() {
        return;
    }
    assert_eq!(
        edge_build(
            "src/a.njk",
            "{% parr %}x{% endparr %}|{% pobj %}x{% endpobj %}|{% pnull %}x{% endpnull %}|{% pasync %}x{% endpasync %}"
        ),
        "&lt;b&gt;,2|[object Object]|undefined|1,2"
    );
}

// Expected from 11ty 3.1.8: src/Util/TransformsUtil.js `runAll` calls each transform in
// registration order with `this = { inputPath, outputPath, url, page }` and `(content,
// outputPath)`, awaiting it (`page.outputFileExtension` is the engine's, `html` for njk, Template.js:449); src/Template.js `#renderPageEntryWithLayoutsAndTransforms` runs
// them on the layout-wrapped content of every output.
const TRANSFORM_CONFIG: &str = r#"module.exports = function (eleventyConfig) {
  eleventyConfig.addTransform("stamp", function (content, outputPath) {
    return content + "<!--" + this.page.outputPath + "|" + outputPath + "|" + this.page.url + "|" + this.page.inputPath + "|" + this.page.outputFileExtension + "-->";
  });
  eleventyConfig.addTransform("shout", async function (content, outputPath) {
    await null;
    return this.outputPath.endsWith(".html") ? content.toUpperCase() : content;
  });
  eleventyConfig.addTransform("boom", function (content) {
    if (content.includes("EXPLODE")) throw new Error("nope");
    return content;
  });
  return { dir: { input: "src" } };
};
"#;

#[test]
fn transforms_run_in_order_on_every_output() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            ("eleventy.config.js", TRANSFORM_CONFIG),
            ("src/a.njk", "a{{ 1 + 1 }}"),
            ("src/b.md", "# b"),
            ("src/feed.njk", "---\npermalink: feed.xml\n---\nx<y>"),
            ("src/off.njk", "---\npermalink: false\n---\nnope"),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o));
    let read = |p: &str| fs::read_to_string(t.path().join(p)).unwrap();
    assert_eq!(
        read("_site/a/index.html"),
        "A2<!--./_SITE/A/INDEX.HTML|./_SITE/A/INDEX.HTML|/A/|./SRC/A.NJK|HTML-->"
    );
    assert_eq!(
        read("_site/b/index.html"),
        "<H1>B</H1>\n<!--./_SITE/B/INDEX.HTML|./_SITE/B/INDEX.HTML|/B/|./SRC/B.MD|HTML-->"
    );
    assert_eq!(
        read("_site/feed.xml"),
        "x<y><!--./_site/feed.xml|./_site/feed.xml|/feed.xml|./src/feed.njk|html-->"
    );
    assert!(!t.path().join("_site/off").exists());
}

#[test]
fn a_throwing_transform_refuses_its_page_naming_the_transform() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            ("eleventy.config.js", TRANSFORM_CONFIG),
            ("src/bad.njk", "EXPLODE"),
            ("src/ok.njk", "fine"),
        ],
        &[],
    );
    let s = text(&o);
    assert!(
        s.contains("boom") && s.contains("nope") && s.contains("eleventy.config.js"),
        "{s}"
    );
    assert!(!t.path().join("_site/bad/index.html").exists());
    assert!(t.path().join("_site/ok/index.html").exists());
}

// The feed plugin's `renderTransforms` runs the site's transforms on every entry (11ty
// 3.1.8 `@11ty/eleventy-plugin-rss`); Brrrty can't yet, so it refuses the feed, not the build.
#[test]
fn a_feed_is_refused_while_the_config_has_transforms() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            (
                "brrrty.config.rhai",
                "config.set_js_config(\"node\");\nconfig.add_plugin(\"rss\", #{ type: \"atom\", output_path: \"/feed.xml\", collection: #{ name: \"posts\" }, metadata: #{ title: \"T\", base: \"https://example.com/\" } });\n",
            ),
            ("eleventy.config.js", TRANSFORM_CONFIG),
            ("src/p.md", "---\ntags: posts\n---\nhi"),
            ("src/ok.njk", "fine"),
        ],
        &[],
    );
    let s = text(&o);
    assert!(s.contains("renderTransforms"), "{s}");
    assert!(!t.path().join("_site/feed.xml").exists());
    assert!(t.path().join("_site/ok/index.html").exists());
}

// 11ty 3.1.8 `UserConfig.addPlugin`: a plugin is a function (or `{ configFunction }`) called
// with the config and its options, after the config function returns, in the order added.
#[test]
fn add_plugin_runs_local_plugins_against_the_recorder() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            (
                "eleventy.config.js",
                r#"const local = require("./plugins/local.js");
module.exports = function (c) {
  c.addPlugin(local, { word: "hi" });
  c.addPlugin({ configFunction(cfg, opts) { cfg.addFilter("viaObject", (s) => s + (opts.tag || "?")); } }, { tag: "!" });
  c.addTransform("direct", (content) => content + "[direct]");
  c.addGlobalData("order", "after");
  return { dir: { input: "src" } };
};
"#,
            ),
            (
                "plugins/local.js",
                r#"module.exports = function (cfg, opts) {
  cfg.addFilter("greet", (s) => opts.word + " " + s);
  cfg.addPassthroughCopy({ "assets": "static" }, { expand: false });
  cfg.addTransform("fromPlugin", (content) => content + "[plugin]");
};
"#,
            ),
            ("assets/a.txt", "asset"),
            (
                "src/a.njk",
                "{{ 'x' | greet }}|{{ 'y' | viaObject }}|{{ order }}",
            ),
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o));
    let a = fs::read_to_string(t.path().join("_site/a/index.html")).unwrap();
    assert_eq!(a, "hi x|y!|after[direct][plugin]");
    assert_eq!(
        fs::read_to_string(t.path().join("_site/static/a.txt")).unwrap(),
        "asset"
    );
}

/// Doctor reports the JS config: mode, config file, each JS function (hottest first, with its
/// calls and time) and each unsupported method with its place.
#[test]
fn doctor_reports_the_js_config_functions_and_unsupported_methods() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
        (
            "eleventy.config.js",
            "module.exports = function (c) {\n  c.addFilter(\"shout\", (s) => String(s).toUpperCase());\n  c.addFilter(\"idle\", (s) => s);\n  c.addShortcode(\"hi\", () => \"hi\");\n  c.addCollection(\"x\", () => []);\n};\n",
        ),
        (
            "index.njk",
            "{{ 'a' | shout }}{{ 'b' | shout }}{{ 'c' | shout }}{% hi %}",
        ),
    ]);
    let out = run(t.path(), "doctor", &[]);
    let s = text(&out);
    assert!(s.contains("JavaScript config: on"), "{s}");
    assert!(s.contains("eleventy.config.js"), "{s}");
    assert!(s.contains("filter shout"), "{s}");
    assert!(
        s.contains("eleventyConfig.addCollection") && s.contains("eleventy.config.js:5"),
        "{s}"
    );
    let json: serde_json::Value =
        serde_json::from_slice(&run(t.path(), "doctor", &["--json"]).stdout).unwrap();
    let js = &json["jsConfig"];
    assert_eq!(js["mode"], "node", "{json}");
    assert_eq!(js["file"], "eleventy.config.js", "{json}");
    let fns = js["functions"].as_array().unwrap();
    assert_eq!(fns[0]["kind"], "filter", "{json}");
    assert_eq!(fns[0]["name"], "shout", "{json}");
    assert_eq!(fns[0]["calls"], 3, "{json}");
    assert!(fns[0]["ms"].is_number(), "{json}");
    let idle = fns.iter().find(|f| f["name"] == "idle").unwrap();
    assert_eq!(idle["calls"], 0, "{json}");
    assert!(
        fns.iter()
            .any(|f| f["kind"] == "shortcode" && f["name"] == "hi")
    );
    let un = js["unsupported"].as_array().unwrap();
    assert_eq!(un[0]["method"], "addCollection", "{json}");
    assert_eq!(un[0]["file"], "eleventy.config.js", "{json}");
    assert_eq!(un[0]["line"], 5, "{json}");
}

/// Without the JS config there is no report.
#[test]
fn doctor_has_no_js_config_section_when_it_is_off() {
    let t = site(&[("index.md", "hi\n")]);
    let json: serde_json::Value =
        serde_json::from_slice(&run(t.path(), "doctor", &["--json"]).stdout).unwrap();
    assert_eq!(json["jsConfig"]["mode"], "off", "{json}");
    assert_eq!(json["jsConfig"]["functions"], serde_json::json!([]));
}

/// A method the JS config can't run and that changes the output blocks the build, wherever the
/// call sits (here: inside a helper module the config calls).
#[test]
fn an_unrun_output_changing_method_blocks_the_build() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            (
                "eleventy.config.js",
                r#"const addHelpers = require("eleventy-helpers"); module.exports = function (c) { addHelpers(c); return { dir: { input: "src" } }; };"#,
            ),
            (
                "node_modules/eleventy-helpers/index.js",
                r#"module.exports = function (c) { c.addFilter("shout", s => s); c.amendLibrary("md", md => md.disable("emphasis")); };"#,
            ),
            ("src/index.md", "*x*"),
        ],
        &[],
    );
    let s = text(&o);
    assert!(!o.status.success(), "{s}");
    assert!(s.contains("amendLibrary"), "{s}");
    assert!(!t.path().join("_site/index.html").exists(), "{s}");
}

/// Any unsupported method blocks unless it's known to leave the pages alone, not only a list of
/// known bad ones.
#[test]
fn an_unlisted_output_changing_method_blocks_the_build() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            (
                "eleventy.config.js",
                r#"const addHelpers = require("eleventy-helpers"); module.exports = function (c) { addHelpers(c); return { dir: { input: "src" } }; };"#,
            ),
            (
                "node_modules/eleventy-helpers/index.js",
                r#"module.exports = function (c) { c.addFilter("shout", s => s.toUpperCase()); c.addNunjucksGlobal("site", "Hello"); };"#,
            ),
            ("src/index.njk", r#"{{ site }}|{{ "x" | shout }}"#),
        ],
        &[],
    );
    let s = text(&o);
    assert!(!o.status.success(), "{s}");
    assert!(s.contains("addNunjucksGlobal"), "{s}");
    assert!(!t.path().join("_site/index.html").exists(), "{s}");
}

/// A method that leaves the pages alone stays a warning.
#[test]
fn a_harmless_unsupported_method_only_warns() {
    if !has_node() {
        return;
    }
    let (t, o) = build_with(
        &[
            (
                "eleventy.config.js",
                r#"module.exports = function (c) { c.addFilter("shout", s => s.toUpperCase()); c.addWatchTarget("./x.css"); return { dir: { input: "src" } }; };"#,
            ),
            ("src/index.njk", r#"{{ "x" | shout }}"#),
        ],
        &[],
    );
    let s = text(&o);
    assert!(o.status.success(), "{s}");
    assert!(s.contains("addWatchTarget"), "{s}");
    assert!(t.path().join("_site/index.html").exists(), "{s}");
}

/// A config that throws still gives doctor a report, with the error as the blocker.
#[test]
fn doctor_json_reports_a_config_that_throws() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
        (
            "eleventy.config.js",
            "module.exports = function () { throw new Error(\"boom\"); };\n",
        ),
        ("index.md", "hi\n"),
    ]);
    let out = run(t.path(), "doctor", &["--json"]);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", text(&out)));
    assert_eq!(json["jsConfig"]["mode"], "node", "{json}");
    assert!(json.to_string().contains("boom"), "{json}");
    assert!(!json["groups"].as_array().unwrap().is_empty(), "{json}");
}

/// 11ty's IdAttributePlugin (no options) in the JS config builds natively: headings get ids,
/// and the plugin's `htmlTransformer` call doesn't refuse the page.
#[test]
fn the_id_attribute_plugin_in_the_js_config_runs_natively() {
    if !has_node() {
        return;
    }
    let plugin = "const POSTHTML_PLUGIN_NAME = \"11ty/eleventy/id-attribute\";\nfunction IdAttributePlugin(c, options = {}) {\n  const sel = \"[id],h1,h2,h3,h4,h5,h6\", checkDuplicates = 1;\n  c.htmlTransformer.addPosthtmlPlugin(\"html\", () => {}, { name: POSTHTML_PLUGIN_NAME });\n}\n";
    let t = site(&[
        ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
        (
            "eleventy.config.js",
            &format!(
                "{plugin}module.exports = (c) => {{ c.addPlugin(IdAttributePlugin); return {{}}; }};\n"
            ),
        ),
        ("index.md", "## Hi there\n"),
    ]);
    let o = run(t.path(), "build", &[]);
    let s = text(&o);
    assert!(o.status.success(), "{s}");
    let html = fs::read_to_string(t.path().join("_site/index.html")).expect(&s);
    assert!(html.contains("<h2 id=\"hi-there\">Hi there</h2>"), "{html}");
}

/// 11ty 3.1.8 keeps transforms in an object, so an integer-like name sorts ahead of the
/// `@11ty/eleventy/html-transformer` one (which adds heading ids): transform `0` sees the
/// page before the ids exist, a named one sees them.
#[test]
fn an_integer_named_transform_runs_before_the_html_transformer() {
    if !has_node() {
        return;
    }
    let plugin = "const POSTHTML_PLUGIN_NAME = \"11ty/eleventy/id-attribute\";\nfunction IdAttributePlugin(c, options = {}) {\n  const sel = \"[id],h1,h2,h3,h4,h5,h6\", checkDuplicates = 1;\n  c.htmlTransformer.addPosthtmlPlugin(\"html\", () => {}, { name: POSTHTML_PLUGIN_NAME });\n}\n";
    let config = format!(
        "{plugin}module.exports = (c) => {{\n  c.addPlugin(IdAttributePlugin);\n  c.addTransform(\"late\", (s) => s.replace(\"<h2 id=\\\"hi\\\">\", \"<h2 late>\"));\n  c.addTransform(\"0\", (s) => s.replace(\"<h2 id=\\\"hi\\\">\", \"<h2 early>\"));\n  return {{}};\n}};\n"
    );
    let t = site(&[
        ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
        ("eleventy.config.js", &config),
        ("index.md", "## Hi\n"),
    ]);
    let o = run(t.path(), "build", &[]);
    let s = text(&o);
    assert!(o.status.success(), "{s}");
    let html = fs::read_to_string(t.path().join("_site/index.html")).expect(&s);
    assert!(html.contains("<h2 late>Hi</h2>"), "{html}");
}
