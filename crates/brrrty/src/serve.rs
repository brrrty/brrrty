//! Watch the site's sources, rebuild on change, and point the server at each new build.
//!
//! Every build goes into a fresh folder under `<output>/.brrrty-serve/`, and the server is
//! switched to it only once the build has finished without errors, so a page is never served
//! half written and a broken edit leaves the last good build in place. An old build folder is
//! removed once no request is still using it.
//!
//! After every good build the output folder is brought up to date too, as 11ty's `--serve`
//! writes it on every build: each file whose bytes changed is copied in and renamed into place,
//! so `_site` is current while the session runs and after it is killed. Like `brrrty build`,
//! this never deletes a file from the output folder. When the session stops, the server is
//! pointed at the output folder and the staging folder is removed.
//!
//! ```no_run
//! # use std::path::Path;
//! use brrrty::serve::{Session, Settings};
//! use brrrty_serve::Server;
//!
//! let session = Session::start(Path::new("."), Settings::default()).unwrap();
//! let server = Server::start(session.serving(), "localhost", 8080).unwrap();
//! let stop = session.stop_handle(); // call `stop.stop()` from a Ctrl-C handler
//! session.run(&server);
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use brrrty_core::ignore_files::IgnoreRules;
use brrrty_core::{BuildOptions, Problem, RunMode};
use brrrty_markdown::MarkdownIt;
use brrrty_serve::{RetiredRoot, RunningServer};
use notify::event::{AccessKind, AccessMode, ModifyKind};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::options::{Flags, build_options};
use crate::report;

/// The folder inside the output folder that holds the builds of a session.
pub const STAGING_DIR: &str = brrrty_core::SERVE_STAGING_DIR;

/// How long a file must stay unchanged before a rebuild starts. 11ty's watcher waits for a
/// file to be stable for 150 ms (chokidar's `awaitWriteFinish.stabilityThreshold`) and then
/// builds at once (`watchThrottleWaitTime` 0), so a burst of saves makes one build.
pub const DEBOUNCE: Duration = Duration::from_millis(150);

/// How often an idle session checks whether an old build folder can be removed.
const REAP_EVERY: Duration = Duration::from_millis(250);

/// How long stopping waits for requests still reading the last build before removing it.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// Files in the project folder whose change means a rebuild: the configs and the ignore
/// files, as 11ty watches them.
const ROOT_FILES: &[&str] = &[
    brrrty_config::CONFIG_FILE,
    ".gitignore",
    ".eleventyignore",
    "package.json",
    "eleventy.config.js",
    "eleventy.config.cjs",
    "eleventy.config.mjs",
    ".eleventy.js",
];

/// How a session prints and waits.
#[derive(Debug, Clone)]
pub struct Settings {
    /// The build flags, as `brrrty build` takes them.
    pub flags: Flags,
    /// Don't print the summary line or the changed files.
    pub quiet: bool,
    /// Color the problems.
    pub color: bool,
    /// See [`DEBOUNCE`].
    pub debounce: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            flags: Flags::default(),
            quiet: false,
            color: false,
            debounce: DEBOUNCE,
        }
    }
}

/// What one rebuild did, for the live-reload layer.
#[derive(Debug, Clone)]
pub struct Rebuild {
    /// The build had no errors and is now being served.
    pub ok: bool,
    /// The files whose change started it, relative to the project folder.
    pub changed_paths: Vec<PathBuf>,
    /// Every changed file is a stylesheet outside `_includes`, which 11ty's dev server
    /// swaps in place instead of reloading the page.
    pub css_only: bool,
    /// What the build reported, warnings included.
    pub problems: Vec<Problem>,
}

/// Called after every rebuild, on the session's thread.
pub type RebuildHook = dyn Fn(&Rebuild) + Send + Sync;

/// What a session ended with.
#[derive(Debug, Default)]
pub struct Finished {
    /// Rebuilds run after the first build.
    pub rebuilds: usize,
    /// The output folder holds the last good build (false when there was none, or a file
    /// could not be written).
    pub output_written: bool,
    /// What went wrong bringing the output folder up to date or removing staging.
    pub problems: Vec<Problem>,
}

