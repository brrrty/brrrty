use crate::ignore_files::IgnoreRules;
use crate::options::{BuildOptions, root_relative};
use crate::problem::{Problem, codes};
use ignore::WalkBuilder;
use std::path::PathBuf;
use std::sync::Arc;

const TEMPLATE_EXTS: &[&str] = &[".md", ".html"];
const UNSUPPORTED_EXTS: &[&str] = &[".liquid", ".njk", ".11ty.js", ".11ty.cjs", ".11ty.mjs"];
/// Template types 11ty only picks up when a plugin registers them.
pub const PLUGIN_ONLY_EXTS: &[&str] = &[".webc", ".hbs", ".mustache", ".ejs", ".haml", ".pug"];

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

    let rules = Arc::new(IgnoreRules::load(&root, &input)?);

    let includes = input.join("_includes");
    let data = input.join("_data");

    let (out_dir, inc_dir, data_dir) = (output.clone(), includes.clone(), data.clone());
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

    let mut found = Discovery::default();
    for entry in walker {
        let entry = match entry {
            Err(e) => {
                if let Some(link) = loop_child(&e) {
                    found.warnings.push(
                        Problem::warning(
                            codes::SYMLINK_LOOP,
                            "this link points back into a folder that contains it, so it was skipped",
                        )
                        .with_path(root_relative(&root, link))
                        .with_help("Remove the link or point it outside its own folder."),
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
            .with_path(root_relative(&root, &input))
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
                .with_path(root_relative(&root, entry.path()))
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

    // Only worth a second look when an ignore line exists and templates are being built.
    let templates: Vec<PathBuf> = found.templates.iter().map(|t| input.join(t)).collect();
    let entries: Vec<PathBuf> = if rules.is_empty() {
        Vec::new()
    } else {
        WalkBuilder::new(&input)
            .standard_filters(false)
            .hidden(false)
            .follow_links(true)
            .filter_entry(move |e| {
                let is_dir = e.file_type().is_some_and(|t| t.is_dir());
                !(is_dir && (e.path() == out_dir || e.path() == inc_dir || e.path() == data_dir))
                    && e.file_name() != "node_modules"
            })
            .build()
            .filter_map(Result::ok)
            .map(|e| e.into_path())
            .collect()
    };
    found
        .warnings
        .extend(rules.anchored_warnings(&entries, &templates));
    Ok(found)
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
