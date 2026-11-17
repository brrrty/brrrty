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
        concat!("brrrty ", env!("CARGO_PKG_VERSION"))
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
    assert!(stdout(&out).starts_with("Wrote 1 file, copied 0 files in "));
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
        ("src/c.11ty.js", "module.exports = '{{ secret }}';\n"),
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
    let dir = site(&[("src/c.11ty.js", "x\n")]);
    let err = stderr(&run_in(dir.path(), &["--input", "src"]));
    assert!(err.contains("c.11ty.js: "), "{err}");
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
    let dir = site(&[("src/a.11ty.js", "x\n"), ("src/b.11ty.cjs", "x\n")]);
    let err = stderr(&run_in(dir.path(), &["--input", "src"]));
    assert!(err.contains("%20.11ty.js%20templates"), "{err}");
    assert!(err.contains("%20.11ty.cjs%20templates"), "{err}");
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

/// Eleventy's `slug` and `slugify` stringify a `Date` as JavaScript does,
/// in the local zone and with the zone's name, and `url` throws on one.
/// node with `@11ty/eleventy/src/Filters/{Slug,Slugify,Url}.js` on
/// `new Date(Date.UTC(2026, 2, 4))`, the `page.date` of `date: 2026-03-04`.
#[test]
fn slug_of_a_date_prints_it_in_the_local_zone() {
    let cases = [
        (
            "UTC",
            "wed-mar-04-2026-00:00:00-gmt+0000-(coordinated-universal-time)",
            "wed-mar-04-2026-00-00-00-gmt-0000-coordinated-universal-time",
        ),
        (
            "America/New_York",
            "tue-mar-03-2026-19:00:00-gmt-0500-(eastern-standard-time)",
            "tue-mar-03-2026-19-00-00-gmt-0500-eastern-standard-time",
        ),
    ];
    for (tz, slug, slugify) in cases {
        let dir = site(&[
            (
                "src/index.liquid",
                "---\ndate: 2026-03-04\n---\n{{ page.date | slug }}|{{ page.date | slugify }}",
            ),
            (
                "src/url.liquid",
                "---\ndate: 2026-03-04\n---\n{{ page.date | url }}",
            ),
        ]);
        let out = Command::new(env!("CARGO_BIN_EXE_brrrty"))
            .args(["--input", "src"])
            .env("TZ", tz)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{tz}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("url.startsWith is not a function"),
            "{tz}: {}",
            stderr(&out)
        );
        let html = std::fs::read_to_string(dir.path().join("_site/index.html")).unwrap();
        assert_eq!(html, format!("{slug}|{slugify}"), "{tz}");
    }
}

