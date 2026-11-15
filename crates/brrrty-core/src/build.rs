//! The build pipeline: discover, parse, resolve, render, write, copy.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use brrrty_markdown::MarkdownRenderer;
use rayon::prelude::*;

use crate::discover::discover;
use crate::frontmatter::split_front_matter;
use crate::options::{BuildOptions, root_relative};
use crate::page::{Page, file_slug, resolve_date};
use crate::passthrough::copy_passthrough_avoiding;
use crate::permalink::{Permalink, resolve_permalink};
use crate::problem::{Problem, Severity, codes};

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

/// Load one template. Problems carry paths relative to the project root.
fn load(input: &Path, input_rel: &Path, rel: &Path) -> Result<Option<Prepared>, Problem> {
    load_page(input, input_rel, rel).map_err(|mut p| {
        // Problems from here are relative to the input folder.
        p.path = Some(input_rel.join(p.path.as_deref().unwrap_or(rel)));
        p
    })
}

fn load_page(input: &Path, input_rel: &Path, rel: &Path) -> Result<Option<Prepared>, Problem> {
    let full = input.join(rel);
    let bytes = fs::read(&full).map_err(|e| {
        Problem::error(codes::TEMPLATE_UNREADABLE, format!("cannot read file: {e}")).with_path(rel)
    })?;
    let text = String::from_utf8(bytes).map_err(|_| {
        Problem::error(codes::TEMPLATE_NOT_UTF8, "file is not valid UTF-8")
            .with_path(rel)
            .with_help("save the file as UTF-8")
    })?;
    let (data, body) = split_front_matter(&text).map_err(|p| with_path_if_missing(p, rel))?;
    let meta = fs::metadata(&full).ok();
    let modified = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let created = meta
        .as_ref()
        .and_then(|m| m.created().ok())
        .unwrap_or(modified);
    let date = resolve_date(&data, &input_rel.join(rel), created, modified)
        .map_err(|p| with_path_if_missing(p, rel))?;
    let permalink = resolve_permalink(rel, &data).map_err(|p| with_path_if_missing(p, rel))?;
    let Permalink::Write { output_rel, .. } = permalink else {
        return Ok(None);
    };
    reject_unsupported_features(&text, &data, body, rel)?;
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

/// Front matter keys that change a page's output in ways Brrrty can't reproduce yet.
const UNSUPPORTED_KEYS: &[&str] = &["layout", "pagination", "eleventyComputed"];

/// Refuse pages that 11ty would process further than Brrrty can yet, instead of writing
/// output that silently differs.
fn reject_unsupported_features(
    text: &str,
    data: &serde_json::Map<String, serde_json::Value>,
    body: &str,
    rel: &Path,
) -> Result<(), Problem> {
    let keys: Vec<String> = UNSUPPORTED_KEYS
        .iter()
        .filter(|k| data.contains_key(**k))
        .map(|k| format!("`{k}`"))
        .collect();
    if !keys.is_empty() {
        let message = if keys.len() == 1 {
            format!("Front matter key {} isn't supported yet", keys[0])
        } else {
            format!("Front matter keys {} aren't supported yet", keys.join(", "))
        };
        return Err(Problem::error(codes::FRONT_MATTER_UNSUPPORTED, message)
            .with_path(rel)
            .with_help("Layouts, pagination, and computed data are in progress."));
    }
    if let Some(at) = body.find("{{").into_iter().chain(body.find("{%")).min() {
        // `body` is the tail of the file, so its start is where the file text ends minus its length.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let upto = &text[..text.len() - body.len() + at];
        let line = upto.matches('\n').count() + 1;
        let column = upto.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        return Err(Problem::error(
            codes::TEMPLATE_SYNTAX_UNSUPPORTED,
            "Template syntax ({{ or {%) isn't supported yet",
        )
        .with_path(rel)
        .at(line, column)
        .with_help("Liquid and Nunjucks support is in progress."));
    }
    Ok(())
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
    if let Ok(root) = opts.root_dir() {
        for name in [
            "eleventy.config.js",
            "eleventy.config.cjs",
            "eleventy.config.mjs",
            ".eleventy.js",
        ] {
            if root.join(name).is_file() {
                report.problems.push(
                    Problem::warning(
                        codes::ELEVENTY_CONFIG_IGNORED,
                        "Brrrty doesn't read your 11ty config yet",
                    )
                    .with_path(name)
                    .with_help(
                        "Pass your input folder and passthrough paths as flags, like --input src --passthrough src/images.",
                    ),
                );
            }
        }
    }
    let (root, input, out) = match (opts.root_dir(), opts.input_dir(), opts.output_dir()) {
        (Ok(r), Ok(i), Ok(o)) => (r, i, o),
        (Err(p), _, _) | (_, Err(p), _) | (_, _, Err(p)) => {
            report.problems.push(p);
            return finish(report, strict, start);
        }
    };
    // Pages are tracked relative to the input folder; problems report them relative to the root.
    let input_rel = root_relative(&root, &input);

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
                codes::TEMPLATE_UNSUPPORTED,
                "this template language is not supported yet",
            )
            .with_path(input_rel.join(path))
            .with_help("Liquid and Nunjucks support is in progress."),
        );
    }
    if strict && has_error(&report) {
        return finish(report, strict, start);
    }

    let loaded: Vec<Result<Option<Prepared>, Problem>> = discovery
        .templates
        .par_iter()
        .map(|rel| load(&input, &input_rel, rel))
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
            let list: Vec<String> = inputs
                .iter()
                .map(|i| input_rel.join(i).display().to_string())
                .collect();
            report.problems.push(
                Problem::error(
                    codes::OUTPUT_COLLISION,
                    format!(
                        "{} pages write to {}: {}",
                        inputs.len(),
                        output.display(),
                        list.join(", ")
                    ),
                )
                .with_path(input_rel.join(inputs[0]))
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
                    codes::OUTPUT_ESCAPES,
                    format!(
                        "{} would be written outside the output folder",
                        p.output_rel.display()
                    ),
                )
                .with_path(input_rel.join(rel)));
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
                    Problem::error(codes::WRITE_FAILED, format!("cannot create folder: {e}"))
                        .with_path(input_rel.join(rel))
                })?;
            }
            fs::write(&dest, html).map_err(|e| {
                Problem::error(codes::WRITE_FAILED, format!("cannot write output: {e}"))
                    .with_path(input_rel.join(rel))
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
        .map(|p| (out.join(&p.output_rel), input_rel.join(&p.page.input_path)))
        .collect();
    let pt = copy_passthrough_avoiding(opts, &pages);
    // A passthrough file that replaced a page means that page was not really written.
    report.written = report.written.saturating_sub(pt.overridden.len());
    report.copied = pt.copied;
    report.unchanged = pt.unchanged;
    report.problems.extend(pt.problems);
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
