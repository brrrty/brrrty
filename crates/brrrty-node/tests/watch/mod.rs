//! A watchdog for tests that start Node: a test that runs far longer than
//! any should is named on stderr, with the processes still alive, and the
//! test binary stops. Without it a stuck test is a silent hang until
//! whatever runs the tests gives up, with nothing to say which test.
//!
//! A test is watched from its first call to [`started`] until its thread
//! ends.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Mutex, Once};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// Longer than the slowest test by a wide margin (the slowest builds a
/// site 200 times).
const LIMIT: Duration = Duration::from_secs(240);

static RUNNING: Mutex<Option<HashMap<ThreadId, (String, Instant)>>> = Mutex::new(None);
static WATCHING: Once = Once::new();

struct Running(ThreadId);

impl Drop for Running {
    fn drop(&mut self) {
        if let Ok(mut running) = RUNNING.lock()
            && let Some(running) = running.as_mut()
        {
            running.remove(&self.0);
        }
    }
}

thread_local! {
    static THIS: std::cell::RefCell<Option<Running>> = const { std::cell::RefCell::new(None) };
}

/// The test on this thread is running from now.
pub fn started() {
    let me = std::thread::current();
    let name = me.name().unwrap_or("a test").to_string();
    RUNNING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .entry(me.id())
        .or_insert((name, Instant::now()));
    THIS.with(|t| {
        t.borrow_mut().get_or_insert(Running(me.id()));
    });
    WATCHING.call_once(|| {
        std::thread::spawn(|| {
            let mut rounds = 0u32;
            loop {
                std::thread::sleep(Duration::from_secs(5));
                rounds += 1;
                if rounds.is_multiple_of(6) {
                    left_behind();
                }
                let stuck: Vec<(String, Duration)> = RUNNING
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .flatten()
                    .map(|(_, (name, since))| (name.clone(), since.elapsed()))
                    .filter(|(_, d)| *d > LIMIT)
                    .collect();
                if stuck.is_empty() {
                    continue;
                }
                // Straight to stderr: the test harness keeps what a test
                // prints, and this process is about to stop.
                let mut err = std::io::stderr();
                for (name, d) in &stuck {
                    let _ = writeln!(
                        err,
                        "\nWATCHDOG: test `{name}` has run for {} s",
                        d.as_secs()
                    );
                }
                let ps = std::process::Command::new("ps")
                    .args(["-eo", "pid,ppid,pgid,stat,etime,args"])
                    .output();
                if let Ok(ps) = ps {
                    let _ = writeln!(
                        err,
                        "WATCHDOG: this process is {}; processes:",
                        std::process::id()
                    );
                    for line in String::from_utf8_lossy(&ps.stdout).lines() {
                        if line.contains("node") || line.contains("brrrty") || line.contains("PID")
                        {
                            let short: String = line.chars().take(170).collect();
                            let _ = writeln!(err, "  {short}");
                        }
                    }
                }
                let _ = err.flush();
                std::process::abort();
            }
        });
    });
}

/// On CI, a worker Node that has been alive for five minutes was left
/// behind by some test (no test runs that long): it holds the pipes the
/// tests' output goes to, so whatever waits for them to close waits for
/// good, long after every test has passed. Say which, and stop.
fn left_behind() {
    if std::env::var_os("CI").is_none() {
        return;
    }
    let Ok(ps) = std::process::Command::new("ps")
        .args(["-eo", "pid,ppid,etimes,args"])
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&ps.stdout).into_owned();
    let old: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("--input-type=module"))
        .filter(|l| {
            l.split_whitespace()
                .nth(2)
                .and_then(|t| t.parse::<u64>().ok())
                .is_some_and(|t| t > 300)
        })
        .collect();
    if old.is_empty() {
        return;
    }
    let mut err = std::io::stderr();
    let running: Vec<String> = RUNNING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .flatten()
        .map(|(_, (name, _))| name.clone())
        .collect();
    let _ = writeln!(
        err,
        "\nWATCHDOG: a worker Node was left behind (this process is {}, running {running:?}):",
        std::process::id()
    );
    for line in old {
        let short: String = line.chars().take(120).collect();
        let _ = writeln!(err, "  {short}");
        if let Some(pid) = line.split_whitespace().next() {
            for what in ["cwd", "fd/0", "fd/1", "fd/2"] {
                let link = std::fs::read_link(format!("/proc/{pid}/{what}"));
                let _ = writeln!(err, "    {what}: {link:?}");
            }
            let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
            for l in status.lines().filter(|l| {
                l.starts_with("State") || l.starts_with("PPid") || l.starts_with("Threads")
            }) {
                let _ = writeln!(err, "    {l}");
            }
        }
    }
    for line in text
        .lines()
        .filter(|l| l.contains("brrrty") || l.contains("PID"))
    {
        let short: String = line.chars().take(150).collect();
        let _ = writeln!(err, "  {short}");
    }
    let _ = err.flush();
    std::process::abort();
}