#[test]
fn the_config_sets_the_folders_and_flags_win() {
    let d = site(&[
        (
            "brrrty.config.rhai",
            "#{ dir: #{ input: \"src\", output: \"dist\" } }\n",
        ),
        ("src/a.md", "a\n"),
        ("other/b.md", "b\n"),
    ]);
    let o = run_in(d.path(), &["build", "--quiet"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(d.path().join("dist/a/index.html").is_file());
    // 11ty's command line overrides its config's `dir`.
    let o = run_in(
        d.path(),
        &["build", "--quiet", "--input", "other", "--output", "out"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(d.path().join("out/b/index.html").is_file());
    assert!(!d.path().join("out/a").exists());
}

#[test]
fn a_config_error_stops_before_building() {
    let d = site(&[
        ("brrrty.config.rhai", "let a = 1;\nlet b = ;\n"),
        ("a.md", "a\n"),
    ]);
    let o = run_in(d.path(), &["build"]);
    assert_eq!(o.status.code(), Some(1));
    let err = stderr(&o);
    assert!(err.contains(codes::CONFIG_SYNTAX), "{err}");
    assert!(err.contains("brrrty.config.rhai:2:9"), "{err}");
    assert!(!d.path().join("_site").exists());
}

#[test]
fn a_bad_source_date_epoch_is_an_error() {
    let d = site(&[("a.md", "a\n")]);
    let o = Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .current_dir(d.path())
        .env("SOURCE_DATE_EPOCH", "soon")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains(codes::SOURCE_DATE_EPOCH_INVALID),
        "{}",
        stderr(&o)
    );
}

#[test]
fn rhai_data_files_work_without_a_config_and_follow_source_date_epoch() {
    // 2023-11-14T12:00:00Z
    let d = site(&[
        ("src/_data/ms.rhai", "get_time(now())"),
        ("src/_data/site.rhai", "#{ name: \"Mine\" }"),
        ("src/_data/site.js", "module.exports = () => ({});"),
        ("src/index.liquid", "{{ site.name }} {{ ms }}"),
    ]);
    let out = Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .args(["--input", "src"])
        .env("SOURCE_DATE_EPOCH", "1699963200")
        .current_dir(d.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let html = std::fs::read_to_string(d.path().join("_site/index.html")).unwrap();
    assert_eq!(html, "Mine 1699963200000");
    // The shadowed JavaScript file is a note, not a warning or an error.
    assert!(stderr(&out).contains("site.rhai supplies this key"));
    assert!(
        stderr(&out).contains("0 errors, 0 warnings, 1 note"),
        "{}",
        stderr(&out)
    );
}

/// The panic hook stands down inside containment, so a Markdown panic (an integer overflow in
/// markdown-it, so only in debug builds) refuses its page and the build exits 1, not 101. Many
/// copies run across the Rayon workers. In a release build the overflow wraps and the page
/// matches markdown-it 14, printed by Node.
#[test]
fn a_markdown_panic_is_a_page_error_not_a_crash() {
    let mut files = vec![("src/ok.md".to_string(), "# fine\n".to_string())];
    for i in 0..24 {
        files.push((format!("src/bad{i}.md"), "2) `~~中\r\n---~~(![i](u)".into()));
    }
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let t = site(&files);
    let out = run_in(t.path(), &[]);
    let err = stderr(&out);
    assert!(!err.contains("internal bug"), "{err}");
    assert_eq!(
        std::fs::read_to_string(t.path().join("_site/src/ok/index.html")).unwrap(),
        "<h1>fine</h1>\n"
    );
    if cfg!(debug_assertions) {
        assert_eq!(out.status.code(), Some(1), "{err}");
        assert_eq!(err.matches("markdown-render-failed").count(), 24, "{err}");
        assert!(err.contains("markdown-it-0.6.1/src/"), "{err}");
        assert!(!t.path().join("_site/src/bad0").exists());
    } else {
        assert_eq!(out.status.code(), Some(0), "{err}");
        assert_eq!(
            std::fs::read_to_string(t.path().join("_site/src/bad0/index.html")).unwrap(),
            "<ol start=\"2\">\n<li>`<s>中\n---</s>(<img src=\"u\" alt=\"i\"></li>\n</ol>\n"
        );
    }
}

/// Outside containment the hook still reports the bug and exits 101.
#[cfg(debug_assertions)]
#[test]
fn an_uncontained_panic_is_a_bug_report_and_101() {
    let t = site(&[]);
    let out = run_in(t.path(), &["--__panic"]);
    assert_eq!(out.status.code(), Some(101));
    let err = stderr(&out);
    assert!(err.contains("Brrrty hit an internal bug"), "{err}");
    assert!(err.contains("deliberate test panic"), "{err}");
    assert!(err.contains("issues/new?"), "{err}");
}

/// A panic while a script runs is a Problem through the real binary, panic hook and all: at the
/// config's top level it stops the build (exit 1), in a data file it refuses the pages that use
/// its data, and in a filter or collection it refuses its pages. Debug builds only, where
/// scripts can call `__brrrty_test_panic()`.
#[cfg(debug_assertions)]
#[test]
fn script_panics_are_problems_not_crashes() {
    let top = site(&[
        ("brrrty.config.rhai", "__brrrty_test_panic();\n"),
        ("a.md", "a\n"),
    ]);
    let o = run_in(top.path(), &[]);
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(err.contains(codes::CONFIG_ERROR), "{err}");
    assert!(err.contains("deliberate test panic from a script"), "{err}");
    assert!(!err.contains("internal bug"), "{err}");

    let data = site(&[
        ("_data/boom.rhai", "__brrrty_test_panic()"),
        ("uses.liquid", "{{ boom }}"),
        ("ok.md", "ok\n"),
    ]);
    let o = run_in(data.path(), &[]);
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(err.contains(codes::DATA_RHAI_ERROR), "{err}");
    assert!(err.contains("deliberate test panic from a script"), "{err}");
    assert!(!err.contains("internal bug"), "{err}");

    let filter = site(&[
        (
            "brrrty.config.rhai",
            "config.add_filter(\"boom\", |v| __brrrty_test_panic());\n\
             config.add_collection(\"bad\", |api| __brrrty_test_panic());\n",
        ),
        ("f.liquid", "{{ 1 | boom }}"),
        ("c.liquid", "{{ collections.bad }}"),
        ("ok.md", "ok\n"),
    ]);
    let o = run_in(filter.path(), &[]);
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(
        err.contains("filter `boom` failed: deliberate test panic"),
        "{err}"
    );
    assert_eq!(err.matches("deliberate test panic").count(), 3, "{err}");
    assert!(!err.contains("internal bug"), "{err}");
    assert!(filter.path().join("_site/ok/index.html").is_file());
    assert!(!filter.path().join("_site/f/index.html").exists());
    assert!(!filter.path().join("_site/c/index.html").exists());
}

/// A feed post nested 100,000 elements deep used to overflow the stack and abort (exit 134).
/// Its feed is now refused with `feed-failed` (11ty's posthtml runs out of stack and stops the
/// build), and every other page is written.
#[test]
fn a_deeply_nested_feed_post_refuses_the_feed_not_the_build() {
    let deep = format!(
        "---\ntags: posts\ntitle: Deep\n---\n{}x{}\n",
        "<div>".repeat(100_000),
        "</div>".repeat(100_000)
    );
    let t = site(&[
        (
            "brrrty.config.rhai",
            "config.add_plugin(\"rss\", #{ type: \"atom\", output_path: \"/feed.xml\", \
             collection: #{ name: \"posts\" }, metadata: #{ title: \"T\", base: \"https://example.com/\" } });\n",
        ),
        ("posts/deep.html", deep.as_str()),
        ("posts/ok.md", "---\ntags: posts\ntitle: Ok\n---\nok\n"),
        ("index.md", "home\n"),
    ]);
    let out = run_in(t.path(), &[]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("feed-failed"), "{err}");
    assert!(err.contains("more than 2000 deep"), "{err}");
    assert!(!t.path().join("_site/feed.xml").exists());
    assert!(t.path().join("_site/index.html").is_file());
    assert!(t.path().join("_site/posts/ok/index.html").is_file());
}

/// Runs `brrrty args` in `root` with stdout and stderr connected to pipes
/// whose read ends are already closed, as in `brrrty build 2>&1 | head -1`
/// once `head` exits: every write fails with a broken pipe.
fn run_with_closed_pipes(root: &std::path::Path, args: &[&str]) -> std::process::ExitStatus {
    let mut child = Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .args(args)
        .current_dir(root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    drop(child.stderr.take());
    child.wait().unwrap()
}

/// A closed stdout or stderr doesn't abort (exit 134) or panic: the build
/// runs, and the exit code is the build's own.
#[test]
fn a_closed_output_pipe_keeps_the_builds_exit_code() {
    let mut files: Vec<(String, String)> = (0..30)
        .map(|i| (format!("bad{i}.njk"), "{{ nope | nofilter }}\n".to_string()))
        .collect();
    files.push(("ok.md".to_string(), "ok\n".to_string()));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let t = site(&refs);
    let status = run_with_closed_pipes(t.path(), &[]);
    assert_eq!(status.code(), Some(1), "{status:?}");
    assert!(t.path().join("_site/ok/index.html").exists());

    let clean = site(&[("a.md", "a\n")]);
    let status = run_with_closed_pipes(clean.path(), &[]);
    assert_eq!(status.code(), Some(0), "{status:?}");

    let logs = site(&[
        ("brrrty.config.rhai", "print(\"hi\");\ndebug(\"there\");\n"),
        ("a.njk", "{{ \"x\" | log }}\n"),
    ]);
    let status = run_with_closed_pipes(logs.path(), &[]);
    assert_eq!(status.code(), Some(0), "{status:?}");
    assert_eq!(
        std::fs::read_to_string(logs.path().join("_site/a/index.html")).unwrap(),
        "x\n"
    );
}

/// The panic hook itself doesn't panic on a closed stderr: still 101.
#[cfg(debug_assertions)]
#[test]
fn a_closed_stderr_keeps_the_bug_reports_exit_code() {
    let t = site(&[]);
    let status = run_with_closed_pipes(t.path(), &["--__panic"]);
    assert_eq!(status.code(), Some(101), "{status:?}");
}
