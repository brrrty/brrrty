use brrrty_core::codes;
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
        "brrrty 0.0.2"
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
        err.contains(codes::FRONT_MATTER_INVALID) && err.contains("bad.md"),
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
    assert!(stderr(&out).contains(codes::OUTPUT_OVERLAPS_SOURCE));
}

#[test]
fn issue_url_is_encoded() {
    assert_eq!(
        brrrty::issue_url("a b", "c&d"),
        "https://github.com/brrrty/brrrty/issues/new?title=a%20b&body=c%26d"
    );
}

#[test]
fn top_level_args_with_subcommand_is_usage_error() {
    let dir = site(&[("src/index.md", "# Hi\n")]);
    let out = run_in(dir.path(), &["--input", "src", "build"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn problem_location_appears_once_and_counts_are_singular() {
    let dir = site(&[("src/bad.md", "---\ntitle: [oops\n---\nbody\n")]);
    let out = run_in(dir.path(), &["--input", "src"]);
    let err = stderr(&out);
    assert_eq!(err.matches("bad.md:3:1").count(), 1, "{err}");
    assert!(err.contains("1 error, 0 warnings"), "{err}");
}

#[test]
fn unsupported_without_snippet_keeps_path_in_message() {
    let dir = site(&[("src/c.njk", "x\n")]);
    let err = stderr(&run_in(dir.path(), &["--input", "src"]));
    assert!(err.contains("c.njk: "), "{err}");
}

#[test]
fn missing_input_names_the_path_once() {
    let dir = site(&[("other/index.md", "x\n")]);
    let out = run_in(dir.path(), &["--input", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains(codes::INPUT_MISSING), "{err}");
    assert_eq!(err.matches("nope").count(), 1, "{err}");
}

#[test]
fn file_as_input_names_the_path() {
    let dir = site(&[("page.md", "x\n")]);
    let out = run_in(dir.path(), &["--input", "page.md"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains(codes::INPUT_NOT_A_DIRECTORY), "{err}");
    assert!(err.contains("page.md"), "{err}");
    assert!(!err.contains("× :"), "{err}");
}

#[test]
fn issue_link_for_11ty_js_templates_names_the_full_extension() {
    let dir = site(&[("src/a.11ty.js", "x\n"), ("src/b.njk", "x\n")]);
    let err = stderr(&run_in(dir.path(), &["--input", "src"]));
    assert!(err.contains("%20.11ty.js%20templates"), "{err}");
    assert!(err.contains("%20.njk%20templates"), "{err}");
    assert!(!err.contains("Unsupported%3A%20.js%20"), "{err}");
}

#[test]
fn strict_stops_at_the_first_error_but_not_for_warnings() {
    let dir = site(&[
        ("src/bad1.md", "---\ntitle: [oops\n---\nx\n"),
        ("src/bad2.md", "---\ntitle: [oops\n---\nx\n"),
    ]);
    let out = run_in(dir.path(), &["--input", "src", "--strict"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("1 error, 0 warnings"),
        "{}",
        stderr(&out)
    );

    let ok = site(&[("src/index.md", "# Hi\n")]);
    let out = run_in(
        ok.path(),
        &["--input", "src", "--strict", "--passthrough", "src/missing"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(codes::PASSTHROUGH_MISSING),
        "{}",
        stderr(&out)
    );
}

#[test]
fn strict_help_describes_stopping_at_the_first_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .args(["build", "--help"])
        .output()
        .unwrap();
    assert!(stdout(&out).contains("Stop at the first error (for CI)"));
}

#[test]
fn error_snippet_uses_root_relative_path() {
    let dir = site(&[
        ("src/bad.md", "---\ntitle: [unclosed\n---\nbody\n"),
        ("src/posts/raw.md", "{{ x }}\n"),
    ]);
    let out = run_in(dir.path(), &["build", "--input", "src"]);
    let err = stderr(&out);
    assert!(err.contains("src/bad.md:"), "{err}");
    assert!(err.contains("title: [unclosed"), "{err}");
}

#[test]
fn plugin_only_extensions_exit_zero() {
    let dir = site(&[
        ("src/a.webc", "x"),
        ("src/b.pug", "x"),
        ("src/i.md", "hi\n"),
    ]);
    let out = run_in(dir.path(), &["build", "--input", "src"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

#[cfg(unix)]
#[test]
fn dangling_symlink_exits_zero() {
    let dir = site(&[("src/i.md", "hi\n")]);
    std::os::unix::fs::symlink("nowhere.md", dir.path().join("src/dangling.md")).unwrap();
    let out = run_in(dir.path(), &["build", "--input", "src"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

#[test]
fn input_outside_the_root_builds_without_crashing() {
    let dir = site(&[
        ("site/.gitignore", "drafts\n"),
        ("real/index.md", "# Hi\n"),
        ("real/drafts/a.md", "draft\n"),
        ("real/.eleventyignore", "drafts\n"),
    ]);
    let root = dir.path().join("site");
    let out = run_in(&root, &["build", "--input", "../real"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(root.join("_site/index.html").exists());
    assert!(!root.join("_site/drafts").exists());
}
