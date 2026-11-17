//! `brrrty doctor`: what a [`survey`](crate::survey()) build found, grouped
//! by root cause.
//!
//! Every problem that refused a page gets a cause: a filter Brrrty doesn't
//! have, a JavaScript data file to port, a template language Brrrty doesn't
//! run, or, for anything else, the problem itself. Problems that only follow
//! from others (a page that needs pages refused for their own reasons) are
//! grouped last and are not counted as blockers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::build::BuildReport;
use crate::data::js_help;
use crate::problem::{Problem, Severity, codes};

/// Why pages are refused.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Cause {
    /// A filter neither Brrrty nor the config defines.
    Filter(String),
    /// A JavaScript data file, which Brrrty doesn't run.
    JsData(PathBuf),
    /// A template language Brrrty doesn't run, by extension (`11ty.js`).
    TemplateLanguage(String),
    /// Anything else: the problem's code and message.
    Other { code: &'static str, message: String },
    /// Refused because of other blockers.
    Follows,
}

/// Where a cause shows up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// Relative to the project root.
    pub path: Option<PathBuf>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub message: String,
}

/// Every page refused for one cause.
#[derive(Debug, Clone)]
pub struct Group {
    pub cause: Cause,
    /// One line naming the cause.
    pub title: String,
    /// How to fix it.
    pub help: Option<String>,
    /// Pages refused for it (among others, perhaps).
    pub pages: usize,
    /// The templates of those pages, relative to the project root.
    pub templates: Vec<PathBuf>,
    /// Every place it shows up.
    pub locations: Vec<Location>,
}

impl Group {
    /// Whether it is a blocker of its own, not one that follows from others.
    pub fn is_blocker(&self) -> bool {
        self.cause != Cause::Follows
    }
}

/// The doctor's findings.
#[derive(Debug)]
pub struct Diagnosis {
    /// Pages the build writes.
    pub building: usize,
    /// Pages the build writes or refuses. A template Brrrty can't expand
    /// into pages counts as one.
    pub pages: usize,
    /// Blockers first, by pages affected; then what follows from them.
    pub groups: Vec<Group>,
    /// Warnings and notes, which stop nothing.
    pub notes: Vec<Problem>,
    /// The build's own report.
    pub report: BuildReport,
}

impl Diagnosis {
    /// The number of blockers: causes, not counting what follows from them.
    pub fn blockers(&self) -> usize {
        self.groups.iter().filter(|g| g.is_blocker()).count()
    }

    /// Whether everything builds: `brrrty build` would report no error.
    pub fn builds(&self) -> bool {
        !self.report.has_errors()
    }

    /// `N of M pages will build; K blockers, grouped below.`
    pub fn verdict(&self) -> String {
        let pages =
            |n: usize| format!("{} {}", thousands(n), if n == 1 { "page" } else { "pages" });
        if self.builds() {
            return if self.pages == 1 {
                "The 1 page will build.".to_string()
            } else {
                format!("All {} will build.", pages(self.pages))
            };
        }
        let k = self.blockers();
        format!(
            "{} of {} will build; {} {}, grouped below.",
            thousands(self.building),
            pages(self.pages),
            thousands(k),
            if k == 1 { "blocker" } else { "blockers" }
        )
    }
}

/// `2272` as `2,272`.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn is_js_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("js" | "cjs" | "mjs")
    )
}

/// The first word after `prefix` in `message`: a filter's name, without
/// what an include adds after it (` while rendering ...`).
fn word_after<'a>(message: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = message.strip_prefix(prefix)?;
    rest.split_whitespace().next()
}

