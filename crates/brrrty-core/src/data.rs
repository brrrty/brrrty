//! Page data: 11ty's deep merge, JavaScript key order, reserved names, and
//! the data sources Brrrty doesn't read yet.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use brrrty_js::{Value, array_index};
use indexmap::IndexMap;
use serde_json::{Map, Value as Json};

use crate::problem::{Problem, codes};

const OVERRIDE_PREFIX: &str = "override:";

/// 11ty's `Merge` (from `@11ty/eleventy-utils`), which builds the data
/// cascade with `dataDeepMerge` on, the default:
///
/// - arrays are joined, target first;
/// - plain objects merge key by key;
/// - a plain object target stays when the source is not an object;
/// - anything else is replaced by the source;
/// - a key written `override:name` replaces `name` instead of merging.
pub(crate) fn merge(target: &mut Map<String, Json>, source: &Map<String, Json>) {
    for (key, value) in source {
        let clean = key.strip_prefix(OVERRIDE_PREFIX).unwrap_or(key);
        // 11ty merges with the key as written, so an `override:` key merges
        // with nothing and replaces.
        let current = target.get(key.as_str()).cloned();
        let merged = merged_item(current, value);
        target.insert(clean.to_string(), merged);
    }
}

fn merged_item(target: Option<Json>, source: &Json) -> Json {
    match (target, source) {
        (Some(Json::Array(mut t)), Json::Array(s)) => {
            t.extend(s.iter().cloned());
            Json::Array(t)
        }
        (Some(Json::Object(mut t)), Json::Object(s)) => {
            merge(&mut t, s);
            Json::Object(t)
        }
        (Some(t @ Json::Object(_)), _) => t,
        // A missing or falsy target takes a copy of a source object, with
        // its `override:` keys cleaned.
        (t, Json::Object(s)) if t.as_ref().is_none_or(|t| !crate::js::js_truthy(t)) => {
            let mut copy = Map::new();
            merge(&mut copy, s);
            Json::Object(copy)
        }
        (_, s) => s.clone(),
    }
}

/// `v` as a JavaScript value. Objects list integer-like keys first in
/// ascending order, then the other keys in insertion order, as JavaScript
/// objects do; 11ty's data is a JavaScript object.
pub(crate) fn js_value(v: &Json) -> Value {
    match v {
        Json::Object(o) => Value::Object(Arc::new(js_object(o))),
        Json::Array(a) => Value::Array(Arc::new(a.iter().map(js_value).collect())),
        other => Value::from_json(other),
    }
}

pub(crate) fn js_object(o: &Map<String, Json>) -> IndexMap<String, Value> {
    let mut index: Vec<(u32, &String, &Json)> = Vec::new();
    let mut rest = Vec::new();
    for (k, v) in o {
        match array_index(k) {
            Some(i) => index.push((i, k, v)),
            None => rest.push((k, v)),
        }
    }
    index.sort_by_key(|(i, _, _)| *i);
    index
        .into_iter()
        .map(|(_, k, v)| (k, v))
        .chain(rest)
        .map(|(k, v)| (k.clone(), js_value(v)))
        .collect()
}

/// Top-level names 11ty sets itself and refuses from data.
const RESERVED: &[&str] = &["content", "collections"];
/// `page` properties 11ty sets itself and refuses from data.
const RESERVED_PAGE: &[&str] = &[
    "date",
    "inputPath",
    "fileSlug",
    "filePathStem",
    "outputFileExtension",
    "templateSyntax",
    "url",
    "outputPath",
];

/// The reserved names `data` sets, as 11ty's `ReservedData` finds them.
pub(crate) fn reserved_keys(data: &Map<String, Json>) -> Vec<String> {
    let mut keys: Vec<String> = RESERVED
        .iter()
        .filter(|k| data.contains_key(**k))
        .map(|k| k.to_string())
        .collect();
    match data.get("page") {
        None => {}
        Some(Json::Object(page)) => keys.extend(
            RESERVED_PAGE
                .iter()
                .filter(|k| page.contains_key(**k))
                .map(|k| format!("page.{k}")),
        ),
        // `typeof null` is "object" in JavaScript, so 11ty lets null through.
        Some(Json::Null) => {}
        Some(_) => keys.push("page".to_string()),
    }
    keys
}

pub(crate) fn reserved_problem(keys: &[String]) -> Problem {
    let list = keys
        .iter()
        .map(|k| format!("`{k}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let (noun, verb) = if keys.len() == 1 {
        ("name", "is")
    } else {
        ("names", "are")
    };
    Problem::error(
        codes::DATA_RESERVED,
        format!("{list} {verb} set by 11ty and can't be set in data"),
    )
    .with_help(format!(
        "rename or remove the {noun}; 11ty stops the whole build here"
    ))
}

/// The 1-based line of the top-level `key:` in front matter `text`, the
/// whole file, if there is one.
pub(crate) fn key_line(text: &str, key: &str) -> Option<usize> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    lines.next()?; // the opening `---`
    for (i, line) in lines.enumerate() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("---") && line[3..].trim().is_empty() {
            return None;
        }
        let Some(rest) = line.strip_prefix(key) else {
            continue;
        };
        let rest = rest.trim_start_matches([' ', '\t']);
        if rest.starts_with(':') {
            return Some(i + 2);
        }
    }
    None
}

/// The top-level names global data files would set: one per file in
/// `_data` (`.json`, `.mjs`, `.cjs`, `.js`) and one per folder.
pub(crate) fn global_data_names(data_dir: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return names;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            names.insert(name);
        } else if let Some(stem) = ["json", "mjs", "cjs", "js"]
            .iter()
            .find_map(|ext| name.strip_suffix(&format!(".{ext}")))
        {
            names.insert(stem.to_string());
        }
    }
    names
}

