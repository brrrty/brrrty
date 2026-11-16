//! `git_head()`: the commit `git rev-parse HEAD` prints, read from the
//! repository's files. No process is started, and nothing is written.
//!
//! The repository is found as git finds it from the project root: the
//! first folder, going up, that has a `.git` folder, a `.git` file naming
//! one (`gitdir: ...`, as in a linked worktree or a submodule), or that is
//! a git folder itself. A linked worktree's `commondir` names the folder
//! that holds the shared refs. `HEAD` is a commit, or a ref that is read
//! from its loose file, then from `packed-refs`.

use std::fs;
use std::path::{Path, PathBuf};

/// How many symbolic refs are followed before giving up, as git does.
const MAX_SYMREF_DEPTH: usize = 5;

/// The commit `HEAD` points at in the repository `start` is in.
pub fn head(start: &Path) -> Result<String, String> {
    let start = start
        .canonicalize()
        .map_err(|e| format!("cannot read the folder {}: {e}", start.display()))?;
    let git_dir = find_git_dir(&start).ok_or_else(|| {
        format!(
            "{} is not in a git repository (no .git in it or any folder above it)",
            start.display()
        )
    })?;
    let common = match read_trimmed(&git_dir.join("commondir")) {
        Some(c) => git_dir.join(c),
        None => git_dir.clone(),
    };
    if common.join("reftable").is_dir() {
        return Err(
            "the repository keeps its refs in a reftable, which Brrrty doesn't read".into(),
        );
    }
    let mut name = "HEAD".to_string();
    for _ in 0..=MAX_SYMREF_DEPTH {
        let value = read_ref(&git_dir, &common, &name)?.ok_or_else(|| {
            if name == "HEAD" {
                "the repository has no HEAD".to_string()
            } else {
                format!("HEAD points at {name}, which has no commit yet")
            }
        })?;
        match value.strip_prefix("ref:") {
            Some(target) => name = target.trim().to_string(),
            None if is_hash(&value) => return Ok(value),
            None => return Err(format!("{name} holds `{value}`, which isn't a commit")),
        }
    }
    Err(format!(
        "HEAD goes through more than {MAX_SYMREF_DEPTH} symbolic refs"
    ))
}

/// The git folder for `start`, looking in it and the folders above it.
fn find_git_dir(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() && is_git_dir(&dot_git) {
            return Some(dot_git);
        }
        if dot_git.is_file()
            && let Some(target) = read_trimmed(&dot_git)
            && let Some(path) = target.strip_prefix("gitdir:")
        {
            let path = dir.join(path.trim());
            if is_git_dir(&path) {
                return Some(path);
            }
        }
        if is_git_dir(dir) {
            return Some(dir.to_path_buf());
        }
    }
    None
}

/// git's test for a git folder: it has `HEAD`, and `objects` and `refs`
/// (in it, or in the folder its `commondir` names).
fn is_git_dir(dir: &Path) -> bool {
    if !dir.join("HEAD").is_file() {
        return false;
    }
    let common = match read_trimmed(&dir.join("commondir")) {
        Some(c) => dir.join(c),
        None => dir.to_path_buf(),
    };
    common.join("objects").is_dir() && common.join("refs").is_dir()
}

/// A ref's value: its loose file, else its line in `packed-refs`. `HEAD`
/// and the refs a worktree keeps for itself are in its own git folder;
/// the rest are shared.
fn read_ref(git_dir: &Path, common: &Path, name: &str) -> Result<Option<String>, String> {
    if name
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == "..")
        || name.contains('\\')
    {
        return Err(format!("`{name}` isn't a ref name git allows"));
    }
    let own = name == "HEAD"
        || ["refs/bisect/", "refs/worktree/", "refs/rewritten/"]
            .iter()
            .any(|p| name.starts_with(p))
        || !name.starts_with("refs/");
    let base = if own { git_dir } else { common };
    let loose = base.join(name);
    if loose.is_file() {
        return read_trimmed(&loose)
            .map(Some)
            .ok_or_else(|| format!("cannot read {}", loose.display()));
    }
    if own {
        return Ok(None);
    }
    let Ok(packed) = fs::read_to_string(common.join("packed-refs")) else {
        return Ok(None);
    };
    for line in packed.lines() {
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        if let Some((hash, ref_name)) = line.split_once(' ')
            && ref_name == name
        {
            return Ok(Some(hash.to_string()));
        }
    }
    Ok(None)
}

fn read_trimmed(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    Some(text.trim_end_matches(['\n', '\r']).to_string())
}

/// A SHA-1 or SHA-256 object name, as git writes it.
fn is_hash(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=Brrrty",
                "-c",
                "user.email=brrrty@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn rev_parse(dir: &Path) -> String {
        git(dir, &["rev-parse", "HEAD"])
    }

    fn repo() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        git(t.path(), &["init", "-q"]);
        std::fs::write(t.path().join("a.txt"), "a").unwrap();
        git(t.path(), &["add", "a.txt"]);
        git(t.path(), &["commit", "-q", "-m", "one"]);
        std::fs::write(t.path().join("a.txt"), "b").unwrap();
        git(t.path(), &["commit", "-q", "-am", "two"]);
        t
    }

    #[test]
    fn a_branch() {
        let t = repo();
        let sub = t.path().join("site/src");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(head(t.path()).unwrap(), rev_parse(t.path()));
        assert_eq!(head(&sub).unwrap(), rev_parse(t.path()));
    }

    #[test]
    fn a_detached_head() {
        let t = repo();
        git(t.path(), &["checkout", "-q", "--detach", "HEAD~1"]);
        assert!(
            std::fs::read_to_string(t.path().join(".git/HEAD"))
                .unwrap()
                .len()
                >= 40
        );
        assert_eq!(head(t.path()).unwrap(), rev_parse(t.path()));
    }

    #[test]
    fn packed_refs() {
        let t = repo();
        git(t.path(), &["pack-refs", "--all"]);
        assert!(!t.path().join(".git/refs/heads/main").exists());
        assert_eq!(head(t.path()).unwrap(), rev_parse(t.path()));
        // A loose ref wins over its packed line.
        git(t.path(), &["commit", "-q", "--allow-empty", "-m", "three"]);
        assert!(t.path().join(".git/refs/heads/main").exists());
        assert_eq!(head(t.path()).unwrap(), rev_parse(t.path()));
    }

    #[test]
    fn a_linked_worktree() {
        let t = repo();
        let wt = tempfile::tempdir().unwrap();
        let path = wt.path().join("wt");
        git(
            t.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "other",
                path.to_str().unwrap(),
                "HEAD~1",
            ],
        );
        assert!(path.join(".git").is_file());
        assert_eq!(head(&path).unwrap(), rev_parse(&path));
        assert_ne!(head(&path).unwrap(), head(t.path()).unwrap());
        // The worktree's branch packed in the shared folder.
        git(t.path(), &["pack-refs", "--all"]);
        assert_eq!(head(&path).unwrap(), rev_parse(&path));
        git(&path, &["checkout", "-q", "--detach"]);
        assert_eq!(head(&path).unwrap(), rev_parse(&path));
    }

    #[test]
    fn no_repository_or_no_commit_is_an_error() {
        let t = tempfile::tempdir().unwrap();
        // A temp folder isn't in a repository on CI or dev machines.
        let e = head(t.path()).unwrap_err();
        assert!(e.contains("not in a git repository"), "{e}");
        git(t.path(), &["init", "-q"]);
        let e = head(t.path()).unwrap_err();
        assert!(
            e.contains("refs/heads/main") && e.contains("no commit"),
            "{e}"
        );
    }
}
