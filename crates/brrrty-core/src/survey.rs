//! What a survey build records for `brrrty doctor`: every problem that
//! refused each template, including the ones an earlier refusal hid, and
//! what the static scan needs to look further.
//!
//! A plain build stops a page at its first refusal and reports each
//! problem once, by path. A survey build also keeps, per template, every
//! refusal it met, its layouts and how many of its pages were written or
//! refused, so doctor can list everything that stands in a page's way.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::problem::Problem;
use crate::templates::Lang;

/// What a survey build found, by template (relative to the project root).
#[derive(Debug, Clone, Default)]
pub struct Survey {
    pub templates: BTreeMap<PathBuf, TemplateSurvey>,
    /// Global data keys a JavaScript data file sets, which Brrrty can't
    /// supply, with the file (relative to the project root).
    pub js_keys: BTreeMap<String, PathBuf>,
}

/// One template's record.
#[derive(Debug, Clone, Default)]
pub struct TemplateSurvey {
    /// Every problem that refused one of its pages, in the order met, without
    /// duplicates. Includes problems that only follow from others.
    pub problems: Vec<Problem>,
    /// Its layouts, nearest first, relative to the project root.
    pub layouts: Vec<PathBuf>,
    /// Pages written (or that would be, in a dry run).
    pub written: usize,
    /// Pages refused. A template refused before it was expanded into pages
    /// counts as one.
    pub refused: usize,
    /// It has `pagination`.
    pub paginates: bool,
    /// Pagination made its pages: [`TemplateSurvey::refused`] counts them.
    pub expanded: bool,
    /// The problems that stopped it before its pagination made any page:
    /// fixing them reveals pages no one can count yet.
    pub stopped: Vec<Problem>,
    /// Problems that only say a page needs the content of a page in another
    /// template (the second item), which was refused for its own reasons.
    pub needs: Vec<(Problem, PathBuf)>,
    /// The values 11ty renders as templates from its data: `permalink` and
    /// every string in `eleventyComputed`, as `(key path, text, line)`, the
    /// line being that of the top-level key in its front matter when it is
    /// there.
    pub(crate) data_strings: Vec<(String, String, Option<usize>)>,
    /// The language those values render in.
    pub(crate) data_lang: Option<Lang>,
    /// The template language that reads its body, when `lang_known`.
    pub(crate) lang: Option<Lang>,
    pub(crate) lang_known: bool,
}

impl TemplateSurvey {
    /// Whether [`TemplateSurvey::refused`] counts its pages, not just the
    /// template: false when pagination never made them.
    pub fn pages_known(&self) -> bool {
        !self.paginates || self.expanded
    }
}

impl Survey {
    /// The templates that had a page refused.
    pub fn refused(&self) -> impl Iterator<Item = (&PathBuf, &TemplateSurvey)> {
        self.templates.iter().filter(|(_, t)| t.refused > 0)
    }
}

/// How a build runs: plainly, or for doctor.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Mode {
    /// Record a [`Survey`].
    pub survey: bool,
    /// Write nothing: no pages, no passthrough copies, no cleanup.
    pub dry_run: bool,
}

/// The survey being recorded, shared by the render threads.
#[derive(Debug, Default)]
pub(crate) struct Log(Mutex<BTreeMap<PathBuf, TemplateSurvey>>);

impl Log {
    fn with(&self, template: &Path, f: impl FnOnce(&mut TemplateSurvey)) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(map.entry(template.to_path_buf()).or_default());
    }

    /// Record that `template` was refused because it needs the content of a page of
    /// `needed`, with the problem that says so.
    pub fn needs(&self, template: &Path, p: &Problem, needed: &Path) {
        self.with(template, |t| {
            if !t.needs.iter().any(|(q, _)| q == p) {
                t.needs.push((p.clone(), needed.to_path_buf()));
            }
        });
    }

    /// Record the values 11ty renders from `template`'s data, in `lang`.
    pub fn data_strings(
        &self,
        template: &Path,
        lang: Option<Lang>,
        strings: Vec<(String, String, Option<usize>)>,
    ) {
        self.with(template, |t| {
            t.data_lang = lang;
            t.data_strings = strings;
        });
    }

    /// Record a problem that refused `template`.
    pub fn problem(&self, template: &Path, p: &Problem) {
        self.with(template, |t| {
            if !t.problems.contains(p) && !t.needs.iter().any(|(q, _)| q == p) {
                t.problems.push(p.clone());
            }
        });
    }

    pub fn layouts(&self, template: &Path, layouts: Vec<PathBuf>) {
        self.with(template, |t| t.layouts = layouts);
    }

    pub fn lang(&self, template: &Path, lang: Option<Lang>) {
        self.with(template, |t| {
            t.lang = lang;
            t.lang_known = true;
        });
    }

    pub fn written(&self, template: &Path) {
        self.with(template, |t| {
            t.written += 1;
            t.expanded = true;
        });
    }

    pub fn refused(&self, template: &Path) {
        self.with(template, |t| {
            t.refused += 1;
            t.expanded = true;
        });
    }

    pub fn paginates(&self, template: &Path) {
        self.with(template, |t| t.paginates = true);
    }

    /// Record the problem that stopped `template` while it was loading.
    pub fn stopped(&self, template: &Path, p: &Problem) {
        self.with(template, |t| {
            if !t.stopped.contains(p) {
                t.stopped.push(p.clone());
            }
        });
    }

    /// Make sure `template` counts as refused, as one page when nothing else
    /// counted it.
    pub fn refused_at_least_once(&self, template: &Path) {
        self.with(template, |t| t.refused = t.refused.max(1));
    }

    pub fn finish(self) -> BTreeMap<PathBuf, TemplateSurvey> {
        self.0.into_inner().unwrap_or_else(|e| e.into_inner())
    }
}
