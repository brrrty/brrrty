//! JavaScript data files through the `brrrty` binary: on, off, without Node,
//! errors, values that can't cross, and stopping.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

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

fn build(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brrrty"));
    cmd.current_dir(dir)
        .args(["build", "--input", "src"])
        .args(args)
        .env_remove("BRRRTY_NODE");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("run brrrty")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

const ON: &str = "config.set_js_data(\"node\");\n";

#[test]
fn with_node_on_a_global_data_file_builds_its_pages() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_data/site.js",
            "module.exports = async () => ({ title: \"Hi\" });",
        ),
        ("src/index.njk", "{{ site.title }}"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "Hi"
    );
}

#[test]
fn off_refuses_the_page_and_names_both_fixes() {
    let t = site(&[
        ("src/_data/site.js", "module.exports = { title: \"Hi\" };"),
        ("src/index.njk", "{{ site.title }}"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1));
    let out = text(&o);
    assert!(out.contains("data-js-unsupported"), "{out}");
    assert!(out.contains("config.set_js_data(\"node\")"), "{out}");
    assert!(out.contains("site.rhai"), "{out}");
    // The flag turns it off for one run too.
    let t = site(&[
        ("brrrty.config.rhai", ON),
        ("src/_data/site.js", "module.exports = { title: \"Hi\" };"),
        ("src/index.njk", "{{ site.title }}"),
    ]);
    let o = build(t.path(), &["--js-data=off"], &[]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(!t.path().join("_site/index.html").exists());
}

#[test]
fn on_without_node_says_so_and_refuses_only_the_pages_that_need_it() {
    let t = site(&[
        ("brrrty.config.rhai", ON),
        ("src/_data/site.js", "module.exports = { title: \"Hi\" };"),
        ("src/index.njk", "{{ site.title }}"),
        ("src/other.njk", "plain"),
    ]);
    let o = build(t.path(), &[], &[("BRRRTY_NODE", "/nonexistent/node")]);
    assert_eq!(o.status.code(), Some(1));
    let out = text(&o);
    assert!(out.contains("js-data-node-missing"), "{out}");
    assert!(out.contains("BRRRTY_NODE=/nonexistent/node"), "{out}");
    assert!(!t.path().join("_site/index.html").exists());
    assert_eq!(
        fs::read_to_string(t.path().join("_site/other/index.html")).unwrap(),
        "plain"
    );
}

#[test]
fn a_rhai_port_beside_the_file_still_wins() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        ("src/_data/site.js", "throw new Error(\"never run\");"),
        ("src/_data/site.rhai", "#{ title: \"from rhai\" }"),
        ("src/index.njk", "{{ site.title }}"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(
        text(&o).contains("site.rhai supplies this key"),
        "{}",
        text(&o)
    );
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "from rhai"
    );
}

#[test]
fn a_thrown_error_names_the_file_and_other_pages_build() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_data/posts.js",
            "module.exports = () => { const x = undefined; return x.map((p) => p); };",
        ),
        ("src/index.njk", "{{ posts }}"),
        ("src/other.njk", "plain"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1));
    let out = text(&o);
    assert!(out.contains("js-data-error"), "{out}");
    assert!(out.contains("src/_data/posts.js"), "{out}");
    assert!(out.contains("TypeError"), "{out}");
    assert!(!t.path().join("_site/index.html").exists());
    assert!(t.path().join("_site/other/index.html").exists());
}

#[test]
fn a_value_that_cannot_cross_refuses_the_pages_that_read_it() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_data/stuff.js",
            "module.exports = { posts: [{ title: \"a\" }, { when: new Map() }] };",
        ),
        ("src/_data/fine.js", "module.exports = { ok: 1 };"),
        ("src/reads.njk", "{{ stuff.posts[0].title }}"),
        ("src/other.njk", "{{ fine.ok }}"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1));
    let out = text(&o);
    assert!(out.contains("js-data-value"), "{out}");
    assert!(out.contains("data.posts[1].when is a Map object"), "{out}");
    assert!(!t.path().join("_site/reads/index.html").exists());
    assert_eq!(
        fs::read_to_string(t.path().join("_site/other/index.html")).unwrap(),
        "1"
    );
}

#[test]
fn a_data_file_that_exits_stops_the_build_with_its_status() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_data/bye.js",
            "console.error(\"leaving now\"); process.exit(4);",
        ),
        ("src/index.njk", "plain"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1));
    let out = text(&o);
    assert!(out.contains("js-worker-failed"), "{out}");
    assert!(out.contains("status 4"), "{out}");
    assert!(out.contains("leaving now"), "{out}");
}

/// Whether a process with this id still runs.
fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(unix)]
#[test]
fn ctrl_c_mid_load_leaves_no_node_behind() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_data/hang.js",
            "require(\"fs\").writeFileSync(require(\"path\").join(__dirname, \"..\", \"..\", \"pid\"), String(process.pid));\nmodule.exports = () => new Promise(() => setInterval(() => {}, 1000));",
        ),
        ("src/index.njk", "{{ hang }}"),
    ]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .current_dir(t.path())
        .args(["build", "--input", "src"])
        .env_remove("BRRRTY_NODE")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid_file = t.path().join("pid");
    let start = Instant::now();
    while !pid_file.exists() && start.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(100));
    let node_pid = fs::read_to_string(&pid_file).expect("the data file ran");
    assert!(alive(&node_pid));
    Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    let _ = child.wait();
    let start = Instant::now();
    while alive(&node_pid) && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(&node_pid), "Node {node_pid} outlived brrrty");
}

#[cfg(unix)]
#[test]
fn ctrl_c_stops_a_data_file_stuck_in_a_loop() {
    signal_stops_a_data_file_stuck_in_a_loop("-INT");
}

#[cfg(unix)]
#[test]
fn a_hangup_stops_a_data_file_stuck_in_a_loop() {
    signal_stops_a_data_file_stuck_in_a_loop("-HUP");
    signal_stops_a_data_file_stuck_in_a_loop("-QUIT");
}

#[cfg(unix)]
fn signal_stops_a_data_file_stuck_in_a_loop(signal: &str) {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_data/spin.js",
            "require(\"fs\").writeFileSync(require(\"path\").join(__dirname, \"..\", \"..\", \"pid\"), String(process.pid));\nfor (;;) {}",
        ),
        ("src/index.njk", "{{ spin }}"),
    ]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .current_dir(t.path())
        .args(["build", "--input", "src"])
        .env_remove("BRRRTY_NODE")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid_file = t.path().join("pid");
    let start = Instant::now();
    while !pid_file.exists() && start.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(100));
    let node_pid = fs::read_to_string(&pid_file).expect("the data file ran");
    assert!(alive(&node_pid));
    Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .unwrap();
    let _ = child.wait();
    let start = Instant::now();
    while alive(&node_pid) && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let survived = alive(&node_pid);
    if survived {
        let _ = Command::new("kill").args(["-9", &node_pid]).status();
    }
    assert!(!survived, "Node {node_pid} outlived brrrty");
}

