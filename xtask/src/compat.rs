//! `cargo xtask compat` and `cargo xtask scoreboard`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use brrrty_compat::{AllowList, Counts, compare_dirs};
use sha2::{Digest, Sha256};

use crate::settings::SiteSettings;

struct SiteRun {
    eleventy_version: String,
    brrrty_exit: Option<i32>,
    reference_files: usize,
    report_path: PathBuf,
    /// Comparison outcome; an Err keeps the brrrty exit visible.
    compare: Result<(Counts, bool), String>,
}

impl SiteRun {
    fn gate_pass(&self) -> bool {
        matches!(self.compare, Ok((_, p)) if passes(p, self.brrrty_exit))
    }
}

/// A run passes only when the comparison is clean and brrrty ran without panicking.
fn passes(compare_pass: bool, brrrty_exit: Option<i32>) -> bool {
    compare_pass && matches!(brrrty_exit, Some(c) if c != 101)
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

/// Hash every input that can change the reference output. Each part is
/// length-prefixed so parts can't run into each other.
fn stamp_from_parts(
    head: &str,
    diff: &[u8],
    untracked: &[(String, Vec<u8>)],
    lockfile: Option<&[u8]>,
) -> String {
    let mut h = Sha256::new();
    let mut part = |tag: &str, bytes: &[u8]| {
        h.update(tag.as_bytes());
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    };
    part("diff", diff);
    let mut sorted: Vec<&(String, Vec<u8>)> = untracked.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, content) in sorted {
        part("untracked-name", name.as_bytes());
        part("untracked-content", content);
    }
    match lockfile {
        Some(l) => part("lock", l),
        None => part("nolock", b""),
    }
    let digest: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("{head}\n{digest}\n")
}

/// The reference stamp for a site, or None when it isn't in a working git
/// repo (then the reference is never reused).
fn compute_stamp(site_dir: &Path) -> Option<String> {
    let head = String::from_utf8_lossy(&git_output(site_dir, &["rev-parse", "HEAD"])?)
        .trim()
        .to_string();
    let diff = git_output(site_dir, &["diff", "HEAD", "--", "."])?;
    let list = git_output(
        site_dir,
        &["ls-files", "-z", "--others", "--exclude-standard"],
    )?;
    let mut untracked = Vec::new();
    for name in list.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        let name = String::from_utf8_lossy(name).into_owned();
        // Unreadable (e.g. a broken symlink) still changes the stamp.
        let content = fs::read(site_dir.join(&name)).unwrap_or_default();
        untracked.push((name, content));
    }
    let lock = fs::read(site_dir.join("package-lock.json")).ok();
    Some(stamp_from_parts(&head, &diff, &untracked, lock.as_deref()))
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

