//! Global data: every file under `_data`, as nested keys.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use brrrty_js::Value;

use super::rhai;
use super::{js_object, reserved_keys, reserved_problem};
use crate::discover::{broken_link_warning, loop_warning};
use crate::frontmatter::Data;
use crate::options::DataRunner;
use crate::problem::{Problem, codes};

/// A JavaScript data file. Brrrty can't run it yet, so it only records which
/// key the file would set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JsDataRef {
    pub key_path: Vec<String>,
    pub file: PathBuf,
}

#[derive(Debug)]
pub(crate) struct GlobalData {
    /// An object of everything loaded from JSON files.
    pub value: Value,
    pub js_keys: Vec<JsDataRef>,
    /// Notes, such as a JavaScript file a `.rhai` port stands in for.
    pub notes: Vec<Problem>,
    pub problems: Vec<Problem>,
    /// Top-level keys Brrrty can't give a correct value for: a JavaScript
    /// data file or a failed file sets something under them.
    pub unavailable: BTreeSet<String>,
}

/// The extensions 11ty reads from `_data`, in its priority order: a later
/// extension overrides an earlier one at the same key. `rhai` is Brrrty's
/// own, and comes last: a port wins over the file it ports.
const EXTENSIONS: [&str; 5] = ["json", "mjs", "cjs", "js", "rhai"];