#[cfg(unix)]
#[test]
fn a_build_started_ignoring_ctrl_c_keeps_ignoring_it() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_data/slow.js",
            "require(\"fs\").writeFileSync(require(\"path\").join(__dirname, \"..\", \"..\", \"pid\"), String(process.pid));\nmodule.exports = () => new Promise((r) => setTimeout(() => r({ a: 1 }), 1500));",
        ),
        ("src/index.njk", "{{ slow.a }}"),
    ]);
    let bin = env!("CARGO_BIN_EXE_brrrty");
    let mut child = Command::new("sh")
        .args([
            "-c",
            &format!("trap '' INT; exec '{bin}' build --input src"),
        ])
        .current_dir(t.path())
        .env_remove("BRRRTY_NODE")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid_file = t.path().join("pid");
    let start = Instant::now();
    while !pid_file.exists() && start.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(100));
    Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "{status:?}");
    assert_eq!(
        fs::read_to_string(t.path().join("_site/index.html")).unwrap(),
        "1"
    );
}

#[cfg(unix)]
#[test]
fn a_process_a_data_file_leaves_running_does_not_keep_the_build_open() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_data/kid.js",
            "require(\"child_process\").spawn(\"sleep\", [\"8\"], { stdio: \"inherit\", detached: true }).unref();\nmodule.exports = { a: 1 };",
        ),
        ("src/index.njk", "{{ kid.a }}"),
    ]);
    let start = Instant::now();
    let o = build(t.path(), &[], &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "took {:?}",
        start.elapsed()
    );
}

/// A withheld key read through another page's `data`: a collection loop,
/// a pagination alias and a permalink over the items.
fn reads_through_items(on: bool) -> (Output, tempfile::TempDir) {
    let config = if on { ON } else { "" };
    let t = site(&[
        ("brrrty.config.rhai", config),
        (
            "src/_data/v.js",
            "module.exports = { m: new Map(), t: \"x\" };",
        ),
        (
            "src/index.njk",
            "---\ntags: post\n---\n{% for p in collections.post %}[{{ p.data.v.t }}]{% endfor %}",
        ),
        ("src/other.md", "---\ntags: post\nlayout: false\n---\nhi"),
        (
            "src/pg.njk",
            "---\npagination:\n  data: collections.post\n  size: 1\n  alias: it\npermalink: \"pg/{{ pagination.pageNumber }}/\"\n---\n[{{ it.data.v.t }}]",
        ),
        (
            "src/link.njk",
            "---\npagination:\n  data: collections.post\n  size: 1\n  alias: it\npermalink: \"link/{{ it.data.v.t }}{{ pagination.pageNumber }}/\"\n---\nlink",
        ),
        (
            "src/fine.njk",
            "{% for p in collections.post %}[{{ p.data.title }}{{ p.url }}]{% endfor %}",
        ),
    ]);
    (build(t.path(), &[], &[]), t)
}

fn check_reads_through_items(on: bool) {
    let (o, t) = reads_through_items(on);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    for page in ["index/index.html", "pg/0/index.html", "pg/1/index.html"] {
        assert!(!t.path().join("_site").join(page).exists(), "{page}: {out}");
    }
    assert!(!t.path().join("_site/link").exists(), "{out}");
    assert!(out.contains("src/index.njk"), "{out}");
    assert!(out.contains("src/pg.njk"), "{out}");
    assert!(out.contains("src/link.njk"), "{out}");
    assert!(out.contains("_data/v.js"), "{out}");
    assert!(
        out.contains("src/fine.njk"),
        "a pagination that fails leaves the collections incomplete: {out}"
    );
}

#[test]
fn a_withheld_key_read_through_collection_items_refuses_the_page_with_node() {
    if !has_node() {
        return;
    }
    check_reads_through_items(true);
}

#[test]
fn a_withheld_key_read_through_collection_items_refuses_the_page_without_node() {
    check_reads_through_items(false);
}

#[test]
fn a_proxy_of_a_plain_object_is_merged_as_11ty_merges_it() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/posts/posts.11tydata.js",
            "module.exports = new Proxy({ title: \"T\" }, {});",
        ),
        (
            "src/_data/p.js",
            "module.exports = new Proxy({ t: \"x\" }, {});",
        ),
        ("src/posts/a.njk", "[{{ title }}] [{{ p.t }}]"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(!text(&o).contains("js-data-not-object"), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/posts/a/index.html")).unwrap(),
        "[T] [x]"
    );
}

#[test]
fn a_local_data_object_that_cannot_cross_is_refused_not_ignored() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/posts/posts.11tydata.js",
            "const o = { title: \"T\" }; Object.defineProperty(o, \"hid\", { value: 1 }); module.exports = { inner: Object.freeze(o) };",
        ),
        (
            "src/posts/root.11tydata.js",
            "const o = { title: \"T\" }; Object.defineProperty(o, \"hid\", { value: 1 }); module.exports = Object.freeze(o);",
        ),
        ("src/posts/a.njk", "[{{ inner.hid }}]"),
        ("src/posts/root.njk", "[{{ title }}]"),
    ]);
    let o = build(t.path(), &[], &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(!out.contains("js-data-not-object"), "{out}");
    assert!(!t.path().join("_site/posts/a/index.html").exists(), "{out}");
    assert!(
        !t.path().join("_site/posts/root/index.html").exists(),
        "{out}"
    );
}

#[test]
fn a_local_data_function_runs_once_for_each_template_it_applies_to() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/posts/posts.11tydata.js",
            "const fs = require(\"fs\");\nmodule.exports = () => { fs.appendFileSync(require(\"path\").join(__dirname, \"..\", \"..\", \"calls\"), \"x\\n\"); return { tags: \"post\", section: \"posts\" }; };",
        ),
        (
            "src/posts/a.njk",
            "a {{ section }} {{ tags | dump | safe }}",
        ),
        ("src/posts/b.njk", "b {{ section }}"),
        ("src/posts/c.njk", "c {{ section }}"),
        ("src/top.njk", "[{{ section }}]"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert!(o.status.success(), "{}", text(&o));
    // Once per template, and once more in each of the two checks that load
    // the data again in another order.
    assert_eq!(
        fs::read_to_string(t.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        9
    );
    assert_eq!(
        fs::read_to_string(t.path().join("_site/posts/a/index.html")).unwrap(),
        "a posts [\"post\"]"
    );
    assert_eq!(
        fs::read_to_string(t.path().join("_site/top/index.html")).unwrap(),
        "[]"
    );
}

#[test]
fn a_local_data_function_that_changes_between_templates_is_refused() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/posts/posts.11tydata.js",
            "let n = 0;\nmodule.exports = () => ({ n: ++n });",
        ),
        ("src/posts/a.njk", "{{ n }}"),
        ("src/posts/b.njk", "{{ n }}"),
        ("src/other.njk", "plain"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1));
    let out = text(&o);
    assert!(out.contains("js-data-impure"), "{out}");
    assert!(
        out.contains("returned different data for src/posts/a.njk and src/posts/b.njk"),
        "{out}"
    );
    assert!(!t.path().join("_site/posts/a/index.html").exists());
    assert!(t.path().join("_site/other/index.html").exists());
}

