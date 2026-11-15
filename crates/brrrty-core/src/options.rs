use crate::problem::Problem;
use std::path::{Component, Path, PathBuf};

/// Options for one build.
#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub root: PathBuf,
    /// Relative to `root`.
    pub input: PathBuf,
    /// Relative to `root`, or absolute.
    pub output: PathBuf,
    /// Relative to `root`.
    pub passthrough: Vec<PathBuf>,
    pub strict: bool,
}

/// Resolve `.` and `..` lexically, without touching the filesystem.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Make `path` absolute and resolve symlinks. The path may not exist yet: the longest
/// existing ancestor is canonicalized and the remainder is appended.
pub(crate) fn resolve(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from(std::path::MAIN_SEPARATOR_STR))
            .join(path)
    };
    let abs = normalize(&abs);
    let mut existing = abs.clone();
    let mut rest = Vec::new();
    loop {
        if let Ok(c) = existing.canonicalize() {
            let mut out = c;
            out.extend(rest.iter().rev());
            return out;
        }
        match existing.file_name().map(|n| n.to_os_string()) {
            Some(name) if existing.pop() => rest.push(name),
            _ => return abs,
        }
    }
}

impl BuildOptions {
    pub fn root_dir(&self) -> PathBuf {
        resolve(&self.root)
    }

    pub fn input_dir(&self) -> PathBuf {
        resolve(&self.root_dir().join(&self.input))
    }

    pub fn output_dir(&self) -> PathBuf {
        resolve(&self.root_dir().join(&self.output))
    }

    /// Refuse an output folder that is the root, the input dir, or an ancestor of the input dir.
    pub fn validate(&self) -> Result<(), Problem> {
        let out = self.output_dir();
        let input = self.input_dir();
        if out == self.root_dir() || input.starts_with(&out) {
            return Err(Problem::error(
                "output-overlaps-source",
                format!(
                    "the output folder {} would overlap the source folder {}",
                    out.display(),
                    input.display()
                ),
            )
            .with_help(
                "choose an output folder that is separate from your source files, such as _site",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_makes_dot_absolute_and_nonempty() {
        let r = resolve(Path::new("."));
        assert!(r.is_absolute());
        assert_eq!(r, std::env::current_dir().unwrap().canonicalize().unwrap());
    }

    #[test]
    fn resolve_handles_missing_tail() {
        let r = resolve(Path::new("./nope/../_site"));
        assert_eq!(r, resolve(Path::new(".")).join("_site"));
    }
}
