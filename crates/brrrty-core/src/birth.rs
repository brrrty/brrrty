//! A file's creation and change times, as 11ty reads them: Node's
//! `stat.birthtimeMs` and `stat.ctimeMs`.
//!
//! 11ty dates a page without a `date` by it, and when it is `0` dates the page
//! (and `date: Last Modified`) by the time of the build instead
//! (`Template._getDateInstance`). On Linux, Node's libuv gets it from `statx`:
//!
//! - `statx` works: `stx_btime`, copied without looking at the mask, so a file
//!   system that records no creation time (ext4 with 128-byte inodes, some
//!   network and FUSE mounts) gives `0`, and so the build time.
//! - `statx` fails (a kernel before 4.11, a seccomp profile that blocks it):
//!   libuv falls back to `stat` and reports the change time (ctime).
//!
//! The standard library asks `statx` only when built against glibc and never
//! falls back to ctime, so on Linux this asks `statx` itself, whatever the C
//! library, and follows libuv. Elsewhere it is the standard library's
//! `created()`.

use std::fs::Metadata;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// When 11ty considers the file at `path` (with metadata `meta`, following
/// symlinks) created. `None` when Node would report a birth time of `0`: 11ty
/// then dates the page by the time of the build.
pub(crate) fn created(path: &Path, meta: &Metadata) -> Option<SystemTime> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        from_statx(statx_btime(path), (meta.ctime(), meta.ctime_nsec()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        meta.created().ok().filter(|t| *t != UNIX_EPOCH)
    }
}

/// When 11ty considers the file last modified for `date: Last Modified`:
/// Node's `stat.ctimeMs`, the inode change time, not the modification time
/// (`Template._getDateInstance("ctimeMs")`). Writing a file moves both, but
/// so does a `chmod`, a rename or a hard link, which leave the
/// modification time alone. Windows, which isn't a release target, has no
/// change time in the standard library; there it is the modification time.
pub(crate) fn changed(meta: &Metadata) -> SystemTime {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        at(meta.ctime(), meta.ctime_nsec())
    }
    #[cfg(not(unix))]
    {
        meta.modified().unwrap_or(UNIX_EPOCH)
    }
}

/// The time `secs` and `nanos` after the epoch.
#[cfg_attr(not(unix), allow(dead_code))]
fn at(secs: i64, nanos: i64) -> SystemTime {
    let nanos = Duration::from_nanos(u64::try_from(nanos).unwrap_or(0));
    if secs >= 0 {
        UNIX_EPOCH + Duration::from_secs(secs as u64) + nanos
    } else {
        UNIX_EPOCH - Duration::from_secs(secs.unsigned_abs()) + nanos
    }
}

/// libuv's birth time from `statx`'s outcome: `Some(btime)` when the call
/// worked, `None` when it failed and libuv falls back to `stat`'s ctime.
/// Times are (seconds, nanoseconds) since the epoch; a result of exactly the
/// epoch is Node's `0`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn from_statx(statx: Option<(i64, u32)>, ctime: (i64, i64)) -> Option<SystemTime> {
    let (secs, nanos) = match statx {
        Some(btime) => btime,
        None => (ctime.0, u32::try_from(ctime.1).unwrap_or(0)),
    };
    if secs == 0 && nanos == 0 {
        return None;
    }
    Some(at(secs, i64::from(nanos)))
}

/// `stx_btime` of `path`, or `None` when `statx` fails.
#[cfg(target_os = "linux")]
fn statx_btime(path: &Path) -> Option<(i64, u32)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

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
    if rc != 0 {
        return None;
    }
    Some((buf.stx_btime.tv_sec, buf.stx_btime.tv_nsec))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64, nanos: u64) -> Option<SystemTime> {
        Some(UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_nanos(nanos))
    }

    #[test]
    fn statx_birth_time_wins() {
        assert_eq!(
            from_statx(Some((1_700_000_000, 5)), (1_800_000_000, 0)),
            at(1_700_000_000, 5)
        );
    }

    #[test]
    fn no_birth_time_from_statx_means_the_build_time() {
        // The kernel zeroes stx_btime when the file system has none; Node
        // reports 0 and 11ty uses `new Date()`.
        assert_eq!(from_statx(Some((0, 0)), (1_800_000_000, 0)), None);
    }

    #[test]
    fn failed_statx_means_ctime() {
        assert_eq!(from_statx(None, (1_800_000_000, 7)), at(1_800_000_000, 7));
        assert_eq!(from_statx(None, (0, 0)), None);
    }

    #[test]
    fn before_the_epoch() {
        assert_eq!(
            from_statx(Some((-10, 500)), (0, 0)),
            Some(UNIX_EPOCH - Duration::from_secs(10) + Duration::from_nanos(500))
        );
    }

    /// `date: Last Modified` is the change time: setting the modification
    /// time back a day leaves it at now.
    #[cfg(unix)]
    #[test]
    fn the_change_time_is_not_the_modification_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("page.md");
        std::fs::write(&path, "x").unwrap();
        let earlier = SystemTime::now() - Duration::from_secs(86_400);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(earlier)
            .unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.modified().unwrap(), earlier);
        let changed = changed(&meta);
        assert!(
            changed > earlier + Duration::from_secs(3_600),
            "{changed:?}"
        );
    }

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