/// Why a session could not start.
#[derive(Debug)]
pub enum StartError {
    /// The config or the folders are wrong; nothing was built.
    Problems(Vec<Problem>),
    /// The staging folder could not be made.
    Staging(PathBuf, io::Error),
    /// The file watcher could not start.
    Watch(notify::Error),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Problems(p) => write!(f, "{} problem(s) in the config or folders", p.len()),
            StartError::Staging(path, e) => {
                write!(f, "cannot make the folder {}: {e}", path.display())
            }
            StartError::Watch(e) => write!(f, "cannot watch the source files: {e}"),
        }
    }
}

impl std::error::Error for StartError {}

enum Msg {
    Fs(notify::Result<Event>),
    Stop,
}

/// Stops a running [`Session`]. Safe to call from a signal handler thread, and more than once.
#[derive(Clone)]
pub struct StopHandle(Sender<Msg>);

impl StopHandle {
    /// Ask the session to finish. A build already running finishes first.
    pub fn stop(&self) {
        let _ = self.0.send(Msg::Stop);
    }
}

/// A build folder waiting to be removed.
struct Retired {
    root: Option<RetiredRoot>,
    path: PathBuf,
}

/// A serve session: the watcher, the staging folder and the builds in it.
pub struct Session {
    settings: Settings,
    /// The flags with the input and output folders fixed at the first build.
    flags: Flags,
    root: PathBuf,
    input: PathBuf,
    output: PathBuf,
    staging: PathBuf,
    next_build: u64,
    /// The last good build, under `staging`.
    current: Option<PathBuf>,
    /// What the server should start on.
    serving: PathBuf,
    first_ok: bool,
    first_problems: Vec<Problem>,
    retired: Vec<Retired>,
    passthrough: Vec<PathBuf>,
    rules: Option<IgnoreRules>,
    /// `None` once the session is finishing.
    watcher: Option<RecommendedWatcher>,
    /// Folders watched without their subfolders: the input folder and the project folder.
    tops: Vec<PathBuf>,
    /// Folders watched recursively.
    watched: Vec<PathBuf>,
    /// The watches can't be trusted (the watcher asked for a rescan, or the input folder
    /// itself went away): set them all up again before the next build.
    rewatch: bool,
    /// What went wrong the last time the output folder was brought up to date.
    mirror_problems: Vec<Problem>,
    /// A temporary file name counter, for copies on their way into the output folder.
    next_tmp: u64,
    /// The watcher's last error, so a repeated one is printed once.
    last_watch_error: Option<String>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    hook: Option<Arc<RebuildHook>>,
}

