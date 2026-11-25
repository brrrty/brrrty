//! What `brrrty doctor` prints: the groups of blockers as text, or as JSON.

use std::fmt::Write as _;
use std::path::Path;

use brrrty_core::doctor::{Cause, Diagnosis, Group, Location};
use brrrty_core::{Problem, Severity};
use serde_json::{Value, json};

pub mod verify;

/// Places listed per group in the text output; `--json` lists them all.
const SHOWN_PLACES: usize = 5;

fn place(path: Option<&Path>, line: Option<usize>, column: Option<usize>) -> String {
    let Some(path) = path else {
        return String::new();
    };
    match (line, column) {
        (Some(l), Some(c)) => format!("{}:{l}:{c}", path.display()),
        (Some(l), None) => format!("{}:{l}", path.display()),
        _ => path.display().to_string(),
    }
}

fn location_line(l: &Location) -> String {
    let at = place(l.path.as_deref(), l.line, l.column);
    if at.is_empty() {
        l.message.clone()
    } else {
        format!("{at}: {}", l.message)
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        brrrty_core::doctor::thousands(n),
        if n == 1 { one } else { many }
    )
}

fn group_text(out: &mut String, n: usize, g: &Group) {
    let mut parts = Vec::new();
    if !g.templates.is_empty() {
        parts.push(format!(
            "{}{}, {}",
            if g.pages_exact { "" } else { "at least " },
            count(g.pages, "page", "pages"),
            count(g.templates.len(), "template", "templates")
        ));
    }
    if !g.also_templates.is_empty() {
        parts.push(format!(
            "{}blocks {}{} that {} its content",
            if g.templates.is_empty() { "" } else { "also " },
            if g.also_pages_exact { "" } else { "at least " },
            count(g.also_pages, "page", "pages"),
            if g.also_pages == 1 { "uses" } else { "use" },
        ));
    }
    let mut head = format!("{n}. {}", g.title);
    if !parts.is_empty() {
        let _ = write!(head, " ({})", parts.join("; "));
    }
    let _ = writeln!(out, "{head}");
    for l in g.locations.iter().take(SHOWN_PLACES) {
        let _ = writeln!(out, "   {}", location_line(l));
    }
    if g.locations.len() > SHOWN_PLACES {
        let more = g.locations.len() - SHOWN_PLACES;
        let _ = writeln!(
            out,
            "   ... and {} (--json lists every one)",
            count(more, "more place", "more places")
        );
    }
    if let Some(help) = &g.help {
        let _ = writeln!(out, "   Fix: {help}");
    }
    out.push('\n');
}

/// The text report: each group, then warnings and notes, then the verdict.
pub fn text(d: &Diagnosis) -> String {
    let mut out = String::new();
    let mut n = 0;
    for g in &d.groups {
        n += 1;
        group_text(&mut out, n, g);
    }
    if !d.notes.is_empty() {
        out.push_str("Warnings and notes (these stop nothing):\n");
        for p in &d.notes {
            let at = place(p.path.as_deref(), p.line, p.column);
            let kind = match p.severity {
                Severity::Warning => "warning",
                _ => "note",
            };
            if at.is_empty() {
                let _ = writeln!(out, "   {kind}: {}", p.message);
            } else {
                let _ = writeln!(out, "   {kind}: {at}: {}", p.message);
            }
            if let Some(help) = &p.help {
                let _ = writeln!(out, "     help: {help}");
            }
        }
        out.push('\n');
    }
    let _ = writeln!(out, "{}", d.verdict());
    out
}

fn cause_json(c: &Cause) -> Value {
    match c {
        Cause::Filter(name) => json!({ "kind": "filter", "name": name }),
        Cause::JsData(file) => json!({ "kind": "js-data", "file": file }),
        Cause::TemplateLanguage(ext) => json!({ "kind": "template-language", "extension": ext }),
        Cause::Other { code, message } => {
            json!({ "kind": "problem", "code": code, "message": message })
        }
        Cause::Follows => json!({ "kind": "follows" }),
    }
}

