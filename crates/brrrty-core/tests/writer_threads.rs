//! Output is written on a pool of its own on macOS, and passthrough files are
//! copied in parallel. A build must come out the same as with one thread.
//!
//! Each scenario runs in a child process twice: once with one render thread
//! and no writer pool (every write one at a time, in walk order), and once
//! with a forced 4-thread writer pool. The two reports and output trees must
//! match. The pool is process-wide, so it can only be forced per process.

use brrrty_core::{BuildOptions, BuildReport, PassthroughEntry, build, codes};
use brrrty_markdown::MarkdownIt;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SCENARIO: &str = "BRRRTY_WRITER_SCENARIO";

fn write(root: &Path, rel: &str, body: impl AsRef<[u8]>) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn opts(root: &Path, entries: &[(&str, Option<&str>)]) -> BuildOptions {
    let config = brrrty_core::ConfigOptions {
        passthrough: entries
            .iter()
            .map(|(from, to)| PassthroughEntry {
                from: from.to_string(),
                to: to.map(str::to_string),
            })
            .collect(),
        ..Default::default()
    };
    BuildOptions {
        write_to: None,
        run_mode: Default::default(),
        root: root.to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        passthrough: Vec::new(),
        strict: false,
        config,
    }
}

/// Everything a build reports and writes, with the temporary root masked.
fn summary(root: &Path, r: &BuildReport) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "errors={} written={} copied={} unchanged={}",
        r.has_errors(),
        r.written,
        r.copied,
        r.unchanged
    );
    for p in &r.problems {
        let _ = writeln!(s, "problem {} {:?} {}", p.code, p.path, p.message);
    }
    for f in &r.passthrough {
        let _ = writeln!(s, "file {f:?}");
    }
    let mut tree = Vec::new();
    walk(&root.join("_site"), &root.join("_site"), &mut tree);
    tree.sort();
    for t in tree {
        let _ = writeln!(s, "out {t}");
    }
    s.replace(&root.display().to_string(), "<root>")
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let rel = p.strip_prefix(base).unwrap().display().to_string();
        if p.is_dir() {
            out.push(format!("{rel}/"));
            walk(base, &p, out);
        } else {
            out.push(format!("{rel} {:?}", fs::read(&p).unwrap()));
        }
    }
}

/// A file and a folder both mapped to `x`: one at a time, `a.txt` lands at
/// `x` and every file of the folder fails with "File exists".
fn file_and_folder_on_one_path(root: &Path) -> String {
    write(root, "src/index.md", "hi");
    write(root, "a.txt", "file");
    for i in 0..40 {
        write(root, &format!("d/f{i:02}.txt"), format!("{i}"));
    }
    for i in 0..200 {
        write(root, &format!("other/o{i:03}.txt"), format!("{i}"));
    }
    let o = opts(
        root,
        &[("a.txt", Some("x")), ("d", Some("x")), ("other", None)],
    );
    let r = build(&o, &MarkdownIt::new());
    assert_eq!(fs::read_to_string(root.join("_site/x")).unwrap(), "file");
    let failed: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::PASSTHROUGH_COPY_FAILED)
        .collect();
    assert_eq!(failed.len(), 40, "{:?}", r.problems);
    assert_eq!(failed[0].path.as_deref(), Some(Path::new("d/f00.txt")));
    summary(root, &r)
}

/// Names APFS treats as one file: the last in walk order wins, as it did
/// when every copy ran one at a time.
fn names_differing_by_case(root: &Path) -> String {
    write(root, "src/index.md", "hi");
    write(root, "big.bin", vec![b'b'; 1 << 20]);
    write(root, "small.txt", "small");
    let o = opts(
        root,
        &[
            ("big.bin", Some("y/Name.bin")),
            ("small.txt", Some("y/name.bin")),
        ],
    );
    let r = build(&o, &MarkdownIt::new());
    summary(root, &r)
}

