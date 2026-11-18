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
