//! `brrrty serve`'s watch loop: rebuild on change, swap, keep the last good build, clean up.
//!
//! Timing: every wait polls toward a condition with a generous deadline, and "nothing
//! happened" checks wait well past the debounce and the watcher's own latency.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
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
    let settings = Settings {
        flags: Flags {
            input: Some(PathBuf::from("src")),
            ..Flags::default()
        },
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
fn a_burst_of_twenty_edits_makes_at_most_two_builds_and_serves_the_last() {
    let t = site();
    with_session(t.path(), |live| {
        live.settle();
        for i in 1..=20 {
            write(
                live.root,
                "src/index.md",
                &format!("---\nlayout: base.liquid\n---\nv{i}\n"),
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        wait_for("the last edit to be served", || {
            live.page() == "<main><p>v20</p>\n</main>"
        });
        // Count every build the burst caused, including any still to come.
        let mut builds = 0;
        while live.rebuilds.recv_timeout(QUIET).is_ok() {
            builds += 1;
        }
        assert!((1..=2).contains(&builds), "{builds} builds");
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
        assert_eq!(live.page(), "<main><p>v0</p>\n</main>");
        // The failed build left nothing behind in staging.
        let staging = live.root.join("_site").join(STAGING_DIR);
        assert_eq!(fs::read_dir(&staging).unwrap().count(), 1);

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
        wait_for("old builds to be removed", || {
            fs::read_dir(&staging).unwrap().count() == 1
        });
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
        finished.problems.iter().any(|p| p.contains(STAGING_DIR)),
        "{:?}",
        finished.problems
    );
}
