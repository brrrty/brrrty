//! Directory and template data files: every data file that applies to a
//! template, read once and shared by the pages under it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use brrrty_js::Value;

use super::cascade::{clean_tags, frozen_keys};
use super::global::read_json;
use super::js::{JsLoad, JsSession, first_inexact};
use super::jskeys::{export_keys, module_kind};
use super::rhai;
use super::{
    BUILD_KEYS, OVERRIDE_PREFIX, RESERVED, json_text, local_data_files, merge, port_of,
    reserved_keys, reserved_problem,
};
use crate::frontmatter::Data;
use crate::options::DataRunner;
use crate::problem::{Problem, codes};

/// One data file, read.
#[derive(Debug)]
pub(crate) enum LocalFile {
    /// A JSON file's object, its `tags` cleaned. `None` when the file
    /// holds something else, which 11ty skips.
    Json(Option<Data>),
    /// A JavaScript file Brrrty can't run, and the top-level keys it sets.
    Js(Vec<String>),
    /// A JavaScript file that ran: its object, `tags` cleaned, and the
    /// top-level keys whose value has a part that can't cross (where, and
    /// what it is). `None` when it gave something other than an object,
    /// which 11ty skips.
    JsRan(Option<Data>, Vec<JsInexact>),
    /// A file no page under it can be built with, and why.
    Failed(Problem),
}

/// A top-level key of a JavaScript data file whose value has a part that
/// can't cross exactly.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JsInexact {
    pub key: String,
    pub file: PathBuf,
    pub at: Vec<brrrty_js::runtime::PathPart>,
    pub what: String,
}

/// The data files that apply to a template, merged.
#[derive(Debug, Default)]
pub(crate) struct Local {
    pub data: Data,
    /// Keys set by JavaScript files, each with the file that sets it (the
    /// highest-priority one when several do).
    pub js: Vec<(String, PathBuf)>,
    /// Keys set by JavaScript files that ran, whose value can't cross.
    pub js_values: Vec<JsInexact>,
    /// For each top-level key, the highest-priority file that sets it.
    pub source: HashMap<String, PathBuf>,
}

/// Every directory and template data file, by its root-relative path.
#[derive(Debug, Default)]
pub(crate) struct DirDataIndex {
    files: HashMap<PathBuf, LocalFile>,
    /// Each template's data files, lowest priority first.
    pages: HashMap<PathBuf, Vec<PathBuf>>,
    /// Notes, such as a JavaScript file a `.rhai` port stands in for.
    pub notes: Vec<Problem>,
    noted: Vec<PathBuf>,
    /// Warnings, such as a JavaScript file that gave something other than
    /// an object.
    pub warnings: Vec<Problem>,
}

