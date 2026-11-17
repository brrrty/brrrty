//! What to undo when the process ends early: temporary folders to remove and a child
//! process group to stop. `brrrty doctor --verify` registers both; Ctrl-C, SIGTERM and the
//! panic handler call [`run`], which ends the process before any destructor runs.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

static FOLDERS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CHILD: Mutex<Option<u32>> = Mutex::new(None);
/// The signal that is stopping the process, or 0.
static STOPPING: AtomicI32 = AtomicI32::new(0);

/// Stop because of `signal`: mark the process as stopping before anything else, so a thread
/// that wakes when the child is killed knows not to go on, then clean up and exit
/// `128 + signal`. Called on the signal thread.
pub fn stop(signal: i32) -> ! {
    STOPPING.store(signal, Ordering::SeqCst);
    run();
    crate::say_err!("Stopped; the temporary folders are removed.");
    std::process::exit(128 + signal);
}

/// When a signal is stopping the process, wait for [`stop`] to end it instead of going on.
/// Call after anything a signal can cut short, such as waiting for the child.
pub fn halt_if_stopping() {
    while STOPPING.load(Ordering::SeqCst) != 0 {
        std::thread::park();
    }
}

/// A temporary folder that is removed when it is dropped, and by [`run`] when the process
/// ends before that.
pub struct TempFolder(Option<tempfile::TempDir>);

impl TempFolder {
    /// A new folder in the system temp folder whose name starts with `prefix`.
    pub fn new(prefix: &str) -> std::io::Result<TempFolder> {
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

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The process group of a running child, led by `pid`, which [`run`] stops; `None` once it
/// has ended.
pub fn set_child(pid: Option<u32>) {
    *lock(&CHILD) = pid;
}

/// Stop the child's process group and remove the temporary folders. Safe to call from any
/// thread, more than once.
pub fn run() {
    #[cfg(unix)]
    if let Some(pid) = lock(&CHILD).take()
        && let Ok(pid) = i32::try_from(pid)
    {
        // SAFETY: killpg only sends a signal; the group is the child's own.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
    for path in lock(&FOLDERS).drain(..) {
        let _ = std::fs::remove_dir_all(path);
    }
}
