//! `brrrty init`: setting up an 11ty site, end to end through the binary.
//!
//! Tests run without a terminal on standard input, which is the case where
//! `init` writes nothing unless given `--yes`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use brrrty_compat::{AllowList, Outcome, compare_dirs};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn site(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        let p = dir.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }
    dir
}

fn run_in(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_brrrty"))
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .env_remove("SOURCE_DATE_EPOCH")
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap()
}

const PACKAGE: &str = r#"{
  "name": "my-site",
  "version": "1.0.0",
  "scripts": {
    "build": "npx @11ty/eleventy",
    "start": "npx @11ty/eleventy --serve",
    "test": "node --test"
  },
  "devDependencies": {
    "@11ty/eleventy": "^3.1.2"
  }
}
"#;

const CJS: &str = r#"const syntaxHighlight = require("@11ty/eleventy-plugin-syntaxhighlight");

module.exports = function (eleventyConfig) {
  eleventyConfig.addPassthroughCopy("src/images");
  eleventyConfig.addPassthroughCopy({ "src/favicon.svg": "favicon.svg" });
  eleventyConfig.addPlugin(syntaxHighlight);
  eleventyConfig.addFilter("shout", (s) => s.toUpperCase() + "!");
  eleventyConfig.addShortcode("year", () => `${new Date().getFullYear()}`);

  return {
    dir: { input: "src", output: "dist" },
    markdownTemplateEngine: "njk",
  };
};
"#;

fn cjs_site() -> tempfile::TempDir {
    site(&[
        ("package.json", PACKAGE),
        ("eleventy.config.js", CJS),
        ("src/index.md", "# Hi\n"),
        ("src/images/a.txt", "a\n"),
        ("src/favicon.svg", "<svg/>\n"),
        ("src/_data/site.js", "module.exports = { title: 'x' };\n"),
        ("src/_data/ported.js", "module.exports = 1;\n"),
        ("src/_data/ported.rhai", "1\n"),
    ])
}

