use std::process::Command;

#[test]
fn version_flag_prints_name_and_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        "brrrty 0.0.1"
    );
}

#[test]
fn unknown_subcommand_is_usage_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .arg("frobnicate")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

fn site(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    dir
}

fn run_in(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}

fn stdout(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn builds_and_prints_summary() {
    let dir = site(&[("src/index.md", "# Hi\n")]);
    let out = run_in(dir.path(), &["build", "--input", "src"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).starts_with("Wrote 1 files, copied 0 files in "));
    assert!(dir.path().join("_site/index.html").exists());
}

#[test]
fn quiet_suppresses_summary() {
    let dir = site(&[("src/index.md", "# Hi\n")]);
    let out = run_in(dir.path(), &["--input", "src", "--quiet"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "");
}

#[test]
fn bare_command_means_build() {
    let dir = site(&[("src/index.md", "# Hi\n")]);
    let out = run_in(dir.path(), &["--input", "src"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(dir.path().join("_site/index.html").exists());
}

#[test]
fn errors_exit_1_and_show_location_and_issue_link() {
    let dir = site(&[
        ("src/bad.md", "---\ntitle: [oops\n---\nbody\n"),
        ("src/c.njk", "{{ secret }}\n"),
    ]);
    let out = run_in(dir.path(), &["--input", "src"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("front-matter-invalid") && err.contains("bad.md"),
        "{err}"
    );
    assert!(err.contains("title: [oops"), "{err}");
    assert!(!err.contains("{{ secret }}"), "{err}");
    assert!(
        err.contains("https://github.com/brrrty/brrrty/issues/new?title=Unsupported"),
        "{err}"
    );
    assert!(err.contains("errors"), "{err}");
}

#[test]
fn refuses_output_over_source() {
    let dir = site(&[("src/index.md", "# Hi\n")]);
    let out = run_in(dir.path(), &["--input", "src", "--output", "src"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("output-overlaps-source"));
}

#[test]
fn issue_url_is_encoded() {
    assert_eq!(
        brrrty::issue_url("a b", "c&d"),
        "https://github.com/brrrty/brrrty/issues/new?title=a%20b&body=c%26d"
    );
}
