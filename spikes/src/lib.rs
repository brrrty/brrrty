//! Measurement harness: scores candidate libraries against Eleventy output.

pub mod case;
pub mod corpus;
pub mod engines;
pub mod reference;
pub mod score;

/// Private sites live two levels above the spikes crate; `BRRRTY_SITES` overrides.
pub const DEFAULT_SITES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sites");
/// Per-site settings; `BRRRTY_SETTINGS` overrides.
pub const DEFAULT_SETTINGS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../planning/compat");
/// The Eleventy CLI used for references; the `--eleventy` flag overrides.
pub const DEFAULT_ELEVENTY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../sites/joshtronic.com/node_modules/@11ty/eleventy/cmd.cjs"
);

/// `var` from the environment, else `default`.
pub fn dir_from_env(var: &str, default: &str) -> std::path::PathBuf {
    std::env::var_os(var)
        .map(Into::into)
        .unwrap_or_else(|| default.into())
}
