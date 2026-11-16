//! Ignore files, read the way 11ty reads them.
//!
//! Each line of `.gitignore`, `.eleventyignore` (root) and `<input>/.eleventyignore` becomes a
//! glob anchored at that file's own folder. This is not gitignore: a line matches at any depth
//! there, and only at one place here.

use crate::problem::{Problem, codes};
use globset::{GlobBuilder, GlobMatcher};
use ignore::gitignore::GitignoreBuilder;
use std::fs;
use std::path::{Component, Path, PathBuf};

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
    /// Added by the config file's `ignores.add`, not read from an ignore file.
    from_config: bool,
}

#[derive(Debug, Default)]
pub struct IgnoreRules {
    root: PathBuf,
    /// The input folder's real path, which is what the walker yields paths under.
    input: PathBuf,
    /// The input folder as spelled relative to the root; may start with `..`.
    input_rel: PathBuf,
    rules: Vec<Rule>,
    /// Lines that could not be read as a pattern and were skipped.
    pub warnings: Vec<Problem>,
}

impl IgnoreRules {
    /// Read the ignore files. `root` and `input` are absolute real paths; `input_rel` is the
    /// input folder as spelled relative to the root (see `BuildOptions::input_rel`).
    pub fn load(root: &Path, input: &Path, input_rel: &Path) -> Result<IgnoreRules, Problem> {
        let mut files = vec![
            (root.join(".gitignore"), PathBuf::new()),
            (root.join(".eleventyignore"), PathBuf::new()),
        ];
        if !input_rel.as_os_str().is_empty() {
            files.push((input.join(".eleventyignore"), input_rel.to_path_buf()));
        }
        let mut rules = Vec::new();
        let mut warnings = Vec::new();
        for (file, dir) in files {
            if !file.is_file() {
                continue;
            }
            let rel_file = dir.join(file.file_name().unwrap_or_default());
            let body = fs::read(&file).map_err(|e| {
                Problem::error(
                    codes::IGNORE_FILE_INVALID,
                    format!("could not read this ignore file: {e}"),
                )
                .with_path(&rel_file)
            })?;
            let body = String::from_utf8_lossy(&body);
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
                let glob = match GlobBuilder::new(&anchored).literal_separator(true).build() {
                    Ok(g) => g,
                    Err(e) => {
                        warnings.push(
                            Problem::warning(
                                codes::IGNORE_LINE_INVALID,
                                format!(
                                    "{} line {} `{}` is not a valid pattern ({}), so it was skipped",
                                    path_str(&rel_file),
                                    i + 1,
                                    text,
                                    e.kind()
                                ),
                            )
                            .with_path(&rel_file)
                            .at(i + 1, 1)
                            .with_help(
                                "check the brackets and braces on this line, or escape them with a backslash",
                            ),
                        );
                        continue;
                    }
                };
                rules.push(Rule {
                    file: rel_file.clone(),
                    dir: dir.clone(),
                    line_no: i + 1,
                    text: text.to_string(),
                    anchored,
                    matcher: glob.compile_matcher(),
                    from_config: false,
                });
            }
        }
        Ok(IgnoreRules {
            root: root.to_path_buf(),
            input: input.to_path_buf(),
            input_rel: input_rel.to_path_buf(),
            rules,
            warnings,
        })
    }

    /// Add the config file's `ignores.add` globs. 11ty reads each one from the
    /// project root (`./` is added and the path normalized), except one that
    /// starts with `**/`, which matches anywhere. A folder is not given `/**`
    /// the way an ignore file line is; it is still skipped whole, because
    /// the walk doesn't enter an ignored folder.
    pub fn add_config(&mut self, file: &Path, globs: &[String]) {
        for (i, text) in globs.iter().enumerate() {
            let anchored = if text.starts_with("**/") {
                text.clone()
            } else {
                let p = crate::options::normalize(Path::new(text.trim_start_matches('/')));
                path_str(&p)
            };
            if anchored.is_empty() || anchored.starts_with("..") {
                continue;
            }
            match GlobBuilder::new(&anchored).literal_separator(true).build() {
                Ok(glob) => self.rules.push(Rule {
                    file: file.to_path_buf(),
                    dir: PathBuf::new(),
                    line_no: 0,
                    text: text.clone(),
                    anchored,
                    matcher: glob.compile_matcher(),
                    from_config: true,
                }),
                Err(e) => self.warnings.push(
                    Problem::warning(
                        codes::IGNORE_LINE_INVALID,
                        format!(
                            "ignore `{text}` (number {} added in {}) is not a valid pattern ({}), so it was skipped",
                            i + 1,
                            path_str(file),
                            e.kind()
                        ),
                    )
                    .with_path(file)
                    .with_help(
                        "check the brackets and braces in the pattern, or escape them with a backslash",
                    ),
                ),
            }
        }
    }

    /// `path` (absolute, as the walker yields it) relative to the root, spelled through the
    /// input folder's own spelling. `None` for a path outside both.
    pub fn root_rel(&self, path: &Path) -> Option<PathBuf> {
        if let Ok(r) = path.strip_prefix(&self.input) {
            return Some(self.input_rel.join(r));
        }
        path.strip_prefix(&self.root).ok().map(Path::to_path_buf)
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// True if the path, or any folder above it, matches a rule. `path` is absolute, under the
    /// input folder or the root.
    /// A folder is matched by the same rules as a file; `_is_dir` is accepted for callers
    /// that know, but 11ty's globs don't treat the two differently.
    pub fn is_ignored(&self, path: &Path, _is_dir: bool) -> bool {
        if self.rules.is_empty() {
            return false;
        }
        let Some(rel) = self.root_rel(path) else {
            return false;
        };
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
    /// `entries` lists every file and folder under the root (and under the input folder, when
    /// that is outside the root), relative to the root, before any ignoring. It is only called
    /// when some line names a path that does not exist. `templates` is what will be built, as
    /// absolute paths under the input folder. Paths in messages are relative to the root.
    pub fn anchored_warnings(
        &self,
        entries: impl FnOnce() -> Vec<String>,
        templates: &[PathBuf],
    ) -> Vec<Problem> {
        let mut out = Vec::new();
        let mut entries_fn = Some(entries);
        let mut listed: Option<Vec<String>> = None;
        let templates: Vec<PathBuf> = templates.iter().filter_map(|t| self.root_rel(t)).collect();
        for rule in &self.rules {
            if rule.from_config || self.root.join(&rule.anchored).exists() {
                continue;
            }
            let all =
                listed.get_or_insert_with(|| entries_fn.take().map(|f| f()).unwrap_or_default());
            if all.iter().any(|e| rule.matcher.is_match(e)) {
                continue;
            }
            let mut gi = GitignoreBuilder::new(self.root.join(&rule.dir));
            if gi.add_line(None, &rule.text).is_err() {
                continue;
            }
            let Ok(gi) = gi.build() else { continue };
            // Gitignore only reaches templates under the ignore file's own folder.
            let matched: Vec<String> = templates
                .iter()
                .filter(|t| {
                    t.strip_prefix(&rule.dir).is_ok_and(|under| {
                        !under.as_os_str().is_empty()
                            && under
                                .components()
                                .all(|c| matches!(c, Component::Normal(_)))
                            && gi.matched_path_or_any_parents(under, false).is_ignore()
                    })
                })
                .map(|t| path_str(t))
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
        let rules = IgnoreRules::load(&root, &root.join("src"), Path::new("src")).unwrap();
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
    fn invalid_lines_warn_and_are_skipped() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join(".gitignore"), "ok\n[abc\n{a,b\n[]\nend\\\n").unwrap();
        fs::write(t.path().join("ok"), "x").unwrap();
        let root = t.path().canonicalize().unwrap();
        let r = IgnoreRules::load(&root, &root.join("src"), Path::new("src")).unwrap();
        let lines: Vec<_> = r.warnings.iter().map(|w| w.line.unwrap()).collect();
        assert_eq!(lines, [2, 3, 4, 5]);
        let w = &r.warnings[0];
        assert_eq!(w.code, codes::IGNORE_LINE_INVALID);
        assert!(matches!(w.severity, crate::problem::Severity::Warning));
        assert!(
            w.message
                .starts_with(".gitignore line 2 `[abc` is not a valid pattern"),
            "{}",
            w.message
        );
        assert!(w.message.ends_with("so it was skipped"), "{}", w.message);
        assert_eq!(w.path.as_deref(), Some(Path::new(".gitignore")));
        // The valid line still works.
        assert!(r.is_ignored(&root.join("ok"), false));
    }
}
