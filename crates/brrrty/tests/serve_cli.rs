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
        Serve::start_with_env(root, args, &[])
    }

    fn start_with_env(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Serve {
        let mut child = Command::new(env!("CARGO_BIN_EXE_brrrty"))
            .arg("serve")
            .args(args)
            .envs(env.iter().copied())
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

/// A free port outside the ephemeral range, where the ports other tests' servers and
/// clients get from the system land, so nothing takes it before `brrrty serve` binds it.
fn free_port() -> u16 {
    let seed = std::process::id() as u16 % 4000;
    (0..4000u16)
        .map(|i| 33000 + (seed + i) % 4000)
        .find(|&p| TcpListener::bind(("127.0.0.1", p)).is_ok())
        .expect("a free port")
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
fn started_by_the_npm_launcher_it_stops_cleanly_when_the_launcher_is_gone() {
    // The launcher names itself in BRRRTY_LAUNCHER_PID. Naming a process that
    // is not the parent stands in for a launcher killed with SIGKILL.
    let dir = site(&[("src/index.md", "# Hi\n")]);
    let serve = Serve::start_with_env(
        dir.path(),
        &["--input", "src", "--port", "0"],
        &[("BRRRTY_LAUNCHER_PID", "999999999")],
    );
    // It may stop before it serves; the stop is the clean one either way.
    let (status, _, err) = serve.wait();
    assert_eq!(status.signal(), Some(libc::SIGTERM), "{status:?} {err}");
    assert!(!dir.path().join("_site/.brrrty-serve").exists());
}

#[test]
fn without_the_launcher_variable_it_keeps_serving() {
    let dir = site(&[("src/index.md", "# Hi\n")]);
    let mut serve = Serve::start(dir.path(), &["--input", "src", "--port", "0"]);
    let port = serve.port();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(get_ok(port));
    serve.signal(libc::SIGINT);
    let (status, _, err) = serve.wait();
    assert_eq!(status.code(), Some(0), "{status:?} {err}");
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
fn quiet_prints_only_the_server_line_and_problems() {
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
    assert_eq!(out, vec![format!("Server at http://localhost:{port}/")]);
    assert!(err.is_empty(), "{err}");
}

fn get_ok(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).is_ok() && get(port, "/").starts_with("HTTP/1.1 200")
}

#[test]
fn a_first_build_with_a_broken_page_serves_the_rest() {
    let dir = site(&[
        ("src/index.liquid", "{% if %}\n"),
        ("src/about.md", "# About\n"),
    ]);
    let mut serve = Serve::start(dir.path(), &["--input", "src", "--port", "0"]);
    let port = serve.port();
    // The page that never built has nothing to serve; the rest is up.
    assert!(get(port, "/").starts_with("HTTP/1.1 404"));
    assert!(get(port, "/about/").contains("<h1>About</h1>"));
    std::fs::write(dir.path().join("src/index.liquid"), "fixed\n").unwrap();
    let until = Instant::now() + WAIT;
    while !get(port, "/").contains("fixed") {
        assert!(Instant::now() < until, "the fix was never served");
        std::thread::sleep(Duration::from_millis(50));
    }
    serve.signal(libc::SIGINT);
    let (status, out, err) = serve.wait();
    assert_eq!(status.code(), Some(0), "{err}");
    assert!(err.contains("src/index.liquid"), "{err}");
    assert!(
        !out.iter().any(|l| l.starts_with("The first build")),
        "{out:?}"
    );
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
    // As with 11ty, a build that never wrote leaves no output folder.
    assert!(!dir.path().join("_site").exists());
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
