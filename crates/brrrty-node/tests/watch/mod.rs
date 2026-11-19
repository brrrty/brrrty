//! A watchdog for tests that start a process: a test that runs far longer
//! than any should is named on stderr, with the processes still alive,
//! and the test binary stops. Without it a stuck test is a silent hang
//! until whatever runs the tests gives up, with nothing to say which
//! test.
//!
//! A test is watched from its first call to [`started`] until its thread
//! ends.
//!
//! Once it is running, the watchdog shares nothing with the code under
//! test: no `Mutex`, no allocation, no read of the environment, and it
//! writes with a bare `write(2)` rather than through Rust's stderr and
//! its lock. Before it writes it
//! sets an alarm, so if even that write can't get through (nothing is
//! reading the tests' output any more), the alarm ends the process
//! instead of the hang going on.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Longer than the slowest test by a wide margin, on a loaded runner
/// with four cores too (the slowest builds a site 200 times: about 35 s
/// on a busy ten-core machine), and well inside the CI job's 45 minutes.
const LIMIT: Duration = Duration::from_secs(600);

/// One per test that registered: when it started, in milliseconds since
/// the first did (0 once its thread has ended), and its name.
const SLOTS: usize = 512;
static STARTED: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
static NAMES: [OnceLock<String>; SLOTS] = [const { OnceLock::new() }; SLOTS];
static NEXT: AtomicUsize = AtomicUsize::new(0);
static EPOCH: OnceLock<Instant> = OnceLock::new();
static WATCHING: OnceLock<()> = OnceLock::new();

struct Running(usize);

impl Drop for Running {
    fn drop(&mut self) {
        STARTED[self.0].store(0, Ordering::SeqCst);
    }
}

thread_local! {
    static THIS: std::cell::RefCell<Option<Running>> = const { std::cell::RefCell::new(None) };
}

fn now() -> u64 {
    // Never 0: that means "ended".
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// The test on this thread is running from now.
pub fn started() {
    THIS.with(|t| {
        let mut t = t.borrow_mut();
        if t.is_some() {
            return;
        }
        let slot = NEXT.fetch_add(1, Ordering::SeqCst);
        if slot >= SLOTS {
            return;
        }
        let name = std::thread::current()
            .name()
            .unwrap_or("a test")
            .to_string();
        let _ = NAMES[slot].set(name);
        STARTED[slot].store(now(), Ordering::SeqCst);
        *t = Some(Running(slot));
    });
    WATCHING.get_or_init(|| {
        // Everything it will say is put together in this, which never
        // grows.
        let mut text = String::with_capacity(16 * 1024);
        // `BRRRTY_TEST_LIMIT_SECS` shortens the limit, to see the watchdog
        // work. Read once, here: the environment has a lock of its own.
        let limit = std::env::var("BRRRTY_TEST_LIMIT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map_or(LIMIT.as_millis() as u64, |s| s * 1000);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let at = now();
                let mut stuck = false;
                for slot in 0..NEXT.load(Ordering::SeqCst).min(SLOTS) {
                    let since = STARTED[slot].load(Ordering::SeqCst);
                    if since != 0 && at.saturating_sub(since) > limit {
                        stuck = true;
                        if let Some(name) = NAMES[slot].get()
                            && text.len() + name.len() + 80 < text.capacity()
                        {
                            use std::fmt::Write;
                            let _ = writeln!(
                                text,
                                "\nWATCHDOG: test `{name}` has run for {} s",
                                (at - since) / 1000
                            );
                        }
                    }
                }
                if stuck {
                    say_and_stop(&text);
                }
            }
        });
    });
}

#[cfg(unix)]
fn say_and_stop(text: &str) -> ! {
    // SAFETY: plain system calls on this process's own descriptor, with a
    // buffer that outlives them.
    unsafe {
        // Whatever happens below, the process ends within a minute.
        libc::alarm(60);
        libc::write(2, text.as_ptr().cast(), text.len());
    }
    // What every thread of this process is doing: its name, its state,
    // and the system call it is in with its arguments (the first is
    // usually a descriptor), then what each descriptor is.
    let mut out = String::from("WATCHDOG: threads (name, state, syscall and arguments):\n");
    if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
        for task in tasks.flatten() {
            let dir = task.path();
            let read = |what: &str| {
                std::fs::read_to_string(dir.join(what))
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            };
            let stat = read("stat");
            let state = stat
                .rsplit(')')
                .next()
                .and_then(|rest| rest.split_whitespace().next())
                .unwrap_or("?")
                .to_string();
            out.push_str(&format!(
                "  {} {:<16} {state} {}\n",
                task.file_name().to_string_lossy(),
                read("comm"),
                read("syscall")
            ));
        }
    }
    out.push_str("WATCHDOG: descriptors:\n");
    if let Ok(fds) = std::fs::read_dir("/proc/self/fd") {
        for fd in fds.flatten().take(400) {
            if let Ok(to) = std::fs::read_link(fd.path()) {
                out.push_str(&format!(
                    "  {} -> {}\n",
                    fd.file_name().to_string_lossy(),
                    to.display()
                ));
            }
        }
    }
    // SAFETY: as above.
    unsafe {
        libc::write(2, out.as_ptr().cast(), out.len());
    }
    // The processes, which may take locks and memory: last, and under the
    // alarm.
    if let Ok(ps) = std::process::Command::new("ps")
        .args(["-eo", "pid,ppid,pgid,stat,etime,args"])
        .output()
    {
        let mut out = format!(
            "WATCHDOG: this process is {}; processes:\n",
            std::process::id()
        );
        for line in String::from_utf8_lossy(&ps.stdout).lines() {
            if line.contains("node") || line.contains("brrrty") || line.contains("PID") {
                out.extend(line.chars().take(170));
                out.push('\n');
                if let Some(pid) = line.split_whitespace().next()
                    && let Ok(cwd) = std::fs::read_link(format!("/proc/{pid}/cwd"))
                {
                    out.push_str(&format!("    in {}\n", cwd.display()));
                }
            }
        }
        // SAFETY: as above.
        unsafe {
            libc::write(2, out.as_ptr().cast(), out.len());
        }
    }
    std::process::abort();
}

#[cfg(not(unix))]
fn say_and_stop(text: &str) -> ! {
    eprintln!("{text}");
    std::process::abort();
}
