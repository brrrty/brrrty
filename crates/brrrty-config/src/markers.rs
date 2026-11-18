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
pub fn todos(src: &str) -> Vec<Todo> {
    let lines: Vec<&str> = src.lines().collect();
    let by_init = lines
        .iter()
        .take(5)
        .any(|l| l.starts_with("// Written by `brrrty init`"));
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(text) = comment(line) else { continue };
        let (whole, rest) = if let Some(r) = text.strip_prefix(TODO_FILE) {
            (true, r)
        } else if let Some(r) = text.strip_prefix(TODO) {
            (false, r)
        } else if by_init
            && let Some(r) = text.strip_prefix("TODO: ")
            && lines.get(i + 1).is_some_and(|n| {
                n.starts_with("// See https://brrrty.com/")
                    || n.starts_with("// See https://github.com/brrrty/")
            })
        {
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
/// names and the hash recorded.
pub fn ported_from(src: &str) -> Option<(usize, &str, &str)> {
    src.lines().enumerate().find_map(|(i, l)| {
        let rest = l.strip_prefix(PORTED_FROM)?;
        let (name, hash) = rest.trim().split_once(' ')?;
        let hash = hash.trim();
        (ELEVENTY_CONFIGS.contains(&name) && hash.starts_with("sha256:")).then_some((
            i + 1,
            name,
            hash,
        ))
    })
}

/// The hash a `Ported from` line records: `sha256:` and the hex digest.
pub fn source_hash(bytes: &[u8]) -> String {
    let mut out = String::from("sha256:");
    for b in Sha256::digest(bytes) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// The 11ty config the file was ported from, as its header names it.
fn source_name(src: &str) -> Option<&str> {
    if let Some((_, name, _)) = ported_from(src) {
        return Some(name);
    }
    let first = src.lines().next()?;
    let rest = first.strip_prefix("// Written by `brrrty init` from ")?;
    let name = rest.split([',', ' ']).next()?;
    ELEVENTY_CONFIGS.contains(&name).then_some(name)
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
/// since. `root` is the project folder. A missing 11ty config is no warning:
/// 11ty can't build the site then, so there is nothing to differ from.
pub fn stale_problem(root: &Path, src: &str) -> Option<Problem> {
    use std::io::Read as _;
    let (line, name, recorded) = ported_from(src)?;
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
    let now = source_hash(&bytes);
    if now == recorded {
        return None;
    }
    Some(
        Problem::warning(
            codes::CONFIG_STALE,
            format!("{name} changed after `brrrty init` ported it to {CONFIG_FILE}"),
        )
        .with_path(CONFIG_FILE)
        .at(line, 1)
        .with_help(format!(
            "Port what changed to {CONFIG_FILE}, if it changes what 11ty writes, then set this line to `{PORTED_FROM}{name} {now}`."
        )),
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
    fn a_marker_must_start_its_comment() {
        assert!(todos("let s = \"// TODO(brrrty): x\";\n// see TODO(brrrty): y\n").is_empty());
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
        std::fs::remove_file(t.path().join("eleventy.config.js")).unwrap();
        assert!(stale_problem(t.path(), &ok).is_none());
        // Only 11ty's config names.
        let odd = format!("{PORTED_FROM}../x.js {}\n", source_hash(b"a"));
        assert!(ported_from(&odd).is_none());
    }
}
