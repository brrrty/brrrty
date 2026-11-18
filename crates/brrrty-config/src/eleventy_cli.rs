//! The flags `package.json`'s scripts pass to 11ty.
//!
//! 11ty 3.1.8's command line (`cmd.cjs`) takes, through minimist, the string
//! flags `--input`, `--output`, `--formats`, `--config`, `--pathprefix`,
//! `--port`, `--to`, `--incremental` and `--loader`, and the booleans
//! `--quiet`, `--version`, `--watch`, `--dryrun`, `--help`, `--serve` and
//! `--ignore-initial`. Anything else stops it. A flag wins over the config:
//! `--input` and `--output` freeze the folders (`Eleventy.directories`),
//! `--pathprefix` is an override (`TemplateConfig.setPathPrefix`), `--formats`
//! replaces the template formats, and `--config` names the one config file
//! read (an empty value reads none; a missing file is an error).
//!
//! `brrrty init` ports `--input` (a folder) and `--output` into the config's
//! `dir`, reads the file `--config` names instead of the default, and marks
//! every other flag that changes what 11ty writes with a TODO. A script that
//! mentions 11ty in a way this can't read, or runs a file that uses 11ty's
//! JavaScript API, is a TODO too: never silence.
//!
//! A build reads the scripts again ([`read`]) to warn when they changed since
//! `init` recorded them ([`Cli::fingerprint`]), and to know which config file
//! 11ty reads ([`current_config`]).

use std::path::Path;

use crate::markers::source_hash;

/// 11ty's string flags.
const STRINGS: [&str; 9] = [
    "input",
    "output",
    "formats",
    "config",
    "pathprefix",
    "port",
    "to",
    "incremental",
    "loader",
];
/// 11ty's boolean flags.
const BOOLS: [&str; 7] = [
    "quiet",
    "version",
    "watch",
    "dryrun",
    "help",
    "serve",
    "ignore-initial",
];
/// Flags that change nothing 11ty writes in a build.
const HARMLESS: [&str; 8] = [
    "quiet",
    "version",
    "help",
    "watch",
    "serve",
    "port",
    "ignore-initial",
    "loader",
];

/// A TODO about a script.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliTodo {
    pub why: String,
    /// The script's name, like `build`.
    pub script: String,
    /// The script, as written.
    pub command: String,
}

/// What the script that builds the site passes to 11ty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cli {
    /// The script's name, like `build`, when one 11ty command was read.
    pub script: String,
    /// That script, as written.
    pub command: String,
    /// `--input`, a folder.
    pub input: Option<String>,
    /// `--output`.
    pub output: Option<String>,
    /// `--config`; `Some("")` reads no config file.
    pub config: Option<String>,
    /// Why each other flag or script that changes the output isn't ported.
    pub todos: Vec<CliTodo>,
    /// What was read, for [`Cli::fingerprint`]: the flags that matter, the
    /// scripts that couldn't be read and the API files with their hashes. No
    /// script names, so `init` renaming `build` to `build:11ty` changes nothing.
    facts: Vec<String>,
    /// Where the facts come from, as messages name them.
    pub sources: Vec<String>,
}

impl Cli {
    /// The hash a config records on its [`FLAGS_LINE`] line.
    pub fn fingerprint(&self) -> String {
        let mut facts = self.facts.clone();
        facts.sort();
        facts.dedup();
        source_hash(facts.join("\n").as_bytes())
    }

    fn todo(&mut self, why: String, script: &str, command: &str) {
        self.todos.push(CliTodo {
            why,
            script: script.to_string(),
            command: command.to_string(),
        });
    }
}

/// Where a script comes from, as a TODO quotes it: `package.json, scripts.build`, or a
/// deploy config's own label.
pub fn place(name: &str) -> String {
    if let Some((first, rest)) = name.split_once(" → ") {
        return format!("{} → {rest}", place(first));
    }
    if is_deploy(name) {
        name.to_string()
    } else {
        format!("package.json, scripts.{name}")
    }
}

/// Whether a script name is a deploy config's label (`netlify.toml, [build] command`).
fn is_deploy(name: &str) -> bool {
    [
        "netlify.toml, ",
        "vercel.json, ",
        ".gitlab-ci.yml, ",
        ".github/workflows/",
    ]
    .iter()
    .any(|p| name.starts_with(p))
}

/// A file in `root`, read up to 1 MiB, if it is one.
fn read_text(path: &Path) -> Option<String> {
    use std::io::Read as _;
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(1 << 20)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// The build commands deploy configs in `root` run, as `(label, command)`: Netlify's
/// `[build] command` and `[context.production] command`, Vercel's `buildCommand`, and
/// the `run:` steps of GitHub Actions workflows and the `script:` lists of GitLab CI.
///
/// The third value is set for a line that mentions 11ty but isn't a command
/// the scan could read: a TODO, whatever it says.
fn deploy_commands(root: &Path) -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    if let Some(t) =
        read_text(&root.join("netlify.toml")).and_then(|t| toml::from_str::<toml::Table>(&t).ok())
    {
        for (table, label) in [
            (t.get("build"), "[build] command"),
            (
                t.get("context").and_then(|c| c.get("production")),
                "[context.production] command",
            ),
        ] {
            if let Some(c) = table
                .and_then(|b| b.get("command"))
                .and_then(|c| c.as_str())
            {
                out.push((format!("netlify.toml, {label}"), c.to_string(), false));
            }
        }
    }
    if let Some(c) = read_text(&root.join("vercel.json"))
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("buildCommand")?.as_str().map(str::to_string))
    {
        out.push(("vercel.json, buildCommand".into(), c, false));
    }
    if let Ok(dir) = std::fs::read_dir(root.join(".github/workflows")) {
        let mut files: Vec<_> = dir
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
            .collect();
        files.sort();
        for f in files {
            if let Some(t) = read_text(&root.join(".github/workflows").join(&f)) {
                let (cmds, covered) = yaml_commands(&t, &["run"]);
                for (line, cmd) in cmds {
                    out.push((format!(".github/workflows/{f}, line {line}"), cmd, false));
                }
                for (line, text) in yaml_unread(&t, &covered) {
                    out.push((format!(".github/workflows/{f}, line {line}"), text, true));
                }
            }
        }
    }
    if let Some(t) = read_text(&root.join(".gitlab-ci.yml")) {
        let (cmds, covered) = yaml_commands(&t, &["script", "before_script", "after_script"]);
        for (line, cmd) in cmds {
            out.push((format!(".gitlab-ci.yml, line {line}"), cmd, false));
        }
        for (line, text) in yaml_unread(&t, &covered) {
            out.push((format!(".gitlab-ci.yml, line {line}"), text, true));
        }
    }
    out
}