impl Session {
    /// Resolve the folders, start watching, and run the first build. A first build with
    /// errors does not stop the session: its problems are printed, the output folder is served
    /// as it is (or an empty folder when there is none), and the next good build replaces it.
    /// [`Session::first_ok`] says which happened.
    pub fn start(root: &Path, settings: Settings) -> Result<Session, StartError> {
        let opts = build_options(root, &settings.flags).map_err(StartError::Problems)?;
        opts.validate().map_err(|p| StartError::Problems(vec![p]))?;
        let problem = |p| StartError::Problems(vec![p]);
        let root = opts.root_dir().map_err(problem)?;
        let input = opts.input_dir().map_err(problem)?;
        let output = opts.output_dir().map_err(problem)?;
        let mut flags = settings.flags.clone();
        flags.input = Some(opts.input.clone());
        flags.output = Some(opts.output.clone());

        let staging = output.join(STAGING_DIR);
        // A folder left by a session that was killed before it could clean up, or a symlink
        // by that name, which is removed and never followed.
        if fs::symlink_metadata(&staging).is_ok() {
            remove_any(&staging).map_err(|e| StartError::Staging(staging.clone(), e))?;
        }
        fs::create_dir_all(&staging).map_err(|e| StartError::Staging(staging.clone(), e))?;
        // Compare against real paths: the watcher reports them.
        let output = output.canonicalize().unwrap_or(output);
        let staging = output.join(STAGING_DIR);

        let (tx, rx) = mpsc::channel();
        let events = tx.clone();
        let watcher = notify::recommended_watcher(move |res| {
            let _ = events.send(Msg::Fs(res));
        })
        .map_err(StartError::Watch)?;

        let mut session = Session {
            settings,
            flags,
            root,
            input,
            output: output.clone(),
            staging,
            next_build: 0,
            current: None,
            serving: output,
            first_ok: false,
            first_problems: Vec::new(),
            retired: Vec::new(),
            passthrough: Vec::new(),
            rules: None,
            watcher: Some(watcher),
            tops: Vec::new(),
            watched: Vec::new(),
            rewatch: false,
            mirror_problems: Vec::new(),
            next_tmp: 0,
            last_watch_error: None,
            tx,
            rx,
            hook: None,
        };
        session.refresh(&opts);
        session.watch_all().map_err(StartError::Watch)?;

        let (ok, problems, dir) = session.build();
        session.first_ok = ok;
        session.first_problems = problems;
        match dir {
            Some(dir) => {
                session.mirror(&dir);
                session.serving = dir.clone();
                session.current = Some(dir);
            }
            None if session.output.is_dir() => {}
            None => {
                let empty = session.new_build_dir();
                fs::create_dir_all(&empty).map_err(|e| StartError::Staging(empty.clone(), e))?;
                session.serving = empty;
            }
        }
        Ok(session)
    }

    /// The folder to start the server on.
    pub fn serving(&self) -> &Path {
        &self.serving
    }

    /// Whether the first build had no errors.
    pub fn first_ok(&self) -> bool {
        self.first_ok
    }

    /// What the first build reported.
    pub fn first_problems(&self) -> &[Problem] {
        &self.first_problems
    }

    /// The staging folder, inside the output folder.
    pub fn staging(&self) -> &Path {
        &self.staging
    }

    /// The output folder, as its real path.
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// A handle that stops [`Session::run`].
    pub fn stop_handle(&self) -> StopHandle {
        StopHandle(self.tx.clone())
    }

    /// Call `hook` after every rebuild.
    pub fn on_rebuild(&mut self, hook: Arc<RebuildHook>) {
        self.hook = Some(hook);
    }

    /// Watch and rebuild until stopped, then point the server at the output folder, which
    /// holds the last good build, and remove the staging folder.
    pub fn run(mut self, server: &RunningServer) -> Finished {
        let mut finished = Finished::default();
        // The server may have been started on something else.
        if let Some(cur) = &self.current
            && server.root() != *cur
            && let Ok(old) = server.swap_root(cur)
        {
            self.retire(old);
        }
        'outer: loop {
            self.reap();
            let first = match self.rx.recv_timeout(REAP_EVERY) {
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Msg::Fs(ev)) => ev,
                Err(RecvTimeoutError::Timeout) => continue,
            };
            let mut changed = Vec::new();
            self.collect(first, &mut changed);
            if changed.is_empty() {
                continue;
            }
            // Wait for the files to settle: every relevant event restarts the wait.
            let mut quiet_since = Instant::now();
            let mut stop = false;
            loop {
                let left = self.settings.debounce.saturating_sub(quiet_since.elapsed());
                if left.is_zero() {
                    break;
                }
                match self.rx.recv_timeout(left) {
                    Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => {
                        stop = true;
                        break;
                    }
                    Ok(Msg::Fs(ev)) => {
                        if self.collect(ev, &mut changed) {
                            quiet_since = Instant::now();
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                }
            }
            if stop {
                break 'outer;
            }
            // Saves that land while this runs queue up in the channel and make exactly one
            // more build once it is done, after their own wait.
            self.rebuild(server, &changed);
            finished.rebuilds += 1;
        }
        self.finish(server, &mut finished);
        finished
    }

