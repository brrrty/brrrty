//! 11ty 3.1.8's `InputPathToUrlTransformPlugin` (`src/Plugins/InputPathToUrl.js`, default
//! options): in every page written to `.html`, a URL (`href`, `src`, ... as
//! `@11ty/posthtml-urls` reads them) that names a template's source file becomes the URL of
//! that template's first page, keeping its `?query` and `#hash`. 11ty's content map is the
//! list of those URLs by input path; a URL it can't find is left as it was.

use std::collections::HashMap;

use url::Url;

use super::rss::posthtml::Node;
use super::rss::urls;

/// The templates' pages: input path (`./src/a.md`) to their URLs, as 11ty's
/// `eleventy.contentMap` event gives them.
pub struct ContentMap {
    urls: HashMap<String, Vec<String>>,
    /// The input folder as 11ty has it (`./src/`).
    input_dir: String,
}

impl ContentMap {
    pub fn new(input_dir: String) -> Self {
        ContentMap {
            urls: HashMap::new(),
            input_dir,
        }
    }

    /// Add one page of a template (its pages come in order).
    pub fn push(&mut self, input_path: &str, url: String) {
        self.urls
            .entry(input_path.to_string())
            .or_default()
            .push(url);
    }

    /// A template with no pages still has an (empty) entry in 11ty's map.
    pub fn known(&mut self, input_path: &str) {
        self.urls.entry(input_path.to_string()).or_default();
    }

    /// What the plugin's URL callback returns for `target` found on the page `source`.
    fn rewrite(&self, target: &str, source: &str) -> Result<String, String> {
        if Url::parse(target).is_ok() {
            return Ok(target.to_string());
        }
        let (suffix, path) = parse_file_path(target);
        let mut path = path;
        if !path.is_empty() {
            path = self.normalize(&path, source)?;
        }
        if urls::INHERITED.contains(&path.as_str()) {
            return Err(format!(
                "a link to `{path}`, a name JavaScript objects have, which the input-path-to-URL plugin can't read the way 11ty does"
            ));
        }
        match self.urls.get(&path).and_then(|u| u.first()) {
            Some(url) if !path.is_empty() => Ok(format!("{url}{suffix}")),
            _ => Ok(format!("{path}{suffix}")),
        }
    }

    fn valid(&self, test: &str) -> Result<Option<String>, String> {
        let decoded = decode_uri_component(test)?;
        let normalized = add_leading_dot_slash(&decoded);
        Ok(self.urls.contains_key(&normalized).then_some(normalized))
    }

    fn normalize(&self, target: &str, source: &str) -> Result<String, String> {
        let input = join(&[&self.input_dir]);
        if join(&[target]).starts_with(&input)
            && let Some(p) = self.valid(target)?
        {
            return Ok(p);
        }
        if let Some(p) = self.valid(&join(&[&self.input_dir, target]))? {
            return Ok(p);
        }
        if !target.is_empty() && !target.starts_with('/') {
            let dir = dir_of(source);
            if let Some(p) = self.valid(&join(&[&dir, target]))? {
                return Ok(p);
            }
        }
        Ok(target.to_string())
    }
}

/// Run the plugin over a page's parsed HTML; `source` is the page's `inputPath`.
pub fn apply(tree: &mut [Node], map: &ContentMap, source: &str) -> Result<(), String> {
    urls::transform(tree, &mut |u, _, _| {
        map.rewrite(u, source).map_err(|e| {
            format!(
                "InputPathToUrlTransformPlugin can't read the link `{u}`: {e}, a `%` must be followed by two hex digits"
            )
        })
    })
}

/// `parseFilePath`: the `?query#hash` suffix, and the path without it.
fn parse_file_path(path: &str) -> (String, String) {
    if path.starts_with('#') || path.starts_with('?') {
        return (path.to_string(), String::new());
    }
    let Ok(u) = Url::parse(&format!("file:{path}")) else {
        return (String::new(), path.to_string());
    };
    let search = match u.query() {
        Some(q) if !q.is_empty() => format!("?{q}"),
        _ => String::new(),
    };
    let hash = match u.fragment() {
        Some(f) if !f.is_empty() => format!("#{f}"),
        _ => String::new(),
    };
    let mut rest = path.to_string();
    if !search.is_empty() {
        rest = rest.replacen(&search, "", 1);
    }
    if !hash.is_empty() {
        rest = rest.replacen(&hash, "", 1);
    }
    (format!("{search}{hash}"), rest)
}

/// Node's `path.normalize` on a POSIX path, then `TemplatePath.normalize` (no trailing `/`).
fn normalize(p: &str) -> String {
    if p.is_empty() {
        return ".".to_string();
    }
    let abs = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|l| *l != "..") {
                    out.pop();
                } else if !abs {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    let mut s = out.join("/");
    if abs {
        s.insert(0, '/');
    }
    if s.is_empty() { ".".to_string() } else { s }
}

/// `TemplatePath.join`: Node's `path.join` (empty parts skipped), normalized.
fn join(parts: &[&str]) -> String {
    let joined = parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    normalize(&joined)
}

fn add_leading_dot_slash(p: &str) -> String {
    if p == "." || p == ".." {
        return format!("{p}/");
    }
    if p.starts_with('/') || p.starts_with("./") || p.starts_with("../") {
        return p.to_string();
    }
    format!("./{p}")
}

/// `TemplatePath.getDirFromFilePath`: `path.parse(p).dir || "."`.
fn dir_of(p: &str) -> String {
    match p.trim_end_matches('/').rfind('/') {
        None => ".".to_string(),
        Some(0) => "/".to_string(),
        Some(i) => p[..i].to_string(),
    }
}

/// JavaScript's `decodeURIComponent`; a malformed escape is its `URIError`.
fn decode_uri_component(s: &str) -> Result<String, String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok());
            let digits = hex.filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit()));
            match digits.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => out.push(v),
                None => return Err("URI malformed".to_string()),
            }
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "URI malformed".to_string())
}
