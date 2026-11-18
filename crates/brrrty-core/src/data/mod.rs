//! Page data: 11ty's deep merge, JavaScript key order, reserved names,
//! global data, directory and template data files, and the cascade.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use brrrty_js::Value;

use crate::frontmatter::Data;
use crate::problem::{Problem, codes};

mod cascade;
mod computed;
mod dir;
mod global;
mod js;
mod jskeys;
mod rhai;
pub(crate) use cascade::{clean_tags, frozen_keys, page_data};
pub(crate) use computed::{Plan as ComputedPlan, set_path};
pub(crate) use dir::{DirDataIndex, Local, frozen_problem, js_help};
pub(crate) use global::{GlobalData, load_global};
pub use js::JsFileState;
pub(crate) use js::{
    JsCalls, JsFilters, JsPage, JsSession, JsTemplateLoad, calls as js_calls, first_unusable,
    value_problem,
};
pub(crate) use rhai::{is_js, port_of};

pub(crate) const OVERRIDE_PREFIX: &str = "override:";

/// Data keys that change how a page is built, not only what it prints.
/// A JavaScript data file setting one of these affects every page it
/// applies to, read or not.
pub(crate) const BUILD_KEYS: &[&str] = &[
    "layout",
    "permalink",
    "date",
    "tags",
    "templateEngineOverride",
    "eleventyComputed",
    "eleventyDataSchema",
    "pagination",
    "eleventyExcludeFromCollections",
    "dynamicPermalink",
    "eleventyAllowMissingExtension",
    "permalinkBypassOutputDir",
    "page",
    "eleventy",
    "pkg",
];

/// 11ty's `Merge` (from `@11ty/eleventy-utils`), which builds the data
/// cascade with `dataDeepMerge` on, the default:
///
/// - arrays are joined, target first;
/// - plain objects merge key by key;
/// - a plain object target stays when the source is not an object;
/// - anything else is replaced by the source;
/// - a key written `override:name` replaces `name` instead of merging.
pub(crate) fn merge(target: &mut Data, source: &Data) {
    for (key, value) in source {
        let clean = key.strip_prefix(OVERRIDE_PREFIX).unwrap_or(key);
        // 11ty merges with the key as written, so an `override:` key merges
        // with nothing and replaces.
        let current = target.get(key.as_str()).cloned();
        let merged = merged_item(current, value);
        target.insert(clean.to_string(), merged);
    }
}

pub(crate) fn merged_item(target: Option<Value>, source: &Value) -> Value {
    match (target, source) {
        (Some(Value::Array(t)), Value::Array(s)) => {
            Value::Array(Arc::new(t.iter().chain(s.iter()).cloned().collect()))
        }
        (Some(Value::Object(t)), Value::Object(s)) => {
            let mut t = Arc::unwrap_or_clone(t);
            merge(&mut t, s);
            Value::Object(Arc::new(t))
        }
        (Some(t @ Value::Object(_)), _) => t,
        // A missing or falsy target takes a copy of a source object, with
        // its `override:` keys cleaned.
        (t, Value::Object(s)) if t.as_ref().is_none_or(|t| !t.js_truthy()) => {
            // Values never change in place, so a source with nothing to
            // clean is shared rather than copied: pages keep one object.
            if !has_override_keys(s) {
                return source.clone();
            }
            let mut copy = Data::new();
            merge(&mut copy, s);
            Value::Object(Arc::new(copy))
        }
        (_, s) => s.clone(),
    }
}

/// Whether an object, or one inside it (not through a list), has an
/// `override:` key that a merge would clean.
fn has_override_keys(o: &Data) -> bool {
    o.iter().any(|(k, v)| {
        k.starts_with(OVERRIDE_PREFIX)
            || matches!(v, Value::Object(inner) if has_override_keys(inner))
    })
}

/// The config's `addGlobalData` values as 11ty sets them: each name in
/// JavaScript key order, with lodash's `set`, so `a.b` sets `b` inside `a`.
pub(crate) fn config_global(added: &Data) -> Result<Data, Problem> {
    let mut out = Data::new();
    for (name, value) in Value::js_entries(added) {
        let refuse = |why: String| {
            Problem::error(
                codes::CONFIG_UNSUPPORTED,
                format!("global data `{name}` can't be set: {why}"),
            )
            .with_help("name the value with plain keys separated by dots, like `site.title`")
        };
        if name.contains(['[', ']', '"', '\'']) {
            return Err(refuse(
                "Brrrty doesn't read brackets or quotes in a key path".into(),
            ));
        }
        let path: Vec<String> = name.split('.').map(str::to_string).collect();
        set_path(&mut out, &path, value.clone()).map_err(refuse)?;
    }
    Ok(out)
}

