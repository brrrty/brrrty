//! `brrrty doctor --verify`: build the site with its own 11ty and with
//! Brrrty, at one frozen time, and compare every file.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use std::collections::BTreeSet;

use brrrty_compat::{CompareReport, FileResult, Outcome};
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
    let pkg: Option<Value> = fs::read_to_string(pkg_dir.join("package.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    // The command the package installs: `cmd.cjs` in 11ty 3, `cmd.js` before.
    let bin = pkg.as_ref().and_then(|p| match p.get("bin")? {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o
            .get("eleventy")
            .or_else(|| o.values().next())?
            .as_str()
            .map(str::to_string),
        _ => None,
    });
    let cmd = pkg_dir.join(bin.as_deref().unwrap_or("cmd.cjs"));
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
    let version = pkg
        .as_ref()
        .and_then(|v| v.get("version")?.as_str().map(str::to_string))
        .unwrap_or_else(|| "(unknown version)".to_string());
    Ok(Eleventy { node, cmd, version })
}

/// Run 11ty in `root`, writing to `out`, with its clock frozen at `epoch` by
/// the shim at `shim`. Returns how long it took. It runs in a process group of its own,
/// which a signal kills (see [`crate::cleanup`]); it isn't started once one came.
pub fn run_eleventy(
    e: &Eleventy,
    root: &Path,
    out: &Path,
    shim: &Path,
    epoch: u64,
) -> Result<Duration, String> {
    let start = Instant::now();
    let mut cmd = Command::new(&e.node);
    cmd.arg("--require")
        .arg(shim)
        .arg(&e.cmd)
        .arg("--quiet")
        .arg(format!("--output={}", out.display()))
        .env("SOURCE_DATE_EPOCH", epoch.to_string())
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let Some(child) =
        crate::cleanup::spawn(&mut cmd).map_err(|err| format!("cannot run node: {err}"))?
    else {
        return Err("stopped before 11ty started".to_string());
    };
    let output = child.wait_with_output();
    crate::cleanup::child_done();
    let output = output.map_err(|err| format!("cannot run node: {err}"))?;
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
    /// Output files of random pages (that call a random filter, or show
    /// such a page's output), relative to the output folder: 11ty writes
    /// them differently on every build, so they can't be compared.
    pub random: BTreeSet<PathBuf>,
    /// Those of them a random filter placed: 11ty writes them at another
    /// path on every build.
    pub random_paths: BTreeSet<PathBuf>,
}

/// The files of each outcome, random pages' files apart.
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    identical: usize,
    random: usize,
    /// Different, including only in whitespace.
    different: usize,
    whitespace: usize,
    missing: usize,
    extra: usize,
}

impl Verified {
    /// Files in either output.
    pub fn files(&self) -> usize {
        self.report.results.len()
    }

    /// Files 11ty wrote at a path Brrrty didn't, which can be the random
    /// pages' own: when no more of them are missing than random paths
    /// Brrrty wrote that 11ty didn't. Which is which can't be told, so
    /// either all of them count as random, or none.
    fn missing_random(&self) -> bool {
        let (mut missing, mut random_extra) = (0, 0);
        for r in &self.report.results {
            match r.outcome {
                Outcome::Missing => missing += 1,
                Outcome::Extra if self.random_paths.contains(&r.path) => random_extra += 1,
                _ => {}
            }
        }
        random_extra > 0 && missing <= random_extra
    }

    /// Whether `r` is a random page's file, not compared.
    fn is_random(&self, r: &FileResult, missing_random: bool) -> bool {
        match r.outcome {
            Outcome::Different { .. } | Outcome::Equivalent => self.random.contains(&r.path),
            Outcome::Extra => self.random_paths.contains(&r.path),
            Outcome::Missing => missing_random,
            Outcome::Identical => false,
        }
    }

    fn tally(&self) -> Tally {
        let missing_random = self.missing_random();
        let mut t = Tally::default();
        for r in &self.report.results {
            if self.is_random(r, missing_random) {
                t.random += 1;
                continue;
            }
            match r.outcome {
                Outcome::Identical => t.identical += 1,
                Outcome::Equivalent => {
                    t.different += 1;
                    t.whitespace += 1;
                }
                Outcome::Different { .. } => t.different += 1,
                Outcome::Missing => t.missing += 1,
                Outcome::Extra => t.extra += 1,
            }
        }
        t
    }

    /// The files compared, apart from random pages'.
    fn compared(&self) -> impl Iterator<Item = &FileResult> {
        let missing_random = self.missing_random();
        self.report
            .results
            .iter()
            .filter(move |r| !self.is_random(r, missing_random))
    }

    /// The random pages' files, not compared.
    fn random_files(&self) -> Vec<&Path> {
        let missing_random = self.missing_random();
        self.report
            .results
            .iter()
            .filter(|r| self.is_random(r, missing_random))
            .map(|r| r.path.as_path())
            .collect()
    }

    /// Every file compared is byte-identical. Random pages' files aren't
    /// compared.
    pub fn passes(&self) -> bool {
        let t = self.tally();
        t.identical + t.random == self.files()
    }