/// The template language of a template Brrrty doesn't run.
fn template_language(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    for multi in ["11ty.js", "11ty.cjs", "11ty.mjs"] {
        if name.ends_with(&format!(".{multi}")) {
            return multi.to_string();
        }
    }
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// The causes of `p`. `js_files` are the JavaScript data files the build
/// met, to find the ones a problem names.
pub fn causes(p: &Problem, js_files: &BTreeSet<PathBuf>) -> Vec<Cause> {
    if p.follows {
        return vec![Cause::Follows];
    }
    let filter = match p.code {
        codes::LIQUID_UNKNOWN_FILTER => word_after(&p.message, "undefined filter: "),
        codes::NUNJUCKS_RENDER => word_after(&p.message, "Error: filter not found: "),
        _ => None,
    };
    if let Some(name) = filter {
        return vec![Cause::Filter(name.to_string())];
    }
    if p.code == codes::DATA_JS_UNSUPPORTED {
        let mut files: Vec<Cause> = Vec::new();
        if let Some(path) = &p.path
            && is_js_file(path)
        {
            files.push(Cause::JsData(path.clone()));
        }
        for f in js_files {
            let named = format!("{}", f.display());
            if p.message.contains(&named) && !files.contains(&Cause::JsData(f.clone())) {
                files.push(Cause::JsData(f.clone()));
            }
        }
        if !files.is_empty() {
            return files;
        }
    }
    if p.code == codes::TEMPLATE_UNSUPPORTED
        && let Some(path) = &p.path
    {
        return vec![Cause::TemplateLanguage(template_language(path))];
    }
    vec![Cause::Other {
        code: p.code,
        message: p.message.clone(),
    }]
}

fn title_and_help(cause: &Cause, first: &Problem) -> (String, Option<String>) {
    match cause {
        Cause::Filter(name) => (
            format!("Filter `{name}` isn't defined"),
            Some(format!(
                "Port it to brrrty.config.rhai with `config.add_filter(\"{name}\", |value| ...)`; PORTING.md says how."
            )),
        ),
        Cause::JsData(file) => (
            format!("JavaScript data file {}", file.display()),
            Some(js_help(file)),
        ),
        Cause::TemplateLanguage(ext) => (
            format!("`.{ext}` templates"),
            first.help.clone(),
        ),
        Cause::Other { code, message } => (format!("{code}: {message}"), first.help.clone()),
        Cause::Follows => (
            "Waiting on the blockers above".to_string(),
            Some("These pages need pages or data that the blockers above refuse; they build once those are fixed.".to_string()),
        ),
    }
}

/// The key that tells two reports of one problem apart from two problems.
fn key(
    p: &Problem,
) -> (
    &'static str,
    Option<&Path>,
    Option<usize>,
    Option<usize>,
    &str,
) {
    (p.code, p.path.as_deref(), p.line, p.column, &p.message)
}

#[derive(Default)]
struct Building {
    first: Option<Problem>,
    pages: usize,
    templates: BTreeSet<PathBuf>,
    locations: Vec<Location>,
}

/// Group what a survey build found by cause.
///
/// # Panics
///
/// Never; a report without a survey has no templates to group, and every
/// error it has is grouped on its own.
pub fn diagnose(report: BuildReport) -> Diagnosis {
    let empty = crate::survey::Survey::default();
    let survey = report.survey.as_ref().unwrap_or(&empty);
    let mut js_files: BTreeSet<PathBuf> = survey.js_keys.values().cloned().collect();
    for t in survey.templates.values() {
        for p in &t.problems {
            if let Some(path) = &p.path
                && p.code == codes::DATA_JS_UNSUPPORTED
                && is_js_file(path)
            {
                js_files.insert(path.clone());
            }
        }
    }
    let mut groups: BTreeMap<Cause, Building> = BTreeMap::new();
    let mut attributed = BTreeSet::new();
    let mut add = |cause: Cause, p: &Problem, template: Option<(&PathBuf, usize)>| {
        let g = groups.entry(cause).or_default();
        if g.first.is_none() {
            g.first = Some(p.clone());
        }
        if let Some((t, pages)) = template
            && g.templates.insert(t.clone())
        {
            g.pages += pages;
        }
        g.locations.push(Location {
            path: p.path.clone(),
            line: p.line,
            column: p.column,
            message: p.message.clone(),
        });
    };
    let mut refused_pages = 0;
    for (shown, t) in survey.refused() {
        refused_pages += t.refused;
        for p in &t.problems {
            attributed.insert(key(p));
            for c in causes(p, &js_files) {
                add(c, p, Some((shown, t.refused)));
            }
        }
    }
    // Errors no refused template explains still stop the build.
    let mut notes = Vec::new();
    for p in &report.problems {
        if attributed.contains(&key(p)) {
            continue;
        }
        if matches!(p.severity, Severity::Error) {
            let refused_template = p
                .path
                .as_ref()
                .is_some_and(|path| survey.templates.get(path).is_some_and(|t| t.refused > 0));
            // A shared layout or data problem is already in its pages' lists
            // without the page count the build adds to its help.
            let shared = survey.refused().any(|(_, t)| {
                t.problems
                    .iter()
                    .any(|q| q.code == p.code && q.path == p.path && q.message == p.message)
            });
            if !refused_template && !shared {
                for c in causes(p, &js_files) {
                    add(c, p, None);
                }
            }
        } else {
            notes.push(p.clone());
        }
    }
    let mut groups: Vec<Group> = groups
        .into_iter()
        .map(|(cause, b)| {
            let first = b.first.expect("a group has a problem");
            let (title, help) = title_and_help(&cause, &first);
            Group {
                title,
                help,
                pages: b.pages,
                templates: b.templates.into_iter().collect(),
                locations: tidy(b.locations),
                cause,
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        (!a.is_blocker())
            .cmp(&!b.is_blocker())
            .then(b.pages.cmp(&a.pages))
            .then(a.title.cmp(&b.title))
    });
    Diagnosis {
        building: report.written,
        pages: report.written + refused_pages,
        groups,
        notes,
        report,
    }
}

/// Sorted by place, each place once. A place with no line is dropped when
/// the same file has one with a line.
fn tidy(mut locations: Vec<Location>) -> Vec<Location> {
    locations.sort_by(|a, b| {
        (&a.path, a.line, a.column, &a.message).cmp(&(&b.path, b.line, b.column, &b.message))
    });
    locations.dedup_by(|b, a| a.path == b.path && a.line == b.line);
    let with_line: BTreeSet<Option<PathBuf>> = locations
        .iter()
        .filter(|l| l.line.is_some())
        .map(|l| l.path.clone())
        .collect();
    locations.retain(|l| l.line.is_some() || !with_line.contains(&l.path));
    locations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn js() -> BTreeSet<PathBuf> {
        BTreeSet::from([
            PathBuf::from("src/_data/a.js"),
            PathBuf::from("src/_data/b.js"),
        ])
    }

    #[test]
    fn filters_from_either_engine_and_from_includes() {
        let liquid = Problem::error(codes::LIQUID_UNKNOWN_FILTER, "undefined filter: shout");
        assert_eq!(causes(&liquid, &js()), [Cause::Filter("shout".into())]);
        let njk = Problem::error(
            codes::NUNJUCKS_RENDER,
            "Error: filter not found: num while rendering src/index.njk",
        );
        assert_eq!(causes(&njk, &js()), [Cause::Filter("num".into())]);
        let other = Problem::error(codes::NUNJUCKS_RENDER, "Error: something else");
        assert!(matches!(causes(&other, &js())[0], Cause::Other { .. }));
    }

    #[test]
    fn every_js_file_a_problem_names() {
        let p = Problem::error(
            codes::DATA_JS_UNSUPPORTED,
            "this page uses `a`, `b` from src/_data/a.js, src/_data/b.js, JavaScript data files",
        );
        assert_eq!(
            causes(&p, &js()),
            [
                Cause::JsData("src/_data/a.js".into()),
                Cause::JsData("src/_data/b.js".into())
            ]
        );
        let own =
            Problem::error(codes::DATA_JS_UNSUPPORTED, "can't tell").with_path("src/x.11tydata.js");
        assert_eq!(
            causes(&own, &js()),
            [Cause::JsData("src/x.11tydata.js".into())]
        );
    }

    #[test]
    fn javascript_templates_and_followers() {
        let p = Problem::error(codes::TEMPLATE_UNSUPPORTED, "no").with_path("src/a.11ty.js");
        assert_eq!(
            causes(&p, &js()),
            [Cause::TemplateLanguage("11ty.js".into())]
        );
        let f = Problem::error(codes::DATA_UNSUPPORTED, "x").following("fix the above");
        assert_eq!(causes(&f, &js()), [Cause::Follows]);
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(2272), "2,272");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    #[test]
    fn a_place_without_a_line_gives_way_to_one_with() {
        let at = |path: &str, line: Option<usize>| Location {
            path: Some(path.into()),
            line,
            column: None,
            message: "m".into(),
        };
        let out = tidy(vec![
            at("a", None),
            at("a", Some(3)),
            at("a", Some(3)),
            at("b", None),
        ]);
        assert_eq!(out, [at("a", Some(3)), at("b", None)]);
    }
}
