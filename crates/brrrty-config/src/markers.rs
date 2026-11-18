//! The markers `brrrty init` leaves in `brrrty.config.rhai`, and what a build
//! makes of them.
//!
//! `init` ports only what it can read exactly. Every statement of the 11ty
//! config it can't port is quoted under a `// TODO(brrrty): ...` line, and a
//! config it can't port at all becomes one `// TODO(brrrty-file): ...` line.
//! Each of those may change any page, so while one is left a build writes no
//! pages: deleting the line is how a person says "ported, or changes nothing".
//!
//! `init` also records the 11ty config it read, with a hash, on a
//! `// Ported from: eleventy.config.js sha256:...` line. When that file
//! changes afterwards, a build warns.

use std::path::Path;

use brrrty_core::{Problem, codes};
use sha2::{Digest, Sha256};

use crate::CONFIG_FILE;

/// The marker on each statement `brrrty init` couldn't port.
pub const TODO: &str = "TODO(brrrty)";
/// The marker for an 11ty config `brrrty init` couldn't port at all.
pub const TODO_FILE: &str = "TODO(brrrty-file)";
/// How the line naming the ported 11ty config starts.
pub const PORTED_FROM: &str = "// Ported from: ";
/// How a line naming one of the site's own modules `init` read with the
/// config (a helper or a plugin it follows) starts.
pub const ALSO_PORTED_FROM: &str = "// Also ported from: ";

/// The 11ty config file names a `Ported from` line may name.
const ELEVENTY_CONFIGS: [&str; 4] = [
    ".eleventy.js",
    "eleventy.config.js",
    "eleventy.config.mjs",
    "eleventy.config.cjs",
];

/// The most read of an 11ty config to check its hash.
const MAX_READ: u64 = 16 << 20;

/// Marker lines a message lists before saying how many more there are.
const SHOWN_LINES: usize = 12;

/// Where docs say what to do with the markers.
const HELP_LINK: &str = "https://brrrty.com/docs/getting-started/#1-port-the-config";

/// One unresolved marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Todo {
    /// 1-based.
    pub line: usize,
    /// The whole 11ty config is unported.
    pub whole: bool,
    /// What the marker says after `TODO(brrrty):`.
    pub text: String,
}

/// A marker anywhere in a line, leniently: any case, spaces inside the parentheses, after
/// `//`, `///`, `//!`, `/*` or a block comment's leading `*`. A false match only refuses
/// pages with a message naming the line, so this errs towards matching.
fn marker() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)(?:(?://[/!]?|/\*+)|^\s*\*)\s*todo\s*\(\s*brrrty\s*(-\s*file\s*)?\)\s*:?\s*(.*)$",
        )
        .expect("valid")
    })
}

/// The text of a `//` comment line, if `line` is one.
fn comment(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("//")?;
    Some(rest.trim_start_matches('/').trim_start())
}

/// The unresolved markers in a config's text, in order.
///
/// Besides `TODO(brrrty)` and `TODO(brrrty-file)`, this reads the markers of
/// a config written by `brrrty init` 0.0.18 or earlier: a `// TODO: ...` line
/// followed by the `// See https://...` line init put under it, in a file
/// that starts with init's header.
///
/// Lines are split as `str::lines` splits them (LF and CRLF), as Rhai counts them; a lone CR
/// (classic Mac line endings) doesn't end a line here.
pub fn todos(src: &str) -> Vec<Todo> {
    let lines: Vec<&str> = src.lines().collect();
    let by_init = lines
        .iter()
        .take(5)
        .any(|l| l.starts_with("// Written by `brrrty init`"));
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if let Some(c) = marker().captures(line) {
            let text = c.get(2).map_or("", |m| m.as_str());
            let text = text.trim_end().trim_end_matches("*/").trim_end();
            out.push(Todo {
                line: i + 1,
                whole: c.get(1).is_some(),
                text: text.trim_end_matches('.').to_string(),
            });
            continue;
        }
        let Some(text) = comment(line) else { continue };
        let (whole, rest) = if by_init
            && let Some(r) = text.strip_prefix("TODO: ")
            && lines.get(i + 1).is_some_and(|n| {
                n.starts_with("// See https://brrrty.com/")
                    || n.starts_with("// See https://github.com/brrrty/")
            }) {
            (r.starts_with("port all of this file by hand"), r)
        } else {
            continue;
        };
        let rest = rest.trim_start_matches(':').trim();
        out.push(Todo {
            line: i + 1,
            whole,
            text: rest.trim_end_matches('.').to_string(),
        });
    }
    out
}

