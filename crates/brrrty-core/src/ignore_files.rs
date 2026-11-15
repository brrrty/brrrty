//! Ignore files, read the way 11ty reads them.
//!
//! Each line of `.gitignore`, `.eleventyignore` (root) and `<input>/.eleventyignore` becomes a
//! glob anchored at that file's own folder. This is not gitignore: a line matches at any depth
//! there, and only at one place here.

use crate::problem::{Problem, codes};
use globset::{GlobBuilder, GlobMatcher};
use ignore::gitignore::GitignoreBuilder;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct Rule {
    /// Ignore file, relative to the root.
    file: PathBuf,
    /// Folder the line is anchored at, relative to the root (empty for the root).
    dir: PathBuf,
    line_no: usize,
    /// The line as written.
    text: String,
    /// The anchored path pattern, relative to the root, with `/` separators.
    anchored: String,
    matcher: GlobMatcher,
}

#[derive(Debug, Default)]
pub struct IgnoreRules {
    root: PathBuf,
    rules: Vec<Rule>,
}

impl IgnoreRules {
    /// Read the ignore files. `root` and `input` are absolute.
    pub fn load(root: &Path, input: &Path) -> Result<IgnoreRules, Problem> {
        let mut files = vec![root.join(".gitignore"), root.join(".eleventyignore")];
        if input != root {
            files.push(input.join(".eleventyignore"));
        }
        let mut rules = Vec::new();
        for file in files {
            if !file.is_file() {
                continue;
            }
            let rel_file = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
            let body = fs::read(&file).map_err(|e| {
                Problem::error(
                    codes::IGNORE_FILE_INVALID,
                    format!("could not read this ignore file: {e}"),
                )
                .with_path(&rel_file)
            })?;
            let body = String::from_utf8_lossy(&body);
            let dir_abs = file.parent().unwrap_or(root).to_path_buf();
            let dir = dir_abs
                .strip_prefix(root)
                .unwrap_or(Path::new(""))
                .to_path_buf();
            for (i, raw) in body.lines().enumerate() {
                let text = raw.trim();
                if text.is_empty() || text.starts_with('#') || text.starts_with('!') {
                    continue;
                }
                let mut line = text;
                while let Some(rest) = line.strip_prefix("./").or_else(|| line.strip_prefix('/')) {
                    line = rest;
                }
                let line = line.trim_end_matches('/');
                if line.is_empty() {
                    continue;
                }
                let mut anchored = path_str(&dir.join(line));
                if root.join(&anchored).is_dir() {
                    anchored.push_str("/**");
                }
                let glob = GlobBuilder::new(&anchored)
                    .literal_separator(true)
                    .build()
                    .map_err(|e| {
                        Problem::error(
                            codes::IGNORE_FILE_INVALID,
                            format!(
                                "line {} `{}` is not a valid pattern: {}",
                                i + 1,
                                text,
                                e.kind()
                            ),
                        )
                        .with_path(&rel_file)
                        .at(i + 1, 1)
                    })?;
                rules.push(Rule {
                    file: rel_file.clone(),
                    dir: dir.clone(),
                    line_no: i + 1,
                    text: text.to_string(),
                    anchored,
                    matcher: glob.compile_matcher(),
                });
            }
        }
        Ok(IgnoreRules {
            root: root.to_path_buf(),
            rules,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// True if the path, or any folder above it, matches a rule. `path` is absolute.
    /// A folder is matched by the same rules as a file; `_is_dir` is accepted for callers
    /// that know, but 11ty's globs don't treat the two differently.
    pub fn is_ignored(&self, path: &Path, _is_dir: bool) -> bool {
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return false;
        };
        if self.rules.is_empty() {
            return false;
        }
        let mut cur = PathBuf::new();
        for part in rel.components() {
            cur.push(part);
            let s = path_str(&cur);
            if self.rules.iter().any(|r| r.matcher.is_match(&s)) {
                return true;
            }
        }
        false
    }

    /// Warn about lines that look like gitignore habits: the line matches nothing where 11ty
    /// anchors it, but gitignore would have matched templates Brrrty is building.
    /// `entries` is every file and folder under the input folder, before any ignoring.
    /// `templates` is what will be built. Both are absolute. Paths in messages are relative
    /// to the root.
    pub fn anchored_warnings(&self, entries: &[PathBuf], templates: &[PathBuf]) -> Vec<Problem> {
        let mut out = Vec::new();
        for rule in &self.rules {
            let hits_something = self.root.join(&rule.anchored).exists()
                || entries.iter().any(|e| {
                    e.strip_prefix(&self.root)
                        .map(|r| rule.matcher.is_match(path_str(r)))
                        .unwrap_or(false)
                });
            if hits_something {
                continue;
            }
            let mut gi = GitignoreBuilder::new(self.root.join(&rule.dir));
            if gi.add_line(None, &rule.text).is_err() {
                continue;
            }
            let Ok(gi) = gi.build() else { continue };
            let matched: Vec<String> = templates
                .iter()
                .filter(|t| gi.matched_path_or_any_parents(t, false).is_ignore())
                .map(|t| path_str(t.strip_prefix(&self.root).unwrap_or(t)))
                .collect();
            let Some(first) = matched.first() else {
                continue;
            };
            let place = if rule.dir.as_os_str().is_empty() {
                "at the project root".to_string()
            } else {
                format!("from `{}`", path_str(&rule.dir))
            };
            let more = match matched.len() - 1 {
                0 => String::new(),
                n => format!(" and {n} more"),
            };
            let file = path_str(&rule.file);
            let line = rule.text.trim_end_matches('/');
            let full = path_str(
                &rule
                    .dir
                    .join(line.trim_start_matches("./").trim_start_matches('/')),
            );
            out.push(
                Problem::warning(
                    codes::IGNORE_LINE_ANCHORED,
                    format!(
                        "{file} line {} `{}` only ignores `{}` {place} (11ty reads ignore lines as paths from the file's folder); it does not ignore {first}{more}",
                        rule.line_no, rule.text, rule.anchored.trim_end_matches("/**"),
                    ),
                )
                .with_path(&rule.file)
                .at(rule.line_no, 1)
                .with_help(format!(
                    "to ignore it, write the full path, like {}",
                    help_example(&full, first)
                )),
            );
        }
        out
    }
}

/// The folder (or file) of `first` that a full-path line should name, ending where the
/// gitignore-style `full` name ends.
fn help_example(name: &str, first: &str) -> String {
    let want = name.rsplit('/').next().unwrap_or(name);
    let parts: Vec<&str> = first.split('/').collect();
    match parts.iter().position(|p| *p == want) {
        Some(i) => parts[..=i].join("/"),
        None => first.to_string(),
    }
}

fn path_str(p: &Path) -> String {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(files: &[&str], ignore: &str) -> (tempfile::TempDir, IgnoreRules) {
        let t = tempfile::tempdir().unwrap();
        for f in files {
            let p = t.path().join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "x").unwrap();
        }
        fs::write(t.path().join(".gitignore"), ignore).unwrap();
        let root = t.path().canonicalize().unwrap();
        let rules = IgnoreRules::load(&root, &root.join("src")).unwrap();
        (t, rules)
    }

    fn ign(t: &tempfile::TempDir, r: &IgnoreRules, rel: &str) -> bool {
        r.is_ignored(&t.path().canonicalize().unwrap().join(rel), false)
    }

    #[test]
    fn crlf_lines_are_trimmed() {
        let (t, r) = setup(&["drafts"], "drafts\r\n");
        assert!(ign(&t, &r, "drafts"));
    }

    #[test]
    fn trailing_slash_ignores_an_existing_folder() {
        let (t, r) = setup(&["drafts/a.md"], "drafts/\n");
        assert!(ign(&t, &r, "drafts/a.md"));
        assert!(!ign(&t, &r, "src/drafts/a.md"));
    }

    #[test]
    fn negations_and_comments_are_skipped() {
        let (t, r) = setup(&["keep.md", "# comment"], "!keep.md\n# comment\n");
        assert!(!ign(&t, &r, "keep.md"));
        assert!(!ign(&t, &r, "# comment"));
    }

    #[test]
    fn star_is_anchored_at_the_ignore_files_folder() {
        let (t, r) = setup(&["a.md", "src/a.md"], "*.md\n");
        assert!(ign(&t, &r, "a.md"));
        assert!(!ign(&t, &r, "src/a.md"));
    }

    #[test]
    fn missing_folder_pattern_matches_only_that_exact_path() {
        let (t, r) = setup(&["gone", "src/gone"], "gone\n");
        assert!(ign(&t, &r, "gone"));
        assert!(!ign(&t, &r, "src/gone"));
    }

    #[test]
    fn invalid_glob_names_file_line_and_pattern() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join(".gitignore"), "ok\n[abc\n").unwrap();
        let root = t.path().canonicalize().unwrap();
        let e = IgnoreRules::load(&root, &root.join("src")).unwrap_err();
        assert_eq!(e.code, codes::IGNORE_FILE_INVALID);
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("[abc"));
        assert_eq!(e.path.as_deref(), Some(Path::new(".gitignore")));
    }
}
