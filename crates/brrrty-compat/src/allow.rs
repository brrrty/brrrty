//! Approved allow-list of known, accepted differences.

use crate::CompatError;
use globset::{GlobBuilder, GlobMatcher};
use regex::Regex;
use serde::Deserialize;
use std::borrow::Cow;
use std::path::Path;

#[derive(Deserialize)]
struct RawFile {
    #[serde(default)]
    allow: Vec<RawRule>,
}

#[derive(Deserialize)]
struct RawRule {
    id: String,
    files: String,
    pattern: String,
    // Parsed so the file is validated, but only approval is checked.
    #[allow(dead_code)]
    reason: String,
    #[serde(default)]
    approved_by: String,
    #[serde(default)]
    approved_on: String,
}

struct Rule {
    id: String,
    files: GlobMatcher,
    pattern: Regex,
}

/// Approved rules that blank out known differences before comparison.
pub struct AllowList {
    rules: Vec<Rule>,
}

impl AllowList {
    /// An allow-list with no rules.
    pub fn empty() -> Self {
        Self { rules: Vec::new() }
    }

    /// Parse `[[allow]]` rules from TOML. Other top-level keys are ignored.
    /// Every rule needs a non-empty `approved_by` and `approved_on`.
    pub fn from_toml(src: &str) -> Result<Self, CompatError> {
        let raw: RawFile = toml::from_str(src)?;
        let mut rules = Vec::new();
        for r in raw.allow {
            if r.approved_by.trim().is_empty() || r.approved_on.trim().is_empty() {
                return Err(CompatError::Unapproved { id: r.id });
            }
            rules.push(Rule {
                files: GlobBuilder::new(&r.files)
                    .literal_separator(true)
                    .build()?
                    .compile_matcher(),
                pattern: Regex::new(&r.pattern)?,
                id: r.id,
            });
        }
        Ok(Self { rules })
    }

    /// Replace every match of each rule that applies to `rel_path` with `⟨allowed:ID⟩`.
    pub fn apply<'a>(&self, rel_path: &Path, content: &'a str) -> Cow<'a, str> {
        let mut out = Cow::Borrowed(content);
        for rule in &self.rules {
            if !rule.files.is_match(rel_path) {
                continue;
            }
            let marker = format!("⟨allowed:{}⟩", rule.id);
            if let Cow::Owned(s) = rule
                .pattern
                .replace_all(&out, regex::NoExpand(marker.as_str()))
            {
                out = Cow::Owned(s);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CompatError;
    use std::path::Path;

    const TOML: &str = r#"
input = "src"
[[allow]]
id = "build-timestamp"
files = "**/*.html"
pattern = '<meta name="build-timestamp" content="[^"]*">'
reason = "Build time differs every run"
approved_by = "Josh"
approved_on = "2026-11-15"
"#;

    #[test]
    fn replaces_matches_in_matching_files_only() {
        let list = AllowList::from_toml(TOML).unwrap();
        let html = r#"<meta name="build-timestamp" content="Sat, 15 Nov 2026">"#;
        assert_eq!(
            list.apply(Path::new("a/index.html"), html),
            "⟨allowed:build-timestamp⟩"
        );
        assert_eq!(list.apply(Path::new("a.css"), html), html);
    }
    #[test]
    fn unapproved_rule_is_rejected() {
        let src = TOML.replace("approved_by = \"Josh\"\n", "");
        assert!(
            matches!(AllowList::from_toml(&src), Err(CompatError::Unapproved { id }) if id == "build-timestamp")
        );
    }

    #[test]
    fn empty_or_missing_approval_rejected() {
        let empty = TOML.replace("approved_by = \"Josh\"", "approved_by = \"\"");
        assert!(matches!(
            AllowList::from_toml(&empty),
            Err(CompatError::Unapproved { .. })
        ));
        let missing = TOML.replace("approved_on = \"2026-11-15\"\n", "");
        assert!(
            matches!(AllowList::from_toml(&missing), Err(CompatError::Unapproved { id }) if id == "build-timestamp")
        );
    }
    #[test]
    fn invalid_regex_and_glob() {
        let bad_re = TOML.replace(
            "'<meta name=\"build-timestamp\" content=\"[^\"]*\">'",
            "'('",
        );
        assert!(matches!(
            AllowList::from_toml(&bad_re),
            Err(CompatError::Regex(_))
        ));
        let bad_glob = TOML.replace("**/*.html", "[");
        assert!(matches!(
            AllowList::from_toml(&bad_glob),
            Err(CompatError::Glob(_))
        ));
    }
    #[test]
    fn multiple_rules_and_literal_dollar_id() {
        let src = r#"
[[allow]]
id = "a$1"
files = "*.txt"
pattern = 'x+'
reason = "r"
approved_by = "J"
approved_on = "d"
[[allow]]
id = "b"
files = "*.txt"
pattern = 'y'
reason = "r"
approved_by = "J"
approved_on = "d"
"#;
        let list = AllowList::from_toml(src).unwrap();
        assert_eq!(
            list.apply(Path::new("f.txt"), "xxy"),
            "⟨allowed:a$1⟩⟨allowed:b⟩"
        );
    }
    #[test]
    fn star_does_not_cross_separators() {
        let src = TOML.replace("**/*.html", "sub/*.html");
        let list = AllowList::from_toml(&src).unwrap();
        let html = r#"<meta name="build-timestamp" content="x">"#;
        assert_ne!(list.apply(Path::new("sub/a.html"), html), html);
        assert_eq!(list.apply(Path::new("sub/x/y.html"), html), html);
        let deep = AllowList::from_toml(TOML).unwrap();
        assert_ne!(deep.apply(Path::new("a/b/c.html"), html), html);
        assert_ne!(deep.apply(Path::new("c.html"), html), html);
    }
}