/// The shell commands under `keys` in a YAML file, read a line at a time:
/// `key: command`, `key: [a, b]`, a block scalar (`key: |`), and a list of
/// items (`- command`, `- |`) indented under the key. Gives each with its
/// 1-based line.
fn yaml_commands(text: &str, keys: &[&str]) -> (Vec<(usize, String)>, Vec<bool>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut covered = vec![false; lines.len()];
    let indent = |l: &str| l.len() - l.trim_start().len();
    let unquote = |v: &str| {
        let v = v.trim();
        if v.len() >= 2
            && ((v.starts_with('"') && v.ends_with('"'))
                || (v.starts_with('\'') && v.ends_with('\'')))
        {
            v[1..v.len() - 1].to_string()
        } else {
            v.to_string()
        }
    };
    // The lines of a block scalar after `at`, more indented than `base`.
    let block = |at: usize, base: usize, folded: bool, covered: &mut Vec<bool>| {
        let mut body = Vec::new();
        for (k, l) in lines.iter().enumerate().skip(at + 1) {
            if !l.trim().is_empty() && indent(l) <= base {
                break;
            }
            covered[k] = true;
            body.push(l.trim());
        }
        body.join(if folded { " " } else { "\n" })
    };
    // A plain scalar's continuation lines after `at`, more indented than `col`, folded
    // in with spaces: not a list item, not a `key:`.
    let plain = |at: usize, col: usize, first: String, covered: &mut Vec<bool>| {
        let mut value = first;
        for (k, l) in lines.iter().enumerate().skip(at + 1) {
            let t = l.trim();
            if t.is_empty()
                || indent(l) <= col
                || t.starts_with("- ")
                || t.starts_with('#')
                || t.split_once(':')
                    .is_some_and(|(k, v)| !k.contains(' ') && (v.is_empty() || v.starts_with(' ')))
            {
                break;
            }
            covered[k] = true;
            value.push(' ');
            value.push_str(t);
        }
        value
    };
    let is_block = |v: &str| v.starts_with('|') || v.starts_with('>');
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        let t0 = l.trim_start();
        let item = t0.starts_with("- ");
        let t = t0.strip_prefix("- ").unwrap_or(t0).trim_start();
        let Some((key, value)) = t.split_once(':') else {
            i += 1;
            continue;
        };
        let key = key.trim().trim_matches(['"', '\'']);
        if t.starts_with('#') || !keys.contains(&key) {
            i += 1;
            continue;
        }
        covered[i] = true;
        let base = indent(l);
        let col = base + if item { 2 } else { 0 };
        let value = value.trim();
        if is_block(value) {
            out.push((i + 1, block(i, base, value.starts_with('>'), &mut covered)));
        } else if let Some(list) = value.strip_prefix('[') {
            for item in list.trim_end_matches(']').split(',') {
                out.push((i + 1, unquote(item)));
            }
        } else if !value.is_empty() && !value.starts_with('#') {
            let v = plain(i, col, unquote(value), &mut covered);
            out.push((i + 1, v));
        } else {
            // A list of items under the key.
            let mut k = i + 1;
            while k < lines.len() {
                let m = lines[k];
                if m.trim().is_empty() || m.trim_start().starts_with('#') {
                    k += 1;
                    continue;
                }
                if indent(m) < base || (indent(m) == base && !m.trim_start().starts_with("- ")) {
                    break;
                }
                let Some(item) = m.trim_start().strip_prefix("- ") else {
                    break;
                };
                covered[k] = true;
                let item = item.trim();
                if is_block(item) {
                    out.push((
                        k + 1,
                        block(k, indent(m), item.starts_with('>'), &mut covered),
                    ));
                } else {
                    out.push((k + 1, plain(k, indent(m), unquote(item), &mut covered)));
                }
                // Past the item's own block or continuation lines.
                k += 1;
                while k < lines.len() && covered[k] {
                    k += 1;
                }
            }
        }
        i += 1;
    }
    (out, covered)
}

/// Lines of a CI file the scan didn't read as a command that still mention 11ty: an
/// anchor, a `!reference`, a flow-style step. They are TODOs, never silence. Comments,
/// and prose keys (`name:`, `description:`, `title:`), don't count.
fn yaml_unread(text: &str, covered: &[bool]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (i, l) in text.lines().enumerate() {
        let t = l.trim();
        if covered.get(i).copied().unwrap_or(false) || t.is_empty() || t.starts_with('#') {
            continue;
        }
        let t = t.split(" #").next().unwrap_or(t);
        let key = t
            .strip_prefix("- ")
            .unwrap_or(t)
            .split_once(':')
            .map(|(k, _)| k.trim().trim_matches(['"', '\'']));
        if key.is_some_and(|k| matches!(k, "name" | "description" | "title")) {
            continue;
        }
        if mentions(t) {
            out.push((i + 1, t.to_string()));
        }
    }
    out
}

/// How the line recording the scripts' 11ty flags starts.
pub const FLAGS_LINE: &str = "// 11ty flags from package.json: ";

/// `cmd` up to a shell comment: a `#` that starts an unquoted word.
fn without_comment(cmd: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut prev_space = true;
    for (i, c) in cmd.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '#' && prev_space => return &cmd[..i],
            None => {}
        }
        prev_space = quote.is_none() && c.is_whitespace();
    }
    cmd
}

/// A command line split into words, with `'...'` and `"..."` quoting.
fn words(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in cmd.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            None => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

/// The flags as minimist reads them: `(name, value)`, with `None` for an
/// argument 11ty doesn't know (it stops on one).
fn parse(args: &[String]) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        let Some(flag) = a.strip_prefix("--") else {
            out.push((a.clone(), None));
            continue;
        };
        let (name, value) = match flag.split_once('=') {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (flag.to_string(), None),
        };
        if STRINGS.contains(&name.as_str()) {
            let value = value.or_else(|| {
                let next = args.get(i)?;
                let flagish = next.len() > 1
                    && next.starts_with('-')
                    && !next.starts_with("---")
                    && next != "--";
                (!flagish).then(|| {
                    i += 1;
                    next.clone()
                })
            });
            out.push((name, Some(value.unwrap_or_default())));
        } else if BOOLS.contains(&name.as_str()) {
            let value = value.or_else(|| {
                let next = args.get(i)?;
                (next == "true" || next == "false").then(|| {
                    i += 1;
                    next.clone()
                })
            });
            out.push((name, Some(value.unwrap_or_else(|| "true".into()))));
        } else {
            out.push((a.clone(), None));
        }
    }
    out
}

/// The flags in `args` that change what 11ty writes, in order.
fn significant(args: &[String]) -> Vec<(String, Option<String>)> {
    parse(args)
        .into_iter()
        .filter(|(n, v)| v.is_none() || !HARMLESS.contains(&n.as_str()))
        .collect()
}

/// Whether `args` serve or watch rather than build.
fn serves(args: &[String]) -> bool {
    parse(args)
        .iter()
        .any(|(n, v)| (n == "serve" || n == "watch") && v.as_deref() != Some("false"))
}

/// Whether a word names the 11ty program: `eleventy`, a path to its bin, or
/// the package (`@11ty/eleventy`, `@11ty/eleventy@3.1.8`).
fn is_program(w: &str) -> bool {
    w == "eleventy"
        || w.ends_with("/.bin/eleventy")
        || w == "@11ty/eleventy"
        || w.starts_with("@11ty/eleventy@")
}

/// Whether a word mentions 11ty: the program, or the package (not
/// `@11ty/eleventy-img` and the like).
fn mentions(w: &str) -> bool {
    // A quoted word may hold a whole command (`concurrently "eleventy --serve"`).
    w.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '=' | '(' | ')'))
        .any(mentions_word)
}

fn mentions_word(w: &str) -> bool {
    let w = w.to_ascii_lowercase();
    if w == "eleventy" || w.ends_with("/eleventy") || w.ends_with("/eleventy.cmd") {
        return true;
    }
    let mut at = 0;
    while let Some(i) = w[at..].find("@11ty/eleventy") {
        let end = at + i + "@11ty/eleventy".len();
        if !w[end..].starts_with('-') {
            return true;
        }
        at = end;
    }
    false
}

/// `w` with shell redirections (`> log.txt`, `2>&1`, `&>out`, `< in`) left out.
fn without_redirections(w: Vec<String>) -> Vec<String> {
    let op = regex::Regex::new(r"^(\d*|&)(>>?|<)(&(\d+|-))?$").expect("valid");
    let attached = regex::Regex::new(r"^(\d*|&)(>>?|<)\S").expect("valid");
    let mut out = Vec::new();
    let mut it = w.into_iter();
    while let Some(x) = it.next() {
        if let Some(c) = op.captures(&x) {
            if c.get(3).is_none() {
                it.next();
            }
        } else if !attached.is_match(&x) {
            out.push(x);
        }
    }
    out
}

/// One step of a script, as far as 11ty goes.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// The 11ty command, with its arguments.
    Eleventy(Vec<String>),
    /// `node file`, where `file` may use 11ty's JavaScript API.
    Runs(String),
    /// `cd` to somewhere else.
    Cd,
    /// It mentions 11ty in a way this can't read.
    Unreadable,
    Other,
}

/// The steps of a script: `&&`, `||`, `;`, `|` and line breaks split it, and
/// a subshell's or group's brackets are dropped.
fn steps(script: &str) -> Vec<String> {
    script
        .split("&&")
        .flat_map(|p| p.split("||"))
        .flat_map(|p| p.split(';'))
        .flat_map(|p| p.split('|'))
        .flat_map(|p| p.split('\n'))
        .map(|p| {
            p.trim()
                .trim_start_matches(['(', '{', ' '])
                .trim_end_matches([')', '}', ' '])
                .trim()
                .to_string()
        })
        .filter(|p| !p.is_empty())
        .collect()
}