#[test]
fn a_local_data_file_that_is_not_an_object_is_skipped_with_a_warning() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        ("src/x.11tydata.js", "module.exports = [1, 2];"),
        ("src/x.njk", "[{{ title }}]"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("js-data-not-object"), "{}", text(&o));
    assert_eq!(
        fs::read_to_string(t.path().join("_site/x/index.html")).unwrap(),
        "[]"
    );
}

/// The build's output for one global data file `src/_data/d.<ext>` that a
/// page reads, plus `extra` files.
fn failing(ext: &str, body: &str, extra: &[(&str, &str)]) -> String {
    let name = format!("src/_data/d.{ext}");
    let mut files = vec![
        ("brrrty.config.rhai", ON),
        (name.as_str(), body),
        ("src/index.njk", "{{ d }}"),
    ];
    files.extend_from_slice(extra);
    let t = site(&files);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    text(&o)
}

#[test]
fn a_sync_throw_is_reported_at_its_line_and_column() {
    if !has_node() {
        return;
    }
    let out = failing(
        "js",
        "module.exports = () => {\n  const posts = undefined;\n  return posts.map((p) => p);\n};\n",
        &[],
    );
    assert!(out.contains("js-data-error"), "{out}");
    assert!(out.contains("[src/_data/d.js:3:16]"), "{out}");
    assert!(
        out.contains("TypeError: Cannot read properties of undefined (reading 'map')"),
        "{out}"
    );
}

#[test]
fn an_async_rejection_is_reported_at_its_line() {
    if !has_node() {
        return;
    }
    let out = failing(
        "js",
        "module.exports = async () => {\n  await null;\n  throw new RangeError(\"too far\");\n};\n",
        &[],
    );
    assert!(out.contains("[src/_data/d.js:3:9]"), "{out}");
    assert!(out.contains("RangeError: too far"), "{out}");
}

#[test]
fn syntax_errors_are_reported_at_their_line() {
    if !has_node() {
        return;
    }
    let cjs = failing("js", "module.exports = {\n  a: 1,\n  b: ,\n};\n", &[]);
    assert!(cjs.contains("[src/_data/d.js:3"), "{cjs}");
    assert!(cjs.contains("SyntaxError"), "{cjs}");
    let esm = failing("mjs", "export default {\n  a: 1\n  b: 2,\n};\n", &[]);
    assert!(esm.contains("[src/_data/d.mjs:3"), "{esm}");
    assert!(esm.contains("SyntaxError"), "{esm}");
}

#[test]
fn an_error_in_a_required_helper_is_reported_there() {
    if !has_node() {
        return;
    }
    let out = failing(
        "js",
        "const h = require(\"../../lib/h.js\");\nmodule.exports = () => h.run();\n",
        &[(
            "lib/h.js",
            "exports.run = function () {\n  return null.x;\n};\n",
        )],
    );
    assert!(out.contains("[lib/h.js:2:15]"), "{out}");
    assert!(out.contains("src/_data/d.js"), "{out}");
}

#[test]
fn an_error_inside_a_package_is_reported_where_the_data_file_calls_it() {
    if !has_node() {
        return;
    }
    let out = failing(
        "js",
        "const t = require(\"thrower\");\nmodule.exports = () => {\n  return t();\n};\n",
        &[(
            "node_modules/thrower/index.js",
            "module.exports = function () { throw new Error(\"inside\"); };\n",
        )],
    );
    assert!(out.contains("[src/_data/d.js:3:10]"), "{out}");
    assert!(out.contains("Error: inside"), "{out}");
}

#[test]
fn a_thrown_non_error_is_reported_at_the_data_file() {
    if !has_node() {
        return;
    }
    let out = failing("js", "throw \"plain text\";\n", &[]);
    assert!(out.contains("Uncaught \"plain text\""), "{out}");
    assert!(out.contains("src/_data/d.js"), "{out}");
}

#[test]
fn collections_with_local_javascript_data_build_and_their_order_is_refused() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/posts/posts.11tydata.js",
            "module.exports = { tags: \"post\" };",
        ),
        ("src/posts/a.md", "---\ndate: 2024-01-01\n---\na"),
        ("src/posts/b.md", "---\ndate: 2024-01-02\n---\nb"),
        (
            "src/list.njk",
            "{% for p in collections.post %}{{ p.url }}{% endfor %}",
        ),
        (
            "src/keys.njk",
            "{% for k, v in collections %}{{ k }}{% endfor %}",
        ),
    ]);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1));
    let out = text(&o);
    assert!(out.contains("by file-read timing"), "{out}");
    assert_eq!(
        written(&t, "list/index.html").as_deref(),
        Some("/posts/a//posts/b/")
    );
    assert_eq!(written(&t, "keys/index.html"), None);
}

/// What the build wrote at `rel` under `_site`, or nothing.
fn written(t: &tempfile::TempDir, rel: &str) -> Option<String> {
    fs::read_to_string(t.path().join("_site").join(rel)).ok()
}

#[test]
fn a_function_that_keeps_state_writes_nothing_and_names_its_line() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_data/ids.js",
            "let n = 0;\nmodule.exports = {\n  next: () => ++n,\n};\n",
        ),
        ("src/a.njk", "{{ ids.next() }}"),
        ("src/b.njk", "{{ ids.next() }}"),
        ("src/c.njk", "plain"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let out = text(&o);
    assert!(out.contains("js-data-impure"), "{out}");
    assert!(out.contains("`next`"), "{out}");
    assert!(out.contains("src/_data/ids.js:3:"), "{out}");
    // Any page may hold a result from it: nothing is written.
    assert_eq!(written(&t, "c/index.html"), None);
}

#[test]
fn a_function_that_remembers_earlier_calls_is_caught_across_pages() {
    if !has_node() {
        return;
    }
    // Each page sees "new" the first time it asks: the second call of the
    // same arguments differs.
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_data/seen.js",
            "const s = new Set();\nmodule.exports = { mark: (x) => (s.has(x) ? \"again\" : (s.add(x), \"new\")) };\n",
        ),
        ("src/a.njk", "{{ seen.mark(\"x\") }}"),
        ("src/b.njk", "{{ seen.mark(\"x\") }}"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("js-data-impure"), "{}", text(&o));
    assert_eq!(written(&t, "a/index.html"), None);
    assert_eq!(written(&t, "b/index.html"), None);
}