    fn rebuild(&mut self, server: &RunningServer, changed: &[PathBuf]) {
        if !self.settings.quiet {
            for p in changed {
                println!("File changed: {}", p.display());
            }
        }
        let (mut ok, problems, dir) = self.build();
        if let Some(dir) = dir {
            match server.swap_root(&dir) {
                Ok(old) => {
                    self.retire(old);
                    self.mirror(&dir);
                    self.current = Some(dir);
                }
                Err(e) => {
                    eprintln!("Brrrty could not serve the new build: {e}");
                    let _ = fs::remove_dir_all(&dir);
                    ok = false;
                }
            }
        }
        let css_only = !changed.is_empty()
            && changed.iter().all(|p| {
                p.extension().is_some_and(|e| e == "css")
                    && !self.root.join(p).starts_with(self.input.join("_includes"))
            });
        if let Some(hook) = &self.hook {
            hook(&Rebuild {
                ok,
                changed_paths: changed.to_vec(),
                css_only,
                problems,
            });
        }
    }

    fn new_build_dir(&mut self) -> PathBuf {
        self.next_build += 1;
        self.staging.join(format!("build-{}", self.next_build))
    }

    /// Build into a fresh folder. Gives whether it had no errors, its problems, and the folder
    /// when it did; a failed build's folder is removed at once.
    fn build(&mut self) -> (bool, Vec<Problem>, Option<PathBuf>) {
        let color = self.settings.color;
        let mut opts = match build_options(&self.root, &self.flags) {
            Ok(o) => o,
            Err(problems) => {
                report::print_problems(&problems, &self.root, color);
                return (false, problems, None);
            }
        };
        self.refresh(&opts);
        let dir = self.new_build_dir();
        // The user (or a `brrrty build`) may have removed the output folder meanwhile.
        let _ = fs::create_dir_all(&self.staging);
        if let Err(e) = fs::create_dir_all(&dir) {
            let p = Problem::error(
                brrrty_core::codes::WRITE_FAILED,
                format!("cannot make the build folder {}: {e}", dir.display()),
            );
            report::print_problems(std::slice::from_ref(&p), &self.root, color);
            return (false, vec![p], None);
        }
        opts.write_to = Some(dir.clone());
        opts.run_mode = RunMode::Serve;
        let report = brrrty_core::build(&opts, &MarkdownIt::new());
        report::print_build(&report, &self.root, self.settings.quiet, color);
        let ok = !report.has_errors();
        if !ok {
            let _ = fs::remove_dir_all(&dir);
        }
        (ok, report.problems, ok.then_some(dir))
    }

    /// Re-read what decides which changes count: the ignore files and the passthrough paths.
    fn refresh(&mut self, opts: &BuildOptions) {
        self.rules = opts
            .input_rel()
            .ok()
            .and_then(|rel| IgnoreRules::load(&self.root, &self.input, &rel).ok());
        let mut pt: Vec<PathBuf> = opts
            .config
            .passthrough
            .iter()
            .map(|e| PathBuf::from(&e.from))
            .chain(opts.passthrough.iter().cloned())
            .map(|p| self.root.join(p))
            .map(|p| p.canonicalize().unwrap_or(p))
            .collect();
        pt.sort();
        pt.dedup();
        self.passthrough = pt;
        if self.rewatch {
            self.rewatch_all();
        } else {
            self.sync_subfolders();
        }
        // Passthrough folders outside the input folder need watches of their own.
        let extra: Vec<PathBuf> = self
            .passthrough
            .iter()
            .filter(|p| !p.starts_with(&self.input) && p.exists())
            .cloned()
            .collect();
        for p in extra {
            if !self.watched.contains(&p) && !self.skipped(&p) {
                // A source that can't be watched still builds; its edits just aren't seen.
                if self.watch(&p, RecursiveMode::Recursive).is_ok() {
                    self.watched.push(p);
                }
            }
        }
    }

