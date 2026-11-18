//! Runs an Eleventy site's JavaScript data files in the site's own Node.
//!
//! One Node process per build runs a worker module embedded in this crate.
//! Brrrty sends it JSON requests on stdin; it answers on stdout with framed
//! replies, the values encoded exactly (see `worker/encode.mjs`) or
//! refused.

mod client;
pub mod decode;
mod find;
pub mod stack;

pub use client::{RunMode, Worker, WorkerError, encode};
pub use decode::decode;
pub use find::{MIN_NODE, NodeInfo, NodeMissing, find_node, parse_version};

/// The worker's value encoding, as JavaScript source.
pub const ENCODE_SOURCE: &str = include_str!("worker/encode.mjs");
