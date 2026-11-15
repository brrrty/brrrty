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

impl BuildOptions {
    pub fn input_dir(&self) -> PathBuf {
        normalize(&self.root.join(&self.input))
    }

    pub fn output_dir(&self) -> PathBuf {
        normalize(&self.root.join(&self.output))
    }

    /// Refuse an output folder that is the root, the input dir, or an ancestor of the input dir.
    pub fn validate(&self) -> Result<(), Problem> {
        let out = self.output_dir();
        let input = self.input_dir();
        if out == normalize(&self.root) || input.starts_with(&out) {
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
