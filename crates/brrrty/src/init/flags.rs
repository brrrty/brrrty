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
//! every other flag that changes what 11ty writes with a TODO.

use std::path::Path;

use super::{eleventy_step, is_eleventy_command};

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

/// What the script that builds the site passes to 11ty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Cli {
    /// The script's name, like `build`.
    pub script: String,
    /// The 11ty step of it.
    pub command: String,
    /// `--input`, a folder.
    pub input: Option<String>,
    /// `--output`.
    pub output: Option<String>,
    /// `--config`; `Some("")` reads no config file.
    pub config: Option<String>,
    /// Why each other flag that changes the output isn't ported.
    pub todos: Vec<String>,
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

/// The arguments after the 11ty program in an 11ty command.
fn arguments(step: &str) -> Vec<String> {
    let w = words(step);
    let at = w.iter().position(|x| {
        x == "eleventy"
            || x.ends_with("/.bin/eleventy")
            || x == "@11ty/eleventy"
            || x.starts_with("@11ty/eleventy@")
    });
    at.map_or_else(Vec::new, |i| w[i + 1..].to_vec())
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

/// The flags of `step` that change what 11ty writes, in order.
fn significant(step: &str) -> Vec<(String, Option<String>)> {
    parse(&arguments(step))
        .into_iter()
        .filter(|(n, v)| v.is_none() || !HARMLESS.contains(&n.as_str()))
        .collect()
}

/// Whether `step` serves or watches rather than builds.
fn serves(step: &str) -> bool {
    parse(&arguments(step))
        .iter()
        .any(|(n, v)| (n == "serve" || n == "watch") && v.as_deref() != Some("false"))
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
/// to build, then those that serve or watch. When those disagree on flags,
/// it is a TODO.
pub(crate) fn find(pkg: &serde_json::Value, root: &Path) -> Option<Cli> {
    let scripts = pkg.get("scripts")?.as_object()?;
    let mut tiers: [Vec<(String, String)>; 3] = Default::default();
    for (name, cmd) in scripts {
        let Some(cmd) = cmd.as_str() else { continue };
        let step = if is_eleventy_command(cmd) {
            cmd
        } else if let Some(s) = eleventy_step(cmd) {
            s
        } else {
            continue;
        };
        let tier = if name == "build" || name == "build:11ty" {
            0
        } else if !serves(step) {
            1
        } else {
            2
        };
        tiers[tier].push((name.clone(), step.to_string()));
    }
    let chosen = tiers.into_iter().find(|t| !t.is_empty())?;
    let (script, command) = chosen[0].clone();
    let flags = significant(&command);
    if let Some((other, other_cmd)) = chosen.iter().find(|(_, c)| significant(c) != flags) {
        return Some(Cli {
            script: script.clone(),
            command: command.clone(),
            todos: vec![format!(
                "package.json's scripts `{script}` and `{other}` both run 11ty, with {} and with {}, so Brrrty can't tell which one builds the site; port the flags of the one that does",
                shown(&flags),
                shown(&significant(other_cmd))
            )],
            ..Cli::default()
        });
    }
    if flags.is_empty() {
        return None;
    }
    let mut cli = Cli {
        script,
        command,
        ..Cli::default()
    };
    let mut seen: Vec<&str> = Vec::new();
    for (name, value) in &flags {
        let Some(v) = value else {
            cli.todos.push(format!(
                "11ty doesn't know `{name}` and stops on it, so this script doesn't build with 11ty as written"
            ));
            continue;
        };
        if seen.contains(&name.as_str()) {
            cli.todos.push(format!(
                "`--{name}` is passed more than once; set what 11ty makes of it by hand"
            ));
            continue;
        }
        seen.push(name);
        match name.as_str() {
            "input" => {
                if !v.is_empty() && !glob(v) && root.join(v).is_dir() {
                    cli.input = Some(v.clone());
                } else {
                    cli.todos.push(format!(
                        "`--input={v}` isn't a folder in the site, and Brrrty builds a folder; set `dir.input` by hand"
                    ));
                }
            }
            "output" => {
                if !v.is_empty() && !glob(v) {
                    cli.output = Some(v.clone());
                } else {
                    cli.todos.push(format!("port `--output={v}` to `dir.output`"));
                }
            }
            "config" => cli.config = Some(v.clone()),
            "pathprefix" if v.is_empty() || v == "/" => {}
            "pathprefix" => cli.todos.push(format!(
                "`--pathprefix={v}` sets 11ty's `pathPrefix`, and Brrrty's is always `/` so far"
            )),
            "formats" => cli.todos.push(format!(
                "`--formats={v}` sets which template formats 11ty builds, and Brrrty builds 11ty's default ones (Liquid, Markdown, Nunjucks and HTML) so far"
            )),
            "to" if v == "fs" => {}
            "to" => cli.todos.push(format!(
                "`--to={v}` prints the pages instead of writing files"
            )),
            "dryrun" if v == "false" => {}
            "dryrun" => cli
                .todos
                .push("`--dryrun` writes no files".to_string()),
            "incremental" => cli.todos.push(format!(
                "`--incremental{}` builds only part of the site",
                if v.is_empty() {
                    String::new()
                } else {
                    format!("={v}")
                }
            )),
            _ => {}
        }
    }
    (cli.input.is_some() || cli.output.is_some() || cli.config.is_some() || !cli.todos.is_empty())
        .then_some(cli)
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

    #[test]
    fn reads_flags_as_minimist_does() {
        let a = |s: &str| parse(&arguments(s));
        assert_eq!(
            a("eleventy --input=src --output out --quiet --serve false"),
            [
                ("input".into(), Some("src".into())),
                ("output".into(), Some("out".into())),
                ("quiet".into(), Some("true".into())),
                ("serve".into(), Some("false".into())),
            ]
        );
        assert_eq!(
            a("npx @11ty/eleventy --pathprefix \"/a b/\" --config --quiet"),
            [
                ("pathprefix".into(), Some("/a b/".into())),
                ("config".into(), Some(String::new())),
                ("quiet".into(), Some("true".into())),
            ]
        );
        assert_eq!(
            a("eleventy -x src"),
            [("-x".into(), None), ("src".into(), None)]
        );
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
        assert!(c.todos[0].contains("--pathprefix=/blog/"));
        assert!(c.todos[1].contains("--formats=md"));
    }

    #[test]
    fn harmless_flags_and_a_root_prefix_are_nothing() {
        let t = site();
        let pkg = json!({ "scripts": { "build": "eleventy --quiet --pathprefix=/ --to=fs", "dev": "eleventy --serve --port 9000" }});
        assert_eq!(find(&pkg, t.path()), None);
        assert_eq!(
            find(&json!({ "scripts": { "test": "node --test" }}), t.path()),
            None
        );
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
        assert!(c.todos[0].contains("both run 11ty"), "{:?}", c.todos);
    }

    #[test]
    fn an_input_that_isnt_a_folder_is_a_todo() {
        let t = site();
        let pkg = json!({ "scripts": { "build": "eleventy --input=README.md" }});
        let c = find(&pkg, t.path()).unwrap();
        assert_eq!(c.input, None);
        assert!(c.todos[0].contains("isn't a folder"));
    }
}
