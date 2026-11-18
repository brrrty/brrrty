//! `brrrty init`: sets up an existing 11ty site to build with Brrrty.
//!
//! It adds Brrrty to `package.json` (a devDependency and the `build` and
//! `start` scripts) and writes `brrrty.config.rhai` from the 11ty config,
//! reading that without running it. Those two files, in the site's folder,
//! are all it ever writes, and it never overwrites a `brrrty.config.rhai`.

mod js;
mod json;
mod port;

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

pub use brrrty_config::CONFIG_FILE;

/// 11ty's config file names, in the order 11ty looks for them.
pub const ELEVENTY_CONFIGS: [&str; 4] = [
    ".eleventy.js",
    "eleventy.config.js",
    "eleventy.config.mjs",
    "eleventy.config.cjs",
];

const PACKAGE_JSON: &str = "package.json";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const GETTING_STARTED: &str = "https://brrrty.com/docs/getting-started/";

/// How `brrrty init` was asked to run.
#[derive(Debug, Clone)]
pub struct Options {
    /// The site's folder.
    pub dir: PathBuf,
    /// Write without asking.
    pub yes: bool,
    /// Show the changes and write nothing.
    pub dry_run: bool,
    /// Standard input is a terminal, so Brrrty can ask.
    pub interactive: bool,
}

/// Why `brrrty init` stopped without writing anything.
#[derive(Debug)]
pub enum Refusal {
    /// The folder isn't an 11ty site (exit status 2).
    NotEleventy(String),
    /// Something Brrrty won't touch, or can't read (exit status 1).
    Refused(String),
}

/// One file `brrrty init` writes.
#[derive(Debug)]
pub struct Change {
    /// The name shown, relative to the site's folder.
    pub name: &'static str,
    /// The file as it is, or `None` when it is new.
    pub old: Option<String>,
    pub new: String,
    /// Where the file is written: inside the site's folder, symlinks resolved.
    pub path: PathBuf,
}

/// What `brrrty init` would do.
#[derive(Debug)]
pub struct Plan {
    pub root: PathBuf,
    /// The 11ty config file read, if any.
    pub config: Option<String>,
    pub changes: Vec<Change>,
    /// `// TODO(brrrty)` markers in the new `brrrty.config.rhai`.
    pub todos: usize,
    /// JavaScript data files without a `.rhai` port, relative to the root.
    pub js_data: Vec<String>,
    /// Things left for a person to change in `package.json`.
    pub advice: Vec<String>,
    /// `build:11ty` is in the scripts after the change.
    pub has_build_11ty: bool,
    /// The output folder, as the new config sets it.
    pub output: String,
}

/// The most `brrrty init` reads of `package.json` or an 11ty config.
pub const MAX_READ: u64 = 1 << 20;

const BOM: &str = "\u{feff}";

/// Whether `dir` looks like an 11ty site: `package.json` depends on
/// `@11ty/eleventy`, or 11ty's config file is there. Reads only regular
/// files inside `dir`, and at most [`MAX_READ`] bytes, so it can't hang or
/// fill memory.
pub fn looks_like_eleventy(dir: &Path) -> bool {
    let Ok(root) = fs::canonicalize(dir) else {
        return false;
    };
    detect(&root).is_some()
}

/// The reason `root` (canonical) is an 11ty site.
fn detect(root: &Path) -> Option<String> {
    if let Some(name) = ELEVENTY_CONFIGS.iter().find(|n| present(root, n)) {
        return Some((*name).to_string());
    }
    let text = match read_inside(root, PACKAGE_JSON) {
        Ok(Some(Read::Text(t))) => t,
        _ => return None,
    };
    let v: serde_json::Value =
        serde_json::from_str(text.strip_prefix(BOM).unwrap_or(&text)).ok()?;
    ["dependencies", "devDependencies"]
        .iter()
        .any(|k| v.get(k).and_then(|d| d.get("@11ty/eleventy")).is_some())
        .then(|| "package.json".to_string())
}

/// Whether `name` is in `root` as a file or a symlink, without opening it.
fn present(root: &Path, name: &str) -> bool {
    fs::symlink_metadata(root.join(name)).is_ok_and(|m| m.is_file() || m.file_type().is_symlink())
}

/// Whether a bare `brrrty` here should suggest `brrrty init`. Never fails.
pub fn should_hint(dir: &Path) -> bool {
    looks_like_eleventy(dir) && fs::symlink_metadata(dir.join(CONFIG_FILE)).is_err()
}

