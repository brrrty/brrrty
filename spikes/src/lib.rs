//! Measurement harness: scores candidate libraries against Eleventy output.

pub mod case;
pub mod corpus;
pub mod engines;
pub mod markdown_corpus;
pub mod reference;
pub mod score;

/// Private sites live two levels above the spikes crate; `BRRRTY_SITES` overrides.
pub const DEFAULT_SITES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sites");
/// Per-site settings; `BRRRTY_SETTINGS` overrides.
pub const DEFAULT_SETTINGS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../planning/compat");
/// Environment variable naming the Eleventy CLI (`.../@11ty/eleventy/cmd.cjs`).
pub const ELEVENTY_ENV: &str = "BRRRTY_ELEVENTY";

/// Find the Eleventy CLI: the `--eleventy` flag, then `BRRRTY_ELEVENTY`, then the first
/// `--site`'s own install under `BRRRTY_SITES`.
pub fn eleventy_path(
    flag: Option<std::path::PathBuf>,
    first_site: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    pick_eleventy(
        flag,
        std::env::var_os(ELEVENTY_ENV),
        &dir_from_env("BRRRTY_SITES", DEFAULT_SITES),
        first_site,
    )
}

fn pick_eleventy(
    flag: Option<std::path::PathBuf>,
    env: Option<std::ffi::OsString>,
    sites_dir: &std::path::Path,
    first_site: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    if let Some(p) = flag {
        return Ok(p);
    }
    if let Some(p) = env.filter(|v| !v.is_empty()) {
        return Ok(p.into());
    }
    if let Some(site) = first_site {
        return Ok(sites_dir
            .join(site)
            .join("node_modules/@11ty/eleventy/cmd.cjs"));
    }
    Err(format!(
        "no Eleventy CLI given: pass --eleventy <path to @11ty/eleventy/cmd.cjs> or set {ELEVENTY_ENV}"
    ))
}

/// `var` from the environment, else `default`.
pub fn dir_from_env(var: &str, default: &str) -> std::path::PathBuf {
    std::env::var_os(var)
        .map(Into::into)
        .unwrap_or_else(|| default.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn eleventy_flag_then_env_then_first_site() {
        let sites = Path::new("/sites");
        let flag = || Some(PathBuf::from("/flag/cmd.cjs"));
        let env = || Some("/env/cmd.cjs".into());
        assert_eq!(
            pick_eleventy(flag(), env(), sites, Some("a")).unwrap(),
            PathBuf::from("/flag/cmd.cjs")
        );
        assert_eq!(
            pick_eleventy(None, env(), sites, Some("a")).unwrap(),
            PathBuf::from("/env/cmd.cjs")
        );
        assert_eq!(
            pick_eleventy(None, Some("".into()), sites, Some("a")).unwrap(),
            PathBuf::from("/sites/a/node_modules/@11ty/eleventy/cmd.cjs")
        );
        let err = pick_eleventy(None, None, sites, None).unwrap_err();
        assert!(err.contains("--eleventy"), "{err}");
        assert!(err.contains(ELEVENTY_ENV), "{err}");
    }
}
