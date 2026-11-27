//! 11ty's bundle plugin, `eleventyConfig.addBundle`: `{% css %}` collects code, `getBundle` prints it.

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

fn run(dir: &Path, cmd: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .current_dir(dir)
        .arg(cmd)
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

fn config(add: &str) -> String {
    format!("module.exports = function (eleventyConfig) {{\n  {add}\n}};\n")
}

#[test]
fn a_page_prints_the_css_it_collected() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
        (
            "eleventy.config.js",
            &config("eleventyConfig.addBundle(\"css\");"),
        ),
        ("package.json", "{}"),
        (
            "index.njk",
            "{% css %}a{color:red}{% endcss %}\n<style>{% getBundle \"css\" %}</style>",
        ),
        (
            "other.njk",
            "<style>{% getBundle \"css\" %}</style>{% css %}b{}{% endcss %}{% css %}b{}{% endcss %}{% css %}c{}{% endcss %}",
        ),
    ]);
    let o = run(t.path(), "build");
    assert!(o.status.success(), "{}", text(&o));
    let out = t.path().join("_site");
    assert_eq!(
        fs::read_to_string(out.join("index.html")).unwrap(),
        "\n<style>a{color:red}</style>"
    );
    // Code collected after `getBundle` is in, each entry once, and one page's code stays its own.
    assert_eq!(
        fs::read_to_string(out.join("other/index.html")).unwrap(),
        "<style>b{}\nc{}</style>"
    );
}

#[test]
fn bundle_options_are_refused_by_name() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
        (
            "eleventy.config.js",
            &config("eleventyConfig.addBundle(\"css\", { toFileDirectory: \"bundle\" });"),
        ),
        ("package.json", "{}"),
        ("index.njk", "x"),
    ]);
    let o = run(t.path(), "build");
    assert!(!o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("addBundle"), "{}", text(&o));
}

fn bundle_site(files: &[(&str, &str)]) -> tempfile::TempDir {
    let mut all = vec![
        ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
        ("package.json", "{}"),
    ];
    all.extend_from_slice(files);
    site(&all)
}

const BOTH: &str = "eleventyConfig.addBundle(\"css\"); eleventyConfig.addBundle(\"js\");";

#[test]
fn entries_are_trimmed_empty_ones_dropped_and_deduplicated_after_the_trim() {
    if !has_node() {
        return;
    }
    let cfg = config(BOTH);
    let t = bundle_site(&[
        ("eleventy.config.js", &cfg),
        (
            "index.njk",
            "{% css %}\n  a { color: red }\n{% endcss %}{% css %}   {% endcss %}{% css %} a { color: red } {% endcss %}<style>{% getBundle \"css\" %}</style>",
        ),
    ]);
    let o = run(t.path(), "build");
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "<style>a { color: red }</style>"
    );
}

#[test]
fn empty_bundle_tags_are_pruned_unless_kept() {
    if !has_node() {
        return;
    }
    let cfg = config(BOTH);
    let t = bundle_site(&[
        ("eleventy.config.js", &cfg),
        (
            "index.njk",
            "<style>{% getBundle \"css\" %}</style><script>{% getBundle \"js\" %}</script><style eleventy:keep>{% getBundle \"css\" %}</style><script src=\"a.js\"></script><script src=\"\"></script><link rel=\"stylesheet\" href=\"\"><p>x</p>",
        ),
    ]);
    let o = run(t.path(), "build");
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "<style></style><script src=\"a.js\"></script><p>x</p>"
    );
}

#[test]
fn comma_buckets_add_to_each_and_get_returns_each() {
    if !has_node() {
        return;
    }
    let cfg = config(BOTH);
    let t = bundle_site(&[
        ("eleventy.config.js", &cfg),
        (
            "index.njk",
            "{% css \"a,b\" %}x{}{% endcss %}{% css \"b\" %}y{}{% endcss %}[{% getBundle \"css\", \"a\" %}][{% getBundle \"css\", \"b\" %}][{% getBundle \"css\", \"a,b\" %}]",
        ),
    ]);
    let o = run(t.path(), "build");
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "[x{}][x{}\ny{}][x{}\ny{}]"
    );
}

#[test]
fn liquid_bundles_are_refused_by_name() {
    if !has_node() {
        return;
    }
    let cfg = config(BOTH);
    let t = bundle_site(&[
        ("eleventy.config.js", &cfg),
        ("index.liquid", "{% css %}a{}{% endcss %}"),
    ]);
    let o = run(t.path(), "build");
    assert!(!o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("Nunjucks-only"), "{}", text(&o));
}
