//! A file's creation time, which 11ty reads as Node's `stat.birthtime`.
//!
//! On Linux the standard library asks for it (through `statx`) only when built
//! against glibc, so the static musl build would fall back to the modification
//! time where 11ty, through libuv's `statx`, gets the real creation time. A
//! page without a `date` is dated by its creation time, so the two would
//! disagree about any page edited after it was created. On Linux this asks
//! `statx` itself, whatever the C library.

use std::fs::Metadata;
use std::path::Path;
use std::time::SystemTime;

/// When the file at `path` (with metadata `meta`, following symlinks) was
/// created, if the file system records it.
pub(crate) fn created(path: &Path, meta: &Metadata) -> Option<SystemTime> {
    #[cfg(target_os = "linux")]
    if let Some(t) = statx_birth(path) {
        return Some(t);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = path;
    meta.created().ok()
}

#[cfg(target_os = "linux")]
fn statx_birth(path: &Path) -> Option<SystemTime> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::time::{Duration, UNIX_EPOCH};

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: an all-zero statx is a valid value of the plain C struct.
    let mut buf: libc::statx = unsafe { std::mem::zeroed() };
    // The raw syscall, so this works with any musl version (the libc wrapper
    // is newer than some). Flags 0 follow symlinks, as `fs::metadata` does.
    // SAFETY: `c_path` is a NUL-terminated string and `buf` a statx the
    // kernel may fill; both outlive the call.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_statx,
            libc::AT_FDCWD,
            c_path.as_ptr(),
            0,
            libc::STATX_BTIME,
            &mut buf as *mut libc::statx,
        )
    };
    if rc != 0 || buf.stx_mask & libc::STATX_BTIME == 0 {
        return None;
    }
    let t = buf.stx_btime;
    let nanos = Duration::from_nanos(u64::from(t.tv_nsec));
    Some(if t.tv_sec >= 0 {
        UNIX_EPOCH + Duration::from_secs(t.tv_sec as u64) + nanos
    } else {
        UNIX_EPOCH - Duration::from_secs(t.tv_sec.unsigned_abs()) + nanos
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn creation_time_is_not_the_modification_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("page.md");
        std::fs::write(&path, "x").unwrap();
        let before = std::fs::metadata(&path).unwrap();
        // Linux file systems record it (ext4, btrfs, xfs, tmpfs, overlayfs),
        // and so do APFS and NTFS.
        let born = created(&path, &before).expect("a creation time");
        // An edit a day later moves the modification time only.
        let later = born + Duration::from_secs(86_400);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(after.modified().unwrap(), later);
        assert_eq!(created(&path, &after), Some(born));
    }
}
