//! `brrrty serve`'s watch loop: rebuild on change, swap, keep the last good build, clean up.
//!
//! Timing: every wait polls toward a condition with a generous deadline, and "nothing
//! happened" checks wait well past the debounce and the watcher's own latency.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use brrrty::options::Flags;
use brrrty::serve::{Finished, Rebuild, STAGING_DIR, Session, Settings, StopHandle};
use brrrty_serve::{RunningServer, Server};

/// Long enough for any build or file event on a loaded CI machine.
const DEADLINE: Duration = Duration::from_secs(30);
/// How long "no rebuild happened" is watched for: many times the debounce and the
/// watcher's latency.
const QUIET: Duration = Duration::from_millis(1500);

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn site() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    write(
        root,
        "src/_includes/base.liquid",
        "<main>{{ content }}</main>",
    );
    write(root, "src/index.md", "---\nlayout: base.liquid\n---\nv0\n");
    write(root, "src/style.css", "body{}");
    t
}

fn get(server: &RunningServer, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(server.local_addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status = text[9..12].parse().unwrap();
    let body = text.split_once("\r\n\r\n").unwrap().1.to_string();
    (status, body)
}

/// Poll until `f` holds.
fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let until = Instant::now() + DEADLINE;
    while !f() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

struct Live<'a> {
    root: &'a Path,
    server: &'a RunningServer,
    stop: StopHandle,
    rebuilds: Receiver<Rebuild>,
}

impl Live<'_> {
    /// Wait out any events from writing the site just before the watcher started.
    fn settle(&self) {
        while self.rebuilds.recv_timeout(QUIET).is_ok() {}
    }

    fn next_rebuild(&self) -> Rebuild {
        self.rebuilds
            .recv_timeout(DEADLINE)
            .expect("a rebuild within the deadline")
    }

    fn no_rebuild(&self) {
        if let Ok(r) = self.rebuilds.recv_timeout(QUIET) {
            panic!("unexpected rebuild for {:?}", r.changed_paths);
        }
    }

    fn page(&self) -> String {
        get(self.server, "/").1
    }
}

/// Start a session on `root`, serve it, and run `test` while the watch loop runs.
fn with_session(root: &Path, test: impl FnOnce(&Live)) -> Finished {
    with_session_holding(root, Duration::ZERO, test)
}

/// [`with_session`], with the session's thread held up for `hold` after the first rebuild,
/// as if that build were still running.
fn with_session_holding(root: &Path, hold: Duration, test: impl FnOnce(&Live)) -> Finished {
    let flags = Flags {
        input: Some(PathBuf::from("src")),
        ..Flags::default()
    };
    with_session_flags(root, flags, hold, test)
}

/// [`with_session_holding`] with the given flags.
fn with_session_flags(
    root: &Path,
    flags: Flags,
    hold: Duration,
    test: impl FnOnce(&Live),
) -> Finished {
    let settings = Settings {
        flags,
        quiet: true,
        ..Settings::default()
    };
    let mut session = Session::start(root, settings).unwrap();
    let (tx, rx) = mpsc::channel();
    let first = std::sync::atomic::AtomicBool::new(true);
    session.on_rebuild(Arc::new(move |r: &Rebuild| {
        let _ = tx.send(r.clone());
        if first.swap(false, std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(hold);
        }
    }));
    let server = Server::start(session.serving(), "localhost", 0).unwrap();
    let stop = session.stop_handle();
    std::thread::scope(|s| {
        let running = s.spawn(|| session.run(&server));
        let live = Live {
            root,
            server: &server,
            stop: stop.clone(),
            rebuilds: rx,
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| test(&live)));
        live.stop.stop();
        let finished = running.join().unwrap();
        if let Err(p) = outcome {
            std::panic::resume_unwind(p);
        }
        finished
    })
}

#[test]
fn an_edit_rebuilds_and_serves_the_new_bytes() {
    let t = site();
    with_session(t.path(), |live| {
        assert_eq!(live.page(), "<main><p>v0</p>\n</main>");
        live.settle();
        write(
            live.root,
            "src/index.md",
            "---\nlayout: base.liquid\n---\nv1\n",
        );
        let r = live.next_rebuild();
        assert!(r.ok);
        assert_eq!(r.changed_paths, vec![PathBuf::from("src/index.md")]);
        assert!(!r.css_only);
        assert_eq!(live.page(), "<main><p>v1</p>\n</main>");
    });
}

