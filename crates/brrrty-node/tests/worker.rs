//! The worker and its client, against real Node: lifecycle, framing, crashes
//! and finding Node.

use std::fs;
use std::path::Path;
use std::process::Command;

use brrrty_js::Value;
use brrrty_js::runtime::{JsResult, JsRuntime};
use brrrty_node::{NodeMissing, RunMode, Worker, find_node};

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

fn ok<T: std::fmt::Debug>(r: JsResult<T>) -> T {
    match r {
        JsResult::Ok(v) => v,
        other => panic!("{other:?}"),
    }
}

fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    match v {
        Value::Object(o) => &o[k],
        other => panic!("not an object: {other:?}"),
    }
}

/// Whether a process with this id still runs.
fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .expect("kill")
        .success()
}

#[test]
fn loads_a_file_and_calls_its_function_with_the_argument() {
    let Some(node) = node() else { return };
    let t = site(&[
        ("a.js", "module.exports = { a: 1 };"),
        ("f.js", "module.exports = (arg) => ({ got: arg.x, n: 2 });"),
    ]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    ok(w.set_argument(&Value::from_json(&serde_json::json!({"x": "hi"}))));
    let a = ok(w.load(&t.path().join("a.js"), true));
    assert_eq!(get(&a.value, "a"), &Value::Number(1.0));
    let f = ok(w.load(&t.path().join("f.js"), true));
    assert_eq!(get(&f.value, "got"), &Value::String("hi".into()));
    // Not called: the function stays a handle.
    let raw = ok(w.load(&t.path().join("f.js"), false));
    assert_eq!(raw.functions.len(), 1);
    let called = ok(w.call(
        raw.functions[0].1,
        &[Value::from_json(&serde_json::json!({"x": 5}))],
    ));
    assert_eq!(get(&called.value, "got"), &Value::Number(5.0));
}

#[test]
fn eleventy_environment_variables_are_set() {
    let Some(node) = node() else { return };
    let t = site(&[(
        "e.js",
        "module.exports = { root: process.env.ELEVENTY_ROOT, source: process.env.ELEVENTY_SOURCE, mode: process.env.ELEVENTY_RUN_MODE, v: process.env.ELEVENTY_VERSION, cwd: process.cwd() };",
    )]);
    let mut w = Worker::start(&node, t.path(), RunMode::Serve).expect("starts");
    let e = ok(w.load(&t.path().join("e.js"), true));
    let root = t.path().canonicalize().unwrap();
    assert_eq!(get(&e.value, "source"), &Value::String("cli".into()));
    assert_eq!(get(&e.value, "mode"), &Value::String("serve".into()));
    assert_eq!(get(&e.value, "v"), &Value::String("3.1.8".into()));
    let Value::String(cwd) = get(&e.value, "cwd") else {
        panic!()
    };
    assert_eq!(Path::new(&**cwd).canonicalize().unwrap(), root);
    let Value::String(r) = get(&e.value, "root") else {
        panic!()
    };
    assert_eq!(Path::new(&**r).canonicalize().unwrap(), root);
}

#[test]
fn starting_and_stopping_leaves_no_node_behind() {
    let Some(node) = node() else { return };
    let t = site(&[("a.js", "module.exports = 1;")]);
    let mut pids = Vec::new();
    for _ in 0..200 {
        let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
        ok(w.load(&t.path().join("a.js"), true));
        pids.push(w.pid());
    }
    let left: Vec<u32> = pids.into_iter().filter(|p| alive(*p)).collect();
    assert!(left.is_empty(), "still running: {left:?}");
}

#[test]
fn site_logging_never_breaks_the_frames() {
    let Some(node) = node() else { return };
    let t = site(&[(
        "noisy.js",
        r#"console.log("x".repeat(1 << 20));
process.stdout.write("\x1eBRRRTY 5\nhello");
console.info("info"); console.error("err");
module.exports = { fine: true };"#,
    )]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    let v = ok(w.load(&t.path().join("noisy.js"), true));
    assert_eq!(get(&v.value, "fine"), &Value::Bool(true));
}

#[test]
fn an_exit_from_a_data_file_stops_the_worker_with_its_status_and_stderr() {
    let Some(node) = node() else { return };
    let t = site(&[(
        "exit.js",
        r#"console.error("about to leave"); process.exit(3);"#,
    )]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    match w.load(&t.path().join("exit.js"), true) {
        JsResult::Failed(f) => {
            assert!(f.message.contains("status 3"), "{}", f.message);
            assert!(
                f.stderr_tail.iter().any(|l| l.contains("about to leave")),
                "{:?}",
                f.stderr_tail
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_thrown_error_is_the_sites_error_not_a_crash() {
    let Some(node) = node() else { return };
    let t = site(&[
        ("bad.js", "module.exports = () => { null.x; };"),
        ("ok.js", "module.exports = 1;"),
    ]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    match w.load(&t.path().join("bad.js"), true) {
        JsResult::Thrown(e) => {
            assert_eq!(e.name, "TypeError");
            assert!(e.stack.contains("bad.js"), "{}", e.stack);
        }
        other => panic!("{other:?}"),
    }
    // The worker goes on.
    assert_eq!(
        ok(w.load(&t.path().join("ok.js"), true)).value,
        Value::Number(1.0)
    );
}

#[test]
fn reads_lists_files_and_modules_the_code_touched() {
    let Some(node) = node() else { return };
    let t = site(&[
        ("data.csv", "a,b\n"),
        ("lib/helper.js", "module.exports = 2;"),
        (
            "r.js",
            r#"const fs = require("fs"); const path = require("path");
const h = require("./lib/helper.js");
module.exports = { t: fs.readFileSync(path.join(__dirname, "data.csv"), "utf8"), h };"#,
        ),
    ]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    ok(w.load(&t.path().join("r.js"), true));
    let reads: Vec<_> = ok(w.reads())
        .into_iter()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect();
    let root = t.path().canonicalize().unwrap();
    for f in ["data.csv", "lib/helper.js", "r.js"] {
        assert!(reads.contains(&root.join(f)), "{f} not in {reads:?}");
    }
}

#[test]
fn a_missing_node_is_not_found_and_an_old_one_too_old() {
    match find_node(Some("/nonexistent/node".as_ref())) {
        Err(NodeMissing::NotFound { tried }) => {
            assert_eq!(tried, ["BRRRTY_NODE=/nonexistent/node"]);
        }
        other => panic!("{other:?}"),
    }
    let t = tempfile::tempdir().unwrap();
    let fake = t.path().join("node");
    fs::write(
        &fake,
        "#!/bin/sh\nprintf '[\"v16.20.2\",\"2023c\",\"73.1\",\"undefined\"]'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        match find_node(Some(fake.as_os_str())) {
            Err(NodeMissing::TooOld { version, .. }) => assert_eq!(version, "v16.20.2"),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn a_slow_data_file_is_reported_and_waited_for() {
    let Some(node) = node() else { return };
    let t = site(&[(
        "slow.js",
        "module.exports = async () => { await new Promise((r) => setTimeout(r, 700)); return { done: true }; };",
    )]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let s = seen.clone();
    w.set_notes(
        move |n| s.lock().unwrap().push(n.to_string()),
        std::time::Duration::from_millis(200),
        std::time::Duration::from_millis(200),
    );
    let v = ok(w.load(&t.path().join("slow.js"), true));
    assert_eq!(get(&v.value, "done"), &Value::Bool(true));
    let notes = seen.lock().unwrap().clone();
    assert!(notes.len() >= 2, "{notes:?}");
    assert!(
        notes[0].starts_with("note: still waiting on slow.js"),
        "{notes:?}"
    );
}

#[test]
fn a_raw_write_to_the_output_stops_the_worker_loudly() {
    let Some(node) = node() else { return };
    let t = site(&[(
        "raw.js",
        r#"require("fs").writeSync(1, "garbage\n"); module.exports = 1;"#,
    )]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    match w.load(&t.path().join("raw.js"), true) {
        JsResult::Failed(f) => assert!(
            f.message.contains("something other than the worker"),
            "{}",
            f.message
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_data_function_that_changes_its_argument_stops_the_build() {
    let Some(node) = node() else { return };
    let t = site(&[(
        "m.js",
        "module.exports = (d) => { d.site = { title: \"x\" }; return {}; };",
    )]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    ok(w.set_argument(&Value::from_json(&serde_json::json!({"a": 1}))));
    match w.load(&t.path().join("m.js"), true) {
        JsResult::Failed(f) => assert!(
            f.message.contains("changes the data 11ty hands it"),
            "{}",
            f.message
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn opened_files_are_reads_too() {
    let Some(node) = node() else { return };
    let t = site(&[
        ("rows.csv", "a\n"),
        (
            "o.js",
            "module.exports = async () => { const h = await require(\"fs\").promises.open(require(\"path\").join(__dirname, \"rows.csv\")); const s = await h.readFile(\"utf8\"); await h.close(); return { s }; };",
        ),
    ]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    ok(w.load(&t.path().join("o.js"), true));
    let root = t.path().canonicalize().unwrap();
    let reads: Vec<_> = ok(w.reads())
        .into_iter()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect();
    assert!(reads.contains(&root.join("rows.csv")), "{reads:?}");
}

/// `{c: {c: ... {leaf: "x"}}}` with `levels` objects around the leaf, and
/// a `{children: [...]}` tree as deep.
fn deep_site(levels: usize) -> tempfile::TempDir {
    site(&[(
        "deep.js",
        &format!(
            "let o = {{ leaf: \"x\" }}; for (let i = 0; i < {levels}; i++) o = {{ c: o }};\nlet t = {{ name: \"t\" }}; for (let i = 0; i < {levels}; i++) t = {{ children: [t] }};\nmodule.exports = {{ o, t }};"
        ),
    )])
}

#[test]
fn data_nested_two_hundred_levels_deep_crosses() {
    let Some(node) = node() else { return };
    let t = deep_site(200);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    let d = ok(w.load(&t.path().join("deep.js"), true));
    assert!(d.refused.is_empty(), "{:?}", d.refused);
    let mut v = get(&d.value, "o");
    for _ in 0..200 {
        v = get(v, "c");
    }
    assert_eq!(get(v, "leaf"), &Value::String("x".into()));
    let mut v = get(&d.value, "t");
    for _ in 0..200 {
        v = match get(v, "children") {
            Value::Array(a) => &a[0],
            other => panic!("{other:?}"),
        };
    }
    assert_eq!(get(v, "name"), &Value::String("t".into()));
}

#[test]
fn data_nested_deeper_than_brrrty_reads_is_refused_not_a_crash() {
    let Some(node) = node() else { return };
    let t = deep_site(5000);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    let d = ok(w.load(&t.path().join("deep.js"), true));
    let whats: Vec<&str> = d.refused.iter().map(|(_, w)| w.as_str()).collect();
    assert_eq!(whats.len(), 2, "{whats:?}");
    assert!(
        whats.iter().all(|w| w.contains("nested more than")),
        "{whats:?}"
    );
}
