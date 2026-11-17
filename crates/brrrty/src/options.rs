//! Turns the command line flags and the config file into build options.

use std::path::{Path, PathBuf};

use brrrty_core::{BuildOptions, Problem};

/// The build flags the command line takes.
#[derive(Debug, Clone, Default)]
pub struct Flags {
    /// `--input`; the config's `dir.input`, or `.`, when not given.
    pub input: Option<PathBuf>,
    /// `--output`; the config's `dir.output`, or `_site`, when not given.
    pub output: Option<PathBuf>,
    /// `--passthrough`, relative to the root.
    pub passthrough: Vec<PathBuf>,
    /// `--strict`.
    pub strict: bool,
}

/// Read the config in `root` and combine it with `flags`. Flags on the command line win, as
/// 11ty's do. The config's problems come back as the error.
pub fn build_options(root: &Path, flags: &Flags) -> Result<BuildOptions, Vec<Problem>> {
    let site = brrrty_config::load(root)?;
    let dir = site.as_ref().map(|s| s.dir.clone()).unwrap_or_default();
    let mut config = site.map(|s| s.build_options()).unwrap_or_default();
    // `.rhai` data files work with or without a config file.
    config.data = Some(brrrty_config::data_runner(
        brrrty_config::Clock::from_env().unwrap_or(brrrty_config::Clock::System),
        root.to_path_buf(),
        config.reads.clone(),
    ));
    Ok(BuildOptions {
        input: flags
            .input
            .clone()
            .or_else(|| dir.input.map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(".")),
        output: flags
            .output
            .clone()
            .or_else(|| dir.output.map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("_site")),
        root: root.to_path_buf(),
        passthrough: flags.passthrough.clone(),
        strict: flags.strict,
        config,
        ..BuildOptions::default()
    })
}
