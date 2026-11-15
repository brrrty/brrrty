//! Comparison results and their Markdown rendering.

use std::fmt::Write;
use std::path::PathBuf;

/// How one file compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Identical,
    Equivalent,
    Different { diff: String },
    Missing,
    Extra,
}

/// The outcome for one file, with its path relative to the compared roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileResult {
    pub path: PathBuf,
    pub outcome: Outcome,
}

/// Tally of outcomes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub identical: usize,
    pub equivalent: usize,
    pub different: usize,
    pub missing: usize,
    pub extra: usize,
}

/// Results for every file in either folder.
#[derive(Debug, Clone, Default)]
pub struct CompareReport {
    pub results: Vec<FileResult>,
}

impl CompareReport {
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for r in &self.results {
            match r.outcome {
                Outcome::Identical => c.identical += 1,
                Outcome::Equivalent => c.equivalent += 1,
                Outcome::Different { .. } => c.different += 1,
                Outcome::Missing => c.missing += 1,
                Outcome::Extra => c.extra += 1,
            }
        }
        c
    }

    /// True when nothing is different, missing or extra.
    pub fn is_pass(&self) -> bool {
        let c = self.counts();
        c.different + c.missing + c.extra == 0
    }

    /// Summary line, then one section per non-passing file with diffs cut to `max_diff_lines`.
    pub fn to_markdown(&self, max_diff_lines: usize) -> String {
        let c = self.counts();
        let mut out = format!(
            "identical: {}, equivalent: {}, different: {}, missing: {}, extra: {}\n",
            c.identical, c.equivalent, c.different, c.missing, c.extra
        );
        for r in &self.results {
            let path = r.path.display();
            match &r.outcome {
                Outcome::Different { diff } => {
                    let lines: Vec<&str> = diff.lines().collect();
                    let _ = write!(out, "\n## {path}\n\n```diff\n");
                    for l in lines.iter().take(max_diff_lines) {
                        let _ = writeln!(out, "{l}");
                    }
                    out.push_str("```\n");
                    if lines.len() > max_diff_lines {
                        let _ = writeln!(out, "... ({} more lines)", lines.len() - max_diff_lines);
                    }
                }
                Outcome::Missing => {
                    let _ = write!(out, "\n## {path}\n\nmissing from actual output\n");
                }
                Outcome::Extra => {
                    let _ = write!(out, "\n## {path}\n\nextra in actual output\n");
                }
                Outcome::Identical | Outcome::Equivalent => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_diffs() {
        let report = CompareReport {
            results: vec![FileResult {
                path: "a.html".into(),
                outcome: Outcome::Different {
                    diff: "1\n2\n3\n4\n5".into(),
                },
            }],
        };
        let md = report.to_markdown(2);
        assert!(md.contains("## a.html"));
        assert!(md.contains("... (3 more lines)"));
        assert!(!md.contains("\n4\n"));
    }
}
