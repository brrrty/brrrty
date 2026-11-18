//! Runs an Eleventy site's JavaScript data files in the site's own Node.
//!
//! One Node process per build runs a worker module embedded in this crate.
//! Brrrty sends it framed JSON requests on stdin; it answers on stdout with
//! the values encoded exactly (see `worker/encode.mjs`), or refuses them.

pub mod decode;

pub use decode::decode;

/// The worker's value encoding, as JavaScript source.
pub const ENCODE_SOURCE: &str = include_str!("worker/encode.mjs");
