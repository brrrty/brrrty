//! Stopping `brrrty doctor --verify` on Ctrl-C or SIGTERM, and what to undo when it ends.
//! Every stop also kills the JavaScript data worker (see [`brrrty_node::stop_all`]).
//!
//! The work is split so no order of events can leak anything:
//!
//! - The signal thread only marks the process as stopping and kills the child's process
//!   group, if one is running, both under one lock. It never prints, removes files or exits.
//! - A child is spawned under the same lock, and only when no stop is in progress, so a
//!   signal either finds the child registered and kills it, or comes first and the child is
//!   never started.
//! - The main thread owns everything else. It asks [`stopping`] between its steps and, when
//!   a signal came, drops its [`TempFolder`]s (which removes them), says so and exits.
//!
//! The panic handler calls [`run`], which kills the child and removes the folders, since a
//! panic ends the process before any destructor runs.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Mutex;

/// Whether a stop is in progress, and the child process group that is running.
#[derive(Default)]
struct State {
    /// The first signal received, or 0.
    stopping: i32,
    child: Option<u32>,
}

static STATE: Mutex<State> = Mutex::new(State {
    stopping: 0,
    child: None,
});
static FOLDERS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A temporary folder that is removed when it is dropped, and by [`run`] after a panic.
pub struct TempFolder(Option<tempfile::TempDir>);

impl TempFolder {
    /// A new folder in the system temp folder whose name starts with `prefix`.
    pub fn new(prefix: &str) -> io::Result<TempFolder> {
        let dir = tempfile::Builder::new().prefix(prefix).tempdir()?;
        lock(&FOLDERS).push(dir.path().to_path_buf());
        Ok(TempFolder(Some(dir)))
    }

    pub fn path(&self) -> &Path {
        self.0.as_ref().map_or(Path::new(""), |d| d.path())
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            let path = dir.path().to_path_buf();
            drop(dir);
            lock(&FOLDERS).retain(|p| *p != path);
        }
    }
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    if let Ok(pid) = i32::try_from(pid) {
        // SAFETY: killpg only sends a signal; the group is the child's own.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

/// What the signal thread does on Ctrl-C or SIGTERM: mark the process as stopping and kill
/// the running child's process group.
pub fn signal(sig: i32) {
    brrrty_node::stop_all();
    let mut s = lock(&STATE);
    if s.stopping == 0 {
        s.stopping = sig;
    }
    if let Some(pid) = s.child {
        kill_group(pid);
    }
}

/// The signal stopping the process, if one came.
pub fn stopping() -> Option<i32> {
    match lock(&STATE).stopping {
        0 => None,
        sig => Some(sig),
    }
}

/// Catch Ctrl-C and SIGTERM for the rest of the run, calling [`signal`] for each. Does
/// nothing outside Unix.
pub fn install_signals() -> io::Result<()> {
    #[cfg(unix)]
    {
        use signal_hook::consts::{SIGINT, SIGTERM};
        let mut signals = signal_hook::iterator::Signals::new([SIGINT, SIGTERM])?;
        std::thread::Builder::new()
            .name("brrrty-signals".into())
            .spawn(move || {
                for sig in signals.forever() {
                    signal(sig);
                }
            })?;
    }
    Ok(())
}

/// For a build: on Ctrl-C or SIGTERM, stop the JavaScript data worker, then end as the
/// signal would. The worker leads a process group of its own, which a terminal's Ctrl-C
/// doesn't reach. Does nothing outside Unix.
pub fn stop_workers_on_signals() -> io::Result<()> {
    #[cfg(unix)]
    {
        use signal_hook::consts::{SIGINT, SIGTERM};
        let mut signals = signal_hook::iterator::Signals::new([SIGINT, SIGTERM])?;
        std::thread::Builder::new()
            .name("brrrty-signals".into())
            .spawn(move || {
                if let Some(sig) = signals.forever().next() {
                    brrrty_node::stop_all();
                    let _ = signal_hook::low_level::emulate_default_handler(sig);
                    std::process::exit(128 + sig);
                }
            })?;
    }
    Ok(())
}

/// Start `cmd` as the child a signal kills, unless a stop is in progress: then `Ok(None)`,
/// and nothing is started. Call [`child_done`] once it has ended.
pub fn spawn(cmd: &mut Command) -> io::Result<Option<Child>> {
    let mut s = lock(&STATE);
    if s.stopping != 0 {
        return Ok(None);
    }
    let child = cmd.spawn()?;
    s.child = Some(child.id());
    Ok(Some(child))
}

/// The child started by [`spawn`] has ended.
pub fn child_done() {
    lock(&STATE).child = None;
}

/// Kill the child and remove the temporary folders: for the panic handler, which ends the
/// process before destructors run.
pub fn run() {
    brrrty_node::stop_all();
    if let Some(pid) = lock(&STATE).child.take() {
        kill_group(pid);
    }
    for path in lock(&FOLDERS).drain(..) {
        let _ = std::fs::remove_dir_all(path);
    }
}