/// What `step` does. `scripts` are package.json's script names, which
/// `pnpm x` and `yarn x` run before a bin of that name.
fn classify(step: &str, scripts: &serde_json::Map<String, serde_json::Value>) -> Step {
    // A shell comment ends the command.
    let mut w = without_redirections(words(without_comment(step)));
    let assignment = |x: &str| {
        x.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty()
                && !k.starts_with(|c: char| c.is_ascii_digit())
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    };
    loop {
        while w.first().is_some_and(|x| assignment(x)) {
            w.remove(0);
        }
        if w.first().is_some_and(|x| x == "cross-env" || x == "env") {
            w.remove(0);
            continue;
        }
        break;
    }
    let unreadable = |w: &[String]| {
        if w.iter().any(|x| mentions(x)) {
            Step::Unreadable
        } else {
            Step::Other
        }
    };
    let Some(first) = w.first().cloned() else {
        return Step::Other;
    };
    let get = |i: usize| w.get(i).map(String::as_str).unwrap_or_default();
    let mut i = 0;
    match first.as_str() {
        "cd" => {
            return if matches!(get(1), "." | "./") && w.len() == 2 {
                Step::Other
            } else {
                Step::Cd
            };
        }
        "npx" | "bunx" => i = 1,
        "npm" => match get(1) {
            "exec" | "x" => i = 2,
            _ => return Step::Other,
        },
        "pnpm" | "yarn" => {
            i = 1;
            match get(1) {
                "exec" | "dlx" => i = 2,
                "run" | "run-script" => i = 2,
                _ => {}
            }
            if scripts.contains_key(get(i)) {
                return Step::Other;
            }
        }
        "bun" => match get(1) {
            "x" => i = 2,
            "run" if scripts.contains_key(get(2)) => return Step::Other,
            // `bun run <bin>` runs a package's bin, `bun run <file>` a file.
            "run" if is_program(get(2)) => i = 2,
            "run" => return Step::Runs(get(2).to_string()),
            _ => i = usize::MAX,
        },
        "node" | "tsx" | "ts-node" => i = usize::MAX,
        // A file run directly, through its shebang.
        f if f.ends_with("@11ty/eleventy/cmd.cjs") || f.ends_with("@11ty/eleventy/cmd.js") => {
            return Step::Eleventy(w[1..].to_vec());
        }
        f if !is_program(f)
            && !mentions(f)
            && (f.contains('/')
                || f.ends_with(".js")
                || f.ends_with(".mjs")
                || f.ends_with(".cjs")) =>
        {
            return Step::Runs(f.to_string());
        }
        _ => {}
    }
    if i == usize::MAX {
        // A JavaScript runtime: skip its own flags to the file it runs.
        let mut k = 1;
        while get(k).starts_with('-') {
            let takes = matches!(
                get(k),
                "-r" | "--require"
                    | "--import"
                    | "--loader"
                    | "--experimental-loader"
                    | "-e"
                    | "--eval"
                    | "-p"
                    | "--print"
            );
            if matches!(get(k), "-e" | "--eval" | "-p" | "--print") {
                return unreadable(&w);
            }
            k += if takes { 2 } else { 1 };
        }
        let file = get(k);
        if file.ends_with("@11ty/eleventy/cmd.cjs") || file.ends_with("@11ty/eleventy/cmd.js") {
            return Step::Eleventy(w[k + 1..].to_vec());
        }
        if file.is_empty() {
            return unreadable(&w);
        }
        return Step::Runs(file.to_string());
    }
    // A runner's own flags, then `--`.
    while get(i).starts_with('-') && get(i) != "--" {
        i += if matches!(get(i), "-p" | "--package" | "-c" | "--call") {
            2
        } else {
            1
        };
    }
    if get(i) == "--" {
        i += 1;
    }
    if is_program(get(i)) {
        return Step::Eleventy(w[i + 1..].to_vec());
    }
    unreadable(&w)
}

/// The most files [`uses_api`] reads, following a file's local imports.
const MAX_FILES: usize = 64;

/// The file Node runs for `spec` (relative to `base`, a path inside `root`):
/// the path itself, then with `.js`, `.mjs` or `.cjs`, then a folder's `index`.
fn resolve(root: &Path, base: &Path, spec: &str) -> Option<std::path::PathBuf> {
    let rel = normalize(&base.join(spec))?;
    let mut tries = vec![rel.clone()];
    for ext in ["js", "mjs", "cjs"] {
        let mut name = rel.as_os_str().to_owned();
        name.push(format!(".{ext}"));
        tries.push(name.into());
    }
    for ext in ["js", "mjs", "cjs"] {
        tries.push(rel.join(format!("index.{ext}")));
    }
    tries
        .into_iter()
        .find(|t| std::fs::metadata(root.join(t)).is_ok_and(|m| m.is_file()))
}