#[test]
fn a_file_a_data_script_reads_rebuilds_wherever_it_is() {
    let t = site();
    let root = t.path();
    write(
        root,
        "src/_data/names.rhai",
        r#"parse_csv(read_text("data/names.csv"), #{ columns: true })"#,
    );
    write(root, "data/names.csv", "name\nAda\n");
    write(root, "src/_data/notes.rhai", r#"read_dir("notes")"#);
    write(root, "notes/a.md", "");
    write(
        root,
        "src/index.md",
        "---\nlayout: base.liquid\n---\n{% for n in names %}{{ n.name }} {% endfor %}{{ notes | join: ',' }}\n",
    );
    with_session(root, |live| {
        assert_eq!(live.page(), "<main><p>Ada a.md</p>\n</main>");
        live.settle();
        // Outside the input folder, so only the script's read says it matters.
        write(live.root, "data/names.csv", "name\nGrace\n");
        let r = live.next_rebuild();
        assert!(r.ok);
        assert_eq!(r.changed_paths, vec![PathBuf::from("data/names.csv")]);
        assert_eq!(live.page(), "<main><p>Grace a.md</p>\n</main>");
        // A file appearing in a folder a script listed.
        write(live.root, "notes/b.md", "");
        let r = live.next_rebuild();
        assert!(r.ok);
        assert_eq!(live.page(), "<main><p>Grace a.md,b.md</p>\n</main>");
        // Anything else outside the input folder still doesn't count.
        write(live.root, "other/x.txt", "x");
        live.no_rebuild();
    });
}

/// A recursive listing of the project folder: a file added in one of its subfolders
/// outside the input folder rebuilds.
#[test]
fn a_recursive_listing_of_the_project_folder_sees_new_files() {
    let t = site();
    let root = t.path();
    write(root, "data/a.md", "");
    write(
        root,
        "src/_data/names.rhai",
        r#"let out = []; for n in read_dir(".", #{ recursive: true }) { if n.starts_with("data/") { out.push(n); } } out"#,
    );
    write(
        root,
        "src/index.md",
        "---\nlayout: base.liquid\n---\n{{ names | join: ',' }}\n",
    );
    with_session(root, |live| {
        assert_eq!(live.page(), "<main><p>data/a.md</p>\n</main>");
        live.settle();
        write(live.root, "data/b.md", "");
        let r = live.next_rebuild();
        assert!(r.ok);
        assert_eq!(live.page(), "<main><p>data/a.md,data/b.md</p>\n</main>");
    });
}

/// A read under `node_modules`, which serve otherwise never watches.
#[test]
fn a_read_under_node_modules_rebuilds() {
    let t = site();
    let root = t.path();
    write(
        root,
        "node_modules/x/package.json",
        r#"{"version": "1.0.0"}"#,
    );
    write(
        root,
        "src/_data/x.rhai",
        r#"read_json("node_modules/x/package.json")"#,
    );
    write(
        root,
        "src/index.md",
        "---\nlayout: base.liquid\n---\n{{ x.version }}\n",
    );
    with_session(root, |live| {
        assert_eq!(live.page(), "<main><p>1.0.0</p>\n</main>");
        live.settle();
        write(
            live.root,
            "node_modules/x/package.json",
            r#"{"version": "2.0.0"}"#,
        );
        let r = live.next_rebuild();
        assert!(r.ok);
        assert_eq!(live.page(), "<main><p>2.0.0</p>\n</main>");
    });
}

/// `data/current -> v1` retargeted to `v2`: the event is on the symlink, not on
/// anything the build read through it.
#[cfg(unix)]
#[test]
fn retargeting_a_symlink_on_the_way_to_a_read_rebuilds() {
    let t = site();
    let root = t.path();
    write(root, "data/v1/a.txt", "one");
    write(root, "data/v2/a.txt", "two");
    std::os::unix::fs::symlink("v1", root.join("data/current")).unwrap();
    write(
        root,
        "src/_data/x.rhai",
        r#"read_text("data/current/a.txt")"#,
    );
    write(
        root,
        "src/index.md",
        "---\nlayout: base.liquid\n---\n{{ x }}\n",
    );
    with_session(root, |live| {
        assert_eq!(live.page(), "<main><p>one</p>\n</main>");
        live.settle();
        let tmp = live.root.join("data/next");
        std::os::unix::fs::symlink("v2", &tmp).unwrap();
        fs::rename(&tmp, live.root.join("data/current")).unwrap();
        let r = live.next_rebuild();
        assert!(r.ok);
        assert_eq!(live.page(), "<main><p>two</p>\n</main>");
    });
}

#[test]
fn an_include_and_a_stylesheet_rebuild_and_css_alone_is_marked() {
    let t = site();
    with_session(t.path(), |live| {
        live.settle();
        write(live.root, "src/style.css", "body{color:red}");
        let r = live.next_rebuild();
        assert!(r.ok && r.css_only, "{r:?}");
        write(
            live.root,
            "src/_includes/base.liquid",
            "<div>{{ content }}</div>",
        );
        let r = live.next_rebuild();
        assert!(r.ok && !r.css_only, "{r:?}");
        assert_eq!(live.page(), "<div><p>v0</p>\n</div>");
    });
}

#[test]
fn a_burst_of_twenty_edits_inside_the_debounce_makes_one_build() {
    let t = site();
    with_session(t.path(), |live| {
        live.settle();
        let mut last = Instant::now();
        let mut widest = Duration::ZERO;
        for i in 1..=20 {
            write(
                live.root,
                "src/index.md",
                &format!("---\nlayout: base.liquid\n---\nv{i}\n"),
            );
            widest = widest.max(last.elapsed());
            last = Instant::now();
            std::thread::sleep(Duration::from_millis(10));
        }
        wait_for("the last edit to be served", || {
            live.page() == "<main><p>v20</p>\n</main>"
        });
        // Count every build the burst caused, including any still to come.
        let mut seen = Vec::new();
        while let Ok(r) = live.rebuilds.recv_timeout(QUIET) {
            seen.push(r.changed_paths);
        }
        let builds = seen.len();
        // Every save landed well inside the 150 ms window, so the burst is one build. Only
        // a machine so loaded that two saves were 100 ms apart may split it, and then
        // into no more than two.
        if widest < Duration::from_millis(100) {
            assert_eq!(builds, 1, "widest gap {widest:?}, changed: {seen:?}");
        } else {
            assert!(
                (1..=2).contains(&builds),
                "{builds} builds, widest gap {widest:?}, changed: {seen:?}"
            );
        }
        assert_eq!(live.page(), "<main><p>v20</p>\n</main>");
    });
}

#[test]
fn a_broken_template_keeps_the_last_good_build_until_it_is_fixed() {
    let t = site();
    with_session(t.path(), |live| {
        live.settle();
        write(
            live.root,
            "src/index.md",
            "---\nlayout: base.liquid\n---\nbroken {% if x %}\n",
        );
        let r = live.next_rebuild();
        assert!(!r.ok);
        assert!(
            r.problems
                .iter()
                .any(|p| p.path.as_deref() == Some(Path::new("src/index.md"))),
            "{:?}",
            r.problems
        );
        // The rest of the build is served; the refused page keeps its last file.
        assert!(r.served);
        assert_eq!(r.refused, [PathBuf::from("src/index.md")]);
        assert_eq!(r.kept, 1);
        assert_eq!(live.page(), "<main><p>v0</p>\n</main>");
        // Old builds are still removed.
        let staging = live.root.join("_site").join(STAGING_DIR);
        wait_for("the old build to go", || builds_in(&staging) == 1);

        write(
            live.root,
            "src/index.md",
            "---\nlayout: base.liquid\n---\nfixed\n",
        );
        let r = live.next_rebuild();
        assert!(r.ok, "{:?}", r.problems);
        assert_eq!(live.page(), "<main><p>fixed</p>\n</main>");
    });
}

#[test]
fn writes_into_the_output_folder_never_rebuild() {
    let t = site();
    with_session(t.path(), |live| {
        live.settle();
        write(live.root, "_site/stray.html", "x");
        write(live.root, "_site/index.html", "overwritten");
        write(live.root, &format!("_site/{STAGING_DIR}/x.html"), "x");
        live.no_rebuild();
        // The watcher is alive: a source edit still rebuilds.
        write(
            live.root,
            "src/index.md",
            "---\nlayout: base.liquid\n---\nv1\n",
        );
        assert!(live.next_rebuild().ok);
    });
}

#[test]
fn ignored_paths_never_rebuild() {
    let t = site();
    write(t.path(), ".gitignore", "src/drafts\n");
    write(t.path(), "src/drafts/a.md", "a");
    write(t.path(), "src/node_modules/x/a.md", "a");
    write(t.path(), "notes/todo.md", "a");
    with_session(t.path(), |live| {
        live.settle();
        write(live.root, "src/drafts/a.md", "b");
        write(live.root, "src/node_modules/x/a.md", "b");
        write(live.root, "notes/todo.md", "b");
        write(live.root, "random.txt", "b");
        live.no_rebuild();
        write(live.root, "src/new/page.md", "new");
        // A new folder counts as a change of its own; its files are then watched too.
        assert!(live.next_rebuild().ok);
        wait_for("the new page", || {
            get(live.server, "/new/page/").1 == "<p>new</p>\n"
        });
        write(live.root, "src/new/page.md", "newer");
        let r = live.next_rebuild();
        assert_eq!(r.changed_paths, vec![PathBuf::from("src/new/page.md")]);
        assert_eq!(get(live.server, "/new/page/").1, "<p>newer</p>\n");
    });
}

#[test]
fn stopping_moves_the_last_good_build_into_place_and_removes_staging() {
    let t = site();
    write(t.path(), "_site/kept.txt", "from before");
    let finished = with_session(t.path(), |live| {
        live.settle();
        write(
            live.root,
            "src/index.md",
            "---\nlayout: base.liquid\n---\nv1\n",
        );
        assert!(live.next_rebuild().ok);
        write(
            live.root,
            "src/index.md",
            "---\nlayout: base.liquid\n---\n{% if x %}\n",
        );
        assert!(!live.next_rebuild().ok);
    });
    assert!(finished.output_written, "{:?}", finished.problems);
    assert!(finished.problems.is_empty(), "{:?}", finished.problems);
    let out = t.path().join("_site");
    assert!(!out.join(STAGING_DIR).exists());
    assert_eq!(
        fs::read_to_string(out.join("index.html")).unwrap(),
        "<main><p>v1</p>\n</main>"
    );
    // `brrrty build` keeps files it didn't write; so does serve.
    assert_eq!(
        fs::read_to_string(out.join("kept.txt")).unwrap(),
        "from before"
    );
}

#[test]
fn a_session_leaves_the_same_output_as_a_build() {
    let served = site();
    with_session(served.path(), |_| {});
    let built = site();
    let opts = brrrty::options::build_options(
        built.path(),
        &Flags {
            input: Some(PathBuf::from("src")),
            ..Flags::default()
        },
    )
    .unwrap();
    assert!(!brrrty_core::build(&opts, &brrrty_markdown::MarkdownIt::new()).has_errors());
    let list = |root: &Path| {
        let mut files = Vec::new();
        let mut stack = vec![root.join("_site")];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(d).unwrap().flatten() {
                if e.path().is_dir() {
                    stack.push(e.path());
                } else {
                    let rel = e.path().strip_prefix(root).unwrap().to_path_buf();
                    files.push((rel, fs::read(e.path()).unwrap()));
                }
            }
        }
        files.sort();
        files
    };
    assert_eq!(list(served.path()), list(built.path()));
}

