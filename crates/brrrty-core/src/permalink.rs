//! Permalink resolution: where a page is written and what its URL is.

use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};

use crate::problem::Problem;

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
pub fn resolve_permalink(
    rel_to_input: &Path,
    front_matter: &Map<String, Value>,
) -> Result<Permalink, Problem> {
    match front_matter.get("permalink") {
        None => Ok(default_permalink(rel_to_input)),
        Some(Value::Bool(false)) => Ok(Permalink::Skip),
        Some(Value::Bool(true)) => Err(Problem::error(
            "permalink-invalid",
            "`permalink: true` is not valid",
        )
        .with_path(rel_to_input)
        .with_help("Did you mean `permalink: false`?")),
        Some(Value::String(s)) if s.is_empty() => Ok(default_permalink(rel_to_input)),
        Some(Value::String(s)) => explicit_permalink(rel_to_input, s),
        Some(other) => Err(Problem::error(
            "permalink-invalid",
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
    if let Some(stem) = rel.file_stem().map(|s| s.to_string_lossy().into_owned())
        && stem != "index"
    {
        segments.push(stem);
    }
    build(&segments, true)
}

fn explicit_permalink(rel: &Path, raw: &str) -> Result<Permalink, Problem> {
    if raw.contains("{{") || raw.contains("{%") {
        return Err(Problem::error(
            "permalink-templated",
            "Templated permalinks aren't supported yet",
        )
        .with_path(rel)
        .with_help("Coming in a future release. Use a plain path for now."));
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
    if !trailing_slash
        && Path::new(&segments[segments.len() - 1])
            .extension()
            .is_none()
    {
        return Err(Problem::error(
            "permalink-no-extension",
            format!("Permalink `{raw}` has no file extension"),
        )
        .with_path(rel)
        .with_help(
            "Add a file extension (like /dir/noext.html) or end it with / to write an index.html.",
        ));
    }
    Ok(build(&segments, trailing_slash))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(url: &str, out: &str) -> Permalink {
        Permalink::Write {
            url: url.into(),
            output_rel: out.into(),
        }
    }
    fn fm(v: Value) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("permalink".into(), v);
        m
    }
    fn r(v: Value) -> Result<Permalink, Problem> {
        resolve_permalink(Path::new("c.md"), &fm(v))
    }

    #[test]
    fn defaults_match_11ty() {
        let none = Map::new();
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
            resolve_permalink(
                Path::new("posts/2026/2026-11-08-introducing-cuppa.md"),
                &none
            )
            .unwrap(),
            w(
                "/posts/2026/2026-11-08-introducing-cuppa/",
                "posts/2026/2026-11-08-introducing-cuppa/index.html"
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
        assert_eq!(e.code, "permalink-templated");
        assert_eq!(e.message, "Templated permalinks aren't supported yet");
        assert_eq!(
            e.help.as_deref(),
            Some("Coming in a future release. Use a plain path for now.")
        );
        assert_eq!(
            r("/{% x %}/".into()).unwrap_err().code,
            "permalink-templated"
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
        assert_eq!(e.code, "permalink-no-extension");
        assert!(e.message.contains("/dir/noext"));
        assert_eq!(
            e.help.as_deref(),
            Some(
                "Add a file extension (like /dir/noext.html) or end it with / to write an index.html."
            )
        );
    }

    #[test]
    fn invalid_types_are_errors() {
        let e = r(true.into()).unwrap_err();
        assert_eq!(e.code, "permalink-invalid");
        assert_eq!(e.help.as_deref(), Some("Did you mean `permalink: false`?"));
        assert_eq!(r(Value::from(5)).unwrap_err().code, "permalink-invalid");
        assert_eq!(r(Value::Null).unwrap_err().code, "permalink-invalid");
    }
}
