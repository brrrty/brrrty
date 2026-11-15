//! Measurement harness: scores candidate libraries against Eleventy output.

pub mod case;
pub mod corpus;
pub mod engines;
pub mod reference;
pub mod score;

pub const DEFAULT_ELEVENTY: &str =
    "/Users/josh/Code/brrrty/sites/joshtronic.com/node_modules/@11ty/eleventy/cmd.cjs";
