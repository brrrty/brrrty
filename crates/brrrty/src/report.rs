//! Renders build problems through miette.

use std::collections::BTreeSet;
use std::path::Path;

use brrrty_core::{Problem, Severity, codes};
use miette::{
    Diagnostic, GraphicalReportHandler, GraphicalTheme, LabeledSpan, NamedSource, Report,
    SourceCode, SourceSpan,
};

use crate::bug::{environment, issue_url};

#[derive(Debug)]
struct Rendered {
    message: String,
    code: &'static str,
    severity: Severity,
    help: Option<String>,
    source: Option<NamedSource<String>>,
    span: Option<SourceSpan>,
}

impl std::fmt::Display for Rendered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Rendered {}

impl Diagnostic for Rendered {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        Some(Box::new(self.code))
    }
    fn severity(&self) -> Option<miette::Severity> {
        Some(match self.severity {
            Severity::Error => miette::Severity::Error,
            Severity::Warning => miette::Severity::Warning,
        })
    }
    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.help
            .as_ref()
            .map(|h| Box::new(h) as Box<dyn std::fmt::Display>)
    }
    fn source_code(&self) -> Option<&dyn SourceCode> {
        self.source.as_ref().map(|s| s as &dyn SourceCode)
    }
    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        self.span.map(|s| {
            Box::new(std::iter::once(LabeledSpan::new_with_span(
                Some("here".into()),
                s,
            ))) as _
        })
    }
}

/// Byte offset of 1-based `line`/`column` in `text`, plus the length to underline.
///
/// Core counts columns on text with a leading BOM already removed, so the BOM is skipped on
/// line 1 to keep the underline on the right character.
fn locate(text: &str, line: usize, column: usize) -> Option<SourceSpan> {
    let bom = if text.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let mut start = 0;
    for (i, l) in text.split_inclusive('\n').enumerate() {
        if i + 1 == line {
            let skip = if i == 0 { bom } else { 0 };
            let l = &l[skip..];
            start += skip;
            let content = l.trim_end_matches(['\n', '\r']);
            let col_off = content
                .char_indices()
                .nth(column.saturating_sub(1))
                .map_or(content.len(), |(b, _)| b);
            let len = content[col_off..]
                .chars()
                .next()
                .map_or(0, char::len_utf8)
                .max(1)
                .min(content.len() - col_off.min(content.len()));
            return Some((start + col_off, len).into());
        }
        start += l.len();
    }
    None
}

fn to_rendered(p: &Problem, root: &Path) -> Rendered {
    let mut message = p.message.clone();
    let mut source = None;
    let mut span = None;
    if let Some(path) = &p.path {
        if let Some(line) = p.line {
            let full = root.join(path);
            if let Ok(text) = std::fs::read_to_string(&full)
                && let Some(s) = locate(&text, line, p.column.unwrap_or(1))
            {
                source = Some(NamedSource::new(path.display().to_string(), text));
                span = Some(s);
            }
        }
        // miette prints the location with the snippet; otherwise put it in the message.
        if source.is_none() {
            let location = match (p.line, p.column) {
                (Some(l), Some(c)) => format!("{}:{l}:{c}", path.display()),
                (Some(l), None) => format!("{}:{l}", path.display()),
                _ => path.display().to_string(),
            };
            message = format!("{location}: {message}");
        }
    }
    Rendered {
        message,
        code: p.code,
        severity: p.severity,
        help: p.help.clone(),
        source,
        span,
    }
}

/// Extensions made of two parts, which the plain file extension would cut short.
const MULTI_PART_EXTENSIONS: &[&str] = &["11ty.js", "11ty.cjs", "11ty.mjs"];

fn extension_of(p: &Problem) -> Option<String> {
    let path = p.path.as_ref()?;
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if let Some(multi) = MULTI_PART_EXTENSIONS
        .iter()
        .find(|m| name.ends_with(&format!(".{m}")))
    {
        return Some((*multi).to_string());
    }
    path.extension().map(|e| e.to_string_lossy().into_owned())
}

/// Render one problem to a string using a deterministic, colorless handler
/// unless `color` is set.
pub fn render_problem(p: &Problem, root: &Path, color: bool) -> String {
    let theme = if color {
        GraphicalTheme::unicode()
    } else {
        GraphicalTheme::unicode_nocolor()
    };
    let handler = GraphicalReportHandler::new_themed(theme);
    let report = Report::new(to_rendered(p, root));
    let mut out = String::new();
    // Writing to a String cannot fail.
    let _ = handler.render_report(&mut out, report.as_ref());
    out
}

/// Print every problem, issue links for unsupported templates, and the counts line.
pub fn print_problems(problems: &[Problem], root: &Path, color: bool) {
    let mut extensions = BTreeSet::new();
    for p in problems {
        eprint!("{}", render_problem(p, root, color));
        eprintln!();
        if p.code == codes::TEMPLATE_UNSUPPORTED
            && let Some(ext) = extension_of(p)
        {
            extensions.insert(ext);
        }
    }
    for ext in extensions {
        let title = format!("Unsupported: .{ext} templates");
        let body = format!("{}\nExtension: .{ext}", environment());
        eprintln!("Report it or help build it: {}", issue_url(&title, &body));
    }
    let errors = problems
        .iter()
        .filter(|p| matches!(p.severity, Severity::Error))
        .count();
    let warnings = problems.len() - errors;
    eprintln!(
        "{errors} {}, {warnings} {}",
        if errors == 1 { "error" } else { "errors" },
        if warnings == 1 { "warning" } else { "warnings" }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_skips_a_leading_bom_on_line_one() {
        let span = locate("\u{feff}{{ x }}\n", 1, 1).unwrap();
        assert_eq!(span.offset(), 3);
    }
}