/// A file read by [`read_inside`].
enum Read {
    Text(String),
    /// Not UTF-8: its text with replacement characters.
    Lossy(String),
    /// Larger than [`MAX_READ`].
    TooBig,
}

/// Where `name` in `root` really is: a regular file inside `root`, through a
/// symlink only when it stays inside. `Ok(None)` when there is nothing there.
fn resolve_inside(root: &Path, name: &str) -> Result<Option<PathBuf>, String> {
    let p = root.join(name);
    let meta = match fs::symlink_metadata(&p) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Brrrty can't look at {name}: {e}")),
    };
    let real = if meta.file_type().is_symlink() {
        let real = fs::canonicalize(&p)
            .map_err(|e| format!("{name} is a symlink Brrrty can't follow ({e})"))?;
        if !real.starts_with(root) {
            return Err(format!(
                "{name} is a symlink to {}, outside {}. Brrrty only reads and writes inside the site's folder",
                real.display(),
                root.display()
            ));
        }
        real
    } else {
        p
    };
    let meta = fs::metadata(&real).map_err(|e| format!("Brrrty can't look at {name}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{name} isn't a regular file"));
    }
    Ok(Some(real))
}

/// Read `name` in `root` with [`resolve_inside`]'s rules, at most [`MAX_READ`] bytes.
fn read_inside(root: &Path, name: &str) -> Result<Option<Read>, String> {
    use std::io::Read as _;
    let Some(path) = resolve_inside(root, name)? else {
        return Ok(None);
    };
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let f = options
        .open(&path)
        .map_err(|e| format!("Brrrty can't read {name}: {e}"))?;
    // Checked again on the open file, in case it changed in between.
    if !f.metadata().is_ok_and(|m| m.is_file()) {
        return Err(format!("{name} isn't a regular file"));
    }
    let mut bytes = Vec::new();
    f.take(MAX_READ + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Brrrty can't read {name}: {e}"))?;
    if bytes.len() as u64 > MAX_READ {
        return Ok(Some(Read::TooBig));
    }
    Ok(Some(match String::from_utf8(bytes) {
        Ok(s) => Read::Text(s),
        Err(e) => Read::Lossy(String::from_utf8_lossy(e.as_bytes()).into_owned()),
    }))
}

/// Where `name` in `root` is written: see [`resolve_inside`].
fn target(root: &Path, name: &str) -> Result<PathBuf, Refusal> {
    match resolve_inside(root, name) {
        Ok(Some(p)) => Ok(p),
        Ok(None) => Ok(root.join(name)),
        Err(why) => Err(Refusal::Refused(format!("{why}, so nothing was written."))),
    }
}

/// Work out what `brrrty init` would change in `dir`, writing nothing.
pub fn plan(dir: &Path) -> Result<Plan, Refusal> {
    let root = fs::canonicalize(dir).map_err(|e| {
        Refusal::NotEleventy(format!(
            "Brrrty can't open the folder {}: {e}",
            dir.display()
        ))
    })?;
    if !root.is_dir() {
        return Err(Refusal::NotEleventy(format!(
            "{} isn't a folder.",
            dir.display()
        )));
    }
    if fs::symlink_metadata(root.join(CONFIG_FILE)).is_ok() {
        return Err(Refusal::Refused(format!(
            "{CONFIG_FILE} already exists in {}, and `brrrty init` never overwrites it, so nothing was written. To start again from the 11ty config, move it out of the way and run `brrrty init` again.",
            root.display()
        )));
    }
    let package_path = target(&root, PACKAGE_JSON)?;
    let config_path = target(&root, CONFIG_FILE)?;
    if detect(&root).is_none() {
        return Err(Refusal::NotEleventy(format!(
            "{} doesn't look like an 11ty site: there is no package.json that depends on @11ty/eleventy, and no eleventy.config.js, eleventy.config.mjs, eleventy.config.cjs or .eleventy.js. Run `brrrty init` in your site's folder, or point --dir at it.",
            root.display()
        )));
    }

    // package.json
    let old_package = match read_inside(&root, PACKAGE_JSON) {
        Ok(None) => None,
        Ok(Some(Read::Text(s))) => Some(s),
        Ok(Some(Read::Lossy(_))) => {
            return Err(Refusal::Refused(
                "package.json isn't UTF-8, so nothing was written.".into(),
            ));
        }
        Ok(Some(Read::TooBig)) => {
            return Err(Refusal::Refused(format!(
                "package.json is larger than {} MiB, so Brrrty didn't read it and nothing was written.",
                MAX_READ >> 20
            )));
        }
        Err(why) => return Err(Refusal::Refused(format!("{why}, so nothing was written."))),
    };
    let edited = match &old_package {
        Some(src) => match src.strip_prefix(BOM) {
            Some(rest) => {
                let mut e = edit_package(rest)?;
                e.text.insert_str(0, BOM);
                e
            }
            None => edit_package(src)?,
        },
        None => Edited {
            text: new_package(),
            advice: Vec::new(),
            has_build_11ty: false,
        },
    };

    // brrrty.config.rhai: from the config 11ty reads, which the build script's `--config`
    // may name instead of the default. What 11ty is run with is read from the scripts as
    // they are before the edit above: after it, `build` is Brrrty's, which would make
    // every 11ty run that writes elsewhere (a GitLab Pages job's `--output=public`) look
    // like a comparison with it. The `11ty flags` hash is of the scripts after the edit,
    // as a build will read them.
    let parse =
        |t: &str| serde_json::from_str::<serde_json::Value>(t.strip_prefix(BOM).unwrap_or(t)).ok();
    let before_edit = old_package
        .as_deref()
        .and_then(parse)
        .unwrap_or_else(|| serde_json::json!({}));
    let mut cli = brrrty_config::eleventy_cli::find(&before_edit, &root);
    // TODOs name the scripts as they are after the edit, which keeps 11ty's `build` as
    // `build:11ty`.
    if let (Some(c), Some(after)) = (cli.as_mut(), parse(&edited.text)) {
        for name in ["build", "start"] {
            let kept = format!("{name}:11ty");
            let was = before_edit["scripts"].get(name);
            if was.is_some()
                && after["scripts"].get(&kept) == was
                && after["scripts"].get(name) != was
            {
                let rename = |s: &mut String| {
                    if s == name {
                        *s = kept.clone();
                    } else if let Some(rest) = s.strip_prefix(&format!("{name} → ")) {
                        *s = format!("{kept} → {rest}");
                    }
                };
                rename(&mut c.script);
                for t in &mut c.todos {
                    rename(&mut t.script);
                }
            }
        }
    }
    let cli = cli.as_ref();
    let flags_hash = parse(&edited.text)
        .and_then(|v| brrrty_config::eleventy_cli::find(&v, &root))
        .map(|c| c.fingerprint());
    let named = cli.and_then(|c| c.config.clone());
    let config: Option<String> = match &named {
        Some(n) if n.is_empty() => None,
        Some(n) => Some(brrrty_config::eleventy_cli::config_path(&root, n)),
        None => ELEVENTY_CONFIGS
            .iter()
            .find(|n| present(&root, n))
            .map(|n| (*n).to_string()),
    };
    let (mut rhai, todos, input, output) = match &config {
        Some(name) => {
            let outside = !brrrty_config::markers::is_inside(name);
            let read = if outside {
                Err(format!("{name} is outside the site's folder"))
            } else {
                read_inside(&root, name)
            };
            let p = match read {
                Ok(Some(Read::Text(src))) => {
                    let mut p =
                        port::port_with(src.strip_prefix(BOM).unwrap_or(&src), name, cli);
                    record_source(&mut p.text, name, src.as_bytes());
                    p
                }
                Ok(Some(Read::Lossy(src))) => port::unreadable(
                    &src,
                    name,
                    "it isn't UTF-8, so Brrrty couldn't read it",
                    cli,
                ),
                Ok(Some(Read::TooBig)) => port::unreadable(
                    "",
                    name,
                    &format!("it is larger than {} MiB, so Brrrty didn't read it", MAX_READ >> 20),
                    cli,
                ),
                Ok(None) => port::unreadable(
                    "",
                    name,
                    "package.json's build script names it with `--config`, but it doesn't exist, so 11ty stops",
                    cli,
                ),
                Err(why) if named.is_some() => port::unreadable(
                    "",
                    name,
                    &format!("package.json's build script names it with `--config`, and {why}"),
                    cli,
                ),
                Err(why) => {
                    return Err(Refusal::Refused(format!("{why}, so nothing was written.")));
                }
            };
            (p.text, p.todos, p.input, p.output)
        }
        None if cli.is_some() => {
            let p = port::no_config(cli);
            (p.text, p.todos, p.input, p.output)
        }
        None => (
            "// Written by `brrrty init`. This site has no 11ty config file, so\n// Brrrty uses 11ty's defaults, as 11ty does.\n// Reference: https://brrrty.com/docs/configuration/\n".to_string(),
            0,
            None,
            None,
        ),
    };
    if let Some(h) = &flags_hash {
        record_flags(&mut rhai, h);
    }

    let mut changes = Vec::new();
    // The new file first: it is the one that can't already exist.
    changes.push(Change {
        name: CONFIG_FILE,
        old: None,
        new: rhai,
        path: config_path,
    });
    if old_package.as_deref() != Some(edited.text.as_str()) {
        changes.push(Change {
            name: PACKAGE_JSON,
            old: old_package,
            new: edited.text,
            path: package_path,
        });
    }

    let js_data = match input {
        Some(i) => match fs::canonicalize(root.join(i)) {
            Ok(input) if input.starts_with(&root) => js_data_files(&root, &input),
            _ => Vec::new(),
        },
        None => js_data_files(&root, &root),
    };
    Ok(Plan {
        root,
        config,
        changes,
        todos,
        js_data,
        advice: edited.advice,
        has_build_11ty: edited.has_build_11ty,
        output: output.unwrap_or_else(|| "_site".into()),
    })
}

/// Add the `Ported from` line, with `bytes`' hash, after the header's comment lines: a
/// build warns when the 11ty config no longer has that hash.
fn record_source(text: &mut String, name: &str, bytes: &[u8]) {
    use brrrty_config::markers::{PORTED_FROM, source_hash};
    let line = format!("{PORTED_FROM}{name} {}\n", source_hash(bytes));
    let at = text.find("\n\n").map_or(text.len(), |i| i + 1);
    text.insert_str(at, &line);
}

/// Add the line recording what the scripts pass to 11ty: a build warns when they change.
fn record_flags(text: &mut String, hash: &str) {
    use brrrty_config::eleventy_cli::FLAGS_LINE;
    let line = format!("{FLAGS_LINE}{hash}\n");
    let at = text.find("\n\n").map_or(text.len(), |i| i + 1);
    text.insert_str(at, &line);
}

struct Edited {
    text: String,
    advice: Vec<String>,
    has_build_11ty: bool,
}

fn new_package() -> String {
    format!(
        "{{\n  \"private\": true,\n  \"scripts\": {{\n    \"build\": \"brrrty build\",\n    \"start\": \"brrrty serve\"\n  }},\n  \"devDependencies\": {{\n    \"brrrty\": \"{VERSION}\"\n  }}\n}}\n"
    )
}

/// `text` with control characters escaped, so a hostile `package.json` or
/// config can't break a line or send escape codes to the terminal.
fn shown(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let escaped: Vec<char> = if c.is_control() {
                c.escape_default().collect()
            } else {
                vec![c]
            };
            escaped
        })
        .collect()
}