#[test]
fn sets_up_a_cjs_site() {
    let dir = cjs_site();
    let out = run_in(dir.path(), &["init", "--yes"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let pkg = read(dir.path(), "package.json");
    assert_eq!(
        pkg,
        format!(
            r#"{{
  "name": "my-site",
  "version": "1.0.0",
  "scripts": {{
    "build": "brrrty build",
    "build:11ty": "npx @11ty/eleventy",
    "start": "brrrty serve",
    "start:11ty": "npx @11ty/eleventy --serve",
    "test": "node --test"
  }},
  "devDependencies": {{
    "@11ty/eleventy": "^3.1.2",
    "brrrty": "{VERSION}"
  }}
}}
"#
        )
    );
    let rhai = read(dir.path(), "brrrty.config.rhai");
    for want in [
        "config.add_passthrough_copy(\"src/images\");\n",
        "config.add_passthrough_copy(#{ \"src/favicon.svg\": \"favicon.svg\" });\n",
        "config.add_plugin(\"syntaxhighlight\");\n",
        "// TODO(brrrty): port this filter to Rhai with config.add_filter.\n// See https://brrrty.com/docs/configuration/#filters\n// eleventy.config.js, line 7:\n//       eleventyConfig.addFilter(\"shout\", (s) => s.toUpperCase() + \"!\");\n",
        "//       eleventyConfig.addShortcode(\"year\", () => `${new Date().getFullYear()}`);\n",
        "#{\n    dir: #{\n        input: \"src\",\n        output: \"dist\",\n    },\n    markdownTemplateEngine: \"njk\",\n}\n",
    ] {
        assert!(rhai.contains(want), "missing {want:?} in:\n{rhai}");
    }
    let text = stdout(&out);
    assert!(
        text.contains("Wrote brrrty.config.rhai, package.json and src/_data/site.rhai."),
        "{text}"
    );
    assert!(
        text.contains("brrrty.config.rhai has 2 TODO(brrrty) markers"),
        "{text}"
    );
    // A literal data file is translated beside itself.
    assert!(
        text.contains("      src/_data/site.js -> site.rhai\n"),
        "{text}"
    );
    assert!(!text.contains("ported.js"), "{text}");
    assert_eq!(
        read(dir.path(), "src/_data/ported.rhai"),
        "1\n",
        "an existing port is left alone"
    );
    assert!(
        read(dir.path(), "src/_data/site.rhai").ends_with("#{ title: \"x\" }\n"),
        "{}",
        read(dir.path(), "src/_data/site.rhai")
    );
    // Every data file translated: JavaScript data stays off.
    assert!(!rhai.contains("set_js_data"), "{rhai}");
    assert!(
        text.contains("npm install\n  npm run build\n  npm run build:11ty -- --output=_site-11ty\n  diff -r _site-11ty dist\n"),
        "{text}"
    );

    // The 11ty config is recorded with its hash.
    let hash = brrrty_config::markers::source_hash(CJS.as_bytes());
    assert!(
        rhai.contains(&format!("\n// Ported from: eleventy.config.js {hash}\n")),
        "{rhai}"
    );

    // While the TODOs are left, Brrrty writes nothing, and says why.
    let refused = run_in(dir.path(), &["build"]);
    assert_eq!(refused.status.code(), Some(1), "{}", stderr(&refused));
    assert!(
        stderr(&refused).contains("config-todo"),
        "{}",
        stderr(&refused)
    );
    assert!(!dir.path().join("dist").exists());

    // Deleting the markers says they are dealt with: Brrrty builds with the new config.
    let ported: String = rhai
        .lines()
        .filter(|l| !l.starts_with("// TODO(brrrty)"))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(dir.path().join("brrrty.config.rhai"), ported).unwrap();
    let built = run_in(dir.path(), &["build"]);
    assert_eq!(built.status.code(), Some(0), "{}", stderr(&built));
    assert!(dir.path().join("dist/index.html").is_file());
    assert!(dir.path().join("dist/images/a.txt").is_file());
    assert!(dir.path().join("dist/favicon.svg").is_file());
}

