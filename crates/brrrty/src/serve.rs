//! Watch the site's sources, rebuild on change, and point the server at each new build.
//!
//! Every build goes into a fresh folder under `<output>/.brrrty-serve/`, and the server is
//! switched to it once the build has finished, so a page is never served half written. A
//! build whose errors are only refused pages is served, as `brrrty build` writes it, and each
//! refused page whose last build was served keeps that file, so a typo in the page being
//! edited never turns it into a 404. A build that stopped before writing any page leaves the
//! last one in place. An old build folder is removed once no request is still using it.
//!
//! After every build served, the output folder is brought up to date too, as 11ty's `--serve`
//! writes it on every build: each file the build wrote whose bytes changed is copied in and
//! renamed into place, so `_site` is current while the session runs and after it is killed.
//! It gets exactly what `brrrty build` would write, so the pages kept from an earlier build
//! never go there. Like `brrrty build`, this never deletes a file from the output folder. When
//! the session stops, the server is pointed at the output folder and the staging folder is
//! removed.
//!
//! Pages served get a live-reload script ([`Session::live_reload`], see [`live`]).
//!
//! ```no_run
//! # use std::path::Path;
//! use brrrty::serve::{Session, Settings};
//! use brrrty_serve::Server;
//!
//! let mut session = Session::start(Path::new("."), Settings::default()).unwrap();
//! let server = Server::start(session.serving(), "localhost", 8080).unwrap();
//! session.live_reload(&server);
//! let stop = session.stop_handle(); // call `stop.stop()` from a Ctrl-C handler
//! session.run(&server);
//! ```

pub mod live;
pub mod signals;

use std::collections::HashMap;
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

pub use live::LiveReload;

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
    /// The build is now being served, errors or not: it got as far as writing pages.
    pub served: bool,
    /// The templates, relative to the project folder, with a page this build refused.
    pub refused: Vec<PathBuf>,
    /// How many refused pages are served from an earlier build.
    pub kept: usize,
    /// The files whose change started it, relative to the project folder.
    pub changed_paths: Vec<PathBuf>,
    /// Every changed file is a stylesheet outside `_includes`, which 11ty's dev server
    /// swaps in place instead of reloading the page.
    pub css_only: bool,
    /// What the build reported, warnings included.
    pub problems: Vec<Problem>,
    /// The pages of the changed templates, as now served. Read only while live-reload tabs are
    /// connected ([`Session::live_reload`]); empty otherwise, and when the build failed.
    pub pages: Vec<ChangedPage>,
    /// The URLs of the changed stylesheets, as passthrough copied them.
    pub stylesheets: Vec<String>,
    /// What went wrong bringing the output folder up to date with this build.
    pub output_problems: Vec<Problem>,
}

/// A page of a changed template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedPage {
    /// Its `page.url`.
    pub url: String,
    /// The HTML written.
    pub content: String,
}

/// What one build gave.
struct Built {
    /// No errors.
    ok: bool,
    problems: Vec<Problem>,
    /// The build's folder, when it got as far as writing pages and is to be served.
    dir: Option<PathBuf>,
    pages: Vec<brrrty_core::WrittenPage>,
    passthrough: Vec<(PathBuf, PathBuf)>,
    refused: Vec<PathBuf>,
    /// What the output folder should get from it.
    entries: Vec<Entry>,
}

/// What a file in a build holds, as far as bringing the output folder up to date goes.
#[derive(Debug, Clone)]
enum Version {
    /// A page, by the hash of the bytes the build wrote.
    Page(u64),
    /// A passthrough copy, by its source as it was just before the build; `None` when that
    /// could not be read, which matches nothing.
    Copy(Option<Stamp>),
}

impl PartialEq for Version {
    fn eq(&self, other: &Version) -> bool {
        match (self, other) {
            (Version::Page(a), Version::Page(b)) => a == b,
            // An unknown stamp never counts as unchanged.
            (Version::Copy(Some(a)), Version::Copy(Some(b))) => a == b,
            _ => false,
        }
    }
}

