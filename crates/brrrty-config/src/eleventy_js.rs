//! What a JavaScript file a build script runs does with 11ty: use its
//! JavaScript API (`import Eleventy from "@11ty/eleventy"`), or spawn its
//! command line (`execFileSync("node", ["node_modules/@11ty/eleventy/cmd.cjs",
//! "--output=x"])`). Read from the file's string literals, never run.

/// A string literal in a JavaScript file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Lit {
    start: usize,
    end: usize,
    text: String,
    /// A template literal with `${...}` in it.
    interpolated: bool,
}

/// The string literals of `src`, skipping comments. Regular expression
/// literals aren't told apart, which only matters for a quote inside one.
fn literals(src: &str) -> Vec<Lit> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            q @ (b'"' | b'\'' | b'`') => {
                let start = i;
                i += 1;
                let mut text = String::new();
                let mut interpolated = false;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' && i + 1 < b.len() {
                        i += 1;
                    } else if q == b'`' && b[i] == b'$' && b.get(i + 1) == Some(&b'{') {
                        interpolated = true;
                    } else if q != b'`' && b[i] == b'\n' {
                        break;
                    }
                    let ch = src[i..].chars().next().unwrap_or(' ');
                    text.push(ch);
                    i += ch.len_utf8();
                }
                out.push(Lit {
                    start,
                    end: (i + 1).min(b.len()),
                    text,
                    interpolated,
                });
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// What 11ty's command line is named by: its `cmd.cjs`, its bin, or the package.
fn names_cli(s: &str) -> bool {
    s.ends_with("@11ty/eleventy/cmd.cjs")
        || s.ends_with("@11ty/eleventy/cmd.js")
        || s.ends_with("/.bin/eleventy")
        || s == "eleventy"
        || s == "@11ty/eleventy"
        || s.starts_with("@11ty/eleventy@")
}

/// What one file does with 11ty.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct JsUse {
    /// It loads `@11ty/eleventy` as a module: the JavaScript API.
    pub api: bool,
    /// Each spawn of 11ty's command line: its arguments when they could be
    /// read from literals, `None` when not.
    pub spawns: Vec<Option<Vec<String>>>,
}

/// The text before `at`, without trailing whitespace.
fn before(src: &str, at: usize) -> &str {
    src[..at].trim_end()
}

/// The arguments after a program literal, read from the literals that follow
/// it in the same array or call: `None` when anything there isn't a plain
/// literal (a variable, a spread, an interpolation other than `--output`'s
/// value).
fn args_after(src: &str, lits: &[Lit], at: usize) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut pos = lits[at].end;
    let mut k = at + 1;
    loop {
        let rest = src[pos..].trim_start();
        let skipped = src.len() - pos - rest.len();
        if rest.starts_with(']') || rest.starts_with(')') {
            return Some(args);
        }
        let rest2 = rest.strip_prefix(',')?.trim_start();
        if rest2.starts_with(']') || rest2.starts_with(')') {
            return Some(args);
        }
        let next_at = pos + skipped + (rest.len() - rest2.len());
        let lit = lits.get(k)?;
        if lit.start != next_at {
            return None;
        }
        if lit.interpolated && !lit.text.starts_with("--output") {
            return None;
        }
        args.push(lit.text.clone());
        pos = lit.end;
        k += 1;
    }
}

