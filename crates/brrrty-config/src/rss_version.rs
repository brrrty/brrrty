//! Which version of `@11ty/eleventy-plugin-rss` a site installs.
//!
//! Brrrty's `rss` plugin is version 3's: version 2's `dateToRfc822` writes
//! the time zone another way. `brrrty init` records the version it saw on a
//! `// RSS plugin: 3.1.0` line; a build that uses the plugin refuses a site
//! that installs another major version, and warns (`config-stale`) when the
//! version changed since that line was written.

use std::path::Path;

use brrrty_core::{Problem, codes};

use crate::CONFIG_FILE;

/// The package.
pub const PACKAGE: &str = "@11ty/eleventy-plugin-rss";
/// How the line recording the version `brrrty init` saw starts.
pub const RSS_LINE: &str = "// RSS plugin: ";

/// The most read of a JSON file to find the version.
const MAX_READ: u64 = 16 << 20;

/// The major version a version or a plain range names: `3.1.0`, `^3.0.0`,
/// `~3`, `3.x`. `None` for anything wider (`>=2`, `*`, `^2 || ^3`, a tag).
pub fn major(v: &str) -> Option<u64> {
    let v = v.trim();
    let v = v.strip_prefix(['^', '~', '=', 'v']).unwrap_or(v);
    if v.contains([' ', '|', '<', '>', '*']) {
        return None;
    }
    let digits: String = v.chars().take_while(char::is_ascii_digit).collect();
    let rest = &v[digits.len()..];
    if digits.is_empty() || !(rest.is_empty() || rest.starts_with('.')) {
        return None;
    }
    digits.parse().ok()
}

/// The version in `text` (package-lock.json, the installed package.json,
/// or the site's package.json), in that order of trust.
pub fn from_files(
    lock: Option<&str>,
    installed: Option<&str>,
    package: Option<&str>,
) -> Option<String> {
    let json = |t: Option<&str>| -> Option<serde_json::Value> {
        serde_json::from_str(t?.trim_start_matches('\u{feff}')).ok()
    };
    if let Some(lock) = json(lock) {
        let v = lock["packages"][format!("node_modules/{PACKAGE}")]["version"]
            .as_str()
            .or_else(|| lock["dependencies"][PACKAGE]["version"].as_str());
        if let Some(v) = v {
            return Some(v.to_string());
        }
    }
    if let Some(v) = json(installed).and_then(|i| i["version"].as_str().map(str::to_string)) {
        return Some(v);
    }
    let package = json(package)?;
    ["dependencies", "devDependencies"]
        .iter()
        .find_map(|d| package[d][PACKAGE].as_str().map(str::to_string))
}

fn read(root: &Path, name: &str) -> Option<String> {
    use std::io::Read as _;
    let path = root.join(name);
    if !std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(&path)
        .ok()?
        .take(MAX_READ)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// The version the site in `root` installs, as far as its files say.
pub fn installed(root: &Path) -> Option<String> {
    from_files(
        read(root, "package-lock.json").as_deref(),
        read(root, &format!("node_modules/{PACKAGE}/package.json")).as_deref(),
        read(root, "package.json").as_deref(),
    )
}

/// The version `brrrty init` recorded in the config `src`, and its line.
fn recorded(src: &str) -> Option<(usize, &str)> {
    src.lines().enumerate().find_map(|(i, l)| {
        let v = l.trim_end().strip_prefix(RSS_LINE)?.trim();
        (!v.is_empty()).then_some((i + 1, v))
    })
}

/// For a config that adds the `rss` plugin: an error when the site installs
/// a version other than 3, else a warning when the version changed since
/// `brrrty init` recorded it.
pub fn check(root: &Path, src: &str) -> Result<Option<Problem>, Problem> {
    let now = installed(root);
    if let Some(v) = &now
        && let Some(m) = major(v)
        && m != 3
    {
        return Err(Problem::error(
            codes::CONFIG_UNSUPPORTED,
            format!(
                "this site installs version {v} of {PACKAGE}, and Brrrty's `rss` plugin is version 3's"
            ),
        )
        .with_path(CONFIG_FILE)
        .with_help(
            "version 2's `dateToRfc822` writes the time zone another way (`CST` for `-0600`), so pages could differ from 11ty's. Update the plugin to version 3 and check your feeds, or build with 11ty",
        ));
    }
    let Some((line, was)) = recorded(src) else {
        return Ok(None);
    };
    if crate::eleventy_cli::left_11ty(root) || now.as_deref() == Some(was) {
        return Ok(None);
    }
    let shown = now.as_deref().unwrap_or("not found");
    Ok(Some(
        Problem::warning(
            codes::CONFIG_STALE,
            format!(
                "{PACKAGE} is {shown} now, and {CONFIG_FILE} was ported when it was {was}"
            ),
        )
        .with_path(CONFIG_FILE)
        .at(line, 1)
        .with_help(match &now {
            Some(v) => format!(
                "Check that the plugin's filters and feeds still give what they did, then set this line to `{RSS_LINE}{v}`."
            ),
            None => "Brrrty can't find which version the site installs now (package-lock.json, node_modules or package.json); check that it is version 3.".to_string(),
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn majors() {
        assert_eq!(major("3.1.0"), Some(3));
        assert_eq!(major("^3.0.0"), Some(3));
        assert_eq!(major("~2"), Some(2));
        assert_eq!(major("3.x"), Some(3));
        assert_eq!(major(">=2"), None);
        assert_eq!(major("^2 || ^3"), None);
        assert_eq!(major("latest"), None);
        assert_eq!(major("3beta"), None);
    }

    #[test]
    fn checks_the_installed_version() {
        let t = tempfile::tempdir().unwrap();
        let pkg = |v: &str| {
            std::fs::write(
                t.path().join("package.json"),
                format!(
                    "{{\"devDependencies\":{{\"@11ty/eleventy\":\"3\",\"{PACKAGE}\":\"{v}\"}}}}"
                ),
            )
            .unwrap()
        };
        pkg("^3.1.0");
        assert!(check(t.path(), "").unwrap().is_none());
        let rhai = format!("// x\n{RSS_LINE}^3.1.0\n");
        assert!(check(t.path(), &rhai).unwrap().is_none());
        pkg("3.0.0");
        let w = check(t.path(), &rhai).unwrap().unwrap();
        assert_eq!(w.code, codes::CONFIG_STALE);
        assert_eq!(w.line, Some(2));
        pkg("2.0.4");
        let e = check(t.path(), &rhai).unwrap_err();
        assert_eq!(e.code, codes::CONFIG_UNSUPPORTED);
        // The lock file wins over package.json.
        std::fs::write(
            t.path().join("package-lock.json"),
            format!("{{\"packages\":{{\"node_modules/{PACKAGE}\":{{\"version\":\"3.1.0\"}}}}}}"),
        )
        .unwrap();
        assert_eq!(installed(t.path()).as_deref(), Some("3.1.0"));
        assert!(check(t.path(), "").unwrap().is_none());
    }
}