/// `p` with `.` and `..` worked out, `None` if it leaves the project.
fn normalize(p: &Path) -> Option<std::path::PathBuf> {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(x) => out.push(x),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Whether the file a script runs uses 11ty's JavaScript API, itself or
/// through the site's own modules it loads (Node's resolution, up to
/// [`MAX_FILES`] files); a hash of every file read when it does.
fn uses_api(root: &Path, file: &str) -> Option<String> {
    use std::io::Read as _;
    let api = regex::Regex::new(r#"@11ty/eleventy["'`/]"#).expect("valid");
    let local = regex::Regex::new(r#"["'`](\.{1,2}/[^"'`\s]+)["'`]"#).expect("valid");
    let first = resolve(root, Path::new(""), file)?;
    let mut queue = vec![first];
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    let mut all = Vec::new();
    let mut found = false;
    while let Some(rel) = queue.pop() {
        if seen.contains(&rel) || seen.len() >= MAX_FILES {
            continue;
        }
        seen.push(rel.clone());
        let mut bytes = Vec::new();
        let Ok(f) = std::fs::File::open(root.join(&rel)) else {
            continue;
        };
        if f.take(1 << 20).read_to_end(&mut bytes).is_err() {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        found |= api.is_match(&text);
        let dir = rel.parent().unwrap_or(Path::new("")).to_path_buf();
        for c in local.captures_iter(&text) {
            if let Some(next) = resolve(root, &dir, &c[1]) {
                queue.push(next);
            }
        }
        all.extend_from_slice(&bytes);
        all.push(0);
    }
    found.then(|| source_hash(&all))
}

/// Flags in a fixed order: by name, repeated ones in the order given. The
/// order flags are written in doesn't change what minimist gives 11ty.
fn canonical(flags: &[(String, Option<String>)]) -> Vec<(String, Option<String>)> {
    let mut f = flags.to_vec();
    f.sort_by(|a, b| a.0.cmp(&b.0));
    f
}

/// Whether 11ty is in `package.json`'s dependencies.
pub fn depends_on_11ty(pkg: &serde_json::Value) -> bool {
    ["dependencies", "devDependencies"]
        .iter()
        .any(|k| pkg.get(k).and_then(|d| d.get("@11ty/eleventy")).is_some())
}

fn shown(flags: &[(String, Option<String>)]) -> String {
    if flags.is_empty() {
        return "no flags".into();
    }
    flags
        .iter()
        .map(|(n, v)| match v {
            Some(v) => format!("--{n}={v}"),
            None => n.clone(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `path` is a glob to 11ty.
fn glob(path: &str) -> bool {
    path.contains(['*', '?', '[', ']', '{', '}', '(', ')', '!'])
}

/// What the scripts of `package.json` (parsed) pass to 11ty, or `None` when
/// no script runs 11ty with a flag that changes its output. `root` is the
/// site's folder, which npm runs scripts in.
///
/// The `build` script (or the `build:11ty` init keeps) is the one that builds
/// the site. Without one that runs 11ty, it is the other scripts that run 11ty
/// to build, then those that serve or watch. When those disagree on flags, it
/// is a TODO. So is a script that mentions 11ty in a way this can't read, and
/// one that runs a file that uses 11ty's JavaScript API: those count when
/// they are `build`, or when `build` doesn't run 11ty itself.
pub fn find(pkg: &serde_json::Value, root: &Path) -> Option<Cli> {
    find_mode(pkg, root, false)
}

/// The `11ty flags` hash 0.0.19 gave the site in `root`, before comparison runs and
/// spawned command lines were told apart: a config it recorded stays in step.
pub fn legacy_fingerprint(root: &Path) -> Option<String> {
    find_mode(&package(root), root, true).map(|c| c.fingerprint())
}

/// `root/package.json`, parsed, or `{}`.
fn package(root: &Path) -> serde_json::Value {
    read_text(&root.join("package.json"))
        .and_then(|t| serde_json::from_str(t.strip_prefix('\u{feff}').unwrap_or(&t)).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// A run of 11ty a script makes: where from, the script it is in, and its arguments.
struct Run {
    /// The script's name (`build`) or a deploy config's label.
    name: String,
    /// Where it is, as messages say it: `.gitlab-ci.yml, line 59 → ci/compare.mjs`.
    chain: String,
    cmd: String,
    sig: Vec<(String, Option<String>)>,
}

/// What a file a script runs does with 11ty, following the site's own modules it
/// loads (Node's resolution, up to [`MAX_FILES`] files), with a hash of every file read.
fn js_use(root: &Path, file: &str) -> Option<(crate::eleventy_js::JsUse, String)> {
    use std::io::Read as _;
    let local = regex::Regex::new(r#"["'`](\.{1,2}/[^"'`\s]+)["'`]"#).expect("valid");
    let first = resolve(root, Path::new(""), file)?;
    let mut queue = vec![first];
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    let mut all = Vec::new();
    let mut found = crate::eleventy_js::JsUse::default();
    while let Some(rel) = queue.pop() {
        if seen.contains(&rel) || seen.len() >= MAX_FILES {
            continue;
        }
        seen.push(rel.clone());
        let mut bytes = Vec::new();
        let Ok(f) = std::fs::File::open(root.join(&rel)) else {
            continue;
        };
        if f.take(1 << 20).read_to_end(&mut bytes).is_err() {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let u = crate::eleventy_js::analyze(&text);
        found.api |= u.api;
        found.spawns.extend(u.spawns);
        let dir = rel.parent().unwrap_or(Path::new("")).to_path_buf();
        for c in local.captures_iter(&text) {
            if let Some(next) = resolve(root, &dir, &c[1]) {
                queue.push(next);
            }
        }
        all.extend_from_slice(&bytes);
        all.push(0);
    }
    (found.api || !found.spawns.is_empty()).then(|| (found, source_hash(&all)))
}

/// Whether flags name a comparison run's: `--output` (somewhere else to write) or
/// `--dryrun` (nowhere).
fn compare_only(name: &str) -> bool {
    name == "output" || name == "dryrun"
}

/// [`find`], the way 0.0.19 read the scripts when `legacy`: files that spawn 11ty's
/// command line count as using its API, and comparison runs count as builds. A config
/// recorded by 0.0.19 has that hash, which stays valid.
fn find_mode(pkg: &serde_json::Value, root: &Path, legacy: bool) -> Option<Cli> {
    let empty = serde_json::Map::new();
    let scripts = pkg
        .get("scripts")
        .and_then(|s| s.as_object())
        .unwrap_or(&empty);
    let mut tiers: [Vec<Run>; 3] = Default::default();
    let mut unreadable: Vec<(String, String)> = Vec::new();
    // (script, chain, command, fact, why)
    let mut js: Vec<(String, String, String, String, String)> = Vec::new();
    let package = scripts
        .iter()
        .filter_map(|(n, c)| Some((n.clone(), c.as_str()?.to_string(), false)));
    for (name, cmd, unread) in package.chain(deploy_commands(root)) {
        if unread {
            unreadable.push((name, cmd));
            continue;
        }
        let (name, cmd) = (&name, cmd.as_str());
        let tier_of = |args: &[String]| {
            if name == "build" || name == "build:11ty" || is_deploy(name) {
                0
            } else if !serves(args) {
                1
            } else {
                2
            }
        };
        let mut found = Vec::new();
        let mut bad = false;
        let mut cd = false;
        for step in steps(cmd) {
            match classify(&step, scripts) {
                Step::Eleventy(args) => found.push(args),
                Step::Runs(file) if legacy => {
                    if let Some(hash) = uses_api(root, &file) {
                        js.push((
                            name.clone(),
                            String::new(),
                            cmd.to_string(),
                            format!("api {file} {hash}"),
                            String::new(),
                        ));
                    }
                }
                Step::Runs(file) => {
                    let Some((u, hash)) = js_use(root, &file) else {
                        continue;
                    };
                    let chain = format!("{name} → {file}");
                    if u.api {
                        js.push((
                            name.clone(),
                            chain,
                            cmd.to_string(),
                            format!("api {file} {hash}"),
                            format!("{} runs {file}, which uses 11ty's JavaScript API (`@11ty/eleventy`); port the input and output folders, config file and other options it gives 11ty, then delete this line", place(name)),
                        ));
                    } else if u.spawns.iter().any(Option::is_none) {
                        js.push((
                            name.clone(),
                            chain,
                            cmd.to_string(),
                            format!("spawn {file} {hash}"),
                            format!("{} runs {file}, which starts 11ty's command line with arguments Brrrty couldn't read; port the ones that change what 11ty writes, then delete this line", place(name)),
                        ));
                    } else {
                        for args in u.spawns.into_iter().flatten() {
                            tiers[tier_of(&args)].push(Run {
                                name: name.clone(),
                                chain: chain.clone(),
                                cmd: cmd.to_string(),
                                sig: significant(&args),
                            });
                        }
                    }
                }
                Step::Cd => cd = true,
                Step::Unreadable => bad = true,
                Step::Other => {}
            }
        }
        // Two 11ty runs, or one after a `cd`: which flags count is past reading.
        if bad || found.len() > 1 || (cd && !found.is_empty()) {
            unreadable.push((name.clone(), cmd.to_string()));
            continue;
        }
        let Some(args) = found.pop() else { continue };
        // A deploy config's command builds the site, as `build` does (Netlify's
        // overrides it); where they disagree, that is a TODO below.
        tiers[tier_of(&args)].push(Run {
            name: name.clone(),
            chain: name.clone(),
            cmd: cmd.to_string(),
            sig: significant(&args),
        });
    }
    let is_build = |n: &str| n == "build" || n == "build:11ty" || is_deploy(n);
    let builds = !tiers[0].is_empty();
    let mut cli = Cli::default();
    for (name, cmd) in unreadable.iter().filter(|(n, _)| !builds || is_build(n)) {
        cli.facts.push(format!("unreadable {cmd}"));
        cli.sources.push(place(name));
        cli.todo(
            "Brrrty couldn't read the flags this script passes to 11ty; port the ones that change what 11ty writes (`--input` and `--output` go in `dir`), then delete this line".into(),
            name,
            cmd,
        );
    }
    for (name, chain, cmd, fact, why) in js.iter().filter(|(n, ..)| !builds || is_build(n)) {
        cli.facts.push(fact.clone());
        cli.sources.push(place(chain));
        cli.todo(why.clone(), name, cmd);
    }
    let brrrty_build = scripts
        .get("build")
        .and_then(|c| c.as_str())
        .is_some_and(|c| c.trim_start().starts_with("brrrty "));
    let Some(mut chosen) = tiers.into_iter().find(|t| !t.is_empty()) else {
        // 11ty is a dependency and the site has a build script of its own, but no script
        // runs 11ty's command and nothing above says how 11ty runs: the build may reach
        // 11ty's JavaScript API in a way this can't see. Without a build script of the
        // site's own, 11ty is run by hand, as `npx @11ty/eleventy` with no flags.
        let own_build = ["build", "build:11ty"].into_iter().find_map(|n| {
            let c = scripts.get(n)?.as_str()?;
            (!c.trim_start().starts_with("brrrty ")).then_some((n, c))
        });
        if cli.todos.is_empty()
            && depends_on_11ty(pkg)
            && let Some((name, cmd)) = own_build
        {
            cli.facts.push("no 11ty command".into());
            cli.todo(
                "@11ty/eleventy is a dependency, but no script runs 11ty's command, so Brrrty can't tell how 11ty builds this site; if a script builds it through 11ty's JavaScript API, port the options it passes, then delete this line".into(),
                name,
                cmd,
            );
        }
        return (!cli.todos.is_empty()).then_some(cli);
    };
    // Comparison runs: 11ty writing somewhere else (`--output`) or nowhere (`--dryrun`)
    // to compare with the build, as `npm run build:11ty -- --output=_site-11ty` does.
    // With another run as the build, one that differs from it only in those flags isn't
    // a build. With Brrrty as the build (`"build": "brrrty build"`), the comparison runs'
    // other flags are what 11ty builds with.
    if !legacy {
        let core = |r: &Run| {
            canonical(&r.sig)
                .into_iter()
                .filter(|(n, _)| !compare_only(n))
                .collect::<Vec<_>>()
        };
        let shaped = |r: &Run| r.sig.iter().any(|(n, _)| compare_only(n));
        if let Some(reference) = chosen.iter().find(|r| !shaped(r)).map(core) {
            chosen.retain(|r| !shaped(r) || core(r) != reference);
        } else if brrrty_build {
            for r in &mut chosen {
                r.sig.retain(|(n, _)| !compare_only(n));
            }
        }
    }
    let first = &chosen[0];
    let (script, command) = (first.name.clone(), first.cmd.clone());
    let flags = first.sig.clone();
    let key = canonical(&flags);
    cli.script = script.clone();
    cli.command = command.clone();
    if let Some(other) = chosen.iter().find(|r| canonical(&r.sig) != key) {
        let mut sets: Vec<String> = chosen.iter().map(|r| shown(&canonical(&r.sig))).collect();
        sets.sort();
        sets.dedup();
        cli.facts
            .extend(sets.into_iter().map(|s| format!("11ty {s}")));
        cli.sources.extend(chosen.iter().map(|r| place(&r.chain)));
        let why = format!(
            "{} and {} both run 11ty, with {} and with {}, so Brrrty can't tell which one builds the site; port the flags of the one that does",
            place(&first.chain),
            place(&other.chain),
            shown(&flags),
            shown(&other.sig)
        );
        cli.todo(why, &script, &command);
        return Some(cli);
    }
    if !flags.is_empty() {
        cli.facts.push(format!("11ty {}", shown(&key)));
        cli.sources.extend(chosen.iter().map(|r| place(&r.chain)));
    }
    let mut seen: Vec<&str> = Vec::new();
    for (name, value) in &flags {
        let Some(v) = value else {
            cli.todo(
                format!("11ty doesn't know `{name}` and stops on it, so this script doesn't build with 11ty as written"),
                &script,
                &command,
            );
            continue;
        };
        if seen.contains(&name.as_str()) {
            cli.todo(
                format!("`--{name}` is passed more than once; set what 11ty makes of it by hand"),
                &script,
                &command,
            );
            continue;
        }
        seen.push(name);
        let why = match name.as_str() {
            "input" => {
                if !v.is_empty() && !glob(v) && root.join(v).is_dir() {
                    cli.input = Some(v.clone());
                    None
                } else {
                    Some(format!(
                        "`--input={v}` isn't a folder in the site, and Brrrty builds a folder; set `dir.input` by hand"
                    ))
                }
            }
            "output" => {
                if !v.is_empty() && !glob(v) {
                    cli.output = Some(v.clone());
                    None
                } else {
                    Some(format!("port `--output={v}` to `dir.output`"))
                }
            }
            "config" => {
                cli.config = Some(v.clone());
                None
            }
            "pathprefix" if v.is_empty() || v == "/" => None,
            "pathprefix" => Some(format!(
                "`--pathprefix={v}` sets 11ty's `pathPrefix`, and Brrrty's is always `/` so far"
            )),
            "formats" => Some(format!(
                "`--formats={v}` sets which template formats 11ty builds, and Brrrty builds 11ty's default ones (Liquid, Markdown, Nunjucks and HTML) so far"
            )),
            "to" if v == "fs" => None,
            "to" => Some(format!(
                "`--to={v}` prints the pages instead of writing files"
            )),
            "dryrun" if v == "false" => None,
            "dryrun" => Some("`--dryrun` writes no files".to_string()),
            "incremental" => Some(format!(
                "`--incremental{}` builds only part of the site",
                if v.is_empty() {
                    String::new()
                } else {
                    format!("={v}")
                }
            )),
            _ => None,
        };
        if let Some(why) = why {
            cli.todo(why, &script, &command);
        }
    }
    (cli.input.is_some() || cli.output.is_some() || cli.config.is_some() || !cli.todos.is_empty())
        .then_some(cli)
}

/// [`find`] on `root/package.json`, or `None` without one that parses.
pub fn read(root: &Path) -> Option<Cli> {
    find(&package(root), root)
}

/// The config file 11ty reads for the site in `root`: the one the build
/// script's `--config` names (none for `--config=`, or a file that isn't
/// there, where 11ty stops), else the first of 11ty's default names there.
pub fn current_config(root: &Path) -> Option<String> {
    match read(root).and_then(|c| c.config) {
        Some(c) if c.is_empty() => None,
        Some(c) => {
            let c = config_path(root, &c);
            (crate::markers::is_inside(&c) && root.join(&c).is_file()).then_some(c)
        }
        None => crate::markers::default_config(root).map(str::to_string),
    }
}

/// A `--config` value as a path relative to `root`: `./` dropped, and an
/// absolute path inside the project made relative. Anything else as given.
pub fn config_path(root: &Path, value: &str) -> String {
    let p = Path::new(value);
    if p.is_absolute() {
        let real = |x: &Path| std::fs::canonicalize(x).unwrap_or_else(|_| x.to_path_buf());
        if let Ok(rel) = real(p).strip_prefix(real(root)) {
            return rel.to_string_lossy().replace('\\', "/");
        }
        return value.to_string();
    }
    value.trim_start_matches("./").to_string()
}

/// Whether the site has left 11ty: `@11ty/eleventy` isn't a dependency, and
/// no config file 11ty would read is there. Then a port has nothing left to
/// be checked against.
pub fn left_11ty(root: &Path) -> bool {
    use std::io::Read as _;
    let mut text = String::new();
    let depends = std::fs::File::open(root.join("package.json"))
        .ok()
        .and_then(|f| f.take(1 << 20).read_to_string(&mut text).ok())
        .and_then(|_| {
            serde_json::from_str::<serde_json::Value>(
                text.strip_prefix('\u{feff}').unwrap_or(&text),
            )
            .ok()
        })
        .is_some_and(|v| depends_on_11ty(&v));
    !depends && current_config(root).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn site() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("src")).unwrap();
        t
    }

    fn args_of(step: &str) -> Vec<(String, Option<String>)> {
        match classify(step, &serde_json::Map::new()) {
            Step::Eleventy(a) => parse(&a),
            other => panic!("{step}: {other:?}"),
        }
    }

    #[test]
    fn reads_flags_as_minimist_does() {
        assert_eq!(
            args_of("eleventy --input=src --output out --quiet --serve false"),
            [
                ("input".into(), Some("src".into())),
                ("output".into(), Some("out".into())),
                ("quiet".into(), Some("true".into())),
                ("serve".into(), Some("false".into())),
            ]
        );
        assert_eq!(
            args_of("npx @11ty/eleventy --pathprefix \"/a b/\" --config --quiet"),
            [
                ("pathprefix".into(), Some("/a b/".into())),
                ("config".into(), Some(String::new())),
                ("quiet".into(), Some("true".into())),
            ]
        );
        assert_eq!(
            args_of("eleventy -x src"),
            [("-x".into(), None), ("src".into(), None)]
        );
    }

    #[test]
    fn reads_the_common_spellings_of_the_command() {
        let want = [
            ("input".to_string(), Some("src".to_string())),
            ("pathprefix".to_string(), Some("/blog/".to_string())),
        ];
        for cmd in [
            "eleventy --input=src --pathprefix=/blog/",
            "npx eleventy --input=src --pathprefix=/blog/",
            "npx --yes @11ty/eleventy@3.1.8 --input=src --pathprefix=/blog/",
            "pnpm exec eleventy --input=src --pathprefix=/blog/",
            "pnpm eleventy --input=src --pathprefix=/blog/",
            "pnpm dlx @11ty/eleventy --input=src --pathprefix=/blog/",
            "yarn eleventy --input=src --pathprefix=/blog/",
            "yarn run eleventy --input=src --pathprefix=/blog/",
            "bunx @11ty/eleventy --input=src --pathprefix=/blog/",
            "npm exec -- eleventy --input=src --pathprefix=/blog/",
            "node node_modules/@11ty/eleventy/cmd.cjs --input=src --pathprefix=/blog/",
            "node --max-old-space-size=4096 ./node_modules/@11ty/eleventy/cmd.cjs --input=src --pathprefix=/blog/",
            "./node_modules/.bin/eleventy --input=src --pathprefix=/blog/",
            "NODE_ENV=production cross-env A=1 eleventy --input=src --pathprefix=/blog/",
            "eleventy --input=src --pathprefix=/blog/ > log.txt",
            "eleventy --input=src --pathprefix=/blog/ 2>&1",
            "eleventy --input=src --pathprefix=/blog/ >>log.txt 2> err.txt",
            "eleventy --input=src --pathprefix=/blog/ &> all.txt",
        ] {
            assert_eq!(args_of(cmd), want, "{cmd}");
        }
        let t = site();
        for cmd in [
            "(cd . && eleventy --input=src --pathprefix=/blog/)",
            "npm run css && npx eleventy --input=src --pathprefix=/blog/ | tee log",
        ] {
            let c = find(&json!({ "scripts": { "build": cmd } }), t.path()).unwrap();
            assert_eq!(c.input.as_deref(), Some("src"), "{cmd}");
            assert_eq!(c.todos.len(), 1, "{cmd}: {:?}", c.todos);
        }
    }

    #[test]
    fn a_script_that_mentions_11ty_but_cant_be_read_is_a_todo() {
        let t = site();
        for cmd in [
            "concurrently \"eleventy --input=src\" \"postcss -w\"",
            "cd site && eleventy --input=src",
            "eleventy --input=a && eleventy --input=b",
            "node -e \"require('@11ty/eleventy')\"",
            "sh -c 'eleventy --input=src'",
            "time eleventy --input=src",
        ] {
            let c = find(&json!({ "scripts": { "build": cmd } }), t.path())
                .unwrap_or_else(|| panic!("{cmd}: silent"));
            assert_eq!(c.input, None, "{cmd}");
            assert_eq!(c.todos.len(), 1, "{cmd}");
            assert!(c.todos[0].why.contains("couldn't read the flags"), "{cmd}");
            assert_eq!(c.todos[0].command, cmd);
        }
        // Not 11ty: a plugin package, a config file name, a script reference.
        for cmd in [
            "node scripts/img.js @11ty/eleventy-img",
            "cp eleventy.config.js x",
            "npm run eleventy:css",
            "rm -rf .cache",
        ] {
            assert_eq!(
                find(
                    &json!({ "scripts": { "build": cmd, "eleventy:css": "postcss" } }),
                    t.path()
                ),
                None,
                "{cmd}"
            );
        }
    }

    #[test]
    fn a_file_using_the_javascript_api_is_a_todo() {
        let t = site();
        std::fs::write(
            t.path().join("build.mjs"),
            "import Eleventy from \"@11ty/eleventy\";\nawait new Eleventy(\"src\", \"dist\").write();\n",
        )
        .unwrap();
        std::fs::write(
            t.path().join("other.mjs"),
            "import x from \"@11ty/eleventy-img\";\n",
        )
        .unwrap();
        let pkg = json!({ "scripts": { "build": "node build.mjs" } });
        let c = find(&pkg, t.path()).unwrap();
        assert!(
            c.todos[0]
                .why
                .contains("runs build.mjs, which uses 11ty's JavaScript API")
        );
        assert_eq!(
            find(
                &json!({ "scripts": { "build": "node other.mjs" } }),
                t.path()
            ),
            None
        );
        // Editing the file changes the fingerprint.
        let before = c.fingerprint();
        std::fs::write(t.path().join("build.mjs"), "import \"@11ty/eleventy\";\n").unwrap();
        assert_ne!(find(&pkg, t.path()).unwrap().fingerprint(), before);
    }

    #[test]
    fn ports_folders_and_names_the_rest() {
        let t = site();
        let pkg = json!({ "scripts": {
            "build": "eleventy --input=src --output=dist --config=site.config.js --pathprefix=/blog/ --formats=md --quiet",
            "start": "eleventy --serve"
        }});
        let c = find(&pkg, t.path()).unwrap();
        assert_eq!(c.script, "build");
        assert_eq!(c.input.as_deref(), Some("src"));
        assert_eq!(c.output.as_deref(), Some("dist"));
        assert_eq!(c.config.as_deref(), Some("site.config.js"));
        assert_eq!(c.todos.len(), 2, "{:?}", c.todos);
        assert!(c.todos[0].why.contains("--pathprefix=/blog/"));
        assert!(c.todos[1].why.contains("--formats=md"));
    }

    #[test]
    fn harmless_flags_and_a_root_prefix_are_nothing() {
        let t = site();
        let pkg = json!({ "scripts": { "build": "eleventy --quiet --pathprefix=/ --to=fs", "dev": "eleventy --serve --port 9000", "watch": "concurrently \"eleventy --watch\" x" }});
        assert_eq!(find(&pkg, t.path()), None);
        assert_eq!(
            find(&json!({ "scripts": { "test": "node --test" }}), t.path()),
            None
        );
    }

    #[test]
    fn the_fingerprint_ignores_script_names_and_harmless_flags() {
        let t = site();
        let a = find(
            &json!({ "scripts": { "build": "eleventy --input=src --quiet" }}),
            t.path(),
        )
        .unwrap();
        let b = find(
            &json!({ "scripts": { "build": "brrrty build", "build:11ty": "npx eleventy --input=src" }}),
            t.path(),
        )
        .unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint());
        let c = find(
            &json!({ "scripts": { "build": "eleventy --input=src --pathprefix=/b/" }}),
            t.path(),
        )
        .unwrap();
        assert_ne!(a.fingerprint(), c.fingerprint());
    }

    #[test]
    fn a_build_step_elsewhere_and_disagreeing_scripts() {
        let t = site();
        let pkg = json!({ "scripts": {
            "build": "npm-run-all build:*",
            "build:site": "eleventy --input src",
            "start": "eleventy --serve --input=other"
        }});
        let c = find(&pkg, t.path()).unwrap();
        assert_eq!(
            (c.script.as_str(), c.input.as_deref()),
            ("build:site", Some("src"))
        );
        let pkg = json!({ "scripts": {
            "build:a": "eleventy --input=src",
            "build:b": "rm -rf _site && eleventy"
        }});
        let c = find(&pkg, t.path()).unwrap();
        assert_eq!(c.input, None);
        assert!(c.todos[0].why.contains("both run 11ty"), "{:?}", c.todos);
    }

    #[test]
    fn an_input_that_isnt_a_folder_is_a_todo() {
        let t = site();
        let pkg = json!({ "scripts": { "build": "eleventy --input=README.md" }});
        let c = find(&pkg, t.path()).unwrap();
        assert_eq!(c.input, None);
        assert!(c.todos[0].why.contains("isn't a folder"));
    }

    #[test]
    fn the_config_11ty_reads_now() {
        let t = site();
        let w = |n: &str, s: &str| std::fs::write(t.path().join(n), s).unwrap();
        assert_eq!(current_config(t.path()), None);
        w("eleventy.config.js", "a");
        assert_eq!(
            current_config(t.path()).as_deref(),
            Some("eleventy.config.js")
        );
        w(
            "package.json",
            r#"{"scripts":{"build":"eleventy --config=./site.js"}}"#,
        );
        assert_eq!(current_config(t.path()), None);
        w("site.js", "b");
        assert_eq!(current_config(t.path()).as_deref(), Some("site.js"));
        w(
            "package.json",
            r#"{"scripts":{"build":"eleventy --config="}}"#,
        );
        assert_eq!(current_config(t.path()), None);
    }

    #[test]
    fn bun_run_a_bin_is_read() {
        assert_eq!(
            args_of("bun run eleventy --input=src"),
            [("input".to_string(), Some("src".to_string()))]
        );
        assert_eq!(
            args_of("node_modules/@11ty/eleventy/cmd.cjs --input=src"),
            [("input".to_string(), Some("src".to_string()))]
        );
    }

    #[test]
    fn api_files_are_found_as_node_finds_them() {
        let t = site();
        let w = |n: &str, s: &str| {
            let p = t.path().join(n);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, s).unwrap();
        };
        w("scripts/b.js", "const E = require(\"@11ty/eleventy\");\n");
        w(
            "scripts/c.mjs",
            "#!/usr/bin/env node\nimport E from \"@11ty/eleventy\";\n",
        );
        w(
            "scripts/d/index.mjs",
            "import { run } from \"../../lib/run.mjs\";\nrun();\n",
        );
        w(
            "lib/run.mjs",
            "import Eleventy from '@11ty/eleventy';\nexport const run = () => {};\n",
        );
        w("scripts/plain.mjs", "import x from \"./other.mjs\";\n");
        w("scripts/other.mjs", "export default 1;\n");
        for build in [
            "node scripts/b",
            "./scripts/c.mjs",
            "scripts/c.mjs --x",
            "node scripts/d",
            "bun run scripts/b.js",
            "tsx ./scripts/d/index.mjs",
        ] {
            let c = find(&json!({ "scripts": { "build": build } }), t.path())
                .unwrap_or_else(|| panic!("{build}: silent"));
            assert!(
                c.todos[0].why.contains("JavaScript API"),
                "{build}: {:?}",
                c.todos
            );
        }
        assert_eq!(
            find(
                &json!({ "scripts": { "build": "node scripts/plain.mjs" } }),
                t.path()
            ),
            None
        );
        // An edit to a module it loads is a change.
        let pkg = json!({ "scripts": { "build": "node scripts/d" } });
        let before = find(&pkg, t.path()).unwrap().fingerprint();
        w(
            "lib/run.mjs",
            "import Eleventy from '@11ty/eleventy';\nexport const run = () => 2;\n",
        );
        assert_ne!(find(&pkg, t.path()).unwrap().fingerprint(), before);
    }

    #[test]
    fn a_dependency_no_script_runs_is_a_todo_on_build() {
        let t = site();
        let deps = json!({ "@11ty/eleventy": "3" });
        for pkg in [
            json!({ "devDependencies": deps, "scripts": { "build": "node scripts/x.mjs" } }),
            json!({ "dependencies": deps, "scripts": { "build": "make site", "test": "node --test" } }),
        ] {
            let c = find(&pkg, t.path()).unwrap_or_else(|| panic!("{pkg}: silent"));
            assert_eq!(c.todos.len(), 1);
            assert_eq!(c.todos[0].script, "build");
            assert!(c.todos[0].why.contains("no script runs 11ty's command"));
        }
        // No build script of the site's own: 11ty is run by hand, with no flags.
        for scripts in [
            json!({}),
            json!({ "build": "brrrty build", "test": "node --test" }),
        ] {
            let pkg = json!({ "devDependencies": deps, "scripts": scripts });
            assert_eq!(find(&pkg, t.path()), None, "{pkg}");
        }
        // A script running 11ty elsewhere is read instead.
        let pkg = json!({ "devDependencies": deps, "scripts": { "build": "sass a b", "site": "eleventy" } });
        assert_eq!(find(&pkg, t.path()), None);
    }

    #[test]
    fn an_absolute_config_inside_the_site_is_that_file() {
        let t = site();
        std::fs::write(t.path().join("site.js"), "x").unwrap();
        let abs = t.path().join("site.js");
        assert_eq!(config_path(t.path(), &abs.to_string_lossy()), "site.js");
        std::fs::write(
            t.path().join("package.json"),
            json!({ "scripts": { "build": format!("eleventy --config={}", abs.display()) } })
                .to_string(),
        )
        .unwrap();
        assert_eq!(current_config(t.path()).as_deref(), Some("site.js"));
        assert_eq!(
            config_path(t.path(), "/elsewhere/site.js"),
            "/elsewhere/site.js"
        );
    }

    fn write(t: &tempfile::TempDir, rel: &str, body: &str) {
        let p = t.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn reads_yaml_commands_a_line_at_a_time() {
        let gh = "jobs:\n  b:\n    steps:\n      - uses: actions/checkout@v4\n      - run: npm ci\n      - name: Build\n        run: |\n          npm run css\n          npx @11ty/eleventy --input=src\n      - run: \"echo 'done'\"\n      # - run: eleventy --old\n";
        assert_eq!(
            yaml_commands(gh, &["run"]).0,
            [
                (5, "npm ci".to_string()),
                (7, "npm run css\nnpx @11ty/eleventy --input=src".to_string()),
                (10, "echo 'done'".to_string()),
            ]
        );
        let gl = "build:\n  before_script:\n    - npm ci\n  script:\n    # a comment\n    - npx @11ty/eleventy\n    - >\n      rsync a\n      b\n  artifacts:\n    paths:\n      - _site/\ntest:\n  script: npm test\nlint:\n  script: [\"npm run lint\", 'x']\n";
        assert_eq!(
            yaml_commands(gl, &["script", "before_script"]).0,
            [
                (3, "npm ci".to_string()),
                (6, "npx @11ty/eleventy".to_string()),
                (7, "rsync a b".to_string()),
                (14, "npm test".to_string()),
                (16, "npm run lint".to_string()),
                (16, "x".to_string()),
            ]
        );
    }

    #[test]
    fn deploy_configs_are_read_like_build_scripts() {
        for (file, body) in [
            (
                "netlify.toml",
                "[build]\ncommand = \"npx @11ty/eleventy --input=src --pathprefix=/blog/\"\npublish = \"_site\"\n",
            ),
            (
                "netlify.toml",
                "[context.production]\ncommand = \"npx @11ty/eleventy --input=src --pathprefix=/blog/\"\n",
            ),
            (
                "vercel.json",
                "{\"buildCommand\": \"npx @11ty/eleventy --input=src --pathprefix=/blog/\"}",
            ),
            (
                ".github/workflows/deploy.yml",
                "jobs:\n  b:\n    steps:\n      - run: npm ci\n      - run: npx @11ty/eleventy --input=src --pathprefix=/blog/\n",
            ),
            (
                ".gitlab-ci.yml",
                "pages:\n  script:\n    - npm ci\n    - npx @11ty/eleventy --input=src --pathprefix=/blog/ --output=public\n",
            ),
        ] {
            let t = site();
            write(&t, file, body);
            // With no build script in package.json, and with a flagless one, both: the
            // deploy config's flags are read (and disagree with the flagless one).
            let c = find(&json!({}), t.path()).unwrap_or_else(|| panic!("{file}: silent"));
            assert_eq!(c.input.as_deref(), Some("src"), "{file}");
            assert!(
                c.todos
                    .iter()
                    .any(|x| x.why.contains("--pathprefix=/blog/")),
                "{file}: {:?}",
                c.todos
            );
            assert!(
                place(&c.todos[0].script).starts_with(file),
                "{file}: {}",
                c.todos[0].script
            );
            let c = find(&json!({ "scripts": { "build": "eleventy" } }), t.path()).unwrap();
            assert_eq!(c.input, None, "{file}");
            assert!(
                c.todos[0].why.contains("both run 11ty"),
                "{file}: {:?}",
                c.todos
            );
            // A change to it is a change to the fingerprint.
            let before = read(t.path()).unwrap().fingerprint();
            write(&t, file, &body.replace("/blog/", "/b/"));
            assert_ne!(read(t.path()).unwrap().fingerprint(), before, "{file}");
        }
    }

    #[test]
    fn a_deploy_config_that_runs_the_build_script_or_agrees_adds_nothing() {
        let t = site();
        write(&t, "netlify.toml", "[build]\ncommand = \"npm run build\"\n");
        write(
            &t,
            ".gitlab-ci.yml",
            "b:\n  script:\n    - npm ci\n    - npm test\n    - npx @11ty/eleventy --input=src # same flags\n",
        );
        write(
            &t,
            ".github/workflows/x.yaml",
            "jobs:\n  a:\n    steps:\n      - run: |\n          # eleventy --old is a comment\n          npm run build\n",
        );
        let pkg =
            json!({ "scripts": { "build": "eleventy --input=src", "test": "eleventy --dryrun" } });
        let c = find(&pkg, t.path()).unwrap();
        assert_eq!(c.input.as_deref(), Some("src"));
        assert!(c.todos.is_empty(), "{:?}", c.todos);
        // An unreadable mention in a workflow is a TODO naming it.
        write(
            &t,
            ".github/workflows/x.yaml",
            "jobs:\n  a:\n    steps:\n      - run: cd site && eleventy\n",
        );
        let c = find(&pkg, t.path()).unwrap();
        assert_eq!(c.todos.len(), 1);
        assert_eq!(c.todos[0].script, ".github/workflows/x.yaml, line 4");
    }

    #[test]
    fn yaml_forms_the_scan_cant_read_are_todos() {
        let unread = |file: &str, body: &str| {
            let t = site();
            write(&t, file, body);
            find(&json!({}), t.path()).unwrap_or_else(|| panic!("{body}: silent"))
        };
        // Continued on the next line without `|`: read, not just marked.
        let c = unread(
            ".github/workflows/d.yml",
            "jobs:\n  b:\n    steps:\n      - run: npx @11ty/eleventy\n          --input=src --pathprefix=/blog/\n      - run: echo ok\n",
        );
        assert_eq!(c.input.as_deref(), Some("src"));
        assert!(
            c.todos[0].why.contains("--pathprefix=/blog/"),
            "{:?}",
            c.todos
        );
        let c = unread(
            ".gitlab-ci.yml",
            "pages:\n  script:\n    - npx @11ty/eleventy\n      --input=src\n",
        );
        assert_eq!(c.input.as_deref(), Some("src"));
        // An item after a `- |` block is read too (it was skipped, so a flag there was lost).
        let c = unread(
            ".gitlab-ci.yml",
            "pages:\n  script:\n    - |\n      npm ci\n      npm test\n    - npx @11ty/eleventy --input=src\n",
        );
        assert_eq!(c.input.as_deref(), Some("src"));
        assert!(c.todos.is_empty(), "{:?}", c.todos);
        // A quoted key is a key.
        let c = unread(
            ".github/workflows/d.yml",
            "jobs:\n  b:\n    steps:\n      - \"run\": npx @11ty/eleventy --input=src\n",
        );
        assert_eq!(c.input.as_deref(), Some("src"));
        // An anchor, a `!reference` list, a flow-style step: marked, naming the line.
        for (file, body, line) in [
            (
                ".gitlab-ci.yml",
                ".build: &build\n  - npx @11ty/eleventy --input=src\npages:\n  script: *build\n",
                2,
            ),
            (
                ".gitlab-ci.yml",
                ".setup:\n  build_cmds:\n    - npx @11ty/eleventy --input=src\npages:\n  script:\n    - !reference [.setup, build_cmds]\n",
                3,
            ),
            (
                ".github/workflows/d.yml",
                "jobs:\n  b:\n    steps: [{run: \"npx @11ty/eleventy --input=src\"}]\n",
                3,
            ),
        ] {
            let c = unread(file, body);
            assert_eq!(c.input, None, "{body}");
            assert!(
                c.todos
                    .iter()
                    .any(|t| t.script == format!("{file}, line {line}")
                        && t.why.contains("couldn't read the flags")),
                "{body}: {:?}",
                c.todos
            );
        }
    }

    #[test]
    fn yaml_comments_names_and_read_lines_never_trip_the_backstop() {
        let t = site();
        write(
            &t,
            ".gitlab-ci.yml",
            "# eleventy.config.js stamps the sha\nbuild:\n  # runs eleventy\n  script:\n    - npm ci  # then eleventy\n    - npx @11ty/eleventy\n    - |\n      # eleventy dry run first\n      npx @11ty/eleventy\n",
        );
        write(
            &t,
            ".github/workflows/b.yml",
            "name: Build with Eleventy\njobs:\n  b:\n    steps:\n      - name: Eleventy\n        run: npx @11ty/eleventy\n",
        );
        assert_eq!(
            find(&json!({ "scripts": { "build": "eleventy" } }), t.path()),
            None
        );
    }

    #[test]
    fn a_quoted_hash_isnt_a_comment() {
        assert_eq!(
            args_of("eleventy --input=src --pathprefix '#/'"),
            [
                ("input".to_string(), Some("src".to_string())),
                ("pathprefix".to_string(), Some("#/".to_string())),
            ]
        );
        assert_eq!(
            args_of("eleventy --input=src # --pathprefix=/x/"),
            [("input".to_string(), Some("src".to_string()))]
        );
        assert_eq!(without_comment("a b#c 'd #e' #f"), "a b#c 'd #e' ");
    }

    /// A synthetic copy of a real blog's setup: Brrrty builds, `build:11ty` keeps 11ty's
    /// command, and a CI job runs a script that spawns 11ty's command line beside Brrrty's
    /// output to compare the two.
    fn compare_site() -> (tempfile::TempDir, serde_json::Value) {
        let t = site();
        write(
            &t,
            ".gitlab-ci.yml",
            "build:\n  script:\n    - npm ci\n    - npm run build\ncompare:\n  script:\n    - npm ci\n    - node ci/compare.mjs\n",
        );
        write(
            &t,
            "ci/compare.mjs",
            "import { execFileSync } from \"node:child_process\";\nimport { mkdtempSync } from \"node:fs\";\nconst dir = mkdtempSync(\"/tmp/11ty-\");\nexecFileSync(process.execPath, [\"node_modules/@11ty/eleventy/cmd.cjs\", `--output=${dir}`, \"--quiet\"], { stdio: \"inherit\" });\nexecFileSync(\"npx\", [\"brrrty\", \"build\", \"--quiet\"]);\n",
        );
        let pkg = json!({
            "scripts": { "build": "brrrty build", "build:11ty": "npx @11ty/eleventy", "start": "brrrty serve" },
            "devDependencies": { "@11ty/eleventy": "3.1.8", "brrrty": "0.0.19" }
        });
        write(&t, "package.json", &pkg.to_string());
        (t, pkg)
    }

    #[test]
    fn a_comparison_run_in_ci_is_not_a_build() {
        let (t, pkg) = compare_site();
        assert_eq!(find(&pkg, t.path()), None);
        // 0.0.19 read it as API use, with a marker; a config it recorded stays in step.
        let old = legacy_fingerprint(t.path()).unwrap();
        let recorded = format!("{FLAGS_LINE}{old}\n");
        assert!(crate::markers::flags_problem(t.path(), &recorded).is_none());
        assert!(crate::markers::flags_problem(t.path(), "x\n").is_none());
        // As our docs suggest, in CI or a script: `--output` (and `--dryrun`) only.
        for cmd in [
            "npm run build:11ty -- --output=_site-11ty",
            "npx @11ty/eleventy --output=_site-11ty --quiet",
            "npx @11ty/eleventy --dryrun",
        ] {
            write(
                &t,
                ".github/workflows/c.yml",
                &format!("jobs:\n  c:\n    steps:\n      - run: {cmd}\n"),
            );
            assert_eq!(find(&pkg, t.path()), None, "{cmd}");
        }
    }

    #[test]
    fn with_brrrty_as_the_build_a_comparison_runs_flags_are_11tys() {
        let t = site();
        write(
            &t,
            ".gitlab-ci.yml",
            "c:\n  script:\n    - npx @11ty/eleventy --input=src --output=_site-11ty\n",
        );
        let pkg = json!({ "scripts": { "build": "brrrty build" }, "devDependencies": { "@11ty/eleventy": "3" } });
        let c = find(&pkg, t.path()).unwrap();
        assert_eq!(c.input.as_deref(), Some("src"));
        assert_eq!(c.output, None);
        assert!(c.todos.is_empty(), "{:?}", c.todos);
    }

    #[test]
    fn a_spawn_that_differs_in_more_than_output_is_a_conflict_named_by_its_chain() {
        let (t, pkg) = compare_site();
        write(
            &t,
            "ci/compare.mjs",
            "import { spawnSync } from \"node:child_process\";\nspawnSync(\"npx\", [\"@11ty/eleventy\", \"--pathprefix=/blog/\", \"--output=x\"]);\n",
        );
        let c = find(&pkg, t.path()).unwrap();
        assert_eq!(c.todos.len(), 1);
        assert!(
            c.todos[0]
                .why
                .contains(".gitlab-ci.yml, line 8 → ci/compare.mjs"),
            "{:?}",
            c.todos
        );
        assert!(
            c.sources
                .iter()
                .any(|s| s == ".gitlab-ci.yml, line 8 → ci/compare.mjs")
        );
    }

    #[test]
    fn a_spawn_whose_arguments_cant_be_read_says_so() {
        let (t, pkg) = compare_site();
        write(
            &t,
            "ci/compare.mjs",
            "import { execFileSync } from \"node:child_process\";\nconst args = process.argv.slice(2);\nexecFileSync(\"node\", [\"node_modules/@11ty/eleventy/cmd.cjs\", ...args]);\n",
        );
        let c = find(&pkg, t.path()).unwrap();
        assert_eq!(c.todos.len(), 1);
        let why = &c.todos[0].why;
        assert!(
            why.starts_with(
                ".gitlab-ci.yml, line 8 runs ci/compare.mjs, which starts 11ty's command line"
            ),
            "{why}"
        );
        assert!(!why.contains("JavaScript API"), "{why}");
    }
}