/// Load every data file under `<input>/_data`, as 11ty's `getAllGlobalData`
/// does: the files are found with a case-insensitive glob that includes
/// dot files and folders, ordered by extension priority, and each is set at
/// the path of its folders plus its name without the last extension. Names
/// keep their dots (`a.b.json` is the key `a.b`).
///
/// `.rhai` files are run by `runner`, given `initial` (what 11ty hands an
/// exported function). A `.js` file is ignored, with a note, when a `.rhai`
/// file has the same key.
pub(crate) fn load_global(
    input: &Path,
    root: &Path,
    runner: Option<&DataRunner>,
    initial: &Data,
) -> GlobalData {
    let mut out = GlobalData {
        value: Value::Object(Default::default()),
        js_keys: Vec::new(),
        notes: Vec::new(),
        problems: Vec::new(),
        unavailable: BTreeSet::new(),
    };
    let data_dir = input.join("_data");
    let mut files = Vec::new();
    let start = data_dir.canonicalize().unwrap_or_else(|_| data_dir.clone());
    collect(
        &data_dir,
        root,
        &mut vec![start],
        &mut files,
        &mut out.problems,
    );
    // `Vec::sort_by_key` is stable, like the sort 11ty does.
    files.sort_by_key(|f| priority(f));

    let key_path = |file: &Path| -> Vec<String> {
        let rel = file.strip_prefix(&data_dir).unwrap_or(file);
        let mut path: Vec<String> = rel
            .parent()
            .into_iter()
            .flat_map(Path::components)
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        path.push(
            file.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        path
    };
    let ext_of = |file: &Path| file.extension().map(|e| e.to_string_lossy().into_owned());
    // The keys `.rhai` files set: a JavaScript file for one of them is
    // ported, and not read.
    let ported: Vec<Vec<String>> = files
        .iter()
        .filter(|f| ext_of(f).as_deref() == Some("rhai"))
        .map(|f| key_path(f))
        .collect();

    let mut data = Data::new();
    for file in files {
        let shown = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
        let path = key_path(&file);
        let root_key = path[0].clone();
        let loaded = match ext_of(&file).as_deref() {
            Some("json") => read_json(&file).map_err(|(message, at)| {
                let p = Problem::error(
                    codes::DATA_FILE_INVALID,
                    format!("invalid JSON in {}: {message}", shown.display()),
                )
                .with_path(&shown)
                .with_help("11ty stops the whole build on this file. Fix the file or remove it.");
                match at {
                    Some((line, col)) => p.at(line, col),
                    None => p,
                }
            }),
            Some("rhai") => rhai::run(runner, &file, &shown, initial),
            Some("js" | "cjs" | "mjs") => {
                if ported.contains(&path) {
                    out.notes.push(rhai::shadowed(&shown));
                } else {
                    out.js_keys.push(JsDataRef {
                        key_path: path,
                        file: shown,
                    });
                    out.unavailable.insert(root_key);
                }
                continue;
            }
            _ => {
                // The glob matches names like `Up.JSON` and `.json`, and 11ty
                // then has no parser for them and stops.
                out.problems.push(
                    Problem::error(
                        codes::DATA_FILE_INVALID,
                        format!("no data parser for {}", shown.display()),
                    )
                    .with_path(&shown)
                    .with_help("11ty stops the whole build on this file. Use a lowercase `.json`, `.js`, `.cjs`, `.mjs` or `.rhai` extension."),
                );
                out.unavailable.insert(root_key);
                continue;
            }
        };
        match loaded {
            Ok(v) => {
                if set_path(&mut data, &path, v).is_err() {
                    out.problems.push(
                        Problem::error(
                            codes::DATA_FILE_INVALID,
                            format!(
                                "{} can't be set: its key passes through a list, or is a list position",
                                shown.display()
                            ),
                        )
                        .with_path(&shown)
                        .with_help("Brrrty doesn't build lists from data folders yet. Give the file and its folders names that aren't list positions like `0`, or put the list in one file."),
                    );
                    out.unavailable.insert(root_key);
                }
            }
            Err(p) => {
                out.problems.push(p);
                out.unavailable.insert(root_key);
            }
        }
    }

    // 11ty refuses its reserved names in any data.
    let reserved = reserved_keys(&data);
    if !reserved.is_empty() {
        out.problems.push(
            reserved_problem(&reserved).with_path(data_dir.strip_prefix(root).unwrap_or(&data_dir)),
        );
        for key in &reserved {
            let top = key.split('.').next().unwrap_or(key).to_string();
            data.shift_remove(&top);
            out.unavailable.insert(top);
        }
    }
    for key in &out.unavailable {
        data.shift_remove(key);
    }
    out.value = Value::Object(Arc::new(js_object(&data)));
    out
}

/// Data files under `dir`: each folder's files by name, then its folders by
/// name, which is the order the glob walk gave on the probed systems.
/// Symbolic links are followed, as 11ty's glob does; a link that loops back
/// into its own folder or points nowhere is skipped with a warning, as
/// discovery does.
fn collect(
    dir: &Path,
    root: &Path,
    ancestors: &mut Vec<PathBuf>,
    found: &mut Vec<PathBuf>,
    warnings: &mut Vec<Problem>,
) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    let shown = |p: &Path| p.strip_prefix(root).unwrap_or(p).to_path_buf();
    let mut dirs = Vec::new();
    for entry in entries {
        let path = entry.path();
        // `metadata` looks through links; a link whose target is gone fails.
        let Ok(meta) = std::fs::metadata(&path) else {
            if entry.file_type().is_ok_and(|t| t.is_symlink()) {
                warnings.push(broken_link_warning(&path, &shown(&path)));
            }
            continue;
        };
        if meta.is_dir() {
            dirs.push(path);
        } else if meta.is_file() && matches_glob(&entry.file_name().to_string_lossy()) {
            found.push(path);
        }
    }
    for d in dirs {
        let canonical = d.canonicalize().unwrap_or_else(|_| d.clone());
        if ancestors.contains(&canonical) {
            warnings.push(loop_warning(&shown(&d)));
            continue;
        }
        ancestors.push(canonical);
        collect(&d, root, ancestors, found, warnings);
        ancestors.pop();
    }
}

/// 11ty's glob is `**/*.{json,mjs,cjs,js}`, matched without regard to case.
fn matches_glob(name: &str) -> bool {
    let lower = name.to_lowercase();
    EXTENSIONS.iter().any(|e| lower.ends_with(&format!(".{e}")))
}

