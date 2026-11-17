//! `brrrty doctor --verify`: build the site with its own 11ty and with
//! Brrrty, at one frozen time, and compare every file.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use brrrty_compat::{CompareReport, Outcome};
use brrrty_core::doctor::thousands;
use serde_json::{Value, json};

/// The preload script that freezes 11ty's clock at `SOURCE_DATE_EPOCH`, the
/// way Brrrty reads it: the same file the compat harness uses.
pub const SHIM: &str = include_str!("freeze-time.cjs");

/// Diffs shown in the text output, and lines of each.
const SHOWN_DIFFS: usize = 5;
const DIFF_LINES: usize = 20;
/// Missing or extra files named in the text output.
const SHOWN_FILES: usize = 10;

/// The site's own 11ty, ready to run.
#[derive(Debug, Clone)]
pub struct Eleventy {
    pub node: PathBuf,
    /// `node_modules/@11ty/eleventy/cmd.cjs`.
    pub cmd: PathBuf,
    pub version: String,
}

/// Find Node and the site's 11ty, or say how to install what's missing.
pub fn find(root: &Path) -> Result<Eleventy, String> {
    let node = PathBuf::from("node");
    let works = Command::new(&node)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !works {
        return Err("--verify runs your site's 11ty build, which needs Node.js, and `node` isn't on your PATH. Install Node.js (https://nodejs.org), then run `brrrty doctor --verify` again.".to_string());
    }
    let pkg_dir = root.join("node_modules/@11ty/eleventy");
    let cmd = pkg_dir.join("cmd.cjs");
    if !cmd.is_file() {
        let listed = fs::read_to_string(root.join("package.json"))
            .is_ok_and(|s| s.contains("\"@11ty/eleventy\""));
        let how = if listed {
            "Run `npm install` (or `npm ci`) here first"
        } else {
            "Install it with `npm install --save-dev @11ty/eleventy`"
        };
        return Err(format!(
            "--verify runs your site's own 11ty build, but 11ty isn't installed here (node_modules/@11ty/eleventy is missing). {how}, then run `brrrty doctor --verify` again."
        ));
    }
    let version = fs::read_to_string(pkg_dir.join("package.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("version")?.as_str().map(str::to_string))
        .unwrap_or_else(|| "(unknown version)".to_string());
    Ok(Eleventy { node, cmd, version })
}

/// Run 11ty in `root`, writing to `out`, with its clock frozen at `epoch` by
/// the shim at `shim`. Returns how long it took.
pub fn run_eleventy(
    e: &Eleventy,
    root: &Path,
    out: &Path,
    shim: &Path,
    epoch: u64,
) -> Result<Duration, String> {
    let start = Instant::now();
    let output = Command::new(&e.node)
        .arg("--require")
        .arg(shim)
        .arg(&e.cmd)
        .arg("--quiet")
        .arg(format!("--output={}", out.display()))
        .env("SOURCE_DATE_EPOCH", epoch.to_string())
        .current_dir(root)
        .output()
        .map_err(|err| format!("cannot run node: {err}"))?;
    let took = start.elapsed();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first: Vec<&str> = stderr.lines().take(20).collect();
        return Err(format!(
            "your site's 11ty build failed ({}), so there is nothing to compare. Its first lines of output:\n{}",
            output.status,
            first.join("\n")
        ));
    }
    Ok(took)
}

/// The comparison and both timings.
#[derive(Debug)]
pub struct Verified {
    pub eleventy_version: String,
    pub eleventy_time: Duration,
    pub brrrty_time: Duration,
    pub report: CompareReport,
}

impl Verified {
    /// Files in either output.
    pub fn files(&self) -> usize {
        self.report.results.len()
    }

    /// Every file is byte-identical.
    pub fn passes(&self) -> bool {
        let c = self.report.counts();
        c.identical == self.files()
    }

    /// Different, including files that differ only in whitespace.
    fn different(&self) -> usize {
        let c = self.report.counts();
        c.different + c.equivalent
    }

    /// The one line to paste into an issue or a post.
    pub fn verdict(&self) -> String {
        let c = self.report.counts();
        if self.passes() {
            format!(
                "Your site is byte-compatible on Brrrty: {} files, {} vs {}",
                thousands(self.files()),
                seconds(self.brrrty_time),
                seconds(self.eleventy_time)
            )
        } else {
            format!(
                "Not byte-compatible yet: {} different, {} missing, {} extra, of {} files",
                thousands(self.different()),
                thousands(c.missing),
                thousands(c.extra),
                thousands(self.files())
            )
        }
    }
}

/// `0.85 s`.
pub fn seconds(d: Duration) -> String {
    format!("{:.2} s", d.as_secs_f64())
}

/// The text report.
pub fn text(v: &Verified) -> String {
    let c = v.report.counts();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Built your site with 11ty {} and with Brrrty {}, then compared every file.",
        v.eleventy_version,
        env!("CARGO_PKG_VERSION")
    );
    let whitespace = if c.equivalent > 0 {
        format!(" ({} only in whitespace)", thousands(c.equivalent))
    } else {
        String::new()
    };
    let _ = writeln!(
        out,
        "  {} files: {} byte-identical, {} different{whitespace}, {} missing, {} extra",
        thousands(v.files()),
        thousands(c.identical),
        thousands(v.different()),
        thousands(c.missing),
        thousands(c.extra)
    );
    let ratio = v.eleventy_time.as_secs_f64() / v.brrrty_time.as_secs_f64().max(0.001);
    let _ = writeln!(
        out,
        "  11ty: {}   Brrrty: {}   ({ratio:.1}x faster)",
        seconds(v.eleventy_time),
        seconds(v.brrrty_time)
    );
    let mut diffs = 0;
    let mut missing = Vec::new();
    let mut extra = Vec::new();
    for r in &v.report.results {
        match &r.outcome {
            Outcome::Different { diff } => {
                diffs += 1;
                if diffs <= SHOWN_DIFFS {
                    let _ = writeln!(out, "\nDifferent: {}", r.path.display());
                    let lines: Vec<&str> = diff.lines().collect();
                    for l in lines.iter().take(DIFF_LINES) {
                        let _ = writeln!(out, "  {l}");
                    }
                    if lines.len() > DIFF_LINES {
                        let _ = writeln!(out, "  ... ({} more lines)", lines.len() - DIFF_LINES);
                    }
                }
            }
            Outcome::Equivalent => {
                diffs += 1;
                if diffs <= SHOWN_DIFFS {
                    let _ = writeln!(out, "\nDifferent only in whitespace: {}", r.path.display());
                }
            }
            Outcome::Missing => missing.push(&r.path),
            Outcome::Extra => extra.push(&r.path),
            Outcome::Identical => {}
        }
    }
    if diffs > SHOWN_DIFFS {
        let _ = writeln!(
            out,
            "\n... and {} more different files (--json lists every one)",
            diffs - SHOWN_DIFFS
        );
    }
    for (label, list) in [
        ("Missing (11ty wrote them, Brrrty didn't)", &missing),
        ("Extra (Brrrty wrote them, 11ty didn't)", &extra),
    ] {
        if list.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\n{label}:");
        for p in list.iter().take(SHOWN_FILES) {
            let _ = writeln!(out, "  {}", p.display());
        }
        if list.len() > SHOWN_FILES {
            let _ = writeln!(out, "  ... and {} more", list.len() - SHOWN_FILES);
        }
    }
    let _ = writeln!(out, "\n{}", v.verdict());
    out
}

