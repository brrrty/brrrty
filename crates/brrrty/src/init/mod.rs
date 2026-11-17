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
    pub config: Option<&'static str>,
    pub changes: Vec<Change>,
    /// `// TODO:` blocks in the new `brrrty.config.rhai`.
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

/// Whether `dir` looks like an 11ty site: `package.json` depends on
/// `@11ty/eleventy`, or 11ty's config file is there.
pub fn looks_like_eleventy(dir: &Path) -> bool {
    detect(dir).is_some()
}

/// The reason `dir` is an 11ty site.
fn detect(dir: &Path) -> Option<String> {
    if let Some(name) = ELEVENTY_CONFIGS.iter().find(|n| dir.join(n).is_file()) {
        return Some((*name).to_string());
    }
    let text = fs::read_to_string(dir.join(PACKAGE_JSON)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    ["dependencies", "devDependencies"]
        .iter()
        .any(|k| v.get(k).and_then(|d| d.get("@11ty/eleventy")).is_some())
        .then(|| "package.json".to_string())
}

/// Whether a bare `brrrty` here should suggest `brrrty init`.
pub fn should_hint(dir: &Path) -> bool {
    looks_like_eleventy(dir) && fs::symlink_metadata(dir.join(CONFIG_FILE)).is_err()
}

/// Where `name` in `root` is written. A symlink is followed only when it
/// stays inside `root`.
fn target(root: &Path, name: &str) -> Result<PathBuf, Refusal> {
    let p = root.join(name);
    match fs::symlink_metadata(&p) {
        Ok(m) if m.file_type().is_symlink() => {
            let real = fs::canonicalize(&p).map_err(|e| {
                Refusal::Refused(format!(
                    "{name} is a symlink Brrrty can't follow ({e}), so nothing was written."
                ))
            })?;
            if !real.starts_with(root) {
                return Err(Refusal::Refused(format!(
                    "{name} is a symlink to {}, outside {}. Brrrty only writes inside the site's folder, so nothing was written.",
                    real.display(),
                    root.display()
                )));
            }
            if !real.is_file() {
                return Err(Refusal::Refused(format!(
                    "{name} is a symlink to {}, which isn't a file, so nothing was written.",
                    real.display()
                )));
            }
            Ok(real)
        }
        Ok(m) if !m.is_file() => Err(Refusal::Refused(format!(
            "{name} isn't a file, so nothing was written."
        ))),
        _ => Ok(p),
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
    if detect(&root).is_none() {
        return Err(Refusal::NotEleventy(format!(
            "{} doesn't look like an 11ty site: there is no package.json that depends on @11ty/eleventy, and no eleventy.config.js, eleventy.config.mjs, eleventy.config.cjs or .eleventy.js. Run `brrrty init` in your site's folder, or point --dir at it.",
            root.display()
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

    // brrrty.config.rhai
    let config = ELEVENTY_CONFIGS
        .iter()
        .copied()
        .find(|n| root.join(n).is_file());
    let (rhai, todos, input, output) = match config {
        Some(name) => {
            let src = fs::read_to_string(root.join(name))
                .map_err(|e| Refusal::Refused(format!("Brrrty can't read {name}: {e}")))?;
            let p = port::port(&src, name);
            (p.text, p.todos, p.input, p.output)
        }
        None => (
            "// Written by `brrrty init`. This site has no 11ty config file, so\n// Brrrty uses 11ty's defaults, as 11ty does.\n// Reference: https://brrrty.com/docs/configuration/\n".to_string(),
            0,
            None,
            None,
        ),
    };

    // package.json
    let old_package = match fs::read_to_string(&package_path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(Refusal::Refused(format!(
                "Brrrty can't read package.json: {e}"
            )));
        }
    };
    let edited = match &old_package {
        Some(src) => edit_package(src)?,
        None => Edited {
            text: new_package(),
            advice: Vec::new(),
            has_build_11ty: false,
        },
    };

    let mut changes = Vec::new();
    if old_package.as_deref() != Some(edited.text.as_str()) {
        changes.push(Change {
            name: PACKAGE_JSON,
            old: old_package,
            new: edited.text,
            path: package_path,
        });
    }
    changes.push(Change {
        name: CONFIG_FILE,
        old: None,
        new: rhai,
        path: config_path,
    });

    let input = root.join(input.unwrap_or_else(|| ".".into()));
    let js_data = js_data_files(&root, &input);
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

/// Whether a script runs 11ty.
fn runs_eleventy(cmd: &str) -> bool {
    cmd.split(|c: char| c.is_whitespace() || ";&|()".contains(c))
        .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
        .any(|w| {
            w == "eleventy"
                || w.ends_with("/eleventy")
                || w.starts_with("@11ty/eleventy@")
                || w.contains("@11ty/eleventy/")
        })
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

    // Scripts.
    let wanted = [("build", "brrrty build"), ("start", "brrrty serve")];
    let mut has_build_11ty = false;
    match top.get("scripts") {
        None => top_append.push(("scripts".into(), json::new_object(1, &style, &wanted))),
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
                has_build_11ty = scripts.get("build:11ty").is_some();
                let mut append = Vec::new();
                for (name, cmd) in wanted {
                    let quoted = serde_json::to_string(cmd).unwrap_or_default();
                    let Some(member) = scripts.get(name) else {
                        append.push((name.to_string(), quoted));
                        continue;
                    };
                    let current = string(name);
                    if current.as_deref() == Some(cmd) {
                        continue;
                    }
                    let keep = format!("{name}:11ty");
                    match current {
                        Some(old) if runs_eleventy(&old) => {
                            if scripts.get(&keep).is_some() {
                                advice.push(format!(
                                    "scripts.{name} runs 11ty, and scripts.{keep} is taken, so Brrrty left both alone. Change scripts.{name} to \"{cmd}\" when you're ready."
                                ));
                                continue;
                            }
                            if name == "build" {
                                has_build_11ty = true;
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
                        }
                        Some(old) => advice.push(format!(
                            "scripts.{name} runs \"{old}\", which isn't 11ty itself, so Brrrty left it alone. Where it runs 11ty, run \"{cmd}\" instead."
                        )),
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
                        if runs_eleventy(cmd) {
                            advice.push(format!(
                                "scripts.{k} runs 11ty (\"{cmd}\"). Brrrty's commands are `brrrty build` and `brrrty serve`, if you want it to run Brrrty."
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
                    "{k} already has brrrty at {v}, so Brrrty left it alone. This is brrrty {VERSION}."
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
                None => top_append.push((
                    "devDependencies".into(),
                    json::new_object(1, &style, &[("brrrty", VERSION)]),
                )),
            }
        }
    }
    if !top_append.is_empty() {
        edits.push(json::append(src, &top, 0, &style, &top_append));
    }
    let text = json::apply(src, edits);
    // The edit must leave valid JSON with what was asked for in it.
    let check: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| bad(format!("couldn't be edited safely ({e})")))?;
    let mut expect = value;
    if let Some(o) = expect.as_object_mut() {
        o.remove("scripts");
        o.remove("devDependencies");
    }
    let mut got = check.clone();
    if let Some(o) = got.as_object_mut() {
        o.remove("scripts");
        o.remove("devDependencies");
    }
    if expect != got {
        return Err(bad("couldn't be edited safely".into()));
    }
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
    walk(input, 0, &mut |p| {
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

fn walk(dir: &Path, depth: usize, f: &mut dyn FnMut(&Path)) {
    if depth > 32 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            if name.starts_with('.') || name == "node_modules" || name == "_site" {
                continue;
            }
            walk(&e.path(), depth + 1, f);
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
pub fn apply(plan: &Plan) -> io::Result<()> {
    for c in &plan.changes {
        // Checked again: something may have made it since the plan.
        if c.old.is_none() && fs::symlink_metadata(&c.path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "{} appeared while Brrrty was working, so it wasn't overwritten",
                    c.name
                ),
            ));
        }
        write_atomic(&c.path, &c.new)?;
    }
    Ok(())
}

fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
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
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// What's left to do, and the next steps.
pub fn summary(plan: &Plan) -> String {
    let mut out = String::new();
    let mut todo = Vec::new();
    if plan.todos > 0 {
        let source = plan.config.unwrap_or("the 11ty config");
        todo.push(format!(
            "{CONFIG_FILE} has {} TODO{}: parts of {source} that Brrrty can't port by itself. Each quotes the JavaScript it stands in for.",
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
    let from = plan.config.unwrap_or("package.json");
    crate::say!(
        "Brrrty found an 11ty site in {} ({from}).\n",
        plan.root.display()
    );
    crate::say!("{}", diff(&plan).trim_end());
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
    crate::say!("{}", summary(&plan).trim_end());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_scripts_that_run_eleventy() {
        for yes in [
            "eleventy",
            "npx @11ty/eleventy --serve",
            "rm -rf _site && eleventy",
            "npx @11ty/eleventy@3.1.2",
            "node node_modules/@11ty/eleventy/cmd.cjs",
            "cross-env NODE_ENV=production eleventy",
        ] {
            assert!(runs_eleventy(yes), "{yes}");
        }
        for no in ["vite build", "npm-run-all build:*", "eleventy-dev-server"] {
            assert!(!runs_eleventy(no), "{no}");
        }
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