impl DirDataIndex {
    /// Read the data files that apply to `templates` (relative to the
    /// input folder).
    ///
    /// `.rhai` files run once each, on `runner`, with `global` as the data
    /// 11ty hands an exported function. A JavaScript file with a `.rhai`
    /// port beside it is dropped, with a note. Otherwise, with JavaScript
    /// data on, it runs in `js` once for every template it applies to, as
    /// 11ty runs it; every result must be the same.
    pub fn load(
        input: &Path,
        input_rel: &Path,
        templates: &[PathBuf],
        runner: Option<&DataRunner>,
        global: &Data,
        js: &mut JsSession<'_>,
    ) -> DirDataIndex {
        let mut index = DirDataIndex::default();
        // Each JavaScript file's result for each template, in template order.
        let mut runs: Vec<(PathBuf, Vec<(PathBuf, JsLoad)>)> = Vec::new();
        let mut ported_local: Vec<PathBuf> = Vec::new();
        for rel in templates {
            // 11ty lists the paths highest priority first, drops repeats
            // (`posts/posts.md` names `posts/posts.json` twice), then
            // reverses the list.
            let mut paths: Vec<PathBuf> = Vec::new();
            for p in local_data_files(input, input_rel, rel) {
                if !paths.contains(&p) {
                    paths.push(p);
                }
            }
            paths.reverse();
            let ported: Vec<PathBuf> = paths
                .iter()
                .filter(|p| rhai::is_js(&p.to_string_lossy()) && paths.contains(&port_of(p)))
                .cloned()
                .collect();
            for p in &ported {
                if !index.noted.contains(p) {
                    index.noted.push(p.clone());
                    index.notes.push(rhai::shadowed(p));
                    ported_local.push(p.clone());
                }
            }
            paths.retain(|p| !ported.contains(p));
            for shown in &paths {
                let full = input.join(shown.strip_prefix(input_rel).unwrap_or(shown));
                if rhai::is_js(&shown.to_string_lossy()) && js.on() {
                    let r = js.load(&full, shown, true);
                    let template = input_rel.join(rel);
                    match runs.iter_mut().find(|(f, _)| f == shown) {
                        Some((_, list)) => list.push((template, r)),
                        None => runs.push((shown.clone(), vec![(template, r)])),
                    }
                    continue;
                }
                if rhai::is_js(&shown.to_string_lossy()) {
                    js.record(shown, super::js::JsFileState::NotRun);
                }
                if !index.files.contains_key(shown) {
                    index
                        .files
                        .insert(shown.clone(), read_local(&full, shown, runner, global));
                }
            }
            index.pages.insert(rel.clone(), paths);
        }
        for shown in ported_local {
            let full = input.join(shown.strip_prefix(input_rel).unwrap_or(&shown));
            let port = match index.files.get(&port_of(&shown)) {
                Some(LocalFile::Json(Some(d))) => {
                    Some(Value::Object(std::sync::Arc::new(d.clone())))
                }
                _ => None,
            };
            js.check_port(&full, &shown, port.as_ref());
        }
        for (shown, results) in runs {
            let full = input.join(shown.strip_prefix(input_rel).unwrap_or(&shown));
            let (file, warning) = settle_js(&full, &shown, results);
            index.warnings.extend(warning);
            index.files.insert(shown, file);
        }
        index
    }

    /// Whether any page has a JavaScript data file Brrrty didn't run.
    pub fn has_js(&self) -> bool {
        self.files.values().any(|f| matches!(f, LocalFile::Js(_)))
            || self.files.iter().any(|(p, f)| {
                matches!(f, LocalFile::Failed(_)) && rhai::is_js(&p.to_string_lossy())
            })
    }

    /// Whether `file` (one of [`DirDataIndex::files`]) is a JavaScript file
    /// that ran in Node.
    pub fn ran(&self, file: &Path) -> bool {
        matches!(self.files.get(file), Some(LocalFile::JsRan(..)))
    }

    /// The template and directory data files of the template at `rel`,
    /// lowest priority first.
    pub fn files(&self, rel: &Path) -> &[PathBuf] {
        self.pages.get(rel).map_or(&[], Vec::as_slice)
    }

    /// The merged data files of the template at `rel`, or the problem of
    /// the first file that keeps it from being built.
    pub fn local(&self, rel: &Path) -> Result<Local, &Problem> {
        let mut local = Local::default();
        let Some(paths) = self.pages.get(rel) else {
            return Ok(local);
        };
        for shown in paths {
            match &self.files[shown] {
                LocalFile::Failed(p) => return Err(p),
                LocalFile::Json(None) => {}
                LocalFile::Json(Some(data)) => {
                    merge(&mut local.data, data);
                    for k in data.keys() {
                        let k = k.strip_prefix(OVERRIDE_PREFIX).unwrap_or(k);
                        local.source.insert(k.to_string(), shown.clone());
                    }
                }
                LocalFile::Js(keys) => {
                    for k in keys {
                        local.js.retain(|(j, _)| j != k);
                        local.js_values.retain(|j| &j.key != k);
                        local.js.push((k.clone(), shown.clone()));
                        local.source.insert(k.clone(), shown.clone());
                    }
                }
                LocalFile::JsRan(None, _) => {}
                LocalFile::JsRan(Some(data), inexact) => {
                    merge(&mut local.data, data);
                    for k in data.keys() {
                        let k = k.strip_prefix(OVERRIDE_PREFIX).unwrap_or(k);
                        local.source.insert(k.to_string(), shown.clone());
                        // A later file's value replaces or merges with an earlier
                        // one: a key it sets exactly is exact again only when it
                        // replaces the whole value, which Brrrty doesn't track, so
                        // an inexact key stays withheld.
                    }
                    for j in inexact {
                        local.js.retain(|(k, _)| k != &j.key);
                        local.js_values.retain(|v| v.key != j.key);
                        local.js_values.push(j.clone());
                    }
                }
            }
        }
        Ok(local)
    }
}

