//! `cargo xtask compat` and `cargo xtask scoreboard`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use brrrty_compat::{AllowList, Counts, compare_dirs};
use sha2::{Digest, Sha256};

use crate::settings::SiteSettings;

struct SiteRun {
    counts: Counts,
    reference_files: usize,
    brrrty_exit: Option<i32>,
    report_path: PathBuf,
    pass: bool,
}

fn check_site_name(site: &str) -> Result<(), String> {
    if site.is_empty() || site.contains(['/', '\\']) || site == "." || site == ".." {
        return Err(format!("invalid site name: {site:?}"));
    }
    Ok(())
}

fn settings_path(settings_dir: &Path, site: &str) -> PathBuf {
    settings_dir.join(format!("{site}.toml"))
}

fn build_release(root: &Path) -> Result<(), String> {
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "brrrty"])
        .current_dir(root)
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("cargo build --release -p brrrty failed".into())
    }
}

fn git_output(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// HEAD plus a hash of the working-tree status of the site, or None outside git.
fn compute_stamp(site_dir: &Path) -> String {
    let head = git_output(site_dir, &["rev-parse", "HEAD"])
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .unwrap_or_else(|| "no-git".into());
    let status = git_output(site_dir, &["status", "--porcelain"]).unwrap_or_default();
    let digest: String = Sha256::digest(&status)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{head}\n{digest}\n")
}

fn first_lines(bytes: &[u8], n: usize) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .take(n)
        .collect::<Vec<_>>()
        .join("\n")
}

fn count_files(dir: &Path) -> usize {
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    rd.filter_map(Result::ok)
        .map(|e| {
            let p = e.path();
            if p.is_dir() { count_files(&p) } else { 1 }
        })
        .sum()
}

fn ensure_reference(site_dir: &Path, work: &Path, refresh: bool) -> Result<PathBuf, String> {
    let reference = work.join("reference");
    let stamp_path = work.join("reference.stamp");
    let stamp = compute_stamp(site_dir);
    if !refresh && reference.is_dir() && fs::read_to_string(&stamp_path).is_ok_and(|s| s == stamp) {
        println!("reference: reusing {}", reference.display());
        return Ok(reference);
    }
    if !site_dir.join("node_modules").is_dir() {
        println!("reference: running npm ci in {}", site_dir.display());
        let out = Command::new("npm")
            .arg("ci")
            .current_dir(site_dir)
            .output()
            .map_err(|e| format!("cannot run npm: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "npm ci failed ({}):\n{}",
                out.status,
                first_lines(&out.stderr, 20)
            ));
        }
    }
    if reference.exists() {
        fs::remove_dir_all(&reference)
            .map_err(|e| format!("cannot delete {}: {e}", reference.display()))?;
    }
    fs::create_dir_all(work).map_err(|e| format!("cannot create {}: {e}", work.display()))?;
    println!("reference: building with Eleventy");
    let out = Command::new("npx")
        .args(["@11ty/eleventy", "--quiet"])
        .arg(format!("--output={}", reference.display()))
        .current_dir(site_dir)
        .output()
        .map_err(|e| format!("cannot run npx: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "reference build (Eleventy) failed ({}), first lines of stderr:\n{}",
            out.status,
            first_lines(&out.stderr, 20)
        ));
    }
    fs::write(&stamp_path, stamp)
        .map_err(|e| format!("cannot write {}: {e}", stamp_path.display()))?;
    Ok(reference)
}

