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
    let mut head = format!("{n}. {}", g.title);
    if !g.templates.is_empty() {
        let _ = write!(
            head,
            " ({}{}, {})",
            if g.pages_exact { "" } else { "at least " },
            count(g.pages, "page", "pages"),
            count(g.templates.len(), "template", "templates")
        );
    }
    if !g.also_templates.is_empty() {
        let _ = write!(
            head,
            "; also blocks {}{} that use its content",
            if g.also_pages_exact { "" } else { "at least " },
            count(g.also_pages, "page", "pages"),
        );
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
pub fn json(d: &Diagnosis, verify: Option<Value>) -> Value {
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
        "verify": verify,
    })
}