/// The help for a JavaScript data file Brrrty can't run: the exact name
/// of the port that would stand in for it.
pub(crate) fn js_help(file: &Path) -> String {
    format!(
        "Turn on JavaScript data with config.set_js_data(\"node\") in brrrty.config.rhai, which runs it in your own Node (18 or later), or port {} to {}, a Rhai script whose last expression is the data (see PORTING.md).",
        file.display(),
        port_of(file).display()
    )
}

fn read_local(full: &Path, shown: &Path, runner: Option<&DataRunner>, global: &Data) -> LocalFile {
    let name = shown.to_string_lossy();
    if rhai::is_js(&name) {
        return read_js(full, shown);
    }
    let value = if name.ends_with(".rhai") {
        match rhai::run(runner, full, shown, global) {
            Ok(Value::Undefined) => return LocalFile::Json(None),
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => {
                return LocalFile::Failed(
                    Problem::error(
                        codes::DATA_RHAI_ERROR,
                        "a template or directory data file must give a map or object()",
                    )
                    .with_path(shown)
                    .with_help("end the script with #{ ... } or an object()"),
                );
            }
            Err(p) => return LocalFile::Failed(p),
        }
    } else {
        match read_json(full) {
            Ok(v) => v,
            Err((message, at)) => {
                let p = Problem::error(
                    codes::DATA_FILE_INVALID,
                    format!("invalid JSON in {}: {message}", shown.display()),
                )
                .with_path(shown)
                .with_help("11ty stops the whole build on this file. Fix the file or remove it.");
                return LocalFile::Failed(match at {
                    Some((line, col)) => p.at(line, col),
                    None => p,
                });
            }
        }
    };
    let Value::Object(o) = value else {
        return LocalFile::Json(None);
    };
    let mut data = o.as_ref().clone();
    if let Err(bad) = clean_tags(&mut data) {
        return LocalFile::Failed(
            Problem::error(
                codes::TAGS_INVALID,
                format!(
                    "`tags` must be a string or a list, found {}",
                    json_text(&bad)
                ),
            )
            .with_path(shown)
            .with_help("11ty stops the whole build on this file."),
        );
    }
    let reserved = reserved_keys(&data);
    if !reserved.is_empty() {
        return LocalFile::Failed(reserved_problem(&reserved).with_path(shown));
    }
    let frozen = frozen_keys(&data);
    if !frozen.is_empty() {
        return LocalFile::Failed(frozen_problem(&frozen).with_path(shown));
    }
    LocalFile::Json(Some(data))
}

/// The problem for data that adds to 11ty's frozen `eleventy` or `pkg`.
pub(crate) fn frozen_problem(keys: &[String]) -> Problem {
    let list = keys
        .iter()
        .map(|k| format!("`{k}`"))
        .collect::<Vec<_>>()
        .join(" and ");
    Problem::error(
        codes::DATA_RESERVED,
        format!("{list} is set by 11ty and can't be added to here"),
    )
    .with_help("rename or remove it; 11ty stops the whole build here")
}

