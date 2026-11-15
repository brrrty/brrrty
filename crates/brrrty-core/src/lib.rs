//! Core build pipeline for Brrrty.

// `Problem` is the crate-wide diagnostic type; boxing it would burden every caller.
#![allow(clippy::result_large_err)]

pub mod discover;
pub mod options;
pub mod problem;

pub use discover::{Discovery, discover};
pub use options::BuildOptions;
pub use problem::{Problem, Severity};
