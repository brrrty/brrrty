//! Loading the site's 11ty config in Node against a recording
//! `eleventyConfig`.

use std::fs;

use brrrty_js::Value;
use brrrty_js::runtime::{JsResult, JsRuntime, PathPart};
use brrrty_node::{RunMode, Worker, find_node};

fn node() -> Option<brrrty_node::NodeInfo> {
    match find_node(None) {
        Ok(n) => Some(n),
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("CI needs Node for these tests: {e}");
            }
            eprintln!("skipped: {e}");
            None
        }
    }
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

fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    match v {
        Value::Object(o) => o.get(k).unwrap_or_else(|| panic!("no `{k}` in {v:?}")),
        other => panic!("not an object: {other:?}"),
    }
}

fn list(v: &Value) -> &[Value] {
    match v {
        Value::Array(a) => a,
        other => panic!("not an array: {other:?}"),
    }
}

fn s(v: &str) -> Value {
    Value::String(v.into())
}

const CONFIG: &str = r#"module.exports = function (eleventyConfig) {
  eleventyConfig.addPassthroughCopy("a").addPassthroughCopy("b");
  require("./_config/filters.js")(eleventyConfig);
  eleventyConfig.setTemplateFormats(["njk", "md"]);
  eleventyConfig.addGlobalData("x", 1);
  eleventyConfig.setSomethingNew(true);
  return { dir: { input: "src" } };
};
"#;

const FILTERS: &str = r#"module.exports = (eleventyConfig) => {
  eleventyConfig.addFilter("upper", function upper(s) { return String(s).toUpperCase(); });
  eleventyConfig.addAsyncFilter("slow", async function slow(s) { return s; });
};
"#;

#[test]
fn the_config_loads_against_a_recorder_and_replies_with_what_it_set() {
    let Some(node) = node() else { return };
    let t = site(&[
        ("eleventy.config.js", CONFIG),
        ("_config/filters.js", FILTERS),
    ]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    let d = match w.load_config(&t.path().join("eleventy.config.js")) {
        JsResult::Ok(d) => d,
        other => panic!("{other:?}"),
    };
    let v = &d.value;
    let values = get(v, "values");
    let copies: Vec<&Value> = list(get(values, "passthroughCopy"))
        .iter()
        .map(|c| get(c, "input"))
        .collect();
    assert_eq!(copies, [&s("a"), &s("b")]);
    assert_eq!(
        get(values, "templateFormats"),
        &Value::Array(vec![s("njk"), s("md")].into())
    );
    assert_eq!(get(get(values, "globalData"), "x"), &Value::Number(1.0));
    let dir = get(get(v, "returned"), "dir");
    assert_eq!(get(dir, "input"), &s("src"));

    let funcs = list(get(v, "functions"));
    let names: Vec<&Value> = funcs.iter().map(|f| get(f, "name")).collect();
    assert_eq!(names, [&s("upper"), &s("slow")]);
    assert_eq!(get(&funcs[0], "kind"), &s("filter"));
    assert_eq!(get(&funcs[1], "async"), &Value::Bool(true));
    // Each function is a handle, at `functions[i].fn`.
    assert_eq!(d.functions.len(), 2);
    assert_eq!(
        d.functions[0].0,
        vec![
            PathPart::Key("functions".into()),
            PathPart::Index(0),
            PathPart::Key("fn".into())
        ]
    );

    let un = list(get(v, "unsupported"));
    assert_eq!(un.len(), 1);
    assert_eq!(get(&un[0], "method"), &s("setSomethingNew"));
    assert_eq!(get(&un[0], "at"), &s("eleventy.config.js:6"));
}

#[test]
fn a_config_that_throws_refuses_with_the_file_and_line() {
    let Some(node) = node() else { return };
    let t = site(&[(
        "eleventy.config.js",
        "module.exports = function (eleventyConfig) {\n  eleventyConfig.addGlobalData(\"x\", 1);\n  throw new Error(\"boom\");\n};\n",
    )]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    match w.load_config(&t.path().join("eleventy.config.js")) {
        JsResult::Thrown(e) => {
            assert_eq!(e.message, "boom");
            assert_eq!(e.line, Some(3));
            assert!(
                e.file
                    .as_ref()
                    .is_some_and(|f| f.ends_with("eleventy.config.js")),
                "{:?}",
                e.file
            );
        }
        other => panic!("{other:?}"),
    }
}