/// One JavaScript file's results, one per template it applies to, settled
/// into the file's data: refused when it threw or when two results differ
/// (11ty's would depend on the order it loads templates), skipped with a
/// warning when it isn't an object.
fn settle_js(
    full: &Path,
    shown: &Path,
    results: Vec<(PathBuf, JsLoad)>,
) -> (LocalFile, Option<Problem>) {
    // The first result, and its fingerprint once one is needed.
    let mut first: Option<(PathBuf, brrrty_js::runtime::Decoded, Option<String>)> = None;
    for (template, r) in results {
        let d = match r {
            JsLoad::Value(d) => d,
            JsLoad::Thrown(p) => return (LocalFile::Failed(p), None),
            // The runtime stopped: the build stops too, so this is only for
            // the record.
            JsLoad::Unavailable => return (read_js(full, shown), None),
        };
        let differs = match &mut first {
            None => false,
            // The runtime hands back the very same value when the file gave
            // the same data again.
            Some((_, d0, _)) if same_value(d0, &d) => false,
            Some((_, d0, p0)) => *p0.get_or_insert_with(|| fingerprint(d0)) != fingerprint(&d),
        };
        match &first {
            None => first = Some((template, d, None)),
            Some((t0, _, _)) if differs => {
                return (
                    LocalFile::Failed(
                        Problem::error(
                            codes::JS_DATA_IMPURE,
                            format!(
                                "{} returned different data for {} and {}; 11ty's result depends on the order it loads templates",
                                shown.display(),
                                t0.display(),
                                template.display()
                            ),
                        )
                        .with_path(shown)
                        .with_help("Make the data file return the same data every time it is called: move counters, random values and the current time out of it, or into each page's own data."),
                    ),
                    None,
                );
            }
            Some(_) => {}
        }
    }
    let Some((_, d, _)) = first else {
        return (LocalFile::JsRan(None, Vec::new()), None);
    };
    if let (Value::Undefined, Some((path, what))) = (&d.value, d.refused.first())
        && path.is_empty()
        && brrrty_js::runtime::refused_plain_object(what)
    {
        // 11ty merges it; Brrrty can't read all of it.
        return (
            LocalFile::Failed(
                Problem::error(
                    codes::JS_DATA_VALUE,
                    format!(
                        "this template or directory data file gives {what}, which can't cross from JavaScript to Brrrty exactly"
                    ),
                )
                .with_path(shown)
                .with_help("Return a plain object that isn't frozen, or one without hidden keys and well-known symbols."),
            ),
            None,
        );
    }
    let Value::Object(o) = &d.value else {
        let what = match &d.value {
            Value::Undefined if !d.refused.is_empty() || !d.functions.is_empty() => {
                first_inexact(&d).map_or("something else".to_string(), |(_, w)| w)
            }
            Value::Function(_) => "a function".to_string(),
            other => crate::data::json_text(other),
        };
        return (
            LocalFile::JsRan(None, Vec::new()),
            Some(
                Problem::warning(
                    codes::JS_DATA_NOT_OBJECT,
                    format!("this template or directory data file gives {what}, not an object, so 11ty ignores it"),
                )
                .with_path(shown)
                .with_help("Return an object, as `module.exports = { ... }` or `export default { ... }`, or remove the file."),
            ),
        );
    };
    let mut data = o.as_ref().clone();
    // Keys whose value has a function or a refused part inside.
    let mut inexact: Vec<JsInexact> = Vec::new();
    for (path, what) in d.refused.iter().map(|(p, w)| (p, w.clone())).chain(
        d.functions
            .iter()
            .filter(|(p, _)| !super::js::function_allowed(p))
            .map(|(p, _)| (p, "a function".to_string())),
    ) {
        let Some(brrrty_js::runtime::PathPart::Key(key)) = path.first() else {
            continue;
        };
        let key = key.strip_prefix(OVERRIDE_PREFIX).unwrap_or(key).to_string();
        if !inexact.iter().any(|j| j.key == key) {
            inexact.push(JsInexact {
                key,
                file: shown.to_path_buf(),
                at: path.clone(),
                what,
            });
        }
    }
    let shaping: Vec<String> = inexact
        .iter()
        .filter(|j| BUILD_KEYS.contains(&j.key.as_str()))
        .map(|j| format!("`{}`", j.key))
        .collect();
    if !shaping.is_empty() {
        let j = inexact
            .iter()
            .find(|j| BUILD_KEYS.contains(&j.key.as_str()))
            .expect("one is");
        return (
            LocalFile::Failed(
                Problem::error(
                    codes::JS_DATA_VALUE,
                    format!(
                        "this JavaScript data file sets {}, which changes how every page it applies to is built, and its {} is {}, which can't cross from JavaScript to Brrrty exactly",
                        shaping.join(", "),
                        brrrty_js::runtime::path_text(&j.at),
                        j.what
                    ),
                )
                .with_path(shown),
            ),
            None,
        );
    }
    if let Err(bad) = clean_tags(&mut data) {
        return (
            LocalFile::Failed(
                Problem::error(
                    codes::TAGS_INVALID,
                    format!(
                        "`tags` must be a string or a list, found {}",
                        json_text(&bad)
                    ),
                )
                .with_path(shown)
                .with_help("11ty stops the whole build on this file."),
            ),
            None,
        );
    }
    let reserved = reserved_keys(&data);
    if !reserved.is_empty() {
        return (
            LocalFile::Failed(reserved_problem(&reserved).with_path(shown)),
            None,
        );
    }
    let frozen = frozen_keys(&data);
    if !frozen.is_empty() {
        return (
            LocalFile::Failed(frozen_problem(&frozen).with_path(shown)),
            None,
        );
    }
    for j in &inexact {
        data.shift_remove(&j.key);
    }
    (LocalFile::JsRan(Some(data), inexact), None)
}

