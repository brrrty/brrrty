//! `brrrty doctor` and `brrrty doctor --verify`.
//!
//! The real-11ty test is ignored: it needs Node and
//! `BRRRTY_ELEVENTY_NODE_MODULES=<dir>`, a node_modules with
//! @11ty/eleventy 3.1.8:
//!
//!   BRRRTY_ELEVENTY_NODE_MODULES=<dir> cargo test -p brrrty --test doctor -- --ignored

use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn site(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    dir
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .env_remove("SOURCE_DATE_EPOCH")
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

/// One page with four blockers: a filter in its layout, two in the page and
/// a JavaScript data file it reads. Rendering stops at the first.
const BLOCKED: &[(&str, &str)] = &[
    (
        "_includes/base.njk",
        "<title>{{ title | shout }}</title>\n{{ content | safe }}\n",
    ),
    ("_data/stuff.js", "module.exports = { n: 3 };\n"),
    (
        "index.njk",
        "---\nlayout: base.njk\ntitle: Home\n---\n{{ stuff.n | num }}\n{{ 3 | usd }}\n",
    ),
];

#[test]
fn every_blocker_of_a_page_is_listed_and_nothing_is_written() {
    let dir = site(BLOCKED);
    let out = run(dir.path(), &["doctor"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{text}{}", stderr(&out));
    for want in [
        "JavaScript data file _data/stuff.js (1 page, 1 template)",
        "   index.njk:5:4: this template reads `stuff`",
        "Filter `shout` isn't defined",
        "   _includes/base.njk:1:19: Error: filter not found: shout",
        "Filter `num` isn't defined",
        "Filter `usd` isn't defined",
        "   Fix: Port it to brrrty.config.rhai",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    assert!(
        text.ends_with("0 of 1 page will build; 4 blockers, grouped below.\n"),
        "{text}"
    );
    assert!(!dir.path().join("_site").exists());
}

#[test]
fn json_lists_the_same_groups() {
    let dir = site(BLOCKED);
    let out = run(dir.path(), &["doctor", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["builds"], false);
    assert_eq!(v["blockers"], 4);
    assert_eq!(v["pages"]["total"], 1);
    assert_eq!(v["pages"]["building"], 0);
    assert_eq!(v["verify"], Value::Null);
    let kinds: Vec<&str> = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["cause"]["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["filter", "filter", "filter", "js-data"]);
    let shout = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["cause"]["name"] == "shout")
        .unwrap();
    assert_eq!(shout["templates"][0], "index.njk");
    assert_eq!(shout["locations"][0]["path"], "_includes/base.njk");
    assert_eq!(shout["locations"][0]["line"], 1);
}

#[test]
fn a_site_that_builds_exits_0() {
    let dir = site(&[("a.md", "# A\n"), ("b.md", "# B\n")]);
    let out = run(dir.path(), &["doctor"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), "All 2 pages will build.\n");
    assert!(!dir.path().join("_site").exists());
}

#[test]
fn usage_errors_exit_2() {
    let dir = site(&[("a.md", "# A\n")]);
    assert_eq!(
        run(dir.path(), &["doctor", "--frob"]).status.code(),
        Some(2)
    );
    // `--yes` only means something with `--verify`.
    assert_eq!(run(dir.path(), &["doctor", "--yes"]).status.code(), Some(2));
}

fn has_node() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn verify_says_how_to_install_11ty() {
    let dir = site(&[("a.md", "# A\n")]);
    let out = run(dir.path(), &["doctor", "--verify", "--yes"]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    if has_node() {
        assert!(
            err.contains("npm install --save-dev @11ty/eleventy"),
            "{err}"
        );
    } else {
        assert!(err.contains("Install Node.js"), "{err}");
    }
}

/// A stand-in for 11ty's `cmd.cjs` that writes `files` to `--output`.
fn fake_eleventy(root: &Path, files: &[(&str, &str)]) {
    let list = serde_json::to_string(files).unwrap();
    let script = format!(
        "const fs = require('fs'), path = require('path');\n\
         const out = process.argv.find(a => a.startsWith('--output=')).slice(9);\n\
         if (typeof Date.now() !== 'number') throw new Error('no clock');\n\
         for (const [rel, body] of {list}) {{\n\
           fs.mkdirSync(path.dirname(path.join(out, rel)), {{ recursive: true }});\n\
           fs.writeFileSync(path.join(out, rel), body);\n\
         }}\n"
    );
    let dir = root.join("node_modules/@11ty/eleventy");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cmd.cjs"), script).unwrap();
    std::fs::write(dir.join("package.json"), r#"{"version":"3.1.8"}"#).unwrap();
}

#[test]
fn verify_needs_yes_off_a_terminal() {
    if !has_node() {
        eprintln!("skipped: no node on PATH");
        return;
    }
    let dir = site(&[("a.md", "# A\n")]);
    fake_eleventy(dir.path(), &[("a/index.html", "<h1>A</h1>\n")]);
    let out = run(dir.path(), &["doctor", "--verify"]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains("runs your site's JavaScript"), "{err}");
    assert!(err.contains("pass --yes"), "{err}");
}

#[test]
fn verify_proves_byte_compatible_output() {
    if !has_node() {
        eprintln!("skipped: no node on PATH");
        return;
    }
    let dir = site(&[("a.md", "# A\n"), ("b.md", "# B\n")]);
    fake_eleventy(
        dir.path(),
        &[
            ("a/index.html", "<h1>A</h1>\n"),
            ("b/index.html", "<h1>B</h1>\n"),
        ],
    );
    let out = run(dir.path(), &["doctor", "--verify", "--yes"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}{}", stderr(&out));
    assert!(text.contains("Built your site with 11ty 3.1.8 and with Brrrty"));
    assert!(text.contains("  2 files: 2 byte-identical, 0 different, 0 missing, 0 extra"));
    assert!(
        text.contains("Your site is byte-compatible on Brrrty: 2 files, "),
        "{text}"
    );
    assert!(!dir.path().join("_site").exists());
}

#[test]
fn verify_lists_what_differs() {
    if !has_node() {
        eprintln!("skipped: no node on PATH");
        return;
    }
    let dir = site(&[("a.md", "# A\n"), ("b.md", "# B\n"), ("c.md", "# C\n")]);
    fake_eleventy(
        dir.path(),
        &[
            ("a/index.html", "<h1>A</h1>\n"),
            ("b/index.html", "<h1>Not B</h1>\n"),
            ("d/index.html", "<h1>D</h1>\n"),
        ],
    );
    let out = run(dir.path(), &["doctor", "--verify", "--yes"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{text}{}", stderr(&out));
    assert!(text.contains("  4 files: 1 byte-identical, 1 different, 1 missing, 1 extra"));
    assert!(text.contains("Different: b/index.html"), "{text}");
    assert!(text.contains("-<h1>Not B</h1>"), "{text}");
    assert!(text.contains("+<h1>B</h1>"), "{text}");
    assert!(text.contains("Missing (11ty wrote them, Brrrty didn't):\n  d/index.html"));
    assert!(text.contains("Extra (Brrrty wrote them, 11ty didn't):\n  c/index.html"));
    assert!(
        text.ends_with("Not byte-compatible yet: 1 different, 1 missing, 1 extra, of 4 files\n")
    );
    let out = run(dir.path(), &["doctor", "--verify", "--yes", "--json"]);
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["verify"]["byte_compatible"], false);
    assert_eq!(v["verify"]["different"], 1);
    assert_eq!(v["verify"]["differences"].as_array().unwrap().len(), 3);
}

/// A small site of the usual kinds of files, built with real 11ty 3.1.8.
#[test]
#[ignore = "needs Node and BRRRTY_ELEVENTY_NODE_MODULES"]
fn verify_with_real_eleventy() {
    let modules = std::env::var("BRRRTY_ELEVENTY_NODE_MODULES")
        .expect("set BRRRTY_ELEVENTY_NODE_MODULES to a node_modules with @11ty/eleventy 3.1.8");
    let dir = site(&[
        (
            "brrrty.config.rhai",
            "config.add_passthrough_copy(\"src/img\");\n#{ dir: #{ input: \"src\" } }\n",
        ),
        (
            "eleventy.config.js",
            "export default function (c) { c.addPassthroughCopy(\"src/img\"); return { dir: { input: \"src\" } }; }\n",
        ),
        ("src/img/a.txt", "a\n"),
        ("src/_data/site.json", "{ \"name\": \"Test\" }\n"),
        (
            "src/_includes/base.njk",
            "<!doctype html><title>{{ title }} | {{ site.name }}</title>\n{{ content | safe }}\n",
        ),
        (
            "src/posts/2024-01-02-hello.md",
            "---\nlayout: base.njk\ntitle: Hello\ntags: post\n---\n# Hello\n\nSome *text*.\n",
        ),
        (
            "src/posts/2024-02-03-again.md",
            "---\nlayout: base.njk\ntitle: Again\ntags: post\n---\nAgain.\n",
        ),
        (
            "src/index.njk",
            "---\nlayout: base.njk\ntitle: Home\n---\n<ul>{% for p in collections.post | reverse %}<li>{{ p.data.title }} {{ p.url }}</li>{% endfor %}</ul>\n",
        ),
        (
            "src/feed.liquid",
            "---\npermalink: /now.txt\n---\n{{ \"now\" | date: \"%Y\" }}\n",
        ),
    ]);
    std::fs::write(
        dir.path().join("package.json"),
        "{ \"type\": \"module\" }\n",
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&modules, dir.path().join("node_modules")).unwrap();
    let out = run(dir.path(), &["doctor", "--verify", "--yes"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}{}", stderr(&out));
    assert!(text.contains("Built your site with 11ty 3.1.8"), "{text}");
    assert!(
        text.contains("Your site is byte-compatible on Brrrty: 5 files, "),
        "{text}"
    );
}

/// A stand-in for 11ty that starts a child of its own, writes both process ids to
/// `pids` in the site, writes 3,000 files (so removing the temporary folder takes a
/// while), waits a little, stops its child and ends.
#[cfg(unix)]
fn slow_eleventy(root: &Path) {
    let script = "const fs = require('fs'), path = require('path'), cp = require('child_process');\n\
         const child = cp.spawn(process.execPath, ['-e', 'setTimeout(() => {}, 60000)'], { stdio: 'ignore' });\n\
         fs.writeFileSync('pids', process.pid + ' ' + child.pid);\n\
         const out = process.argv.find(a => a.startsWith('--output=')).slice(9);\n\
         for (let i = 0; i < 3000; i++) {\n\
           const d = path.join(out, 'p' + (i % 50));\n\
           fs.mkdirSync(d, { recursive: true });\n\
           fs.writeFileSync(path.join(d, i + '.html'), 'x'.repeat(2000));\n\
         }\n\
         setTimeout(() => { child.kill('SIGKILL'); }, 300);\n";
    let dir = root.join("node_modules/@11ty/eleventy");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cmd.cjs"), script).unwrap();
    std::fs::write(
        dir.join("package.json"),
        r#"{"version":"3.1.8","bin":{"eleventy":"cmd.cjs"}}"#,
    )
    .unwrap();
}

#[cfg(unix)]
fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// A site big enough that Brrrty's own build takes a moment, with the slow 11ty.
#[cfg(unix)]
fn interruptible_site() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..1500 {
        let p = dir.path().join(format!("posts/p{i}.md"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("# Post {i}\n\nSome *text*.\n")).unwrap();
    }
    slow_eleventy(dir.path());
    dir
}

/// One `doctor --verify --yes` run, signalled after `delay` (none when `None`), checked.
/// Returns how long it took and whether the signal stopped it.
#[cfg(unix)]
fn signalled_run(
    dir: &Path,
    signal: i32,
    delay: Option<std::time::Duration>,
) -> (std::time::Duration, bool) {
    use std::os::unix::process::ExitStatusExt;
    use std::time::{Duration, Instant};
    let tmp = tempfile::tempdir().unwrap();
    let _ = std::fs::remove_file(dir.join("pids"));
    let start = Instant::now();
    let child = Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .args(["doctor", "--verify", "--yes"])
        .current_dir(dir)
        .env("TMPDIR", tmp.path())
        .env_remove("SOURCE_DATE_EPOCH")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(d) = delay {
        std::thread::sleep(d);
        // SAFETY: signals the brrrty process this test started.
        unsafe { libc::kill(child.id() as i32, signal) };
    }
    let out = child.wait_with_output().unwrap();
    let took = start.elapsed();
    let (so, se) = (stdout(&out), stderr(&out));
    let said_stopped = se.contains("Stopped");
    let stopped = out.status.code() == Some(128 + signal);
    let killed_early = out.status.signal() == Some(signal);
    let finished = matches!(out.status.code(), Some(0 | 1));
    let at = format!(
        "delay {delay:?}: {:?}\nstdout: {so}\nstderr: {se}",
        out.status
    );
    if stopped {
        assert!(said_stopped && so.is_empty(), "{at}");
    } else if killed_early {
        // Before the handler was installed: nothing had started yet.
        assert!(so.is_empty() && !said_stopped, "{at}");
    } else {
        assert!(finished && !said_stopped && !so.is_empty(), "{at}");
    }
    let left: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "temporary folders left: {left:?}; {at}");
    if let Ok(pids) = std::fs::read_to_string(dir.join("pids")) {
        let ids: Vec<i32> = pids
            .split_whitespace()
            .filter_map(|p| p.parse().ok())
            .collect();
        let start = Instant::now();
        while ids.iter().any(|p| alive(*p)) {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "11ty's processes are still running; {at}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    (took, stopped)
}

/// Signal `brrrty doctor --verify` at delays spread over a whole run: before and during
/// Brrrty's build, while 11ty runs, during the comparison and while the report prints. Each
/// run must either stop cleanly (130 or 143, "Stopped", nothing on stdout) or finish
/// normally without saying it stopped, and leave no temporary folder or process behind.
#[cfg(unix)]
fn interrupted_verify_cleans_up(signal: i32) {
    if !has_node() {
        eprintln!("skipped: no node on PATH");
        return;
    }
    let dir = interruptible_site();
    // The first run warms the caches; the second says how long a run takes.
    signalled_run(dir.path(), signal, None);
    let (whole, _) = signalled_run(dir.path(), signal, None);
    let mut stops = 0;
    for step in 0..=24 {
        let (_, stopped) = signalled_run(dir.path(), signal, Some(whole * step / 20));
        stops += usize::from(stopped);
    }
    assert!(stops > 0, "no run was stopped");
}

#[test]
#[cfg(unix)]
fn ctrl_c_during_verify_stops_cleanly_at_any_moment() {
    interrupted_verify_cleans_up(libc::SIGINT);
}

#[test]
#[cfg(unix)]
fn sigterm_during_verify_stops_cleanly_at_any_moment() {
    interrupted_verify_cleans_up(libc::SIGTERM);
}

#[test]
fn a_failing_11ty_build_still_prints_the_report() {
    if !has_node() {
        eprintln!("skipped: no node on PATH");
        return;
    }
    let dir = site(BLOCKED);
    let pkg = dir.path().join("node_modules/@11ty/eleventy");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("cmd.cjs"), "throw new Error('config broke');\n").unwrap();
    std::fs::write(pkg.join("package.json"), r#"{"version":"3.1.8"}"#).unwrap();
    let out = run(dir.path(), &["doctor", "--verify", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("4 blockers"), "{}", stdout(&out));
    assert!(stderr(&out).contains("config broke"), "{}", stderr(&out));
    let out = run(dir.path(), &["doctor", "--verify", "--yes", "--json"]);
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["blockers"], 4);
    assert!(
        v["verify"]["error"]
            .as_str()
            .unwrap()
            .contains("config broke"),
        "{v}"
    );
}

#[test]
fn a_page_that_reads_refused_content_is_listed_under_its_blocker() {
    let dir = site(&[
        ("posts/a.njk", "---\ntags: post\n---\n{{ 1 | nope }}\n"),
        (
            "index.njk",
            "{% for p in collections.post %}{{ p.templateContent | safe }}{% endfor %}\n",
        ),
    ]);
    let out = run(dir.path(), &["doctor"]);
    let text = stdout(&out);
    assert!(
        text.contains("1. Filter `nope` isn't defined (1 page, 1 template; also blocks 1 page that uses its content)"),
        "{text}"
    );
    assert!(
        text.ends_with("0 of 2 pages will build; 1 blocker, grouped below.\n"),
        "{text}"
    );
}