    /// The one line to paste into an issue or a post.
    pub fn verdict(&self) -> String {
        let t = self.tally();
        if !self.passes() {
            return format!(
                "Not byte-compatible yet: {} different, {} missing, {} extra, of {} files",
                thousands(t.different),
                thousands(t.missing),
                thousands(t.extra),
                thousands(self.files())
            );
        }
        if t.random == 0 {
            return format!(
                "Your site is byte-compatible on Brrrty: {} files, {} vs {}",
                thousands(self.files()),
                seconds(self.brrrty_time),
                seconds(self.eleventy_time)
            );
        }
        let skipped = if t.random == 1 {
            "1 file wasn't compared: it is a random page, which 11ty writes differently on every build".to_string()
        } else {
            format!(
                "{} files weren't compared: they are random pages, which 11ty writes differently on every build",
                thousands(t.random)
            )
        };
        format!(
            "Every file compared is byte-identical: {} of {} files, {} vs {}. {skipped}.",
            thousands(t.identical),
            thousands(self.files()),
            seconds(self.brrrty_time),
            seconds(self.eleventy_time),
        )
    }
}

/// `0.85 s`.
pub fn seconds(d: Duration) -> String {
    format!("{:.2} s", d.as_secs_f64())
}

/// The text report.
pub fn text(v: &Verified) -> String {
    let c = v.tally();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Built your site with 11ty {} and with Brrrty {}, then compared every file.",
        v.eleventy_version,
        env!("CARGO_PKG_VERSION")
    );
    let whitespace = if c.whitespace > 0 {
        format!(" ({} only in whitespace)", thousands(c.whitespace))
    } else {
        String::new()
    };
    let random = match c.random {
        0 => String::new(),
        n => format!(
            ", {} random, not compared (11ty writes {} differently on every build)",
            thousands(n),
            if n == 1 { "it" } else { "them" }
        ),
    };
    let _ = writeln!(
        out,
        "  {} files: {} byte-identical{random}, {} different{whitespace}, {} missing, {} extra",
        thousands(v.files()),
        thousands(c.identical),
        thousands(c.different),
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
    for r in v.compared() {
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
    let c = v.tally();
    let files: Vec<Value> = v
        .compared()
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
        "different": c.different,
        "random": v.random_files(),
        "missing": c.missing,
        "extra": c.extra,
        "eleventy_seconds": v.eleventy_time.as_secs_f64(),
        "brrrty_seconds": v.brrrty_time.as_secs_f64(),
        "byte_compatible": v.passes() && c.random == 0,
        "compared_identical": v.passes(),
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
            random: BTreeSet::new(),
            random_paths: BTreeSet::new(),
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

    #[test]
    fn a_random_page_that_differs_is_not_compared_and_the_verdict_says_so() {
        let mut v = verified(vec![
            Outcome::Identical,
            Outcome::Different {
                diff: "-a\n+b".into(),
            },
        ]);
        assert!(!v.passes());
        v.random.insert("f1.html".into());
        assert!(v.passes());
        let verdict = v.verdict();
        assert!(!verdict.contains("byte-compatible"), "{verdict}");
        assert_eq!(
            verdict,
            "Every file compared is byte-identical: 1 of 2 files, 0.85 s vs 2.32 s. 1 file wasn't compared: it is a random page, which 11ty writes differently on every build."
        );
        let t = text(&v);
        assert!(
            t.contains("1 byte-identical, 1 random, not compared"),
            "{t}"
        );
        assert!(!t.contains("Different: f1.html"), "{t}");
        let j = json(&v);
        assert_eq!(j["random"], serde_json::json!(["f1.html"]));
        assert_eq!(j["byte_compatible"], false);
        assert_eq!(j["compared_identical"], true);
        // A file random pages didn't write still counts.
        v.random.clear();
        v.random.insert("f9.html".into());
        assert!(!v.passes());
    }

    #[test]
    fn a_random_page_that_differs_only_in_whitespace_isnt_counted_as_whitespace() {
        let mut v = verified(vec![Outcome::Equivalent, Outcome::Equivalent]);
        v.random.insert("f0.html".into());
        let t = text(&v);
        assert!(t.contains("1 different (1 only in whitespace)"), "{t}");
        v.random.insert("f1.html".into());
        let t = text(&v);
        assert!(t.contains("0 different, 0 missing"), "{t}");
    }

    #[test]
    fn a_random_path_pairs_a_missing_file_with_an_extra_one() {
        // 11ty wrote f0 and Brrrty f1, each at the path its draw gave.
        let mut v = verified(vec![Outcome::Missing, Outcome::Extra, Outcome::Identical]);
        assert!(!v.passes());
        v.random.insert("f1.html".into());
        v.random_paths.insert("f1.html".into());
        assert!(v.passes(), "{}", text(&v));
        assert_eq!(
            json(&v)["random"],
            serde_json::json!(["f0.html", "f1.html"])
        );
        // More missing than random paths: which is which can't be told.
        let mut v = verified(vec![Outcome::Missing, Outcome::Missing, Outcome::Extra]);
        v.random_paths.insert("f2.html".into());
        assert!(!v.passes());
        assert!(text(&v).contains("2 missing, 0 extra"), "{}", text(&v));
    }
}
