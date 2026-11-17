//! `.rhai` data files: `_data/<name>.rhai`, `<name>.11tydata.rhai` and
//! `<dir>/<dir>.11tydata.rhai`.
//!
//! What 11ty 3.1.8 does with data files was probed (see
//! `brrrty-core/tests/data.rs` and the `data` fixture); these tests pin
//! what Brrrty adds: a script's last expression, or its `data()`, is the
//! value, and `.rhai` takes the slot `.js` holds.

use std::fs;
use std::path::{Path, PathBuf};

use brrrty_config::{CONFIG_FILE, Clock, data_runner, load_with};
use brrrty_core::{BuildOptions, BuildReport, Problem, Severity, build, codes};

const NOON_MS: i64 = 1_699_963_200_000;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

/// Build `files` (paths under the root) with the data runner installed and
/// no config file, as `brrrty` does for a site without one.
fn site(files: &[(&str, &str)]) -> (tempfile::TempDir, BuildReport) {
    site_with(files, Clock::Fixed(NOON_MS), None)
}

fn site_with(
    files: &[(&str, &str)],
    clock: Clock,
    config: Option<&str>,
) -> (tempfile::TempDir, BuildReport) {
    let t = tempfile::tempdir().unwrap();
    for (name, body) in files {
        write(t.path(), name, body);
    }
    let mut cfg = match config {
        Some(script) => {
            write(t.path(), CONFIG_FILE, script);
            load_with(t.path(), clock).unwrap().unwrap().build_options()
        }
        None => Default::default(),
    };
    cfg.data = Some(data_runner(
        clock,
        t.path().to_path_buf(),
        cfg.reads.clone(),
    ));
    let opts = BuildOptions {
        root: t.path().to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        config: cfg,
        ..Default::default()
    };
    let report = build(&opts, &brrrty_markdown::MarkdownIt::new());
    (t, report)
}

/// A page's output without the paragraph markdown wraps it in.
fn out(t: &tempfile::TempDir, rel: &str) -> Option<String> {
    fs::read_to_string(t.path().join("_site").join(rel))
        .ok()
        .map(|s| s.replace("<p>", "").replace("</p>", ""))
}

fn errors(r: &BuildReport) -> Vec<&Problem> {
    r.problems
        .iter()
        .filter(|p| p.severity == Severity::Error)
        .collect()
}

fn page(body: &str) -> (&'static str, String) {
    (
        "src/index.md",
        format!("---\npermalink: out.html\n---\n{body}"),
    )
}

fn run(files: &[(&str, &str)], body: &str) -> (tempfile::TempDir, BuildReport) {
    let (name, text) = page(body);
    let mut all = files.to_vec();
    all.push((name, &text));
    site(&all)
}

#[test]
fn the_last_expression_is_the_value() {
    let (t, r) = run(
        &[(
            "src/_data/site.rhai",
            "#{ name: \"Mine\", url: \"https://x.test\" }",
        )],
        "{{ site.name }} {{ site.url }}",
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "out.html").unwrap().trim(), "Mine https://x.test");
}

#[test]
fn data_function_is_called_like_an_exported_function() {
    let (t, r) = run(
        &[(
            "src/_data/site.rhai",
            "fn data() { #{ name: \"Called\" } }\n\"ignored\"",
        )],
        "{{ site.name }}",
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "out.html").unwrap().trim(), "Called");
}

#[test]
fn data_function_gets_the_config_global_data() {
    // 11ty calls an exported function with the config's global data.
    let (t, r) = site_with(
        &[
            (
                "src/_data/site.rhai",
                "fn data(g) { #{ name: g.brand + \"!\" } }",
            ),
            page("{{ site.name }}").into_pair(),
        ],
        Clock::Fixed(NOON_MS),
        Some("config.add_global_data(\"brand\", \"Acme\");\n#{ dir: #{ input: \"src\" } }"),
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "out.html").unwrap().trim(), "Acme!");
}

trait IntoPair {
    fn into_pair(self) -> (&'static str, &'static str);
}
impl IntoPair for (&'static str, String) {
    fn into_pair(self) -> (&'static str, &'static str) {
        (self.0, Box::leak(self.1.into_boxed_str()))
    }
}