/// The `Ported from` line: its 1-based line number, the 11ty config it
/// names and the hash recorded. The name is one of 11ty's config names, or a
/// path inside the project (from a build script's `--config`).
pub fn ported_from(src: &str) -> Option<(usize, &str, &str)> {
    src.lines().enumerate().find_map(|(i, l)| {
        let rest = l.trim_end().strip_prefix(PORTED_FROM)?;
        let (name, hash) = rest.trim().rsplit_once(' ')?;
        let name = name.trim();
        (inside(name) && hash.starts_with("sha256:")).then_some((i + 1, name, hash))
    })
}

/// A relative path that stays inside the project folder.
pub fn is_inside(name: &str) -> bool {
    inside(name)
}

fn inside(name: &str) -> bool {
    let p = Path::new(name);
    !name.is_empty()
        && p.is_relative()
        && !name.starts_with(['/', '\\'])
        && p.components().all(|c| {
            matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
}

/// The hash a `Ported from` line records: `sha256:` and the hex digest of the
/// text with a leading BOM dropped and CRLF line endings made LF, so a
/// checkout with other line endings has the same hash.
pub fn source_hash(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let mut h = Sha256::new();
    let mut start = 0;
    for i in 0..bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            h.update(&bytes[start..i]);
            start = i + 1;
        }
    }
    h.update(&bytes[start..]);
    let mut out = String::from("sha256:");
    for b in h.finalize() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// The 11ty config 11ty reads by default: the first of its names that exists in `root`.
pub fn default_config(root: &Path) -> Option<&'static str> {
    ELEVENTY_CONFIGS
        .iter()
        .copied()
        .find(|n| std::fs::metadata(root.join(n)).is_ok_and(|m| m.is_file()))
}

/// The hash of `root/name`, read up to [`MAX_READ`] bytes; `None` if it isn't a file.
fn hash_of(root: &Path, name: &str) -> Option<String> {
    use std::io::Read as _;
    let path = root.join(name);
    if !std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .ok()?
        .take(MAX_READ)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(source_hash(&bytes))
}

/// The 11ty config the file was ported from, as its header names it.
fn source_name(src: &str) -> Option<&str> {
    if let Some((_, name, _)) = ported_from(src) {
        return Some(name);
    }
    let first = src.lines().next()?;
    let rest = first.strip_prefix("// Written by `brrrty init` from ")?;
    let name = rest.split([',', ' ']).next()?.trim_end_matches('.');
    inside(name).then_some(name)
}

/// `5`, `5 and 9`, `5, 9 and 12`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The error that refuses every page while markers are left, or `None`.
pub fn unported_problem(src: &str) -> Option<Problem> {
    let todos = todos(src);
    let first = todos.first()?;
    let source = source_name(src).unwrap_or("the 11ty config");
    let message = if let Some(w) = todos.iter().find(|t| t.whole) {
        let why = w
            .text
            .strip_prefix("port all of this file by hand")
            .map_or(w.text.as_str(), |r| r.trim_start_matches([':', ' ']));
        format!(
            "`brrrty init` couldn't port any of {source} ({why}), so Brrrty would build with 11ty's default settings and any page could differ from 11ty's"
        )
    } else {
        let mut lines: Vec<String> = todos
            .iter()
            .take(SHOWN_LINES)
            .map(|t| t.line.to_string())
            .collect();
        if todos.len() > SHOWN_LINES {
            lines.push(format!("{} more", todos.len() - SHOWN_LINES));
        }
        format!(
            "{CONFIG_FILE} has {} `{TODO}` {} for parts of {source} that `brrrty init` couldn't port (line{} {}); any of them could change any page",
            todos.len(),
            if todos.len() == 1 {
                "marker"
            } else {
                "markers"
            },
            if todos.len() == 1 { "" } else { "s" },
            and_list(&lines)
        )
    };
    let line = todos
        .iter()
        .find(|t| t.whole)
        .map_or(first.line, |t| t.line);
    Some(
        Problem::error(codes::CONFIG_TODO, message)
            .with_path(CONFIG_FILE)
            .at(line, 1)
            .with_help(format!(
                "Brrrty writes no pages and copies no files while a TODO(brrrty) line is left. Port each one to Rhai, or decide it changes nothing Brrrty writes, then delete its TODO line. {HELP_LINK}"
            )),
    )
}

