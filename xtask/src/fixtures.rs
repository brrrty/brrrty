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

/// `fixtures-refresh --docker`: build one fixture with 11ty 3.1.8 in each
/// image (`node:22`, `node:24` by default), with no network and the
/// fixture's zone and locale, and keep the output only when every image
/// gives the same bytes. Packages are installed with `--ignore-scripts` in
/// the image, with the network, before the build; nothing runs on the host.
pub fn refresh_docker(root: &Path, name: &str, images: &[String]) -> Result<bool, String> {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(format!("invalid fixture name: {name:?}"));
    }
    let fixture = root.join("tests").join("fixtures").join(name);
    let toml_file = ["fixture.toml", "pending.toml"]
        .iter()
        .map(|f| fixture.join(f))
        .find(|f| f.is_file())
        .ok_or_else(|| format!("{name} has no fixture.toml or pending.toml"))?;
    let table: toml::Table = fs::read_to_string(&toml_file)
        .map_err(|e| format!("cannot read {}: {e}", toml_file.display()))?
        .parse()
        .map_err(|e| format!("bad {}: {e}", toml_file.display()))?;
    let get = |k: &str| table.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let tz = get("tz").unwrap_or_else(|| crate::clock::COMPAT_TZ.to_string());
    let lang = get("lang").unwrap_or_else(|| crate::clock::COMPAT_LOCALE.to_string());
    let input = get("input");

    let scratch = tempfile::tempdir().map_err(|e| format!("cannot make a temp folder: {e}"))?;
    let mut outputs = Vec::new();
    for (i, image) in images.iter().enumerate() {
        let work = scratch.path().join(format!("site{i}"));
        copy_dir(&fixture.join("site"), &work)?;
        // Brrrty's own files mean nothing to 11ty.
        let _ = fs::remove_dir_all(work.join("node_modules"));
        remove_rhai(&work)?;
        let vol = format!("{}:/w", work.display());
        if work.join("package-lock.json").is_file() {
            docker(&[
                "run",
                "--rm",
                "-v",
                &vol,
                "-w",
                "/w",
                image,
                "npm",
                "ci",
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
                "--silent",
            ])?;
        }
        let out = scratch.path().join(format!("out{i}"));
        fs::create_dir_all(&out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
        let input_flag = input
            .as_ref()
            .map(|i| format!("--input={i}"))
            .unwrap_or_default();
        let script = format!(
            "mkdir -p /e && cd /e && npm init -y >/dev/null && npm install --no-audit --no-fund --silent {ELEVENTY} >/dev/null 2>&1 || exit 9"
        );
        // 11ty is installed with the network, then the build runs without it.
        let eleventy = scratch.path().join(format!("eleventy{i}"));
        fs::create_dir_all(&eleventy).map_err(|e| format!("cannot create folder: {e}"))?;
        let evol = format!("{}:/e", eleventy.display());
        docker(&["run", "--rm", "-v", &evol, image, "sh", "-c", &script])?;
        let evol_ro = format!("{}:/e:ro", eleventy.display());
        let ovol = format!("{}:/out", out.display());
        let build = format!(
            "node /e/node_modules/@11ty/eleventy/cmd.cjs --quiet {input_flag} --output=/tmp/o && cp -R /tmp/o/. /out/"
        );
        docker(&[
            "run",
            "--rm",
            "--network",
            "none",
            "-e",
            &format!("TZ={tz}"),
            "-e",
            &format!("LC_ALL={lang}"),
            "-v",
            &evol_ro,
            "-v",
            &vol,
            "-v",
            &ovol,
            "-w",
            "/w",
            image,
            "sh",
            "-c",
            &build,
        ])?;
        outputs.push(out);
    }
    for (i, out) in outputs.iter().enumerate().skip(1) {
        let same = Command::new("diff")
            .arg("-r")
            .arg(&outputs[0])
            .arg(out)
            .status()
            .map_err(|e| format!("cannot run diff: {e}"))?
            .success();
        if !same {
            eprintln!(
                "{name}: {} and {} differ; keeping old expected/",
                images[0], images[i]
            );
            return Ok(false);
        }
    }
    let expected = fixture.join("expected");
    if expected.exists() {
        fs::remove_dir_all(&expected)
            .map_err(|e| format!("cannot delete {}: {e}", expected.display()))?;
    }
    copy_dir(&outputs[0], &expected)?;
    println!("{name}: {} agree", images.join(" and "));
    Ok(true)
}

fn docker(args: &[&str]) -> Result<(), String> {
    let out = Command::new("docker")
        .args(args)
        .output()
        .map_err(|e| format!("cannot run docker: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "docker {} failed ({}):\n{}{}",
            args.first().unwrap_or(&""),
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
    for entry in fs::read_dir(from).map_err(|e| format!("cannot read {}: {e}", from.display()))? {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", from.display()))?;
        let target = to.join(entry.file_name());
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .map_err(|e| format!("cannot copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn remove_rhai(dir: &Path) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            remove_rhai(&path)?;
        } else if path.extension().is_some_and(|e| e == "rhai") {
            fs::remove_file(&path).map_err(|e| format!("cannot delete {}: {e}", path.display()))?;
        }
    }
    Ok(())
}