/// `v` with every object's keys in JavaScript order: integer-like keys first
/// in ascending order, then the other keys in insertion order, as JavaScript
/// objects do; 11ty's data is a JavaScript object.
#[cfg(test)]
pub(crate) fn js_value(v: &Value) -> Value {
    reordered(v).unwrap_or_else(|| v.clone())
}

pub(crate) fn js_object(o: &Data) -> Data {
    reordered_object(o).unwrap_or_else(|| o.clone())
}

/// `v` in JavaScript key order, or `None` when it already is, so data that
/// pages share (a global data list) stays one shared value.
fn reordered(v: &Value) -> Option<Value> {
    match v {
        Value::Object(o) => reordered_object(o).map(|d| Value::Object(Arc::new(d))),
        Value::Array(a) => {
            let mut out: Option<Vec<Value>> = None;
            for (i, x) in a.iter().enumerate() {
                match (reordered(x), &mut out) {
                    (Some(y), None) => {
                        let mut list = a[..i].to_vec();
                        list.push(y);
                        out = Some(list);
                    }
                    (Some(y), Some(list)) => list.push(y),
                    (None, Some(list)) => list.push(x.clone()),
                    (None, None) => {}
                }
            }
            out.map(|list| Value::Array(Arc::new(list)))
        }
        _ => None,
    }
}

fn reordered_object(o: &Data) -> Option<Data> {
    let has_index = o.keys().any(|k| brrrty_js::array_index(k).is_some());
    let inner: Vec<Option<Value>> = o.values().map(reordered).collect();
    if !has_index && inner.iter().all(Option::is_none) {
        return None;
    }
    let fresh: Data = o
        .iter()
        .zip(inner)
        .map(|((k, v), r)| (k.clone(), r.unwrap_or_else(|| v.clone())))
        .collect();
    Some(
        Value::js_entries(&fresh)
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

/// `v` the way `JSON.stringify` writes it, for messages: dates are ISO strings and
/// non-finite numbers are `null`.
pub(crate) fn json_text(v: &Value) -> String {
    fn to_json(v: &Value) -> serde_json::Value {
        use serde_json::Value as J;
        match v {
            Value::Undefined | Value::Null | Value::Function(_) => J::Null,
            Value::Bool(b) => J::Bool(*b),
            Value::Number(n) => serde_json::Number::from_f64(*n).map_or(J::Null, J::Number),
            Value::String(s) => J::String(s.to_string()),
            Value::Array(a) => J::Array(a.iter().map(to_json).collect()),
            Value::Object(o) => J::Object(o.iter().map(|(k, v)| (k.clone(), to_json(v))).collect()),
            Value::Date(d) => J::String(
                d.with_timezone(&chrono::Utc)
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            ),
        }
    }
    serde_json::to_string(&to_json(v)).unwrap_or_default()
}

/// Top-level names 11ty sets itself and refuses from data.
pub(crate) const RESERVED: &[&str] = &["content", "collections"];
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
pub(crate) fn reserved_keys(data: &Data) -> Vec<String> {
    let mut keys: Vec<String> = RESERVED
        .iter()
        .filter(|k| data.contains_key(**k))
        .map(|k| k.to_string())
        .collect();
    match data.get("page") {
        None => {}
        Some(Value::Object(page)) => keys.extend(
            RESERVED_PAGE
                .iter()
                .filter(|k| page.contains_key(**k))
                .map(|k| format!("page.{k}")),
        ),
        // `typeof null` is "object" in JavaScript, so 11ty lets null through.
        Some(Value::Null) => {}
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

/// Template and directory data files that apply to the template at `rel`
/// (relative to the input folder), as 11ty's `getLocalDataPaths` lists
/// them: `<name>.11tydata.{rhai,js,cjs,mjs,json}` and `<name>.json` for the
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
            ".11tydata.rhai",
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

    fn map(v: serde_json::Value) -> Data {
        match Value::from_json(&v) {
            Value::Object(m) => Arc::unwrap_or_clone(m),
            _ => unreachable!(),
        }
    }

    fn val(v: serde_json::Value) -> Value {
        Value::from_json(&v)
    }

    /// Probed: layout `{list: [l1, l2], obj: {a: 1, b: 2}, author: {name: lay}}`
    /// under page `{list: [p1], obj: {b: 3, c: 4}, author: Bob}`.
    #[test]
    fn merges_like_11ty() {
        let mut t = Data::new();
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
            Value::Object(Arc::new(t)),
            val(
                json!({"list": ["l1", "l2", "p1"], "obj": {"a": 1, "b": 3, "c": 4}, "author": {"name": "lay"}, "arr": {"k": "v"}, "str": {"k": "v"}})
            )
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
            Value::Object(Arc::new(t)),
            val(json!({"tags": ["b"], "o": {"y": 2}, "n": {"z": [1]}}))
        );
    }

    #[test]
    fn javascript_key_order() {
        let v = js_value(&val(
            json!({"b": "B", "2": "two", "a": "A", "1": "one", "01": "x"}),
        ));
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
