//! `brrrty serve` as a process: its output, its exit codes, and stopping on a signal.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(30);

fn site(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    dir
}

/// A running `brrrty serve`, killed if a test fails before it ends.
struct Serve {
    child: Child,
    lines: Receiver<String>,
    seen: Vec<String>,
    stderr: Receiver<String>,
}

impl Serve {
    fn start(root: &Path, args: &[&str]) -> Serve {
        let mut child = Command::new(env!("CARGO_BIN_EXE_brrrty"))
            .arg("serve")
            .args(args)
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, lines) = mpsc::channel();
        let out = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        let (etx, stderr) = mpsc::channel();
        let mut err = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            let _ = etx.send(s);
        });
        Serve {
            child,
            lines,
            seen: Vec::new(),
            stderr,
        }
    }

    /// The next stdout line that starts with `prefix`.
    fn line_starting(&mut self, prefix: &str) -> String {
        let until = Instant::now() + WAIT;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(l) => {
                    self.seen.push(l.clone());
                    if l.starts_with(prefix) {
                        return l;
                    }
                }
                Err(_) => panic!("no line starting {prefix:?}; saw {:?}", self.seen),
            }
        }
    }

    /// The port from the `Server at` line.
    fn port(&mut self) -> u16 {
        let line = self.line_starting("Server at ");
        let rest = line.trim_start_matches("Server at http://localhost:");
        rest.split('/').next().unwrap().parse().unwrap()
    }

    fn signal(&self, sig: i32) {
        // SAFETY: kill(2) on our own child's pid.
        let r = unsafe { libc::kill(self.child.id() as libc::pid_t, sig) };
        assert_eq!(r, 0);
    }

    /// Wait for it to end; gives the status, the rest of stdout and all of stderr.
    fn wait(mut self) -> (ExitStatus, Vec<String>, String) {
        let until = Instant::now() + WAIT;
        let status = loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                break s;
            }
            assert!(Instant::now() < until, "brrrty serve did not stop");
            std::thread::sleep(Duration::from_millis(20));
        };
        let mut seen = std::mem::take(&mut self.seen);
        while let Ok(l) = self.lines.recv_timeout(Duration::from_secs(5)) {
            seen.push(l);
        }
        let err = self.stderr.recv_timeout(WAIT).unwrap_or_default();
        (status, seen, err)
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn get(port: u16, path: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn assert_released(port: u16) {
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_err(),
        "port {port} still answers"
    );
    TcpListener::bind(("127.0.0.1", port)).expect("the port is free again");
}

#[test]
fn ctrl_c_stops_cleanly_and_exits_0() {
    let dir = site(&[("src/index.md", "# Hi\n")]);
    let mut serve = Serve::start(dir.path(), &["--input", "src", "--port", "0"]);
    serve.line_starting("Wrote 1 files, copied 0 files in ");
    serve.line_starting("Watching…");
    let port = serve.port();
    assert!(get(port, "/").contains("<h1>Hi</h1>"));
    assert!(dir.path().join("_site/.brrrty-serve").is_dir());

    std::fs::write(dir.path().join("src/index.md"), "# Edited\n").unwrap();
    assert_eq!(
        serve.line_starting("File changed: "),
        "File changed: ./src/index.md"
    );
    serve.line_starting("Watching…");

    serve.signal(libc::SIGINT);
    let (status, _, err) = serve.wait();
    assert_eq!(status.code(), Some(0), "{err}");
    assert!(err.is_empty(), "{err}");
    assert_released(port);
    assert!(!dir.path().join("_site/.brrrty-serve").exists());
    let html = std::fs::read_to_string(dir.path().join("_site/index.html")).unwrap();
    assert_eq!(html, "<h1>Edited</h1>\n");
}

#[test]
fn sigterm_stops_cleanly_then_ends_by_the_signal() {
    let dir = site(&[("src/index.md", "# Hi\n")]);
    let mut serve = Serve::start(dir.path(), &["--input", "src", "--port", "0"]);
    let port = serve.port();
    serve.signal(libc::SIGTERM);
    let (status, _, err) = serve.wait();
    // As `eleventy --serve` ends on SIGTERM, but after the clean stop.
    assert_eq!(status.signal(), Some(libc::SIGTERM), "{status:?} {err}");
    assert_released(port);
    assert!(!dir.path().join("_site/.brrrty-serve").exists());
    assert!(dir.path().join("_site/index.html").is_file());
}

