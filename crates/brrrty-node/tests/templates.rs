//! JavaScript templates in the worker, against real Node: 11ty 3.1.8's
//! `_getInstance`, `getJavaScriptData` and render, `this`, and what the
//! checks shared with data functions catch.

use std::fs;

use brrrty_js::Value;
use brrrty_js::runtime::{JsResult, JsRuntime, LoadedTemplate};
use brrrty_node::{RunMode, Worker, find_node};

mod watch;

fn node() -> Option<brrrty_node::NodeInfo> {
    watch::started();
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

fn ok<T: std::fmt::Debug>(r: JsResult<T>) -> T {
    match r {
        JsResult::Ok(v) => v,
        other => panic!("{other:?}"),
    }
}

fn refuse_all(name: &str, _: &[Value]) -> Result<Value, String> {
    Err(format!("calls `this.{name}` with no filters here"))
}

fn json(v: serde_json::Value) -> Value {
    Value::from_json(&v)
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_string(),
        other => panic!("not text: {other:?}"),
    }
}

fn start(t: &tempfile::TempDir, filters: &[&str]) -> Worker {
    let node = node().expect("node");
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    ok(w.set_argument(&json(serde_json::json!({}))));
    let names: Vec<String> = filters.iter().map(|s| s.to_string()).collect();
    ok(w.template_setup(&names));
    w
}

fn load(w: &mut Worker, t: &tempfile::TempDir, rel: &str) -> LoadedTemplate {
    ok(w.load_template(&t.path().join(rel), false))
}

#[test]
fn every_export_shape_gives_11tys_data_and_render() {
    if node().is_none() {
        return;
    }
    let t = site(&[
        ("s.11ty.js", "module.exports = 'text';"),
        (
            "f.11ty.js",
            "module.exports = (d) => `f ${d.x}`;\nmodule.exports.data = { t: 1 };",
        ),
        (
            "c.11ty.js",
            "module.exports = class { data() { return { t: 2 }; } render(d) { return `c ${d.x}`; } };",
        ),
        (
            "o.11ty.js",
            "let n = 0;\nmodule.exports = { get data() { n += 1; return { n }; }, render: (d) => `o ${d.x}` };",
        ),
        (
            "m.11ty.mjs",
            "export const data = { t: 3 };\nexport default function () { return 'never'; }",
        ),
        (
            "a.11ty.js",
            "module.exports = { async data() { return { t: 4 }; }, async render(d) { return `a ${d.x}`; } };",
        ),
    ]);
    let mut w = start(&t, &[]);
    let data = json(serde_json::json!({"x": "X"}));
    let cases = [
        ("s.11ty.js", serde_json::json!({}), "text"),
        ("f.11ty.js", serde_json::json!({"t": 1}), "f X"),
        ("c.11ty.js", serde_json::json!({"t": 2}), "c X"),
        // The getter is read four times before its value is kept.
        ("o.11ty.js", serde_json::json!({"n": 4}), "o X"),
        // In a CommonJS project an .mjs default plus `data` is an object
        // with no render: it renders "".
        ("m.11ty.mjs", serde_json::json!({"t": 3}), ""),
        ("a.11ty.js", serde_json::json!({"t": 4}), "a X"),
    ];
    for (file, want_data, want) in cases {
        let l = load(&mut w, &t, file);
        assert_eq!(l.data.value, json(want_data), "{file}");
        let r = ok(w.render(l.id, &data, None, &[], &mut refuse_all));
        assert_eq!(text(&r.value), want, "{file}");
    }
}