/// A file a build wrote, relative to the build folder.
#[derive(Debug, Clone)]
struct Entry {
    rel: PathBuf,
    version: Version,
}

/// A file the session put in the output folder (or found there already the same), and how
/// the output folder's file looked just after, so a change made there by anything else is
/// noticed without reading it.
#[derive(Debug, Clone)]
struct Record {
    version: Version,
    len: u64,
    modified: Option<std::time::SystemTime>,
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
    first_served: bool,
    first_problems: Vec<Problem>,
    /// The pages of the build being served, those kept from earlier builds included.
    served_pages: Vec<brrrty_core::WrittenPage>,
    /// What the session knows the output folder holds, by path relative to it.
    mirrored: HashMap<PathBuf, Record>,
    /// The files of the last build served, for another try at a copy that failed.
    last_entries: Vec<Entry>,
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
    /// The passthrough files as they were just before the last build, which copies them. On
    /// macOS, copying a file can make FSEvents report it again, flags and all, though nothing
    /// changed; an event for a file that still looks like this is that echo, not an edit.
    copied: HashMap<PathBuf, Stamp>,
    /// The same, just before the build before it; `None` until there was one. A file whose
    /// stamp moved between the two builds without being among the files that started the
    /// later one was edited in the moment between the debounce firing and the stamps being
    /// taken, so its event is real even though its stamp still matches.
    copied_before: Option<HashMap<PathBuf, Stamp>>,
    /// The files that started the last build, relative to the project folder.
    last_changed: Vec<PathBuf>,
    /// The live-reload channel, when attached: changed pages are read only for open tabs.
    live: Option<LiveReload>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    hooks: Vec<Arc<RebuildHook>>,
}

