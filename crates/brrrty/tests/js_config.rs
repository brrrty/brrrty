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
  eleventyConfig.setSomethingNew(true);
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
    assert!(s.contains("setSomethingNew"), "{s}");
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
