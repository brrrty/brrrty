//! Copying passthrough files and folders into the output folder.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use rayon::prelude::*;

use crate::options::{BuildOptions, root_relative};
use crate::problem::{Problem, codes};

/// Copy every `opts.passthrough` entry into the output folder.
///
/// Returns `(copied, unchanged, problems)`. A file is `unchanged` when the destination
/// already has the same length and a modification time at least as new as the source.
pub fn copy_passthrough(opts: &BuildOptions) -> (usize, usize, Vec<Problem>) {
    let result = copy_passthrough_avoiding(opts, &HashMap::new(), false);
    (result.copied, result.unchanged, result.problems)
}

/// The outcome of copying passthrough entries next to rendered pages.
pub(crate) struct PassthroughResult {
    pub copied: usize,
    pub unchanged: usize,
    /// Rendered-page output paths that a passthrough file replaced.
    pub overridden: HashSet<PathBuf>,
    /// Each file now in place: its source relative to the project folder, and its
    /// destination relative to the folder written to.
    pub files: Vec<(PathBuf, PathBuf)>,
    pub problems: Vec<Problem>,
}

/// Running state while walking passthrough entries.
struct Copier<'a> {
    root: &'a Path,
    /// Where files are written.
    out: &'a Path,
    /// The output folder, which is `out` unless the build writes elsewhere.
    output: &'a Path,
    pages: &'a HashMap<PathBuf, PathBuf>,
    /// Every `(source file, destination)` pair found while walking, in walk order.
    plan: Vec<(PathBuf, PathBuf)>,
    copied: usize,
    unchanged: usize,
    overridden: HashSet<PathBuf>,
    files: Vec<(PathBuf, PathBuf)>,
    problems: Vec<Problem>,
}

/// Like [`copy_passthrough`], but lets passthrough files override any destination in `pages`
/// (absolute output path -> template input path): the file wins and a
/// `passthrough-overrides-page` warning is reported.
///
/// With `dry_run`, the entries are checked and walked, and the files that would be copied are
/// counted, but nothing is copied.
pub(crate) fn copy_passthrough_avoiding(
    opts: &BuildOptions,
    pages: &HashMap<PathBuf, PathBuf>,
    dry_run: bool,
) -> PassthroughResult {
    let dirs = (|| {
        Ok::<_, Problem>((
            opts.root_dir()?,
            opts.input_dir()?,
            opts.write_dir()?,
            opts.output_dir()?,
        ))
    })();
    let (root, input, out, output) = match dirs {
        Ok(d) => d,
        Err(p) => {
            return PassthroughResult {
                copied: 0,
                unchanged: 0,
                overridden: HashSet::new(),
                files: Vec::new(),
                problems: vec![p],
            };
        }
    };
    let mut c = Copier {
        root: &root,
        out: &out,
        output: &output,
        pages,
        plan: Vec::new(),
        copied: 0,
        unchanged: 0,
        overridden: HashSet::new(),
        files: Vec::new(),
        problems: Vec::new(),
    };

    let entries = opts
        .config
        .passthrough
        .iter()
        .map(|e| (e.from.as_str().into(), e.to.as_deref(), true))
        .chain(opts.passthrough.iter().map(|p| (p.clone(), None, false)));
    for (entry, to, from_config) in entries {
        let entry: PathBuf = entry;
        if from_config && is_glob(&entry.to_string_lossy()) {
            c.problems.push(
                Problem::error(
                    codes::PASSTHROUGH_UNSUPPORTED,
                    format!(
                        "passthrough path {} is a glob, which Brrrty doesn't copy yet",
                        entry.display()
                    ),
                )
                .with_path(config_path(opts, &entry))
                .with_help("name the files or folders to copy, one entry each"),
            );
            continue;
        }
        let escapes = entry.is_absolute()
            || entry.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::Prefix(_) | Component::RootDir
                )
            });
        if escapes {
            c.problems.push(
                Problem::error(
                    codes::PASSTHROUGH_ESCAPES,
                    format!(
                        "passthrough path {} must be relative to the project and stay inside it",
                        entry.display()
                    ),
                )
                .with_path(&entry)
                .with_help("remove `..` segments and absolute paths from passthrough entries"),
            );
            continue;
        }
        let clean: PathBuf = entry
            .components()
            .filter(|c| !matches!(c, Component::CurDir))
            .collect();
        let source = root.join(&clean);
        if !source.exists() {
            c.problems.push(
                Problem::warning(
                    codes::PASSTHROUGH_MISSING,
                    format!("passthrough path {} does not exist", entry.display()),
                )
                .with_path(&clean),
            );
            continue;
        }
        // 11ty treats an empty target like no target.
        let mapped = to.filter(|t| !t.is_empty());
        let dest = match mapped {
            // A target is relative to the output folder, with any leading
            // slash dropped; `/` is the output folder itself.
            Some(t) => crate::options::normalize(&out.join(t.trim_start_matches('/'))),
            None => match source.strip_prefix(&input) {
                Ok(rel) => out.join(rel),
                Err(_) => out.join(&clean),
            },
        };
        if dest == out && (mapped.is_none() || !source.is_dir()) {
            let p = if mapped.is_none() {
                Problem::error(
                    codes::PASSTHROUGH_IS_INPUT,
                    format!(
                        "passthrough path {} is the input or project folder itself",
                        entry.display()
                    ),
                )
                .with_help("Pass through specific folders inside it instead (like src/images).")
            } else {
                Problem::error(
                    codes::PASSTHROUGH_UNSUPPORTED,
                    format!(
                        "passthrough file {} would be copied onto the output folder itself",
                        entry.display()
                    ),
                )
                .with_help("give the file a name in the output folder, like `favicon.ico`")
            };
            c.problems.push(p.with_path(&clean));
            continue;
        }
        if dest != out && !crate::build::is_inside(&out, &dest) {
            c.problems.push(
                Problem::error(
                    codes::OUTPUT_ESCAPES,
                    format!(
                        "{} would be written outside the output folder",
                        dest.display()
                    ),
                )
                .with_path(&clean),
            );
            continue;
        }
        collect_entry(&source, &dest, &mut c);
    }
    if dry_run {
        let dests: HashSet<&PathBuf> = c.plan.iter().map(|(_, d)| d).collect();
        c.copied = dests.len();
        // A file that would replace a page means the page isn't really written. Whether it
        // is worth a warning depends on the bytes the page would have, so none is given.
        c.overridden = dests
            .into_iter()
            .filter(|d| c.pages.contains_key(*d))
            .cloned()
            .collect();
    } else {
        copy_planned(&mut c);
    }
    PassthroughResult {
        copied: c.copied,
        unchanged: c.unchanged,
        overridden: c.overridden,
        files: c.files,
        problems: c.problems,
    }
}

