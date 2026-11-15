//! Liquid errors.

use std::fmt;

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The template does not follow Liquid's grammar.
    Syntax,
    /// A tag name that no tag uses, including a stray `endif` or `else`.
    UnknownTag,
    /// A tag, output, block or `raw` that is never closed.
    Unclosed,
    /// Tags or expressions nest more than 256 levels deep.
    TooDeep,
}

/// An error with its position in the template source.
///
/// `message` is the LiquidJS message without its position suffix. `line`
/// and `column` are 1-based. Columns count UTF-16 code units, as LiquidJS
/// does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiquidError {
    pub kind: ErrorKind,
    pub message: String,
    pub template: String,
    pub line: usize,
    pub column: usize,
    pub help: Option<String>,
}

impl fmt::Display for LiquidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.template, self.line, self.column, self.message
        )?;
        if let Some(help) = &self.help {
            write!(f, "\nhelp: {help}")?;
        }
        Ok(())
    }
}

impl std::error::Error for LiquidError {}

/// An error at a byte offset in the source being parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Located {
    pub kind: ErrorKind,
    pub message: String,
    pub help: Option<String>,
    pub offset: usize,
}

/// A parse error. `Plain` errors take the position of the innermost token
/// being parsed, as LiquidJS's `ParseError` does. `Located` errors already
/// have one, like LiquidJS's `TokenizationError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParseErr {
    Located(Located),
    Plain {
        kind: ErrorKind,
        message: String,
        help: Option<String>,
    },
}

impl ParseErr {
    pub fn plain(kind: ErrorKind, message: impl Into<String>) -> ParseErr {
        ParseErr::Plain {
            kind,
            message: message.into(),
            help: None,
        }
    }

    pub fn with_help(mut self, text: impl Into<String>) -> ParseErr {
        match &mut self {
            ParseErr::Located(l) => l.help = Some(text.into()),
            ParseErr::Plain { help, .. } => *help = Some(text.into()),
        }
        self
    }

    /// Gives a `Plain` error the position `offset`.
    pub fn locate(self, offset: usize) -> Located {
        match self {
            ParseErr::Located(l) => l,
            ParseErr::Plain {
                kind,
                message,
                help,
            } => Located {
                kind,
                message,
                help,
                offset,
            },
        }
    }
}

impl From<Located> for ParseErr {
    fn from(l: Located) -> ParseErr {
        ParseErr::Located(l)
    }
}