#[test]
fn a_failed_first_build_serves_nothing_until_fixed() {
    let t = site();
    write(
        t.path(),
        "src/index.md",
        "---\nlayout: base.liquid\n---\n{% if x %}\n",
    );
    with_session(t.path(), |live| {
        let (status, body) = get(live.server, "/");
        assert_eq!(status, 404);
        assert!(body.contains("Cannot GET /"), "{body}");
        live.settle();
        write(
            live.root,
            "src/index.md",
            "---\nlayout: base.liquid\n---\nok\n",
        );
        assert!(live.next_rebuild().ok);
        assert_eq!(live.page(), "<main><p>ok</p>\n</main>");
    });
    assert!(t.path().join("_site/index.html").is_file());
}

#[test]
fn a_session_that_never_built_leaves_no_output_folder() {
    let t = site();
    write(t.path(), "src/index.md", "{% if x %}\n");
    fs::remove_file(t.path().join("src/style.css")).unwrap();
    with_session(t.path(), |live| {
        assert_eq!(get(live.server, "/").0, 404);
    });
    assert!(!t.path().join("_site").exists());
    // One that was there already stays.
    fs::create_dir(t.path().join("_site")).unwrap();
    with_session(t.path(), |_| {});
    assert!(t.path().join("_site").is_dir());
}