/// A path 11ty would read as a glob rather than a file name.
fn is_glob(path: &str) -> bool {
    path.contains(['*', '?', '[', ']', '{', '}', '!'])
}

/// Where to point a problem with a config entry: the config file when there
/// is one.
fn config_path(opts: &BuildOptions, entry: &Path) -> PathBuf {
    opts.config
        .file
        .clone()
        .unwrap_or_else(|| entry.to_path_buf())
}

fn collect_entry(source: &Path, dest: &Path, c: &mut Copier) {
    // Never copy the output folder into itself.
    if source == c.out || source == c.output {
        return;
    }
    let meta = match fs::metadata(source) {
        Ok(m) => m,
        Err(e) => {
            c.problems.push(
                Problem::error(
                    codes::PASSTHROUGH_COPY_FAILED,
                    format!("cannot read passthrough source: {e}"),
                )
                .with_path(root_relative(c.root, source)),
            );
            return;
        }
    };
    if meta.is_dir() {
        let entries = match fs::read_dir(source) {
            Ok(e) => e,
            Err(e) => {
                c.problems.push(
                    Problem::error(
                        codes::PASSTHROUGH_COPY_FAILED,
                        format!("cannot read folder: {e}"),
                    )
                    .with_path(root_relative(c.root, source)),
                );
                return;
            }
        };
        let mut children = Vec::new();
        for entry in entries {
            match entry {
                Ok(c) => children.push(c),
                Err(e) => c.problems.push(
                    Problem::error(
                        codes::PASSTHROUGH_COPY_FAILED,
                        format!("cannot read folder entry: {e}"),
                    )
                    .with_path(root_relative(c.root, source)),
                ),
            }
        }
        children.sort_by_key(|c| c.file_name());
        for child in children {
            let path = child.path();
            // Skip symlinked folders to avoid cycles.
            if child.file_type().is_ok_and(|t| t.is_symlink()) && path.is_dir() {
                continue;
            }
            collect_entry(&path, &dest.join(child.file_name()), c);
        }
        return;
    }
    c.plan.push((source.to_path_buf(), dest.to_path_buf()));
}

