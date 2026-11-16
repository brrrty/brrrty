//! `git_head()`: the commit `git rev-parse HEAD` prints, read from the
//! repository's files. No process is started, and nothing is written.
//!
//! The repository is found as git finds it from the project root, going
//! up: a `.git` file must say `gitdir: <path>` (a linked worktree or a
//! submodule) and name a git folder, or that is an error, as in git; a
//! `.git` folder, or the folder itself, counts only when it is a git
//! folder (`HEAD` holding a ref under `refs/` or an object name, and
//! `objects` and `refs` folders), and otherwise the search goes on up. A
//! linked worktree's `commondir` names the folder that holds the shared
//! refs. `HEAD` is a commit, or a ref that is read from its loose file,
//! then from `packed-refs`, following at most 5 refs in all.
//!
//! What a file holds is read the way git reads it: trailing whitespace
//! goes, `ref:` may be followed by spaces, an object name may be in upper
//! case and may be followed by whitespace and anything, and a
//! `packed-refs` line is exactly `<name> <ref>`.
//!
//! Environment variables (`GIT_DIR`, `GIT_WORK_TREE`,
//! `GIT_CEILING_DIRECTORIES` and the rest) are not read: the result
//! depends only on the project's files. git's `safe.directory` ownership
//! check and its stop at file system boundaries aren't applied either.

use std::fs;
use std::path::{Path, PathBuf};

/// How many refs git reads, `HEAD` included, before it gives up on a
/// chain of symbolic refs (`SYMREF_MAXDEPTH`).
const MAX_SYMREF_DEPTH: usize = 5;

/// The commit `HEAD` points at in the repository `start` is in.
pub fn head(start: &Path) -> Result<String, String> {
    let start = start
        .canonicalize()
        .map_err(|e| format!("cannot read the folder {}: {e}", start.display()))?;
    let git_dir = find_git_dir(&start)?.ok_or_else(|| {
        format!(
            "{} is not in a git repository (no .git in it or any folder above it)",
            start.display()
        )
    })?;
    let common = common_dir(&git_dir);
    if common.join("reftable").is_dir() {
        return Err(
            "the repository keeps its refs in a reftable, which Brrrty doesn't read".into(),
        );
    }
    let len = hash_len(&common);
    let mut name = "HEAD".to_string();
    for _ in 0..MAX_SYMREF_DEPTH {
        let value = read_ref(&git_dir, &common, &name)?.ok_or_else(|| {
            if name == "HEAD" {
                "the repository has no HEAD".to_string()
            } else {
                format!("HEAD points at {name}, which has no commit yet")
            }
        })?;
        match parse_ref(&value, len) {
            Some(Ref::Symbolic(target)) => name = target,
            Some(Ref::Object(id)) => return Ok(id),
            None => return Err(format!("{name} holds neither a commit nor a ref")),
        }
    }
    Err(format!(
        "HEAD goes through more than {} symbolic refs",
        MAX_SYMREF_DEPTH - 1
    ))
}

/// git's `isspace`.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

enum Ref {
    Symbolic(String),
    Object(String),
}

/// A loose ref's content, as `parse_loose_ref_contents` reads it.
fn parse_ref(content: &str, len: usize) -> Option<Ref> {
    let content = content.trim_end_matches(is_space);
    if let Some(target) = content.strip_prefix("ref:") {
        return Some(Ref::Symbolic(
            target.trim_start_matches(is_space).to_string(),
        ));
    }
    let id = content.get(..len)?;
    if !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match content[len..].chars().next() {
        None => Some(Ref::Object(id.to_ascii_lowercase())),
        Some(c) if is_space(c) => Some(Ref::Object(id.to_ascii_lowercase())),
        Some(_) => None,
    }
}

/// The git folder for `start`, looking in it and the folders above it.
/// `Err` for a `.git` file that doesn't name a git folder.
fn find_git_dir(start: &Path) -> Result<Option<PathBuf>, String> {
    for dir in start.ancestors() {
        let dot_git = dir.join(".git");
        if dot_git.is_file() {
            return read_git_file(&dot_git).map(Some);
        }
        if dot_git.is_dir() && is_git_dir(&dot_git) {
            return Ok(Some(dot_git));
        }
        if is_git_dir(dir) {
            return Ok(Some(dir.to_path_buf()));
        }
    }
    Ok(None)
}