#[test]
fn eleventys_refusals_are_marked_invalid() {
    if node().is_none() {
        return;
    }
    let t = site(&[
        ("e.11ty.js", "module.exports = {};"),
        (
            "d.11ty.js",
            "module.exports = { data: () => 5, render: () => '' };",
        ),
        ("n.11ty.js", "module.exports = 5;"),
    ]);
    let mut w = start(&t, &[]);
    for (file, says) in [
        (
            "e.11ty.js",
            "No JavaScript template returned from e.11ty.js",
        ),
        (
            "d.11ty.js",
            "Invalid data format returned from d.11ty.js: typeof number",
        ),
        (
            "n.11ty.js",
            "11ty can't make a template of what n.11ty.js exports",
        ),
    ] {
        match w.load_template(&t.path().join(file), false) {
            JsResult::Thrown(e) => {
                assert!(e.invalid, "{file}: {e:?}");
                assert!(e.message.contains(says), "{file}: {}", e.message);
            }
            other => panic!("{file}: {other:?}"),
        }
    }
}

#[test]
fn this_holds_the_first_page_eleventy_and_the_filters() {
    if node().is_none() {
        return;
    }
    let t = site(&[(
        "p.11ty.js",
        "module.exports = function (d) { return [Object.keys(this).join(','), this.page.url, d.page.url, this.eleventy.v, typeof this.slugify].join('|'); };",
    )]);
    let mut w = start(&t, &["slugify", "url"]);
    let l = load(&mut w, &t, "p.11ty.js");
    let page0 = json(serde_json::json!({"url": "/p/0/"}));
    for (n, url) in [(1, "/p/1/"), (0, "/p/0/")] {
        let data = json(serde_json::json!({"page": {"url": url}, "eleventy": {"v": 3}}));
        let r = ok(w.render(l.id, &data, Some(&page0), &[], &mut refuse_all));
        assert_eq!(
            text(&r.value),
            format!("render,page,eleventy,slugify,url|/p/0/|{url}|3|function"),
            "page {n}"
        );
    }
}

#[test]
fn a_filter_call_a_trap_and_a_write_are_refused() {
    if node().is_none() {
        return;
    }
    let t = site(&[
        (
            "f.11ty.js",
            "module.exports = function () { return this.slugify('x'); };",
        ),
        ("t.11ty.js", "module.exports = (d) => String(d.secret);"),
        ("w.11ty.js", "module.exports = (d) => d.list.sort().join();"),
    ]);
    let mut w = start(&t, &["slugify"]);
    let data = json(serde_json::json!({"list": [2, 1]}));
    let f = load(&mut w, &t, "f.11ty.js");
    let tr = load(&mut w, &t, "t.11ty.js");
    let wr = load(&mut w, &t, "w.11ty.js");
    ok(w.begin_calls());
    let refused = |r: JsResult<brrrty_js::runtime::Decoded>| match r {
        JsResult::Refused(r) => r.why,
        other => panic!("{other:?}"),
    };
    assert!(
        refused(w.render(f.id, &data, None, &[], &mut refuse_all)).contains("calls `this.slugify`")
    );
    let traps = [(
        "secret".to_string(),
        "reads `secret`, held back".to_string(),
    )];
    assert!(
        refused(w.render(tr.id, &data, None, &traps, &mut refuse_all)).contains("reads `secret`")
    );
    assert!(
        refused(w.render(wr.id, &data, None, &[], &mut refuse_all))
            .contains("changes the data it is given")
    );
}

#[test]
fn values_that_are_not_text_cross_as_values() {
    if node().is_none() {
        return;
    }
    let t = site(&[
        ("u.11ty.js", "module.exports = () => undefined;"),
        ("n.11ty.js", "module.exports = () => 42;"),
        ("b.11ty.js", "module.exports = () => Buffer.from('buf');"),
        (
            "p.11ty.js",
            "module.exports = () => Promise.resolve('later');",
        ),
    ]);
    let mut w = start(&t, &[]);
    let data = json(serde_json::json!({}));
    for (file, want) in [
        ("u.11ty.js", Value::Undefined),
        ("n.11ty.js", Value::Number(42.0)),
        ("b.11ty.js", Value::String("buf".into())),
        ("p.11ty.js", Value::String("later".into())),
    ] {
        let l = load(&mut w, &t, file);
        assert_eq!(
            ok(w.render(l.id, &data, None, &[], &mut refuse_all)).value,
            want,
            "{file}"
        );
    }
}

