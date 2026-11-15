use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Liquid,
    Nunjucks,
}

impl Lang {
    /// Template file extension 11ty recognises for this language.
    pub fn ext(self) -> &'static str {
        match self {
            Lang::Liquid => "liquid",
            Lang::Nunjucks => "njk",
        }
    }
    /// Directory name of the case set and the permalink prefix.
    pub fn dir(self) -> &'static str {
        match self {
            Lang::Liquid => "liquid",
            Lang::Nunjucks => "nunjucks",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub name: String,
    pub group: String,
    pub template: String,
    pub data: serde_json::Map<String, serde_json::Value>,
    pub includes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expected {
    Output(String),
    Error(String),
}

const GROUPS: [&str; 4] = ["core", "filters", "eleventy", "errors"];
const RESERVED: [&str; 10] = [
    "page",
    "collections",
    "eleventy",
    "pkg",
    "content",
    "permalink",
    "layout",
    "tags",
    "pagination",
    "eleventyComputed",
];

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    case: Vec<Raw>,
}

#[derive(Deserialize)]
struct Raw {
    name: String,
    group: String,
    template: String,
    data: Option<String>,
    #[serde(default)]
    includes: BTreeMap<String, String>,
}

/// Loads every `*.toml` in `dir` (sorted by file name) and pairs each case
/// with its `expected/<name>.out` or `.error` file when one exists.
pub fn load_cases(dir: &Path) -> Result<Vec<(Case, Option<Expected>)>, String> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();

    let mut seen = BTreeSet::new();
    let mut includes: BTreeMap<String, String> = BTreeMap::new();
    let mut out = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file: File = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        for raw in file.case {
            let name = raw.name;
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(format!("case name {name:?} must be [A-Za-z0-9_-]+"));
            }
            if !seen.insert(name.clone()) {
                return Err(format!("duplicate case name {name:?}"));
            }
            if !GROUPS.contains(&raw.group.as_str()) {
                return Err(format!("case {name}: unknown group {:?}", raw.group));
            }
            let data = match raw.data {
                None => serde_json::Map::new(),
                Some(s) => match serde_json::from_str::<serde_json::Value>(&s) {
                    Ok(serde_json::Value::Object(m)) => m,
                    Ok(_) => return Err(format!("case {name}: data must be a JSON object")),
                    Err(e) => return Err(format!("case {name}: bad data JSON: {e}")),
                },
            };
            if let Some(k) = RESERVED.iter().find(|k| data.contains_key(**k)) {
                return Err(format!("case {name}: data key {k:?} is reserved by 11ty"));
            }
            for (file_name, content) in &raw.includes {
                match includes.get(file_name) {
                    Some(prev) if prev != content => {
                        return Err(format!(
                            "case {name}: include {file_name:?} reused with different content"
                        ));
                    }
                    _ => {
                        includes.insert(file_name.clone(), content.clone());
                    }
                }
            }
            let expected = read_expected(dir, &name)?;
            out.push((
                Case {
                    name,
                    group: raw.group,
                    template: raw.template,
                    data,
                    includes: raw.includes,
                },
                expected,
            ));
        }
    }
    Ok(out)
}

fn read_expected(dir: &Path, name: &str) -> Result<Option<Expected>, String> {
    let base = dir.join("expected");
    if base.join(format!("{name}.out")).exists() && base.join(format!("{name}.error")).exists() {
        return Err(format!(
            "case {name}: has both .out and .error expected files"
        ));
    }
    for (ext, make) in [
        ("out", Expected::Output as fn(String) -> Expected),
        ("error", Expected::Error),
    ] {
        let p = base.join(format!("{name}.{ext}"));
        if p.exists() {
            let text = fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            return Ok(Some(make(text)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const TWO: &str = r#"
[[case]]
name = "a"
group = "core"
template = "{{ x }}"
data = '{"x": 1}'
[case.includes]
"p.liquid" = "part"

[[case]]
name = "b"
group = "filters"
template = "hi"
"#;

    fn load(toml: &str) -> Result<Vec<(Case, Option<Expected>)>, String> {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("c.toml"), toml).unwrap();
        load_cases(d.path())
    }

    #[test]
    fn loads_two_cases_with_data_and_includes() {
        let cases = load(TWO).unwrap();
        assert_eq!(cases.len(), 2);
        let (a, ea) = &cases[0];
        assert_eq!(a.name, "a");
        assert_eq!(a.data["x"], 1);
        assert_eq!(a.includes["p.liquid"], "part");
        assert!(ea.is_none());
        assert!(cases[1].0.data.is_empty());
    }

    #[test]
    fn loads_expected_files() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("c.toml"), TWO).unwrap();
        fs::create_dir(d.path().join("expected")).unwrap();
        fs::write(d.path().join("expected/a.out"), "1").unwrap();
        fs::write(d.path().join("expected/b.error"), "boom").unwrap();
        let cases = load_cases(d.path()).unwrap();
        assert_eq!(cases[0].1, Some(Expected::Output("1".into())));
        assert_eq!(cases[1].1, Some(Expected::Error("boom".into())));
    }

    #[test]
    fn both_expected_files_fail() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("c.toml"), TWO).unwrap();
        fs::create_dir(d.path().join("expected")).unwrap();
        fs::write(d.path().join("expected/a.out"), "1").unwrap();
        fs::write(d.path().join("expected/a.error"), "1").unwrap();
        assert!(load_cases(d.path()).unwrap_err().contains("both"));
    }

    #[test]
    fn duplicate_names_fail() {
        let t = "[[case]]\nname='a'\ngroup='core'\ntemplate='x'\n[[case]]\nname='a'\ngroup='core'\ntemplate='y'\n";
        assert!(load(t).unwrap_err().contains("duplicate"));
    }

    #[test]
    fn reserved_key_fails() {
        let t = "[[case]]\nname='a'\ngroup='core'\ntemplate='x'\ndata='{\"page\": 1}'\n";
        assert!(load(t).unwrap_err().contains("page"));
    }

    #[test]
    fn conflicting_include_content_fails() {
        let t = "[[case]]\nname='a'\ngroup='core'\ntemplate='x'\n[case.includes]\n'p.liquid'='1'\n\
                 [[case]]\nname='b'\ngroup='core'\ntemplate='x'\n[case.includes]\n'p.liquid'='2'\n";
        assert!(load(t).unwrap_err().contains("p.liquid"));
    }

    #[test]
    fn unknown_group_fails() {
        let t = "[[case]]\nname='a'\ngroup='nope'\ntemplate='x'\n";
        assert!(load(t).unwrap_err().contains("group"));
    }
}