/// [`shown`], keeping line breaks and tabs: for the diff.
fn shown_lines(text: &str) -> String {
    text.split('\n')
        .map(|l| {
            l.split('\t')
                .map(|p| shown(p.strip_suffix('\r').unwrap_or(p)))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a script is exactly one 11ty command: `eleventy`,
/// `npx @11ty/eleventy` or `node_modules/.bin/eleventy`, after any
/// `NAME=value` settings (or `cross-env` ones), with its own flags.
fn is_eleventy_command(cmd: &str) -> bool {
    if cmd.contains(['&', '|', ';', '`', '>', '<', '(', ')', '\n', '\r']) || cmd.contains("$(") {
        return false;
    }
    let mut words = cmd.split_whitespace().peekable();
    let assignment = |w: &str| {
        w.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    };
    while words.peek().is_some_and(|w| assignment(w)) {
        words.next();
    }
    if words.peek() == Some(&"cross-env") {
        words.next();
        while words.peek().is_some_and(|w| assignment(w)) {
            words.next();
        }
    }
    match words.next() {
        Some("eleventy" | "node_modules/.bin/eleventy" | "./node_modules/.bin/eleventy") => true,
        Some("npx") => {
            let mut w = words.next();
            if matches!(w, Some("-y" | "--yes")) {
                w = words.next();
            }
            w.is_some_and(|w| {
                w == "eleventy" || w == "@11ty/eleventy" || w.starts_with("@11ty/eleventy@")
            })
        }
        _ => false,
    }
}

/// The step of a compound script (`a && b`, `a; b`, `a | b`) that is an
/// 11ty command, if one is.
fn eleventy_step(cmd: &str) -> Option<&str> {
    cmd.split("&&")
        .flat_map(|p| p.split("||"))
        .flat_map(|p| p.split(';'))
        .flat_map(|p| p.split('|'))
        .map(str::trim)
        .find(|step| is_eleventy_command(step))
}

fn edit_package(src: &str) -> Result<Edited, Refusal> {
    let bad =
        |why: String| Refusal::Refused(format!("package.json {why}, so nothing was written."));
    let value: serde_json::Value =
        serde_json::from_str(src).map_err(|e| bad(format!("isn't valid JSON ({e})")))?;
    if !value.is_object() {
        return Err(bad("isn't a JSON object".into()));
    }
    let top = json::object_at(src, 0).ok_or_else(|| bad("couldn't be read".into()))?;
    let style = json::Style::of(src, &top);
    let mut edits = Vec::new();
    let mut advice = Vec::new();
    let mut top_append: Vec<(String, String)> = Vec::new();
    // What the file should hold afterwards, to check the edit against.
    let mut expect = value.clone();
    let set = |expect: &mut serde_json::Value, object: &str, key: &str, v: &str| {
        if let Some(o) = expect.as_object_mut() {
            let entry = o
                .entry(object.to_string())
                .or_insert_with(|| serde_json::Value::Object(Default::default()));
            if let Some(inner) = entry.as_object_mut() {
                inner.insert(key.to_string(), serde_json::Value::String(v.to_string()));
            }
        }
    };

    // Scripts.
    let wanted = [("build", "brrrty build"), ("start", "brrrty serve")];
    match top.get("scripts") {
        None => {
            top_append.push(("scripts".into(), json::new_object(1, &style, &wanted)));
            for (name, cmd) in wanted {
                set(&mut expect, "scripts", name, cmd);
            }
        }
        Some(m) => match json::object_at(src, m.value_start) {
            None => advice.push(
                "`scripts` in package.json isn't an object, so Brrrty left it alone. Add \"build\": \"brrrty build\" and \"start\": \"brrrty serve\" to it.".into(),
            ),
            Some(scripts) => {
                let string = |k: &str| {
                    value["scripts"]
                        .get(k)
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                };
                let mut append = Vec::new();
                for (name, cmd) in wanted {
                    let quoted = serde_json::to_string(cmd).unwrap_or_default();
                    let Some(member) = scripts.get(name) else {
                        append.push((name.to_string(), quoted));
                        set(&mut expect, "scripts", name, cmd);
                        continue;
                    };
                    let current = string(name);
                    if current.as_deref() == Some(cmd) {
                        continue;
                    }
                    let keep = format!("{name}:11ty");
                    match current {
                        Some(old) if is_eleventy_command(&old) => {
                            if scripts.get(&keep).is_some() {
                                advice.push(format!(
                                    "scripts.{name} runs 11ty, and scripts.{keep} is taken, so Brrrty left both alone. Change scripts.{name} to \"{cmd}\" when you're ready."
                                ));
                                continue;
                            }
                            // Rename it to `name:11ty`, with the new script in front.
                            let i = scripts
                                .members
                                .iter()
                                .rposition(|m| m.key == name)
                                .unwrap_or(0);
                            let prev_end = if i == 0 {
                                scripts.open + 1
                            } else {
                                scripts.members[i - 1].value_end
                            };
                            let mut e = json::insert_before(src, prev_end, member, name, &quoted);
                            e.end = member.key_end;
                            e.text.push_str(&serde_json::to_string(&keep).unwrap_or_default());
                            edits.push(e);
                            set(&mut expect, "scripts", name, cmd);
                            set(&mut expect, "scripts", &keep, &old);
                        }
                        Some(old) => match eleventy_step(&old) {
                            Some(step) => advice.push(format!(
                                "scripts.{name} runs 11ty as one step of \"{}\", so Brrrty left it alone. To use Brrrty there, copy it to scripts.{keep} (to compare with 11ty), then replace `{}` in scripts.{name} with `{cmd}`.", shown(&old), shown(step)
                            )),
                            None => advice.push(format!(
                                "scripts.{name} runs \"{}\", which isn't an 11ty command, so Brrrty left it alone. Where it runs 11ty, run `{cmd}` instead.", shown(&old)
                            )),
                        },
                        None => advice.push(format!(
                            "scripts.{name} isn't a string, so Brrrty left it alone. Set it to \"{cmd}\"."
                        )),
                    }
                }
                if !append.is_empty() {
                    edits.push(json::append(src, &scripts, 1, &style, &append));
                }
                // Other scripts that run 11ty.
                if let Some(all) = value["scripts"].as_object() {
                    for (k, v) in all {
                        let Some(cmd) = v.as_str() else { continue };
                        if ["build", "start", "build:11ty", "start:11ty"].contains(&k.as_str()) {
                            continue;
                        }
                        if is_eleventy_command(cmd) || eleventy_step(cmd).is_some() {
                            advice.push(format!(
                                "scripts.{} runs 11ty (\"{}\"). Brrrty's commands are `brrrty build` and `brrrty serve`, if you want it to run Brrrty.", shown(k), shown(cmd)
                            ));
                        }
                    }
                }
            }
        },
    }

    // The devDependency.
    let listed = ["dependencies", "devDependencies"]
        .iter()
        .find_map(|k| value.get(k).and_then(|d| d.get("brrrty")).map(|v| (*k, v)));
    match listed {
        Some((k, v)) => {
            if v.as_str() != Some(VERSION) {
                advice.push(format!(
                    "{k} already has brrrty at {}, so Brrrty left it alone. This is brrrty {VERSION}.", shown(&v.to_string())
                ));
            }
        }
        None => {
            let dep = serde_json::to_string(VERSION).unwrap_or_default();
            match top
                .get("devDependencies")
                .and_then(|m| json::object_at(src, m.value_start))
            {
                Some(dev) => {
                    set(&mut expect, "devDependencies", "brrrty", VERSION);
                    // In order, as npm keeps them.
                    match dev.members.iter().position(|m| m.key.as_str() > "brrrty") {
                        Some(i) => {
                            let prev_end = if i == 0 {
                                dev.open + 1
                            } else {
                                dev.members[i - 1].value_end
                            };
                            edits.push(json::insert_before(
                                src,
                                prev_end,
                                &dev.members[i],
                                "brrrty",
                                &dep,
                            ));
                        }
                        None => edits.push(json::append(
                            src,
                            &dev,
                            1,
                            &style,
                            &[("brrrty".into(), dep)],
                        )),
                    }
                }
                None if top.get("devDependencies").is_some() => advice.push(format!(
                    "`devDependencies` in package.json isn't an object, so Brrrty left it alone. Add \"brrrty\": \"{VERSION}\" to it."
                )),
                None => {
                    set(&mut expect, "devDependencies", "brrrty", VERSION);
                    top_append.push((
                        "devDependencies".into(),
                        json::new_object(1, &style, &[("brrrty", VERSION)]),
                    ));
                }
            }
        }
    }
    if !top_append.is_empty() {
        edits.push(json::append(src, &top, 0, &style, &top_append));
    }
    let text = json::apply(src, edits);
    // The edit must leave exactly the original plus the intended changes.
    let check: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| bad(format!("couldn't be edited safely ({e})")))?;
    if check != expect {
        return Err(bad("couldn't be edited safely".into()));
    }
    let has_build_11ty = check["scripts"]
        .get("build:11ty")
        .and_then(|v| v.as_str())
        .is_some_and(is_eleventy_command);
    Ok(Edited {
        text,
        advice,
        has_build_11ty,
    })
}

/// JavaScript data files without a `.rhai` next to them: in `_data`, and
/// `*.11tydata.js` files anywhere in the input folder.
fn js_data_files(root: &Path, input: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut budget = 100_000usize;
    walk(input, 0, &mut budget, &mut |p| {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let Some((stem, ext)) = name.rsplit_once('.') else {
            return;
        };
        if !matches!(ext, "js" | "cjs" | "mjs") {
            return;
        }
        let in_data = p.starts_with(input.join("_data"));
        if !in_data && !stem.ends_with(".11tydata") {
            return;
        }
        if p.with_file_name(format!("{stem}.rhai")).exists() {
            return;
        }
        let shown = p.strip_prefix(root).unwrap_or(p);
        found.push(shown.to_string_lossy().replace('\\', "/"));
    });
    found.sort();
    found
}

/// Visit the files under `dir` without following symlinks, giving up after
/// `budget` entries.
fn walk(dir: &Path, depth: usize, budget: &mut usize, f: &mut dyn FnMut(&Path)) {
    if depth > 32 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let name = e.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            if name.starts_with('.') || name == "node_modules" || name == "_site" {
                continue;
            }
            walk(&e.path(), depth + 1, budget, f);
        } else if kind.is_file() {
            f(&e.path());
        }
    }
}