#[test]
fn computed_functions_refuse_what_brrrty_cannot_hand_them() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/this/this.11tydata.js",
            "module.exports = { eleventyComputed: { t: function (d) { return typeof this.slugify; } } };",
        ),
        ("src/this/page.njk", "{{ t }}"),
        (
            "src/coll/coll.11tydata.js",
            "module.exports = { eleventyComputed: { n: (d) => d.collections.all.length } };",
        ),
        ("src/coll/page.njk", "{{ n }}"),
        (
            "src/poke/poke.11tydata.js",
            "module.exports = { eleventyComputed: { n: (d) => { d.extra = 1; return 1; } } };",
        ),
        ("src/poke/page.njk", "{{ n }}"),
        (
            "src/throw/throw.11tydata.js",
            "module.exports = {\n  eleventyComputed: {\n    n: (d) => d.missing.deep,\n  },\n};\n",
        ),
        ("src/throw/page.njk", "{{ n }}"),
        ("src/fine.njk", "fine"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let out = text(&o);
    assert!(out.contains("reads `collections`"), "{out}");
    assert!(out.contains("changes the data it is given"), "{out}");
    assert!(out.contains("Cannot read properties of undefined"), "{out}");
    assert!(out.contains("src/throw/throw.11tydata.js:3:"), "{out}");
    for page in ["coll", "poke", "throw"] {
        assert_eq!(
            written(&t, &format!("{page}/page/index.html")),
            None,
            "{page}"
        );
    }
    // `this` is 11ty's filters, Brrrty's own.
    assert_eq!(
        written(&t, "this/page/index.html").as_deref(),
        Some("function")
    );
    assert_eq!(written(&t, "fine/index.html").as_deref(), Some("fine"));
}

#[test]
fn a_function_that_reads_a_withheld_key_is_refused() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        ("src/_data/odd.js", "module.exports = { m: new Map() };"),
        (
            "src/_data/f.js",
            "module.exports = { size: (d) => { try { return typeof d.odd; } catch { return 'caught'; } } };",
        ),
        (
            "src/p/p.11tydata.js",
            "module.exports = { eleventyComputed: { s: (d) => require('./helper.js')(d) } };",
        ),
        (
            "src/p/helper.js",
            "module.exports = (d) => { try { return Object.keys(d.odd).length; } catch { return -1; } };",
        ),
        ("src/p/page.njk", "{{ s }}"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let out = text(&o);
    assert!(out.contains("reads `odd`"), "{out}");
    assert!(out.contains("a Map object"), "{out}");
    assert_eq!(written(&t, "p/page/index.html"), None);
}

#[test]
fn functions_where_11ty_builds_differently_stay_refused() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/l/l.11tydata.js",
            "module.exports = { layout: () => 'x.njk' };",
        ),
        ("src/l/page.njk", "x"),
        (
            "src/n/n.11tydata.js",
            "module.exports = { permalink: () => 5 };",
        ),
        ("src/n/page.njk", "x"),
        (
            "src/liquid.liquid",
            "---\npermalink: /liquid/\n---\n{{ helper }}",
        ),
        (
            "src/liquid.11tydata.js",
            "module.exports = { helper() { return this.title; }, title: 'T' };",
        ),
    ]);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let out = text(&o);
    assert!(out.contains("data.layout is a function"), "{out}");
    assert!(
        out.contains("`permalink` function returned a number"),
        "{out}"
    );
    assert!(out.contains("LiquidJS's variable scope"), "{out}");
    assert_eq!(written(&t, "liquid/index.html"), None);
}

#[test]
fn builds_with_data_functions_are_the_same_every_time() {
    if !has_node() {
        return;
    }
    let mut files: Vec<(String, String)> = vec![
        ("brrrty.config.rhai".into(), ON.into()),
        (
            "src/_data/fmt.js".into(),
            "module.exports = { money: (n) => '$' + n.toFixed(2), slug: (s) => s.toLowerCase().replace(/ +/g, '-') };".into(),
        ),
        // Global computed data, so the collection's order stays 11ty's own
        // (local JavaScript data makes it depend on file-read timing).
        (
            "src/_data/eleventyComputed.js".into(),
            "module.exports = { slug: (d) => d.title ? d.fmt.slug(d.title) : '', heading: async (d) => d.title + ' / ' + d.slug, permalink: (d) => d.title ? `/p/${d.slug}/` : undefined };".into(),
        ),
        (
            "src/posts/posts.11tydata.json".into(),
            "{ \"tags\": [\"post\"] }".into(),
        ),
        (
            "src/index.njk".into(),
            "{% for p in collections.post %}{{ p.data.heading }} {{ fmt.money(loop.index) }} {{ p.url }}\n{% endfor %}".into(),
        ),
    ];
    for i in 0..40 {
        files.push((
            format!("src/posts/p{i}.njk"),
            format!(
                "---\ntitle: Post {i}\ndate: 2026-01-{:02}\n---\nbody {{{{ fmt.money({i}) }}}}",
                i % 28 + 1
            ),
        ));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let t = site(&refs);
    let mut first: Option<Vec<(String, String)>> = None;
    for _ in 0..5 {
        let _ = fs::remove_dir_all(t.path().join("_site"));
        let o = build(t.path(), &[], &[]);
        assert!(o.status.success(), "{}", text(&o));
        let mut all = Vec::new();
        let mut stack = vec![t.path().join("_site")];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    all.push((
                        p.strip_prefix(t.path()).unwrap().display().to_string(),
                        fs::read_to_string(&p).unwrap(),
                    ));
                }
            }
        }
        all.sort();
        assert_eq!(all.len(), 41);
        match &first {
            None => first = Some(all),
            Some(f) => assert_eq!(f, &all),
        }
    }
    let index = written(&t, "index.html").unwrap();
    assert!(index.contains("Post 3 / post-3 $"), "{index}");
    assert!(index.contains("/p/post-3/"), "{index}");
}

#[test]
fn a_function_that_changes_a_shared_list_is_refused() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_data/list.js",
            "module.exports = { items: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10] };",
        ),
        (
            "src/_data/eleventyComputed.js",
            "module.exports = { n: (d) => { d.list.items.push(0); return d.list.items.length; } };",
        ),
        ("src/a.njk", "{{ n }}"),
        ("src/b.njk", "{{ n }}"),
    ]);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let out = text(&o);
    // The change is refused where it happens, and the list is sent again,
    // unchanged, for the pages after.
    assert!(out.contains("changes the data it is given"), "{out}");
    assert_eq!(written(&t, "a/index.html"), None);
    assert_eq!(written(&t, "b/index.html"), None);
}

