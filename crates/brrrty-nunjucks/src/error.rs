//! Nunjucks errors.

use std::fmt;

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Nunjucks rejects the template. This includes the places where
    /// Nunjucks's lexer or parser throws a JavaScript error, such as a
    /// `TypeError`, instead of a `Template render error`: the message is
    /// the one Nunjucks reports.
    Syntax,
    /// Tags or expressions nest more than 256 levels deep.
    TooDeep,
    /// A tag Eleventy adds to Nunjucks that Brrrty does not run, such as
    /// `setAsync`.
    Unsupported,
}

/// An error with its position in the template source.
///
/// `message` is the message Nunjucks puts after the position line. An
/// error Nunjucks raises as a JavaScript error has its name in front, as
/// in `TypeError: Cannot read properties of null (reading 'type')`.
/// `line` and `column` are 1-based, and are `None` where Nunjucks gives no
/// position, such as at the end of the file. Columns count UTF-16 code
/// units, as Nunjucks does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NunjucksError {
    pub kind: ErrorKind,
    pub message: String,
    pub template: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub help: Option<String>,
}

impl fmt::Display for NunjucksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.line, self.column) {
            (Some(line), Some(column)) => write!(f, "{}:{line}:{column}: ", self.template)?,
            _ => write!(f, "{}: ", self.template)?,
        }
        f.write_str(&self.message)?;
        if let Some(help) = &self.help {
            write!(f, "\nhelp: {help}")?;
        }
        Ok(())
    }
}

impl std::error::Error for NunjucksError {}

/// An error before it is given its template name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fail {
    pub kind: ErrorKind,
    pub message: String,
    /// 1-based line and UTF-16 column.
    pub at: Option<(usize, usize)>,
    pub help: Option<String>,
}

impl Fail {
    /// Nunjucks's `TemplateError`, from `parser.fail`.
    pub fn template(message: impl Into<String>, at: Option<(usize, usize)>) -> Fail {
        Fail {
            kind: ErrorKind::Syntax,
            message: message.into(),
            at,
            help: None,
        }
    }

    /// A plain JavaScript error, such as `Error` or `TypeError`, which
    /// Nunjucks wraps with its name and without a position.
    pub fn js(name: &str, message: impl AsRef<str>) -> Fail {
        Fail {
            kind: ErrorKind::Syntax,
            message: format!("{name}: {}", message.as_ref()),
            at: None,
            help: None,
        }
    }

    /// `TypeError: Cannot read properties of null (reading 'prop')`.
    pub fn null_read(prop: &str) -> Fail {
        Fail::js(
            "TypeError",
            format!("Cannot read properties of null (reading '{prop}')"),
        )
    }

    /// `TypeError: Cannot read properties of undefined (reading 'prop')`.
    pub fn undefined_read(prop: &str) -> Fail {
        Fail::js(
            "TypeError",
            format!("Cannot read properties of undefined (reading '{prop}')"),
        )
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Fail {
        self.help = Some(help.into());
        self
    }

    pub fn named(self, template: &str) -> NunjucksError {
        NunjucksError {
            kind: self.kind,
            message: self.message,
            template: template.to_string(),
            line: self.at.map(|a| a.0),
            column: self.at.map(|a| a.1),
            help: self.help,
        }
    }
}
