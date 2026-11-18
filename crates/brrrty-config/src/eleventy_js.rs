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

/// Functions that start a process: Node's `child_process`, execa, shelljs, zx.
const PROCESS_CALLS: [&str; 13] = [
    "spawn",
    "spawnSync",
    "exec",
    "execSync",
    "execFile",
    "execFileSync",
    "fork",
    "execa",
    "execaSync",
    "execaCommand",
    "execaCommandSync",
    "execaNode",
    "$",
];

/// Calls that only look at a path or print it.
const LOOKS_ONLY: [&str; 16] = [
    "existsSync",
    "exists",
    "statSync",
    "stat",
    "lstatSync",
    "lstat",
    "accessSync",
    "access",
    "readFileSync",
    "realpathSync",
    "log",
    "info",
    "warn",
    "error",
    "debug",
    "assert",
];

/// Where a token sits: inside an array literal or a call's parentheses, with the
/// bracket's position. Brackets inside string literals don't count.
fn enclosing(src: &str, lits: &[Lit], at: usize) -> Option<(u8, usize)> {
    let b = src.as_bytes();
    let mut depth = 0i32;
    let mut i = at;
    while i > 0 {
        i -= 1;
        if let Some(l) = lits.iter().find(|l| l.start <= i && i < l.end) {
            i = l.start;
            continue;
        }
        match b[i] {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' => {
                if depth == 0 {
                    return Some((b[i], i));
                }
                depth -= 1;
            }
            b';' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

/// The name of the function whose call's `(` is at `paren`: its last segment
/// (`spawnSync` for `child_process.spawnSync(`).
fn call_name(src: &str, paren: usize) -> &str {
    let head = src[..paren].trim_end();
    let start = head
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .map_or(0, |i| i + 1);
    &head[start..]
}

fn is_process_call(name: &str) -> bool {
    PROCESS_CALLS.contains(&name)
}

/// The literal starting at `pos`, read as an argument: `None` for an
/// interpolation other than `--output`'s value.
fn item_at(lits: &[Lit], pos: usize) -> Option<Option<&Lit>> {
    let lit = lits.iter().find(|l| l.start == pos)?;
    if lit.interpolated && !lit.text.starts_with("--output") {
        return Some(None);
    }
    Some(Some(lit))
}

/// The array items from `pos` (just after an item, or after `[`) to its `]`,
/// all plain literals; then the position after `]`. `None` for anything else.
fn items_to_close(
    src: &str,
    lits: &[Lit],
    mut pos: usize,
    first: bool,
) -> Option<(Vec<String>, usize)> {
    let mut out = Vec::new();
    let mut need_comma = !first;
    loop {
        let rest = src[pos..].trim_start();
        let at = src.len() - rest.len();
        if let Some(r) = rest.strip_prefix(']') {
            return Some((out, src.len() - r.len()));
        }
        let at = if need_comma {
            let r = rest.strip_prefix(',')?.trim_start();
            if let Some(r2) = r.strip_prefix(']') {
                return Some((out, src.len() - r2.len()));
            }
            src.len() - r.len()
        } else {
            at
        };
        let lit = item_at(lits, at)??;
        out.push(lit.text.clone());
        pos = lit.end;
        need_comma = true;
    }
}

/// Whether the array whose `[` is at `open` is used as it is: passed straight
/// to a process call, or a `const` never changed afterwards (no `push`,
/// `unshift`, `splice`, `concat` or index assignment), and nothing is done to
/// the literal itself (`[...].concat(x)`).
fn trusted_array(src: &str, lits: &[Lit], open: usize, close: usize) -> bool {
    if src[close..].trim_start().starts_with('.') {
        return false;
    }
    // Up through the literals it sits in (`const tools = [{ cmd, args: [...] }]`) to
    // the call it's passed to, or the `const` that holds it; noting the property
    // names on the way, which mustn't be written to either (`tool.args.push(...)`).
    let mut keys: Vec<String> = Vec::new();
    let mut open = open;
    let binding = regex::Regex::new(r"\bconst\s+([A-Za-z_$][\w$]*)\s*=$").expect("valid");
    let property = regex::Regex::new(r"([A-Za-z_$][\w$]*)\s*:$").expect("valid");
    loop {
        let head = src[..open].trim_end();
        if let Some(c) = binding.captures(head) {
            let mut names = vec![regex::escape(&c[1])];
            // Through the element (`tool.args`), or a destructured name (`{ args }`).
            names.extend(keys.iter().map(|k| format!(r"\b{}", regex::escape(k))));
            return names.iter().all(|name| {
                let changed = regex::Regex::new(&format!(
                    r"{name}\b\s*(\.\s*(push|unshift|splice|concat|pop|shift|length)\b|\[[^\]]*\]\s*=[^=]|=[^=>])"
                ))
                .expect("valid");
                !changed.is_match(&src[close..])
            });
        }
        let key = property.captures(head).map(|c| c[1].to_string());
        let Some((b, at)) = enclosing(src, lits, open) else {
            return false;
        };
        match b {
            b'(' if key.is_none() && (head.ends_with('(') || head.ends_with(',')) => {
                return is_process_call(call_name(src, at));
            }
            b'{' if key.is_some() => keys.extend(key),
            b'[' if key.is_none() && (head.ends_with('[') || head.ends_with(',')) => {}
            _ => return false,
        }
        open = at;
    }
}

/// The arguments of the spawn whose program token spans `start..end` (a
/// literal, or a `const` name holding one): `None` when they can't be read
/// with confidence.
fn spawn_args(src: &str, lits: &[Lit], start: usize, end: usize) -> Option<Vec<String>> {
    match enclosing(src, lits, start)? {
        // The program in an argument array: the rest of the array.
        (b'[', open) => {
            let (args, close) = items_to_close(src, lits, end, false)?;
            trusted_array(src, lits, open, close).then_some(args)
        }
        // The program as a process call's first argument: then an array, or nothing.
        (b'(', paren) => {
            if !is_process_call(call_name(src, paren)) || !src[paren + 1..start].trim().is_empty() {
                return None;
            }
            let rest = src[end..].trim_start();
            if rest.starts_with(')') {
                return Some(Vec::new());
            }
            let rest = rest.strip_prefix(',')?.trim_start();
            if rest.starts_with('{') || rest.starts_with(')') {
                return Some(Vec::new());
            }
            let open = src.len() - rest.len();
            rest.strip_prefix('[')?;
            let (args, close) = items_to_close(src, lits, open + 1, true)?;
            trusted_array(src, lits, open, close).then_some(args)
        }
        _ => None,
    }
}

/// What `src` does with 11ty.
pub(crate) fn analyze(src: &str) -> JsUse {
    let lits = literals(src);
    let mut out = JsUse::default();
    // A spawn needs something that runs a process.
    let runs = ["child_process", "execa", "spawn", "exec", "zx", "$`"]
        .iter()
        .any(|w| src.contains(w));
    let spec =
        regex::Regex::new(r"(\bfrom|\brequire\s*\(|\bimport\s*\(|\bimport)$").expect("valid");
    let assignment =
        regex::Regex::new(r"\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=$").expect("valid");
    for l in &lits {
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
        let whole = t.contains(' ') && t.split_whitespace().any(names_cli);
        if !names_cli(t) && !whole {
            continue;
        }
        // Built up with `+`: the rest can't be read.
        let joined =
            before(src, l.start).ends_with('+') || src[l.end..].trim_start().starts_with('+');
        // `const cli = "…/cmd.cjs";` and then `cli` once in a spawn: follow it.
        let at = match assignment.captures(before(src, l.start)) {
            Some(c) if before(src, l.start).contains("const") && !joined => {
                let used =
                    regex::Regex::new(&format!(r"\b{}\b", regex::escape(&c[1]))).expect("valid");
                let uses: Vec<_> = used.find_iter(&src[l.end..]).collect();
                match uses.as_slice() {
                    [u] => Some((l.end + u.start(), l.end + u.end())),
                    _ => None,
                }
            }
            Some(_) => None,
            None if joined => None,
            None => Some((l.start, l.end)),
        };
        if whole {
            // A whole command in one string: `execSync("npx @11ty/eleventy --output=x")`,
            // or zx's `` $`…` ``. In a call that only prints it, it's no spawn.
            let ctx = at.and_then(|(s, _)| enclosing(src, &lits, s));
            if let Some((b'(', p)) = ctx
                && matches!(
                    call_name(src, p),
                    "log" | "info" | "warn" | "error" | "debug"
                )
            {
                continue;
            }
            let zx = before(src, l.start).ends_with('$');
            let direct = zx
                || matches!(ctx, Some((b'(', p)) if is_process_call(call_name(src, p))
                    && at.is_some_and(|(s, _)| src[p + 1..s].trim().is_empty()));
            let words: Vec<&str> = t.split_whitespace().collect();
            let k = words.iter().position(|w| names_cli(w)).unwrap_or(0);
            let args: Vec<String> = words[k + 1..].iter().map(|w| w.to_string()).collect();
            let unknown = l.interpolated
                && args
                    .iter()
                    .any(|a| a.contains("${") && !a.starts_with("--output"));
            out.spawns.push((direct && !unknown).then_some(args));
            continue;
        }
        // An argument of a call that only looks at the file (`fs.existsSync(...)`) or
        // prints it isn't a spawn. `path.join(...)` still is unreadable: its result may
        // be what's spawned.
        if let Some((b'(', p)) = enclosing(src, &lits, l.start)
            && LOOKS_ONLY.contains(&call_name(src, p))
        {
            continue;
        }
        out.spawns
            .push(at.and_then(|(s, e)| spawn_args(src, &lits, s, e)));
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
    fn only_arrays_passed_as_they_are_are_read() {
        // A literal array straight into the call, or a `const` left alone.
        let s = analyze(
            "const a = [\"node_modules/@11ty/eleventy/cmd.cjs\", \"--input=src\"];\nspawnSync(\"node\", a);\n",
        );
        assert_eq!(s.spawns, [Some(vec!["--input=src".into()])]);
        let s = analyze("execa(\"eleventy\", [\"--input=src\"], { stdio: \"inherit\" })");
        assert_eq!(s.spawns, [Some(vec!["--input=src".into()])]);
        let s = analyze("$`npx @11ty/eleventy --input=src`");
        assert_eq!(s.spawns, [Some(vec!["--input=src".into()])]);
        for src in [
            // The review's three: a path built with path.join, an array pushed to, a
            // command built with `+`.
            "import path from \"node:path\";\nimport { spawnSync } from \"node:child_process\";\nconst bin = path.join(\"node_modules\", \".bin\", \"eleventy\");\nspawnSync(bin, [\"--input=src\", \"--pathprefix=/b/\"]);\n",
            "import { spawnSync } from \"node:child_process\";\nconst a = [\"node_modules/@11ty/eleventy/cmd.cjs\", \"--input=src\"];\nif (process.env.CI) a.push(\"--pathprefix=/b/\");\nspawnSync(\"node\", a);\n",
            "import { execSync } from \"node:child_process\";\nexecSync(\"npx @11ty/eleventy \" + flags);\n",
            // And their kin.
            "const args = [\"@11ty/eleventy\"]; args.unshift(\"-y\"); spawn(\"npx\", args);",
            "let a = [\"@11ty/eleventy\", \"--input=src\"]; spawn(\"npx\", a);",
            "spawn(\"npx\", [\"@11ty/eleventy\", \"--input=src\"].concat(more));",
            "const a = [\"@11ty/eleventy\"]; a[1] = \"--pathprefix=/b/\"; spawn(\"npx\", a);",
            "spawn(path.resolve(\"node_modules/@11ty/eleventy/cmd.cjs\"), [])",
            "run(\"npx @11ty/eleventy --pathprefix=/b/\"); // run wraps exec",
            "const cmd = \"npx @11ty/eleventy \" + extra; execSync(cmd);",
        ] {
            assert_eq!(analyze(src).spawns, [None], "{src}");
        }
    }

    /// The real blog's `ci/compare.mjs` keeps its runs in a `const` list of objects and
    /// loops over them.
    const TOOLS: &str = "import { spawnSync } from \"node:child_process\";\nconst OUT_11TY = \"_site-11ty\";\nconst tools = [\n  {\n    name: \"11ty\",\n    out: OUT_11TY,\n    cmd: process.execPath,\n    args: [\n      \"--require\", \"./ci/freeze-time.cjs\",\n      \"node_modules/@11ty/eleventy/cmd.cjs\", `--output=${OUT_11TY}`, \"--quiet\",\n    ],\n    times: [],\n  },\n  {\n    name: \"Brrrty\",\n    cmd: \"node_modules/.bin/brrrty\",\n    args: [\"build\", \"--quiet\"],\n  },\n];\nfor (const tool of tools) {\n  const result = spawnSync(tool.cmd, tool.args, { stdio: \"inherit\" });\n  tool.times.push(1);\n}\n";

    #[test]
    fn a_const_list_of_runs_is_read() {
        assert_eq!(
            analyze(TOOLS).spawns,
            [Some(vec!["--output=${OUT_11TY}".into(), "--quiet".into()])]
        );
        // Written to through the element or a destructured name: not trusted.
        for change in [
            "tool.args.push(\"--pathprefix=/b/\");",
            "tools[0].args = [];",
            "const { args } = tools[0]; args.unshift(\"x\");",
            "tools.push({ cmd: \"x\", args: [] });",
        ] {
            let src = TOOLS.replace("tool.times.push(1);", change);
            assert_eq!(analyze(&src).spawns, [None], "{change}");
        }
        // A check that the bin is there doesn't taint the file.
        let s = analyze(&format!(
            "import fs from \"node:fs\";\nif (!fs.existsSync(\"node_modules/.bin/eleventy\")) process.exit(1);\n{TOOLS}"
        ));
        assert_eq!(
            s.spawns,
            [Some(vec!["--output=${OUT_11TY}".into(), "--quiet".into()])]
        );
        // A `let` list isn't trusted.
        assert_eq!(
            analyze(&TOOLS.replace("const tools", "let tools")).spawns,
            [None]
        );
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
