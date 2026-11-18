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

/// How the line recording the scripts' 11ty flags starts.
pub const FLAGS_LINE: &str = "// 11ty flags from package.json: ";

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
    let mut w = without_redirections(words(step));
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
            _ => i = usize::MAX,
        },
        "node" | "tsx" | "ts-node" => i = usize::MAX,
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

/// Whether the file a script runs uses 11ty's JavaScript API; its hash when it does.
fn uses_api(root: &Path, file: &str) -> Option<String> {
    use std::io::Read as _;
    if !crate::markers::is_inside(file) {
        return None;
    }
    let path = root.join(file);
    if !std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .ok()?
        .take(1 << 20)
        .read_to_end(&mut bytes)
        .ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let api = regex::Regex::new(r#"@11ty/eleventy["'`/]"#).expect("valid");
    api.is_match(&text).then(|| source_hash(&bytes))
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
    let scripts = pkg.get("scripts")?.as_object()?;
    let mut tiers: [Vec<(String, String, Vec<String>)>; 3] = Default::default();
    let mut unreadable: Vec<(String, String)> = Vec::new();
    let mut api: Vec<(String, String, String, String)> = Vec::new();
    for (name, cmd) in scripts {
        let Some(cmd) = cmd.as_str() else { continue };
        let mut found = Vec::new();
        let mut bad = false;
        let mut cd = false;
        for step in steps(cmd) {
            match classify(&step, scripts) {
                Step::Eleventy(args) => found.push(args),
                Step::Runs(file) => {
                    if let Some(hash) = uses_api(root, &file) {
                        api.push((name.clone(), cmd.to_string(), file, hash));
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
        let tier = if name == "build" || name == "build:11ty" {
            0
        } else if !serves(&args) {
            1
        } else {
            2
        };
        tiers[tier].push((name.clone(), cmd.to_string(), args));
    }
    let is_build = |n: &str| n == "build" || n == "build:11ty";
    let builds = !tiers[0].is_empty();
    let mut cli = Cli::default();
    for (name, cmd) in unreadable.iter().filter(|(n, _)| !builds || is_build(n)) {
        cli.facts.push(format!("unreadable {cmd}"));
        cli.todo(
            "Brrrty couldn't read the flags this script passes to 11ty; port the ones that change what 11ty writes (`--input` and `--output` go in `dir`), then delete this line".into(),
            name,
            cmd,
        );
    }
    for (name, cmd, file, hash) in api.iter().filter(|(n, ..)| !builds || is_build(n)) {
        cli.facts.push(format!("api {file} {hash}"));
        cli.todo(
            format!("this script runs {file}, which uses 11ty's JavaScript API (`@11ty/eleventy`); port the input and output folders, config file and other options it gives 11ty, then delete this line"),
            name,
            cmd,
        );
    }
    let Some(chosen) = tiers.into_iter().find(|t| !t.is_empty()) else {
        return (!cli.todos.is_empty()).then_some(cli);
    };
    let (script, command, args) = chosen[0].clone();
    let flags = significant(&args);
    cli.script = script.clone();
    cli.command = command.clone();
    if let Some((other, other_cmd, other_args)) =
        chosen.iter().find(|(_, _, a)| significant(a) != flags)
    {
        let mut sets: Vec<String> = chosen
            .iter()
            .map(|(_, _, a)| shown(&significant(a)))
            .collect();
        sets.sort();
        sets.dedup();
        cli.facts
            .extend(sets.into_iter().map(|s| format!("11ty {s}")));
        let why = format!(
            "package.json's scripts `{script}` and `{other}` both run 11ty, with {} and with {}, so Brrrty can't tell which one builds the site; port the flags of the one that does",
            shown(&flags),
            shown(&significant(other_args))
        );
        let _ = other_cmd;
        cli.todo(why, &script, &command);
        return Some(cli);
    }
    if !flags.is_empty() {
        cli.facts.push(format!("11ty {}", shown(&flags)));
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
    use std::io::Read as _;
    let mut text = String::new();
    std::fs::File::open(root.join("package.json"))
        .ok()?
        .take(1 << 20)
        .read_to_string(&mut text)
        .ok()?;
    let v: serde_json::Value =
        serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)).ok()?;
    find(&v, root)
}

/// The config file 11ty reads for the site in `root`: the one the build
/// script's `--config` names (none for `--config=`, or a file that isn't
/// there, where 11ty stops), else the first of 11ty's default names there.
pub fn current_config(root: &Path) -> Option<String> {
    match read(root).and_then(|c| c.config) {
        Some(c) if c.is_empty() => None,
        Some(c) => {
            let c = c.trim_start_matches("./").to_string();
            (crate::markers::is_inside(&c) && root.join(&c).is_file()).then_some(c)
        }
        None => crate::markers::default_config(root).map(str::to_string),
    }
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
}