/// The git folder a `.git` file names (`read_gitfile`).
fn read_git_file(file: &Path) -> Result<PathBuf, String> {
    let bytes = fs::read(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let Some(path) = text.strip_prefix("gitdir: ") else {
        return Err(format!("{} isn't a `gitdir: <path>` file", file.display()));
    };
    let path = path.trim_end_matches(['\n', '\r']);
    if path.is_empty() {
        return Err(format!("{} isn't a `gitdir: <path>` file", file.display()));
    }
    let path = file.parent().unwrap_or(Path::new("")).join(path);
    if !is_git_dir(&path) {
        return Err(format!(
            "{} names {}, which isn't a git repository",
            file.display(),
            path.display()
        ));
    }
    Ok(path)
}

/// The folder `commondir` names, or `dir` itself.
fn common_dir(dir: &Path) -> PathBuf {
    match fs::read_to_string(dir.join("commondir")) {
        Ok(c) => dir.join(c.trim_end_matches(['\n', '\r'])),
        Err(_) => dir.to_path_buf(),
    }
}

/// git's test for a git folder (`is_git_directory`): `objects` and `refs`
/// (in it, or in its `commondir`), and a `HEAD` that `validate_headref`
/// accepts: `ref:` and a name under `refs/`, or an object name.
fn is_git_dir(dir: &Path) -> bool {
    let common = common_dir(dir);
    if !common.join("objects").is_dir() || !common.join("refs").is_dir() {
        return false;
    }
    let Ok(bytes) = fs::read(dir.join("HEAD")) else {
        return false;
    };
    let head = String::from_utf8_lossy(&bytes);
    match head.strip_prefix("ref:") {
        Some(target) => target.trim_start_matches(is_space).starts_with("refs/"),
        None => head
            .get(..40)
            .is_some_and(|id| id.bytes().all(|b| b.is_ascii_hexdigit())),
    }
}

/// 64 for a SHA-256 repository, else 40.
fn hash_len(common: &Path) -> usize {
    let config = fs::read_to_string(common.join("config")).unwrap_or_default();
    let sha256 = config.lines().any(|l| {
        let l: String = l.chars().filter(|c| !c.is_whitespace()).collect();
        l.eq_ignore_ascii_case("objectformat=sha256")
    });
    if sha256 { 64 } else { 40 }
}

/// A ref's content: its loose file, else its object name in
/// `packed-refs`. `HEAD` and the refs a worktree keeps for itself are in
/// its own git folder; the rest are shared.
fn read_ref(git_dir: &Path, common: &Path, name: &str) -> Result<Option<String>, String> {
    if name
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == "..")
        || name.contains('\\')
    {
        return Err("HEAD leads to a ref name git doesn't allow".into());
    }
    let own = name == "HEAD"
        || ["refs/bisect/", "refs/worktree/", "refs/rewritten/"]
            .iter()
            .any(|p| name.starts_with(p))
        || !name.starts_with("refs/");
    let base = if own { git_dir } else { common };
    let loose = base.join(name);
    if loose.is_file() {
        return fs::read(&loose)
            .map(|b| Some(String::from_utf8_lossy(&b).into_owned()))
            .map_err(|e| format!("cannot read {}: {e}", loose.display()));
    }
    if own {
        return Ok(None);
    }
    let Ok(packed) = fs::read(common.join("packed-refs")) else {
        return Ok(None);
    };
    let packed = String::from_utf8_lossy(&packed);
    // Lines end at `\n` only: a `\r` would be part of the name.
    for line in packed.split('\n') {
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        if let Some((id, ref_name)) = line.split_once(' ')
            && ref_name == name
        {
            return Ok(Some(id.to_string()));
        }
    }
    Ok(None)
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

    /// `git rev-parse HEAD` in `dir`, or `None` where it fails.
    fn rev_parse_opt(dir: &Path) -> Option<String> {
        let out = Command::new("git")
            .args(["rev-parse", "--verify", "-q", "HEAD"])
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .expect("git runs");
        out.status
            .success()
            .then(|| String::from_utf8(out.stdout).unwrap().trim().to_string())
    }

    /// `head` gives what `git rev-parse HEAD` gives, or fails where it
    /// fails.
    fn same(dir: &Path, what: &str) -> Option<String> {
        let want = rev_parse_opt(dir);
        let got = head(dir).ok();
        assert_eq!(got, want, "{what}");
        got
    }

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn symbolic_refs_are_followed_as_far_as_git_follows_them() {
        let t = repo();
        let g = t.path().join(".git");
        write(&g.join("refs/heads/l1"), "ref: refs/heads/main\n");
        for i in 2..=6 {
            write(
                &g.join(format!("refs/heads/l{i}")),
                &format!("ref: refs/heads/l{}\n", i - 1),
            );
        }
        let mut found = Vec::new();
        for i in 1..=6 {
            write(&g.join("HEAD"), &format!("ref: refs/heads/l{i}\n"));
            found.push(same(t.path(), &format!("HEAD -> l{i}")).is_some());
        }
        // HEAD and 4 refs: git reads 5 in all.
        assert_eq!(found, [true, true, true, false, false, false]);
    }

    #[test]
    fn ref_contents_are_read_as_git_reads_them() {
        let t = repo();
        let g = t.path().join(".git");
        let h = rev_parse(t.path());
        let upper = h.to_ascii_uppercase();
        let heads = [
            format!("{h}\n"),
            h.clone(),
            format!("{h} junk\n"),
            format!("{h}\tjunk\n"),
            format!("{h} \n"),
            format!("{h}\r\n"),
            format!(" {h}\n"),
            format!("{h}x\n"),
            format!("{h}{}\n", &h[..24]),
            format!("{upper}\n"),
            "ref: refs/heads/main\r\n".into(),
            "ref:refs/heads/main\n".into(),
            "ref:  refs/heads/main \n".into(),
            "ref:\trefs/heads/main\n".into(),
            "ref: FOO\n".into(),
            "nonsense\n".into(),
            String::new(),
        ];
        for content in &heads {
            write(&g.join("HEAD"), content);
            same(t.path(), &format!("HEAD = {content:?}"));
        }
        write(&g.join("HEAD"), "ref: refs/heads/main\n");
        let refs = [
            format!("{h}\r\n"),
            format!("{h} junk\n"),
            format!("{h}x\n"),
            upper.clone(),
            "ref:refs/heads/other\n".into(),
            "nonsense\n".into(),
        ];
        write(&g.join("refs/heads/other"), &format!("{h}\n"));
        for content in &refs {
            write(&g.join("refs/heads/main"), content);
            same(t.path(), &format!("refs/heads/main = {content:?}"));
        }
    }

    #[test]
    fn packed_refs_lines_are_read_as_git_reads_them() {
        let t = repo();
        let g = t.path().join(".git");
        let h = rev_parse(t.path());
        git(t.path(), &["tag", "-a", "-m", "t", "v1"]);
        git(t.path(), &["pack-refs", "--all"]);
        assert!(
            std::fs::read_to_string(g.join("packed-refs"))
                .unwrap()
                .contains("\n^")
        );
        // An annotated tag is its tag object, not the commit its `^` line
        // peels to.
        write(&g.join("HEAD"), "ref: refs/tags/v1\n");
        assert_ne!(same(t.path(), "HEAD -> annotated tag").unwrap(), h);
        write(&g.join("HEAD"), "ref: refs/heads/main\n");
        let upper = h.to_ascii_uppercase();
        for packed in [
            format!("{h} refs/heads/main\n"),
            format!("{h} refs/heads/main\r\n"),
            format!("{h}  refs/heads/main\n"),
            format!("{h} refs/heads/main \n"),
            format!("{upper} refs/heads/main\n"),
        ] {
            write(&g.join("packed-refs"), &packed);
            same(t.path(), &format!("packed-refs = {packed:?}"));
        }
    }

    #[test]
    fn a_folder_that_isnt_a_repository_is_passed_over() {
        let t = repo();
        let sub = t.path().join("sub");
        let g = sub.join(".git");
        std::fs::create_dir_all(&g).unwrap();
        same(&sub, "an empty .git folder");
        std::fs::create_dir_all(g.join("objects")).unwrap();
        std::fs::create_dir_all(g.join("refs")).unwrap();
        for content in ["ref: FOO\n", "junk\n", ""] {
            write(&g.join("HEAD"), content);
            let got = same(&sub, &format!("nested HEAD = {content:?}"));
            assert_eq!(got, Some(rev_parse(t.path())));
        }
        // A valid one with no commit stops the search.
        write(&g.join("HEAD"), "ref: refs/heads/nope\n");
        assert_eq!(same(&sub, "nested unborn"), None);
    }

    #[test]
    fn a_git_file_that_names_no_repository_is_an_error() {
        let t = repo();
        let sub = t.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let other = repo();
        let target = other.path().join(".git");
        for content in [
            "gitdir: /etc\n".to_string(),
            "garbage\n".into(),
            String::new(),
            "gitdir:../.git\n".into(),
            "GITDIR: ../.git\n".into(),
            "gitdir: ../.git  \n".into(),
            "gitdir: \n".into(),
            format!("gitdir: {}\r\n", target.display()),
            format!("gitdir: {}", target.display()),
        ] {
            write(&sub.join(".git"), &content);
            same(&sub, &format!(".git file = {content:?}"));
        }
        write(&sub.join(".git"), "gitdir: /etc\n");
        assert!(head(&sub).unwrap_err().contains("isn't a git repository"));
    }
}
