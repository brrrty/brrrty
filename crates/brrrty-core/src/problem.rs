use std::path::PathBuf;

/// Stable ids for every [`Problem`] code.
pub mod codes {
    pub const BUILD_THREADS_FAILED: &str = "build-threads-failed";
    pub const COMPUTED_CYCLE: &str = "computed-cycle";
    pub const COMPUTED_UNSUPPORTED: &str = "computed-unsupported";
    pub const CONFIG_ERROR: &str = "config-error";
    pub const CONFIG_INVALID: &str = "config-invalid";
    pub const CONFIG_LIMIT: &str = "config-limit";
    pub const CONFIG_SYNTAX: &str = "config-syntax";
    pub const CONFIG_UNREADABLE: &str = "config-unreadable";
    pub const CONFIG_UNSUPPORTED: &str = "config-unsupported";
    pub const CONTENT_CYCLE: &str = "content-cycle";
    pub const CONTENT_UNAVAILABLE: &str = "content-unavailable";
    pub const CWD_UNAVAILABLE: &str = "cwd-unavailable";
    pub const DATA_FILE_INVALID: &str = "data-file-invalid";
    pub const DATA_JS_UNSUPPORTED: &str = "data-js-unsupported";
    pub const DATA_RHAI_ERROR: &str = "data-rhai-error";
    pub const DATA_RHAI_LIMIT: &str = "data-rhai-limit";
    pub const DATA_RHAI_SHADOWS_JS: &str = "data-rhai-shadows-js";
    pub const DATA_RHAI_SYNTAX: &str = "data-rhai-syntax";
    pub const DATA_RESERVED: &str = "data-reserved";
    pub const DATA_UNSUPPORTED: &str = "data-unsupported";
    pub const DATE_INVALID: &str = "date-invalid";
    pub const DISCOVER_FAILED: &str = "discover-failed";
    pub const ELEVENTY_CONFIG_IGNORED: &str = "eleventy-config-ignored";
    pub const FEED_FAILED: &str = "feed-failed";
    pub const FEED_UNSUPPORTED: &str = "feed-unsupported";
    pub const FRONT_MATTER_INVALID: &str = "front-matter-invalid";
    pub const FRONT_MATTER_UNSUPPORTED: &str = "front-matter-unsupported";
    pub const IGNORE_FILE_INVALID: &str = "ignore-file-invalid";
    pub const IGNORE_LINE_ANCHORED: &str = "ignore-line-anchored";
    pub const IGNORE_LINE_INVALID: &str = "ignore-line-invalid";
    pub const INPUT_MISSING: &str = "input-missing";
    pub const INPUT_NOT_A_DIRECTORY: &str = "input-not-a-directory";
    pub const LAYOUT_CYCLE: &str = "layout-cycle";
    pub const LAYOUT_INVALID: &str = "layout-invalid";
    pub const LAYOUT_MISSING: &str = "layout-missing";
    pub const LAYOUT_UNSUPPORTED: &str = "layout-unsupported";
    pub const LIQUID_LIMIT: &str = "liquid-limit";
    pub const LIQUID_RENDER: &str = "liquid-render";
    pub const LIQUID_SYNTAX: &str = "liquid-syntax";
    pub const LIQUID_UNKNOWN_FILTER: &str = "liquid-unknown-filter";
    pub const MARKDOWN_RENDER_FAILED: &str = "markdown-render-failed";
    pub const NUNJUCKS_LIMIT: &str = "nunjucks-limit";
    pub const NUNJUCKS_RENDER: &str = "nunjucks-render";
    pub const NUNJUCKS_SYNTAX: &str = "nunjucks-syntax";
    pub const NUNJUCKS_UNSUPPORTED: &str = "nunjucks-unsupported";
    pub const OUTPUT_COLLISION: &str = "output-collision";
    pub const OUTPUT_ESCAPES: &str = "output-escapes";
    pub const OUTPUT_OVERLAPS_SOURCE: &str = "output-overlaps-source";
    pub const PAGINATION_INVALID: &str = "pagination-invalid";
    pub const PAGINATION_UNSUPPORTED: &str = "pagination-unsupported";
    pub const PASSTHROUGH_COLLISION: &str = "passthrough-collision";
    pub const PASSTHROUGH_COPY_FAILED: &str = "passthrough-copy-failed";
    pub const PASSTHROUGH_ESCAPES: &str = "passthrough-escapes";
    pub const PASSTHROUGH_IS_INPUT: &str = "passthrough-is-input";
    pub const PASSTHROUGH_MISSING: &str = "passthrough-missing";
    pub const PASSTHROUGH_OVERRIDES_PAGE: &str = "passthrough-overrides-page";
    pub const PASSTHROUGH_UNSUPPORTED: &str = "passthrough-unsupported";
    pub const PERMALINK_INVALID: &str = "permalink-invalid";
    pub const PERMALINK_NO_EXTENSION: &str = "permalink-no-extension";
    pub const PERMALINK_UNSUPPORTED: &str = "permalink-unsupported";
    pub const SERVE_STAGING_IN_USE: &str = "serve-staging-in-use";
    pub const SERVE_STAGING_LEFT: &str = "serve-staging-left";
    pub const SOURCE_DATE_EPOCH_INVALID: &str = "source-date-epoch-invalid";
    pub const TAGS_INVALID: &str = "tags-invalid";
    pub const SYMLINK_BROKEN: &str = "symlink-broken";
    pub const SYMLINK_LOOP: &str = "symlink-loop";
    pub const SYNTAX_HIGHLIGHT_FAILED: &str = "syntax-highlight-failed";
    pub const SYNTAX_HIGHLIGHT_UNSUPPORTED: &str = "syntax-highlight-unsupported";
    pub const TEMPLATE_ENGINE_OVERRIDE_INVALID: &str = "template-engine-override-invalid";
    pub const TEMPLATE_ENGINE_UNSUPPORTED: &str = "template-engine-unsupported";
    pub const TEMPLATE_NOT_UTF8: &str = "template-not-utf8";
    pub const TEMPLATE_PLUGIN_ONLY: &str = "template-plugin-only";
    pub const TEMPLATE_UNREADABLE: &str = "template-unreadable";
    pub const TEMPLATE_UNSUPPORTED: &str = "template-unsupported";
    pub const WRITE_FAILED: &str = "write-failed";
}

/// How serious a [`Problem`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    /// Something Brrrty did on purpose that the user may want to know.
    Info,
}

/// A diagnostic produced anywhere in the build. Plain data; the CLI renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub severity: Severity,
    /// Stable kebab-case id, such as `front-matter-invalid`.
    pub code: &'static str,
    pub path: Option<PathBuf>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub message: String,
    pub help: Option<String>,
    /// Caused by another problem the build reports (its help says to fix
    /// the problems "reported above"): it is listed after every problem
    /// that isn't, so that it is.
    pub follows: bool,
}

impl Problem {
    fn new(severity: Severity, code: &'static str, message: impl Into<String>) -> Self {
        Problem {
            severity,
            code,
            path: None,
            line: None,
            column: None,
            message: message.into(),
            help: None,
            follows: false,
        }
    }

    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, message)
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, code, message)
    }

    pub fn info(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Info, code, message)
    }

    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn at(mut self, line: usize, column: usize) -> Self {
        self.line = Some(line);
        self.column = Some(column);
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Marks the problem as caused by another one the build reports, with
    /// `help` saying to fix the problems reported above it.
    pub fn following(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self.follows = true;
        self
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Problem {}
