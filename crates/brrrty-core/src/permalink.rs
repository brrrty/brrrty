//! Permalink resolution: where a page is written and what its URL is.

use std::path::{Component, Path, PathBuf};

use brrrty_js::Value;

use crate::frontmatter::Data;
use crate::problem::{Problem, codes};

/// The outcome of resolving a page's permalink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Permalink {
    /// Write the page to `output_rel` (relative to the output folder), served at `url`.
    Write { url: String, output_rel: PathBuf },
    /// `permalink: false`: write nothing.
    Skip,
}

/// Resolve the URL and output path for a page.
///
/// `rel_to_input` is the source file path relative to the input directory.
pub fn resolve_permalink(rel_to_input: &Path, front_matter: &Data) -> Result<Permalink, Problem> {
    match front_matter.get("permalink") {
        None => Ok(default_permalink(rel_to_input)),
        Some(Value::Bool(false)) => Ok(Permalink::Skip),
        Some(Value::Bool(true)) => Err(Problem::error(
            codes::PERMALINK_INVALID,
            "`permalink: true` is not valid",
        )
        .with_path(rel_to_input)
        .with_help("Did you mean `permalink: false`?")),
        Some(Value::Null) => Ok(default_permalink(rel_to_input)),
        Some(Value::String(s)) if s.is_empty() => Ok(default_permalink(rel_to_input)),
        Some(Value::String(s)) => explicit_permalink(rel_to_input, s, front_matter),
        Some(other) => Err(Problem::error(
            codes::PERMALINK_INVALID,
            format!(
                "`permalink` must be a string or `false`, found {}",
                kind(other)
            ),
        )
        .with_path(rel_to_input)),
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "a map",
        Value::Undefined => "undefined",
        Value::Date(_) => "a date",
    }
}

fn build(segments: &[String], trailing_slash: bool) -> Permalink {
    let mut url = String::from("/");
    url.push_str(&segments.join("/"));
    if trailing_slash && !segments.is_empty() {
        url.push('/');
    }
    let mut output_rel: PathBuf = segments.iter().collect();
    if trailing_slash {
        output_rel.push("index.html");
    }
    Permalink::Write { url, output_rel }
}

fn default_permalink(rel: &Path) -> Permalink {
    let mut segments: Vec<String> = rel
        .parent()
        .into_iter()
        .flat_map(|p| p.components())
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    // 11ty's `_hasDuplicateFolder`: `posts/posts.md` is the folder's index.
    if let Some(stem) = rel.file_stem().map(|s| s.to_string_lossy().into_owned())
        && stem != "index"
        && segments.last() != Some(&stem)
    {
        segments.push(stem);
    }
    build(&segments, true)
}