#[test]
fn a_long_list_passed_to_functions_on_many_pages_stays_one_list() {
    if !has_node() {
        return;
    }
    let mut files: Vec<(String, String)> = vec![
        ("brrrty.config.rhai".into(), ON.into()),
        (
            "src/_data/list.js".into(),
            "module.exports = { items: Array.from({ length: 50 }, (_, i) => ({ i })) };".into(),
        ),
        (
            "src/_data/h.js".into(),
            "let seen; module.exports = { same: (l) => { const s = seen === undefined || seen === l; seen = seen || l; return l.length + (s ? ' same' : ' other'); } };".into(),
        ),
    ];
    for i in 0..20 {
        files.push((format!("src/p{i}.njk"), "{{ h.same(list.items) }}".into()));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let t = site(&refs);
    let o = build(t.path(), &[], &[]);
    // The function keeps state (the first list it saw), so it is checked:
    // each page hands it the same list, as 11ty does, and it gives the
    // same result every time.
    assert!(o.status.success(), "{}", text(&o));
    for i in 0..20 {
        assert_eq!(
            written(&t, &format!("p{i}/index.html")).as_deref(),
            Some("50 same")
        );
    }
}

/// A build's exit code and the pages it wrote.
type Outcome = (Option<i32>, Vec<(String, String)>);

/// Build `files` `runs` times; each gives the same exit code and the same
/// pages (none when refused).
fn same_every_run(files: &[(&str, &str)], runs: usize) -> (Output, tempfile::TempDir) {
    let t = site(files);
    let mut first: Option<Outcome> = None;
    let mut last = None;
    for _ in 0..runs {
        let _ = fs::remove_dir_all(t.path().join("_site"));
        let o = build(t.path(), &[], &[]);
        let mut pages = Vec::new();
        if let Ok(entries) = fs::read_dir(t.path().join("_site")) {
            for e in entries.flatten() {
                let p = e.path().join("index.html");
                pages.push((
                    e.file_name().to_string_lossy().into_owned(),
                    fs::read_to_string(p).unwrap_or_default(),
                ));
            }
        }
        pages.sort();
        let now = (o.status.code(), pages);
        match &first {
            None => first = Some(now),
            Some(f) => assert_eq!(f, &now, "{}", text(&o)),
        }
        last = Some(o);
    }
    (last.expect("ran"), t)
}

#[test]
fn a_registry_shared_between_calls_is_refused_every_time() {
    if !has_node() {
        return;
    }
    // 11ty 3.1.8 prints p1:1, p2:2, p3:3, p4:4, in its own page order.
    let reg = "const seen = [];\nmodule.exports = {\n  add(x) { seen.push(x); return \"\"; },\n  total(label) { return label + \":\" + seen.length; },\n};\n";
    let pages: Vec<(String, String)> = (1..=4)
        .map(|i| {
            (
                format!("src/p{i}.njk"),
                format!("{{{{ reg.add(\"p{i}\") }}}}{{{{ reg.total(\"p{i}\") }}}}"),
            )
        })
        .collect();
    let mut files: Vec<(&str, &str)> = vec![("brrrty.config.rhai", ON), ("src/_data/reg.js", reg)];
    files.extend(pages.iter().map(|(a, b)| (a.as_str(), b.as_str())));
    let (o, t) = same_every_run(&files, 10);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("js-data-impure"), "{}", text(&o));
    assert_eq!(written(&t, "p1/index.html"), None);
}

#[test]
fn a_flag_one_page_sets_and_another_reads_is_refused_every_time() {
    if !has_node() {
        return;
    }
    let (o, _t) = same_every_run(
        &[
            ("brrrty.config.rhai", ON),
            (
                "src/_data/flag.js",
                "let on = false;\nmodule.exports = { set() { on = true; return \"\"; }, get() { return String(on); } };\n",
            ),
            ("src/a.njk", "{{ flag.set() }}"),
            ("src/b.njk", "{{ flag.get() }}"),
            ("src/c.liquid", "{{ flag.get }}"),
        ],
        10,
    );
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("js-data-impure"), "{}", text(&o));
}

#[test]
fn caches_closures_and_bound_functions_build() {
    if !has_node() {
        return;
    }
    let mut files = vec![
        ("brrrty.config.rhai", ON),
        (
            "src/_data/i18n.js",
            "const strings = { en: { home: \"Home\" }, fr: { home: \"Accueil\" } };\nconst make = (lang) => ({ t: (key) => strings[lang][key] });\nfunction tr(lang, key) { return lang + \"/\" + key; }\nconst cache = new Map();\nfunction slow(x) { if (!cache.has(x)) cache.set(x, x.toUpperCase()); return cache.get(x); }\nmodule.exports = { en: make(\"en\"), fr: make(\"fr\"), ben: tr.bind(null, \"en\"), bfr: tr.bind(null, \"fr\"), slow };\n",
        ),
    ];
    let pages: Vec<(String, String)> = (0..12)
        .map(|i| {
            (
                format!("src/p{i}.njk"),
                format!("{{{{ i18n.en.t(\"home\") }}}} {{{{ i18n.fr.t(\"home\") }}}} {{{{ i18n.ben(\"x\") }}}} {{{{ i18n.bfr(\"x\") }}}} {{{{ i18n.slow(\"k{}\") }}}}", i % 3),
            )
        })
        .collect();
    files.extend(pages.iter().map(|(a, b)| (a.as_str(), b.as_str())));
    let (o, t) = same_every_run(&files, 5);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        written(&t, "p4/index.html").as_deref(),
        Some("Home Accueil en/x fr/x K1")
    );
}

#[test]
fn a_computed_value_from_the_clock_builds_every_time() {
    if !has_node() {
        return;
    }
    // The time of the call, as under 11ty: the replay check is handed the
    // same values, so the outcome never depends on the millisecond.
    let mut files = vec![
        ("brrrty.config.rhai", ON),
        (
            "src/_data/eleventyComputed.js",
            "const c = require(\"crypto\");\nmodule.exports = { built: () => new Date(), lucky: () => Math.random() < 2, id: () => c.randomUUID(), t: () => performance.now() > 0, hr: () => String(process.hrtime.bigint() % 7n), f: () => new Intl.DateTimeFormat(\"en-US\", { timeStyle: \"medium\" }).format() };",
        ),
    ];
    let pages: Vec<(String, String)> = (0..40)
        .map(|i| {
            (
                format!("src/p{i}.njk"),
                "{{ built }}|{{ id | length }}|{{ t }}|{{ f | length > 0 }}".to_string(),
            )
        })
        .collect();
    files.extend(pages.iter().map(|(a, b)| (a.as_str(), b.as_str())));
    let t = site(&files);
    for _ in 0..8 {
        let _ = fs::remove_dir_all(t.path().join("_site"));
        let o = build(t.path(), &[], &[]);
        assert!(o.status.success(), "{}", text(&o));
        let p = written(&t, "p3/index.html").unwrap();
        assert!(p.ends_with("|36|true|true"), "{p}");
    }
}

#[test]
fn the_time_a_data_file_read_as_it_loaded_is_fine() {
    if !has_node() {
        return;
    }
    // The replay check loads the data again: it is handed the same clock.
    let (o, _t) = same_every_run(
        &[
            ("brrrty.config.rhai", ON),
            (
                "src/_data/age.js",
                "const now = Date.now();\nconst salt = Math.random();\nmodule.exports = { stamp: (x) => x + \":\" + now + salt };\n",
            ),
            ("src/a.njk", "{{ age.stamp(\"a\") | length }}"),
            ("src/b.njk", "{{ age.stamp(\"b\") | length }}"),
        ],
        1,
    );
    assert!(o.status.success(), "{}", text(&o));
}