impl Session {
    /// Resolve the folders, start watching, and run the first build. A first build with
    /// errors does not stop the session: its problems are printed, and it is served when it got
    /// as far as writing pages; otherwise the output folder is served as it is (or an empty
    /// folder when there is none) until a build does. [`Session::first_ok`] and
    /// [`Session::first_served`] say which happened.
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
            first_served: false,
            first_problems: Vec::new(),
            served_pages: Vec::new(),
            mirrored: HashMap::new(),
            last_entries: Vec::new(),
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
            copied: HashMap::new(),
            copied_before: None,
            last_changed: Vec::new(),
            live: None,
            tx,
            rx,
            hooks: Vec::new(),
        };
        session.refresh(&opts);
        session.watch_all().map_err(StartError::Watch)?;

        let Built {
            ok,
            problems,
            dir,
            pages,
            entries,
            ..
        } = session.build();
        session.first_ok = ok;
        session.first_served = dir.is_some();
        session.first_problems = problems;
        match dir {
            Some(dir) => {
                session.mirror(&dir, entries);
                session.served_pages = pages;
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

    /// Whether the first build is being served: it got as far as writing pages, though it may
    /// have refused some.
    pub fn first_served(&self) -> bool {
        self.first_served
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

    /// Call `hook` after every rebuild, after the hooks added before it.
    pub fn on_rebuild(&mut self, hook: Arc<RebuildHook>) {
        self.hooks.push(hook);
    }

    /// Inject the live-reload script into `server`'s pages and tell them about every rebuild.
    /// Pages that connect while the first build's errors stand, or while the output folder
    /// could not be written, are told about those.
    pub fn live_reload(&mut self, server: &RunningServer) -> LiveReload {
        let live = LiveReload::attach(server);
        if !self.first_ok {
            live.show_problems(&self.first_problems);
        } else if !self.mirror_problems.is_empty() {
            live.show_problems(&self.mirror_problems);
        }
        let hook = live.clone();
        self.on_rebuild(Arc::new(move |r: &Rebuild| hook.rebuilt(r)));
        self.live = Some(live.clone());
        live
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

    /// End a session without serving it, such as when the server could not start or the
    /// first build failed and the caller won't go on: stop watching and remove the staging
    /// folder. The output folder holds the first build when it was good.
    pub fn close(mut self) -> Finished {
        let mut finished = Finished::default();
        self.watcher = None;
        if self.current.take().is_some() {
            finished.output_written = self.mirror_problems.is_empty();
            finished.problems.append(&mut self.mirror_problems);
        }
        self.cleanup(&mut finished);
        finished
    }

    fn rebuild(&mut self, server: &RunningServer, changed: &[PathBuf]) {
        if !self.settings.quiet {
            for p in changed {
                crate::say!("File changed: {}", shown(p));
            }
        }
        self.last_changed = changed.to_vec();
        let built = self.build();
        let (mut ok, problems) = (built.ok, built.problems);
        let tabs_open = self.live.as_ref().is_some_and(|l| l.clients() > 0);
        let mut pages = Vec::new();
        let mut output_problems = Vec::new();
        let mut served = false;
        let mut kept = 0;
        if let Some(dir) = built.dir {
            let kept_pages = if ok {
                Vec::new()
            } else {
                self.keep_refused(&dir, &built.refused, &built.pages)
            };
            match server.swap_root(&dir) {
                Ok(old) => {
                    served = true;
                    kept = kept_pages.len();
                    if kept > 0 && !self.settings.quiet {
                        let (noun, clause) = if kept == 1 {
                            ("page", "its template has")
                        } else {
                            ("pages", "their templates have")
                        };
                        crate::say!("Kept {kept} {noun} from the last build, as {clause} errors");
                    }
                    self.retire(old);
                    self.mirror(&dir, built.entries);
                    output_problems = self.mirror_problems.clone();
                    // The changed templates' pages, for a page showing one to patch itself.
                    for p in built.pages.iter().filter(|_| tabs_open) {
                        if changed.contains(&p.source)
                            && let Ok(content) = fs::read_to_string(dir.join(&p.output))
                        {
                            pages.push(ChangedPage {
                                url: p.url.clone(),
                                content,
                            });
                        }
                    }
                    let mut all = built.pages.clone();
                    all.extend(kept_pages);
                    self.served_pages = all;
                    self.current = Some(dir);
                }
                Err(e) => {
                    crate::say_err!("Brrrty could not serve the new build: {e}");
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
        let stylesheets = if served {
            built
                .passthrough
                .iter()
                .filter(|(src, _)| {
                    src.extension().is_some_and(|e| e == "css") && changed.contains(src)
                })
                .map(|(_, dest)| url_of(dest))
                .collect()
        } else {
            Vec::new()
        };
        let rebuild = Rebuild {
            ok,
            served,
            refused: built.refused,
            kept,
            changed_paths: changed.to_vec(),
            css_only,
            problems,
            pages,
            stylesheets,
            output_problems,
        };
        for hook in &self.hooks {
            hook(&rebuild);
        }
    }

    fn new_build_dir(&mut self) -> PathBuf {
        self.next_build += 1;
        self.staging.join(format!("build-{}", self.next_build))
    }

    /// Build into a fresh folder. Gives whether it had no errors, its problems, and the folder
    /// when it did; a failed build's folder is removed at once.
    fn build(&mut self) -> Built {
        let color = self.settings.color;
        let failed = |problems| Built {
            ok: false,
            problems,
            dir: None,
            pages: Vec::new(),
            passthrough: Vec::new(),
            refused: Vec::new(),
            entries: Vec::new(),
        };
        let mut opts = match build_options(&self.root, &self.flags) {
            Ok(o) => o,
            Err(problems) => {
                report::print_problems(&problems, &self.root, color);
                return failed(problems);
            }
        };
        self.refresh(&opts);
        let now = stamps(&self.passthrough);
        let first = self.next_build == 0;
        let before = std::mem::replace(&mut self.copied, now);
        self.copied_before = (!first).then_some(before);
        let dir = self.new_build_dir();
        // The user (or a `brrrty build`) may have removed the output folder meanwhile.
        let _ = fs::create_dir_all(&self.staging);
        if let Err(e) = fs::create_dir_all(&dir) {
            let p = Problem::error(
                brrrty_core::codes::WRITE_FAILED,
                format!("cannot make the build folder {}: {e}", dir.display()),
            );
            report::print_problems(std::slice::from_ref(&p), &self.root, color);
            return failed(vec![p]);
        }
        opts.write_to = Some(dir.clone());
        opts.run_mode = RunMode::Serve;
        let report = brrrty_core::build(&opts, &MarkdownIt::new());
        report::print_build(&report, &self.root, self.settings.quiet, color);
        let ok = !report.has_errors();
        // A build that stopped before writing pages is never served, nor under `--strict` one
        // with any error.
        let served = report.complete && (ok || !self.flags.strict);
        if !served {
            let _ = fs::remove_dir_all(&dir);
        }
        let entries = report
            .pages
            .iter()
            .map(|p| Entry {
                rel: p.output.clone(),
                version: Version::Page(p.hash),
            })
            .chain(report.passthrough.iter().map(|(src, dest)| {
                let stamp = stamp_of(&self.copied, &self.root.join(src)).cloned();
                Entry {
                    rel: dest.clone(),
                    version: Version::Copy(stamp),
                }
            }))
            .collect();
        Built {
            ok,
            problems: report.problems,
            dir: served.then_some(dir),
            pages: report.pages,
            passthrough: report.passthrough,
            refused: report.refused,
            entries,
        }
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
                    crate::say_err!("Brrrty's file watcher reported a problem: {message}");
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
            let echo = {
                let rel = p.strip_prefix(&self.root).unwrap_or(p);
                is_echo(
                    Stamp::of(p).as_ref(),
                    self.copied.get(p),
                    self.copied_before.as_ref().map(|b| b.get(p)),
                    self.last_changed.iter().any(|c| c == rel),
                )
            };
            if !echo && self.counts(p) {
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
                let entries = std::mem::take(&mut self.last_entries);
                self.mirror(&cur, entries);
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

    /// Give each refused page that the build being served had the file it had there, in the
    /// new build `dir`, unless the new build wrote something at that path. Gives the pages
    /// kept.
    fn keep_refused(
        &mut self,
        dir: &Path,
        refused: &[PathBuf],
        written: &[brrrty_core::WrittenPage],
    ) -> Vec<brrrty_core::WrittenPage> {
        let Some(cur) = &self.current else {
            return Vec::new();
        };
        let mut kept = Vec::new();
        for page in &self.served_pages {
            if !refused.contains(&page.source) || written.iter().any(|w| w.output == page.output) {
                continue;
            }
            let to = dir.join(&page.output);
            if fs::symlink_metadata(&to).is_ok() {
                // A passthrough file took the path.
                continue;
            }
            let from = cur.join(&page.output);
            let copied = (|| {
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent)?;
                }
                // Build folders are never written to once made, so a link is as good as a copy.
                fs::hard_link(&from, &to).or_else(|_| fs::copy(&from, &to).map(drop))
            })();
            if copied.is_ok() {
                kept.push(page.clone());
            }
        }
        kept
    }

    /// Bring the output folder up to date with the files `entries` lists in the build in
    /// `build`, printing what went wrong. A file the session put there itself, still as it
    /// left it, and of the same version, is not looked at again.
    fn mirror(&mut self, build: &Path, entries: Vec<Entry>) {
        let mut problems = Vec::new();
        let mut problem = |message: String| {
            problems.push(Problem::warning(brrrty_core::codes::WRITE_FAILED, message));
        };
        if entries.iter().any(|e| {
            e.rel.components().next() == Some(std::path::Component::Normal(STAGING_DIR.as_ref()))
        }) {
            // `<output>/.brrrty-serve` is the staging folder: the site's own file by that
            // name is left out, and must never be shipped.
            problem(format!(
                "the site wrote {STAGING_DIR}, which is Brrrty's staging folder, so it was left out of {}",
                self.output.display()
            ));
        }
        for e in &entries {
            if e.rel.components().next() == Some(std::path::Component::Normal(STAGING_DIR.as_ref()))
            {
                continue;
            }
            let src = build.join(&e.rel);
            let dst = self.output.join(&e.rel);
            let existing = fs::symlink_metadata(&dst).ok();
            let as_left = |r: &Record, m: &fs::Metadata| {
                m.is_file() && r.len == m.len() && r.modified == m.modified().ok()
            };
            match (self.mirrored.get(&e.rel), &existing) {
                (Some(r), Some(m)) if as_left(r, m) && r.version == e.version => continue,
                (Some(r), Some(m)) if as_left(r, m) => {}
                // Not known: compare once, so unchanged files keep their times.
                (_, Some(m)) if m.is_file() && same_bytes(&src, &dst) => {
                    self.record(e, &dst);
                    continue;
                }
                _ => {}
            }
            self.next_tmp += 1;
            let tmp = self.staging.join(format!("copy-{}", self.next_tmp));
            let copied = (|| {
                make_parents(&self.output, &e.rel)?;
                fs::copy(&src, &tmp)?;
                if existing.as_ref().is_some_and(|m| m.is_dir()) {
                    fs::remove_dir_all(&dst)?;
                }
                fs::rename(&tmp, &dst)
            })();
            match copied {
                Ok(()) => self.record(e, &dst),
                Err(err) => {
                    let _ = fs::remove_file(&tmp);
                    self.mirrored.remove(&e.rel);
                    problem(format!("cannot write {}: {err}", dst.display()));
                }
            }
        }
        self.last_entries = entries;
        self.mirror_problems = problems;
        if !self.mirror_problems.is_empty() {
            report::print_problems(&self.mirror_problems, &self.root, self.settings.color);
        }
    }

    fn record(&mut self, e: &Entry, dst: &Path) {
        match fs::symlink_metadata(dst) {
            Ok(m) => {
                self.mirrored.insert(
                    e.rel.clone(),
                    Record {
                        version: e.version.clone(),
                        len: m.len(),
                        modified: m.modified().ok(),
                    },
                );
            }
            Err(_) => {
                self.mirrored.remove(&e.rel);
            }
        }
    }
}

/// What identifies a file's version without reading it: its size and times, and on Unix its
/// inode and change time, which any write, rename or `touch` moves.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64, u64),
}

impl Stamp {
    fn of(p: &Path) -> Option<Stamp> {
        let m = fs::metadata(p).ok().filter(|m| m.is_file())?;
        #[cfg(unix)]
        let changed = {
            use std::os::unix::fs::MetadataExt;
            (m.ctime(), m.ctime_nsec(), m.ino())
        };
        Some(Stamp {
            len: m.len(),
            modified: m.modified().ok(),
            #[cfg(unix)]
            changed,
        })
    }
}

/// Whether an event for a passthrough file is the echo of the last build copying it: the file
/// looks as it did just before that build (`now` and `at_build`), and either it looked the
/// same before the build before (`before`; `None` when there was none) or its change was
/// among the files that started the last build (`announced`).
fn is_echo(
    now: Option<&Stamp>,
    at_build: Option<&Stamp>,
    before: Option<Option<&Stamp>>,
    announced: bool,
) -> bool {
    let Some(at_build) = at_build else {
        return false;
    };
    if now != Some(at_build) {
        return false;
    }
    match before {
        None => true,
        Some(b) => b == Some(at_build) || announced,
    }
}

/// The stamps of every file at or under `paths`. Symlinked folders are not followed, as the
/// passthrough copy doesn't follow them, so a link back up the tree can't loop; a symlinked
/// file is stamped by its target.
fn stamps(paths: &[PathBuf]) -> HashMap<PathBuf, Stamp> {
    let mut out = HashMap::new();
    let mut todo: Vec<PathBuf> = paths.to_vec();
    while let Some(p) = todo.pop() {
        let Ok(meta) = fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.is_dir() {
            if let Ok(entries) = fs::read_dir(&p) {
                todo.extend(entries.flatten().map(|e| e.path()));
            }
        } else if let Some(s) = Stamp::of(&p) {
            // A file, or a symlink to one; a symlink to a folder has no stamp.
            out.insert(p, s);
        }
    }
    out
}

/// The stamp `stamps` took for the passthrough file at `src`. `stamps` walks the resolved
/// passthrough roots, so its keys sit under real folders: look `src` up as written, then with
/// its folder resolved, then resolved whole (a passthrough entry that is a symlink to a file).
fn stamp_of<'a>(stamps: &'a HashMap<PathBuf, Stamp>, src: &Path) -> Option<&'a Stamp> {
    if let Some(s) = stamps.get(src) {
        return Some(s);
    }
    let in_real_dir = src
        .parent()
        .zip(src.file_name())
        .and_then(|(dir, name)| dir.canonicalize().ok().map(|d| d.join(name)));
    in_real_dir
        .and_then(|p| stamps.get(&p))
        .or_else(|| src.canonicalize().ok().and_then(|p| stamps.get(&p)))
}

/// A path relative to the project folder, for printing as 11ty does (`./src/index.md`). The
/// project folder itself is `.` (the watcher lost track of an input folder that is the
/// project folder). A path outside the project folder is printed whole.
fn shown(rel: &Path) -> String {
    if rel.as_os_str().is_empty() {
        ".".to_string()
    } else if rel.is_absolute() {
        rel.display().to_string()
    } else {
        format!("./{}", rel.display())
    }
}

/// The URL of a file at `rel` in the output folder.
fn url_of(rel: &Path) -> String {
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    format!("/{}", parts.join("/"))
}

fn remove_any(p: &Path) -> io::Result<()> {
    if fs::symlink_metadata(p)?.is_dir() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

/// Make the folders `rel` goes in, under `output`. A file or symlink where a folder belongs
/// is replaced, never followed, so nothing lands outside `output`.
fn make_parents(output: &Path, rel: &Path) -> io::Result<()> {
    let mut at = output.to_path_buf();
    let Some(parent) = rel.parent() else {
        return Ok(());
    };
    for c in parent.components() {
        at.push(c);
        match fs::symlink_metadata(&at) {
            Ok(m) if m.is_dir() => continue,
            Ok(_) => {
                fs::remove_file(&at)?;
                fs::create_dir(&at)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir(&at)?,
            Err(e) => return Err(e),
        }
    }
    Ok(())
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

#[cfg(test)]
mod echo_tests {
    use super::*;

    fn stamp(len: u64) -> Stamp {
        Stamp {
            len,
            modified: None,
            #[cfg(unix)]
            changed: (0, 0, 0),
        }
    }

    #[test]
    fn changed_paths_print_as_11ty_prints_them() {
        assert_eq!(shown(Path::new("src/index.md")), "./src/index.md");
        // The input folder is the project folder and the watcher lost track of it.
        assert_eq!(shown(Path::new("")), ".");
        // A passthrough source outside the project folder.
        assert_eq!(shown(Path::new("/elsewhere/x.css")), "/elsewhere/x.css");
    }

    #[test]
    fn echoes_and_edits_are_told_apart() {
        let (a, b) = (stamp(1), stamp(2));
        // After the first build: unchanged is an echo, changed or gone is an edit.
        assert!(is_echo(Some(&a), Some(&a), None, false));
        assert!(!is_echo(Some(&b), Some(&a), None, false));
        assert!(!is_echo(None, Some(&a), None, false));
        // A file that isn't a passthrough file is never an echo.
        assert!(!is_echo(Some(&a), None, None, false));
        // Unchanged across two builds: an echo.
        assert!(is_echo(Some(&a), Some(&a), Some(Some(&a)), false));
        // Changed between the two builds and announced by the last one: an echo.
        assert!(is_echo(Some(&b), Some(&b), Some(Some(&a)), true));
        // Changed between them but not announced: edited after the debounce fired and
        // before the stamps were taken, so the event is the edit's own.
        assert!(!is_echo(Some(&b), Some(&b), Some(Some(&a)), false));
        assert!(!is_echo(Some(&b), Some(&b), Some(None), false));
    }
}