fn priority(path: &Path) -> usize {
    let name = path.to_string_lossy();
    EXTENSIONS
        .iter()
        .position(|e| name.ends_with(e))
        .unwrap_or(EXTENSIONS.len())
}

/// A JSON data file as 11ty reads it: a leading byte order mark is dropped
/// and a blank file is `{}`. The error is serde's message and its position.
pub(crate) type JsonError = (String, Option<(usize, usize)>);

pub(crate) fn read_json(file: &Path) -> Result<Value, JsonError> {
    let bytes = std::fs::read(file).map_err(|e| (format!("cannot read file: {e}"), None))?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if text.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_str::<serde_json::Value>(text)
        .map(|v| Value::from_json(&v))
        .map_err(|e| {
            let full = e.to_string();
            let message = full
                .rsplit_once(" at line ")
                .map_or(full.clone(), |(m, _)| m.to_string());
            (message, Some((e.line(), e.column())))
        })
}

/// True for the keys lodash's `set` turns into a list position.
fn is_index(key: &str) -> bool {
    key.parse::<u32>()
        .is_ok_and(|n| n != u32::MAX && n.to_string() == key)
}

/// lodash's `set(data, path, value)`, which 11ty uses: missing folders and
/// folders holding a scalar become objects. Lists are the part Brrrty can't
/// mirror: `Err` when the path would make or pass through one.
fn set_path(data: &mut Data, path: &[String], value: Value) -> Result<(), ()> {
    let (first, rest) = path.split_first().ok_or(())?;
    if rest.is_empty() {
        data.insert(first.clone(), value);
        return Ok(());
    }
    let child = match data.get_mut(first.as_str()) {
        Some(Value::Object(o)) => Arc::make_mut(o),
        Some(Value::Array(_)) => return Err(()),
        _ => {
            if is_index(&rest[0]) {
                return Err(());
            }
            data.insert(first.clone(), Value::Object(Default::default()));
            match data.get_mut(first.as_str()) {
                Some(Value::Object(o)) => Arc::make_mut(o),
                _ => return Err(()),
            }
        }
    };
    set_path(child, rest, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn site(files: &[(&str, &str)]) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            let p = t.path().join("src/_data").join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, body).unwrap();
        }
        fs::create_dir_all(t.path().join("src")).unwrap();
        t
    }

    fn load(t: &tempfile::TempDir) -> GlobalData {
        load_global(&t.path().join("src"), t.path(), None, &Data::new())
    }

    fn j(v: serde_json::Value) -> Value {
        Value::from_json(&v)
    }

    #[test]
    fn nested_folders_make_nested_keys() {
        let t = site(&[
            ("a.json", r#"{"x": 1, "y": {"p": 1}}"#),
            ("a/b.json", r#"{"b": 2, "y": {"q": 2}}"#),
            ("sub/deep/leaf.json", r#"{"in": 1}"#),
        ]);
        let g = load(&t);
        assert!(g.problems.is_empty(), "{:?}", g.problems);
        // Probed with 11ty 3.1.8: `a/b.json` is set at `a.b`, whole.
        assert_eq!(
            g.value,
            j(json!({
                "a": {"x": 1, "y": {"p": 1}, "b": {"b": 2, "y": {"q": 2}}},
                "sub": {"deep": {"leaf": {"in": 1}}},
            }))
        );
    }

    #[test]
    fn a_folder_file_replaces_the_same_key_of_the_parent_file() {
        let t = site(&[
            ("a.json", r#"{"b": 1, "k": 2}"#),
            ("a/b.json", r#"{"z": 3}"#),
        ]);
        assert_eq!(load(&t).value, j(json!({"a": {"b": {"z": 3}, "k": 2}})));
    }

    #[test]
    fn dots_in_names_stay_in_the_key() {
        // Probed: `a.b.json` is the key "a.b", and `x.11tydata.json` is
        // "x.11tydata"; neither nests.
        let t = site(&[("a.b.json", r#"{"dot": 1}"#), ("x.11tydata.json", "{}")]);
        assert_eq!(
            load(&t).value,
            j(json!({"a.b": {"dot": 1}, "x.11tydata": {}}))
        );
    }

    #[test]
    fn dot_files_and_dot_folders_are_read() {
        let t = site(&[(".d.json", r#"{"v": 1}"#), (".h/x.json", r#"{"v": 2}"#)]);
        assert_eq!(
            load(&t).value,
            j(json!({".d": {"v": 1}, ".h": {"x": {"v": 2}}}))
        );
    }

    #[test]
    fn empty_and_null_and_bom_and_array_roots() {
        let t = site(&[
            ("empty.json", ""),
            ("blank.json", "  \n"),
            ("nul.json", "null"),
            ("arr.json", "[1, 2]"),
            ("str.json", "\"s\""),
            ("bom.json", "\u{feff}{\"bom\": 1}"),
        ]);
        let g = load(&t);
        assert!(g.problems.is_empty(), "{:?}", g.problems);
        assert_eq!(
            g.value,
            j(json!({
                "arr": [1, 2], "blank": {}, "bom": {"bom": 1}, "empty": {},
                "nul": null, "str": "s",
            }))
        );
    }

    #[test]
    fn a_scalar_is_replaced_by_a_folder() {
        // Probed: `n.json` is 5 and `n/c.json` is {"c": 1}; `n` ends up
        // `{c: {c: 1}}`.
        let t = site(&[("n.json", "5"), ("n/c.json", r#"{"c": 1}"#)]);
        assert_eq!(load(&t).value, j(json!({"n": {"c": {"c": 1}}})));
    }

    #[test]
    fn absent_and_empty_data_folders_are_empty_objects() {
        let t = site(&[]);
        assert_eq!(load(&t).value, j(json!({})));
        fs::create_dir_all(t.path().join("src/_data")).unwrap();
        let g = load(&t);
        assert_eq!(g.value, j(json!({})));
        assert!(g.problems.is_empty() && g.js_keys.is_empty());
    }

    #[test]
    fn keys_follow_javascript_order() {
        let t = site(&[("o.json", r#"{"z": 1, "2": 2, "a": 3, "1": 4}"#)]);
        let Value::Object(o) = load(&t).value else {
            panic!()
        };
        let Value::Object(inner) = &o["o"] else {
            panic!()
        };
        assert_eq!(inner.keys().collect::<Vec<_>>(), ["1", "2", "z", "a"]);
    }

    #[test]
    fn invalid_json_is_one_problem_with_a_position() {
        let t = site(&[
            ("bad.json", "{\n  \"a\": \n}"),
            ("ok.json", "{\"v\": 1}"),
            ("deep/worse.json", "{"),
        ]);
        let g = load(&t);
        assert_eq!(g.value, j(json!({"ok": {"v": 1}})));
        let mut p = g.problems.clone();
        p.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(p.len(), 2, "{p:?}");
        assert_eq!(p[0].code, codes::DATA_FILE_INVALID);
        assert_eq!(p[0].path.as_deref(), Some(Path::new("src/_data/bad.json")));
        assert_eq!((p[0].line, p[0].column), (Some(3), Some(1)));
        assert_eq!(
            p[1].path.as_deref(),
            Some(Path::new("src/_data/deep/worse.json"))
        );
        assert_eq!(
            g.unavailable,
            BTreeSet::from(["bad".to_string(), "deep".to_string()])
        );
    }

    #[test]
    fn javascript_files_are_refs_and_mark_their_key() {
        let t = site(&[
            ("a.js", "module.exports = {}"),
            ("b/c.cjs", ""),
            ("d.mjs", ""),
            ("a.json", r#"{"x": 1}"#),
            ("e.json", "{}"),
        ]);
        let g = load(&t);
        assert!(g.problems.is_empty(), "{:?}", g.problems);
        let refs: Vec<(Vec<String>, PathBuf)> = g
            .js_keys
            .iter()
            .map(|r| (r.key_path.clone(), r.file.clone()))
            .collect();
        assert_eq!(
            refs,
            vec![
                (vec!["d".into()], "src/_data/d.mjs".into()),
                (vec!["b".into(), "c".into()], "src/_data/b/c.cjs".into()),
                (vec!["a".into()], "src/_data/a.js".into()),
            ]
        );
        assert_eq!(
            g.unavailable,
            BTreeSet::from(["a".to_string(), "b".to_string(), "d".to_string()])
        );
        // A JavaScript file contributes no value, and its key is withheld.
        assert_eq!(g.value, j(json!({"e": {}})));
    }

    #[test]
    fn extensions_11ty_cannot_parse_are_problems() {
        // Probed: `Up.JSON` and `.json` are matched by 11ty's glob and then
        // stop the whole build.
        let t = site(&[("Up.JSON", "{}"), (".json", "{}"), ("notes.txt", "x")]);
        let g = load(&t);
        assert_eq!(g.problems.len(), 2, "{:?}", g.problems);
        assert!(
            g.problems
                .iter()
                .all(|p| p.code == codes::DATA_FILE_INVALID)
        );
        assert_eq!(g.value, j(json!({})));
    }

    #[test]
    fn index_like_folder_keys_are_withheld() {
        // lodash would build an array for `u/0.json`; Brrrty doesn't.
        let t = site(&[
            ("u/0.json", "{}"),
            ("arr.json", "[1]"),
            ("arr/c.json", "{}"),
        ]);
        let g = load(&t);
        assert_eq!(g.problems.len(), 2, "{:?}", g.problems);
        assert_eq!(
            g.unavailable,
            BTreeSet::from(["arr".to_string(), "u".to_string()])
        );
        assert_eq!(g.value, j(json!({})));
    }

    #[test]
    fn reserved_names_are_refused() {
        let t = site(&[("content.json", "\"x\""), ("ok.json", "1")]);
        let g = load(&t);
        assert_eq!(g.problems.len(), 1, "{:?}", g.problems);
        assert_eq!(g.problems[0].code, codes::DATA_RESERVED);
        assert_eq!(g.value, j(json!({"ok": 1})));
    }

    /// Probed: 11ty's glob follows symlinked folders and files in `_data`.
    #[cfg(unix)]
    #[test]
    fn symlinked_folders_and_files_are_followed() {
        use std::os::unix::fs::symlink;
        let t = site(&[("real/x.json", r#"{"v": 1}"#)]);
        let data = t.path().join("src/_data");
        fs::create_dir_all(t.path().join("ext")).unwrap();
        fs::write(t.path().join("ext/y.json"), r#"{"v": 2}"#).unwrap();
        symlink("../../ext", data.join("linked")).unwrap();
        symlink("real/x.json", data.join("file.json")).unwrap();
        symlink("missing.json", data.join("gone.json")).unwrap();
        symlink(".", data.join("real/back")).unwrap();
        let g = load(&t);
        assert_eq!(
            g.value,
            j(json!({
                "file": {"v": 1},
                "linked": {"y": {"v": 2}},
                "real": {"x": {"v": 1}},
            }))
        );
        let mut codes: Vec<_> = g
            .problems
            .iter()
            .map(|p| (p.code, p.path.clone()))
            .collect();
        codes.sort();
        assert_eq!(
            codes,
            vec![
                (
                    crate::problem::codes::SYMLINK_BROKEN,
                    Some(PathBuf::from("src/_data/gone.json"))
                ),
                (
                    crate::problem::codes::SYMLINK_LOOP,
                    Some(PathBuf::from("src/_data/real/back"))
                ),
            ]
        );
    }
}
