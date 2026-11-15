//! Core build pipeline for Brrrty.

pub mod discover;
pub mod options;
pub mod problem;

pub use discover::{Discovery, discover};
pub use options::BuildOptions;
pub use problem::{Problem, Severity};
