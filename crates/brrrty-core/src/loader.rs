//! The loaders for included templates: 11ty's LiquidJS roots, `_includes`
//! then the input folder, and its Nunjucks roots, `_includes` then the
//! project folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use brrrty_liquid::{LoadError, Loader};

/// One folder partials are looked up in.
#[derive(Debug, Clone)]
struct Root {
    /// The canonical folder.
    real: PathBuf,
    /// The folder relative to the project root, for messages.
    shown: PathBuf,
}

/// Loads partials from the includes folder, then the input folder, as 11ty
/// configures LiquidJS, or from the includes folder, then the project
/// folder, as it configures Nunjucks's `FileSystemLoader`. A name may not
/// lead outside those folders: it is normalized lexically first, and the
/// file found is canonicalized and must still be inside its folder, so a
/// symbolic link can't point elsewhere.
#[derive(Debug, Clone)]
pub(crate) struct FsLoader {
    roots: Vec<Root>,
    /// Nunjucks: an absolute name is a file in a root, the way Nunjucks
    /// resolves `./` names against the including template's path. A name
    /// outside every root is missing, as Nunjucks reports it.
    nunjucks: bool,
}

/// `path` with `.` and `..` resolved, as Node's `path.resolve` does for an
/// absolute path; `None` when it isn't absolute.
fn lexical_absolute(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                // `/..` is `/`.
                if out.parent().is_some() {
                    out.pop();
                }
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    Some(out)
}