#[test]
fn a_computed_key_inside_shared_global_data_stays_the_pages_own() {
    if !has_node() {
        return;
    }
    // `site` has enough keys to be kept in the worker and shared by pages;
    // setting `site.title` must still change only each page's copy, as
    // 11ty's merge gives every page its own copy of a plain object.
    let (o, t) = same_every_run(
        &[
            ("brrrty.config.rhai", ON),
            (
                "src/_data/site.js",
                "module.exports = { a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8, name: \"S\" };",
            ),
            (
                "src/_data/eleventyComputed.js",
                "module.exports = { site: { title: (d) => d.site.name + \"/\" + d.page.fileSlug } };",
            ),
            ("src/a.njk", "{{ site.title }}"),
            ("src/b.njk", "{{ site.title }}"),
        ],
        3,
    );
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(written(&t, "a/index.html").as_deref(), Some("S/a"));
    assert_eq!(written(&t, "b/index.html").as_deref(), Some("S/b"));
}

#[test]
fn a_function_that_changes_a_data_files_own_list_is_refused() {
    if !has_node() {
        return;
    }
    // 11ty's templates see the list sorted, or the style added, from that
    // call on; Brrrty's see it as it loaded.
    for (data, page) in [
        (
            "const all = [{t:\"b\",d:2},{t:\"a\",d:1},{t:\"c\",d:3}];\nmodule.exports = { all, newest() { return all.sort((x, y) => y.d - x.d); } };\n",
            "{% for p in d.newest() %}{{ p.t }}{% endfor %} first={{ d.all[0].t }}",
        ),
        (
            "const styles = [];\nmodule.exports = { styles, use(n) { if (!styles.includes(n)) styles.push(n); return \"\"; } };\n",
            "{{ d.use(\"gallery\") }}css=[{{ d.styles | join(\",\") }}]",
        ),
        // Changed and changed back within one page: 11ty's template between
        // the two calls sees the change.
        (
            "const stack = [];\nmodule.exports = { stack, open(n) { stack.push(n); return \"\"; }, close() { stack.pop(); return \"\"; } };\n",
            "{{ d.open(\"x\") }}[{{ d.stack | join }}]{{ d.close() }}",
        ),
    ] {
        let (o, t) = same_every_run(
            &[
                ("brrrty.config.rhai", ON),
                ("src/_data/d.js", data),
                ("src/a.njk", page),
            ],
            3,
        );
        assert_eq!(o.status.code(), Some(1), "{}", text(&o));
        let out = text(&o);
        assert!(
            out.contains("changes the data src/_data/d.js gives"),
            "{out}"
        );
        assert!(out.contains("js-data-impure"), "{out}");
        assert_eq!(written(&t, "a/index.html"), None);
    }
}

#[test]
fn state_a_function_keeps_to_itself_and_returns_later_is_caught() {
    if !has_node() {
        return;
    }
    // Not part of what the file exports: the replay check sees it.
    let (o, _t) = same_every_run(
        &[
            ("brrrty.config.rhai", ON),
            (
                "src/_data/d.js",
                "const seen = [];\nmodule.exports = { add(x) { seen.push(x); return \"\"; }, list() { return seen.join(\",\"); } };\n",
            ),
            ("src/a.njk", "{{ d.add(\"a\") }}"),
            ("src/b.njk", "[{{ d.list() }}]"),
        ],
        5,
    );
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let out = text(&o);
    assert!(out.contains("js-data-impure"), "{out}");
    // Named as a reader would: no internal labels.
    assert!(out.contains("in the content of src/"), "{out}");
}

#[test]
fn a_helper_that_resets_and_counts_within_each_page_is_refused_every_time() {
    if !has_node() {
        return;
    }
    // 11ty renders one page at a time and builds this; Brrrty's pages call
    // functions at the same time, and the replay check runs the calls
    // interleaved too, so it refuses it every time, never only sometimes.
    let mut files = vec![
        ("brrrty.config.rhai", ON),
        (
            "src/_data/c.js",
            "let n = 0;\nmodule.exports = { reset() { n = 0; return \"\"; }, next() { n += 1; return n; } };\n",
        ),
    ];
    let pages: Vec<(String, String)> = (0..40)
        .map(|i| {
            (
                format!("src/n{i}.njk"),
                "{{ c.reset() }}{{ c.next() }}{{ c.next() }}{{ c.next() }}".to_string(),
            )
        })
        .collect();
    files.extend(pages.iter().map(|(a, b)| (a.as_str(), b.as_str())));
    let (o, _t) = same_every_run(&files, 10);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("js-data-impure"), "{}", text(&o));
}

#[test]
fn a_replay_that_never_finishes_is_given_up_on() {
    if !has_node() {
        return;
    }
    // The data file hangs only when it loads a second time.
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/_data/d.js",
            "const fs = require(\"fs\");\nconst again = fs.existsSync(\"marker\");\nfs.writeFileSync(\"marker\", \"1\");\nmodule.exports = async () => { if (again) await new Promise(() => {}); return { f: () => \"x\" }; };\n",
        ),
        ("src/a.njk", "{{ d.f() }}"),
    ]);
    let started = Instant::now();
    let o = build(t.path(), &[], &[]);
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(
        text(&o).contains("longer than the first run allows"),
        "{}",
        text(&o)
    );
}

#[test]
fn every_kind_of_change_to_a_data_files_value_is_refused() {
    if !has_node() {
        return;
    }
    // Each changes, in a call, something 11ty's templates then read.
    for (data, page) in [
        (
            "const list = [1,2]; module.exports = { list, f() { list.label = \"L\"; return \"\"; } };",
            "{{ h.f() }}[{{ h.list.label }}]",
        ),
        (
            "const list = [1,2]; module.exports = { list, f() { Object.defineProperty(list, \"label\", { value: \"L\", enumerable: false }); return \"\"; } };",
            "{{ h.f() }}[{{ h.list.label }}]",
        ),
        (
            "const arr = [1]; module.exports = { get list() { return arr; }, f() { arr.push(2); return \"\"; } };",
            "{{ h.f() }}[{{ h.list | length }}]",
        ),
        (
            "module.exports = { l: [1], f() { Array.prototype.extra = \"E\"; return \"\"; } };",
            "{{ h.f() }}[{{ h.l.extra }}]",
        ),
        (
            "module.exports = { o: { a: 1 }, f() { Object.prototype.polluted = \"P\"; return \"\"; } };",
            "{{ h.f() }}[{{ h.o.polluted }}]",
        ),
        (
            "const d = new Date(Date.UTC(2020,0,1)); module.exports = { d, f() { d.setUTCFullYear(2021); return \"\"; } };",
            "{{ h.f() }}[{{ h.d.getUTCFullYear() }}]",
        ),
        (
            "const o = { a: { b: [ { c: 1 } ] } }; module.exports = { o, f() { o.a.b[0].c = 2; return \"\"; } };",
            "{{ h.f() }}[{{ h.o.a.b[0].c }}]",
        ),
        (
            "const o = { a: 1 }; module.exports = { o, f() { Object.setPrototypeOf(o, { inh: \"I\" }); return \"\"; } };",
            "{{ h.f() }}[{{ h.o.inh }}]",
        ),
        (
            "const o = { a: 1 }; module.exports = { o, f() { o[Symbol(\"s\")] = 1; return \"\"; } };",
            "{{ h.f() }}[{{ h.o.a }}]",
        ),
        (
            "const list = [{ on: false }, { on: false }]; module.exports = { list, on(i) { list[i].on = true; return \"\"; }, off(i) { list[i].on = false; return \"\"; } };",
            "{{ h.on(0) }}[{{ h.list[0].on }}]{{ h.off(0) }}",
        ),
    ] {
        let (o, t) = same_every_run(
            &[
                ("brrrty.config.rhai", ON),
                ("src/_data/h.js", data),
                ("src/a.njk", page),
            ],
            2,
        );
        assert_eq!(o.status.code(), Some(1), "{data}\n{}", text(&o));
        let out = text(&o);
        assert!(out.contains("js-data-impure"), "{data}\n{out}");
        // One problem for one cause.
        assert_eq!(out.matches("  × ").count(), 1, "{data}\n{out}");
        assert_eq!(written(&t, "a/index.html"), None);
    }
}

