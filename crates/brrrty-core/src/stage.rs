//! The folder a build's pages wait in while the order check runs:
//! `<output>/.brrrty-stage-<pid>/`, each page under a number. A page is
//! moved to its place once the check passes; the folder goes when the
//! build ends, however it ends:
//!
//! - a build that returns drops its [`Stage`];
//! - a signal or a panic ends the process before destructors run, so the
//!   program calls [`remove_stages`] on those paths;
//! - a process killed outright leaves the folder behind, and the next
//!   build into that output folder removes it ([`sweep`]).
//!
//! A build that made the output folder for its stage and wrote nothing
//! there removes that too: a refused build leaves no output folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What the folders' names start with, directly inside an output folder.
/// Only Brrrty writes such names.
pub const STAGE_PREFIX: &str = ".brrrty-stage-";

struct Live {
    /// Set by [`remove_stages`]: the process is ending, and no build
    /// stages anything from then on.
    stopped: bool,
    stages: Vec<Entry>,
}

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    /// The folders this build made to hold `path`, outermost first: the
    /// output folder and any parent it lacked.
    made: Vec<PathBuf>,
}

static LIVE: Mutex<Live> = Mutex::new(Live {
    stopped: false,
    stages: Vec::new(),
});

fn live() -> std::sync::MutexGuard<'static, Live> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

fn remove(e: &Entry) {
    // Pages may still be landing in it from the writer threads: until it
    // is gone, or it is clear that it won't go.
    for _ in 0..50 {
        match fs::remove_dir_all(&e.path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                std::thread::sleep(std::time::Duration::from_millis(2))
            }
            _ => break,
        }
    }
    // Only while empty: `remove_dir` refuses a folder that holds anything.
    for dir in e.made.iter().rev() {
        if fs::remove_dir(dir).is_err() {
            break;
        }
    }
}

/// A build's staging folder, removed when this drops.
pub(crate) struct Stage(Entry);

impl Stage {
    /// Make the staging folder of this process inside `out`, or `None`
    /// where it can't be made or the process is ending.
    pub(crate) fn new(out: &Path) -> Option<Stage> {
        let path = out.join(format!("{STAGE_PREFIX}{}", std::process::id()));
        let mut made: Vec<PathBuf> = out
            .ancestors()
            .take_while(|p| !p.as_os_str().is_empty() && !p.exists())
            .map(Path::to_path_buf)
            .collect();
        made.reverse();
        let entry = Entry { path, made };
        let mut l = live();
        if l.stopped {
            return None;
        }
        // Registered before it exists, so no signal finds it unlisted.
        l.stages.push(entry.clone());
        drop(l);
        // One left by an earlier build of this process (which a panic
        // caught by the caller may leave) is not this build's.
        let _ = fs::remove_dir_all(&entry.path);
        let stage = Stage(entry);
        fs::create_dir_all(&stage.0.path).ok()?;
        Some(stage)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0.path
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        remove(&self.0);
        live().stages.retain(|e| e.path != self.0.path);
    }
}

/// Remove the staging folder of every build in progress, and keep any
/// build from making one: for a signal handler's thread and a panic
/// handler, which end the process without running destructors.
pub fn remove_stages() {
    let mut l = live();
    l.stopped = true;
    for e in &l.stages {
        remove(e);
    }
}

/// Remove the staging folders earlier builds left in `out`: those of
/// processes that are gone (killed before they could tidy up), and any
/// of this process that no build in progress owns. One whose process is
/// alive belongs to a build running now, and stays.
pub(crate) fn sweep(out: &Path) {
    let Ok(entries) = fs::read_dir(out) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix(STAGE_PREFIX)) else {
            continue;
        };
        let path = entry.path();
        let stale = match pid.parse::<u32>() {
            Ok(pid) if pid == std::process::id() => !live().stages.iter().any(|e| e.path == path),
            Ok(pid) => !alive(pid),
            Err(_) => true,
        };
        if !stale {
            continue;
        }
        // Never followed, if a link by that name is there.
        let _ = match fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => fs::remove_dir_all(&path),
            _ => fs::remove_file(&path),
        };
    }
}

/// Whether a process with this id is running.
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 sends nothing; it only asks whether the process
    // is there. EPERM says it is, and is another user's.
    unsafe {
        libc::kill(pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

/// Elsewhere there is no cheap way to ask: a folder that isn't this
/// process's is taken for stale. Should its build be running after all,
/// it writes its pages straight to their places instead.
#[cfg(not(unix))]
fn alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_sweep_removes_the_folders_of_dead_processes_and_keeps_a_live_ones() {
        let t = tempfile::tempdir().expect("temp dir");
        let out = t.path();
        // A process that has ended: its id is free, or at least not ours.
        let mut gone = std::process::Command::new(std::env::current_exe().expect("this test"))
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("starts");
        let dead = gone.id();
        gone.wait().expect("ends");
        let stale = out.join(format!("{STAGE_PREFIX}{dead}"));
        let odd = out.join(format!("{STAGE_PREFIX}x"));
        let ours = out.join(format!("{STAGE_PREFIX}{}", std::process::id()));
        // A process that is alive and not this one: the one that runs the tests.
        let parent = out.join(format!(
            "{STAGE_PREFIX}{}",
            std::os::unix::process::parent_id()
        ));
        for d in [&stale, &odd, &ours, &parent] {
            fs::create_dir_all(d).expect("made");
            fs::write(d.join("0"), "page").expect("written");
        }
        fs::write(out.join("index.html"), "kept").expect("written");
        sweep(out);
        assert!(!stale.exists(), "a dead process's folder stays");
        assert!(!odd.exists(), "a folder with no process id stays");
        assert!(!ours.exists(), "this process's own stale folder stays");
        assert!(parent.exists(), "a live process's folder went");
        assert!(out.join("index.html").exists());
    }
}