/// The installed Eleventy version, from the site's own node_modules.
fn eleventy_version(site_dir: &Path) -> String {
    let pkg = site_dir.join("node_modules/@11ty/eleventy/package.json");
    fs::read_to_string(pkg)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("version")?.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

fn ensure_reference(
    site_dir: &Path,
    work: &Path,
    refresh: bool,
) -> Result<(PathBuf, String), String> {
    let reference = work.join("reference");
    let stamp_path = work.join("reference.stamp");
    let stamp = compute_stamp(site_dir);
    if !refresh
        && reference.is_dir()
        && let Some(stamp) = &stamp
        && fs::read_to_string(&stamp_path).is_ok_and(|s| &s == stamp)
    {
        println!("reference: reusing {}", reference.display());
        return Ok((reference, eleventy_version(site_dir)));
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
    let bin = site_dir.join("node_modules/.bin/eleventy");
    if !bin.exists() {
        return Err(format!(
            "Eleventy is not installed in the site: {} is missing (is @11ty/eleventy in its package.json and lockfile?)",
            bin.display()
        ));
    }
    let version = eleventy_version(site_dir);
    if reference.exists() {
        fs::remove_dir_all(&reference)
            .map_err(|e| format!("cannot delete {}: {e}", reference.display()))?;
    }
    // Make sure a stale stamp can't outlive a failed build.
    let _ = fs::remove_file(&stamp_path);
    fs::create_dir_all(work).map_err(|e| format!("cannot create {}: {e}", work.display()))?;
    println!("reference: building with Eleventy {version}");
    let out = Command::new(&bin)
        .arg("--quiet")
        .arg(format!("--output={}", reference.display()))
        .current_dir(site_dir)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", bin.display()))?;
    if !out.status.success() {
        return Err(format!(
            "reference build (Eleventy) failed ({}), first lines of stderr:\n{}",
            out.status,
            first_lines(&out.stderr, 20)
        ));
    }
    if let Some(stamp) = stamp {
        fs::write(&stamp_path, stamp)
            .map_err(|e| format!("cannot write {}: {e}", stamp_path.display()))?;
    }
    Ok((reference, version))
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
    let (reference, eleventy_version) = ensure_reference(&site_dir, &work, refresh)?;

    let actual = work.join("brrrty");
    if actual.exists() {
        fs::remove_dir_all(&actual)
            .map_err(|e| format!("cannot delete {}: {e}", actual.display()))?;
    }
    // A panicking brrrty may create nothing; the comparison still needs a folder.
    fs::create_dir_all(&actual).map_err(|e| format!("cannot create {}: {e}", actual.display()))?;
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

    let report_path = work.join("report.md");
    let compare = compare_dirs(&reference, &actual, &allow)
        .map_err(|e| format!("comparison failed: {e}"))
        .and_then(|report| {
            fs::write(&report_path, report.to_markdown(200))
                .map_err(|e| format!("cannot write {}: {e}", report_path.display()))?;
            Ok((report.counts(), report.is_pass()))
        });
    Ok(SiteRun {
        eleventy_version,
        brrrty_exit,
        reference_files: count_files(&reference),
        report_path,
        compare,
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
    println!("11ty {}", run.eleventy_version);
    println!("brrrty exit: {}", exit_label(run.brrrty_exit));
    match &run.compare {
        Ok((c, _)) => {
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
            println!("report: {}", run.report_path.display());
        }
        Err(e) => eprintln!("error: {e}"),
    }
    Ok(run.gate_pass())
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

    println!(
        "| site | 11ty | identical | equivalent | different | missing | extra | brrrty exit |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for site in sites {
        match run_site(root, &site, sites_dir, settings_dir, false) {
            Ok(r) => {
                let exit = exit_label(r.brrrty_exit);
                match &r.compare {
                    Ok((c, _)) => println!(
                        "| {site} | {} | {} | {} | {} | {} | {} | {exit} |",
                        r.eleventy_version,
                        c.identical,
                        c.equivalent,
                        c.different,
                        c.missing,
                        c.extra
                    ),
                    Err(e) => println!(
                        "| {site} | {} | - | - | - | - | - | {exit} (ERROR: {}) |",
                        r.eleventy_version,
                        e.lines().next().unwrap_or("error")
                    ),
                }
            }
            Err(e) => {
                let one_line = e.lines().next().unwrap_or("error");
                println!("| {site} | - | - | - | - | - | - | ERROR: {one_line} |");
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(diff: &[u8], untracked: &[(&str, &[u8])], lock: Option<&[u8]>) -> String {
        let u: Vec<(String, Vec<u8>)> = untracked
            .iter()
            .map(|(n, c)| (n.to_string(), c.to_vec()))
            .collect();
        stamp_from_parts("abc", diff, &u, lock)
    }

    #[test]
    fn stamp_is_stable_and_order_independent() {
        let a = stamp(b"d", &[("a", b"1"), ("b", b"2")], Some(b"L"));
        let b = stamp(b"d", &[("b", b"2"), ("a", b"1")], Some(b"L"));
        assert_eq!(a, b);
        assert!(a.starts_with("abc\n"));
    }

    #[test]
    fn stamp_changes_with_each_input() {
        let base = stamp(b"d", &[("a", b"1")], Some(b"L"));
        assert_ne!(base, stamp(b"d2", &[("a", b"1")], Some(b"L")));
        assert_ne!(base, stamp(b"d", &[("a", b"changed")], Some(b"L")));
        assert_ne!(base, stamp(b"d", &[("renamed", b"1")], Some(b"L")));
        assert_ne!(base, stamp(b"d", &[("a", b"1")], Some(b"L2")));
        assert_ne!(base, stamp(b"d", &[("a", b"1")], None));
        assert_ne!(
            base,
            stamp_from_parts("other", b"d", &[("a".into(), b"1".to_vec())], Some(b"L"))
        );
    }

    #[test]
    fn stamp_parts_cannot_run_together() {
        assert_ne!(stamp(b"ab", &[], None), stamp(b"a", &[("b", b"")], None));
    }

    #[test]
    fn panic_or_signal_fails_the_gate() {
        assert!(passes(true, Some(0)));
        assert!(passes(true, Some(1)));
        assert!(!passes(true, Some(101)));
        assert!(!passes(true, None));
        assert!(!passes(false, Some(0)));
    }

    #[test]
    fn exit_labels() {
        assert_eq!(exit_label(Some(0)), "0");
        assert_eq!(exit_label(Some(1)), "1");
        assert_eq!(exit_label(Some(101)), "101 PANIC");
        assert_eq!(exit_label(None), "killed by signal");
    }

    #[test]
    fn compute_stamp_is_none_outside_git() {
        let d = std::env::temp_dir().join(format!("xtask-nogit-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        // A temp dir is not inside a git repo on CI or dev machines.
        assert!(compute_stamp(&d).is_none());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn eleventy_version_reads_package_json() {
        let d = std::env::temp_dir().join(format!("xtask-ver-{}", std::process::id()));
        let pkg = d.join("node_modules/@11ty/eleventy");
        fs::create_dir_all(&pkg).unwrap();
        assert_eq!(eleventy_version(&d), "unknown");
        fs::write(pkg.join("package.json"), r#"{"version":"3.1.8"}"#).unwrap();
        assert_eq!(eleventy_version(&d), "3.1.8");
        fs::remove_dir_all(&d).unwrap();
    }
}