#[test]
fn a_data_file_that_changes_another_while_template_data_loads_is_refused() {
    if !has_node() {
        return;
    }
    // 11ty 3.1.8 prints `[1,3,2]`: it loads a template's directory data
    // before the global data, and then the order is its own.
    for f in [
        "module.exports = { list: [1] };",
        "module.exports = { list: [1], f() { return \"f\"; } };",
    ] {
        let (o, t) = same_every_run(
            &[
                ("brrrty.config.rhai", ON),
                ("src/_data/a.js", f),
                (
                    "src/_data/b.js",
                    "require(\"./a\").list.push(2); module.exports = { b: 1 };",
                ),
                (
                    "src/sub/sub.11tydata.js",
                    "require(\"../_data/a\").list.push(3); module.exports = { t: 1 };",
                ),
                ("src/sub/p1.njk", "{{ a.list | dump | safe }}"),
                ("src/p2.njk", "{{ a.list | dump | safe }}"),
            ],
            2,
        );
        assert_eq!(o.status.code(), Some(1), "{}", text(&o));
        let out = text(&o);
        assert!(
            out.contains(
                "src/_data/a.js gave changed while template and directory data files loaded"
            ),
            "{out}"
        );
        assert_eq!(written(&t, "p2/index.html"), None);
    }
}

#[test]
fn a_value_saved_from_the_first_call_that_read_the_clock_builds_every_time() {
    if !has_node() {
        return;
    }
    let mut files = vec![
        ("brrrty.config.rhai", ON),
        (
            "src/_data/h.js",
            "const c = require(\"crypto\");\nlet t; let id;\nmodule.exports = { when() { if (!t) t = new Date(); return t.toISOString(); }, id() { id ??= c.randomUUID(); return id; } };\n",
        ),
    ];
    let pages: Vec<(String, String)> = (0..6)
        .map(|i| {
            (
                format!("src/p{i}.njk"),
                "{{ h.when() }} {{ h.id() }}".to_string(),
            )
        })
        .collect();
    files.extend(pages.iter().map(|(a, b)| (a.as_str(), b.as_str())));
    let t = site(&files);
    for _ in 0..6 {
        let _ = fs::remove_dir_all(t.path().join("_site"));
        let o = build(t.path(), &[], &[]);
        assert!(o.status.success(), "{}", text(&o));
        let pages: Vec<String> = (0..6)
            .map(|i| written(&t, &format!("p{i}/index.html")).unwrap())
            .collect();
        assert!(pages.iter().all(|p| *p == pages[0]), "{pages:?}");
    }
}

#[test]
fn pure_functions_on_pages_paginated_over_a_collection_build() {
    if !has_node() {
        return;
    }
    let mut files: Vec<(String, String)> = vec![
        ("brrrty.config.rhai".into(), ON.into()),
        ("src/_data/h.js".into(), "module.exports = { up(s) { return String(s).toUpperCase(); } };".into()),
        ("src/_data/eleventyComputed.js".into(), "module.exports = { shout: (d) => (d.title || \"\") + \"!\" };".into()),
        ("src/posts/posts.json".into(), "{\"tags\":[\"posts\"],\"layout\":\"post.njk\"}".into()),
        ("src/_includes/post.njk".into(), "{{ h.up(title) }}{{ content | safe }}".into()),
        ("src/_includes/base.njk".into(), "<html>{{ h.up(\"x\") }}{{ content | safe }}</html>".into()),
        (
            "src/list.njk".into(),
            "---\npagination:\n  data: collections.posts\n  size: 10\nlayout: base.njk\npermalink: \"/list/{{ pagination.pageNumber }}/\"\n---\n{% for p in pagination.items %}{{ h.up(p.data.title) }}{{ p.templateContent | safe }}{% endfor %}".into(),
        ),
    ];
    for i in 0..60 {
        files.push((
            format!("src/posts/p{i}.njk"),
            format!("---\ntitle: post {i}\n---\n{{{{ shout }}}}"),
        ));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let (o, t) = same_every_run(&refs, 3);
    assert!(o.status.success(), "{}", text(&o));
    assert!(written(&t, "list/2/index.html").unwrap().contains("POST"));
}

#[test]
fn a_key_order_changed_and_restored_between_pages_is_refused() {
    if !has_node() {
        return;
    }
    // 11ty prints `ba` on b (a.njk's call moved `a` last); Brrrty's
    // templates list the keys as they loaded.
    let (o, t) = same_every_run(
        &[
            ("brrrty.config.rhai", ON),
            (
                "src/_data/h.js",
                "const list=[{a:1,b:2}]; module.exports={list, f1(){ const o=list[0]; const v=o.a; delete o.a; o.a=v; return \"\"; }, f2(){ const o=list[0]; const v=o.b; delete o.b; o.b=v; return \"\"; }};",
            ),
            ("src/a.njk", "{{ h.f1() }}"),
            (
                "src/b.njk",
                "{% for k, v in h.list[0] %}{{ k }}{% endfor %}",
            ),
            ("src/c.njk", "{{ h.f2() }}"),
        ],
        3,
    );
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("js-data-impure"), "{}", text(&o));
    assert_eq!(written(&t, "b/index.html"), None);
}

/// How many calls the worker ran fast (`BRRRTY_JS_FAST=verify` prints it).
fn fast_calls(out: &str) -> usize {
    out.lines()
        .filter_map(|l| l.strip_prefix("brrrty-js-fast: "))
        .filter_map(|l| l.strip_suffix(" fast"))
        .filter_map(|n| n.parse::<usize>().ok())
        .sum()
}