/// `name` with `.` and `..` resolved, or `None` when it is absolute or
/// climbs above its starting folder.
pub(crate) fn lexical(name: &str) -> Option<PathBuf> {
    if name.starts_with('/') || Path::new(name).is_absolute() {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(parts.iter().collect())
}

impl FsLoader {
    /// `includes` and `input` are canonical; the `_shown` paths are relative
    /// to the project root.
    pub fn new(
        includes: PathBuf,
        includes_shown: PathBuf,
        input: PathBuf,
        input_shown: PathBuf,
    ) -> FsLoader {
        FsLoader {
            roots: vec![
                Root {
                    real: includes,
                    shown: includes_shown,
                },
                Root {
                    real: input,
                    shown: input_shown,
                },
            ],
            nunjucks: false,
        }
    }

    /// 11ty's Nunjucks loader: `includes` (canonical, shown as
    /// `includes_shown`), then `root`, the canonical project folder.
    pub fn nunjucks(includes: PathBuf, includes_shown: PathBuf, root: PathBuf) -> FsLoader {
        FsLoader {
            roots: vec![
                Root {
                    real: includes,
                    shown: includes_shown,
                },
                Root {
                    real: root,
                    shown: PathBuf::new(),
                },
            ],
            nunjucks: true,
        }
    }

    fn folders(&self) -> String {
        self.roots
            .iter()
            .map(|r| {
                if r.shown.as_os_str().is_empty() {
                    "the project folder".to_string()
                } else {
                    r.shown.display().to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" and ")
    }

    /// A path `load` returned, relative to the project root.
    pub fn shown(&self, real: &Path) -> Option<PathBuf> {
        // The includes folder is inside the input folder, so it is tried first.
        self.roots.iter().find_map(|r| {
            real.strip_prefix(&r.real)
                .ok()
                .map(|rest| r.shown.join(rest))
        })
    }
}

impl Loader for FsLoader {
    fn load(&self, name: &str) -> Result<(String, String), LoadError> {
        // Each root with the path the name has in it.
        let candidates: Vec<(&Root, PathBuf)> = if self.nunjucks && Path::new(name).is_absolute() {
            let path = lexical_absolute(Path::new(name)).unwrap_or_default();
            self.roots
                .iter()
                .filter_map(|r| {
                    path.strip_prefix(&r.real)
                        .ok()
                        .map(|rel| (r, rel.to_path_buf()))
                })
                .collect()
        } else {
            match lexical(name) {
                Some(rel) => self.roots.iter().map(|r| (r, rel.clone())).collect(),
                // Nunjucks finds nothing in a root a name leads out of.
                None if self.nunjucks => Vec::new(),
                None => {
                    return Err(LoadError {
                        tried: Vec::new(),
                        io: Some(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            format!("the name leads outside {}", self.folders()),
                        )),
                    });
                }
            }
        };
        let mut tried = Vec::new();
        for (root, rel) in candidates {
            tried.push(root.shown.join(&rel).display().to_string());
            let candidate = root.real.join(&rel);
            match fs::metadata(&candidate) {
                Ok(m) if m.is_file() => {}
                // Nunjucks's `existsSync` finds a folder, which it then
                // fails to read.
                Ok(_) if self.nunjucks => {
                    return Err(LoadError {
                        tried,
                        io: Some(io::Error::new(
                            io::ErrorKind::IsADirectory,
                            "it is a folder",
                        )),
                    });
                }
                Ok(_) => continue,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(LoadError { tried, io: Some(e) }),
            }
            let fail = |e: io::Error, tried: Vec<String>| LoadError { tried, io: Some(e) };
            let real = candidate
                .canonicalize()
                .map_err(|e| fail(e, tried.clone()))?;
            if !self.roots.iter().any(|r| real.starts_with(&r.real)) {
                return Err(fail(
                    io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("it links to a file outside {}", self.folders()),
                    ),
                    tried,
                ));
            }
            let bytes = fs::read(&real).map_err(|e| fail(e, tried.clone()))?;
            let text = String::from_utf8(bytes).map_err(|_| {
                fail(
                    io::Error::new(io::ErrorKind::InvalidData, "the file is not valid UTF-8"),
                    tried.clone(),
                )
            })?;
            return Ok((real.display().to_string(), text));
        }
        Err(LoadError { tried, io: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_names() {
        assert_eq!(lexical("a/b.liquid"), Some(PathBuf::from("a/b.liquid")));
        assert_eq!(lexical("./a//b/../c"), Some(PathBuf::from("a/c")));
        assert_eq!(lexical("../x"), None);
        assert_eq!(lexical("a/../../x"), None);
        assert_eq!(lexical("/etc/passwd"), None);
    }

    fn setup() -> (tempfile::TempDir, FsLoader) {
        let t = tempfile::tempdir().unwrap();
        let input = t.path().join("src");
        fs::create_dir_all(input.join("_includes/sub")).unwrap();
        fs::write(input.join("_includes/a.liquid"), "A").unwrap();
        fs::write(input.join("_includes/sub/b.liquid"), "B").unwrap();
        fs::write(input.join("top.liquid"), "TOP").unwrap();
        fs::write(input.join("_includes/bad.liquid"), [0xff, 0xfe]).unwrap();
        let input = input.canonicalize().unwrap();
        let loader = FsLoader::new(
            input.join("_includes"),
            PathBuf::from("src/_includes"),
            input,
            PathBuf::from("src"),
        );
        (t, loader)
    }

    #[test]
    fn loads_from_includes_then_input_with_canonical_paths() {
        let (_t, l) = setup();
        let (path, src) = l.load("sub/../a.liquid").unwrap();
        assert_eq!(src, "A");
        assert!(path.ends_with("src/_includes/a.liquid"), "{path}");
        assert!(Path::new(&path).is_absolute());
        assert_eq!(l.load("top.liquid").unwrap().1, "TOP");
        assert_eq!(
            l.shown(Path::new(&path)),
            Some(PathBuf::from("src/_includes/a.liquid"))
        );
    }

    #[test]
    fn missing_names_list_what_was_tried() {
        let (_t, l) = setup();
        let e = l.load("nope.liquid").unwrap_err();
        assert!(e.io.is_none());
        assert_eq!(e.tried, ["src/_includes/nope.liquid", "src/nope.liquid"]);
        // A folder is not a partial.
        assert!(l.load("sub").unwrap_err().io.is_none());
    }

    #[test]
    fn unreadable_files_are_not_missing() {
        let (_t, l) = setup();
        let e = l.load("bad.liquid").unwrap_err();
        assert_eq!(e.io.unwrap().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn names_cannot_leave_the_roots() {
        let (_t, l) = setup();
        for name in ["../../etc/passwd", "/etc/passwd", "../x.liquid"] {
            let e = l.load(name).unwrap_err();
            let io = e.io.unwrap();
            assert_eq!(io.kind(), io::ErrorKind::PermissionDenied, "{name}");
            assert!(io.to_string().contains("outside"), "{name}: {io}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_leave_the_roots() {
        let (t, l) = setup();
        fs::write(t.path().join("secret.liquid"), "SECRET").unwrap();
        std::os::unix::fs::symlink(
            t.path().join("secret.liquid"),
            t.path().join("src/_includes/link.liquid"),
        )
        .unwrap();
        let e = l.load("link.liquid").unwrap_err();
        assert!(
            e.io.unwrap()
                .to_string()
                .contains("outside src/_includes and src")
        );
        // A link that stays inside is fine.
        std::os::unix::fs::symlink("a.liquid", t.path().join("src/_includes/in.liquid")).unwrap();
        assert_eq!(l.load("in.liquid").unwrap().1, "A");
        // Nunjucks's second root is the project folder, named as such.
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("x.njk"), "X").unwrap();
        std::os::unix::fs::symlink(outside.path().join("x.njk"), t.path().join("link.njk"))
            .unwrap();
        let root = t.path().canonicalize().unwrap();
        let n = FsLoader::nunjucks(
            root.join("src/_includes"),
            PathBuf::from("src/_includes"),
            root,
        );
        let e = n.load("link.njk").unwrap_err().io.unwrap().to_string();
        assert!(
            e.contains("outside src/_includes and the project folder"),
            "{e}"
        );
    }
}
