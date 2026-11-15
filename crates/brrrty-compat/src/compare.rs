//! Directory comparison.

use crate::normalize::normalize_markup;
use crate::report::{CompareReport, FileResult, Outcome};
use crate::{AllowList, CompatError};
use similar::TextDiff;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(PartialEq)]
enum Kind {
    Markup,
    Text,
    Binary,
}

fn kind(path: &Path) -> Kind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html" | "htm" | "xml" | "svg") => Kind::Markup,
        Some("css" | "js" | "mjs" | "json" | "txt" | "md" | "asc" | "webmanifest" | "map") => {
            Kind::Text
        }
        _ => Kind::Binary,
    }
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> CompatError + '_ {
    move |source| CompatError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<PathBuf>) -> Result<(), CompatError> {
    for entry in fs::read_dir(dir).map_err(io_err(dir))? {
        let path = entry.map_err(io_err(dir))?.path();
        if path.is_dir() {
            walk(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.insert(rel.to_path_buf());
        }
    }
    Ok(())
}

fn files(root: &Path) -> Result<BTreeSet<PathBuf>, CompatError> {
    let mut set = BTreeSet::new();
    walk(root, root, &mut set)?;
    Ok(set)
}

fn binary_diff(a: &[u8], b: &[u8]) -> Outcome {
    Outcome::Different {
        diff: format!("binary files differ ({} vs {} bytes)", a.len(), b.len()),
    }
}

fn compare_file(rel: &Path, a: &[u8], b: &[u8], allow: &AllowList) -> Outcome {
    if a == b {
        return Outcome::Identical;
    }
    let k = kind(rel);
    if k == Kind::Binary {
        return binary_diff(a, b);
    }
    let (Ok(a), Ok(b)) = (std::str::from_utf8(a), std::str::from_utf8(b)) else {
        return binary_diff(a, b);
    };
    let (mut a, mut b) = (
        allow.apply(rel, a).into_owned(),
        allow.apply(rel, b).into_owned(),
    );
    if a == b {
        return Outcome::Equivalent;
    }
    if k == Kind::Markup {
        (a, b) = (normalize_markup(&a), normalize_markup(&b));
        if a == b {
            return Outcome::Equivalent;
        }
    }
    let diff = TextDiff::from_lines(&a, &b)
        .unified_diff()
        .context_radius(3)
        .header("reference", "actual")
        .to_string();
    Outcome::Different { diff }
}

/// Compare every file under `reference` with the same relative path under `actual`.
pub fn compare_dirs(
    reference: &Path,
    actual: &Path,
    allow: &AllowList,
) -> Result<CompareReport, CompatError> {
    let (ref_files, act_files) = (files(reference)?, files(actual)?);
    let mut results = Vec::new();
    for rel in ref_files.union(&act_files) {
        let outcome = match (ref_files.contains(rel), act_files.contains(rel)) {
            (true, false) => Outcome::Missing,
            (false, true) => Outcome::Extra,
            _ => {
                let (rp, ap) = (reference.join(rel), actual.join(rel));
                let a = fs::read(&rp).map_err(io_err(&rp))?;
                let b = fs::read(&ap).map_err(io_err(&ap))?;
                compare_file(rel, &a, &b, allow)
            }
        };
        results.push(FileResult {
            path: rel.clone(),
            outcome,
        });
    }
    Ok(CompareReport { results })
}
