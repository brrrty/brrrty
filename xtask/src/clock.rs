//! A frozen clock for both builds, through `SOURCE_DATE_EPOCH`.
//!
//! Brrrty reads the variable itself. 11ty runs under `node --require` with
//! `freeze-time.cjs`, which makes `Date.now()` and `new Date()` return it, so
//! a site that prints the build time gives the same bytes in both.

use std::path::{Path, PathBuf};

/// The preload script, also hashed into the reference stamp.
pub const SHIM: &str = include_str!("freeze-time.cjs");

/// The instant both compat builds run at: 2026-01-01T12:00:00Z. Noon, so the
/// date is the same in every time zone.
pub const COMPAT_EPOCH: u64 = 1_767_268_800;

/// The local zone both compat builds run in. A zone with daylight saving
/// time exercises the local-date code, and it is the zone the first
/// references were made in.
pub const COMPAT_TZ: &str = "America/Chicago";

/// The locale both compat builds run in. Node's no-argument
/// `localeCompare` and `toLocaleString` follow it; Brrrty's ports are
/// en-US's.
pub const COMPAT_LOCALE: &str = "en_US.UTF-8";

/// Pins the zone and locale on a compat build's command.
pub fn pin_env(cmd: &mut std::process::Command) -> &mut std::process::Command {
    cmd.env("TZ", COMPAT_TZ).env("LC_ALL", COMPAT_LOCALE)
}

/// The preload script's path in the repo.
pub fn shim_path(root: &Path) -> PathBuf {
    root.join("xtask").join("src").join("freeze-time.cjs")
}

/// `NODE_OPTIONS` that preload the shim into every Node process.
pub fn node_options(root: &Path) -> String {
    format!("--require \"{}\"", shim_path(root).display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compat_builds_pin_the_zone_and_locale() {
        let mut cmd = std::process::Command::new("true");
        pin_env(&mut cmd);
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.contains(&("TZ".as_ref(), Some(COMPAT_TZ.as_ref()))));
        assert!(envs.contains(&("LC_ALL".as_ref(), Some(COMPAT_LOCALE.as_ref()))));
    }

    #[test]
    fn the_shim_is_in_the_repo_and_generic() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert_eq!(std::fs::read_to_string(shim_path(root)).unwrap(), SHIM);
        assert!(SHIM.contains("SOURCE_DATE_EPOCH"));
    }

    #[test]
    fn brrrty_doctor_ships_the_same_shim() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let shipped = root.join("crates/brrrty/src/doctor/freeze-time.cjs");
        assert_eq!(std::fs::read_to_string(shipped).unwrap(), SHIM);
    }
}