#[test]
fn old_builds_are_removed_once_unused() {
    let t = site();
    with_session(t.path(), |live| {
        live.settle();
        for i in 1..=3 {
            write(
                live.root,
                "src/index.md",
                &format!("---\nlayout: base.liquid\n---\nv{i}\n"),
            );
            assert!(live.next_rebuild().ok);
        }
        let staging = live.root.join("_site").join(STAGING_DIR);
        wait_for("old builds to be removed", || builds_in(&staging) == 1);
    });
}

#[test]
fn edits_while_a_build_runs_make_exactly_one_more_build() {
    let t = site();
    with_session_holding(t.path(), Duration::from_secs(1), |live| {
        live.settle();
        write(
            live.root,
            "src/index.md",
            "---\nlayout: base.liquid\n---\nA\n",
        );
        let first = live.next_rebuild();
        assert!(first.ok);
        // The session is busy for a second: these all land before it reads them.
        write(live.root, "src/other.md", "B1");
        write(
            live.root,
            "src/index.md",
            "---\nlayout: base.liquid\n---\nA2\n",
        );
        write(live.root, "src/other.md", "B2");
        let second = live.next_rebuild();
        assert!(second.ok);
        let mut changed = second.changed_paths.clone();
        changed.sort();
        assert_eq!(
            changed,
            vec![PathBuf::from("src/index.md"), PathBuf::from("src/other.md")]
        );
        live.no_rebuild();
        assert_eq!(get(live.server, "/other/").1, "<p>B2</p>\n");
        assert_eq!(live.page(), "<main><p>A2</p>\n</main>");
    });
}

#[test]
fn a_page_named_like_staging_never_reaches_the_output() {
    let t = site();
    write(
        t.path(),
        "src/sneaky.md",
        "---\npermalink: /.brrrty-serve/x.html\n---\nsneaky\n",
    );
    let finished = with_session(t.path(), |live| {
        assert_eq!(live.page(), "<main><p>v0</p>\n</main>");
    });
    let out = t.path().join("_site");
    assert!(!out.join(STAGING_DIR).exists());
    assert_eq!(
        fs::read_to_string(out.join("index.html")).unwrap(),
        "<main><p>v0</p>\n</main>"
    );
    assert!(
        finished
            .problems
            .iter()
            .any(|p| p.message.contains(STAGING_DIR)),
        "{:?}",
        finished.problems
    );
}

/// How many builds the staging folder holds (it also holds the session's lock).
fn builds_in(staging: &Path) -> usize {
    fs::read_dir(staging)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("build-")
        })
        .count()
}

fn read(root: &Path, rel: &str) -> Option<String> {
    fs::read_to_string(root.join(rel)).ok()
}

fn page_md(body: &str) -> String {
    format!("---\nlayout: base.liquid\n---\n{body}\n")
}

#[test]
fn the_output_folder_is_current_after_every_good_build() {
    let t = site();
    write(t.path(), "src/about.md", "about");
    write(t.path(), "src/old.md", "old");
    with_session(t.path(), |live| {
        assert_eq!(
            read(live.root, "_site/index.html").as_deref(),
            Some("<main><p>v0</p>\n</main>")
        );
        live.settle();
        let about = live.root.join("_site/about/index.html");
        let before = fs::metadata(&about).unwrap().modified().unwrap();
        write(live.root, "src/index.md", &page_md("v1"));
        assert!(live.next_rebuild().ok);
        // Current before the hook runs, as 11ty writes before it reloads.
        assert_eq!(
            read(live.root, "_site/index.html").as_deref(),
            Some("<main><p>v1</p>\n</main>")
        );
        // Unchanged files are left alone.
        assert_eq!(fs::metadata(&about).unwrap().modified().unwrap(), before);
        // A failed build leaves the output folder as it was.
        write(live.root, "src/index.md", &page_md("{% if x %}"));
        assert!(!live.next_rebuild().ok);
        assert_eq!(
            read(live.root, "_site/index.html").as_deref(),
            Some("<main><p>v1</p>\n</main>")
        );
        // Like `brrrty build`, nothing is deleted from the output folder; the server, which
        // serves the build itself, no longer has the page.
        fs::remove_file(live.root.join("src/old.md")).unwrap();
        write(live.root, "src/index.md", &page_md("v2"));
        wait_for("the fix", || live.page() == "<main><p>v2</p>\n</main>");
        assert_eq!(get(live.server, "/old/").0, 404);
        assert_eq!(
            read(live.root, "_site/old/index.html").as_deref(),
            Some("<p>old</p>\n")
        );
    });
}

