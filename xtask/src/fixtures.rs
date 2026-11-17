//! `cargo xtask fixtures-refresh`: regenerate expected output with real Eleventy.

use std::fs;
use std::path::Path;
use std::process::Command;

const ELEVENTY: &str = "@11ty/eleventy@3.1.8";

/// The fixture's `source_date_epoch`, if its `fixture.toml` sets one.
fn source_date_epoch(fixture: &Path) -> Result<Option<u64>, String> {
    let file = fixture.join("fixture.toml");
    if !file.is_file() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let table: toml::Table = text
        .parse()
        .map_err(|e| format!("bad {}: {e}", file.display()))?;
    match table.get("source_date_epoch") {
        None => Ok(None),
        Some(toml::Value::Integer(n)) if *n >= 0 => Ok(Some(*n as u64)),
        Some(v) => Err(format!(
            "{}: source_date_epoch must be whole seconds, got {v}",
            file.display()
        )),
    }
}

/// The fixture's `tz`, the local time zone both builds run in, if its
/// `fixture.toml` sets one.
fn time_zone(fixture: &Path) -> Result<Option<String>, String> {
    let file = fixture.join("fixture.toml");
    if !file.is_file() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let table: toml::Table = text
        .parse()
        .map_err(|e| format!("bad {}: {e}", file.display()))?;
    match table.get("tz") {
        None => Ok(None),
        Some(toml::Value::String(s)) => Ok(Some(s.clone())),
        Some(v) => Err(format!(
            "{}: tz must be a zone name, got {v}",
            file.display()
        )),
    }
}

/// A string setting from the fixture's `fixture.toml`, such as `lang`, the
/// locale (`LC_ALL`) both builds run in.
fn string_key(fixture: &Path, key: &str) -> Result<Option<String>, String> {
    let file = fixture.join("fixture.toml");
    if !file.is_file() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let table: toml::Table = text
        .parse()
        .map_err(|e| format!("bad {}: {e}", file.display()))?;
    match table.get(key) {
        None => Ok(None),
        Some(toml::Value::String(s)) => Ok(Some(s.clone())),
        Some(v) => Err(format!(
            "{}: {key} must be a string, got {v}",
            file.display()
        )),
    }
}

/// Install a fixture's npm packages (the 11ty plugins its config uses)
/// from its lock file, when it has dependencies and they aren't installed.
fn install_packages(site: &Path) -> Result<(), String> {
    let pkg = site.join("package.json");
    let Ok(text) = fs::read_to_string(&pkg) else {
        return Ok(());
    };
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("bad {}: {e}", pkg.display()))?;
    let has_deps = json
        .get("dependencies")
        .and_then(|d| d.as_object())
        .is_some_and(|d| !d.is_empty());
    if !has_deps || site.join("node_modules").is_dir() {
        return Ok(());
    }
    println!("installing packages in {}", site.display());
    let out = Command::new("npm")
        .args(["ci", "--no-audit", "--no-fund"])
        .current_dir(site)
        .output()
        .map_err(|e| format!("cannot run npm: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "npm ci failed in {}:\n{}",
            site.display(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Returns Ok(true) when every requested fixture was refreshed.
pub fn refresh(root: &Path, name: Option<&str>) -> Result<bool, String> {
    let dir = root.join("tests").join("fixtures");
    let names: Vec<String> = match name {
        Some(n) => {
            if n.is_empty() || n.contains(['/', '\\']) || n == "." || n == ".." {
                return Err(format!("invalid fixture name: {n:?}"));
            }
            if !dir.join(n).join("site").is_dir() {
                return Err(format!(
                    "no such fixture: {n} (expected a site folder at {})",
                    dir.join(n).join("site").display()
                ));
            }
            vec![n.to_string()]
        }
        None => {
            let entries = fs::read_dir(&dir)
                .map_err(|e| format!("cannot read fixtures folder {}: {e}", dir.display()))?;
            let mut v: Vec<String> = entries
                .filter_map(Result::ok)
                .filter(|e| e.path().join("site").is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            v.sort();
            v
        }
    };
    if names.is_empty() {
        return Err(format!("no fixtures found in {}", dir.display()));
    }

    let mut all_ok = true;
    for n in names {
        let fixture = dir.join(&n);
        let expected = fixture.join("expected");
        let tmp = fixture.join("expected.tmp");
        if tmp.exists() {
            fs::remove_dir_all(&tmp)
                .map_err(|e| format!("cannot delete {}: {e}", tmp.display()))?;
        }
        println!("refreshing fixture {n}");
        let epoch = source_date_epoch(&fixture)?;
        install_packages(&fixture.join("site"))?;
        let mut cmd = Command::new("npx");
        cmd.args(["--yes", ELEVENTY, "--quiet"])
            .arg(format!("--output={}", tmp.display()))
            .current_dir(fixture.join("site"))
            .env("NODE_OPTIONS", crate::clock::node_options(root))
            .env_remove("SOURCE_DATE_EPOCH");
        if let Some(e) = epoch {
            cmd.env("SOURCE_DATE_EPOCH", e.to_string());
        }
        if let Some(tz) = time_zone(&fixture)? {
            cmd.env("TZ", tz);
        }
        if let Some(lang) = string_key(&fixture, "lang")? {
            cmd.env("LC_ALL", lang);
        }
        let out = cmd.output().map_err(|e| format!("cannot run npx: {e}"))?;
        if !out.status.success() {
            eprintln!(
                "fixture {n}: eleventy failed ({}), keeping old expected/",
                out.status
            );
            eprintln!("{}", String::from_utf8_lossy(&out.stderr));
            let _ = fs::remove_dir_all(&tmp);
            all_ok = false;
            continue;
        }
        if expected.exists() {
            fs::remove_dir_all(&expected)
                .map_err(|e| format!("cannot delete {}: {e}", expected.display()))?;
        }
        // 11ty writes nothing for a site with no output; keep expected/ present.
        if !tmp.exists() {
            fs::create_dir_all(&tmp)
                .map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
        }
        fs::rename(&tmp, &expected)
            .map_err(|e| format!("cannot move {} into place: {e}", tmp.display()))?;
    }
    Ok(all_ok)
}