/// The changes as a unified diff.
pub fn diff(plan: &Plan) -> String {
    let mut out = String::new();
    for c in &plan.changes {
        let old = c.old.as_deref().unwrap_or("");
        let a = if c.old.is_some() {
            format!("a/{}", c.name)
        } else {
            "/dev/null".into()
        };
        let b = format!("b/{}", c.name);
        let d = similar::TextDiff::from_lines(old, &c.new);
        out.push_str(
            &d.unified_diff()
                .context_radius(3)
                .header(&a, &b)
                .to_string(),
        );
    }
    out
}

/// Write the changes, each atomically.
///
/// A new file is linked into place, so one that appeared since the plan is
/// never overwritten; a changed file is written only if it still holds what
/// the plan read. The new `brrrty.config.rhai` goes first.
pub fn apply(plan: &Plan) -> io::Result<()> {
    for c in &plan.changes {
        if let Some(old) = &c.old {
            let now = fs::read(&c.path)?;
            if now != old.as_bytes() {
                return Err(io::Error::other(format!(
                    "{} changed while Brrrty was working, so it wasn't overwritten",
                    c.name
                )));
            }
        }
        write_atomic(&c.path, &c.new, c.old.is_none()).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                io::Error::new(
                    e.kind(),
                    format!(
                        "{} appeared while Brrrty was working, so it wasn't overwritten",
                        c.name
                    ),
                )
            } else {
                e
            }
        })?;
    }
    Ok(())
}

