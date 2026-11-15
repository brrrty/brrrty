//! `cargo xtask fixtures-refresh`: regenerate expected output with real Eleventy.

use std::fs;
use std::path::Path;
use std::process::Command;

const ELEVENTY: &str = "@11ty/eleventy@3.1.8";

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
        if expected.exists() {
            fs::remove_dir_all(&expected)
                .map_err(|e| format!("cannot delete {}: {e}", expected.display()))?;
        }
        println!("refreshing fixture {n}");
        let out = Command::new("npx")
            .args(["--yes", ELEVENTY, "--quiet"])
            .arg(format!("--output={}", expected.display()))
            .current_dir(fixture.join("site"))
            .output()
            .map_err(|e| format!("cannot run npx: {e}"))?;
        if !out.status.success() {
            eprintln!("fixture {n}: eleventy failed ({})", out.status);
            eprintln!("{}", String::from_utf8_lossy(&out.stderr));
            all_ok = false;
        }
    }
    Ok(all_ok)
}
