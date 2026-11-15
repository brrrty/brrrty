//! Copying passthrough files and folders into the output folder.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::options::BuildOptions;
use crate::problem::Problem;

/// Copy every `opts.passthrough` entry into the output folder.
///
/// Returns `(copied, unchanged, problems)`. A file is `unchanged` when the destination
/// already has the same length and a modification time at least as new as the source.
pub fn copy_passthrough(opts: &BuildOptions) -> (usize, usize, Vec<Problem>) {
    copy_passthrough_avoiding(opts, &HashMap::new())
}

/// Like [`copy_passthrough`], but lets passthrough files override any destination in `pages`
/// (absolute output path -> template input path): the file wins and a
/// `passthrough-overrides-page` warning is reported.
pub(crate) fn copy_passthrough_avoiding(
    opts: &BuildOptions,
    pages: &HashMap<PathBuf, PathBuf>,
) -> (usize, usize, Vec<Problem>) {
    let root = opts.root_dir();
    let input = opts.input_dir();
    let out = opts.output_dir();
    let (mut copied, mut unchanged, mut problems) = (0, 0, Vec::new());

    for entry in &opts.passthrough {
        let escapes = entry.is_absolute()
            || entry.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::Prefix(_) | Component::RootDir
                )
            });
        if escapes {
            problems.push(
                Problem::error(
                    "passthrough-escapes",
                    format!(
                        "passthrough path {} must be relative to the project and stay inside it",
                        entry.display()
                    ),
                )
                .with_path(entry)
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
            problems.push(
                Problem::warning(
                    "passthrough-missing",
                    format!("passthrough path {} does not exist", entry.display()),
                )
                .with_path(entry),
            );
            continue;
        }
        let dest = match source.strip_prefix(&input) {
            Ok(rel) => out.join(rel),
            Err(_) => out.join(&clean),
        };
        if dest == out {
            problems.push(
                Problem::error(
                    "passthrough-is-input",
                    format!(
                        "passthrough path {} is the input or project folder itself",
                        entry.display()
                    ),
                )
                .with_path(entry)
                .with_help("Pass through specific folders inside it instead (like src/images)."),
            );
            continue;
        }
        if !crate::build::is_inside(&out, &dest) {
            problems.push(
                Problem::error(
                    "output-escapes",
                    format!(
                        "{} would be written outside the output folder",
                        dest.display()
                    ),
                )
                .with_path(entry),
            );
            continue;
        }
        copy_entry(
            &source,
            &dest,
            &out,
            pages,
            &mut copied,
            &mut unchanged,
            &mut problems,
        );
    }
    (copied, unchanged, problems)
}

fn copy_entry(
    source: &Path,
    dest: &Path,
    out: &Path,
    pages: &HashMap<PathBuf, PathBuf>,
    copied: &mut usize,
    unchanged: &mut usize,
    problems: &mut Vec<Problem>,
) {
    // Never copy the output folder into itself.
    if source == out {
        return;
    }
    let meta = match fs::metadata(source) {
        Ok(m) => m,
        Err(e) => {
            problems.push(
                Problem::error(
                    "passthrough-copy-failed",
                    format!("cannot read passthrough source: {e}"),
                )
                .with_path(source),
            );
            return;
        }
    };
    if meta.is_dir() {
        let entries = match fs::read_dir(source) {
            Ok(e) => e,
            Err(e) => {
                problems.push(
                    Problem::error(
                        "passthrough-copy-failed",
                        format!("cannot read folder: {e}"),
                    )
                    .with_path(source),
                );
                return;
            }
        };
        let mut children = Vec::new();
        for entry in entries {
            match entry {
                Ok(c) => children.push(c),
                Err(e) => problems.push(
                    Problem::error(
                        "passthrough-copy-failed",
                        format!("cannot read folder entry: {e}"),
                    )
                    .with_path(source),
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
            copy_entry(
                &path,
                &dest.join(child.file_name()),
                out,
                pages,
                copied,
                unchanged,
                problems,
            );
        }
        return;
    }
    let overrides_page = pages.get(dest);
    if let Some(page) = overrides_page {
        problems.push(
            Problem::warning(
                "passthrough-overrides-page",
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
    if overrides_page.is_none() && is_up_to_date(&meta, dest) {
        *unchanged += 1;
        return;
    }
    let result = dest
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::copy(source, dest).map(|_| ()));
    match result {
        Ok(()) => *copied += 1,
        Err(e) => problems.push(
            Problem::error("passthrough-copy-failed", format!("cannot copy file: {e}"))
                .with_path(source),
        ),
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