#[test]
fn arrays_and_scalars() {
    let (t, r) = run(
        &[
            ("src/_data/list.rhai", "[\"a\", \"b\", \"c\"]"),
            ("src/_data/count.rhai", "40 + 2"),
            ("src/_data/flag.rhai", "true"),
            ("src/_data/word.rhai", "\"hi\""),
        ],
        "{{ list | join: '-' }} {{ count }} {{ flag }} {{ word }}",
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "out.html").unwrap().trim(), "a-b-c 42 true hi");
}

#[test]
fn folders_nest_like_json() {
    let (t, r) = run(
        &[
            ("src/_data/a/b.rhai", "#{ v: 1 }"),
            ("src/_data/a/c/d.rhai", "#{ v: 2 }"),
            ("src/_data/e.json", "{\"j\": 3}"),
        ],
        "{{ a.b.v }} {{ a.c.d.v }} {{ e.j }}",
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "out.html").unwrap().trim(), "1 2 3");
}

#[test]
fn object_keeps_key_order() {
    let (t, r) = run(
        &[(
            "src/_data/o.rhai",
            "let o = object(); o.zed = 1; o.alpha = 2; o.mid = 3; o",
        )],
        "{% for k in o %}{{ k }},{% endfor %}",
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "out.html").unwrap().trim(), "zed1,alpha2,mid3,");
}

#[test]
fn rhai_wins_over_json_at_the_same_key() {
    let (t, r) = run(
        &[
            ("src/_data/k.json", "{\"v\": \"json\"}"),
            ("src/_data/k.rhai", "#{ v: \"rhai\" }"),
        ],
        "{{ k.v }}",
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "out.html").unwrap().trim(), "rhai");
}

#[test]
fn a_rhai_port_wins_over_js_and_the_js_is_not_refused() {
    let (t, r) = run(
        &[
            (
                "src/_data/site.js",
                "module.exports = () => ({ name: 'js' });",
            ),
            ("src/_data/site.rhai", "#{ name: \"rhai\" }"),
        ],
        "{{ site.name }}",
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "out.html").unwrap().trim(), "rhai");
    let infos: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.severity == Severity::Info)
        .collect();
    assert_eq!(infos.len(), 1, "{:#?}", r.problems);
    assert_eq!(infos[0].code, codes::DATA_RHAI_SHADOWS_JS);
    assert_eq!(
        infos[0].path.as_deref(),
        Some(Path::new("src/_data/site.js"))
    );
}

#[test]
fn js_refusal_names_the_port() {
    let (_t, r) = run(
        &[(
            "src/_data/site.js",
            "module.exports = () => ({ name: 'js' });",
        )],
        "{{ site.name }}",
    );
    let e = errors(&r);
    assert_eq!(e.len(), 1, "{:#?}", r.problems);
    assert_eq!(e[0].code, codes::DATA_JS_UNSUPPORTED);
    assert!(
        e[0].help
            .as_deref()
            .unwrap()
            .contains("port src/_data/site.js to src/_data/site.rhai"),
        "{:?}",
        e[0].help
    );
}

#[test]
fn template_and_directory_data() {
    let (t, r) = site(&[
        (
            "src/posts/posts.11tydata.rhai",
            "#{ layout_kind: \"dir\", who: \"dir\" }",
        ),
        ("src/posts/one.11tydata.rhai", "#{ who: \"tpl\" }"),
        (
            "src/posts/one.md",
            "---\npermalink: one.html\n---\n{{ layout_kind }} {{ who }}",
        ),
        (
            "src/posts/two.md",
            "---\npermalink: two.html\n---\n{{ layout_kind }} {{ who }}",
        ),
    ]);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "one.html").unwrap().trim(), "dir tpl");
    assert_eq!(out(&t, "two.html").unwrap().trim(), "dir dir");
}

