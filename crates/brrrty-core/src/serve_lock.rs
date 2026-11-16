//! The lock a running `brrrty serve` holds on its staging folder, so `brrrty build` and a
//! second session into the same output folder leave that folder alone.
//!
//! The lock is an OS file lock on `<output>/.brrrty-serve/lock`, which also holds the
//! session's process id for messages. The OS drops the lock when the process ends, however it
//! ends, so a folder left by a killed session is never mistaken for a live one.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::Path;

/// The lock file's name inside the staging folder.
pub const SERVE_LOCK_FILE: &str = "lock";

/// A held lock on a staging folder; released when dropped (or when the process ends).
#[derive(Debug)]
pub struct ServeLock {
    _file: File,
}

impl ServeLock {
    /// Lock `staging` (which must exist) for this process. Fails with
    /// [`io::ErrorKind::WouldBlock`] when another process holds it.
    pub fn take(staging: &Path) -> io::Result<ServeLock> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(staging.join(SERVE_LOCK_FILE))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(io::ErrorKind::WouldBlock.into()),
            Err(TryLockError::Error(e)) => return Err(e),
        }
        file.set_len(0)?;
        write!(file, "{}", std::process::id())?;
        Ok(ServeLock { _file: file })
    }
}

/// Whether a live `brrrty serve` holds `staging`: its process id as written in the lock file
/// (possibly empty), or `None` when no process holds it.
pub fn serve_lock_holder(staging: &Path) -> Option<String> {
    let path = staging.join(SERVE_LOCK_FILE);
    // Only a regular file can be a session's lock; a symlink by that name is never followed.
    if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
        return None;
    }
    let mut file = File::open(&path).ok()?;
    match file.try_lock_shared() {
        Err(TryLockError::WouldBlock) => {
            let mut pid = String::new();
            let _ = file.read_to_string(&mut pid);
            Some(pid.trim().to_string())
        }
        // Not held (the lock is released when `file` drops), or locks don't work here.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_lock_is_seen_and_a_released_one_is_not() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(serve_lock_holder(t.path()), None);
        let lock = ServeLock::take(t.path()).unwrap();
        assert_eq!(
            serve_lock_holder(t.path()),
            Some(std::process::id().to_string())
        );
        let again = ServeLock::take(t.path()).unwrap_err();
        assert_eq!(again.kind(), io::ErrorKind::WouldBlock);
        drop(lock);
        assert_eq!(serve_lock_holder(t.path()), None);
        // A lock file left by a process that has ended is stale.
        assert!(t.path().join(SERVE_LOCK_FILE).exists());
        ServeLock::take(t.path()).unwrap();
    }
}