/// The JSON report.
pub fn json(v: &Verified) -> Value {
    let c = v.report.counts();
    let files: Vec<Value> = v
        .report
        .results
        .iter()
        .filter_map(|r| {
            let (outcome, diff) = match &r.outcome {
                Outcome::Identical => return None,
                Outcome::Equivalent => ("whitespace", None),
                Outcome::Different { diff } => ("different", Some(diff)),
                Outcome::Missing => ("missing", None),
                Outcome::Extra => ("extra", None),
            };
            Some(json!({ "path": r.path, "outcome": outcome, "diff": diff }))
        })
        .collect();
    json!({
        "eleventy": v.eleventy_version,
        "files": v.files(),
        "identical": c.identical,
        "different": v.different(),
        "missing": c.missing,
        "extra": c.extra,
        "eleventy_seconds": v.eleventy_time.as_secs_f64(),
        "brrrty_seconds": v.brrrty_time.as_secs_f64(),
        "byte_compatible": v.passes(),
        "verdict": v.verdict(),
        "differences": files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use brrrty_compat::FileResult;

    fn verified(outcomes: Vec<Outcome>) -> Verified {
        Verified {
            eleventy_version: "3.1.8".into(),
            eleventy_time: Duration::from_millis(2320),
            brrrty_time: Duration::from_millis(850),
            report: CompareReport {
                results: outcomes
                    .into_iter()
                    .enumerate()
                    .map(|(i, outcome)| FileResult {
                        path: format!("f{i}.html").into(),
                        outcome,
                    })
                    .collect(),
            },
        }
    }

    #[test]
    fn the_pasteable_line() {
        let v = verified(vec![Outcome::Identical; 2272]);
        assert!(v.passes());
        assert_eq!(
            v.verdict(),
            "Your site is byte-compatible on Brrrty: 2,272 files, 0.85 s vs 2.32 s"
        );
    }

    #[test]
    fn whitespace_is_a_difference() {
        let v = verified(vec![
            Outcome::Identical,
            Outcome::Equivalent,
            Outcome::Missing,
        ]);
        assert!(!v.passes());
        assert_eq!(
            v.verdict(),
            "Not byte-compatible yet: 1 different, 1 missing, 0 extra, of 3 files"
        );
        assert!(text(&v).contains("1 different (1 only in whitespace)"));
    }
}
