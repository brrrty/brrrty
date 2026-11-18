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
        .args(["-INT", &child.id().to_string()])
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
    assert_eq!(
        fs::read_to_string(t.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        3
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
fn collections_with_local_javascript_data_are_refused_as_11tys_own_nondeterminism() {
    if !has_node() {
        return;
    }
    let t = site(&[
        ("brrrty.config.rhai", ON),
        (
            "src/posts/posts.11tydata.js",
            "module.exports = { tags: \"post\" };",
        ),
        ("src/posts/a.md", "a"),
        ("src/posts/b.md", "b"),
        (
            "src/list.njk",
            "{% for p in collections.post %}{{ p.url }}{% endfor %}",
        ),
    ]);
    let o = build(t.path(), &[], &[]);
    assert_eq!(o.status.code(), Some(1));
    let out = text(&o);
    assert!(out.contains("11ty orders these pages by file"), "{out}");
    assert!(t.path().join("_site/posts/a/index.html").exists());
}
