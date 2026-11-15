//! The build pipeline: discover, parse, resolve, render, write, copy.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use brrrty_markdown::MarkdownRenderer;
use rayon::prelude::*;

use crate::discover::discover;
use crate::frontmatter::split_front_matter;
use crate::options::BuildOptions;
use crate::page::{Page, file_slug, resolve_date};
use crate::passthrough::copy_passthrough_avoiding;
use crate::permalink::{Permalink, resolve_permalink};
use crate::problem::{Problem, Severity};

/// What a build did.
#[derive(Debug)]
pub struct BuildReport {
    pub written: usize,
    pub copied: usize,
    pub unchanged: usize,
    pub problems: Vec<Problem>,
    pub elapsed: Duration,
}

impl BuildReport {
    pub fn has_errors(&self) -> bool {
        self.problems
            .iter()
            .any(|p| matches!(p.severity, Severity::Error))
    }
}

/// True when `path` is lexically inside `base` and is not `base` itself.
pub(crate) fn is_inside(base: &Path, path: &Path) -> bool {
    let normalized = crate::options::normalize(path);
    normalized != base && normalized.starts_with(base)
}

/// Join `rel` onto `out`, refusing anything that is not a plain relative path.
fn contained_output_path(out: &Path, rel: &Path) -> Option<PathBuf> {
    if rel.as_os_str().is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let full = out.join(rel);
    is_inside(out, &full).then_some(full)
}

struct Prepared {
    page: Page,
    output_rel: PathBuf,
}

fn with_path_if_missing(p: Problem, rel: &Path) -> Problem {
    if p.path.is_some() {
        p
    } else {
        p.with_path(rel)
    }
}

fn load(input: &Path, rel: &Path) -> Result<Option<Prepared>, Problem> {
    let full = input.join(rel);
    let bytes = fs::read(&full).map_err(|e| {
        Problem::error("template-unreadable", format!("cannot read file: {e}")).with_path(rel)
    })?;
    let text = String::from_utf8(bytes).map_err(|_| {
        Problem::error("template-not-utf8", "file is not valid UTF-8")
            .with_path(rel)
            .with_help("save the file as UTF-8")
    })?;
    let (data, body) = split_front_matter(&text).map_err(|p| with_path_if_missing(p, rel))?;
    let created = fs::metadata(&full)
        .and_then(|m| m.created().or_else(|_| m.modified()))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let date = resolve_date(&data, rel, created).map_err(|p| with_path_if_missing(p, rel))?;
    let permalink = resolve_permalink(rel, &data).map_err(|p| with_path_if_missing(p, rel))?;
    let Permalink::Write { output_rel, .. } = permalink else {
        return Ok(None);
    };
    Ok(Some(Prepared {
        page: Page {
            input_path: rel.to_path_buf(),
            file_slug: file_slug(rel),
            date,
            data,
            body: body.to_string(),
        },
        output_rel,
    }))
}

/// Sort by path then code, and in strict mode keep only up to the first error.
fn finish(mut report: BuildReport, strict: bool, start: Instant) -> BuildReport {
    report.problems.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.code.cmp(b.code))
            .then_with(|| a.message.cmp(&b.message))
    });
    if strict
        && let Some(i) = report
            .problems
            .iter()
            .position(|p| matches!(p.severity, Severity::Error))
    {
        report.problems.truncate(i + 1);
    }
    report.elapsed = start.elapsed();
    report
}