#[test]
fn rhai_holds_the_slot_of_11tydata_js_above_json() {
    // 11ty's order, lowest first: x.json, x.11tydata.json, x.11tydata.js.
    let (t, r) = site(&[
        ("src/x.json", "{\"v\": \"json\", \"a\": 1}"),
        (
            "src/x.11tydata.json",
            "{\"v\": \"11tydata.json\", \"b\": 2}",
        ),
        ("src/x.11tydata.rhai", "#{ v: \"rhai\", c: 3 }"),
        (
            "src/x.md",
            "---\npermalink: x.html\n---\n{{ v }} {{ a }}{{ b }}{{ c }}",
        ),
    ]);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "x.html").unwrap().trim(), "rhai 123");
}

#[test]
fn a_local_rhai_port_replaces_its_js_file_with_one_info_line() {
    let (t, r) = site(&[
        ("src/x.11tydata.js", "module.exports = { v: 'js' };"),
        ("src/x.11tydata.rhai", "#{ v: \"rhai\" }"),
        ("src/x.md", "---\npermalink: x.html\n---\n{{ v }}"),
        ("src/y.md", "---\npermalink: y.html\n---\nplain"),
    ]);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "x.html").unwrap().trim(), "rhai");
    let infos = r
        .problems
        .iter()
        .filter(|p| p.severity == Severity::Info)
        .count();
    assert_eq!(infos, 1, "{:#?}", r.problems);
}

#[test]
fn local_data_can_set_build_keys_because_its_value_is_known() {
    let (t, r) = site(&[
        ("src/_includes/base.html", "<{{ content }}>"),
        (
            "src/x.11tydata.rhai",
            "#{ layout: \"base.html\", tags: [\"t\"] }",
        ),
        ("src/x.md", "---\npermalink: x.html\n---\nbody"),
    ]);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "x.html").unwrap().trim(), "<body\n>");
}

#[test]
fn a_global_error_refuses_only_the_pages_that_read_the_key() {
    let (t, r) = site(&[
        ("src/_data/good.rhai", "#{ v: 1 }"),
        ("src/_data/bad.rhai", "let x = 1;\nlet y = x +;\n"),
        (
            "src/reads.md",
            "---\npermalink: reads.html\n---\n{{ bad.v }}",
        ),
        (
            "src/fine.md",
            "---\npermalink: fine.html\n---\n{{ good.v }}",
        ),
    ]);
    assert_eq!(out(&t, "fine.html").unwrap().trim(), "1");
    assert!(out(&t, "reads.html").is_none());
    let bad: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::DATA_RHAI_SYNTAX)
        .collect();
    assert_eq!(bad.len(), 1, "{:#?}", r.problems);
    assert_eq!(
        bad[0].path.as_deref(),
        Some(Path::new("src/_data/bad.rhai"))
    );
    assert_eq!(bad[0].line, Some(2));
    assert!(bad[0].column.is_some());
    assert!(
        r.problems
            .iter()
            .any(|p| p.path.as_deref() == Some(Path::new("src/reads.md"))),
        "the page is refused: {:#?}",
        r.problems
    );
}

#[test]
fn a_runtime_error_has_line_and_column() {
    let (_t, r) = run(
        &[("src/_data/bad.rhai", "let a = 1;\n\nlet b = a.nope();\nb")],
        "{{ bad }}",
    );
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::DATA_RHAI_ERROR)
        .unwrap_or_else(|| panic!("{:#?}", r.problems));
    assert_eq!(p.path.as_deref(), Some(Path::new("src/_data/bad.rhai")));
    assert_eq!(p.line, Some(3));
    assert!(p.column.is_some());
}

#[test]
fn limits_apply() {
    let (t, r) = run(
        &[
            ("src/_data/forever.rhai", "loop { }"),
            ("src/_data/deep.rhai", "fn f(n) { f(n + 1) }\nf(0)"),
        ],
        "{{ forever }}{{ deep }}",
    );
    assert!(out(&t, "out.html").is_none());
    let limits = r
        .problems
        .iter()
        .filter(|p| p.code == codes::DATA_RHAI_LIMIT)
        .count();
    assert_eq!(limits, 2, "{:#?}", r.problems);
}