/// Copy the collected files. Destinations claimed by two different sources are reported once
/// and skipped, so the result never depends on entry order; everything else is still copied.
fn copy_planned(c: &mut Copier) {
    let mut sources: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for (source, dest) in &c.plan {
        let list = sources.entry(dest.clone()).or_default();
        if !list.contains(source) {
            list.push(source.clone());
        }
    }
    let plan = std::mem::take(&mut c.plan);
    let mut done: HashSet<PathBuf> = HashSet::new();
    let mut jobs: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (source, dest) in plan {
        if !done.insert(dest.clone()) {
            continue;
        }
        let srcs = &sources[&dest];
        if srcs.len() > 1 {
            let names: Vec<String> = srcs
                .iter()
                .map(|s| root_relative(c.root, s).display().to_string())
                .collect();
            c.problems.push(
                Problem::error(
                    codes::PASSTHROUGH_COLLISION,
                    format!(
                        "passthrough files {} both write to {}",
                        names.join(" and "),
                        root_relative(c.root, &dest).display()
                    ),
                )
                .with_path(root_relative(c.root, &srcs[0]))
                .with_help("remove one of the passthrough entries, or rename one of the files"),
            );
            continue;
        }
        jobs.push((source, dest));
    }
    // Jobs whose destinations touch (one inside another, or the same file to
    // the file system) depend on which runs first: they run one at a time, in
    // walk order, as every job once did. The rest run in parallel. Results are
    // folded in walk order to keep the report stable.
    let (root, out, pages) = (c.root, c.out, c.pages);
    let tangled = tangled_jobs(&jobs);
    let mut outcomes: Vec<Option<Copied>> = jobs
        .iter()
        .zip(&tangled)
        .map(|((source, dest), &t)| t.then(|| copy_file(source, dest, root, out, pages)))
        .collect();
    let free: Vec<(usize, Copied)> = crate::io_threads::install(|| {
        jobs.par_iter()
            .enumerate()
            .filter(|&(i, _)| !tangled[i])
            .map(|(i, (source, dest))| (i, copy_file(source, dest, root, out, pages)))
            .collect()
    });
    for (i, outcome) in free {
        outcomes[i] = Some(outcome);
    }
    let outcomes = outcomes.into_iter().flatten();
    for outcome in outcomes {
        if outcome.overrides_page {
            c.overridden.insert(outcome.dest);
        }
        c.problems.extend(outcome.problems);
        match outcome.placed {
            Some((placed, true)) => {
                c.copied += 1;
                c.files.push(placed);
            }
            Some((placed, false)) => {
                c.unchanged += 1;
                c.files.push(placed);
            }
            None => {}
        }
    }
}

/// For each job, whether its destination is the same file as another job's
/// (to the file system) or lies inside or around another job's destination.
fn tangled_jobs(jobs: &[(PathBuf, PathBuf)]) -> Vec<bool> {
    let keys: Vec<Vec<String>> = jobs.iter().map(|(_, dest)| dest_key(dest)).collect();
    let mut by_key: HashMap<&[String], Vec<usize>> = HashMap::new();
    for (i, key) in keys.iter().enumerate() {
        by_key.entry(key.as_slice()).or_default().push(i);
    }
    let mut tangled = vec![false; jobs.len()];
    for same in by_key.values().filter(|v| v.len() > 1) {
        for &i in same {
            tangled[i] = true;
        }
    }
    for (i, key) in keys.iter().enumerate() {
        for end in 1..key.len() {
            if let Some(outer) = by_key.get(&key[..end]) {
                tangled[i] = true;
                for &j in outer {
                    tangled[j] = true;
                }
            }
        }
    }
    tangled
}

/// A destination as the file system compares it, one entry per component.
/// APFS, the macOS default, ignores case and Unicode normalization.
fn dest_key(dest: &Path) -> Vec<String> {
    dest.components()
        .map(|c| fold(&c.as_os_str().to_string_lossy()))
        .collect()
}

#[cfg(target_os = "macos")]
fn fold(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let lower: String = name.nfd().collect::<String>().to_lowercase();
    lower.nfd().collect()
}

#[cfg(not(target_os = "macos"))]
fn fold(name: &str) -> String {
    name.to_string()
}

/// What happened to one passthrough file.
struct Copied {
    dest: PathBuf,
    overrides_page: bool,
    /// The file's `(source, destination)` as reported, and whether it was
    /// copied (rather than already up to date); `None` if it failed.
    placed: Option<((PathBuf, PathBuf), bool)>,
    problems: Vec<Problem>,
}