    /// Watch the input folder and the project folder. Each is watched without its
    /// subfolders, and each subfolder recursively, so `node_modules`, `.git` and the output
    /// folder are never watched at all (on Linux every watched folder costs an inotify watch).
    fn watch_all(&mut self) -> notify::Result<()> {
        let mut tops = vec![self.input.clone()];
        if self.root != self.input {
            tops.push(self.root.clone());
        }
        for top in tops {
            self.watch(&top, RecursiveMode::NonRecursive)?;
            self.tops.push(top.clone());
            // The project folder's own subfolders are not sources unless they hold the input.
            let deep = top == self.input;
            if let Ok(entries) = fs::read_dir(&top) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() && deep {
                        self.watch_subfolder(&p);
                    }
                }
            }
        }
        Ok(())
    }

    /// Drop every watch and set them up again from what is on disk now. notify walks each
    /// folder it is asked to watch recursively, so this is a full-tree check.
    fn rewatch_all(&mut self) {
        let old: Vec<PathBuf> = self.tops.drain(..).chain(self.watched.drain(..)).collect();
        for p in old {
            self.unwatch(&p);
        }
        // The input folder may still be missing; try again before the next build.
        self.rewatch = self.watch_all().is_err();
        let extra: Vec<PathBuf> = self
            .passthrough
            .iter()
            .filter(|p| !p.starts_with(&self.input) && p.exists())
            .cloned()
            .collect();
        for p in extra {
            self.watch_subfolder(&p);
        }
    }

    /// Make the recursive watches on the input folder's subfolders match the disk: forget
    /// the ones whose folder is gone and watch every subfolder that has none.
    fn sync_subfolders(&mut self) {
        let gone: Vec<PathBuf> = self
            .watched
            .iter()
            .filter(|p| !p.is_dir())
            .cloned()
            .collect();
        for p in gone {
            self.forget(&p);
        }
        if let Ok(entries) = fs::read_dir(&self.input) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    self.watch_subfolder(&p);
                }
            }
        }
    }

    /// Drop the watches on `p` and on anything under it. With inotify a removed folder's
    /// watch is gone even if a folder of the same name comes back, so the name must not
    /// stay listed as watched.
    fn forget(&mut self, p: &Path) {
        let (gone, kept): (Vec<PathBuf>, Vec<PathBuf>) =
            self.watched.drain(..).partition(|w| w.starts_with(p));
        self.watched = kept;
        for w in gone {
            self.unwatch(&w);
        }
    }

    fn unwatch(&mut self, p: &Path) {
        if let Some(w) = &mut self.watcher {
            // It may already be gone with its folder.
            let _ = w.unwatch(p);
        }
    }

    fn watch_subfolder(&mut self, p: &Path) {
        let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        if self.skipped(&p) || self.watched.contains(&p) {
            return;
        }
        if self.watch(&p, RecursiveMode::Recursive).is_ok() {
            self.watched.push(p);
        }
    }

    fn watch(&mut self, p: &Path, mode: RecursiveMode) -> notify::Result<()> {
        match &mut self.watcher {
            Some(w) => w.watch(p, mode),
            None => Ok(()),
        }
    }

    /// Never watched: the output folder (staging included), anything holding it, and
    /// `node_modules` and `.git`.
    fn skipped(&self, p: &Path) -> bool {
        p.starts_with(&self.output)
            || self.output.starts_with(p)
            || p.components().any(|c| {
                let c = c.as_os_str();
                c == "node_modules" || c == ".git"
            })
    }

    /// Add the paths of `ev` that should start a rebuild, relative to the project folder.
    /// Gives whether there was one, even one already in `changed`.
    fn collect(&mut self, ev: notify::Result<Event>, changed: &mut Vec<PathBuf>) -> bool {
        let ev = match ev {
            Ok(ev) => ev,
            Err(e) => {
                // Such as running out of inotify watches. Say so once, and set the watches
                // up again before the next build; no rebuild of its own, so an error that
                // keeps coming back can't loop.
                let message = e.to_string();
                if self.last_watch_error.as_deref() != Some(message.as_str()) {
                    eprintln!("Brrrty's file watcher reported a problem: {message}");
                    self.last_watch_error = Some(message);
                }
                self.rewatch = true;
                return false;
            }
        };
        // The watcher lost track (an inotify queue overflow, FSEvents dropping events):
        // watch everything again and rebuild, since any change may have been missed.
        if ev.need_rescan() {
            self.rewatch = true;
            let rel = self
                .input
                .strip_prefix(&self.root)
                .unwrap_or(&self.input)
                .to_path_buf();
            if !changed.contains(&rel) {
                changed.push(rel);
            }
            return true;
        }
        match ev.kind {
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any => {
            }
            EventKind::Access(AccessKind::Close(AccessMode::Write)) => {}
            _ => return false,
        }
        let mut any = false;
        let moved_or_removed = matches!(
            ev.kind,
            EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)) | EventKind::Any
        );
        let appeared = matches!(
            ev.kind,
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_)) | EventKind::Any
        );
        for p in &ev.paths {
            if moved_or_removed {
                // A watched folder (or one holding it) went away or was renamed.
                self.forget(p);
                if self.tops.iter().any(|t| t.starts_with(p)) {
                    self.rewatch = true;
                }
            }
            // A folder created, re-created or moved into the input folder, which is watched
            // shallowly, needs its own watch. Moved in arrives as a rename, not a create.
            if p.parent() == Some(self.input.as_path()) && p.is_dir() {
                if appeared {
                    // Possibly a new folder under an old name: its old watch is dead.
                    self.forget(p);
                }
                self.watch_subfolder(p);
            }
            if self.rewatch && p.is_dir() && (*p == self.input || *p == self.root) {
                self.rewatch_all();
            }
            if self.counts(p) {
                any = true;
                let rel = p.strip_prefix(&self.root).unwrap_or(p).to_path_buf();
                if !changed.contains(&rel) {
                    changed.push(rel);
                }
            }
        }
        any
    }

    /// Whether a change to `p` should start a rebuild.
    fn counts(&self, p: &Path) -> bool {
        if self.skipped(p) {
            return false;
        }
        let root_file = p.parent() == Some(self.root.as_path())
            && p.file_name()
                .is_some_and(|n| ROOT_FILES.iter().any(|f| n == *f));
        let source = p.starts_with(&self.input) && p != self.input;
        let passthrough = self.passthrough.iter().any(|t| p.starts_with(t));
        if !(root_file || source || passthrough) {
            return false;
        }
        !self.rules.as_ref().is_some_and(|r| r.is_ignored(p, false))
    }

    fn retire(&mut self, old: RetiredRoot) {
        let path = old.path().to_path_buf();
        // Only folders this session made are removed; anything else is left alone.
        if path.starts_with(&self.staging) && path != self.staging {
            self.retired.push(Retired {
                root: Some(old),
                path,
            });
        }
    }

    /// Remove the old build folders no request is using any more. A folder that can't be
    /// removed yet (a file still open on Windows) is tried again later.
    fn reap(&mut self) {
        self.retired.retain(|r| {
            if r.root.as_ref().is_some_and(RetiredRoot::in_use) {
                return true;
            }
            match fs::remove_dir_all(&r.path) {
                Ok(()) => false,
                Err(e) => e.kind() != io::ErrorKind::NotFound,
            }
        });
    }

    fn finish(mut self, server: &RunningServer, finished: &mut Finished) {
        // Stop watching first: nothing after this is an edit.
        self.watcher = None;
        if let Some(cur) = self.current.take() {
            // The output folder is brought up to date after every build; only a copy that
            // failed needs another try.
            if !self.mirror_problems.is_empty() {
                self.mirror(&cur);
            }
            finished.output_written = self.mirror_problems.is_empty();
            finished.problems.append(&mut self.mirror_problems);
        }
        // Serve the output folder, which holds the same files, before staging goes, so no
        // request in between finds nothing.
        if self.output.is_dir()
            && let Ok(old) = server.swap_root(&self.output)
        {
            self.retire(old);
        }
        self.cleanup(finished);
    }

    fn cleanup(&mut self, finished: &mut Finished) {
        let until = Instant::now() + STOP_GRACE;
        while self
            .retired
            .iter()
            .any(|r| r.root.as_ref().is_some_and(RetiredRoot::in_use))
            && Instant::now() < until
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.retired.clear();
        if let Err(e) = fs::remove_dir_all(&self.staging)
            && e.kind() != io::ErrorKind::NotFound
        {
            finished.problems.push(Problem::warning(
                brrrty_core::codes::SERVE_STAGING_LEFT,
                format!(
                    "could not remove the staging folder {}: {e}",
                    self.staging.display()
                ),
            ));
        }
    }

    /// Bring the output folder up to date with the build in `build`, printing what went wrong.
    fn mirror(&mut self, build: &Path) {
        let mut m = Mirror {
            staging: &self.staging,
            next_tmp: &mut self.next_tmp,
            problems: Vec::new(),
        };
        let staged = build.join(STAGING_DIR);
        if fs::symlink_metadata(&staged).is_ok() {
            // `<output>/.brrrty-serve` is the staging folder: the site's own file by that
            // name is left out, and must never be shipped.
            m.problem(format!(
                "the site wrote {STAGING_DIR}, which is Brrrty's staging folder, so it was left out of {}",
                self.output.display()
            ));
        }
        m.tree(build, &self.output, true);
        self.mirror_problems = m.problems;
        if !self.mirror_problems.is_empty() {
            report::print_problems(&self.mirror_problems, &self.root, self.settings.color);
        }
    }
}

