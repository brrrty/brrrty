//! Directory and template data files: every data file that applies to a
//! template, read once and shared by the pages under it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use brrrty_js::Value;

use super::cascade::{clean_tags, frozen_keys};
use super::global::read_json;
use super::jskeys::export_keys;
use super::{
    BUILD_KEYS, OVERRIDE_PREFIX, RESERVED, json_text, local_data_files, merge, reserved_keys,
    reserved_problem,
};
use crate::frontmatter::Data;
use crate::problem::{Problem, codes};

/// One data file, read.
#[derive(Debug)]
pub(crate) enum LocalFile {
    /// A JSON file's object, its `tags` cleaned. `None` when the file
    /// holds something else, which 11ty skips.
    Json(Option<Data>),
    /// A JavaScript file Brrrty can't run, and the top-level keys it sets.
    Js(Vec<String>),
    /// A file no page under it can be built with, and why.
    Failed(Problem),
}

/// The data files that apply to a template, merged.
#[derive(Debug, Default)]
pub(crate) struct Local {
    pub data: Data,
    /// Keys set by JavaScript files, each with the file that sets it (the
    /// highest-priority one when several do).
    pub js: Vec<(String, PathBuf)>,
    /// For each top-level key, the highest-priority file that sets it.
    pub source: HashMap<String, PathBuf>,
}

/// Every directory and template data file, by its root-relative path.
#[derive(Debug, Default)]
pub(crate) struct DirDataIndex {
    files: HashMap<PathBuf, LocalFile>,
    /// Each template's data files, lowest priority first.
    pages: HashMap<PathBuf, Vec<PathBuf>>,
}

impl DirDataIndex {
    /// Read the data files that apply to `templates` (relative to the
    /// input folder).
    pub fn load(input: &Path, input_rel: &Path, templates: &[PathBuf]) -> DirDataIndex {
        let mut index = DirDataIndex::default();
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
            for shown in &paths {
                if !index.files.contains_key(shown) {
                    let full = input.join(shown.strip_prefix(input_rel).unwrap_or(shown));
                    index.files.insert(shown.clone(), read_local(&full, shown));
                }
            }
            index.pages.insert(rel.clone(), paths);
        }
        index
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
                        local.js.push((k.clone(), shown.clone()));
                        local.source.insert(k.clone(), shown.clone());
                    }
                }
            }
        }
        Ok(local)
    }
}

/// The help for a JavaScript data file Brrrty can't run.
pub(crate) fn js_help(file: &Path) -> String {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name
        .rsplit_once('.')
        .map_or(name.as_str(), |(stem, _)| stem);
    format!(
        "Brrrty doesn't run JavaScript data files. They need a Rhai port ({name} becomes {stem}.rhai), coming in a later release."
    )
}

fn read_local(full: &Path, shown: &Path) -> LocalFile {
    let name = shown.to_string_lossy();
    if [".js", ".cjs", ".mjs"].iter().any(|e| name.ends_with(e)) {
        return read_js(full, shown);
    }
    let value = match read_json(full) {
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

fn read_js(full: &Path, shown: &Path) -> LocalFile {
    let text = std::fs::read(full)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let Some(keys) = export_keys(&text) else {
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

    fn local(t: &tempfile::TempDir, rel: &str) -> Local {
        let input = t.path().join("src");
        let index = DirDataIndex::load(&input, Path::new("src"), &[PathBuf::from(rel)]);
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
        let index = DirDataIndex::load(&input, Path::new("src"), &rels);
        let a = index.local(&rels[0]).unwrap_err();
        assert_eq!(a.code, codes::DATA_JS_UNSUPPORTED);
        assert!(a.message.contains("`layout`"), "{}", a.message);
        assert_eq!(
            a.help.as_deref(),
            Some(js_help(Path::new("a.11tydata.js")).as_str())
        );
        assert!(a.help.as_deref().unwrap().contains("a.11tydata.rhai"));
        assert_eq!(
            index.local(&rels[1]).unwrap_err().code,
            codes::DATA_JS_UNSUPPORTED
        );
    }
}