const CHILD_ENV: &str = "BRRRTY_SERVE_TEST_CHILD";

/// Not a test of its own: the body of the session that
/// `a_killed_session_leaves_the_last_good_build_in_the_output` starts in a child process.
#[test]
fn serve_child_process() {
    let Some(root) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let settings = Settings {
        flags: Flags {
            input: Some(PathBuf::from("src")),
            ..Flags::default()
        },
        quiet: true,
        ..Settings::default()
    };
    let session = Session::start(Path::new(&root), settings).unwrap();
    let server = Server::start(session.serving(), "localhost", 0).unwrap();
    session.run(&server);
}

#[test]
fn a_killed_session_leaves_the_last_good_build_in_the_output() {
    let t = site();
    let root = t.path();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["serve_child_process", "--exact", "--test-threads=1"])
        .env(CHILD_ENV, root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let index = |want: &str| read(root, "_site/index.html").as_deref() == Some(want);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_for("the first build", || index("<main><p>v0</p>\n</main>"));
        // Keep saving until the watcher, which starts with the session, has seen one.
        let mut n = 0;
        wait_for("an edit to reach the output folder", || {
            n += 1;
            write(root, "src/index.md", &page_md(&format!("v{n}")));
            std::thread::sleep(Duration::from_millis(500));
            index(&format!("<main><p>v{n}</p>\n</main>"))
        });
        write(root, "src/index.md", &page_md("{% if x %}"));
        std::thread::sleep(QUIET);
        n
    }));
    // SIGKILL on Unix: no cleanup runs.
    child.kill().unwrap();
    child.wait().unwrap();
    let n = match outcome {
        Ok(n) => n,
        Err(p) => std::panic::resume_unwind(p),
    };
    assert_eq!(
        read(root, "_site/index.html").as_deref(),
        Some(format!("<main><p>v{n}</p>\n</main>").as_str())
    );
    // The staging folder it left is removed by the next `brrrty build`.
    let opts = brrrty::options::build_options(
        root,
        &Flags {
            input: Some(PathBuf::from("src")),
            ..Flags::default()
        },
    )
    .unwrap();
    // The lock it held went with the process.
    assert!(root.join("_site").join(STAGING_DIR).join("lock").is_file());
    write(root, "src/index.md", &page_md("built"));
    assert!(!brrrty_core::build(&opts, &brrrty_markdown::MarkdownIt::new()).has_errors());
    assert!(!root.join("_site").join(STAGING_DIR).exists());
}

#[test]
fn a_folder_deleted_and_recreated_is_still_watched() {
    let t = site();
    write(t.path(), "src/posts/a.md", "one");
    with_session(t.path(), |live| {
        let post = || get(live.server, "/posts/a/").1;
        live.settle();
        // Slowly: the removal is seen before the folder comes back.
        fs::remove_dir_all(live.root.join("src/posts")).unwrap();
        wait_for("the removal", || get(live.server, "/posts/a/").0 == 404);
        write(live.root, "src/posts/a.md", "two");
        wait_for("the new folder", || post() == "<p>two</p>\n");
        live.settle();
        write(live.root, "src/posts/a.md", "three");
        assert!(live.next_rebuild().ok);
        assert_eq!(post(), "<p>three</p>\n");

        // Quickly: removed and back before the session looks.
        fs::remove_dir_all(live.root.join("src/posts")).unwrap();
        write(live.root, "src/posts/a.md", "four");
        wait_for("the new folder", || post() == "<p>four</p>\n");
        live.settle();
        write(live.root, "src/posts/a.md", "five");
        assert!(live.next_rebuild().ok);
        assert_eq!(post(), "<p>five</p>\n");
        // A folder made inside it afterwards is watched too.
        write(live.root, "src/posts/deep/b.md", "deep");
        wait_for("the deep page", || {
            get(live.server, "/posts/deep/b/").1 == "<p>deep</p>\n"
        });
        live.settle();
        write(live.root, "src/posts/deep/b.md", "deeper");
        assert!(live.next_rebuild().ok);
        assert_eq!(get(live.server, "/posts/deep/b/").1, "<p>deeper</p>\n");
    });
}

