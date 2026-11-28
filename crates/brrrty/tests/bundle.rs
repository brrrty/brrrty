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

#[test]
fn pruning_reserializes_html_pages_like_posthtml() {
    if !has_node() {
        return;
    }
    let cfg = config(BOTH);
    let t = bundle_site(&[
        ("eleventy.config.js", &cfg),
        (
            "index.njk",
            "<p>a<br/>b</p><input disabled><img src=x />{% css %}a{}{% endcss %}<style>{% getBundle \"css\" %}</style>",
        ),
    ]);
    let o = run(t.path(), "build");
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "<p>a<br>b</p><input disabled=\"\"><img src=\"x\"><style>a{}</style>"
    );
}

#[test]
fn pages_that_are_not_html_are_not_pruned() {
    if !has_node() {
        return;
    }
    let cfg = config(BOTH);
    let t = bundle_site(&[
        ("eleventy.config.js", &cfg),
        (
            "feed.njk",
            "---\npermalink: feed.xml\n---\n<script></script><br/>{% css %}a{}{% endcss %}",
        ),
    ]);
    let o = run(t.path(), "build");
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/feed.xml")).unwrap(),
        "<script></script><br/>"
    );
}

#[test]
fn blank_or_missing_href_and_blank_src_are_pruned() {
    if !has_node() {
        return;
    }
    let cfg = config(BOTH);
    let t = bundle_site(&[
        ("eleventy.config.js", &cfg),
        (
            "index.njk",
            "{% css %}a{}{% endcss %}<link rel=\"stylesheet\"><link rel=\"stylesheet\" href=\" \"><link rel=\"STYLESHEET\" eleventy:keep><link rel=\"stylesheet\" href=\"a.css\"><script src=\" \"></script><script src=\"b.js\"></script><style> </style>",
        ),
    ]);
    let o = run(t.path(), "build");
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "<link rel=\"STYLESHEET\" eleventy:keep=\"\"><link rel=\"stylesheet\" href=\"a.css\"><script src=\"b.js\"></script><style> </style>"
    );
}

/// 11ty resolves bundle placeholders, then runs one posthtml pass (the bundle's pruning,
/// then IdAttributePlugin), so a bundled heading gets its id and a pruned element's id
/// never counts.
#[test]
fn the_id_attribute_plugin_runs_after_bundles_are_resolved_and_pruned() {
    if !has_node() {
        return;
    }
    let cfg = config("eleventyConfig.addBundle(\"html\");");
    let t = site(&[
        (
            "brrrty.config.rhai",
            "config.set_js_config(\"node\");\nconfig.add_plugin(\"idattribute\");\n",
        ),
        ("package.json", "{}"),
        ("eleventy.config.js", &cfg),
        (
            "index.njk",
            "{% html %}<h2>Inside</h2>{% endhtml %}<h2>Outside</h2>{% getBundle \"html\" %}",
        ),
        (
            "pruned.njk",
            "<script id=\"x\" src=\"\"></script><p id=\"x\">kept</p>",
        ),
    ]);
    let o = run(t.path(), "build");
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "<h2 id=\"outside\">Outside</h2><h2 id=\"inside\">Inside</h2>"
    );
    assert_eq!(
        fs::read_to_string(t.path().join("_site/pruned/index.html")).unwrap(),
        "<p id=\"x\">kept</p>"
    );
}

/// An integer-named config transform runs in 11ty before the bundle placeholders are swapped,
/// so it sees `/*__EleventyBundle:get:...*/` text. Brrrty's placeholder is different, so a page
/// with a `getBundle` call and such a transform is refused by name, not built differently.
#[test]
fn an_early_transform_on_a_page_with_a_get_bundle_call_is_refused_by_name() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
        (
            "eleventy.config.js",
            &config(
                "eleventyConfig.addBundle(\"css\");\n  eleventyConfig.addTransform(\"0\", (s) => s.replace(\"x\", \"y\"));",
            ),
        ),
        ("package.json", "{}"),
        (
            "index.njk",
            "{% css %}a{color:red}{% endcss %}\n<style>{% getBundle \"css\" %}</style>",
        ),
    ]);
    let o = run(t.path(), "build");
    let s = text(&o);
    assert!(!t.path().join("_site/index.html").exists(), "{s}");
    assert!(s.contains("getBundle"), "{s}");
    assert!(s.contains("transform"), "{s}");
}

/// U+E000 is a Private Use Area character (icon fonts use it). Without `addBundle` there are
/// no placeholders, so an early transform on a page holding it literally must still run.
#[test]
fn an_early_transform_on_a_bundle_free_page_with_a_private_use_glyph_still_runs() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
        (
            "eleventy.config.js",
            &config("eleventyConfig.addTransform(\"0\", (s) => s.replace(\"x\", \"y\"));"),
        ),
        ("package.json", "{}"),
        ("index.njk", "<i>\u{E000}</i> x"),
    ]);
    let o = run(t.path(), "build");
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "<i>\u{E000}</i> y"
    );
}

/// With bundles on, a page that holds a literal U+E000 but no `getBundle` call has no 11ty
/// placeholder, so an early transform must still run on it.
#[test]
fn an_early_transform_on_a_page_with_a_private_use_glyph_and_no_get_bundle_still_runs() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", "config.set_js_config(\"node\");\n"),
        (
            "eleventy.config.js",
            &config(
                "eleventyConfig.addBundle(\"css\");\n  eleventyConfig.addTransform(\"0\", (s) => s.replace(\"x\", \"y\"));",
            ),
        ),
        ("package.json", "{}"),
        ("index.njk", "<i>\u{E000}</i> x"),
    ]);
    let o = run(t.path(), "build");
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "<i>\u{E000}</i> y"
    );
}
