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

/// 11ty's `processPlugins` runs after the config merged, with `userConfig.pathPrefix` set to
/// the merged `pathPrefix` (default `/`), so plugins like HtmlBasePlugin see a string.
#[test]
fn plugins_see_the_merged_path_prefix() {
    let Some(node) = node() else { return };
    for (returned, want) in [("{}", "/"), (r#"{ pathPrefix: "/blog/" }"#, "/blog/")] {
        let cfg = format!(
            "module.exports = (c) => {{\n  c.addPlugin((p) => {{\n    if (p.pathPrefix === undefined) throw new Error(\"The `baseHref` option is required\");\n    p.addGlobalData(\"seen\", p.pathPrefix);\n  }});\n  return {returned};\n}};\n"
        );
        let t = site(&[("eleventy.config.js", &cfg)]);
        let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
        let d = match w.load_config(&t.path().join("eleventy.config.js")) {
            JsResult::Ok(d) => d,
            other => panic!("{other:?}"),
        };
        let seen = get(get(&d.value, "values"), "globalData");
        assert_eq!(get(seen, "seen"), &s(want));
    }
}

/// 11ty's `processPlugins` also sets `userConfig.dir` to the merged `dir` (defaults plus the
/// returned and exported `dir`), so plugins can read `dir.output` and `dir.input`.
#[test]
fn plugins_see_the_merged_dir() {
    let Some(node) = node() else { return };
    for (returned, want_out, want_in) in [
        ("{}", "_site", "."),
        (r#"{ dir: { input: "src", output: "dist" } }"#, "dist", "src"),
    ] {
        let cfg = format!(
            "module.exports = (c) => {{\n  c.addPlugin((p) => {{\n    p.addGlobalData(\"out\", p.dir.output);\n    p.addGlobalData(\"inp\", p.dir.input);\n  }});\n  return {returned};\n}};\n"
        );
        let t = site(&[("eleventy.config.js", &cfg)]);
        let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
        let d = match w.load_config(&t.path().join("eleventy.config.js")) {
            JsResult::Ok(d) => d,
            other => panic!("{other:?}"),
        };
        let seen = get(get(&d.value, "values"), "globalData");
        assert_eq!(get(seen, "out"), &s(want_out));
        assert_eq!(get(seen, "inp"), &s(want_in));
    }
}

/// 11ty's plugins (HtmlBasePlugin, InputPathToUrlTransformPlugin) call
/// `eleventyConfig.htmlTransformer.addUrlTransform(...)`. The recorder must not throw a
/// TypeError there: it records the call as unsupported, so the build refuses by name.
#[test]
fn html_transformer_calls_are_recorded_not_thrown() {
    let Some(node) = node() else { return };
    let cfg = "module.exports = (c) => {\n  c.addPlugin((p) => {\n    p.htmlTransformer.addUrlTransform([\"html\"], (u) => u, { priority: -1 });\n    p.htmlTransformer.addPosthtmlPlugin(\"html\", () => {});\n  });\n  return {};\n};\n";
    let t = site(&[("eleventy.config.js", cfg)]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    let d = match w.load_config(&t.path().join("eleventy.config.js")) {
        JsResult::Ok(d) => d,
        other => panic!("{other:?}"),
    };
    let methods: Vec<&Value> = list(get(&d.value, "unsupported"))
        .iter()
        .map(|u| get(u, "method"))
        .collect();
    assert_eq!(
        methods,
        [
            &s("htmlTransformer.addUrlTransform"),
            &s("htmlTransformer.addPosthtmlPlugin")
        ]
    );
}

/// 11ty's `IdAttributePlugin` and `HtmlBasePlugin` with no options are what Brrrty runs
/// natively: the recorder records them by name and doesn't run the JS (whose
/// `htmlTransformer` calls would refuse every page). With options, or for a plugin of the
/// same name that isn't 11ty's, it runs the function as before.
#[test]
fn built_in_plugins_are_recorded_by_name() {
    let Some(node) = node() else { return };
    let cfg = r#"const POSTHTML_PLUGIN_NAME = "11ty/eleventy/id-attribute";
function IdAttributePlugin(c, options = {}) {
  const sel = "[id],h1,h2,h3,h4,h5,h6", checkDuplicates = "error";
  c.htmlTransformer.addPosthtmlPlugin("html", () => {}, { name: POSTHTML_PLUGIN_NAME });
}
function eleventyHtmlBasePlugin(c, options = {}) {
  c.addFilter("addPathPrefixToFullUrl", (u) => u);
  c.htmlTransformer.addUrlTransform(["html"], (u) => u, { priority: -1 });
}
function TransformPlugin(c, options = {}) {
  c.on("eleventy.contentMap", () => {});
  c.htmlTransformer.addUrlTransform("html", () => { throw new Error("Internal error: contentMap not available for the `pathToUrl` Transform."); });
}
function Other(c) {
  c.htmlTransformer.addPosthtmlPlugin("html", () => {});
}
module.exports = (c) => {
  c.addPlugin(IdAttributePlugin);
  c.addPlugin(eleventyHtmlBasePlugin, {});
  c.addPlugin(TransformPlugin);
  c.addPlugin(IdAttributePlugin, { selector: "h2" });
  c.addPlugin(Other);
  return {};
};
"#;
    let t = site(&[("eleventy.config.js", cfg)]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    let d = match w.load_config(&t.path().join("eleventy.config.js")) {
        JsResult::Ok(d) => d,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        list(get(get(&d.value, "values"), "nativePlugins")),
        [s("idattribute"), s("htmlbase"), s("inputpathtourl")]
    );
    // Only the call with options and the other plugin ran.
    let methods: Vec<&Value> = list(get(&d.value, "unsupported"))
        .iter()
        .map(|u| get(u, "method"))
        .collect();
    assert_eq!(
        methods,
        [
            &s("htmlTransformer.addPosthtmlPlugin"),
            &s("htmlTransformer.addPosthtmlPlugin")
        ]
    );
    assert!(list(get(&d.value, "functions")).is_empty());
}

/// HtmlBasePlugin is native only under the default `pathPrefix` (`/`): with another, the
/// real plugin runs and its `addUrlTransform` call is recorded, so the build refuses.
#[test]
fn html_base_is_native_only_for_the_default_path_prefix() {
    let Some(node) = node() else { return };
    let cfg = r#"function eleventyHtmlBasePlugin(c, options = {}) {
  c.addFilter("addPathPrefixToFullUrl", (u) => u);
  c.htmlTransformer.addUrlTransform(["html"], (u) => u, { priority: -1 });
}
module.exports = (c) => {
  c.addPlugin(eleventyHtmlBasePlugin);
  return { pathPrefix: "/blog/" };
};
"#;
    let t = site(&[("eleventy.config.js", cfg)]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    let d = match w.load_config(&t.path().join("eleventy.config.js")) {
        JsResult::Ok(d) => d,
        other => panic!("{other:?}"),
    };
    match get(&d.value, "values") {
        Value::Object(o) => assert!(o.get("nativePlugins").is_none(), "{o:?}"),
        other => panic!("{other:?}"),
    }
    let methods: Vec<&Value> = list(get(&d.value, "unsupported"))
        .iter()
        .map(|u| get(u, "method"))
        .collect();
    assert_eq!(methods, [&s("htmlTransformer.addUrlTransform")]);
}