/// The warning when the 11ty config named on the `Ported from` line changed
/// since, or isn't the one 11ty reads any more. `root` is the project folder.
///
/// The config 11ty reads now is the one the build script's `--config` names,
/// else the first of 11ty's own names that exists
/// ([`crate::eleventy_cli::current_config`]): a rename (`.js` to `.mjs`), a
/// new `.eleventy.js` beside the old file, or a change to `--config` makes it
/// another file. When 11ty reads none, the port's settings are what is in
/// question, and that warns too.
pub fn stale_problem(root: &Path, src: &str) -> Option<Problem> {
    let (line, name, recorded) = ported_from(src)?;
    // Moved off 11ty: nothing left to check the port against.
    if crate::eleventy_cli::left_11ty(root) {
        return None;
    }
    let warn = |message: String, help: String| {
        Problem::warning(codes::CONFIG_STALE, message)
            .with_path(CONFIG_FILE)
            .at(line, 1)
            .with_help(help)
    };
    let set_line = |now_name: &str, now: &str| {
        format!(
            "Port what changed to {CONFIG_FILE}, if it changes what 11ty writes, then set this line to `{PORTED_FROM}{now_name} {now}`."
        )
    };
    let Some(current) = crate::eleventy_cli::current_config(root) else {
        return Some(warn(
            format!("11ty reads no config file now, but {CONFIG_FILE} was ported from {name}"),
            format!(
                "If you still build with 11ty, its output no longer has what {name} did, so Brrrty's may differ: run `brrrty init` in a copy of the site to see the port it gives now, or check {CONFIG_FILE} against 11ty's output (`brrrty doctor --verify`) and then delete this line. If you've moved off 11ty, remove @11ty/eleventy from package.json and its config file: Brrrty stops checking then, and nothing in {CONFIG_FILE} needs to change.",
            ),
        ));
    };
    let now = hash_of(root, &current)?;
    if current != name {
        return Some(warn(
            format!("11ty now reads {current}, not the {name} that {CONFIG_FILE} was ported from"),
            set_line(&current, &now),
        ));
    }
    (now != recorded).then(|| {
        warn(
            format!("{name} changed since {CONFIG_FILE} recorded it as ported"),
            set_line(name, &now),
        )
    })
}

/// The warnings for the site's own modules `init` read with the config, on
/// `Also ported from` lines, that changed since or are gone.
pub fn modules_problems(root: &Path, src: &str) -> Vec<Problem> {
    if ported_from(src).is_none() || crate::eleventy_cli::left_11ty(root) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, l) in src.lines().enumerate() {
        let Some(rest) = l.trim_end().strip_prefix(ALSO_PORTED_FROM) else {
            continue;
        };
        let Some((name, recorded)) = rest.trim().rsplit_once(' ') else {
            continue;
        };
        let name = name.trim();
        if !inside(name) || !recorded.starts_with("sha256:") {
            continue;
        }
        let warn = |message: String, help: String| {
            Problem::warning(codes::CONFIG_STALE, message)
                .with_path(CONFIG_FILE)
                .at(i + 1, 1)
                .with_help(help)
        };
        match hash_of(root, name) {
            None => out.push(warn(
                format!("{name} is gone, and {CONFIG_FILE} has what the 11ty config ran from it"),
                format!("Check {CONFIG_FILE} against the 11ty config as it is now, then delete this line."),
            )),
            Some(now) if now != recorded => out.push(warn(
                format!("{name} changed since {CONFIG_FILE} recorded it as ported"),
                format!(
                    "Port what changed to {CONFIG_FILE}, if it changes what 11ty writes, then set this line to `{ALSO_PORTED_FROM}{name} {now}`."
                ),
            )),
            Some(_) => {}
        }
    }
    out
}

/// The warning when the config 11ty reads exists and nothing records it as
/// ported: a hand-written `brrrty.config.rhai`, or one written before the
/// 11ty config was added. Brrrty can't check that it covers the 11ty config.
pub fn unverified_problem(root: &Path, src: &str) -> Option<Problem> {
    if ported_from(src).is_some() {
        return None;
    }
    let current = crate::eleventy_cli::current_config(root)?;
    let now = hash_of(root, &current)?;
    Some(
        Problem::warning(
            codes::CONFIG_UNVERIFIED,
            format!(
                "Brrrty can't check that {CONFIG_FILE} covers {current}: nothing records it as ported"
            ),
        )
        .with_path(CONFIG_FILE)
        .with_help(format!(
            "Brrrty doesn't run {current}, and any of it that {CONFIG_FILE} misses could change any page. Once you've checked that {CONFIG_FILE} does everything in it that changes what 11ty writes, add this line to {CONFIG_FILE}: `{PORTED_FROM}{current} {now}`. Brrrty then warns when {current} changes. {HELP_LINK}"
        )),
    )
}

