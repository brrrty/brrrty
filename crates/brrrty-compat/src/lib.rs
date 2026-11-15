//! Compare two output folders to prove Brrrty's output matches Eleventy's.

mod allow;
mod compare;
mod normalize;
mod report;

pub use allow::AllowList;
pub use compare::compare_dirs;
pub use normalize::normalize_markup;
pub use report::{CompareReport, Counts, FileResult, Outcome};

use std::path::PathBuf;

/// Errors from comparing output folders or loading an allow-list.
#[derive(Debug, thiserror::Error)]
pub enum CompatError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid allow-list TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid regex in allow rule: {0}")]
    Regex(#[from] regex::Error),
    #[error("invalid glob in allow rule: {0}")]
    Glob(#[from] globset::Error),
    #[error("allow rule `{id}` is missing approved_by or approved_on")]
    Unapproved { id: String },
}