fn copy_file(
    source: &Path,
    dest: &Path,
    root: &Path,
    out: &Path,
    pages: &HashMap<PathBuf, PathBuf>,
) -> Copied {
    let mut done = Copied {
        dest: dest.to_path_buf(),
        overrides_page: false,
        placed: None,
        problems: Vec::new(),
    };
    let meta = match fs::metadata(source) {
        Ok(m) => m,
        Err(e) => {
            done.problems.push(
                Problem::error(
                    codes::PASSTHROUGH_COPY_FAILED,
                    format!("cannot read passthrough source: {e}"),
                )
                .with_path(root_relative(root, source)),
            );
            return done;
        }
    };
    let overrides_page = pages.get(dest);
    if let Some(page) = overrides_page {
        done.overrides_page = true;
        // The same bytes landing on the same path is not worth a warning.
        let identical = matches!(
            (fs::read(source), fs::read(dest)),
            (Ok(a), Ok(b)) if a == b
        );
        if !identical {
            done.problems.push(
            Problem::warning(
                codes::PASSTHROUGH_OVERRIDES_PAGE,
                format!(
                    "passthrough file {} replaces the output of template {}",
                    source.display(),
                    page.display()
                ),
            )
            .with_path(page)
            .with_help(
                "11ty copies passthrough files over rendered pages. Move the file out of the passthrough folder if you meant it to be a template.",
            ),
            );
        }
    }
    let placed = (
        root_relative(root, source),
        dest.strip_prefix(out).unwrap_or(dest).to_path_buf(),
    );
    if overrides_page.is_none() && is_up_to_date(&meta, dest) {
        done.placed = Some((placed, false));
        return done;
    }
    let result = dest
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::copy(source, dest).map(|_| ()));
    match result {
        Ok(()) => {
            keep_times(&meta, dest);
            done.placed = Some((placed, true));
        }
        Err(e) => done.problems.push(
            Problem::error(
                codes::PASSTHROUGH_COPY_FAILED,
                format!("cannot copy file: {e}"),
            )
            .with_path(root_relative(root, source)),
        ),
    }
    done
}

/// Give a copy its source's access and modification times, as 11ty's copy
/// does (`fs.utimes` after writing), so tools that compare times, like rsync,
/// see an unchanged file as unchanged. An APFS clone already has them.
/// Like 11ty, a failure here is ignored: the bytes are in place.
fn keep_times(source: &fs::Metadata, dest: &Path) {
    let (Ok(accessed), Ok(modified)) = (source.accessed(), source.modified()) else {
        return;
    };
    if cfg!(target_os = "macos")
        && fs::metadata(dest)
            .and_then(|d| d.modified())
            .is_ok_and(|m| m == modified)
    {
        return;
    }
    // Read-only is enough to set the times of a file one owns, and works for
    // a copy of a read-only source.
    if let Ok(f) = fs::File::open(dest) {
        let _ = f.set_times(
            fs::FileTimes::new()
                .set_accessed(accessed)
                .set_modified(modified),
        );
    }
}

fn is_up_to_date(source: &fs::Metadata, dest: &Path) -> bool {
    let Ok(d) = fs::metadata(dest) else {
        return false;
    };
    match (source.modified(), d.modified()) {
        (Ok(s), Ok(d_time)) => d.len() == source.len() && d_time >= s,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jobs(dests: &[&str]) -> Vec<(PathBuf, PathBuf)> {
        dests
            .iter()
            .map(|d| (PathBuf::from("src"), PathBuf::from("/out").join(d)))
            .collect()
    }

    #[test]
    fn a_file_and_what_lies_under_it_are_tangled() {
        let t = tangled_jobs(&jobs(&["x", "x/a", "x/b/c", "y/a", "xy"]));
        assert_eq!(t, [true, true, true, false, false]);
    }

    #[test]
    fn unrelated_destinations_are_free() {
        let t = tangled_jobs(&jobs(&["a/1", "a/2", "b/1"]));
        assert_eq!(t, [false; 3]);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn names_apfs_treats_as_one_are_tangled_on_macos() {
        let t = tangled_jobs(&jobs(&[
            "y/Name.bin",
            "y/name.bin",
            "caf\u{e9}",
            "cafe\u{301}",
            "z",
        ]));
        assert_eq!(t, [true, true, true, true, false]);
        let t = tangled_jobs(&jobs(&["D", "d/f"]));
        assert_eq!(t, [true, true]);
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn names_differing_by_case_are_free_elsewhere() {
        let t = tangled_jobs(&jobs(&["y/Name.bin", "y/name.bin"]));
        assert_eq!(t, [false, false]);
    }
}
