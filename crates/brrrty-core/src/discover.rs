use crate::ignore_files::IgnoreRules;
use crate::options::{BuildOptions, root_relative};
use crate::problem::{Problem, codes};
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const TEMPLATE_EXTS: &[&str] = &[".md", ".html"];
const UNSUPPORTED_EXTS: &[&str] = &[".liquid", ".njk", ".11ty.js", ".11ty.cjs", ".11ty.mjs"];
/// Template types 11ty only picks up when a plugin registers them.
pub const PLUGIN_ONLY_EXTS: &[&str] = &[
    ".webc",
    ".hbs",
    ".mustache",
    ".ejs",
    ".haml",
    ".pug",
    ".jsx",
    ".tsx",
    ".mdx",
];

/// Templates found under the input dir. Paths are relative to the input dir and sorted.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub templates: Vec<PathBuf>,
    pub unsupported: Vec<PathBuf>,
    /// Files whose type 11ty only treats as a template when a plugin adds it, sorted.
    pub plugin_only: Vec<PathBuf>,
    /// Warnings found while discovering, such as ignore lines that don't do what gitignore would.
    pub warnings: Vec<Problem>,
}

/// Find input templates, following 11ty's default ignore rules.
pub fn discover(opts: &BuildOptions) -> Result<Discovery, Problem> {
    let root = opts.root_dir()?;
    let input = opts.input_dir()?;
    let output = opts.output_dir()?;
    let input_rel = opts.input_rel()?;

    let rules = IgnoreRules::load(&root, &input, &input_rel)?;
    let mut found = Discovery {
        warnings: rules.warnings.clone(),
        ..Discovery::default()
    };
    let rules = Arc::new(rules);

    let includes = input.join("_includes");
    let data = input.join("_data");

    let out_dir = output.clone();
    let filter_rules = Arc::clone(&rules);
    let walker = WalkBuilder::new(&input)
        .standard_filters(false)
        .hidden(false)
        .follow_links(true)
        .filter_entry(move |e| {
            let p = e.path();
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            if is_dir && (p == output || p == includes || p == data) {
                return false;
            }
            if e.file_name() == "node_modules" {
                return false;
            }
            !filter_rules.is_ignored(p, is_dir)
        })
        .build();

    // Paths in problems are relative to the root, through the input folder's own spelling.
    let shown = |p: &std::path::Path| rules.root_rel(p).unwrap_or_else(|| root_relative(&root, p));
    for entry in walker {
        let entry = match entry {
            Err(e) => {
                if let Some(link) = loop_child(&e) {
                    found.warnings.push(
                        Problem::warning(
                            codes::SYMLINK_LOOP,
                            "this link points back into a folder that contains it, so it was skipped",
                        )
                        .with_path(shown(link))
                        .with_help("Remove the link or point it outside its own folder."),
                    );
                    continue;
                }
                if let Some(link) = broken_link(&e) {
                    let target = std::fs::read_link(link)
                        .map(|t| t.display().to_string())
                        .unwrap_or_else(|_| "a missing file".to_string());
                    found.warnings.push(
                        Problem::warning(
                            codes::SYMLINK_BROKEN,
                            format!("this link points to `{target}`, which does not exist"),
                        )
                        .with_path(shown(link))
                        .with_help("remove the link or point it at an existing file"),
                    );
                    continue;
                }
                Err(e)
            }
            ok => ok,
        };
        let entry = entry.map_err(|e| {
            Problem::error(
                codes::DISCOVER_FAILED,
                format!("could not read the input folder: {e}"),
            )
            .with_path(&input_rel)
        })?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(&input)
            .map_err(|_| {
                Problem::error(
                    codes::DISCOVER_FAILED,
                    "found a file outside the input folder while walking it",
                )
                .with_path(shown(entry.path()))
            })?
            .to_path_buf();
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if PLUGIN_ONLY_EXTS.iter().any(|x| name.ends_with(x)) {
            found.plugin_only.push(rel);
        } else if UNSUPPORTED_EXTS.iter().any(|x| name.ends_with(x)) {
            found.unsupported.push(rel);
        } else if TEMPLATE_EXTS.iter().any(|x| name.ends_with(x)) {
            found.templates.push(rel);
        }
    }
    found.templates.sort();
    found.unsupported.sort();
    found.plugin_only.sort();

    // Only worth a second look when an ignore line names a path that doesn't exist.
    let templates: Vec<PathBuf> = found.templates.iter().map(|t| input.join(t)).collect();
    let entries = || {
        let mut all = list_entries(&root, &root, &out_dir, &rules);
        if input_rel.starts_with("..") {
            all.extend(list_entries(&input, &root, &out_dir, &rules));
        }
        all
    };
    let warnings = rules.anchored_warnings(entries, &templates);
    found.warnings.extend(warnings);
    Ok(found)
}

/// Every file and folder under `dir`, before any ignoring, as root-relative strings. Skips
/// `.git`, `node_modules` and the output folder.
fn list_entries(dir: &Path, root: &Path, output: &Path, rules: &IgnoreRules) -> Vec<String> {
    let output = output.to_path_buf();
    WalkBuilder::new(dir)
        .standard_filters(false)
        .hidden(false)
        .follow_links(true)
        .filter_entry(move |e| {
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            !(is_dir && e.path() == output)
                && e.file_name() != "node_modules"
                && e.file_name() != ".git"
        })
        .build()
        .filter_map(Result::ok)
        .filter_map(|e| {
            rules
                .root_rel(e.path())
                .or_else(|| e.path().strip_prefix(root).ok().map(Path::to_path_buf))
        })
        .filter(|r| !r.as_os_str().is_empty())
        .map(|r| {
            r.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

/// The link path of a symlink-loop walk error, looking through path and depth wrappers.
fn loop_child(e: &ignore::Error) -> Option<&std::path::Path> {
    match e {
        ignore::Error::Loop { child, .. } => Some(child),
        ignore::Error::WithPath { err, .. } | ignore::Error::WithDepth { err, .. } => {
            loop_child(err)
        }
        ignore::Error::Partial(errs) => errs.iter().find_map(loop_child),
        _ => None,
    }
}

/// The path of a link whose target is missing, looking through path and depth wrappers.
fn broken_link(e: &ignore::Error) -> Option<&std::path::Path> {
    fn is_not_found(e: &ignore::Error) -> bool {
        match e {
            ignore::Error::Io(io) => io.kind() == std::io::ErrorKind::NotFound,
            ignore::Error::WithPath { err, .. } | ignore::Error::WithDepth { err, .. } => {
                is_not_found(err)
            }
            _ => false,
        }
    }
    match e {
        ignore::Error::WithPath { path, err } if is_not_found(err) => Some(path),
        ignore::Error::WithPath { err, .. } | ignore::Error::WithDepth { err, .. } => {
            broken_link(err)
        }
        ignore::Error::Partial(errs) => errs.iter().find_map(broken_link),
        _ => None,
    }
}