/// Whether two results are one value handed back twice.
fn same_value(a: &brrrty_js::runtime::Decoded, b: &brrrty_js::runtime::Decoded) -> bool {
    let shared = match (&a.value, &b.value) {
        (Value::Object(x), Value::Object(y)) => std::sync::Arc::ptr_eq(x, y),
        (Value::Array(x), Value::Array(y)) => std::sync::Arc::ptr_eq(x, y),
        _ => false,
    };
    shared && a.functions == b.functions && a.refused == b.refused
}

/// A result as text that tells any two different results apart: `-0`
/// from `0`, `NaN`, key order, `undefined`, and where the functions are.
fn fingerprint(d: &brrrty_js::runtime::Decoded) -> String {
    let functions: Vec<_> = d.functions.iter().map(|(p, _)| p).collect();
    format!("{:?}|{:?}|{:?}", d.value, functions, d.refused)
}

fn read_js(full: &Path, shown: &Path) -> LocalFile {
    let text = std::fs::read(full)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let Some(keys) = export_keys(&text, module_kind(full)) else {
        return LocalFile::Failed(
            Problem::error(
                codes::DATA_JS_UNSUPPORTED,
                "Brrrty can't tell which keys this JavaScript data file sets, so it can't build the pages it applies to",
            )
            .with_path(shown)
            .with_help(js_help(shown)),
        );
    };
    let shaping: Vec<String> = keys
        .iter()
        .map(|k| k.strip_prefix(OVERRIDE_PREFIX).unwrap_or(k))
        // 11ty refuses its reserved names in data, so they stop every page too.
        .filter(|k| BUILD_KEYS.contains(k) || RESERVED.contains(k))
        .map(|k| format!("`{k}`"))
        .collect();
    if !shaping.is_empty() {
        return LocalFile::Failed(
            Problem::error(
                codes::DATA_JS_UNSUPPORTED,
                format!(
                    "this JavaScript data file sets {}, which changes how every page it applies to is built",
                    shaping.join(", ")
                ),
            )
            .with_path(shown)
            .with_help(js_help(shown)),
        );
    }
    LocalFile::Js(
        keys.into_iter()
            .map(|k| {
                k.strip_prefix(OVERRIDE_PREFIX)
                    .map_or(k.clone(), str::to_string)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn site(files: &[(&str, &str)]) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            let p = t.path().join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, body).unwrap();
        }
        t
    }

    fn off() -> crate::options::JsOptions {
        crate::options::JsOptions::default()
    }

    fn session<'a>(o: &'a crate::options::JsOptions, t: &tempfile::TempDir) -> JsSession<'a> {
        JsSession::new(
            o,
            crate::options::RunMode::Build,
            t.path(),
            Value::Undefined,
        )
    }

    fn local(t: &tempfile::TempDir, rel: &str) -> Local {
        let input = t.path().join("src");
        let o = off();
        let index = DirDataIndex::load(
            &input,
            Path::new("src"),
            &[PathBuf::from(rel)],
            None,
            &Data::new(),
            &mut session(&o, t),
        );
        index.local(Path::new(rel)).unwrap()
    }

    /// Probed: `src.json` < `src.11tydata.json` < `posts/posts.json` <
    /// `posts/posts.11tydata.json` < `x.json` < `x.11tydata.json`.
    #[test]
    fn files_merge_lowest_first() {
        let t = site(&[
            ("src/src.json", r#"{"lvl": "src", "arr": [1]}"#),
            ("src/src.11tydata.json", r#"{"lvl": "src11"}"#),
            ("src/posts/posts.json", r#"{"lvl": "posts", "arr": [2]}"#),
            ("src/posts/posts.11tydata.json", r#"{"lvl": "posts11"}"#),
            ("src/posts/x.json", r#"{"arr": [3]}"#),
            ("src/posts/x.11tydata.json", r#"{"lvl": "x11", "arr": [4]}"#),
        ]);
        let l = local(&t, "posts/x.md");
        assert_eq!(
            Value::Object(l.data.into()),
            Value::from_json(&serde_json::json!({"lvl": "x11", "arr": [1, 2, 3, 4]}))
        );
        assert_eq!(l.source["lvl"], Path::new("src/posts/x.11tydata.json"));
        assert_eq!(l.source["arr"], Path::new("src/posts/x.11tydata.json"));
    }

    /// Probed: `posts/posts.md` reads `posts/posts.json` once.
    #[test]
    fn a_file_that_is_both_template_and_directory_data_counts_once() {
        let t = site(&[("src/posts/posts.json", r#"{"arr": [1]}"#)]);
        let l = local(&t, "posts/posts.md");
        assert_eq!(
            Value::Object(l.data.into()),
            Value::from_json(&serde_json::json!({"arr": [1]}))
        );
    }

    #[test]
    fn javascript_files_give_keys() {
        let t = site(&[
            (
                "src/a/a.11tydata.js",
                "module.exports = { x: 1, 'override:y': 2 };",
            ),
            ("src/a/b.11tydata.cjs", "module.exports = { x: 3 };"),
        ]);
        let l = local(&t, "a/b.md");
        assert_eq!(
            l.js,
            [
                ("y".to_string(), PathBuf::from("src/a/a.11tydata.js")),
                ("x".to_string(), PathBuf::from("src/a/b.11tydata.cjs")),
            ]
        );
    }

    #[test]
    fn javascript_build_keys_and_unknown_exports_fail() {
        let t = site(&[
            ("src/a/a.11tydata.js", "module.exports = { layout: 'x' };"),
            ("src/b/b.11tydata.js", "module.exports = () => ({});"),
        ]);
        let input = t.path().join("src");
        let rels = [PathBuf::from("a/p.md"), PathBuf::from("b/p.md")];
        let o = off();
        let index = DirDataIndex::load(
            &input,
            Path::new("src"),
            &rels,
            None,
            &Data::new(),
            &mut session(&o, &t),
        );
        let a = index.local(&rels[0]).unwrap_err();
        assert_eq!(a.code, codes::DATA_JS_UNSUPPORTED);
        assert!(a.message.contains("`layout`"), "{}", a.message);
        assert_eq!(
            a.help.as_deref(),
            Some(js_help(Path::new("src/a/a.11tydata.js")).as_str())
        );
        assert!(
            a.help
                .as_deref()
                .unwrap()
                .contains("port src/a/a.11tydata.js to src/a/a.11tydata.rhai")
        );
        assert_eq!(
            index.local(&rels[1]).unwrap_err().code,
            codes::DATA_JS_UNSUPPORTED
        );
    }
}
