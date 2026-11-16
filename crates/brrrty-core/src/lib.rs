//! Core build pipeline for Brrrty.

// `Problem` is the crate-wide diagnostic type; boxing it would burden every caller.
#![allow(clippy::result_large_err)]

pub mod build;
mod data;
pub mod discover;
pub mod frontmatter;
pub mod ignore_files;
pub mod js;
mod layout;
mod loader;
pub mod options;
pub mod page;
pub mod passthrough;
pub mod permalink;
pub mod problem;
mod render;

pub use build::{BuildReport, build};
pub use discover::{Discovery, discover};
pub use frontmatter::split_front_matter;
pub use js::js_truthy;
pub use options::BuildOptions;
pub use page::{Page, file_path_stem, file_slug, resolve_date};
pub use passthrough::copy_passthrough;
pub use permalink::{Permalink, resolve_permalink};
pub use problem::{Problem, Severity, codes};