#[test]
fn a_folder_moved_into_the_input_is_watched() {
    let t = site();
    write(t.path(), "drafts/b.md", "draft");
    write(t.path(), "drafts/deep/c.md", "deep");
    with_session(t.path(), |live| {
        live.settle();
        fs::rename(live.root.join("drafts"), live.root.join("src/drafts")).unwrap();
        wait_for("the moved folder", || {
            get(live.server, "/drafts/b/").1 == "<p>draft</p>\n"
        });
        live.settle();
        write(live.root, "src/drafts/b.md", "edited");
        assert!(live.next_rebuild().ok);
        assert_eq!(get(live.server, "/drafts/b/").1, "<p>edited</p>\n");
        write(live.root, "src/drafts/deep/c.md", "deep edit");
        assert!(live.next_rebuild().ok);
        assert_eq!(get(live.server, "/drafts/deep/c/").1, "<p>deep edit</p>\n");
        // Moved out and back in under the same name.
        fs::rename(live.root.join("src/drafts"), live.root.join("away")).unwrap();
        wait_for("the move out", || get(live.server, "/drafts/b/").0 == 404);
        fs::rename(live.root.join("away"), live.root.join("src/drafts")).unwrap();
        wait_for("the move back", || get(live.server, "/drafts/b/").0 == 200);
        live.settle();
        write(live.root, "src/drafts/b.md", "again");
        assert!(live.next_rebuild().ok);
        assert_eq!(get(live.server, "/drafts/b/").1, "<p>again</p>\n");
    });
}

#[cfg(unix)]
#[test]
fn a_dangling_staging_symlink_is_replaced_at_start() {
    let t = site();
    fs::create_dir_all(t.path().join("_site")).unwrap();
    std::os::unix::fs::symlink(
        t.path().join("nowhere"),
        t.path().join("_site").join(STAGING_DIR),
    )
    .unwrap();
    with_session(t.path(), |live| {
        assert_eq!(live.page(), "<main><p>v0</p>\n</main>");
    });
    assert!(!t.path().join("nowhere").exists());
}

#[test]
fn a_new_page_that_never_built_is_a_404_and_the_rest_is_served() {
    let t = site();
    with_session(t.path(), |live| {
        live.settle();
        write(live.root, "src/new.md", "{% if %}\n");
        let r = live.next_rebuild();
        assert!(!r.ok && r.served, "{r:?}");
        assert_eq!(r.kept, 0);
        assert_eq!(get(live.server, "/new/").0, 404);
        write(
            live.root,
            "src/index.md",
            "---\nlayout: base.liquid\n---\nv1\n",
        );
        let r = live.next_rebuild();
        assert!(!r.ok && r.served, "{r:?}");
        assert_eq!(live.page(), "<main><p>v1</p>\n</main>");
    });
}

#[test]
fn kept_pages_never_reach_the_output_folder() {
    let t = site();
    write(t.path(), "src/about.md", "about v0");
    with_session(t.path(), |live| {
        live.settle();
        let about = live.root.join("_site/about/index.html");
        assert!(about.is_file());
        // `brrrty build` with a broken about.md writes no about page, so neither does the
        // mirror, though the server keeps the last one.
        fs::remove_file(&about).unwrap();
        write(live.root, "src/about.md", "{% if %}");
        let r = live.next_rebuild();
        assert_eq!(r.kept, 1, "{r:?}");
        assert_eq!(
            get(live.server, "/about/"),
            (200, "<p>about v0</p>\n".to_string())
        );
        assert!(!about.exists());
        // A refused page goes on being kept by the builds after it.
        write(
            live.root,
            "src/index.md",
            "---\nlayout: base.liquid\n---\nv1\n",
        );
        let r = live.next_rebuild();
        assert_eq!(r.kept, 1, "{r:?}");
        assert_eq!(get(live.server, "/about/").0, 200);
        write(live.root, "src/about.md", "about v1");
        assert!(live.next_rebuild().ok);
        assert_eq!(get(live.server, "/about/").1, "<p>about v1</p>\n");
        assert_eq!(fs::read_to_string(&about).unwrap(), "<p>about v1</p>\n");
    });
}

/// Pages refused by Nunjucks, and by the syntax highlighting scan for a language that changes
/// a grammar, are kept like any other refused page, and never reach the output folder.
#[test]
fn pages_refused_by_nunjucks_or_a_grammar_change_are_kept() {
    let t = site();
    write(
        t.path(),
        "brrrty.config.rhai",
        r#"config.add_plugin("syntaxhighlight");"#,
    );
    write(t.path(), "src/about.njk", "about {{ 1 + 1 }}");
    write(t.path(), "src/code.md", "```js\nlet a = 1;\n```\n");
    with_session(t.path(), |live| {
        live.settle();
        let code = get(live.server, "/code/").1;
        assert!(code.contains("language-js"), "{code}");
        assert_eq!(get(live.server, "/about/").1, "about 2");
        let about = live.root.join("_site/about/index.html");
        let code_out = live.root.join("_site/code/index.html");
        fs::remove_file(&about).unwrap();
        fs::remove_file(&code_out).unwrap();

        // `random` is refused in Nunjucks; a `regex` fence changes Prism's `javascript`
        // grammar, so code.md is refused too.
        write(live.root, "src/about.njk", "{{ [1, 2] | random }}");
        write(live.root, "src/regex.md", "```regex\n^a+$\n```\n");
        let r = loop {
            let r = live.next_rebuild();
            if r.refused.len() == 3 {
                break r;
            }
        };
        assert!(r.served && !r.ok, "{r:?}");
        let mut refused: Vec<String> = r.refused.iter().map(|p| p.display().to_string()).collect();
        refused.sort();
        assert_eq!(refused, ["src/about.njk", "src/code.md", "src/regex.md"]);
        assert_eq!(r.kept, 2, "{r:?}");
        assert_eq!(get(live.server, "/about/"), (200, "about 2".to_string()));
        assert_eq!(get(live.server, "/code/").1, code);
        // regex.md never built, so it has nothing to keep.
        assert_eq!(get(live.server, "/regex/").0, 404);
        assert!(!about.exists() && !code_out.exists());
        assert_eq!(get(live.server, "/").0, 200);
    });
}

