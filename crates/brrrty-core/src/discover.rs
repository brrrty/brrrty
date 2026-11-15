use crate::options::BuildOptions;
use crate::problem::Problem;
use ignore::WalkBuilder;
use ignore::gitignore::GitignoreBuilder;
use std::path::PathBuf;

const TEMPLATE_EXTS: &[&str] = &[".md", ".html"];
const UNSUPPORTED_EXTS: &[&str] = &[
    ".liquid",
    ".njk",
    ".11ty.js",
    ".11ty.cjs",
    ".11ty.mjs",
    ".webc",
    ".hbs",
    ".mustache",
    ".ejs",
    ".haml",
    ".pug",
];

/// Templates found under the input dir. Paths are relative to the input dir and sorted.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub templates: Vec<PathBuf>,
    pub unsupported: Vec<PathBuf>,
}

/// Find input templates, following 11ty's default ignore rules.
pub fn discover(opts: &BuildOptions) -> Result<Discovery, Problem> {
    let root = crate::options::normalize(&opts.root);
    let input = opts.input_dir();
    let output = opts.output_dir();

    let mut gi = GitignoreBuilder::new(&root);
    for name in [".gitignore", ".eleventyignore"] {
        let file = root.join(name);
        if file.is_file()
            && let Some(err) = gi.add(&file)
        {
            return Err(Problem::error("ignore-file-invalid", err.to_string()).with_path(file));
        }
    }
    let matcher = gi
        .build()
        .map_err(|e| Problem::error("ignore-file-invalid", e.to_string()))?;

    let includes = input.join("_includes");
    let data = input.join("_data");

    let walker = WalkBuilder::new(&input)
        .standard_filters(false)
        .hidden(false)
        .follow_links(false)
        .filter_entry(move |e| {
            let p = e.path();
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            if is_dir && (p == output || p == includes || p == data) {
                return false;
            }
            if e.file_name() == "node_modules" {
                return false;
            }
            !matcher.matched_path_or_any_parents(p, is_dir).is_ignore()
        })
        .build();

    let mut found = Discovery::default();
    for entry in walker {
        let entry = entry.map_err(|e| {
            Problem::error(
                "discover-failed",
                format!("could not read the input folder: {e}"),
            )
            .with_path(input.clone())
        })?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(&input)
            .expect("walked path is under the input dir")
            .to_path_buf();
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if UNSUPPORTED_EXTS.iter().any(|x| name.ends_with(x)) {
            found.unsupported.push(rel);
        } else if TEMPLATE_EXTS.iter().any(|x| name.ends_with(x)) {
            found.templates.push(rel);
        }
    }
    found.templates.sort();
    found.unsupported.sort();
    Ok(found)
}