/// What `src` does with 11ty.
pub(crate) fn analyze(src: &str) -> JsUse {
    let lits = literals(src);
    let mut out = JsUse::default();
    // A spawn needs something that runs a process.
    let runs = ["child_process", "execa", "spawn", "exec", "zx"]
        .iter()
        .any(|w| src.contains(w));
    let spec =
        regex::Regex::new(r"(\bfrom|\brequire\s*\(|\bimport\s*\(|\bimport)$").expect("valid");
    let assignment =
        regex::Regex::new(r"\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=$").expect("valid");
    for (i, l) in lits.iter().enumerate() {
        let t = l.text.trim();
        let specifier = spec.is_match(before(src, l.start));
        if specifier {
            if t == "@11ty/eleventy" || t.starts_with("@11ty/eleventy/") {
                if t.ends_with("/cmd.cjs") || t.ends_with("/cmd.js") {
                    // Loading cmd.cjs runs the command line with this process's arguments.
                    out.spawns.push(None);
                } else {
                    out.api = true;
                }
            }
            continue;
        }
        if !runs {
            continue;
        }
        if names_cli(t) {
            // `const cli = "…/cmd.cjs";` and then `cli` in the arguments: follow it.
            let assigned = assignment
                .captures(before(src, l.start))
                .map(|c| c[1].to_string());
            match assigned {
                Some(name) => {
                    let used = regex::Regex::new(&format!(r"[\[(,]\s*{}\s*", regex::escape(&name)))
                        .expect("valid");
                    let uses: Vec<_> = used.find_iter(&src[l.end..]).collect();
                    if uses.len() != 1 {
                        out.spawns.push(None);
                        continue;
                    }
                    let after = l.end + uses[0].end();
                    let fake = Lit {
                        start: after,
                        end: after,
                        text: String::new(),
                        interpolated: false,
                    };
                    let mut rest: Vec<Lit> = vec![fake];
                    rest.extend(lits.iter().filter(|x| x.start >= after).cloned());
                    out.spawns.push(args_after(src, &rest, 0));
                }
                None => out.spawns.push(args_after(src, &lits, i)),
            }
            continue;
        }
        // A whole command in one string: `execSync("npx @11ty/eleventy --output=x")`.
        if t.contains(' ') && t.split_whitespace().any(names_cli) {
            let words: Vec<&str> = t.split_whitespace().collect();
            let at = words.iter().position(|w| names_cli(w)).unwrap_or(0);
            let args: Vec<String> = words[at + 1..].iter().map(|w| w.to_string()).collect();
            let unknown = l.interpolated
                && args
                    .iter()
                    .any(|a| a.contains("${") && !a.starts_with("--output"));
            out.spawns.push((!unknown).then_some(args));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_and_spawns_are_told_apart() {
        let a = analyze(
            "import Eleventy from \"@11ty/eleventy\";\nconst x = require('@11ty/eleventy/src/Util.js');\n",
        );
        assert!(a.api && a.spawns.is_empty());
        let s = analyze(
            "import { execFileSync } from \"node:child_process\";\n// \"@11ty/eleventy\" in a comment\nexecFileSync(\"node\", [\"node_modules/@11ty/eleventy/cmd.cjs\", `--output=${dir}`, \"--quiet\"], { stdio: \"inherit\" });\n",
        );
        assert!(!s.api);
        assert_eq!(
            s.spawns,
            [Some(vec!["--output=${dir}".into(), "--quiet".into()])]
        );
        let s = analyze("spawn('npx', ['@11ty/eleventy', '--input', 'src'])");
        assert_eq!(s.spawns, [Some(vec!["--input".into(), "src".into()])]);
        let s = analyze("execSync(`npx @11ty/eleventy --output=${out} --quiet`)");
        assert_eq!(
            s.spawns,
            [Some(vec!["--output=${out}".into(), "--quiet".into()])]
        );
        let s = analyze(
            "const cli = \"node_modules/@11ty/eleventy/cmd.cjs\";\nspawnSync(process.execPath, [cli, \"--output=x\"]);\n",
        );
        assert_eq!(s.spawns, [Some(vec!["--output=x".into()])]);
    }

    #[test]
    fn a_mention_without_a_process_is_nothing() {
        assert_eq!(analyze("console.log(\"eleventy\")"), JsUse::default());
    }

    #[test]
    fn spawns_whose_arguments_cant_be_read() {
        for src in [
            "spawn('node', ['node_modules/@11ty/eleventy/cmd.cjs', ...args])",
            "spawn('node', ['node_modules/@11ty/eleventy/cmd.cjs', flag])",
            "spawn('npx', ['@11ty/eleventy', `--input=${dir}`])",
            "execSync(`npx @11ty/eleventy --pathprefix=${p}`)",
            "const cli = 'node_modules/.bin/eleventy'; exec(cli); exec(cli);",
            "require('@11ty/eleventy/cmd.cjs')",
        ] {
            assert_eq!(analyze(src).spawns, [None], "{src}");
        }
    }
}