/// Build the site described by `opts`.
pub fn build(opts: &BuildOptions, markdown: &dyn MarkdownRenderer) -> BuildReport {
    let start = Instant::now();
    let mut report = BuildReport {
        written: 0,
        copied: 0,
        unchanged: 0,
        problems: Vec::new(),
        elapsed: Duration::ZERO,
    };
    let strict = opts.strict;
    let has_error = |r: &BuildReport| r.has_errors();

    if let Err(p) = opts.validate() {
        report.problems.push(p);
        return finish(report, strict, start);
    }
    let discovery = match discover(opts) {
        Ok(d) => d,
        Err(p) => {
            report.problems.push(p);
            return finish(report, strict, start);
        }
    };
    for path in &discovery.unsupported {
        report.problems.push(
            Problem::error(
                "template-unsupported",
                "this template language is not supported yet",
            )
            .with_path(path)
            .with_help("Liquid and Nunjucks support is in progress."),
        );
    }
    if strict && has_error(&report) {
        return finish(report, strict, start);
    }

    let input = opts.input_dir();
    let out = opts.output_dir();

    let loaded: Vec<Result<Option<Prepared>, Problem>> = discovery
        .templates
        .par_iter()
        .map(|rel| load(&input, rel))
        .collect();
    let mut prepared = Vec::new();
    for item in loaded {
        match item {
            Ok(Some(p)) => prepared.push(p),
            Ok(None) => {}
            Err(p) => report.problems.push(p),
        }
    }
    if strict && has_error(&report) {
        return finish(report, strict, start);
    }

    // Detect outputs that two or more pages claim.
    let mut by_output: BTreeMap<&Path, Vec<&Path>> = BTreeMap::new();
    for p in &prepared {
        by_output
            .entry(p.output_rel.as_path())
            .or_default()
            .push(p.page.input_path.as_path());
    }
    let mut colliding: Vec<PathBuf> = Vec::new();
    for (output, inputs) in &by_output {
        if inputs.len() > 1 {
            let list: Vec<String> = inputs.iter().map(|i| i.display().to_string()).collect();
            report.problems.push(
                Problem::error(
                    "output-collision",
                    format!(
                        "{} pages write to {}: {}",
                        inputs.len(),
                        output.display(),
                        list.join(", ")
                    ),
                )
                .with_path(inputs[0])
                .with_help("give each page a unique permalink"),
            );
            colliding.push(output.to_path_buf());
        }
    }
    prepared.retain(|p| !colliding.contains(&p.output_rel));
    if strict && has_error(&report) {
        return finish(report, strict, start);
    }

    // Render and write.
    let results: Vec<Result<(), Problem>> = prepared
        .par_iter()
        .map(|p| {
            let rel = &p.page.input_path;
            let Some(dest) = contained_output_path(&out, &p.output_rel) else {
                return Err(Problem::error(
                    "output-escapes",
                    format!(
                        "{} would be written outside the output folder",
                        p.output_rel.display()
                    ),
                )
                .with_path(rel));
            };
            let is_markdown = rel
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"));
            let html = if is_markdown {
                markdown.render(&p.page.body)
            } else {
                p.page.body.clone()
            };
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    Problem::error("write-failed", format!("cannot create folder: {e}"))
                        .with_path(rel)
                })?;
            }
            fs::write(&dest, html).map_err(|e| {
                Problem::error("write-failed", format!("cannot write output: {e}")).with_path(rel)
            })
        })
        .collect();
    for r in results {
        match r {
            Ok(()) => report.written += 1,
            Err(p) => report.problems.push(p),
        }
    }
    if strict && has_error(&report) {
        return finish(report, strict, start);
    }

    let pages: HashMap<PathBuf, PathBuf> = prepared
        .iter()
        .map(|p| (out.join(&p.output_rel), p.page.input_path.clone()))
        .collect();
    let (copied, unchanged, problems) = copy_passthrough_avoiding(opts, &pages);
    // A passthrough file that replaced a page means that page was not really written.
    report.written -= problems
        .iter()
        .filter(|p| p.code == "passthrough-overrides-page")
        .count();
    report.copied = copied;
    report.unchanged = unchanged;
    report.problems.extend(problems);
    finish(report, strict, start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contained_output_path_accepts_plain_relative_paths() {
        let out = Path::new("/site/_site");
        assert_eq!(
            contained_output_path(out, Path::new("a/b/index.html")),
            Some(PathBuf::from("/site/_site/a/b/index.html"))
        );
    }

    #[test]
    fn contained_output_path_refuses_escapes() {
        let out = Path::new("/site/_site");
        for bad in ["../x.html", "a/../../x.html", "/etc/passwd", "", "."] {
            assert_eq!(contained_output_path(out, Path::new(bad)), None, "{bad}");
        }
    }
}
