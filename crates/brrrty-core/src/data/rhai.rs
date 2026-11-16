//! `.rhai` data files, as the build sees them: a runner (from
//! `brrrty-config`) gives each file's value, and the file takes the slot a
//! JavaScript data file of the same name would hold.

use std::path::{Path, PathBuf};

use brrrty_js::Value;

use crate::frontmatter::Data;
use crate::options::{DataRequest, DataRunner};
use crate::problem::{Problem, codes};

/// Run the `.rhai` file at `full`, reported as `shown`. A problem the
/// runner gives (with the script's line and column) gets the file's path.
pub(crate) fn run(
    runner: Option<&DataRunner>,
    full: &Path,
    shown: &Path,
    global: &Data,
) -> Result<Value, Problem> {
    let Some(runner) = runner else {
        return Err(Problem::error(
            codes::DATA_RHAI_ERROR,
            "this build can't run .rhai data files",
        )
        .with_path(shown)
        .with_help("build with the `brrrty` command, which runs them"));
    };
    (runner.0)(&DataRequest {
        file: full,
        shown,
        global,
    })
    .map_err(|p| p.with_path(shown))
}

/// `site.js` is ported by `site.rhai`; `x.11tydata.cjs` by `x.11tydata.rhai`.
pub(crate) fn port_of(js: &Path) -> PathBuf {
    js.with_extension("rhai")
}

/// The note for a JavaScript data file that a `.rhai` port stands in for.
pub(crate) fn shadowed(js: &Path) -> Problem {
    let port = port_of(js);
    let port = port.file_name().unwrap_or_default().to_string_lossy();
    let js_name = js.file_name().unwrap_or_default().to_string_lossy();
    // A template or directory data file sets several keys; a global one, one.
    let message = if js_name.contains(".11tydata.") {
        format!("{port} is beside it and replaces it, so Brrrty ignores {js_name}")
    } else {
        format!("{port} supplies this key, so Brrrty ignores {js_name}")
    };
    Problem::info(codes::DATA_RHAI_SHADOWS_JS, message).with_path(js)
}

pub(crate) fn is_js(name: &str) -> bool {
    [".js", ".cjs", ".mjs"].iter().any(|e| name.ends_with(e))
}
