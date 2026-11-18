//! Turns the command line flags and the config file into build options.

use std::path::{Path, PathBuf};

use brrrty_core::{
    BuildOptions, ConfigOptions, JsData, JsDataSource, JsNode, JsNodeMissing, JsOptions, Problem,
    RunMode,
};

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
    /// `--js-data`; the config's `set_js_data`, or off, when not given.
    pub js_data: Option<JsData>,
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
        js: js_options(root, &config, flags.js_data),
        config,
        ..BuildOptions::default()
    })
}

/// Whether JavaScript data runs: `--js-data` over `config.set_js_data`, else off. When on,
/// the Node to run it in: `BRRRTY_NODE`, else `node` on `PATH`.
fn js_options(root: &Path, config: &ConfigOptions, flag: Option<JsData>) -> JsOptions {
    let (mode, source) = match (flag, config.js_data) {
        (Some(m), _) => (m, JsDataSource::Flag),
        (None, Some((m, line))) => (
            m,
            JsDataSource::Config {
                file: config.file.clone().unwrap_or_default(),
                line,
            },
        ),
        (None, None) => (JsData::Off, JsDataSource::Default),
    };
    let mut js = JsOptions {
        mode,
        source,
        node: None,
        start: None,
    };
    if mode == JsData::Off {
        return js;
    }
    match brrrty_node::find_node(std::env::var_os("BRRRTY_NODE").as_deref()) {
        Ok(info) => {
            js.node = Some(Ok(JsNode {
                path: info.path.clone(),
                version: info.version_text(),
                tz: info.tz.clone(),
                load_hooks: info.load_hooks,
            }));
            let root = root.to_path_buf();
            js.start = Some(std::sync::Arc::new(move |run_mode| {
                let mode = match run_mode {
                    RunMode::Build => brrrty_node::RunMode::Build,
                    RunMode::Serve => brrrty_node::RunMode::Serve,
                };
                brrrty_node::Worker::start(&info, &root, mode)
                    .map(|w| Box::new(w) as Box<dyn brrrty_js::runtime::JsRuntime>)
                    .map_err(|e| e.to_string())
            }));
        }
        Err(missing) => {
            js.node = Some(Err(JsNodeMissing {
                too_old: matches!(missing, brrrty_node::NodeMissing::TooOld { .. }),
                message: missing.to_string(),
            }));
        }
    }
    js
}