/// The `11ty flags` line: its 1-based line number and the hash recorded.
fn flags_line(src: &str) -> Option<(usize, &str)> {
    use crate::eleventy_cli::FLAGS_LINE;
    src.lines().enumerate().find_map(|(i, l)| {
        let hash = l.trim_end().strip_prefix(FLAGS_LINE)?.trim();
        hash.starts_with("sha256:").then_some((i + 1, hash))
    })
}

/// The warning when what package.json's scripts pass to 11ty (the flags that
/// change its output, scripts Brrrty can't read, files using 11ty's
/// JavaScript API) isn't what the `11ty flags` line records. No line records
/// scripts that pass nothing, as for a config written before the line existed.
pub fn flags_problem(root: &Path, src: &str) -> Option<Problem> {
    use crate::eleventy_cli::{FLAGS_LINE, read};
    if crate::eleventy_cli::left_11ty(root) {
        return None;
    }
    let recorded = flags_line(src);
    let cli = read(root);
    let now = cli.as_ref().map(|c| c.fingerprint());
    // A hash 0.0.19 recorded, before comparison runs and spawned command lines were
    // told apart, stays valid.
    let legacy = crate::eleventy_cli::legacy_fingerprint(root);
    let mut sources: Vec<String> = cli.map(|c| c.sources).unwrap_or_default();
    sources.dedup();
    let sources = match sources.as_slice() {
        [] => "package.json's scripts".to_string(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    let line = recorded.map_or(1, |(l, _)| l);
    let (message, help) = match (recorded.map(|(_, h)| h), now.as_deref()) {
        (None, None) => return None,
        (Some(r), _) if legacy.as_deref() == Some(r) => return None,
        (Some(r), Some(n)) if r == n => return None,
        (None, Some(n)) => (
            format!("the 11ty flags in {sources} aren't recorded as ported in {CONFIG_FILE}"),
            format!(
                "11ty's flags win over its config, and Brrrty doesn't read them when it builds. `brrrty init --dry-run` in a copy of the site shows what they come to. Port them to {CONFIG_FILE}, then add this line to it: `{FLAGS_LINE}{n}`. {HELP_LINK}"
            ),
        ),
        (Some(_), None) => (
            format!(
                "nothing in package.json's scripts or the deploy configs passes the 11ty flags {CONFIG_FILE} records any more"
            ),
            format!(
                "If you still build with 11ty, its output no longer has what those flags did, so Brrrty's may differ: run `brrrty init` in a copy of the site to see the port it gives now, or check {CONFIG_FILE} against 11ty's output (`brrrty doctor --verify`) and then delete this line. If you've moved off 11ty, remove @11ty/eleventy from package.json and its config file: Brrrty stops checking then, and nothing in {CONFIG_FILE} needs to change."
            ),
        ),
        (Some(_), Some(n)) => (
            format!("the 11ty flags in {sources} changed since {CONFIG_FILE} recorded them"),
            format!("Port what changed to {CONFIG_FILE}, then set this line to `{FLAGS_LINE}{n}`."),
        ),
    };
    Some(
        Problem::warning(codes::CONFIG_STALE, message)
            .with_path(CONFIG_FILE)
            .at(line, 1)
            .with_help(help),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_markers_and_their_lines() {
        let src = "// head\nconfig.x();\n\n// TODO(brrrty): port this filter.\n// See x\n  //TODO(brrrty-file): port all of this file by hand: no.\n// TODO: not init's\n";
        let t = todos(src);
        assert_eq!(
            t,
            [
                Todo {
                    line: 4,
                    whole: false,
                    text: "port this filter".into()
                },
                Todo {
                    line: 6,
                    whole: true,
                    text: "port all of this file by hand: no".into()
                },
            ]
        );
    }

    #[test]
    fn markers_match_leniently() {
        for line in [
            "// todo(brrrty): x",
            "// TODO (brrrty): x",
            "//! TODO( brrrty ): x",
            "/* TODO(brrrty): x */",
            " * TODO(brrrty): x",
            "config.x(); // TODO(brrrty): x",
            "\t/// Todo(Brrrty) x",
        ] {
            let t = todos(line);
            assert_eq!(t.len(), 1, "{line}");
            assert!(!t[0].whole, "{line}");
            assert_eq!(t[0].text, "x", "{line}");
        }
        // A false match is safe: it refuses pages, naming the line.
        assert_eq!(todos("let s = \"// TODO(brrrty): x\";").len(), 1);
        let t = todos("// TODO (brrrty - file): y.\r\n");
        assert!(t[0].whole);
        assert_eq!(t[0].text, "y");
        // A mention mid-comment isn't a marker.
        assert!(todos("// see TODO(brrrty): y\n// TODO(other): z\n").is_empty());
    }

    #[test]
    fn reads_the_markers_older_inits_wrote() {
        let src = "// Written by `brrrty init` from eleventy.config.js, which Brrrty doesn't run.\n\n// TODO: port this to Rhai.\n// See https://github.com/brrrty/brrrty/blob/main/crates/brrrty-config/PORTING.md\n// eleventy.config.js, line 3:\n// TODO: mine, no See line\n";
        let t = todos(src);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].line, 3);
        // Without init's header, a `// TODO:` is a person's own.
        let hand = "// my port\n// TODO: port this to Rhai.\n// See https://brrrty.com/docs/\n";
        assert!(todos(hand).is_empty());
    }

    #[test]
    fn the_problem_names_every_line_and_the_file() {
        let src = "// Written by `brrrty init` from .eleventy.js, which Brrrty doesn't run.\n// TODO(brrrty): a.\n// TODO(brrrty): b.\n// TODO(brrrty): c.\n";
        let p = unported_problem(src).unwrap();
        assert_eq!(p.code, codes::CONFIG_TODO);
        assert_eq!(p.line, Some(2));
        assert!(
            p.message
                .contains("3 `TODO(brrrty)` markers for parts of .eleventy.js")
                && p.message.contains("(lines 2, 3 and 4)"),
            "{}",
            p.message
        );
        let whole = "// TODO(brrrty-file): port all of this file by hand: it is odd.\n";
        let p = unported_problem(whole).unwrap();
        assert_eq!(
            p.message,
            "`brrrty init` couldn't port any of the 11ty config (it is odd), so Brrrty would build with 11ty's default settings and any page could differ from 11ty's"
        );
        let many: String = (0..15).map(|_| "// TODO(brrrty): x.\n").collect();
        let p = unported_problem(&many).unwrap();
        assert!(
            p.message
                .contains("(lines 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12 and 3 more)"),
            "{}",
            p.message
        );
        assert!(unported_problem("config.x();\n").is_none());
    }

    #[test]
    fn a_changed_source_warns() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("eleventy.config.js"), "a").unwrap();
        let ok = format!(
            "// x\n{PORTED_FROM}eleventy.config.js {}\n",
            source_hash(b"a")
        );
        assert!(stale_problem(t.path(), &ok).is_none());
        std::fs::write(t.path().join("eleventy.config.js"), "b").unwrap();
        let p = stale_problem(t.path(), &ok).unwrap();
        assert_eq!(p.code, codes::CONFIG_STALE);
        assert_eq!(p.line, Some(2));
        assert!(p.help.unwrap().contains(&source_hash(b"b")));
        // Gone while 11ty is still a dependency: 11ty reads no config, so what the port
        // sets is in question; the help never says to take ported settings out.
        std::fs::write(
            t.path().join("package.json"),
            r#"{"devDependencies":{"@11ty/eleventy":"3"}}"#,
        )
        .unwrap();
        std::fs::remove_file(t.path().join("eleventy.config.js")).unwrap();
        let p = stale_problem(t.path(), &ok).unwrap();
        let help = p.help.clone().unwrap();
        assert!(
            help.contains("brrrty init") && !help.contains("Take out"),
            "{help}"
        );
        // 11ty gone too: the site has moved off it, so nothing is said.
        std::fs::write(t.path().join("package.json"), r#"{"devDependencies":{}}"#).unwrap();
        assert!(stale_problem(t.path(), &ok).is_none());
        std::fs::write(
            t.path().join("package.json"),
            r#"{"devDependencies":{"@11ty/eleventy":"3"}}"#,
        )
        .unwrap();
        assert!(
            p.message.starts_with("11ty reads no config file now"),
            "{}",
            p.message
        );
        // Only paths inside the project.
        for odd in ["../x.js", "/etc/x.js", "a/../../x.js"] {
            let odd = format!("{PORTED_FROM}{odd} {}\n", source_hash(b"a"));
            assert!(ported_from(&odd).is_none(), "{odd}");
        }
    }

    #[test]
    fn a_changed_module_warns() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("_config")).unwrap();
        std::fs::write(t.path().join("eleventy.config.js"), "a").unwrap();
        std::fs::write(t.path().join("_config/f.js"), "f").unwrap();
        let rhai = format!(
            "// x\n{PORTED_FROM}eleventy.config.js {}\n{ALSO_PORTED_FROM}_config/f.js {}\n{ALSO_PORTED_FROM}../out.js {}\n",
            source_hash(b"a"),
            source_hash(b"f"),
            source_hash(b"o"),
        );
        assert!(modules_problems(t.path(), &rhai).is_empty());
        std::fs::write(t.path().join("_config/f.js"), "g").unwrap();
        let p = modules_problems(t.path(), &rhai);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(p[0].code, codes::CONFIG_STALE);
        assert_eq!(p[0].line, Some(3));
        assert!(p[0].help.as_ref().unwrap().contains(&source_hash(b"g")));
        std::fs::remove_file(t.path().join("_config/f.js")).unwrap();
        let p = modules_problems(t.path(), &rhai);
        assert!(p[0].message.contains("is gone"), "{}", p[0].message);
        // Without a `Ported from` line, nothing.
        let alone = format!("{ALSO_PORTED_FROM}_config/f.js {}\n", source_hash(b"f"));
        assert!(modules_problems(t.path(), &alone).is_empty());
    }

    #[test]
    fn line_endings_and_a_bom_dont_change_the_hash() {
        assert_eq!(source_hash(b"a\r\nb\r\n"), source_hash(b"a\nb\n"));
        assert_eq!(source_hash(b"\xef\xbb\xbfa\n"), source_hash(b"a\n"));
        assert_ne!(source_hash(b"a\rb"), source_hash(b"a\nb"));
        assert_ne!(source_hash(b"a\n"), source_hash(b"b\n"));
    }

    #[test]
    fn a_renamed_or_shadowed_config_warns() {
        let t = tempfile::tempdir().unwrap();
        let w = |n: &str, s: &str| std::fs::write(t.path().join(n), s).unwrap();
        let rhai = format!("{PORTED_FROM}eleventy.config.js {}\n", source_hash(b"a"));
        // Renamed to .mjs: 11ty reads that one now.
        w("eleventy.config.mjs", "a");
        let p = stale_problem(t.path(), &rhai).unwrap();
        assert!(
            p.message
                .starts_with("11ty now reads eleventy.config.mjs, not the eleventy.config.js"),
            "{}",
            p.message
        );
        assert!(p.help.unwrap().contains(&format!(
            "`{PORTED_FROM}eleventy.config.mjs {}`",
            source_hash(b"a")
        )));
        // The old one back, and a new .eleventy.js beside it, which 11ty reads first.
        w("eleventy.config.js", "a");
        assert!(stale_problem(t.path(), &rhai).is_none());
        w(".eleventy.js", "b");
        let p = stale_problem(t.path(), &rhai).unwrap();
        assert!(
            p.message.starts_with("11ty now reads .eleventy.js"),
            "{}",
            p.message
        );
        // A `--config` file the build script names: only its own hash counts.
        w(
            "package.json",
            r#"{"scripts":{"build":"eleventy --config=site.config.js"}}"#,
        );
        w("site.config.js", "c");
        let custom = format!("{PORTED_FROM}site.config.js {}\n", source_hash(b"c"));
        assert!(stale_problem(t.path(), &custom).is_none());
        w("site.config.js", "d");
        assert!(stale_problem(t.path(), &custom).is_some());
        // `--config` dropped from the script: 11ty reads .eleventy.js again.
        w("package.json", r#"{"scripts":{"build":"eleventy"}}"#);
        let p = stale_problem(t.path(), &custom).unwrap();
        assert!(
            p.message
                .starts_with("11ty now reads .eleventy.js, not the site.config.js"),
            "{}",
            p.message
        );
    }

    #[test]
    fn an_empty_config_flag_or_a_config_flag_alone() {
        let t = tempfile::tempdir().unwrap();
        let w = |n: &str, s: &str| std::fs::write(t.path().join(n), s).unwrap();
        // `--config=`: 11ty reads no config, so eleventy.config.js isn't unverified.
        w("eleventy.config.js", "a");
        w(
            "package.json",
            r#"{"scripts":{"build":"eleventy --config="}}"#,
        );
        assert!(unverified_problem(t.path(), "config.x();\n").is_none());
        // A hand port of a site whose config only `--config` names.
        w(
            "package.json",
            r#"{"scripts":{"build":"eleventy --config=cfg.js"}}"#,
        );
        w("cfg.js", "b");
        let p = unverified_problem(t.path(), "config.x();\n").unwrap();
        assert!(p.message.contains("covers cfg.js"), "{}", p.message);
    }

    #[test]
    fn changed_flags_in_package_json_warn() {
        use crate::eleventy_cli::{FLAGS_LINE, read};
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("src")).unwrap();
        let w = |s: &str| std::fs::write(t.path().join("package.json"), s).unwrap();
        // No flags and no line: nothing to say, as for a config from before the line.
        w(
            r#"{"devDependencies":{"@11ty/eleventy":"3"},"scripts":{"build":"npx @11ty/eleventy --quiet"}}"#,
        );
        assert!(flags_problem(t.path(), "x\n").is_none());
        // Flags with no line: a hand port, or an old init, that doesn't port them.
        w(
            r#"{"devDependencies":{"@11ty/eleventy":"3"},"scripts":{"build":"eleventy --pathprefix=/blog/ --input=src"}}"#,
        );
        let p = flags_problem(t.path(), "x\n").unwrap();
        assert_eq!(p.code, codes::CONFIG_STALE);
        let now = read(t.path()).unwrap().fingerprint();
        assert!(p.help.unwrap().contains(&format!("`{FLAGS_LINE}{now}`")));
        let recorded = format!("// a\n{FLAGS_LINE}{now}\n");
        assert!(flags_problem(t.path(), &recorded).is_none());
        // Changed, then gone.
        w(
            r#"{"devDependencies":{"@11ty/eleventy":"3"},"scripts":{"build":"eleventy --input=src"}}"#,
        );
        let p = flags_problem(t.path(), &recorded).unwrap();
        assert_eq!(p.line, Some(2));
        assert_eq!(
            p.message,
            "the 11ty flags in package.json, scripts.build changed since brrrty.config.rhai recorded them"
        );
        w(r#"{"devDependencies":{"@11ty/eleventy":"3"},"scripts":{"build":"eleventy"}}"#);
        let p = flags_problem(t.path(), &recorded).unwrap();
        assert!(p.message.contains("passes the 11ty flags"), "{}", p.message);
        assert!(!p.help.unwrap().contains("Take out"));
        // Moved off 11ty: no dependency, no config file. Nothing to say.
        w(r#"{"scripts":{"build":"brrrty build"}}"#);
        assert!(flags_problem(t.path(), &recorded).is_none());
        // Flag order doesn't matter to minimist, so it doesn't change the hash.
        w(
            r#"{"devDependencies":{"@11ty/eleventy":"3"},"scripts":{"build":"eleventy --output=dist --input=src"}}"#,
        );
        let now = read(t.path()).unwrap().fingerprint();
        w(
            r#"{"devDependencies":{"@11ty/eleventy":"3"},"scripts":{"build":"eleventy --input=src --output=dist"}}"#,
        );
        assert_eq!(read(t.path()).unwrap().fingerprint(), now);
    }

    #[test]
    fn an_unrecorded_config_is_unverified() {
        let t = tempfile::tempdir().unwrap();
        assert!(unverified_problem(t.path(), "config.x();\n").is_none());
        std::fs::write(t.path().join("eleventy.config.cjs"), "a\r\n").unwrap();
        let p = unverified_problem(t.path(), "config.x();\n").unwrap();
        assert_eq!(p.code, codes::CONFIG_UNVERIFIED);
        let line = format!("{PORTED_FROM}eleventy.config.cjs {}", source_hash(b"a\n"));
        assert!(p.help.unwrap().contains(&format!("`{line}`")));
        // Adding that line is the acknowledgement.
        let acked = format!("{line}\nconfig.x();\n");
        assert!(unverified_problem(t.path(), &acked).is_none());
        assert!(stale_problem(t.path(), &acked).is_none());
    }
}