#[test]
fn sets_up_an_esm_site() {
    let dir = site(&[
        (
            "package.json",
            "{\n\t\"type\": \"module\",\n\t\"dependencies\": {\n\t\t\"@11ty/eleventy\": \"3.1.2\"\n\t}\n}\n",
        ),
        (
            "eleventy.config.mjs",
            "import { feedPlugin } from \"@11ty/eleventy-plugin-rss\";\nimport { EleventyHtmlBasePlugin } from \"@11ty/eleventy\";\n\nexport default async function (eleventyConfig) {\n\televentyConfig.setInputDirectory(\"content\");\n\televentyConfig.addPassthroughCopy(\"content/**/*.jpg\");\n\televentyConfig.addPlugin(EleventyHtmlBasePlugin);\n\televentyConfig.addPlugin(feedPlugin, {\n\t\ttype: \"atom\",\n\t\toutputPath: \"/feed.xml\",\n\t\tcollection: { name: \"post\", limit: 10 },\n\t\tmetadata: { title: \"Blog\", base: \"https://example.com/\" },\n\t});\n\televentyConfig.addCollection(\"post\", (api) => api.getFilteredByTag(\"post\"));\n}\n\nexport const config = {\n\thtmlTemplateEngine: false,\n\ttemplateFormats: [\"md\", \"html\"],\n};\n",
        ),
        ("content/index.md", "# Hi\n"),
    ]);
    let out = run_in(dir.path(), &["init", "--yes"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let pkg = read(dir.path(), "package.json");
    assert_eq!(
        pkg,
        format!(
            "{{\n\t\"type\": \"module\",\n\t\"dependencies\": {{\n\t\t\"@11ty/eleventy\": \"3.1.2\"\n\t}},\n\t\"scripts\": {{\n\t\t\"build\": \"brrrty build\",\n\t\t\"start\": \"brrrty serve\"\n\t}},\n\t\"devDependencies\": {{\n\t\t\"brrrty\": \"{VERSION}\"\n\t}}\n}}\n"
        )
    );
    let rhai = read(dir.path(), "brrrty.config.rhai");
    for want in [
        "from eleventy.config.mjs",
        "Brrrty copies files and folders, not globs",
        "//     \televentyConfig.addPlugin(EleventyHtmlBasePlugin);\n",
        "config.add_plugin(\"rss\", #{\n    type: \"atom\",\n    output_path: \"/feed.xml\",\n    collection: #{ name: \"post\", limit: 10 },\n    metadata: #{ title: \"Blog\", base: \"https://example.com/\" },\n});\n",
        "Brrrty builds 11ty's default template formats",
        "//     \ttemplateFormats: [\"md\", \"html\"],\n",
        "#{\n    dir: #{\n        input: \"content\",\n    },\n    htmlTemplateEngine: false,\n}\n",
    ] {
        assert!(rhai.contains(want), "missing {want:?} in:\n{rhai}");
    }
    assert!(
        stdout(&out).contains("has 4 TODO(brrrty) markers"),
        "{}",
        stdout(&out)
    );
    // The config loads, though the feed's collection is a TODO still.
    brrrty_config::load(dir.path()).expect("the written config loads");
}

#[test]
fn leaves_scripts_that_dont_run_eleventy() {
    let dir = site(&[
        (
            "package.json",
            "{\n  \"scripts\": {\n    \"build\": \"npm-run-all build:*\",\n    \"build:site\": \"eleventy\"\n  },\n  \"devDependencies\": {\n    \"@11ty/eleventy\": \"3\"\n  }\n}\n",
        ),
        ("index.md", "# Hi\n"),
    ]);
    let out = run_in(dir.path(), &["init", "--yes"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let pkg = read(dir.path(), "package.json");
    assert!(pkg.contains("\"build\": \"npm-run-all build:*\",\n    \"build:site\": \"eleventy\",\n    \"start\": \"brrrty serve\"\n"), "{pkg}");
    let text = stdout(&out);
    assert!(
        text.contains("scripts.build runs \"npm-run-all build:*\", which isn't an 11ty command, so Brrrty left it alone."),
        "{text}"
    );
    assert!(text.contains("scripts.build:site runs 11ty"), "{text}");
    assert!(
        text.contains("npx @11ty/eleventy --output=_site-11ty"),
        "{text}"
    );
    // No 11ty config: a config with nothing to port, which loads.
    let rhai = read(dir.path(), "brrrty.config.rhai");
    assert!(rhai.contains("no 11ty config file"), "{rhai}");
    brrrty_config::load(dir.path()).expect("the written config loads");
}

#[test]
fn never_overwrites_a_brrrty_config() {
    let dir = cjs_site();
    fs::write(dir.path().join("brrrty.config.rhai"), "// mine\n").unwrap();
    for args in [&["init", "--yes"][..], &["init"], &["init", "--dry-run"]] {
        let out = run_in(dir.path(), args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&out).contains("brrrty.config.rhai already exists"),
            "{}",
            stderr(&out)
        );
    }
    assert_eq!(read(dir.path(), "brrrty.config.rhai"), "// mine\n");
    assert_eq!(read(dir.path(), "package.json"), PACKAGE);
}

#[test]
fn dry_run_prints_a_diff_and_writes_nothing() {
    let dir = cjs_site();
    let out = run_in(dir.path(), &["init", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("--- a/package.json\n+++ b/package.json\n@@ -2,11 +2,14 @@\n   \"name\": \"my-site\",\n   \"version\": \"1.0.0\",\n   \"scripts\": {\n-    \"build\": \"npx @11ty/eleventy\",\n-    \"start\": \"npx @11ty/eleventy --serve\",\n+    \"build\": \"brrrty build\",\n+    \"build:11ty\": \"npx @11ty/eleventy\",\n+    \"start\": \"brrrty serve\",\n+    \"start:11ty\": \"npx @11ty/eleventy --serve\",\n"),
        "{text}"
    );
    assert!(
        text.contains("--- /dev/null\n+++ b/brrrty.config.rhai\n"),
        "{text}"
    );
    assert!(
        text.contains("+config.add_plugin(\"syntaxhighlight\");\n"),
        "{text}"
    );
    assert!(text.contains("Dry run: nothing was written."), "{text}");
    assert_eq!(read(dir.path(), "package.json"), PACKAGE);
    assert!(!dir.path().join("brrrty.config.rhai").exists());
}

#[test]
fn without_a_terminal_it_only_shows_the_changes() {
    let dir = cjs_site();
    let out = run_in(dir.path(), &["init"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("+++ b/brrrty.config.rhai"), "{text}");
    assert!(
        text.contains("Nothing was written: there is no terminal to ask in. Run `brrrty init --yes` to make these changes."),
        "{text}"
    );
    assert_eq!(read(dir.path(), "package.json"), PACKAGE);
    assert!(!dir.path().join("brrrty.config.rhai").exists());
}

#[test]
fn dir_flag_and_not_an_eleventy_site() {
    let dir = cjs_site();
    let elsewhere = tempfile::tempdir().unwrap();
    let out = run_in(
        elsewhere.path(),
        &["init", "--yes", "--dir", dir.path().to_str().unwrap()],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(dir.path().join("brrrty.config.rhai").is_file());
    assert!(!elsewhere.path().join("brrrty.config.rhai").exists());

    let plain = site(&[("package.json", "{\"dependencies\": {\"vite\": \"5\"}}\n")]);
    let out = run_in(plain.path(), &["init", "--yes"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("doesn't look like an 11ty site"),
        "{}",
        stderr(&out)
    );
    assert!(!plain.path().join("brrrty.config.rhai").exists());
}

#[cfg(unix)]
#[test]
fn refuses_a_package_json_symlinked_outside() {
    let outside = site(&[("package.json", PACKAGE)]);
    let dir = site(&[("eleventy.config.js", CJS)]);
    std::os::unix::fs::symlink(
        outside.path().join("package.json"),
        dir.path().join("package.json"),
    )
    .unwrap();
    let out = run_in(dir.path(), &["init", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("outside"), "{}", stderr(&out));
    assert_eq!(read(outside.path(), "package.json"), PACKAGE);
    assert!(!dir.path().join("brrrty.config.rhai").exists());

    // A symlink inside the folder is followed, and stays a symlink.
    let inside = site(&[("eleventy.config.js", CJS), ("real/package.json", PACKAGE)]);
    std::os::unix::fs::symlink("real/package.json", inside.path().join("package.json")).unwrap();
    let out = run_in(inside.path(), &["init", "--yes"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        fs::symlink_metadata(inside.path().join("package.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(read(inside.path(), "real/package.json").contains("\"brrrty\""));
}

#[test]
fn bare_command_hints_at_init() {
    let hint = "This looks like an 11ty site. Run `npx brrrty init` to set it up.";
    let dir = site(&[("package.json", PACKAGE), ("index.md", "# Hi\n")]);
    let bare = run_in(dir.path(), &[]);
    assert!(
        stderr(&bare).starts_with(&format!("{hint}\n")),
        "{}",
        stderr(&bare)
    );
    // The exit status is the build's, as before.
    let build = run_in(dir.path(), &["build"]);
    assert_eq!(bare.status.code(), build.status.code());
    assert!(!stderr(&build).contains(hint));

    // Not once it's set up, and not outside an 11ty site.
    fs::write(dir.path().join("brrrty.config.rhai"), "").unwrap();
    assert!(!stderr(&run_in(dir.path(), &[])).contains(hint));
    let plain = site(&[("index.md", "# Hi\n")]);
    assert!(!stderr(&run_in(plain.path(), &[])).contains(hint));
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            fs::copy(e.path(), &dest).unwrap();
        }
    }
}

/// Every fixture whose 11ty config `init` ports in full builds, from the
/// config `init` writes alone, to 11ty's output byte for byte. Every other
/// one gets a config that loads.
#[test]
fn ported_fixtures_match_eleventy() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let mut names: Vec<PathBuf> = fs::read_dir(&fixtures)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("site/eleventy.config.js").is_file())
        .collect();
    names.sort();
    let mut compared = Vec::new();
    for fixture in names {
        let name = fixture.file_name().unwrap().to_string_lossy().into_owned();
        let work = tempfile::tempdir().unwrap();
        let root = work.path().join("site");
        copy_dir(&fixture.join("site"), &root);
        let _ = fs::remove_file(root.join("brrrty.config.rhai"));
        let out = run_in(&root, &["init", "--yes"]);
        assert_eq!(out.status.code(), Some(0), "{name}: {}", stderr(&out));
        // Pages that print `pkg` show package.json, which `init` changed.
        let package = fixture.join("site/package.json");
        if package.is_file() {
            fs::copy(package, root.join("package.json")).unwrap();
        }
        brrrty_config::load(&root)
            .unwrap_or_else(|p| panic!("{name}: the config doesn't load: {p:?}"));
        if read(&root, "brrrty.config.rhai").contains("// TODO") {
            continue;
        }

        let cfg: toml::Table = read(&fixture, "fixture.toml").parse().unwrap();
        let refused: Vec<PathBuf> = cfg
            .get("refused")
            .and_then(|v| v.as_table())
            .map(|t| {
                t.values()
                    .map(|v| PathBuf::from(v.as_str().unwrap()))
                    .collect()
            })
            .unwrap_or_default();
        let dest = work.path().join("out");
        fs::create_dir(&dest).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_brrrty"));
        cmd.current_dir(&root)
            .env_remove("SOURCE_DATE_EPOCH")
            .args(["build", "--output"])
            .arg(&dest);
        if let Some(epoch) = cfg.get("source_date_epoch") {
            cmd.env("SOURCE_DATE_EPOCH", epoch.as_integer().unwrap().to_string());
        }
        if let Some(tz) = cfg.get("tz") {
            cmd.env("TZ", tz.as_str().unwrap());
        }
        let result = cmd.output().unwrap();
        if refused.is_empty() {
            assert!(result.status.success(), "{name}: {}", stderr(&result));
        }
        let report = compare_dirs(&fixture.join("expected"), &dest, &AllowList::empty())
            .unwrap_or_else(|e| panic!("{name}: {e:?}\n{}", stderr(&result)));
        let failing: Vec<_> = report
            .results
            .iter()
            .filter(|r| match r.outcome {
                Outcome::Identical | Outcome::Equivalent => false,
                Outcome::Missing => !refused.contains(&r.path),
                _ => true,
            })
            .collect();
        assert!(
            failing.is_empty(),
            "{name} differs:\n{}",
            report.to_markdown(80)
        );
        compared.push(name);
    }
    for must in [
        "basic",
        "data",
        "highlight-engine-false",
        "init-follow",
        "layouts-dir",
        "nunjucks",
        "pagination",
        "rss-filters",
    ] {
        assert!(
            compared.iter().any(|n| n == must),
            "{must} wasn't compared: {compared:?}"
        );
    }
}

#[test]
fn compound_scripts_keep_their_other_steps() {
    let src = "{\n  \"scripts\": {\n    \"build\": \"npm run css && eleventy && npm run pagefind\",\n    \"start\": \"eleventy --serve\"\n  },\n  \"devDependencies\": { \"@11ty/eleventy\": \"3\" }\n}\n";
    let dir = site(&[("package.json", src)]);
    let out = run_in(dir.path(), &["init", "--yes"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let pkg = read(dir.path(), "package.json");
    // Only the 11ty step becomes Brrrty's; the others stay, in order.
    assert!(pkg.contains("\"build\": \"npm run css && brrrty build && npm run pagefind\",\n    \"build:11ty\": \"npm run css && eleventy && npm run pagefind\",\n    \"start\": \"brrrty serve\",\n    \"start:11ty\": \"eleventy --serve\""), "{pkg}");
    // `npm run build:11ty -- --output=…` would pass the flag to the last step, so the
    // comparison runs 11ty directly.
    let text = stdout(&out);
    assert!(
        text.contains("npx @11ty/eleventy --output=_site-11ty"),
        "{text}"
    );
}

#[test]
fn keeps_a_bom_in_package_json() {
    let src = "\u{feff}{\n  \"devDependencies\": {\n    \"@11ty/eleventy\": \"3\"\n  }\n}\n";
    let dir = site(&[("package.json", src)]);
    let out = run_in(dir.path(), &["init", "--yes"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let pkg = read(dir.path(), "package.json");
    assert!(
        pkg.starts_with(
            "\u{feff}{\n  \"devDependencies\": {\n    \"@11ty/eleventy\": \"3\",\n    \"brrrty\""
        ),
        "{pkg:?}"
    );
}

#[cfg(unix)]
#[test]
fn never_reads_special_files_or_outside_the_folder() {
    use std::time::{Duration, Instant};
    let run_bounded = |root: &Path, args: &[&str]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_brrrty"))
            .args(args)
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if started.elapsed() > Duration::from_secs(20) {
                let _ = child.kill();
                panic!("brrrty {args:?} hung");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        child.wait_with_output().unwrap()
    };

    // A FIFO and /dev/zero as package.json: no hang, no huge read.
    let fifo = tempfile::tempdir().unwrap();
    let made = Command::new("mkfifo")
        .arg(fifo.path().join("package.json"))
        .status()
        .unwrap();
    assert!(made.success());
    fs::write(fifo.path().join("index.md"), "# Hi\n").unwrap();
    // The bare command's check. (The build itself reads package.json for
    // `pkg`, which is older code and outside this test.)
    let hint_in = |dir: PathBuf| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(brrrty::init::should_hint(&dir)));
        rx.recv_timeout(Duration::from_secs(10))
            .expect("should_hint hung")
    };
    assert!(!hint_in(fifo.path().to_path_buf()));
    let out = run_bounded(fifo.path(), &["init", "--dry-run"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));

    let zero = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("/dev/zero", zero.path().join("package.json")).unwrap();
    let out = run_bounded(zero.path(), &["init", "--dry-run"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(!hint_in(zero.path().to_path_buf()));

    // An 11ty config that leads outside the folder is never read or quoted.
    let outside = site(&[("secret.js", "aws_secret_access_key = SECRET123\n")]);
    let dir = site(&[("package.json", PACKAGE)]);
    std::os::unix::fs::symlink(
        outside.path().join("secret.js"),
        dir.path().join("eleventy.config.js"),
    )
    .unwrap();
    let out = run_bounded(dir.path(), &["init", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!stdout(&out).contains("SECRET123") && !stderr(&out).contains("SECRET123"));
    assert!(!dir.path().join("brrrty.config.rhai").exists());
    assert_eq!(read(dir.path(), "package.json"), PACKAGE);

    // A huge package.json is refused, not read.
    let big = site(&[(
        "package.json",
        &format!(
            "{{\"devDependencies\":{{\"@11ty/eleventy\":\"3\"}},\"x\":\"{}\"}}",
            "a".repeat(2 << 20)
        ),
    )]);
    let out = run_bounded(big.path(), &["init", "--dry-run"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

#[test]
fn input_outside_the_folder_isnt_walked() {
    let dir = site(&[
        ("package.json", PACKAGE),
        (
            "eleventy.config.js",
            "module.exports = function () { return { dir: { input: \"/\" } }; };\n",
        ),
    ]);
    let started = std::time::Instant::now();
    let out = run_in(dir.path(), &["init", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(started.elapsed().as_secs() < 10);
}

#[test]
fn quoted_line_breaks_never_become_code() {
    for nl in ["\r", "\u{2028}", "\r\n"] {
        let config = format!(
            "module.exports = function (c) {{{nl}  c.addFilter(\"f\", (s) => s);{nl}  c.addPassthroughCopy(\"a\u{2028}b\");{nl}}};{nl}"
        );
        let dir = site(&[("package.json", PACKAGE), ("eleventy.config.js", &config)]);
        let out = run_in(dir.path(), &["init", "--yes"]);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        let site = brrrty_config::load(dir.path()).unwrap_or_else(|p| {
            panic!("{nl:?}: {p:?}\n{}", read(dir.path(), "brrrty.config.rhai"))
        });
        assert!(site.is_some());
        let rhai = read(dir.path(), "brrrty.config.rhai");
        // The config has a U+2028 (in a string), so it is one TODO whatever
        // its line breaks, and still loads.
        assert!(!rhai.contains("config.add_passthrough_copy"), "{rhai}");
    }
}

#[test]
fn script_values_are_printed_escaped() {
    let src = "{\n  \"scripts\": { \"build\": \"vite\\nbuild \\u001b[31mred\" },\n  \"devDependencies\": { \"@11ty/eleventy\": \"3\" }\n}\n";
    let dir = site(&[("package.json", src)]);
    let out = run_in(dir.path(), &["init", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(!text.contains('\u{1b}'), "{text:?}");
    assert!(
        text.contains(
            "scripts.build runs \"vite\\nbuild \\u{1b}[31mred\", which isn't an 11ty command"
        ),
        "{text}"
    );
}

/// A helper module the config hands its config object to is followed, its
/// hash is recorded, and a change to it afterwards warns.
#[test]
fn a_helper_module_is_followed_and_recorded() {
    let dir = site(&[
        (
            "package.json",
            "{\n  \"devDependencies\": { \"@11ty/eleventy\": \"3.1.8\" }\n}\n",
        ),
        (
            "eleventy.config.js",
            "const setup = require(\"./lib/setup\");\n\nmodule.exports = function (eleventyConfig) {\n  setup(eleventyConfig);\n  if (process.env.ELEVENTY_ENV === \"production\") {\n    eleventyConfig.addPassthroughCopy(\"prod\");\n  }\n  return { dir: { input: \"src\" } };\n};\n",
        ),
        (
            "lib/setup.js",
            "module.exports = (config) => {\n  [\"src/img\"].forEach((d) => config.addPassthroughCopy(d));\n};\n",
        ),
        ("src/index.md", "# Hi\n"),
        ("src/img/a.txt", "a\n"),
    ]);
    let out = run_in(dir.path(), &["init", "--yes"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let rhai = read(dir.path(), "brrrty.config.rhai");
    assert!(
        rhai.contains("// Also ported from: lib/setup.js sha256:"),
        "{rhai}"
    );
    assert!(
        rhai.contains("config.add_passthrough_copy(\"src/img\");"),
        "{rhai}"
    );
    assert!(
        rhai.contains("// TODO(brrrty): 11ty runs this only when `process.env.ELEVENTY_ENV === \"production\"`"),
        "{rhai}"
    );
    // Once the conditional is judged (here: the production build isn't compared), it builds.
    let judged: String = rhai
        .lines()
        .filter(|l| !l.contains("TODO(brrrty)"))
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(dir.path().join("brrrty.config.rhai"), judged).unwrap();
    let out = run_in(dir.path(), &["build"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!stderr(&out).contains("config-stale"), "{}", stderr(&out));
    assert_eq!(read(dir.path(), "_site/img/a.txt"), "a\n");
    // The helper changes: the port may be out of date.
    fs::write(
        dir.path().join("lib/setup.js"),
        "module.exports = (config) => {};\n",
    )
    .unwrap();
    let out = run_in(dir.path(), &["build"]);
    let err = stderr(&out);
    assert!(err.contains("config-stale"), "{err}");
    assert!(
        err.contains("lib/setup.js changed since brrrty.config.rhai recorded it as ported"),
        "{err}"
    );
}

/// The RSS plugin's version `init` saw is recorded, for builds to check again.
#[test]
fn the_rss_plugin_version_is_recorded() {
    let dir = site(&[
        (
            "package.json",
            "{\n  \"type\": \"module\",\n  \"devDependencies\": { \"@11ty/eleventy\": \"3.1.8\", \"@11ty/eleventy-plugin-rss\": \"^3.1.0\" }\n}\n",
        ),
        (
            "package-lock.json",
            "{\"packages\":{\"node_modules/@11ty/eleventy-plugin-rss\":{\"version\":\"3.1.0\"}}}\n",
        ),
        (
            "eleventy.config.js",
            "import pluginRss from \"@11ty/eleventy-plugin-rss\";\nexport default function (eleventyConfig) {\n  eleventyConfig.addPlugin(pluginRss);\n}\n",
        ),
    ]);
    let out = run_in(dir.path(), &["init", "--yes"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let rhai = read(dir.path(), "brrrty.config.rhai");
    assert!(rhai.contains("\n// RSS plugin: 3.1.0\n"), "{rhai}");
    assert!(
        rhai.contains("config.add_plugin(\"rss\", #{ feed: false });"),
        "{rhai}"
    );
}

#[test]
fn data_files_that_need_node_turn_javascript_data_on() {
    let dir = site(&[
        ("package.json", PACKAGE),
        ("index.md", "# Hi\n"),
        ("_data/site.js", "module.exports = { title: 'x' };\n"),
        (
            "_data/posts.js",
            "const { DateTime } = require(\"luxon\");\nmodule.exports = [DateTime.now().year];\n",
        ),
        (
            "_data/rows.js",
            "const fs = require(\"fs\");\nmodule.exports = JSON.parse(fs.readFileSync(\"rows.json\", \"utf8\"));\n",
        ),
        (
            "posts/posts.11tydata.js",
            "module.exports = { b: 1, a: 2 };\n",
        ),
    ]);
    let out = run_in(dir.path(), &["init", "--yes"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let rhai = read(dir.path(), "brrrty.config.rhai");
    assert!(rhai.contains("config.set_js_data(\"node\");\n"), "{rhai}");
    for f in ["_data/posts.js", "_data/rows.js", "posts/posts.11tydata.js"] {
        assert!(rhai.contains(&format!("//   {f}\n")), "{f} in:\n{rhai}");
    }
    let text = stdout(&out);
    assert!(text.contains("Translated to Rhai (1):"), "{text}");
    assert!(text.contains("Brrrty helpers could do it (2):"), "{text}");
    assert!(text.contains("posts/posts.11tydata.js (object)"), "{text}");
    assert!(
        text.contains("_data/rows.js (read_text, read_json, read_dir; parse_json)"),
        "{text}"
    );
    assert!(text.contains("Need Node (1):"), "{text}");
    assert!(dir.path().join("_data/site.rhai").is_file());
    assert!(!dir.path().join("_data/posts.rhai").exists());
    // The pages build with the data running in Node.
    let built = run_in(dir.path(), &["build"]);
    assert!(
        stderr(&built).contains("site.rhai supplies this key"),
        "{}",
        stderr(&built)
    );
}
