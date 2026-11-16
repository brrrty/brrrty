//! What a config file sets, applied to a build: passthrough targets,
//! `ignores` and global data. Each case was probed with 11ty 3.1.8's
//! `eleventyConfig` and pinned here.

use brrrty_core::{BuildOptions, ConfigOptions, PassthroughEntry, Severity, build, codes};
use brrrty_js::Value;
use brrrty_markdown::MarkdownIt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn opts(root: &Path, config: ConfigOptions) -> BuildOptions {
    BuildOptions {
        root: root.to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        config: ConfigOptions {
            file: Some(PathBuf::from("brrrty.config.rhai")),
            ..config
        },
        ..Default::default()
    }
}

fn entry(from: &str, to: Option<&str>) -> PassthroughEntry {
    PassthroughEntry {
        from: from.into(),
        to: to.map(Into::into),
    }
}

/// Every file under `dir`, relative to it, sorted.
fn files(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                out.push(p.strip_prefix(base).unwrap().display().to_string());
            }
        }
    }
    let mut out = Vec::new();
    if dir.exists() {
        walk(dir, dir, &mut out);
    }
    out.sort();
    out
}

#[test]
fn passthrough_targets_are_relative_to_the_output_folder() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/favicon.svg", "svg");
    write(root, "src/dirx/one.txt", "1");
    write(root, "src/dirx/inner/two.txt", "2");
    write(root, "static/robots.txt", "r");
    write(root, "top/t.txt", "t");
    let config = ConfigOptions {
        passthrough: vec![
            // A leading slash is dropped.
            entry("src/favicon.svg", Some("/abs/fav.svg")),
            // A folder's contents go into the target.
            entry("src/dirx", Some("mapped")),
            // A trailing slash on a file's target names the file.
            entry("src/dirx/one.txt", Some("img/")),
            // `/` is the output folder itself.
            entry("static", Some("/")),
            // The target is normalized.
            entry("src/dirx/inner", Some("a/../b")),
            // An empty target is no target: the path is kept.
            entry("top", Some("")),
        ],
        ..Default::default()
    };
    let r = build(&opts(root, config), &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert_eq!(
        files(&root.join("_site")),
        [
            "abs/fav.svg",
            "b/two.txt",
            "img",
            "mapped/inner/two.txt",
            "mapped/one.txt",
            "robots.txt",
            "top/t.txt",
        ]
    );
}

#[test]
fn config_passthrough_comes_before_the_command_line() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.txt", "a");
    write(root, "src/b.txt", "b");
    let mut o = opts(
        root,
        ConfigOptions {
            passthrough: vec![entry("src/a.txt", None)],
            ..Default::default()
        },
    );
    o.passthrough = vec![PathBuf::from("src/b.txt")];
    let r = build(&o, &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert_eq!(files(&root.join("_site")), ["a.txt", "b.txt"]);
}

#[test]
fn passthrough_targets_that_cant_be_copied_are_refused() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.txt", "a");
    write(root, "src/b.txt", "b");
    let config = ConfigOptions {
        passthrough: vec![
            entry("src/*.txt", None),
            entry("src/a.txt", Some("../escaped.txt")),
            entry("src/b.txt", Some("/")),
        ],
        ..Default::default()
    };
    let r = build(&opts(root, config), &MarkdownIt::new());
    let codes: Vec<&str> = r.problems.iter().map(|p| p.code).collect();
    assert_eq!(
        codes,
        [
            codes::PASSTHROUGH_UNSUPPORTED,
            codes::OUTPUT_ESCAPES,
            codes::PASSTHROUGH_UNSUPPORTED
        ],
        "{:?}",
        r.problems
    );
    assert_eq!(
        r.problems[0].path.as_deref(),
        Some(Path::new("brrrty.config.rhai"))
    );
    assert!(!root.join("escaped.txt").exists());
    assert!(files(&root.join("_site")).is_empty());
}