/// Template and directory data files that apply to the template at `rel`
/// (relative to the input folder), as 11ty's `getLocalDataPaths` lists
/// them: `<name>.11tydata.{js,cjs,mjs,json}` and `<name>.json` for the
/// template itself, each folder above it inside the input folder, and the
/// input folder itself unless it is the project root.
pub(crate) fn local_data_files(input: &Path, input_rel: &Path, rel: &Path) -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();
    let dir = rel.parent().unwrap_or(Path::new(""));
    if let Some(stem) = rel.file_stem() {
        bases.push(dir.join(stem));
    }
    let mut d = Some(dir);
    while let Some(here) = d {
        if let Some(last) = here.file_name() {
            bases.push(here.join(last));
        }
        d = here.parent().filter(|p| !p.as_os_str().is_empty());
    }
    let mut found = Vec::new();
    let in_root = input_rel.file_name().map(|last| (input_rel, last));
    let mut check = |base_full: PathBuf, shown: PathBuf| {
        for suffix in [
            ".11tydata.js",
            ".11tydata.cjs",
            ".11tydata.mjs",
            ".11tydata.json",
            ".json",
        ] {
            let mut full = base_full.clone().into_os_string();
            full.push(suffix);
            if Path::new(&full).is_file() {
                let mut s = shown.clone().into_os_string();
                s.push(suffix);
                found.push(PathBuf::from(s));
            }
        }
    };
    for base in bases {
        check(input.join(&base), input_rel.join(&base));
    }
    if let Some((shown_root, last)) = in_root {
        check(input.join(last), shown_root.join(last));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(v: Json) -> Map<String, Json> {
        match v {
            Json::Object(m) => m,
            _ => unreachable!(),
        }
    }

    /// Probed: layout `{list: [l1, l2], obj: {a: 1, b: 2}, author: {name: lay}}`
    /// under page `{list: [p1], obj: {b: 3, c: 4}, author: Bob}`.
    #[test]
    fn merges_like_11ty() {
        let mut t = Map::new();
        merge(
            &mut t,
            &map(
                json!({"list": ["l1", "l2"], "obj": {"a": 1, "b": 2}, "author": {"name": "lay"}, "arr": [1], "str": "s"}),
            ),
        );
        merge(
            &mut t,
            &map(
                json!({"list": ["p1"], "obj": {"b": 3, "c": 4}, "author": "Bob", "arr": {"k": "v"}, "str": {"k": "v"}}),
            ),
        );
        assert_eq!(
            Json::Object(t),
            json!({"list": ["l1", "l2", "p1"], "obj": {"a": 1, "b": 3, "c": 4}, "author": {"name": "lay"}, "arr": {"k": "v"}, "str": {"k": "v"}})
        );
    }

    #[test]
    fn override_prefix_replaces() {
        let mut t = map(json!({"tags": ["a"], "o": {"x": 1}}));
        merge(
            &mut t,
            &map(json!({"override:tags": ["b"], "override:o": {"y": 2}, "n": {"override:z": [1]}})),
        );
        assert_eq!(
            Json::Object(t),
            json!({"tags": ["b"], "o": {"y": 2}, "n": {"z": [1]}})
        );
    }

    #[test]
    fn javascript_key_order() {
        let v = js_value(&json!({"b": "B", "2": "two", "a": "A", "1": "one", "01": "x"}));
        let Value::Object(o) = v else { unreachable!() };
        assert_eq!(o.keys().collect::<Vec<_>>(), ["1", "2", "b", "a", "01"]);
    }

    #[test]
    fn reserved() {
        assert_eq!(
            reserved_keys(&map(json!({"content": 1, "page": {"url": "/", "mine": 1}}))),
            ["content", "page.url"]
        );
        assert_eq!(reserved_keys(&map(json!({"page": "x"}))), ["page"]);
        assert!(reserved_keys(&map(json!({"page": {"mine": 1}}))).is_empty());
    }

    #[test]
    fn key_lines() {
        let text = "---\ntitle: x\nlayout : base\n---\nlayout: body\n";
        assert_eq!(key_line(text, "layout"), Some(3));
        assert_eq!(key_line(text, "title"), Some(2));
        assert_eq!(key_line(text, "nope"), None);
        assert_eq!(key_line("---\nlayouts: x\n---\n", "layout"), None);
    }

    #[test]
    fn local_data_files_follow_11ty() {
        let t = tempfile::tempdir().unwrap();
        let input = t.path().join("src");
        for f in [
            "src.json",
            "sub/sub.json",
            "sub/p.json",
            "sub/p.11tydata.js",
            "sub/deeper/deeper.11tydata.json",
            "sub/other.json",
        ] {
            let p = input.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "{}").unwrap();
        }
        let found = local_data_files(&input, Path::new("src"), Path::new("sub/p.md"));
        assert_eq!(
            found,
            [
                PathBuf::from("src/sub/p.11tydata.js"),
                PathBuf::from("src/sub/p.json"),
                PathBuf::from("src/sub/sub.json"),
                PathBuf::from("src/src.json"),
            ]
        );
        assert_eq!(
            local_data_files(&input, Path::new("src"), Path::new("sub/deeper/q.md")),
            [
                PathBuf::from("src/sub/deeper/deeper.11tydata.json"),
                PathBuf::from("src/sub/sub.json"),
                PathBuf::from("src/src.json"),
            ]
        );
        // The input folder's own data file needs a named input folder.
        assert!(local_data_files(t.path(), Path::new(""), Path::new("x.md")).is_empty());
    }
}
