//! Core build pipeline for Brrrty.

// `Problem` is the crate-wide diagnostic type; boxing it would burden every caller.
#![allow(clippy::result_large_err)]

pub mod discover;
pub mod frontmatter;
pub mod options;
pub mod page;
pub mod permalink;
pub mod problem;

pub use discover::{Discovery, discover};
pub use frontmatter::split_front_matter;
pub use options::BuildOptions;
pub use page::{Page, file_slug, resolve_date};
pub use permalink::{Permalink, resolve_permalink};
pub use problem::{Problem, Severity};