#[test]
fn a_local_error_refuses_the_pages_it_applies_to() {
    let (t, r) = site(&[
        ("src/bad/bad.11tydata.rhai", "throw \"no\";"),
        ("src/bad/p.md", "---\npermalink: p.html\n---\nx"),
        ("src/ok.md", "---\npermalink: ok.html\n---\nfine"),
    ]);
    assert!(out(&t, "p.html").is_none());
    assert_eq!(out(&t, "ok.html").unwrap().trim(), "fine");
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::DATA_RHAI_ERROR)
        .unwrap_or_else(|| panic!("{:#?}", r.problems));
    assert_eq!(
        p.path.as_deref(),
        Some(Path::new("src/bad/bad.11tydata.rhai"))
    );
    assert_eq!(p.line, Some(1));
}

#[test]
fn a_value_that_cannot_be_data_is_an_error() {
    let (_t, r) = run(&[("src/_data/f.rhai", "|x| x")], "{{ f }}");
    assert!(
        r.problems
            .iter()
            .any(|p| p.path.as_deref() == Some(Path::new("src/_data/f.rhai"))),
        "{:#?}",
        r.problems
    );
}

#[test]
fn now_follows_source_date_epoch() {
    // 2023-11-14T12:00:00Z; `year` is local time, so use UTC-safe noon.
    let (t, r) = site_with(
        &[
            ("src/_data/y.rhai", "year(now())"),
            ("src/_data/ms.rhai", "get_time(now())"),
            page("{{ y }} {{ ms }}").into_pair(),
        ],
        Clock::Fixed(NOON_MS),
        None,
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "out.html").unwrap().trim(), "2023 1699963200000");
}

#[test]
fn no_state_leaks_between_data_files() {
    // A function one file defines isn't there in the next, and neither is
    // a variable, so the order files run in can't matter.
    let (t, r) = run(
        &[
            (
                "src/_data/a.rhai",
                "fn helper() { 1 }\nlet leaked = 5;\nhelper()",
            ),
            ("src/_data/b.rhai", "helper()"),
            ("src/_data/c.rhai", "leaked"),
            ("src/_data/d.rhai", "config.add_global_data(\"x\", 1)"),
        ],
        "{{ a }}",
    );
    assert_eq!(out(&t, "out.html").unwrap().trim(), "1");
    for f in ["b.rhai", "c.rhai", "d.rhai"] {
        assert!(
            r.problems
                .iter()
                .any(|p| p.path.as_deref().is_some_and(|x| x.ends_with(f))),
            "{f}: {:#?}",
            r.problems
        );
    }
}

#[test]
fn each_file_runs_once_per_build() {
    // `print` goes to stdout, which a test can't see; count through the
    // dir data instead: one file shared by two pages is one read.
    let (t, r) = site(&[
        ("src/d/d.11tydata.rhai", "#{ stamp: get_time(now()) }"),
        ("src/d/a.md", "---\npermalink: a.html\n---\n{{ stamp }}"),
        ("src/d/b.md", "---\npermalink: b.html\n---\n{{ stamp }}"),
    ]);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "a.html"), out(&t, "b.html"));
}

#[test]
fn without_a_runner_rhai_data_is_refused_with_a_problem() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "src/_data/k.rhai", "1");
    write(t.path(), "src/i.md", "---\npermalink: i.html\n---\n{{ k }}");
    let opts = BuildOptions {
        root: t.path().to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        ..Default::default()
    };
    let r = build(&opts, &brrrty_markdown::MarkdownIt::new());
    assert!(r.problems.iter().any(|p| p.code == codes::DATA_RHAI_ERROR));
}