#[test]
fn a_layout_traps_this_page() {
    if node().is_none() {
        return;
    }
    let t = site(&[(
        "l.11ty.js",
        "module.exports = { render(d) { return d.content + (d.peek ? this.page.url : ''); } };",
    )]);
    let mut w = start(&t, &[]);
    let l = ok(w.load_template(&t.path().join("l.11ty.js"), true));
    let plain = json(serde_json::json!({"content": "c", "page": {"url": "/a/"}}));
    assert_eq!(
        text(&ok(w.render(l.id, &plain, None, &[], &mut refuse_all)).value),
        "c"
    );
    let peek = json(serde_json::json!({"content": "c", "peek": true, "page": {"url": "/a/"}}));
    match w.render(l.id, &peek, None, &[], &mut refuse_all) {
        JsResult::Refused(r) => assert!(
            r.why.contains("`this.page` in a JavaScript layout"),
            "{}",
            r.why
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_replay_catches_state_between_renders_and_passes_a_cache() {
    if node().is_none() {
        return;
    }
    let t = site(&[
        (
            "count.11ty.js",
            "let n = 0;\nmodule.exports = () => String(++n);",
        ),
        (
            "cache.11ty.js",
            "const seen = new Map();\nmodule.exports = (d) => { if (!seen.has(d.k)) seen.set(d.k, d.k.toUpperCase()); return seen.get(d.k); };",
        ),
    ]);
    // A cache: same replies in any order.
    let mut w = start(&t, &[]);
    let cache = load(&mut w, &t, "cache.11ty.js");
    ok(w.begin_calls());
    for k in ["a", "b", "a"] {
        let _unit = brrrty_js::functions::UnitGuard::enter(format!("content {k}"));
        ok(w.render(
            cache.id,
            &json(serde_json::json!({"k": k})),
            None,
            &[],
            &mut refuse_all,
        ));
    }
    ok(w.verify());
    // A counter: its replies follow the order.
    let mut w = start(&t, &[]);
    let count = load(&mut w, &t, "count.11ty.js");
    ok(w.begin_calls());
    for k in ["a", "b", "c"] {
        let _unit = brrrty_js::functions::UnitGuard::enter(format!("content {k}"));
        ok(w.render(
            count.id,
            &json(serde_json::json!({})),
            None,
            &[],
            &mut refuse_all,
        ));
    }
    match w.verify() {
        JsResult::Refused(r) => assert!(r.impure, "{r:?}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_filter_on_this_runs_in_brrrty_and_the_replay_gets_its_answers() {
    if node().is_none() {
        return;
    }
    let t = site(&[(
        "f.11ty.js",
        "module.exports = function (d) { return this.shout(d.x, '!') + this.shout('b', '?'); };",
    )]);
    let mut w = start(&t, &["shout"]);
    let l = load(&mut w, &t, "f.11ty.js");
    ok(w.begin_calls());
    let mut calls = 0;
    let mut shout = |name: &str, args: &[Value]| {
        calls += 1;
        assert_eq!(name, "shout");
        Ok(Value::String(
            format!("{}{}", text(&args[0]).to_uppercase(), text(&args[1])).into(),
        ))
    };
    for x in ["a", "c"] {
        let _unit = brrrty_js::functions::UnitGuard::enter(format!("content {x}"));
        let r = ok(w.render(
            l.id,
            &json(serde_json::json!({"x": x})),
            None,
            &[],
            &mut shout,
        ));
        assert_eq!(text(&r.value), format!("{}!B?", x.to_uppercase()));
    }
    assert_eq!(calls, 4);
    ok(w.verify());
    // A filter that fails refuses the render.
    let r = w.render(
        l.id,
        &json(serde_json::json!({"x": "a"})),
        None,
        &[],
        &mut refuse_all,
    );
    match r {
        JsResult::Refused(r) => assert!(
            r.why.contains("calls `this.shout` with no filters here"),
            "{}",
            r.why
        ),
        other => panic!("{other:?}"),
    }
}