fn explicit_permalink(rel: &Path, raw: &str, front_matter: &Data) -> Result<Permalink, Problem> {
    if raw.contains("{{") || raw.contains("{%") {
        return Err(Problem::error(
            codes::PERMALINK_TEMPLATED,
            "Templated permalinks aren't supported yet",
        )
        .with_path(rel)
        .with_help("Coming in a future release. Use a plain path for now."));
    }
    let is_drive = |seg: &str| {
        let b = seg.as_bytes();
        b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
    };
    if raw.contains('\\') || raw.split('/').any(is_drive) {
        return Err(Problem::error(
            codes::PERMALINK_INVALID,
            format!("Permalink `{raw}` contains a backslash or drive letter"),
        )
        .with_path(rel)
        .with_help(
            "Permalinks use forward slashes and can't contain backslashes or drive letters.",
        ));
    }
    let last = raw.rsplit('/').next().unwrap_or("");
    let mut trailing_slash = last.is_empty() || last == "." || last == "..";
    // Resolve like URL normalization, clamped at the root.
    let mut segments: Vec<String> = Vec::new();
    for seg in raw.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s.to_string()),
        }
    }
    if segments.is_empty() {
        trailing_slash = true;
    }
    // 11ty checks this flag with JavaScript truthiness, so `"yes"` and `1` count too.
    let allow_missing = front_matter
        .get("eleventyAllowMissingExtension")
        .is_some_and(crate::js::js_truthy);
    if !trailing_slash
        && !allow_missing
        && !matches!(
            segments[segments.len() - 1].as_str(),
            "_redirects" | ".htaccess" | "_headers"
        )
        && Path::new(&segments[segments.len() - 1])
            .extension()
            .is_none()
    {
        return Err(Problem::error(
            codes::PERMALINK_NO_EXTENSION,
            format!("Permalink `{raw}` has no file extension"),
        )
        .with_path(rel)
        .with_help(
            "Add a file extension (like /dir/noext.html), end it with / to write an index.html, or set `eleventyAllowMissingExtension: true` to write the file as named.",
        ));
    }
    Ok(build(&segments, trailing_slash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value as Json, json};

    fn w(url: &str, out: &str) -> Permalink {
        Permalink::Write {
            url: url.into(),
            output_rel: out.into(),
        }
    }
    fn fm(v: Json) -> Data {
        let mut m = Data::new();
        m.insert("permalink".into(), Value::from_json(&v));
        m
    }
    fn r(v: Json) -> Result<Permalink, Problem> {
        resolve_permalink(Path::new("c.md"), &fm(v))
    }

    /// Probed: a file named like its folder is the folder's index, matched
    /// with case, and not at the top of the input folder.
    #[test]
    fn a_file_named_like_its_folder_is_its_index() {
        let none = Data::new();
        for (rel, url, out) in [
            ("posts/posts.md", "/posts/", "posts/index.html"),
            ("a/b/b.md", "/a/b/", "a/b/index.html"),
            ("a/a.liquid", "/a/", "a/index.html"),
            ("Up/up.md", "/Up/up/", "Up/up/index.html"),
            (
                "x/2020-01-01-x.md",
                "/x/2020-01-01-x/",
                "x/2020-01-01-x/index.html",
            ),
            ("src.md", "/src/", "src/index.html"),
        ] {
            assert_eq!(
                resolve_permalink(Path::new(rel), &none).unwrap(),
                w(url, out),
                "{rel}"
            );
        }
    }

    #[test]
    fn defaults_match_11ty() {
        let none = Data::new();
        assert_eq!(
            resolve_permalink(Path::new("about.md"), &none).unwrap(),
            w("/about/", "about/index.html")
        );
        assert_eq!(
            resolve_permalink(Path::new("index.md"), &none).unwrap(),
            w("/", "index.html")
        );
        assert_eq!(
            resolve_permalink(Path::new("sub/deep/index.html"), &none).unwrap(),
            w("/sub/deep/", "sub/deep/index.html")
        );
        assert_eq!(
            resolve_permalink(Path::new("posts/2026/2026-11-08-hello-world.md"), &none).unwrap(),
            w(
                "/posts/2026/2026-11-08-hello-world/",
                "posts/2026/2026-11-08-hello-world/index.html"
            )
        );
    }

    #[test]
    fn explicit_permalinks() {
        assert_eq!(
            r("/custom/path.html".into()).unwrap(),
            w("/custom/path.html", "custom/path.html")
        );
        assert_eq!(
            r("custom/noslash.html".into()).unwrap(),
            w("/custom/noslash.html", "custom/noslash.html")
        );
        assert_eq!(r("blog/".into()).unwrap(), w("/blog/", "blog/index.html"));
        assert_eq!(r("/".into()).unwrap(), w("/", "index.html"));
        assert_eq!(
            r("/space dir/".into()).unwrap(),
            w("/space dir/", "space dir/index.html")
        );
        assert_eq!(r(false.into()).unwrap(), Permalink::Skip);
    }

    #[test]
    fn templated_is_rejected() {
        let e = r("/{{ page.fileSlug }}/".into()).unwrap_err();
        assert_eq!(e.code, codes::PERMALINK_TEMPLATED);
        assert_eq!(e.message, "Templated permalinks aren't supported yet");
        assert_eq!(
            e.help.as_deref(),
            Some("Coming in a future release. Use a plain path for now.")
        );
        assert_eq!(
            r("/{% x %}/".into()).unwrap_err().code,
            codes::PERMALINK_TEMPLATED
        );
    }

    #[test]
    fn dot_dot_is_clamped_to_root() {
        assert_eq!(r("/x/../y/".into()).unwrap(), w("/y/", "y/index.html"));
        assert_eq!(
            r("/../etc/x.html".into()).unwrap(),
            w("/etc/x.html", "etc/x.html")
        );
        assert_eq!(
            r("/a/./b//c.html".into()).unwrap(),
            w("/a/b/c.html", "a/b/c.html")
        );
        assert_eq!(r("/../../".into()).unwrap(), w("/", "index.html"));
    }

    #[test]
    fn no_extension_is_an_error() {
        let e = r("/dir/noext".into()).unwrap_err();
        assert_eq!(e.code, codes::PERMALINK_NO_EXTENSION);
        assert!(e.message.contains("/dir/noext"));
        assert_eq!(
            e.help.as_deref(),
            Some(
                "Add a file extension (like /dir/noext.html), end it with / to write an index.html, or set `eleventyAllowMissingExtension: true` to write the file as named."
            )
        );
    }

    fn r_flag(permalink: &str, flag: Json) -> Result<Permalink, Problem> {
        let mut m = fm(permalink.into());
        m.insert(
            "eleventyAllowMissingExtension".into(),
            Value::from_json(&flag),
        );
        resolve_permalink(Path::new("c.md"), &m)
    }

    #[test]
    fn allow_missing_extension_flag_permits_extensionless() {
        assert_eq!(r_flag("/feed", true.into()).unwrap(), w("/feed", "feed"));
    }

    #[test]
    fn allow_missing_extension_flag_is_truthy_like_javascript() {
        for flag in [
            json!("yes"),
            json!(1),
            json!([1]),
            json!({"a": 1}),
            json!([]),
        ] {
            assert_eq!(r_flag("/feed", flag).unwrap(), w("/feed", "feed"));
        }
        for flag in [json!(false), json!(0), json!(""), json!(null)] {
            assert_eq!(
                r_flag("/feed", flag).unwrap_err().code,
                codes::PERMALINK_NO_EXTENSION
            );
        }
    }

    #[test]
    fn extensionless_without_flag_is_an_error() {
        assert_eq!(
            r("/CNAME".into()).unwrap_err().code,
            codes::PERMALINK_NO_EXTENSION
        );
        assert_eq!(
            r_flag("/CNAME", false.into()).unwrap_err().code,
            codes::PERMALINK_NO_EXTENSION
        );
    }

    #[test]
    fn well_known_extensionless_files_need_no_flag() {
        assert_eq!(
            r("/_redirects".into()).unwrap(),
            w("/_redirects", "_redirects")
        );
        assert_eq!(
            r("/.htaccess".into()).unwrap(),
            w("/.htaccess", ".htaccess")
        );
        assert_eq!(r("/_headers".into()).unwrap(), w("/_headers", "_headers"));
        assert_eq!(
            r("/sub/.htaccess".into()).unwrap(),
            w("/sub/.htaccess", "sub/.htaccess")
        );
    }

    #[test]
    fn invalid_types_are_errors() {
        let e = r(true.into()).unwrap_err();
        assert_eq!(e.code, codes::PERMALINK_INVALID);
        assert_eq!(e.help.as_deref(), Some("Did you mean `permalink: false`?"));
        assert_eq!(r(Json::from(5)).unwrap_err().code, codes::PERMALINK_INVALID);
    }

    #[test]
    fn null_and_empty_use_the_default() {
        assert_eq!(r(Json::Null).unwrap(), w("/c/", "c/index.html"));
        assert_eq!(r("".into()).unwrap(), w("/c/", "c/index.html"));
    }

    #[test]
    fn trailing_dot_dot_is_a_directory() {
        assert_eq!(r("/a/..".into()).unwrap(), w("/", "index.html"));
    }

    #[test]
    fn backslashes_and_drive_letters_are_rejected() {
        let help = "Permalinks use forward slashes and can't contain backslashes or drive letters.";
        for bad in [
            "/..\\..\\x.html",
            "/a\\b.html",
            "/C:/x.html",
            "c:/x.html",
            "/a/z:/x.html",
        ] {
            let e = r(bad.into()).unwrap_err();
            assert_eq!(e.code, codes::PERMALINK_INVALID, "{bad}");
            assert_eq!(e.help.as_deref(), Some(help), "{bad}");
        }
        assert!(r("/a:b/x.html".into()).is_ok());
    }
}