fn problem_json(p: &Problem) -> Value {
    json!({
        "severity": match p.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "note",
        },
        "code": p.code,
        "path": p.path,
        "line": p.line,
        "column": p.column,
        "message": p.message,
        "help": p.help,
    })
}

/// The JSON report. `verify` is the `--verify` part, when it ran.
pub fn json(d: &Diagnosis, verify: Option<Value>, js: Value) -> Value {
    let groups: Vec<Value> = d
        .groups
        .iter()
        .map(|g| {
            json!({
                "cause": cause_json(&g.cause),
                "blocker": g.is_blocker(),
                "title": g.title,
                "help": g.help,
                "pages": g.pages,
                "pages_exact": g.pages_exact,
                "hides_pages": g.hides_pages,
                "also_blocks": {
                    "pages": g.also_pages,
                    "pages_exact": g.also_pages_exact,
                    "templates": g.also_templates,
                },
                "templates": g.templates,
                "locations": g.locations.iter().map(|l| json!({
                    "path": l.path,
                    "line": l.line,
                    "column": l.column,
                    "message": l.message,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "schema": 1,
        "brrrty": env!("CARGO_PKG_VERSION"),
        "verdict": d.verdict(),
        "builds": d.builds(),
        "pages": { "total": d.pages, "building": d.building, "exact": d.pages_exact },
        "blockers": d.blockers(),
        "groups": groups,
        "notes": d.notes.iter().map(problem_json).collect::<Vec<_>>(),
        "jsData": js,
        "verify": verify,
    })
}

/// The "JavaScript data" part of the report: the mode and where it came from, the Node,
/// and each file's state.
pub struct JsReport {
    pub mode: brrrty_core::JsData,
    pub source: brrrty_core::JsDataSource,
    /// The Node found: the one that runs the data when on; when off, the one that would.
    pub node: Option<Result<brrrty_core::JsNode, String>>,
    pub files: Vec<JsFileLine>,
    /// Templates refused because a JavaScript data file didn't run.
    pub waiting_pages: usize,
}

/// One JavaScript data file and what happened to it.
pub struct JsFileLine {
    pub path: std::path::PathBuf,
    pub state: brrrty_core::JsFileState,
    /// For a file that didn't run: `init` can translate it.
    pub translatable: bool,
}

impl JsReport {
    /// Gather it from the build's options and report. `root` is where the files are.
    pub fn new(
        js: &brrrty_core::JsOptions,
        d: &Diagnosis,
        root: &Path,
        node_when_off: Option<Result<brrrty_core::JsNode, String>>,
    ) -> JsReport {
        let files = d
            .report
            .js_files
            .iter()
            .map(|(path, state)| {
                let translatable = *state == brrrty_core::JsFileState::NotRun
                    && std::fs::read_to_string(root.join(path)).is_ok_and(|src| {
                        matches!(
                            crate::init::data::classify(&src),
                            crate::init::data::DataClass::Translatable(_)
                        )
                    });
                JsFileLine {
                    path: path.clone(),
                    state: state.clone(),
                    translatable,
                }
            })
            .collect();
        // A template can wait on several files: count it once.
        let mut waiting: Vec<&std::path::PathBuf> = d
            .groups
            .iter()
            .filter(|g| matches!(g.cause, Cause::JsData(_)))
            .flat_map(|g| &g.templates)
            .collect();
        waiting.sort();
        waiting.dedup();
        let waiting_pages = waiting.len();
        let node = match &js.node {
            Some(Ok(n)) => Some(Ok(n.clone())),
            Some(Err(m)) => Some(Err(m.message.clone())),
            None => node_when_off,
        };
        JsReport {
            mode: js.mode,
            source: js.source.clone(),
            node,
            files,
            waiting_pages,
        }
    }

    fn source_text(&self) -> String {
        match &self.source {
            brrrty_core::JsDataSource::Default => String::new(),
            brrrty_core::JsDataSource::Flag => " (--js-data)".to_string(),
            brrrty_core::JsDataSource::JsConfig => " (on because set_js_config is)".to_string(),
            brrrty_core::JsDataSource::Config { file, line } => match line {
                Some(l) => format!(" ({} line {l})", file.display()),
                None => format!(" ({})", file.display()),
            },
        }
    }

    fn state_text(f: &JsFileLine) -> String {
        use brrrty_core::JsFileState as S;
        let name = |p: &Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let port = name(&f.path.with_extension("rhai"));
        let js = name(&f.path);
        match &f.state {
            S::Ran => "runs in Node".to_string(),
            S::Threw => "throws (see js-data-error below)".to_string(),
            S::NotRun if f.translatable => "init can translate it (literal data)".to_string(),
            S::NotRun => "waits on JavaScript data".to_string(),
            S::Ported => format!("ported: {port} stands in for it"),
            S::PortedIdentical => format!("ported: {port} is identical, delete {js}"),
            S::PortedDiffers(at) => format!("ported: {port} differs at {at}"),
        }
    }

    fn state_key(f: &JsFileLine) -> &'static str {
        use brrrty_core::JsFileState as S;
        match &f.state {
            S::Ran => "node",
            S::Threw => "error",
            S::NotRun if f.translatable => "translatable",
            S::NotRun => "blocked",
            S::Ported => "ported",
            S::PortedIdentical => "ported-identical",
            S::PortedDiffers(_) => "ported-differs",
        }
    }

    /// The text section, or nothing when the site has no JavaScript data files.
    pub fn text(&self) -> String {
        if self.files.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        let node = match &self.node {
            Some(Ok(n)) => format!("Node {} at {}, tz {}", n.version, n.path.display(), n.tz),
            Some(Err(why)) => why.clone(),
            None => "Node not looked for".to_string(),
        };
        match self.mode {
            brrrty_core::JsData::Node => {
                let _ = writeln!(out, "JavaScript data: on{}, {node}", self.source_text());
                if let Some(Ok(n)) = &self.node
                    && n.tz != brrrty_js::tz::TZDATA_VERSION
                {
                    let zone = std::env::var("TZ").unwrap_or_else(|_| "the system zone".into());
                    let _ = writeln!(
                        out,
                        "  note: Node's time zone data is {}, Brrrty's is {}: dates the data computes in {zone} differ only if its rules changed between them",
                        n.tz,
                        brrrty_js::tz::TZDATA_VERSION
                    );
                }
            }
            brrrty_core::JsData::Off => {
                let found = match &self.node {
                    Some(Ok(n)) => format!(" (Node {} found)", n.version),
                    Some(Err(why)) => format!(" ({why})"),
                    None => String::new(),
                };
                let _ = writeln!(
                    out,
                    "JavaScript data: off{}. {}, {} wait on them. Turn it on with config.set_js_data(\"node\"){found}, or port them: brrrty init lists which it can translate.",
                    self.source_text(),
                    count(self.files.len(), "file", "files"),
                    count(self.waiting_pages, "template", "templates"),
                );
            }
        }
        let width = self
            .files
            .iter()
            .map(|f| f.path.display().to_string().len())
            .max()
            .unwrap_or(0);
        for f in &self.files {
            let _ = writeln!(
                out,
                "  {:width$}  {}",
                f.path.display().to_string(),
                Self::state_text(f)
            );
        }
        out.push('\n');
        out
    }

    /// The `jsData` object of `--json`.
    pub fn json(&self) -> Value {
        json!({
            "mode": self.mode.as_str(),
            "source": match self.source {
                brrrty_core::JsDataSource::Default => "default",
                brrrty_core::JsDataSource::Flag => "flag",
                brrrty_core::JsDataSource::JsConfig => "js-config",
                brrrty_core::JsDataSource::Config { .. } => "config",
            },
            "node": match &self.node {
                Some(Ok(n)) => json!({ "path": n.path, "version": n.version, "tz": n.tz }),
                _ => Value::Null,
            },
            "files": self.files.iter().map(|f| {
                let mut o = json!({ "path": f.path, "state": Self::state_key(f) });
                if let brrrty_core::JsFileState::PortedDiffers(at) = &f.state {
                    o["differsAt"] = json!(at);
                }
                o
            }).collect::<Vec<_>>(),
        })
    }
}
