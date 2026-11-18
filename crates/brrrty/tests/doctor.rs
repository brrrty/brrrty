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

/// The corpus run's first repro: `brrrty init` can't port the 11ty config at all (it hands
/// `eleventyConfig` to a package's function, which could do anything), so the folders it
/// sets are lost. 11ty writes `<h1>Hello</h1>`;
/// with default folders Brrrty would write `<h1></h1>`.
#[test]
fn a_config_init_could_not_port_blocks_every_page() {
    let dir = site(&[
        (
            "package.json",
            "{\"devDependencies\":{\"@11ty/eleventy\":\"3.1.8\"}}\n",
        ),
        (
            "eleventy.config.js",
            "const addHelpers = require(\"eleventy-helpers\");\n\nmodule.exports = function (eleventyConfig) {\n  addHelpers(eleventyConfig);\n  return { dir: { input: \"src\", data: \"data\" } };\n};\n",
        ),
        ("src/data/site.json", "{\"title\":\"Hello\"}\n"),
        (
            "src/index.njk",
            "---\npermalink: index.html\n---\n<h1>{{ site.title }}</h1>\n",
        ),
    ]);
    let init = run(dir.path(), &["init", "--yes"]);
    assert_eq!(init.status.code(), Some(0), "{}", stderr(&init));
    let out = run(dir.path(), &["doctor"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{text}{}", stderr(&out));
    assert!(
        text.starts_with("1. `brrrty init` couldn't port any of eleventy.config.js ("),
        "{text}"
    );
    assert!(text.contains("   brrrty.config.rhai:6:1: "), "{text}");
    assert!(
        text.ends_with("0 of 1 page will build; 1 blocker, grouped below.\n"),
        "{text}"
    );
    let json: Value =
        serde_json::from_str(&stdout(&run(dir.path(), &["doctor", "--json"]))).unwrap();
    assert_eq!(json["builds"], false);
    assert_eq!(json["groups"][0]["cause"]["code"], "config-todo");
    assert_eq!(json["groups"][0]["pages"], 1);

    let built = run(dir.path(), &["build"]);
    assert_eq!(built.status.code(), Some(1));
    assert!(stderr(&built).contains("config-todo"), "{}", stderr(&built));
    assert!(!dir.path().join("_site").exists());
}

/// The corpus run's second repro: init ports the folder and the passthrough copy but not
/// the transform, which uppercases every page in 11ty. Every page waits on the TODO, which
/// comes first, and the page's other blockers are still listed after it.
#[test]
fn a_todo_init_left_blocks_every_page_and_comes_first() {
    let dir = site(&[
        (
            "package.json",
            "{\"devDependencies\":{\"@11ty/eleventy\":\"3.1.8\"}}\n",
        ),
        (
            "eleventy.config.js",
            "module.exports = function (eleventyConfig) {\n  eleventyConfig.addPassthroughCopy(\"src/style.css\");\n  eleventyConfig.addTransform(\"shout\", function (content) {\n    return content.toUpperCase();\n  });\n  return { dir: { input: \"src\" } };\n};\n",
        ),
        ("src/index.md", "# hi\n"),
        ("src/other.njk", "{{ 1 | usd }}\n"),
        ("src/style.css", "body {}\n"),
    ]);
    let init = run(dir.path(), &["init", "--yes"]);
    assert_eq!(init.status.code(), Some(0), "{}", stderr(&init));
    let out = run(dir.path(), &["doctor", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["pages"]["building"], 0);
    assert_eq!(v["pages"]["total"], 2);
    let groups = v["groups"].as_array().unwrap();
    assert_eq!(groups[0]["cause"]["code"], "config-todo");
    assert_eq!(groups[0]["pages"], 2);
    assert_eq!(groups[0]["locations"][0]["path"], "brrrty.config.rhai");
    assert_eq!(groups[0]["locations"][0]["line"], 8);
    assert_eq!(groups[1]["cause"]["name"], "usd");

    let built = run(dir.path(), &["build"]);
    assert_eq!(built.status.code(), Some(1));
    assert!(
        stdout(&built).starts_with("Wrote 0 files, copied 0 files"),
        "{}",
        stdout(&built)
    );
    assert!(!dir.path().join("_site").exists());

    // Once the marker is gone, the person has decided: the page builds, and a change to
    // the 11ty config since init is a warning.
    let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
    std::fs::write(
        dir.path().join("brrrty.config.rhai"),
        rhai.replace("// TODO(brrrty): ", "// Ported by hand: "),
    )
    .unwrap();
    std::fs::write(dir.path().join("src/other.njk"), "x\n").unwrap();
    let js = std::fs::read_to_string(dir.path().join("eleventy.config.js")).unwrap();
    std::fs::write(dir.path().join("eleventy.config.js"), js + "// later\n").unwrap();
    let out = run(dir.path(), &["doctor"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("warning: brrrty.config.rhai:4:1: eleventy.config.js changed since brrrty.config.rhai recorded it as ported"),
        "{text}"
    );
    assert!(text.ends_with("All 2 pages will build.\n"), "{text}");
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

/// The review's repro: the build script passes 11ty `--input`, `--config` and `--pathprefix`.
/// init reads the config `--config` names, ports `--input`, and marks the transform and the
/// path prefix; nothing builds until they are dealt with.
#[test]
fn the_build_scripts_flags_are_ported_or_marked() {
    let dir = site(&[
        (
            "package.json",
            "{\"scripts\":{\"build\":\"eleventy --input=src --config=site.config.js --pathprefix=/blog/\"},\"devDependencies\":{\"@11ty/eleventy\":\"3.1.8\"}}\n",
        ),
        (
            "site.config.js",
            "module.exports = function (eleventyConfig) {\n  eleventyConfig.addTransform(\"up\", (c) => c.toUpperCase());\n};\n",
        ),
        ("src/index.md", "# Hi\n\n{{ \"/x/\" | url }}\n"),
    ]);
    let init = run(dir.path(), &["init", "--yes"]);
    assert_eq!(init.status.code(), Some(0), "{}", stderr(&init));
    let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
    for want in [
        "// Written by `brrrty init` from site.config.js",
        "// Ported from: site.config.js sha256:",
        "// TODO(brrrty): port this to Rhai.",
        "// TODO(brrrty): `--pathprefix=/blog/` sets 11ty's `pathPrefix`",
        "// package.json, scripts.build:11ty:\n//     eleventy --input=src --config=site.config.js --pathprefix=/blog/\n",
        "#{\n    dir: #{\n        input: \"src\",\n    },\n}\n",
    ] {
        assert!(rhai.contains(want), "missing {want:?} in:\n{rhai}");
    }
    let out = run(dir.path(), &["doctor"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("has 2 `TODO(brrrty)` markers"), "{text}");
    assert!(
        text.ends_with("0 of 1 page will build; 1 blocker, grouped below.\n"),
        "{text}"
    );
    // `--strict` stops too, and nothing is written.
    let strict = run(dir.path(), &["build", "--strict"]);
    assert_eq!(strict.status.code(), Some(1));
    assert!(
        stderr(&strict).contains("config-todo"),
        "{}",
        stderr(&strict)
    );
    assert!(!dir.path().join("_site").exists());
}

/// With markers left, a passthrough file isn't copied, and an old output folder keeps
/// what it had (as 11ty never cleans it) but gets nothing new.
#[test]
fn markers_copy_nothing_into_an_existing_output_folder() {
    let dir = site(&[
        (
            "brrrty.config.rhai",
            "// TODO(brrrty): port this.\nconfig.add_passthrough_copy(\"img\");\n",
        ),
        ("img/a.png", "png"),
        ("index.md", "# A\n"),
        ("_site/old.html", "old"),
    ]);
    let built = run(dir.path(), &["build"]);
    assert_eq!(built.status.code(), Some(1));
    assert!(
        stdout(&built).starts_with("Wrote 0 files, copied 0 files"),
        "{}",
        stdout(&built)
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("_site/old.html")).unwrap(),
        "old"
    );
    assert!(!dir.path().join("_site/img").exists());
    assert!(!dir.path().join("_site/index.html").exists());
}

/// A hand-written config with an 11ty config beside it warns until it records the port;
/// then a change to the 11ty config, or a newer one 11ty reads first, warns through `build`.
#[test]
fn hand_ports_are_unverified_until_recorded_then_tracked() {
    let js = "module.exports = function () {};\n";
    let dir = site(&[
        ("eleventy.config.js", js),
        (
            "brrrty.config.rhai",
            "config.add_passthrough_copy(\"img\");\n",
        ),
        ("img/a.png", "png"),
        ("index.md", "# A\n"),
    ]);
    let built = run(dir.path(), &["build"]);
    assert_eq!(built.status.code(), Some(0), "{}", stderr(&built));
    let err = stderr(&built);
    assert!(err.contains("config-unverified"), "{err}");
    let hash = brrrty_config::markers::source_hash(js.as_bytes());
    let line = format!("// Ported from: eleventy.config.js {hash}");
    assert!(err.contains(&line), "{err}");
    // Doctor shows the line to add too.
    let doc = stdout(&run(dir.path(), &["doctor"]));
    assert!(doc.contains(&line), "{doc}");

    std::fs::write(
        dir.path().join("brrrty.config.rhai"),
        format!("{line}\nconfig.add_passthrough_copy(\"img\");\n"),
    )
    .unwrap();
    let built = run(dir.path(), &["build"]);
    assert_eq!(stderr(&built), "", "{}", stderr(&built));
    // CRLF line endings are the same file.
    std::fs::write(
        dir.path().join("eleventy.config.js"),
        js.replace('\n', "\r\n"),
    )
    .unwrap();
    assert_eq!(stderr(&run(dir.path(), &["build"])), "");
    // A real change warns, with the new hash.
    std::fs::write(
        dir.path().join("eleventy.config.js"),
        "module.exports = 1;\n",
    )
    .unwrap();
    let err = stderr(&run(dir.path(), &["build"]));
    assert!(err.contains("config-stale"), "{err}");
    assert!(
        err.contains(&brrrty_config::markers::source_hash(
            b"module.exports = 1;\n"
        )),
        "{err}"
    );
    // So does an `.eleventy.js` beside it, which 11ty reads first.
    std::fs::write(dir.path().join("eleventy.config.js"), js).unwrap();
    std::fs::write(dir.path().join(".eleventy.js"), js).unwrap();
    let err = stderr(&run(dir.path(), &["build"]));
    assert!(err.contains("11ty now reads .eleventy.js"), "{err}");
}

fn pkg(build: &str) -> String {
    serde_json::json!({
        "scripts": { "build": build },
        "devDependencies": { "@11ty/eleventy": "3.1.8" }
    })
    .to_string()
}

/// Re-review: common spellings of the 11ty command had their flags dropped silently. Each
/// is now read (`dir.input` ported, `--pathprefix` marked); nothing builds until it's dealt
/// with.
#[test]
fn every_spelling_of_the_command_is_read() {
    for build in [
        "npx eleventy --input=src --pathprefix=/blog/",
        "pnpm exec eleventy --input=src --pathprefix=/blog/",
        "yarn eleventy --input=src --pathprefix=/blog/",
        "node node_modules/@11ty/eleventy/cmd.cjs --input=src --pathprefix=/blog/",
        "eleventy --input=src --pathprefix=/blog/ > log.txt",
        "eleventy --input=src --pathprefix=/blog/ 2>&1",
        "(cd . && eleventy --input=src --pathprefix=/blog/)",
    ] {
        let dir = site(&[("package.json", &pkg(build)), ("src/index.md", "# Hi\n")]);
        let init = run(dir.path(), &["init", "--yes"]);
        assert_eq!(init.status.code(), Some(0), "{build}: {}", stderr(&init));
        let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
        assert!(rhai.contains("input: \"src\""), "{build}:\n{rhai}");
        assert!(
            rhai.contains("// TODO(brrrty): `--pathprefix=/blog/`"),
            "{build}:\n{rhai}"
        );
        let text = stdout(&run(dir.path(), &["doctor"]));
        assert!(
            text.ends_with("0 of 1 page will build; 1 blocker, grouped below.\n"),
            "{build}: {text}"
        );
        assert!(!text.contains("warning"), "{build}: {text}");
    }
}

/// A script that mentions 11ty in a way init can't read, or runs a file that uses 11ty's
/// JavaScript API, is a marker naming the script, never silence.
#[test]
fn unreadable_scripts_and_the_javascript_api_are_marked() {
    for (build, why) in [
        (
            "concurrently \"eleventy --input=src\" \"sass x\"",
            "couldn't read the flags",
        ),
        ("cd site && eleventy", "couldn't read the flags"),
        ("node build.mjs", "uses 11ty's JavaScript API"),
    ] {
        let dir = site(&[
            ("package.json", &pkg(build)),
            (
                "build.mjs",
                "import Eleventy from \"@11ty/eleventy\";\nawait new Eleventy(\"src\").write();\n",
            ),
            ("src/index.md", "# Hi\n"),
        ]);
        let init = run(dir.path(), &["init", "--yes"]);
        assert_eq!(init.status.code(), Some(0), "{build}: {}", stderr(&init));
        let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
        assert!(rhai.contains(why), "{build}:\n{rhai}");
        assert!(
            rhai.contains(&format!("//     {build}\n")),
            "{build}:\n{rhai}"
        );
        let text = stdout(&run(dir.path(), &["doctor"]));
        assert!(text.contains("0 of 1 page will build"), "{build}: {text}");
    }
}

/// Re-review: flags changed in package.json after init, and `--config` dropped from the
/// script, now warn (`config-stale`).
#[test]
fn package_json_changes_after_init_warn() {
    let dir = site(&[
        ("package.json", &pkg("eleventy --config=cfg/site.js")),
        ("cfg/site.js", "module.exports = function () {};\n"),
        (
            "eleventy.config.js",
            "module.exports = function (c) { c.addTransform(\"x\", (s) => s); };\n",
        ),
        ("index.md", "# Hi\n"),
    ]);
    assert_eq!(run(dir.path(), &["init", "--yes"]).status.code(), Some(0));
    let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
    assert!(
        rhai.contains("// Ported from: cfg/site.js sha256:"),
        "{rhai}"
    );
    assert!(
        rhai.contains("// 11ty flags from package.json: sha256:"),
        "{rhai}"
    );
    let built = run(dir.path(), &["build"]);
    assert_eq!(built.status.code(), Some(0), "{}", stderr(&built));
    assert_eq!(stderr(&built), "");

    // New flags in the build script (init kept 11ty's command as build:11ty).
    let edited = std::fs::read_to_string(dir.path().join("package.json"))
        .unwrap()
        .replace(
            "eleventy --config=cfg/site.js",
            "eleventy --config=cfg/site.js --pathprefix=/blog/",
        );
    std::fs::write(dir.path().join("package.json"), &edited).unwrap();
    let err = stderr(&run(dir.path(), &["build"]));
    assert!(
        err.contains("the 11ty flags in package.json, scripts.build:11ty changed"),
        "{err}"
    );

    // `--config` dropped: 11ty reads eleventy.config.js, with its transform.
    std::fs::write(
        dir.path().join("package.json"),
        edited.replace(
            "eleventy --config=cfg/site.js --pathprefix=/blog/",
            "eleventy",
        ),
    )
    .unwrap();
    let err = stderr(&run(dir.path(), &["build"]));
    assert!(
        err.contains("11ty now reads eleventy.config.js, not the cfg/site.js"),
        "{err}"
    );
    assert!(
        err.contains("passes the 11ty flags brrrty.config.rhai records any more"),
        "{err}"
    );
}

/// Re-review: `--config=` means 11ty reads no config, so there's nothing to call unverified.
#[test]
fn an_empty_config_flag_is_not_unverified() {
    let dir = site(&[
        ("package.json", &pkg("eleventy --config=")),
        ("eleventy.config.js", "module.exports = function () {};\n"),
        ("index.md", "# Hi\n"),
    ]);
    assert_eq!(run(dir.path(), &["init", "--yes"]).status.code(), Some(0));
    let built = run(dir.path(), &["build"]);
    assert_eq!(built.status.code(), Some(0), "{}", stderr(&built));
    assert_eq!(stderr(&built), "");
    let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
    assert!(rhai.contains("11ty reads no config file"), "{rhai}");
}

/// Re-review 2: build scripts that still went silent are read or marked.
#[test]
fn bun_run_and_api_builds_are_not_silent() {
    for build in [
        "bun run eleventy --input=src --pathprefix=/blog/",
        "node scripts/b",
        "./scripts/c.mjs",
        "node scripts/d.mjs",
        "node scripts/plain.mjs",
    ] {
        let dir = site(&[
            ("package.json", &pkg(build)),
            ("scripts/b.js", "require(\"@11ty/eleventy\");\n"),
            (
                "scripts/c.mjs",
                "#!/usr/bin/env node\nimport E from \"@11ty/eleventy\";\n",
            ),
            ("scripts/d.mjs", "import \"./lib.mjs\";\n"),
            ("scripts/lib.mjs", "import E from \"@11ty/eleventy\";\n"),
            // Nothing in it reaches 11ty: the dependency backstop marks it.
            ("scripts/plain.mjs", "console.log(1);\n"),
            ("src/index.md", "# Hi\n"),
        ]);
        let init = run(dir.path(), &["init", "--yes"]);
        assert_eq!(init.status.code(), Some(0), "{build}: {}", stderr(&init));
        let text = stdout(&run(dir.path(), &["doctor"]));
        assert!(text.contains("0 of 1 page will build"), "{build}: {text}");
    }
}

/// Re-review 2: once a site has moved off 11ty (no dependency, no config), the recorded
/// lines say nothing. While 11ty is still a dependency, the warning stays, and its help
/// never says to take ported settings out.
#[test]
fn moving_off_11ty_ends_the_checks() {
    let dir = site(&[
        ("package.json", &pkg("eleventy --input=src")),
        ("eleventy.config.js", "module.exports = function () {};\n"),
        ("src/index.md", "# Hi\n"),
    ]);
    assert_eq!(run(dir.path(), &["init", "--yes"]).status.code(), Some(0));
    assert_eq!(stderr(&run(dir.path(), &["build"])), "");
    // The 11ty config deleted while 11ty is still a dependency.
    std::fs::remove_file(dir.path().join("eleventy.config.js")).unwrap();
    let err = stderr(&run(dir.path(), &["build"]));
    assert!(err.contains("11ty reads no config file now"), "{err}");
    assert!(
        err.contains("brrrty init") && !err.contains("Take out"),
        "{err}"
    );
    // 11ty gone from package.json too: the migration is done.
    std::fs::write(
        dir.path().join("package.json"),
        "{\"scripts\":{\"build\":\"brrrty build\",\"start\":\"brrrty serve\"}}\n",
    )
    .unwrap();
    let built = run(dir.path(), &["build"]);
    assert_eq!(built.status.code(), Some(0), "{}", stderr(&built));
    assert_eq!(stderr(&built), "");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("_site/index.html")).unwrap(),
        "<h1>Hi</h1>\n"
    );
}

/// Re-review 2: an absolute `--config` inside the site is that file, at init and at build.
#[test]
fn an_absolute_config_path_is_the_file() {
    let dir = site(&[
        ("cfg/site.js", "module.exports = function () {};\n"),
        ("index.md", "# Hi\n"),
    ]);
    let abs = dir.path().join("cfg/site.js");
    std::fs::write(
        dir.path().join("package.json"),
        pkg(&format!("eleventy --config={}", abs.display())),
    )
    .unwrap();
    assert_eq!(run(dir.path(), &["init", "--yes"]).status.code(), Some(0));
    let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
    assert!(
        rhai.contains("// Ported from: cfg/site.js sha256:"),
        "{rhai}"
    );
    let built = run(dir.path(), &["build"]);
    assert_eq!(built.status.code(), Some(0), "{}", stderr(&built));
    assert_eq!(stderr(&built), "");
}

/// Re-review 3: a deploy config's build command is read like a build script: its flags are
/// ported or marked, and a later change to it warns.
#[test]
fn deploy_configs_carry_the_command_too() {
    for (file, body) in [
        (
            "netlify.toml",
            "[build]\ncommand = \"npx @11ty/eleventy --input=src --pathprefix=/blog/\"\n",
        ),
        (
            "vercel.json",
            "{\"buildCommand\": \"npx @11ty/eleventy --input=src --pathprefix=/blog/\"}\n",
        ),
        (
            ".github/workflows/deploy.yml",
            "jobs:\n  b:\n    steps:\n      - run: |\n          npm ci\n          npx @11ty/eleventy --input=src --pathprefix=/blog/\n",
        ),
        (
            ".gitlab-ci.yml",
            "pages:\n  script:\n    - npm ci\n    - npx @11ty/eleventy --input=src --pathprefix=/blog/\n",
        ),
    ] {
        let dir = site(&[
            (
                "package.json",
                "{\"devDependencies\":{\"@11ty/eleventy\":\"3.1.8\"}}\n",
            ),
            (file, body),
            ("src/index.md", "# Hi\n"),
        ]);
        let init = run(dir.path(), &["init", "--yes"]);
        assert_eq!(init.status.code(), Some(0), "{file}: {}", stderr(&init));
        let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
        assert!(rhai.contains("input: \"src\""), "{file}:\n{rhai}");
        assert!(rhai.contains("`--pathprefix=/blog/`"), "{file}:\n{rhai}");
        assert!(rhai.contains(&format!("// {file}")), "{file}:\n{rhai}");
        let text = stdout(&run(dir.path(), &["doctor"]));
        assert!(
            text.ends_with("0 of 1 page will build; 1 blocker, grouped below.\n"),
            "{file}: {text}"
        );

        // Ported: the marker deleted. Then the deploy config changes.
        let ported: String = rhai
            .lines()
            .filter(|l| !l.starts_with("// TODO(brrrty)"))
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(dir.path().join("brrrty.config.rhai"), ported).unwrap();
        assert_eq!(stderr(&run(dir.path(), &["build"])), "", "{file}");
        std::fs::write(dir.path().join(file), body.replace("/blog/", "/b/")).unwrap();
        let err = stderr(&run(dir.path(), &["build"]));
        assert!(err.contains("config-stale"), "{file}: {err}");
    }
}

/// Re-review 4: a GitLab anchor hides the command from the line scan; the line that
/// mentions 11ty is a marker, so the site never reads as building.
#[test]
fn a_ci_line_the_scan_cant_read_is_marked() {
    let dir = site(&[
        (
            "package.json",
            "{\"devDependencies\":{\"@11ty/eleventy\":\"3.1.8\"}}\n",
        ),
        (
            ".gitlab-ci.yml",
            ".build: &build\n  - npx @11ty/eleventy --input=src --pathprefix=/blog/\npages:\n  script: *build\n",
        ),
        ("src/index.md", "# Hi\n"),
    ]);
    assert_eq!(run(dir.path(), &["init", "--yes"]).status.code(), Some(0));
    let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
    assert!(rhai.contains("// .gitlab-ci.yml, line 2:"), "{rhai}");
    let text = stdout(&run(dir.path(), &["doctor"]));
    assert!(text.contains("0 of 1 page will build"), "{text}");
}

/// The shape of the real blog's `ci/compare.mjs`: its runs in a `const` list of objects,
/// with node's `--require` preload before 11ty's `cmd.cjs`, and a loop that spawns each.
const COMPARE_MJS: &str = "import { spawnSync } from \"node:child_process\";\nimport { existsSync, rmSync } from \"node:fs\";\nimport { performance } from \"node:perf_hooks\";\n\nconst RUNS = Number(process.env.RUNS ?? 5);\nconst OUT_11TY = \"_site-11ty\";\nconst OUT_BRRRTY = \"_site-brrrty\";\nconst env = { ...process.env, SOURCE_DATE_EPOCH: \"1767268800\" };\nif (!existsSync(\"node_modules/@11ty/eleventy/cmd.cjs\")) process.exit(1);\n\nconst tools = [\n  {\n    name: \"11ty\",\n    out: OUT_11TY,\n    cmd: process.execPath,\n    args: [\n      \"--require\", \"./ci/freeze-time.cjs\",\n      \"node_modules/@11ty/eleventy/cmd.cjs\", `--output=${OUT_11TY}`, \"--quiet\",\n    ],\n    times: [],\n  },\n  {\n    name: \"Brrrty\",\n    out: OUT_BRRRTY,\n    cmd: \"node_modules/.bin/brrrty\",\n    args: [\"build\", `--output=${OUT_BRRRTY}`, \"--quiet\"],\n    times: [],\n  },\n];\n\nfor (let run = 1; run <= RUNS; run++) {\n  for (const tool of tools) {\n    rmSync(tool.out, { recursive: true, force: true });\n    const start = performance.now();\n    const result = spawnSync(tool.cmd, tool.args, { env, stdio: \"inherit\" });\n    if (result.status !== 0) process.exit(1);\n    tool.times.push(performance.now() - start);\n  }\n}\nconst [eleventy, brrrty] = tools;\nconsole.log(eleventy.times.length, brrrty.times.length);\nconst diff = spawnSync(\"diff\", [\"-r\", OUT_11TY, OUT_BRRRTY], { encoding: \"utf8\" });\nprocess.exit(diff.status);\n";
const COMPARE_CI: &str = "build:\n  script:\n    - npm ci\n    - npm run build\ncompare:\n  script:\n    - npm ci\n    - node ci/compare.mjs\n";

/// A synthetic copy of a real blog's setup: a CI job spawns 11ty's command line with
/// `--output` beside Brrrty's build to compare them. That's a comparison run, not a
/// build and not API use: init marks nothing, and the page builds.
#[test]
fn a_ci_comparison_run_is_not_marked() {
    // As the blog has it: Brrrty is the build already, and 11ty's command is kept.
    let dir = site(&[
        (
            "package.json",
            "{\"scripts\":{\"build\":\"brrrty build\",\"build:11ty\":\"npx @11ty/eleventy\"},\"devDependencies\":{\"@11ty/eleventy\":\"3.1.8\"}}\n",
        ),
        ("eleventy.config.js", "module.exports = function () {};\n"),
        (".gitlab-ci.yml", COMPARE_CI),
        ("ci/compare.mjs", COMPARE_MJS),
        ("index.md", "# Hi\n"),
    ]);
    let init = run(dir.path(), &["init", "--yes"]);
    assert_eq!(init.status.code(), Some(0), "{}", stderr(&init));
    let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
    assert!(!rhai.contains("TODO(brrrty)"), "{rhai}");
    let out = run(dir.path(), &["doctor"]);
    assert_eq!(stdout(&out), "The 1 page will build.\n");
}

/// The same site, ported by 0.0.19: its header records the hash 0.0.19 gave the scripts
/// (with the comparison run read as API use), and the marker was deleted by hand. It
/// stays in step: no warning.
#[test]
fn a_config_0_0_19_recorded_stays_in_step() {
    let js = "module.exports = function () {};\n";
    let dir = site(&[
        (
            "package.json",
            "{\"scripts\":{\"build\":\"brrrty build\",\"build:11ty\":\"npx @11ty/eleventy\"},\"devDependencies\":{\"@11ty/eleventy\":\"3.1.8\"}}\n",
        ),
        ("eleventy.config.js", js),
        (".gitlab-ci.yml", COMPARE_CI),
        ("ci/compare.mjs", COMPARE_MJS),
        ("index.md", "# Hi\n"),
    ]);
    let old = brrrty_config::eleventy_cli::legacy_fingerprint(dir.path()).unwrap();
    let hash = brrrty_config::markers::source_hash(js.as_bytes());
    std::fs::write(
        dir.path().join("brrrty.config.rhai"),
        format!(
            "// Ported from: eleventy.config.js {hash}\n// 11ty flags from package.json: {old}\n"
        ),
    )
    .unwrap();
    let built = run(dir.path(), &["build"]);
    assert_eq!(built.status.code(), Some(0), "{}", stderr(&built));
    assert_eq!(stderr(&built), "");
}

/// Review: the standard GitLab Pages setup, through `init` and `doctor`. init rewrites
/// `build` to Brrrty's, but reads the scripts as they were, so the pages job's
/// `--output=public` is a conflict with `build`'s `_site`, not a comparison.
#[test]
fn a_gitlab_pages_site_is_marked_through_init() {
    let dir = site(&[
        ("package.json", &pkg("eleventy")),
        (
            ".gitlab-ci.yml",
            "pages:\n  script:\n    - npm ci\n    - npx @11ty/eleventy --output=public\n  artifacts:\n    paths:\n      - public\n",
        ),
        ("index.md", "# Hi\n"),
    ]);
    let init = run(dir.path(), &["init", "--yes"]);
    assert_eq!(init.status.code(), Some(0), "{}", stderr(&init));
    let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
    assert!(
        rhai.contains("// TODO(brrrty): 11ty writes to `_site`"),
        "{rhai}"
    );
    assert!(
        rhai.contains(
            "in package.json, scripts.build:11ty but to `public` in .gitlab-ci.yml, line 4"
        ),
        "{rhai}"
    );
    let text = stdout(&run(dir.path(), &["doctor"]));
    assert!(
        text.ends_with("0 of 1 page will build; 1 blocker, grouped below.\n"),
        "{text}"
    );
    // Ported: `dir.output` set to `public` and the marker deleted. As package.json now
    // has Brrrty as `build`, the job is a comparison from here on, and nothing warns.
    let ported: String = rhai
        .lines()
        .filter(|l| !l.starts_with("// TODO(brrrty)"))
        .map(|l| format!("{l}\n"))
        .collect::<String>()
        + "#{ dir: #{ output: \"public\" } }\n";
    std::fs::write(dir.path().join("brrrty.config.rhai"), ported).unwrap();
    let built = run(dir.path(), &["build"]);
    assert_eq!(built.status.code(), Some(0), "{}", stderr(&built));
    assert_eq!(stderr(&built), "");
    assert!(dir.path().join("public/index.html").is_file());
}

/// Before the switch to Brrrty, the same compare job beside an 11ty `build` isn't a
/// comparison with a Brrrty build: its other output is a conflict to resolve.
#[test]
fn a_compare_job_beside_an_11ty_build_is_marked() {
    let dir = site(&[
        ("package.json", &pkg("npx @11ty/eleventy")),
        (".gitlab-ci.yml", COMPARE_CI),
        ("ci/compare.mjs", COMPARE_MJS),
        ("index.md", "# Hi\n"),
    ]);
    assert_eq!(run(dir.path(), &["init", "--yes"]).status.code(), Some(0));
    let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
    assert!(
        rhai.contains(".gitlab-ci.yml, line 8 → ci/compare.mjs"),
        "{rhai}"
    );
    assert!(
        rhai.contains("// package.json, scripts.build:11ty:"),
        "{rhai}"
    );
}

/// Review: init dropped a build script's `TZ=`, so Brrrty built with another time zone and
/// the dates differed. It's carried into `brrrty build` now.
#[test]
fn init_carries_the_build_scripts_tz() {
    for (build, want) in [
        (
            "TZ=Asia/Tokyo eleventy --input=src",
            "TZ=Asia/Tokyo brrrty build",
        ),
        (
            "cross-env TZ=Asia/Tokyo eleventy --input=src",
            "cross-env TZ=Asia/Tokyo brrrty build",
        ),
        (
            "env TZ=Asia/Tokyo eleventy --input=src",
            "TZ=Asia/Tokyo brrrty build",
        ),
        // Only TZ is carried: Brrrty isn't Node.
        (
            "NODE_ENV=production TZ=Asia/Tokyo eleventy --input=src",
            "TZ=Asia/Tokyo brrrty build",
        ),
    ] {
        let dir = site(&[("package.json", &pkg(build)), ("src/index.md", "# Hi\n")]);
        let init = run(dir.path(), &["init", "--yes"]);
        assert_eq!(init.status.code(), Some(0), "{}", stderr(&init));
        let pkg: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("package.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(pkg["scripts"]["build"], want, "{build}");
        // And `brrrty serve`, which writes the output folder too.
        assert_eq!(
            pkg["scripts"]["start"],
            want.replace("brrrty build", "brrrty serve"),
            "{build}"
        );
        assert_eq!(pkg["scripts"]["build:11ty"], build);
        let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
        assert!(!rhai.contains("TODO(brrrty)"), "{build}:\n{rhai}");
        let built = run(dir.path(), &["build"]);
        assert_eq!(built.status.code(), Some(0), "{}", stderr(&built));
        assert_eq!(stderr(&built), "", "{build}");
    }
}

/// Review: a preload runs code inside 11ty; init names the file in a marker.
#[test]
fn init_marks_a_preload() {
    let dir = site(&[
        (
            "package.json",
            &pkg("node -r ./patch.cjs node_modules/@11ty/eleventy/cmd.cjs"),
        ),
        ("index.md", "# Hi\n"),
    ]);
    assert_eq!(run(dir.path(), &["init", "--yes"]).status.code(), Some(0));
    let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
    assert!(
        rhai.contains("// TODO(brrrty): 11ty runs with ./patch.cjs preloaded"),
        "{rhai}"
    );
    assert!(stdout(&run(dir.path(), &["doctor"])).contains("0 of 1 page will build"));
}

/// A build script with other commands beside 11ty (`sass … && eleventy`): they can't
/// change 11ty's environment, so nothing is marked, and init keeps them, in order,
/// replacing only the 11ty step (with its TZ).
#[test]
fn init_keeps_other_commands_in_a_build_script() {
    for (build, want, serve) in [
        (
            "sass src/a.scss _site/a.css && eleventy --input=src",
            "sass src/a.scss _site/a.css && brrrty build",
            "brrrty serve",
        ),
        (
            "tailwindcss -i a.css -o _site/a.css && postcss b.css -o _site/b.css && TZ=Asia/Tokyo eleventy --input=src",
            "tailwindcss -i a.css -o _site/a.css && postcss b.css -o _site/b.css && TZ=Asia/Tokyo brrrty build",
            "TZ=Asia/Tokyo brrrty serve",
        ),
        (
            "node scripts/prep.js && npx @11ty/eleventy --input=src && node scripts/after.js",
            "node scripts/prep.js && brrrty build && node scripts/after.js",
            "brrrty serve",
        ),
    ] {
        let dir = site(&[
            ("package.json", &pkg(build)),
            ("scripts/prep.js", "console.log(1);\n"),
            ("scripts/after.js", "console.log(2);\n"),
            ("src/index.md", "# Hi\n"),
        ]);
        let init = run(dir.path(), &["init", "--yes"]);
        assert_eq!(init.status.code(), Some(0), "{}", stderr(&init));
        let pkg: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("package.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(pkg["scripts"]["build"], want, "{build}");
        assert_eq!(pkg["scripts"]["build:11ty"], build);
        assert_eq!(pkg["scripts"]["start"], serve, "{build}");
        let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
        assert!(!rhai.contains("TODO(brrrty)"), "{build}:\n{rhai}");
        assert!(rhai.contains("input: \"src\""), "{build}:\n{rhai}");
        let text = stdout(&run(dir.path(), &["doctor"]));
        assert_eq!(text, "The 1 page will build.\n", "{build}");
    }
}

/// A shell builtin that changes state beside 11ty is marked through init.
#[test]
fn init_marks_a_builtin_beside_11ty() {
    let dir = site(&[
        ("package.json", &pkg("export TZ=Asia/Tokyo && eleventy")),
        ("index.md", "# Hi\n"),
    ]);
    assert_eq!(run(dir.path(), &["init", "--yes"]).status.code(), Some(0));
    let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
    assert!(
        rhai.contains("couldn't read the flags this script passes to 11ty"),
        "{rhai}"
    );
    assert!(stdout(&run(dir.path(), &["doctor"])).contains("0 of 1 page will build"));
}

/// A non-English locale in the environment Brrrty runs in: 11ty run here would format
/// dates in it, and Brrrty formats them in English. A C or English locale is quiet.
#[test]
fn a_non_english_locale_warns() {
    let dir = site(&[("index.md", "# Hi\n")]);
    let build = |vars: &[(&str, &str)]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_brrrty"));
        c.arg("build")
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .env_remove("LC_ALL")
            .env_remove("LC_MESSAGES")
            .env_remove("LANG")
            .env_remove("SOURCE_DATE_EPOCH");
        for (k, v) in vars {
            c.env(k, v);
        }
        c.output().unwrap()
    };
    let out = build(&[("LANG", "de_DE.UTF-8")]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stderr(&out).contains(
            "In this environment, LANG=de_DE.UTF-8, so 11ty formats dates like %x in de-DE"
        ),
        "{}",
        stderr(&out)
    );
    for loud in [
        vec![("LANG", "en_GB.UTF-8")],
        vec![("LANG", "en_CA.UTF-8")],
        vec![("LANG", "")],
        vec![("LC_ALL", ""), ("LANG", "en_US.UTF-8")],
    ] {
        let out = build(&loud);
        assert!(
            stderr(&out).contains("locale-differs"),
            "{loud:?}: {}",
            stderr(&out)
        );
    }
    for quiet in [
        vec![],
        vec![("LANG", "C.UTF-8")],
        vec![("LANG", "en_US.UTF-8")],
        vec![("LC_ALL", "C.UTF-8"), ("LANG", "C")],
    ] {
        let out = build(&quiet);
        assert_eq!(stderr(&out), "", "{quiet:?}");
    }
}

/// init marks a build script's non-English locale, which it can't carry.
#[test]
fn init_marks_an_inline_locale() {
    let dir = site(&[
        ("package.json", &pkg("LANG=de_DE.UTF-8 eleventy")),
        ("index.md", "# Hi\n"),
    ]);
    assert_eq!(run(dir.path(), &["init", "--yes"]).status.code(), Some(0));
    let rhai = std::fs::read_to_string(dir.path().join("brrrty.config.rhai")).unwrap();
    assert!(
        rhai.contains("// TODO(brrrty): 11ty runs with LANG=de_DE.UTF-8, so 11ty formats dates like %x in de-DE (15.3.2024); Brrrty uses en-US (3/15/2024)"),
        "{rhai}"
    );
}

fn node_here() -> bool {
    let ok = Command::new("node").arg("--version").output().is_ok();
    if !ok {
        assert!(std::env::var_os("CI").is_none(), "CI needs Node");
        eprintln!("skipped: no node on PATH");
    }
    ok
}

/// JavaScript data on: one line before it runs, then a section naming the
/// mode, where it came from, the Node and each file's state.
#[test]
fn javascript_data_on_lists_each_file_and_its_state() {
    if !node_here() {
        return;
    }
    let dir = site(&[
        ("brrrty.config.rhai", "\nconfig.set_js_data(\"node\");\n"),
        ("_data/site.js", "module.exports = { title: \"x\" };\n"),
        ("_data/nav.js", "module.exports = [\"a\", \"b\"];\n"),
        ("_data/nav.rhai", "[\"a\", \"b\"]\n"),
        ("_data/menu.js", "module.exports = { items: 2 };\n"),
        ("_data/menu.rhai", "#{ items: 3 }\n"),
        (
            "_data/bad.js",
            "module.exports = () => { throw new Error(\"no\"); };\n",
        ),
        (
            "posts/posts.11tydata.js",
            "module.exports = { tags: \"post\" };\n",
        ),
        ("posts/a.md", "a\n"),
        ("index.njk", "{{ site.title }} {{ nav }} {{ menu.items }}\n"),
    ]);
    let out = run(dir.path(), &["doctor"]);
    let text = stdout(&out);
    assert!(
        stderr(&out).contains("Running JavaScript data in Node"),
        "{}",
        stderr(&out)
    );
    assert!(
        text.contains("JavaScript data: on (brrrty.config.rhai line 2), Node v"),
        "{text}"
    );
    for want in [
        "_data/site.js",
        "runs in Node",
        "_data/nav.js",
        "ported: nav.rhai is identical, delete nav.js",
        "_data/menu.js",
        "ported: menu.rhai differs at data.items (value)",
        "_data/bad.js",
        "throws",
        "posts/posts.11tydata.js",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    let json: Value =
        serde_json::from_slice(&run(dir.path(), &["doctor", "--json"]).stdout).unwrap();
    let js = &json["jsData"];
    assert_eq!(js["mode"], "node");
    assert_eq!(js["source"], "config");
    assert!(js["node"]["version"].as_str().unwrap().starts_with('v'));
    let state = |f: &str| {
        js["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["path"] == f)
            .unwrap_or_else(|| panic!("{f} in {js}"))
            .clone()
    };
    assert_eq!(state("_data/site.js")["state"], "node");
    assert_eq!(state("_data/nav.js")["state"], "ported-identical");
    assert_eq!(state("_data/menu.js")["state"], "ported-differs");
    assert_eq!(state("_data/menu.js")["differsAt"], "data.items (value)");
    assert_eq!(state("_data/bad.js")["state"], "error");
}

/// Off: one sentence with the files, the pages waiting and both fixes, and
/// each file's state, translatable or not.
#[test]
fn javascript_data_off_says_how_many_pages_wait_and_both_fixes() {
    let dir = site(&[
        ("_data/site.js", "module.exports = { title: \"x\" };\n"),
        (
            "_data/posts.js",
            "const { DateTime } = require(\"luxon\");\nmodule.exports = [DateTime.now().year];\n",
        ),
        ("index.njk", "{{ site.title }} {{ posts }}\n"),
        ("other.njk", "{{ site.title }}\n"),
    ]);
    let out = run(dir.path(), &["doctor", "--js-data=off"]);
    let text = stdout(&out);
    assert!(text.contains("JavaScript data: off (--js-data). 2 files, 2 templates wait on them. Turn it on with config.set_js_data(\"node\")"), "{text}");
    assert!(
        text.contains("brrrty init lists which it can translate"),
        "{text}"
    );
    assert!(
        text.contains("init can translate it (literal data)"),
        "{text}"
    );
    let json: Value =
        serde_json::from_slice(&run(dir.path(), &["doctor", "--json"]).stdout).unwrap();
    let js = &json["jsData"];
    assert_eq!(js["mode"], "off");
    assert_eq!(js["source"], "default");
    let files = js["files"].as_array().unwrap();
    let site_js = files.iter().find(|f| f["path"] == "_data/site.js").unwrap();
    assert_eq!(site_js["state"], "translatable");
    let posts = files
        .iter()
        .find(|f| f["path"] == "_data/posts.js")
        .unwrap();
    assert_eq!(posts["state"], "blocked");
}

/// No JavaScript data files: no section.
#[test]
fn a_site_without_javascript_data_has_no_section() {
    let dir = site(&[("index.md", "hi\n")]);
    let text = stdout(&run(dir.path(), &["doctor"]));
    assert!(!text.contains("JavaScript data"), "{text}");
    let json: Value =
        serde_json::from_slice(&run(dir.path(), &["doctor", "--json"]).stdout).unwrap();
    assert_eq!(json["jsData"]["files"], serde_json::json!([]));
}

/// Local JavaScript data makes 11ty's own collections order depend on file
/// reads, so a page that reads `collections` is refused for that reason
/// itself: a blocker with its fix, not a page waiting on one.
#[test]
fn collections_with_local_javascript_data_build_when_it_runs_and_block_when_it_is_off() {
    if !has_node() {
        return;
    }
    let files = [
        (
            "sharks/sharks.11tydata.js",
            "module.exports = { tags: \"shark\" };\n",
        ),
        ("sharks/a.njk", "a\n"),
        ("sharks/b.njk", "b\n"),
        (
            "sitemap.njk",
            "{% for p in collections.all %}{{ p.url }}\n{% endfor %}",
        ),
    ];
    // On, the data runs, and the collections are the same whatever order
    // 11ty's file reads give: every page builds.
    let mut on = files.to_vec();
    on.push(("brrrty.config.rhai", "config.set_js_data(\"node\");\n"));
    let dir = site(&on);
    let text = stdout(&run(dir.path(), &["doctor"]));
    assert!(text.contains("All 3 pages will build."), "{text}");
    // Off, the data file that sets `tags` is the blocker, with its fix, and
    // the sitemap waits on the pages it refuses.
    let dir = site(&files);
    let text = stdout(&run(dir.path(), &["doctor"]));
    assert!(
        text.contains("1. JavaScript data file sharks/sharks.11tydata.js"),
        "{text}"
    );
    assert!(
        text.contains("Fix: Turn on JavaScript data with config.set_js_data(\"node\")"),
        "{text}"
    );
    assert!(text.contains("2. Waiting on the blockers above"), "{text}");
    assert!(
        text.ends_with("0 of 3 pages will build; 1 blocker, grouped below.\n"),
        "{text}"
    );
}

/// Data that doesn't run and doesn't set a key that changes how pages are
/// built still leaves the collections unknown (it may set `tags` at run
/// time): the reading page is a blocker whose fix turns JavaScript on.
#[test]
fn collections_with_local_javascript_data_that_is_off_name_the_switch() {
    let dir = site(&[
        (
            "sharks/sharks.11tydata.js",
            "module.exports = { fin: 1 };\n",
        ),
        ("sharks/a.njk", "a\n"),
        (
            "sitemap.njk",
            "{% for p in collections.all %}{{ p.url }}\n{% endfor %}",
        ),
    ]);
    let text = stdout(&run(dir.path(), &["doctor"]));
    assert!(
        text.contains("a JavaScript data file that doesn't run"),
        "{text}"
    );
    assert!(
        text.contains("Fix: Turn JavaScript data on (`config.set_js_data(\"node\");`"),
        "{text}"
    );
    assert!(!text.contains("depends on file reads"), "{text}");
}

/// A JavaScript template that reads `collections` the build can't make
/// waits on what stops them, as a Nunjucks page does: one blocker.
#[test]
fn javascript_reads_of_collections_under_a_gap_wait_on_its_blocker() {
    if !has_node() {
        return;
    }
    let dir = site(&[
        ("brrrty.config.rhai", "config.set_js_data(\"node\");\n"),
        (
            "pk.njk",
            "---\npagination:\n  data: collections\n  size: 1\n---\nx",
        ),
        (
            "a.11ty.js",
            "module.exports = (d) => String(d.collections.all.length);\n",
        ),
        (
            "b.11ty.js",
            "module.exports = (d) => String(d.collections.all.length);\n",
        ),
        ("c.njk", "{{ collections.all | length }}"),
    ]);
    let text = stdout(&run(dir.path(), &["doctor"]));
    assert!(
        text.contains("Waiting on the blockers above (3 pages, 3 templates)"),
        "{text}"
    );
    assert!(text.contains("; 1 blocker, grouped below."), "{text}");
}

/// With JavaScript data on and no Node, the collections wait on the
/// missing Node, which is the one blocker; nothing says to turn it on.
#[test]
fn collections_with_local_javascript_data_and_no_node_wait_on_node() {
    let dir = site(&[
        ("brrrty.config.rhai", "config.set_js_data(\"node\");\n"),
        (
            "sharks/sharks.11tydata.js",
            "module.exports = { fin: 1 };\n",
        ),
        ("sharks/a.njk", "a\n"),
        (
            "sitemap.njk",
            "{% for p in collections.all %}{{ p.url }}\n{% endfor %}",
        ),
    ]);
    let out = Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .args(["doctor"])
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .env_remove("SOURCE_DATE_EPOCH")
        .env("BRRRTY_NODE", "/nonexistent/node")
        .output()
        .unwrap();
    let text = stdout(&out);
    assert!(!text.contains("Turn JavaScript data on"), "{text}");
    assert!(text.contains("couldn't run"), "{text}");
}