/// Probed with 11ty 3.1.8: `<key>.json` and `<key>.js` (here `.rhai`, which
/// takes the same place) merge with `Merge`, the later file over the earlier.
/// The expected text is what 11ty printed with `{{ key | json }}`.
#[test]
fn two_files_for_one_key_merge_like_11ty() {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "a",
            r#"{"v":1,"only_json":1,"n":{"x":1}}"#,
            "#{ v: 2, n: #{ y: 2 } }",
            r#"{"v":2,"only_json":1,"n":{"x":1,"y":2}}"#,
        ),
        ("b", "[1]", "[2]", "[1,2]"),
        ("c", r#"{"k":1}"#, "5", r#"{"k":1}"#),
        ("d", "5", "#{ k: 1 }", r#"{"k":1}"#),
        ("e", "5", "6", "6"),
        (
            "g",
            r#"{"a":[1],"o":{"p":1}}"#,
            r#"#{ a: [2], "override:o": #{ q: 1 } }"#,
            r#"{"a":[1,2],"o":{"q":1}}"#,
        ),
        ("h", r#"{"k":1}"#, "()", r#"{"k":1}"#),
        (
            "i",
            r#"{"z":1,"a":2}"#,
            r#"#{ a: 3, m: 4, "2": 5 }"#,
            r#"{"2":5,"z":1,"a":3,"m":4}"#,
        ),
        ("j", r#"{"k":1}"#, "null_value()", r#"{"k":1}"#),
        ("k", "null", "#{ k: 1 }", r#"{"k":1}"#),
    ];
    let mut files: Vec<(String, String)> = Vec::new();
    let mut body = String::new();
    for (k, json, rhai, want) in cases {
        files.push((format!("src/_data/{k}.json"), json.to_string()));
        files.push((format!("src/_data/{k}.rhai"), rhai.to_string()));
        body.push_str(&format!("{k}={{{{ {k} | json }}}}\n"));
        let _ = want;
    }
    let (name, text) = page(&body);
    files.push((name.to_string(), text));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let (t, r) = site(&refs);
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    // The page is markdown, which escapes quotes in text.
    let got = out(&t, "out.html").unwrap().replace("&quot;", "\"");
    let want: String = cases
        .iter()
        .map(|(k, _, _, w)| format!("{k}={w}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(got.trim(), want);
}

#[test]
fn top_level_code_does_not_run_when_data_is_defined() {
    let (t, r) = run(
        &[("src/_data/k.rhai", "let boom = 1 / 0;\nfn data() { 7 }")],
        "{{ k }}",
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "out.html").unwrap().trim(), "7");
}

#[test]
fn using_config_in_a_data_script_gets_data_wording() {
    let (_t, r) = run(
        &[("src/_data/k.rhai", "config.add_filter(\"x\", |v| v);")],
        "{{ k }}",
    );
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::DATA_RHAI_SYNTAX)
        .unwrap_or_else(|| panic!("{:#?}", r.problems));
    let help = p.help.as_deref().unwrap();
    assert!(help.contains("no `config`"), "{help}");
    assert!(!help.contains("curry"), "{help}");
}

#[test]
fn shadow_note_for_template_data_names_the_file() {
    let (_t, r) = site(&[
        ("src/x.11tydata.js", "module.exports = { v: 'js' };"),
        ("src/x.11tydata.rhai", "#{ v: \"rhai\" }"),
        ("src/x.md", "---\npermalink: x.html\n---\n{{ v }}"),
    ]);
    let note = r
        .problems
        .iter()
        .find(|p| p.severity == Severity::Info)
        .unwrap();
    assert!(!note.message.contains("this key"), "{}", note.message);
    assert!(note.message.contains("x.11tydata.rhai"), "{}", note.message);
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=Brrrty",
            "-c",
            "user.email=brrrty@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// `git_head()` is `git rev-parse HEAD` for the project, in the config and
/// in data files; without a repository it is an error.
#[test]
fn git_head_is_the_commit_head_points_at() {
    let files = [
        ("src/_data/deploy.rhai", "#{ sha: git_head() }"),
        (
            "src/index.md",
            "---\npermalink: out.html\n---\n{{ deploy.sha }} {{ sha }}",
        ),
    ];
    let config = "config.add_global_data(\"sha\", git_head());\n#{}";
    let (t, r) = site_with(&files, Clock::Fixed(NOON_MS), None);
    let e = errors(&r);
    assert!(
        e.iter()
            .any(|p| p.message.contains("not in a git repository")),
        "{e:#?}"
    );
    write(t.path(), CONFIG_FILE, config);
    let e = load_with(t.path(), Clock::Fixed(NOON_MS)).unwrap_err();
    assert!(e[0].message.contains("not in a git repository"), "{e:#?}");

    let t2 = tempfile::tempdir().unwrap();
    for (name, body) in files.iter().chain([&(CONFIG_FILE, config)]) {
        write(t2.path(), name, body);
    }
    git(t2.path(), &["init", "-q"]);
    git(t2.path(), &["add", "-A"]);
    git(t2.path(), &["commit", "-q", "-m", "one"]);
    let sha = git(t2.path(), &["rev-parse", "HEAD"]);
    let mut cfg = load_with(t2.path(), Clock::Fixed(NOON_MS))
        .unwrap()
        .unwrap()
        .build_options();
    cfg.data = Some(data_runner(
        Clock::Fixed(NOON_MS),
        t2.path().to_path_buf(),
        cfg.reads.clone(),
    ));
    let opts = BuildOptions {
        root: t2.path().to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        config: cfg,
        ..Default::default()
    };
    let r = build(&opts, &brrrty_markdown::MarkdownIt::new());
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    let got = fs::read_to_string(t2.path().join("_site/out.html")).unwrap();
    assert_eq!(got.trim(), format!("<p>{sha} {sha}</p>"));
    drop(t);
}

#[test]
fn a_data_file_that_is_not_utf8_is_refused_with_help() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "src/index.md",
        "---\npermalink: out.html\n---\n{{ bad }}",
    );
    fs::create_dir_all(t.path().join("src/_data")).unwrap();
    fs::write(t.path().join("src/_data/bad.rhai"), b"#{ a: \"\xff\" }").unwrap();
    let mut cfg = brrrty_core::ConfigOptions::default();
    cfg.data = Some(data_runner(
        Clock::Fixed(NOON_MS),
        t.path().to_path_buf(),
        cfg.reads.clone(),
    ));
    let opts = BuildOptions {
        root: t.path().to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        config: cfg,
        ..Default::default()
    };
    let r = build(&opts, &brrrty_markdown::MarkdownIt::new());
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::DATA_FILE_INVALID)
        .unwrap_or_else(|| panic!("{:#?}", r.problems));
    assert_eq!(p.path.as_deref(), Some(Path::new("src/_data/bad.rhai")));
    assert!(p.message.contains("UTF-8"), "{}", p.message);
    assert_eq!(p.help.as_deref(), Some("save the file as UTF-8"));
    assert!(out(&t, "out.html").is_none());
}

/// 11ty calls an exported data function with one argument, so a `fn data` that wants more is
/// a mistake worth naming, not a silent `()` for the extra parameters.
#[test]
fn a_data_function_with_two_parameters_is_an_error() {
    let (t, r) = run(
        &[("src/_data/two.rhai", "fn data(a, b) { #{ x: 1 } }")],
        "{{ two.x }}",
    );
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::DATA_RHAI_ERROR)
        .unwrap_or_else(|| panic!("{:#?}", r.problems));
    assert_eq!(p.path.as_deref(), Some(Path::new("src/_data/two.rhai")));
    assert!(p.message.contains("takes 2 parameters"), "{}", p.message);
    assert!(out(&t, "out.html").is_none());
}

/// Data scripts print constant numbers in `${}` as JavaScript does, as config scripts do:
/// Rhai's optimizer would fold these to `10.0` and `1e21`. Node: `${10}`, `${1e21}`.
#[test]
fn constants_in_interpolation_print_as_javascript_does() {
    let (t, r) = run(
        &[(
            "src/_data/n.rhai",
            "const PER_PAGE = 10.0;\n#{ a: `${10.0}`, b: `${1e21}`, c: `${PER_PAGE} per page` }",
        )],
        "{{ n.a }}|{{ n.b }}|{{ n.c }}",
    );
    assert!(errors(&r).is_empty(), "{:#?}", r.problems);
    assert_eq!(out(&t, "out.html").unwrap().trim(), "10|1e+21|10 per page");
}
