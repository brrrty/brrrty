use crate::problem::{Problem, codes};
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
pub(crate) fn resolve(path: &Path) -> Result<PathBuf, Problem> {
    resolve_from(path, std::env::current_dir())
}

fn resolve_from(path: &Path, cwd: std::io::Result<PathBuf>) -> Result<PathBuf, Problem> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.map_err(|e| {
            Problem::error(
                codes::CWD_UNAVAILABLE,
                format!("cannot find the current folder: {e}"),
            )
            .with_help("run Brrrty from a folder that exists, or pass an absolute root")
        })?
        .join(path)
    };
    let abs = normalize(&abs);
    let mut existing = abs.clone();
    let mut rest = Vec::new();
    loop {
        if let Ok(c) = existing.canonicalize() {
            let mut out = c;
            out.extend(rest.iter().rev());
            return Ok(out);
        }
        match existing.file_name().map(|n| n.to_os_string()) {
            Some(name) if existing.pop() => rest.push(name),
            _ => return Ok(abs),
        }
    }
}

/// `path` relative to the project `root`, for reporting. A path outside the root is kept as given.
pub(crate) fn root_relative(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

impl BuildOptions {
    pub fn root_dir(&self) -> Result<PathBuf, Problem> {
        resolve(&self.root)
    }

    pub fn input_dir(&self) -> Result<PathBuf, Problem> {
        resolve(&self.root_dir()?.join(&self.input))
    }

    pub fn output_dir(&self) -> Result<PathBuf, Problem> {
        resolve(&self.root_dir()?.join(&self.output))
    }

    /// Refuse a missing input folder, and an output folder that is the root, the input dir,
    /// or an ancestor of the input dir.
    pub fn validate(&self) -> Result<(), Problem> {
        let out = self.output_dir()?;
        let input = self.input_dir()?;
        if !input.exists() {
            return Err(
                Problem::error(codes::INPUT_MISSING, "the input folder doesn't exist")
                    .with_path(&self.input)
                    .with_help("check the --input folder name, relative to where you ran Brrrty"),
            );
        }
        if !input.is_dir() {
            return Err(Problem::error(
                codes::INPUT_NOT_A_DIRECTORY,
                "the input path is a file, not a folder",
            )
            .with_path(&self.input)
            .with_help("pass the folder that holds your source files, like --input src"));
        }
        if out == self.root_dir()? || input.starts_with(&out) {
            return Err(Problem::error(
                codes::OUTPUT_OVERLAPS_SOURCE,
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
        let r = resolve(Path::new(".")).unwrap();
        assert!(r.is_absolute());
        assert_eq!(r, std::env::current_dir().unwrap().canonicalize().unwrap());
    }

    #[test]
    fn resolve_handles_missing_tail() {
        let r = resolve(Path::new("./nope/../_site")).unwrap();
        assert_eq!(r, resolve(Path::new(".")).unwrap().join("_site"));
    }

    #[test]
    fn unavailable_cwd_is_an_error_for_relative_paths_only() {
        let broken = || Err(std::io::Error::other("gone"));
        let p = resolve_from(Path::new("src"), broken()).unwrap_err();
        assert_eq!(p.code, codes::CWD_UNAVAILABLE);
        assert!(resolve_from(&std::env::temp_dir(), broken()).is_ok());
    }
}