#[test]
fn ignores_are_read_from_the_project_root() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    for page in [
        "src/games/a/readme.md",
        "src/top.md",
        "src/skipme.md",
        "src/sub/x.md",
        "src/sub2/x.md",
        "src/sub3/deep/x.md",
        "src/sub3/y.md",
        "src/keep/z.md",
        "src/any/deep/notes.md",
    ] {
        write(root, page, "# x\n");
    }
    let config = ConfigOptions {
        ignores: [
            "src/games/**/*.md",
            // Not `src/skipme.md` or `src/top.md`: these are from the root.
            "skipme.md",
            "*.md",
            // A folder, however it is written.
            "src/sub",
            "./src/sub2/",
            // A leading slash is dropped.
            "/src/top.md",
            // `*` matches a folder, and the folder is skipped whole.
            "src/sub3/*",
            // `**/` matches anywhere.
            "**/notes.md",
        ]
        .map(String::from)
        .to_vec(),
        ..Default::default()
    };
    let r = build(&opts(root, config), &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert!(
        !r.problems
            .iter()
            .any(|p| p.code == codes::IGNORE_LINE_ANCHORED),
        "{:?}",
        r.problems
    );
    assert_eq!(
        files(&root.join("_site")),
        ["keep/z/index.html", "skipme/index.html"]
    );
}

fn object(pairs: &[(&str, Value)]) -> Value {
    Value::Object(Arc::new(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    ))
}

fn s(v: &str) -> Value {
    Value::String(v.into())
}

#[test]
fn global_data_merges_over_data_files() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/_data/both.json",
        r#"{"fromdata": 1, "both": "data", "obj": {"a": 1, "b": 1}}"#,
    );
    write(root, "src/_data/shared.json", r#"{"k": "data"}"#);
    write(root, "src/_data/arr.json", "[1]");
    write(
        root,
        "src/g.liquid",
        "---\npermalink: g.txt\n---\nboth={{ both | json }} shared={{ shared | json }} arr={{ arr | json }} dotted={{ dotted | json }} cfg={{ cfg }}\n",
    );
    let config = ConfigOptions {
        global_data: [
            (
                "both",
                object(&[
                    ("both", s("config")),
                    (
                        "obj",
                        object(&[("b", Value::Number(2.0)), ("c", Value::Number(2.0))]),
                    ),
                    ("newk", Value::Number(1.0)),
                ]),
            ),
            // A value that isn't an object doesn't replace one from `_data`.
            ("shared", s("scalar")),
            // Lists join, `_data` first.
            ("arr", Value::Array(Arc::new(vec![Value::Number(2.0)]))),
            ("dotted.x", s("cfg")),
            ("dotted.y", s("cfg2")),
            ("cfg", s("only")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect(),
        ..Default::default()
    };
    let r = build(&opts(root, config), &MarkdownIt::new());
    assert!(!r.has_errors(), "{:?}", r.problems);
    assert_eq!(
        fs::read_to_string(root.join("_site/g.txt")).unwrap(),
        r#"both={"fromdata":1,"both":"config","obj":{"a":1,"b":2,"c":2},"newk":1} shared={"k":"data"} arr=[1,2] dotted={"x":"cfg","y":"cfg2"} cfg=only
"#
    );
}

#[test]
fn reserved_global_data_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    let config = ConfigOptions {
        global_data: [("collections".to_string(), Value::Number(1.0))]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    let r = build(&opts(root, config), &MarkdownIt::new());
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::DATA_RESERVED)
        .expect("refused");
    assert_eq!(p.path.as_deref(), Some(Path::new("brrrty.config.rhai")));
}

#[test]
fn a_brrrty_config_replaces_the_eleventy_config_warning() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(root, "src/a.md", "a\n");
    write(root, "eleventy.config.js", "module.exports = () => {};");
    let mut config = ConfigOptions::default();
    config.problems.push(brrrty_core::Problem::warning(
        codes::CONFIG_UNSUPPORTED,
        "from the config",
    ));
    let r = build(&opts(root, config), &MarkdownIt::new());
    let codes: Vec<(&str, Severity)> = r.problems.iter().map(|p| (p.code, p.severity)).collect();
    assert_eq!(codes, [(codes::CONFIG_UNSUPPORTED, Severity::Warning)]);
}