fn run_site(
    root: &Path,
    site: &str,
    sites_dir: &Path,
    settings_dir: &Path,
    refresh: bool,
) -> Result<SiteRun, String> {
    check_site_name(site)?;
    let settings_file = settings_path(settings_dir, site);
    let src = fs::read_to_string(&settings_file).map_err(|e| {
        format!(
            "missing settings file {} ({e}); expected <settings-dir>/{site}.toml",
            settings_file.display()
        )
    })?;
    let settings = SiteSettings::from_toml(&src)
        .map_err(|e| format!("bad settings in {}: {e}", settings_file.display()))?;
    let allow = AllowList::from_toml(&src)
        .map_err(|e| format!("bad allow-list in {}: {e}", settings_file.display()))?;

    let site_dir = sites_dir.join(site);
    if !site_dir.is_dir() {
        return Err(format!("no such site folder: {}", site_dir.display()));
    }
    let site_dir = site_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", site_dir.display()))?;

    let work = root.join("target").join("compat").join(site);
    let reference = ensure_reference(&site_dir, &work, refresh)?;

    let actual = work.join("brrrty");
    if actual.exists() {
        fs::remove_dir_all(&actual)
            .map_err(|e| format!("cannot delete {}: {e}", actual.display()))?;
    }
    fs::create_dir_all(&work).map_err(|e| format!("cannot create {}: {e}", work.display()))?;
    let mut cmd = Command::new(root.join("target").join("release").join("brrrty"));
    cmd.args(["build", "--input", &settings.input, "--output"])
        .arg(&actual);
    for p in &settings.passthrough {
        cmd.args(["--passthrough", p]);
    }
    let status = cmd
        .current_dir(&site_dir)
        .status()
        .map_err(|e| format!("cannot run brrrty: {e}"))?;
    let brrrty_exit = status.code();
    if brrrty_exit == Some(101) {
        eprintln!("!!! BRRRTY PANICKED (exit 101) while building {site} !!!");
    }

    let report =
        compare_dirs(&reference, &actual, &allow).map_err(|e| format!("comparison failed: {e}"))?;
    let report_path = work.join("report.md");
    fs::write(&report_path, report.to_markdown(200))
        .map_err(|e| format!("cannot write {}: {e}", report_path.display()))?;
    Ok(SiteRun {
        counts: report.counts(),
        reference_files: count_files(&reference),
        brrrty_exit,
        report_path,
        pass: report.is_pass(),
    })
}

fn exit_label(code: Option<i32>) -> String {
    match code {
        Some(101) => "101 PANIC".into(),
        Some(c) => c.to_string(),
        None => "killed by signal".into(),
    }
}

pub fn run_one(
    root: &Path,
    site: &str,
    sites_dir: &Path,
    settings_dir: &Path,
    refresh: bool,
) -> Result<bool, String> {
    // Fail on a missing settings file before spending time on a build.
    check_site_name(site)?;
    let settings_file = settings_path(settings_dir, site);
    if !settings_file.is_file() {
        return Err(format!(
            "missing settings file: expected {}",
            settings_file.display()
        ));
    }
    build_release(root)?;
    let run = run_site(root, site, sites_dir, settings_dir, refresh)?;
    let c = run.counts;
    println!(
        "identical {} | equivalent {} | different {} | missing {} | extra {}",
        c.identical, c.equivalent, c.different, c.missing, c.extra
    );
    let pct = if run.reference_files == 0 {
        0.0
    } else {
        c.identical as f64 * 100.0 / run.reference_files as f64
    };
    println!(
        "byte-identical: {pct:.1}% ({} of {} reference files)",
        c.identical, run.reference_files
    );
    println!("brrrty exit: {}", exit_label(run.brrrty_exit));
    println!("report: {}", run.report_path.display());
    Ok(run.pass)
}

pub fn scoreboard(root: &Path, sites_dir: &Path, settings_dir: &Path) -> Result<bool, String> {
    let entries = fs::read_dir(sites_dir)
        .map_err(|e| format!("cannot read sites folder {}: {e}", sites_dir.display()))?;
    let mut sites: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| settings_path(settings_dir, n).is_file())
        .collect();
    sites.sort();
    build_release(root)?;

    println!("| site | identical | equivalent | different | missing | extra | brrrty exit |");
    println!("|---|---|---|---|---|---|---|");
    for site in sites {
        match run_site(root, &site, sites_dir, settings_dir, false) {
            Ok(r) => {
                let c = r.counts;
                println!(
                    "| {site} | {} | {} | {} | {} | {} | {} |",
                    c.identical,
                    c.equivalent,
                    c.different,
                    c.missing,
                    c.extra,
                    exit_label(r.brrrty_exit)
                );
            }
            Err(e) => {
                let one_line = e.lines().next().unwrap_or("error");
                println!("| {site} | - | - | - | - | - | ERROR: {one_line} |");
            }
        }
    }
    Ok(true)
}