#[test]
fn a_build_that_stops_before_writing_pages_keeps_the_last_one() {
    let t = site();
    let settings = Settings {
        flags: Flags {
            input: Some(PathBuf::from("src")),
            strict: true,
            ..Flags::default()
        },
        quiet: true,
        ..Settings::default()
    };
    write(t.path(), "_site/old.html", "old");
    write(t.path(), "src/index.md", "{% if %}");
    let session = Session::start(t.path(), settings).unwrap();
    // `--strict` stops at the first error, before any page is written.
    assert!(!session.first_ok() && !session.first_served());
    assert_eq!(session.serving(), session.output());
    let finished = session.close();
    assert!(finished.problems.is_empty(), "{:?}", finished.problems);
    assert!(!t.path().join("_site").join(STAGING_DIR).exists());
    assert!(t.path().join("_site/old.html").is_file());
}

#[test]
fn the_output_folder_follows_the_build_and_repairs_what_else_changed_there() {
    let t = site();
    write(t.path(), "src/about.md", "about");
    let settings = Settings {
        flags: Flags {
            input: Some(PathBuf::from("src")),
            passthrough: vec![PathBuf::from("src/style.css")],
            ..Flags::default()
        },
        quiet: true,
        ..Settings::default()
    };
    let mut session = Session::start(t.path(), settings).unwrap();
    let (tx, rx) = mpsc::channel();
    session.on_rebuild(Arc::new(move |r: &Rebuild| {
        let _ = tx.send(r.clone());
    }));
    let server = Server::start(session.serving(), "localhost", 0).unwrap();
    let stop = session.stop_handle();
    let root = t.path();
    std::thread::scope(|s| {
        let running = s.spawn(|| session.run(&server));
        let live = Live {
            root,
            server: &server,
            stop: stop.clone(),
            rebuilds: rx,
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            live.settle();
            let out = root.join("_site");
            assert_eq!(read(root, "_site/style.css").as_deref(), Some("body{}"));
            // Something else rewrites a page and a copy in the output folder, and removes one.
            write(root, "_site/about/index.html", "tampered");
            write(root, "_site/style.css", "x");
            fs::remove_file(out.join("index.html")).unwrap();
            // A rebuild that changes none of them still puts them right.
            write(root, "src/other.md", "other");
            assert!(live.next_rebuild().ok);
            assert_eq!(
                read(root, "_site/about/index.html").as_deref(),
                Some("<p>about</p>\n")
            );
            assert_eq!(read(root, "_site/style.css").as_deref(), Some("body{}"));
            assert_eq!(
                read(root, "_site/index.html").as_deref(),
                Some("<main><p>v0</p>\n</main>")
            );
            // A passthrough edit is copied.
            write(root, "src/style.css", "body{color:red}");
            assert!(live.next_rebuild().ok);
            assert_eq!(
                read(root, "_site/style.css").as_deref(),
                Some("body{color:red}")
            );
        }));
        live.stop.stop();
        running.join().unwrap();
        if let Err(p) = outcome {
            std::panic::resume_unwind(p);
        }
    });
}

#[cfg(unix)]
#[test]
fn a_symlinked_passthrough_folder_is_mirrored_on_every_edit() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("site");
    write(&root, "src/index.md", "home");
    write(t.path(), "shared/x.css", "v0");
    std::os::unix::fs::symlink("../shared", root.join("public")).unwrap();
    let flags = Flags {
        input: Some(PathBuf::from("src")),
        passthrough: vec![PathBuf::from("public")],
        ..Flags::default()
    };
    let finished = with_session_flags(&root, flags, Duration::ZERO, |live| {
        live.settle();
        assert_eq!(read(&root, "_site/public/x.css").as_deref(), Some("v0"));
        for v in ["v1", "v2"] {
            write(t.path(), "shared/x.css", v);
            assert!(live.next_rebuild().ok);
            assert_eq!(get(live.server, "/public/x.css").1, v);
            assert_eq!(read(&root, "_site/public/x.css").as_deref(), Some(v));
        }
    });
    assert!(finished.problems.is_empty(), "{:?}", finished.problems);
    assert_eq!(read(&root, "_site/public/x.css").as_deref(), Some("v2"));
}