fn remove_any(p: &Path) -> io::Result<()> {
    if fs::symlink_metadata(p)?.is_dir() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

/// Copies a build into the output folder.
struct Mirror<'a> {
    staging: &'a Path,
    next_tmp: &'a mut u64,
    problems: Vec<Problem>,
}

impl Mirror<'_> {
    fn problem(&mut self, message: String) {
        self.problems
            .push(Problem::warning(brrrty_core::codes::WRITE_FAILED, message));
    }

    /// Copy every file of `from` into `to` at the same path, unless `to` already has the
    /// same bytes there. Files in `to` that `from` doesn't have are kept, as `brrrty build`
    /// keeps them. A file is copied into staging first and then renamed into place, so it is
    /// never seen half written. A symlink in `to` is replaced, never followed, so nothing
    /// lands outside `to`. At the top, the staging folder's own name is skipped.
    fn tree(&mut self, from: &Path, to: &Path, top: bool) {
        let entries = match fs::read_dir(from) {
            Ok(e) => e,
            Err(e) => return self.problem(format!("cannot read {}: {e}", from.display())),
        };
        for entry in entries.flatten() {
            if top && entry.file_name() == STAGING_DIR {
                continue;
            }
            let src = entry.path();
            let dst = to.join(entry.file_name());
            let existing = fs::symlink_metadata(&dst).ok();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                let made = (|| {
                    match &existing {
                        Some(m) if m.is_dir() => {}
                        Some(_) => fs::remove_file(&dst)?,
                        None => {}
                    }
                    fs::create_dir_all(&dst)
                })();
                match made {
                    Ok(()) => self.tree(&src, &dst, false),
                    Err(e) => self.problem(format!("cannot make {}: {e}", dst.display())),
                }
                continue;
            }
            if existing.as_ref().is_some_and(|m| m.is_file()) && same_bytes(&src, &dst) {
                continue;
            }
            *self.next_tmp += 1;
            let tmp = self.staging.join(format!("copy-{}", self.next_tmp));
            let copied = (|| {
                fs::copy(&src, &tmp)?;
                if existing.as_ref().is_some_and(|m| m.is_dir()) {
                    fs::remove_dir_all(&dst)?;
                }
                fs::rename(&tmp, &dst)
            })();
            if let Err(e) = copied {
                let _ = fs::remove_file(&tmp);
                self.problem(format!("cannot write {}: {e}", dst.display()));
            }
        }
    }
}

/// Whether two regular files hold the same bytes.
fn same_bytes(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (fs::metadata(a), fs::metadata(b)) else {
        return false;
    };
    if ma.len() != mb.len() {
        return false;
    }
    matches!((fs::read(a), fs::read(b)), (Ok(x), Ok(y)) if x == y)
}