/// Write `path` through a temporary file. With `new`, fails rather than
/// replace a file already there. Replacing breaks a hard link to the old file.
fn write_atomic(path: &Path, text: &str, new: bool) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{name}.brrrty-{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        if let Ok(m) = fs::metadata(path) {
            f.set_permissions(m.permissions())?;
        }
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        if new {
            match fs::hard_link(&tmp, path) {
                Ok(()) => fs::remove_file(&tmp),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
                // A file system without hard links: check, then rename.
                Err(_) if fs::symlink_metadata(path).is_ok() => {
                    Err(io::Error::from(io::ErrorKind::AlreadyExists))
                }
                Err(_) => fs::rename(&tmp, path),
            }
        } else {
            fs::rename(&tmp, path)
        }
    })();
    if result.is_err() || new {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// What's left to do, and the next steps.
pub fn summary(plan: &Plan) -> String {
    let mut out = String::new();
    let mut todo = Vec::new();
    if plan.todos > 0 {
        let source = plan.config.as_deref().unwrap_or("the 11ty config");
        todo.push(format!(
            "{CONFIG_FILE} has {} TODO(brrrty) marker{}: parts of {source} that Brrrty can't port by itself. Each quotes the JavaScript it stands in for. Brrrty builds no pages while one is left: port each one, or decide it changes nothing Brrrty writes, then delete its TODO line. `brrrty doctor` lists them.",
            plan.todos,
            if plan.todos == 1 { "" } else { "s" }
        ));
    }
    if !plan.js_data.is_empty() {
        let mut s = format!(
            "Port these JavaScript data files to .rhai files next to them ({GETTING_STARTED}#2-port-javascript-data-files):"
        );
        for f in &plan.js_data {
            s.push_str(&format!("\n      {f}"));
        }
        todo.push(s);
    }
    todo.extend(plan.advice.iter().cloned());
    if !todo.is_empty() {
        out.push_str("\nTo do by hand:\n");
        for t in todo {
            out.push_str(&format!("  - {t}\n"));
        }
    }
    out.push_str("\nNext steps:\n  npm install\n  npm run build\n");
    if plan.has_build_11ty {
        out.push_str(&format!(
            "  npm run build:11ty -- --output=_site-11ty\n  diff -r _site-11ty {}\n",
            plan.output
        ));
    } else {
        out.push_str(&format!(
            "  npx @11ty/eleventy --output=_site-11ty\n  diff -r _site-11ty {}\n",
            plan.output
        ));
    }
    out.push_str(&format!(
        "The last two build the site with 11ty and compare. See {GETTING_STARTED}#5-check-the-output-against-11ty\n"
    ));
    out
}

/// Run `brrrty init`; returns the exit status.
pub fn run(opts: &Options) -> u8 {
    let plan = match plan(&opts.dir) {
        Ok(p) => p,
        Err(Refusal::NotEleventy(why)) => {
            crate::say_err!("{why}");
            return 2;
        }
        Err(Refusal::Refused(why)) => {
            crate::say_err!("{why}");
            return 1;
        }
    };
    let from = plan.config.as_deref().unwrap_or("package.json");
    crate::say!(
        "Brrrty found an 11ty site in {} ({from}).\n",
        plan.root.display()
    );
    crate::say!("{}", shown_lines(diff(&plan).trim_end()));
    let write = if opts.dry_run {
        crate::say!("\nDry run: nothing was written.");
        false
    } else if opts.yes {
        true
    } else if !opts.interactive {
        crate::say!(
            "\nNothing was written: there is no terminal to ask in. Run `brrrty init --yes` to make these changes."
        );
        false
    } else {
        print!("\nMake these changes? [y/N] ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        let _ = io::stdin().lock().read_line(&mut line);
        let yes = matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes");
        if !yes {
            crate::say!("Nothing was written.");
            return 0;
        }
        true
    };
    if write {
        if let Err(e) = apply(&plan) {
            crate::say_err!("Brrrty couldn't write the changes: {e}");
            return 1;
        }
        let names: Vec<&str> = plan.changes.iter().map(|c| c.name).collect();
        crate::say!("\nWrote {}.", names.join(" and "));
    }
    crate::say!("{}", shown_lines(summary(&plan).trim_end()));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_scripts_that_are_11ty() {
        for yes in [
            "eleventy",
            "npx @11ty/eleventy --serve --port 8080",
            "npx -y @11ty/eleventy@3.1.2",
            "npx eleventy --input=src",
            "NODE_ENV=production eleventy",
            "cross-env ELEVENTY_ENV=prod eleventy --quiet",
            "./node_modules/.bin/eleventy",
        ] {
            assert!(is_eleventy_command(yes), "{yes}");
        }
        for no in [
            "vite build",
            "npm-run-all build:*",
            "eleventy-dev-server",
            "npm run css && eleventy && npm run pagefind",
            "echo building with eleventy",
            "node --run eleventy",
            "rm -rf _site; eleventy",
            "eleventy > log.txt",
        ] {
            assert!(!is_eleventy_command(no), "{no}");
        }
        assert_eq!(
            eleventy_step("npm run css && eleventy --quiet && npm run pagefind"),
            Some("eleventy --quiet")
        );
        assert_eq!(eleventy_step("echo building with eleventy"), None);
    }

    #[test]
    fn edits_scripts_and_dev_dependencies() {
        let src = "{\n  \"name\": \"site\",\n  \"scripts\": {\n    \"build\": \"eleventy\",\n    \"start\": \"vite\"\n  },\n  \"devDependencies\": {\n    \"@11ty/eleventy\": \"^3.1.2\",\n    \"prettier\": \"3\"\n  }\n}\n";
        let e = edit_package(src).unwrap();
        assert_eq!(
            e.text,
            format!(
                "{{\n  \"name\": \"site\",\n  \"scripts\": {{\n    \"build\": \"brrrty build\",\n    \"build:11ty\": \"eleventy\",\n    \"start\": \"vite\"\n  }},\n  \"devDependencies\": {{\n    \"@11ty/eleventy\": \"^3.1.2\",\n    \"brrrty\": \"{VERSION}\",\n    \"prettier\": \"3\"\n  }}\n}}\n"
            )
        );
        assert!(e.has_build_11ty);
        assert_eq!(e.advice.len(), 1, "{:?}", e.advice);
        assert!(e.advice[0].contains("scripts.start runs \"vite\""));
    }

    #[test]
    fn adds_missing_objects() {
        let src =
            "{\n    \"name\": \"site\",\n    \"dependencies\": { \"@11ty/eleventy\": \"3\" }\n}";
        let e = edit_package(src).unwrap();
        assert_eq!(
            e.text,
            format!(
                "{{\n    \"name\": \"site\",\n    \"dependencies\": {{ \"@11ty/eleventy\": \"3\" }},\n    \"scripts\": {{\n        \"build\": \"brrrty build\",\n        \"start\": \"brrrty serve\"\n    }},\n    \"devDependencies\": {{\n        \"brrrty\": \"{VERSION}\"\n    }}\n}}"
            )
        );
    }

    #[test]
    fn leaves_a_package_already_set_up_alone() {
        let src = format!(
            "{{\"scripts\":{{\"build\":\"brrrty build\",\"start\":\"brrrty serve\"}},\"devDependencies\":{{\"brrrty\":\"{VERSION}\"}}}}"
        );
        let e = edit_package(&src).unwrap();
        assert_eq!(e.text, src);
        assert!(e.advice.is_empty());
    }
}