#[test]
fn every_kind_of_change_is_refused_when_calls_run_fast() {
    if !has_node() {
        return;
    }
    // The calls that change nothing run in V8's side-effect-free mode,
    // without the walk; the one that changes something can't, and is
    // walked. `verify` walks after the fast ones too: none may have
    // changed anything.
    let pure = "pure(x) { return String(x).toUpperCase(); }";
    for (data, page) in [
        (
            format!(
                "const list = [1,2]; module.exports = {{ list, {pure}, f() {{ list.label = \"L\"; return \"\"; }} }};"
            ),
            "{{ h.pure(1) }}{{ h.f() }}{{ h.pure(2) }}[{{ h.list.label }}]",
        ),
        (
            format!(
                "const list = [1,2]; module.exports = {{ list, {pure}, f() {{ Object.defineProperty(list, \"label\", {{ value: \"L\", enumerable: false }}); return \"\"; }} }};"
            ),
            "{{ h.pure(1) }}{{ h.f() }}{{ h.pure(2) }}[{{ h.list.label }}]",
        ),
        (
            format!(
                "module.exports = {{ l: [1], {pure}, f() {{ Array.prototype.extra = \"E\"; return \"\"; }} }};"
            ),
            "{{ h.pure(1) }}{{ h.f() }}{{ h.pure(2) }}[{{ h.l.extra }}]",
        ),
        (
            format!(
                "const d = new Date(Date.UTC(2020,0,1)); module.exports = {{ d, {pure}, f() {{ d.setUTCFullYear(2021); return \"\"; }} }};"
            ),
            "{{ h.pure(1) }}{{ h.f() }}{{ h.pure(2) }}[{{ h.d.getUTCFullYear() }}]",
        ),
        (
            format!(
                "const o = {{ a: {{ b: [ {{ c: 1 }} ] }} }}; module.exports = {{ o, {pure}, f() {{ o.a.b[0].c = 2; return \"\"; }} }};"
            ),
            "{{ h.pure(1) }}{{ h.f() }}{{ h.pure(2) }}[{{ h.o.a.b[0].c }}]",
        ),
        (
            format!(
                "const o = {{ a: 1 }}; module.exports = {{ o, {pure}, f() {{ Object.setPrototypeOf(o, {{ inh: \"I\" }}); return \"\"; }} }};"
            ),
            "{{ h.pure(1) }}{{ h.f() }}{{ h.pure(2) }}[{{ h.o.inh }}]",
        ),
        // Changed and changed back by two calls, with fast calls between.
        (
            format!(
                "const list = [{{ on: false }}]; module.exports = {{ list, {pure}, on() {{ list[0].on = true; return \"\"; }}, off() {{ list[0].on = false; return \"\"; }} }};"
            ),
            "{{ h.pure(0) }}{{ h.on() }}{{ h.pure(1) }}[{{ h.list[0].on }}]{{ h.pure(2) }}{{ h.off() }}",
        ),
        // A closure's counter: not the data's own value, but the replay
        // check sees it.
        (
            format!("let n = 0; module.exports = {{ {pure}, next() {{ n += 1; return n; }} }};"),
            "{{ h.pure(1) }}{{ h.next() }}{{ h.pure(2) }}",
        ),
    ] {
        let mut files = vec![
            ("brrrty.config.rhai", ON),
            ("src/_data/h.js", data.as_str()),
            ("src/a.njk", page),
        ];
        let extra = [("src/b.njk", page), ("src/c.njk", page)];
        files.extend(extra);
        let t = site(&files);
        let o = build(t.path(), &[], &[("BRRRTY_JS_FAST", "verify")]);
        let out = text(&o);
        assert_eq!(o.status.code(), Some(1), "{data}\n{out}");
        assert!(out.contains("js-data-impure"), "{data}\n{out}");
        assert!(!out.contains("fast path missed"), "{data}\n{out}");
        assert!(fast_calls(&out) > 0, "{data}\n{out}");
        assert_eq!(written(&t, "a/index.html"), None);
    }
}

#[test]
fn pure_calls_run_fast_and_build_the_same_pages() {
    if !has_node() {
        return;
    }
    let mut files: Vec<(String, String)> = vec![
        ("brrrty.config.rhai".into(), ON.into()),
        (
            "src/_data/site.js".into(),
            "const strings = {}; for (let i = 0; i < 50; i++) strings[\"k\" + i] = \"v\" + i;\nconst posts = Array.from({ length: 40 }, (_, i) => ({ t: \"p\" + i, tags: [\"a\"] }));\nmodule.exports = { strings, posts, t(k) { return this.strings[k]; }, up(s) { return String(s).toUpperCase(); }, pick(i) { return posts[i]; }, list() { return [1, { a: [2] }]; } };".into(),
        ),
        (
            "src/_data/eleventyComputed.js".into(),
            "module.exports = { shout: (d) => String(d.title).toUpperCase() + \"!\" };".into(),
        ),
    ];
    for i in 0..12 {
        files.push((
            format!("src/p{i}.njk"),
            format!("---\ntitle: post {i}\n---\n{{{{ site.t(\"k{i}\") }}}} {{{{ site.up(title) }}}} {{{{ site.pick({i}).t }}}} {{{{ site.list() | dump }}}} {{{{ shout }}}}"),
        ));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let mut pages = Vec::new();
    for mode in ["off", "verify"] {
        let t = site(&refs);
        let o = build(t.path(), &[], &[("BRRRTY_JS_FAST", mode)]);
        let out = text(&o);
        assert!(o.status.success(), "{mode}\n{out}");
        assert!(!out.contains("fast path missed"), "{out}");
        if mode == "verify" {
            // Most of each page's four calls and its computed value.
            assert!(fast_calls(&out) >= 30, "{out}");
        }
        pages.push(
            (0..12)
                .map(|i| written(&t, &format!("p{i}/index.html")).unwrap())
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(pages[0], pages[1]);
    assert_eq!(pages[1][3], "v3 POST 3 p3 [1,{&quot;a&quot;:[2]}] POST 3!");
}

#[test]
fn a_change_a_timer_makes_while_calls_run_fast_is_refused() {
    if !has_node() {
        return;
    }
    // The timer fires after its call, between other pages' calls, which
    // run fast: the event loop still turns as the build goes, and the
    // walk after it sees the change.
    let mut files = vec![
        ("brrrty.config.rhai", ON),
        (
            "src/_data/h.js",
            "const list = [1];\nmodule.exports = { list, later() { setTimeout(() => list.push(2), 0); return \"\"; }, pure(x) { return x; } };\n",
        ),
        ("src/a.njk", "{{ h.later() }}"),
    ];
    let pages: Vec<(String, String)> = (0..40)
        .map(|i| {
            (
                format!("src/p{i}.njk"),
                "{{ h.pure(1) }}{{ h.pure(2) }}[{{ h.list | length }}]".to_string(),
            )
        })
        .collect();
    files.extend(pages.iter().map(|(a, b)| (a.as_str(), b.as_str())));
    for mode in ["verify", "auto"] {
        let t = site(&files);
        let o = build(t.path(), &[], &[("BRRRTY_JS_FAST", mode)]);
        let out = text(&o);
        assert_eq!(o.status.code(), Some(1), "{mode}\n{out}");
        assert!(out.contains("js-data-impure"), "{mode}\n{out}");
        assert!(!out.contains("fast path missed"), "{out}");
    }
}