#[test]
fn a_second_ctrl_c_ends_it_at_once() {
    let dir = site(&[("src/index.md", "# Hi\n")]);
    let mut serve = Serve::start(dir.path(), &["--input", "src", "--port", "0"]);
    serve.port();
    serve.signal(libc::SIGINT);
    serve.signal(libc::SIGINT);
    let (status, _, err) = serve.wait();
    // Either the first stop already finished, or the second ended it by the signal.
    assert!(
        status.code() == Some(0) || status.signal() == Some(libc::SIGINT),
        "{status:?} {err}"
    );
}

#[test]
fn quiet_prints_nothing_but_problems() {
    let dir = site(&[("src/index.md", "# Hi\n")]);
    let port = free_port();
    let p = port.to_string();
    let serve = Serve::start(dir.path(), &["--input", "src", "--port", &p, "--quiet"]);
    let until = Instant::now() + WAIT;
    while !get_ok(port) {
        assert!(Instant::now() < until, "never served");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::fs::write(dir.path().join("src/index.md"), "# Edited\n").unwrap();
    let until = Instant::now() + WAIT;
    while !get(port, "/").contains("Edited") {
        assert!(Instant::now() < until, "never rebuilt");
        std::thread::sleep(Duration::from_millis(50));
    }
    serve.signal(libc::SIGINT);
    let (status, out, err) = serve.wait();
    assert_eq!(status.code(), Some(0), "{err}");
    assert!(out.is_empty(), "{out:?}");
    assert!(err.is_empty(), "{err}");
}

fn get_ok(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).is_ok() && get(port, "/").starts_with("HTTP/1.1 200")
}

#[test]
fn a_failed_first_build_keeps_serving_and_says_so() {
    let dir = site(&[("src/index.liquid", "{% if %}\n")]);
    let mut serve = Serve::start(dir.path(), &["--input", "src", "--port", "0"]);
    let port = serve.port();
    serve.line_starting("The first build failed: serving what ");
    assert!(get(port, "/").starts_with("HTTP/1.1 404"));
    std::fs::write(dir.path().join("src/index.liquid"), "fixed\n").unwrap();
    serve.line_starting("Wrote 1 files");
    let until = Instant::now() + WAIT;
    while !get(port, "/").contains("fixed") {
        assert!(Instant::now() < until, "the fix was never served");
        std::thread::sleep(Duration::from_millis(50));
    }
    serve.signal(libc::SIGINT);
    let (status, _, err) = serve.wait();
    assert_eq!(status.code(), Some(0), "{err}");
    assert!(err.contains("src/index.liquid"), "{err}");
    assert!(!dir.path().join("_site/.brrrty-serve").exists());
}

#[test]
fn strict_exits_1_when_the_first_build_fails() {
    let dir = site(&[("src/index.liquid", "{% if %}\n")]);
    let serve = Serve::start(dir.path(), &["--input", "src", "--port", "0", "--strict"]);
    let (status, out, err) = serve.wait();
    assert_eq!(status.code(), Some(1), "{out:?} {err}");
    assert!(!out.iter().any(|l| l.starts_with("Server at")), "{out:?}");
    assert!(
        err.contains("The first build failed, so Brrrty stopped (--strict)."),
        "{err}"
    );
    assert!(!dir.path().join("_site/.brrrty-serve").exists());
}

#[test]
fn a_busy_port_moves_to_the_next_and_says_so() {
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let dir = site(&[("src/index.md", "# Hi\n")]);
    let p = port.to_string();
    let mut serve = Serve::start(dir.path(), &["--input", "src", "--port", &p]);
    let line = serve.line_starting("Server at ");
    assert!(
        line.ends_with(&format!("/ (port {port} was in use)")),
        "{line}"
    );
    serve.signal(libc::SIGINT);
    let (status, _, err) = serve.wait();
    assert_eq!(status.code(), Some(0), "{err}");
}

#[test]
fn a_config_problem_exits_1_and_leaves_nothing() {
    let dir = site(&[("src/index.md", "# Hi\n")]);
    let serve = Serve::start(
        dir.path(),
        &["--input", "src", "--output", "src", "--port", "0"],
    );
    let (status, _, err) = serve.wait();
    assert_eq!(status.code(), Some(1), "{err}");
    assert!(!dir.path().join("src/.brrrty-serve").exists());
}

#[test]
fn bad_flags_are_a_usage_error() {
    let dir = site(&[]);
    let serve = Serve::start(dir.path(), &["--port", "nope"]);
    let (status, _, _) = serve.wait();
    assert_eq!(status.code(), Some(2));
}