/// One unreadable source among many: its error and every other copy are the
/// same as one at a time.
#[cfg(unix)]
fn a_copy_fails_mid_batch(root: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    write(root, "src/index.md", "hi");
    for i in 0..300 {
        write(root, &format!("assets/f{i:03}.txt"), format!("{i}"));
    }
    let bad = root.join("assets/f150.txt");
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&bad).is_ok() {
        // Root reads anything; there is nothing to fail.
        return "unreadable sources are readable here".into();
    }
    let r = build(&opts(root, &[("assets", None)]), &MarkdownIt::new());
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(r.has_errors());
    let failed: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::PASSTHROUGH_COPY_FAILED)
        .collect();
    assert_eq!(failed.len(), 1, "{:?}", r.problems);
    assert_eq!(
        failed[0].path.as_deref(),
        Some(Path::new("assets/f150.txt"))
    );
    assert_eq!(r.copied, 299);
    summary(root, &r)
}

/// Copies keep the source's modification time, as 11ty's copy does.
fn mtimes_are_kept(root: &Path) -> String {
    write(root, "src/index.md", "hi");
    for i in 0..50 {
        let rel = format!("assets/f{i:02}.txt");
        write(root, &rel, format!("{i}"));
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000 + i);
        fs::File::options()
            .write(true)
            .open(root.join(&rel))
            .unwrap()
            .set_modified(when)
            .unwrap();
    }
    let r = build(&opts(root, &[("assets", None)]), &MarkdownIt::new());
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    for i in 0..50 {
        let rel = format!("assets/f{i:02}.txt");
        let src = fs::metadata(root.join(&rel)).unwrap().modified().unwrap();
        let dest = fs::metadata(root.join("_site").join(&rel))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(src, dest, "{rel}");
    }
    summary(root, &r)
}

fn run(name: &str) -> String {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    match name {
        "file_and_folder" => file_and_folder_on_one_path(&root),
        "case" => names_differing_by_case(&root),
        #[cfg(unix)]
        "fails_mid_batch" => a_copy_fails_mid_batch(&root),
        "mtimes" => mtimes_are_kept(&root),
        _ => panic!("unknown scenario {name}"),
    }
}

/// Run `name` in a child process, serially or on a forced writer pool.
fn child(test: &str, name: &str, pooled: bool) -> String {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env(SCENARIO, name)
        .env_remove("BRRRTY_WRITER_THREADS");
    if pooled {
        cmd.env("RAYON_NUM_THREADS", "10")
            .env("BRRRTY_WRITER_THREADS", "4");
    } else {
        cmd.env("RAYON_NUM_THREADS", "1");
    }
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{name} pooled={pooled}\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let start = stdout.find("<<<").expect("child printed its summary") + 3;
    let end = stdout.find(">>>").unwrap();
    stdout[start..end].to_string()
}

fn same_either_way(test: &str, name: &str) {
    if let Ok(name) = std::env::var(SCENARIO) {
        let pooled = std::env::var("BRRRTY_WRITER_THREADS").is_ok();
        let s = run(&name);
        // The forced pool really ran.
        let threads = brrrty_core::io_threads::separate_pool_threads();
        assert_eq!(threads, pooled.then_some(4));
        println!("<<<{s}>>>");
        return;
    }
    let serial = child(test, name, false);
    let pooled = child(test, name, true);
    assert_eq!(
        serial, pooled,
        "{name}: one thread and the writer pool differ"
    );
}

#[test]
fn file_and_folder_on_one_path_match_one_thread() {
    same_either_way(
        "file_and_folder_on_one_path_match_one_thread",
        "file_and_folder",
    );
}

#[test]
fn names_differing_by_case_match_one_thread() {
    same_either_way("names_differing_by_case_match_one_thread", "case");
}

#[test]
#[cfg(unix)]
fn a_copy_failing_mid_batch_matches_one_thread() {
    same_either_way(
        "a_copy_failing_mid_batch_matches_one_thread",
        "fails_mid_batch",
    );
}

#[test]
fn mtimes_are_kept_on_the_writer_pool() {
    same_either_way("mtimes_are_kept_on_the_writer_pool", "mtimes");
}