#[test]
fn a_build_or_a_second_session_leaves_a_live_session_alone() {
    let t = site();
    let root = t.path();
    let flags = Flags {
        input: Some(PathBuf::from("src")),
        ..Flags::default()
    };
    let finished = with_session(root, |live| {
        live.settle();
        let opts = brrrty::options::build_options(root, &flags).unwrap();
        let report = brrrty_core::build(&opts, &brrrty_markdown::MarkdownIt::new());
        assert!(!report.has_errors(), "{:?}", report.problems);
        let note = report
            .problems
            .iter()
            .find(|p| p.code == brrrty_core::codes::SERVE_STAGING_IN_USE)
            .expect("a note that staging was left alone");
        assert_eq!(note.severity, brrrty_core::Severity::Info);
        assert!(note.message.contains(&std::process::id().to_string()));
        assert!(root.join("_site").join(STAGING_DIR).is_dir());
        assert_eq!(live.page(), "<main><p>v0</p>\n</main>");
        // A second session on the same output folder refuses to start.
        let settings = Settings {
            flags: flags.clone(),
            quiet: true,
            ..Settings::default()
        };
        match Session::start(root, settings) {
            Err(e @ brrrty::serve::StartError::InUse(..)) => {
                assert!(e.to_string().contains("another `brrrty serve`"), "{e}");
            }
            Err(e) => panic!("unexpected error: {e}"),
            Ok(_) => panic!("a second session started on the same output folder"),
        }
        // The first one still serves and rebuilds.
        assert_eq!(live.page(), "<main><p>v0</p>\n</main>");
        write(root, "src/index.md", &page_md("v1"));
        assert!(live.next_rebuild().ok);
        assert_eq!(live.page(), "<main><p>v1</p>\n</main>");
    });
    assert!(finished.problems.is_empty(), "{:?}", finished.problems);
    assert!(!root.join("_site").join(STAGING_DIR).exists());
}

#[test]
fn a_todo_marker_shows_why_until_it_is_deleted() {
    let t = site();
    write(
        t.path(),
        "brrrty.config.rhai",
        "// TODO(brrrty): port this.\n",
    );
    with_session(t.path(), |live| {
        let (status, body) = get(live.server, "/");
        assert_eq!(status, 404);
        assert!(body.contains("config-todo"), "{body}");
        assert!(body.contains("brrrty.config.rhai:1"), "{body}");
        // Nothing reaches the output folder.
        assert!(!live.root.join("_site/index.html").exists());
        live.settle();
        write(live.root, "brrrty.config.rhai", "// Ported by hand.\n");
        let r = live.next_rebuild();
        assert!(r.ok, "{:?}", r.problems);
        assert_eq!(live.page(), "<main><p>v0</p>\n</main>");
    });
}

fn has_node() -> bool {
    let ok = Command::new("node").arg("--version").output().is_ok();
    if !ok {
        assert!(std::env::var_os("CI").is_none(), "CI needs Node");
        eprintln!("skipped: no node on PATH");
    }
    ok
}

/// JavaScript data runs again in a fresh Node on every rebuild, and what it read or
/// loaded is watched: a file it read outside the input folder, a helper it required, a
/// new data file, and the lock file.
#[test]
fn javascript_data_reruns_when_what_it_read_changes() {
    if !has_node() {
        return;
    }
    let t = site();
    let root = t.path();
    write(
        root,
        "brrrty.config.rhai",
        "config.set_js_data(\"node\");\n",
    );
    write(root, "data/names.csv", "Ada\n");
    write(
        root,
        "lib/helper.js",
        "module.exports = { suffix: \"!\" };\n",
    );
    write(
        root,
        "src/_data/names.js",
        "const fs = require(\"fs\");\nconst path = require(\"path\");\nconst h = require(\"../../lib/helper.js\");\nmodule.exports = () => fs.readFileSync(path.join(__dirname, \"../../data/names.csv\"), \"utf8\").trim() + h.suffix;\n",
    );
    write(root, "package-lock.json", "{}\n");
    write(
        root,
        "src/index.md",
        "---\nlayout: base.liquid\n---\n{{ names }} {{ extra.n }}\n",
    );
    with_session(root, |live| {
        assert_eq!(live.page(), "<main><p>Ada!</p>\n</main>");
        live.settle();
        // Outside the input folder: only the data file's read says it matters.
        write(live.root, "data/names.csv", "Grace\n");
        let r = live.next_rebuild();
        assert!(r.ok, "{:?}", r.problems);
        assert_eq!(live.page(), "<main><p>Grace!</p>\n</main>");
        // A module it required, also outside the input folder.
        write(
            live.root,
            "lib/helper.js",
            "module.exports = { suffix: \"?\" };\n",
        );
        assert!(live.next_rebuild().ok);
        assert_eq!(live.page(), "<main><p>Grace?</p>\n</main>");
        // A new data file.
        write(
            live.root,
            "src/_data/extra.js",
            "module.exports = { n: 2 };\n",
        );
        assert!(live.next_rebuild().ok);
        assert_eq!(live.page(), "<main><p>Grace? 2</p>\n</main>");
        // The lock file: packages may have changed.
        write(live.root, "package-lock.json", "{ }\n");
        assert!(live.next_rebuild().ok);
    });
}

/// No Node outlives a serve session: each build's worker stops with the build.
#[test]
fn a_stopped_session_leaves_no_node_behind() {
    if !has_node() {
        return;
    }
    let t = site();
    let root = t.path();
    write(
        root,
        "brrrty.config.rhai",
        "config.set_js_data(\"node\");\n",
    );
    write(
        root,
        "src/_data/pid.js",
        "require(\"fs\").writeFileSync(require(\"path\").join(__dirname, \"../../pid\"), String(process.pid));\nmodule.exports = 1;\n",
    );
    write(
        root,
        "src/index.md",
        "---\nlayout: base.liquid\n---\n{{ pid }}\n",
    );
    with_session(root, |live| {
        assert_eq!(live.page(), "<main><p>1</p>\n</main>");
    });
    let pid = fs::read_to_string(root.join("pid")).unwrap();
    let alive = Command::new("kill")
        .args(["-0", &pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!alive, "Node {pid} outlived the session");
}
