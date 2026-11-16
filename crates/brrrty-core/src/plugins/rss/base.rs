//! 11ty 3.1.8's HTML `<base>` plugin (`Plugins/HtmlBasePlugin.js`), which
//! the RSS plugin adds: turning a URL into a full one with a base, and the
//! `htmlBaseUrl` and `addPathPrefixToFullUrl` filters. 11ty's
//! `pathPrefix` is always `/` in Brrrty.
//!
//! URLs are parsed with the `url` crate, which follows the same WHATWG URL
//! standard as Node's `URL`.

use url::Url;

/// `new URL(input, base)`. The `url` crate refuses three or more slashes
/// at the start of a URL relative to an `http:`-like base (`///x`), where
/// the standard (and Node) skips them all and reads a host (`https://x/`);
/// they are made two first.
pub fn join(base: &Url, input: &str) -> Result<Url, url::ParseError> {
    if base.is_special() {
        // The parser drops leading spaces and controls, and tabs and
        // newlines anywhere.
        let cleaned: String = input
            .trim_start_matches(|c: char| c <= ' ')
            .chars()
            .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
            .collect();
        let slashes = cleaned
            .chars()
            .take_while(|c| matches!(c, '/' | '\\'))
            .count();
        if slashes > 2 {
            return base.join(&format!("//{}", &cleaned[slashes..]));
        }
    }
    base.join(input)
}

/// `isValidUrl`: `new URL(url)` doesn't throw.
pub fn is_valid_url(url: &str) -> bool {
    Url::parse(url).is_ok()
}

/// Node's `path.posix.join("/", path)`: the path with `.` and `..` resolved
/// and repeated slashes collapsed, keeping a trailing slash.
pub fn join_url_parts(path: &str) -> String {
    let joined = format!("/{path}");
    let trailing = joined.ends_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut out = format!("/{}", parts.join("/"));
    if trailing && out != "/" {
        out.push('/');
    }
    out
}

/// `addPathPrefixToUrl(url, "/", base)`: `new URL(url, base)` with its path
/// joined to the prefix. `Err` where JavaScript throws.
pub fn add_path_prefix_to_url(url: &str, base: Option<&str>) -> Result<String, String> {
    let mut u = match base {
        Some(base) => {
            let b = Url::parse(base).map_err(|_| format!("Invalid URL: {base}"))?;
            join(&b, url).map_err(|_| format!("Invalid URL: {url}"))?
        }
        None => Url::parse(url).map_err(|_| format!("Invalid URL: {url}"))?,
    };
    // Only a URL with a path that starts with `/` (http, https and the
    // like) can be given one here; for others JavaScript does nothing.
    if !u.cannot_be_a_base() {
        let joined = join_url_parts(u.path());
        u.set_path(&joined);
    }
    Ok(u.to_string())
}

/// `transformUrl(url, base, { pathPrefix: "/", pageUrl })` for a `base`
/// that is a full URL (the only kind the feeds accept). `Err` where 11ty
/// throws.
pub fn transform_url(url: &str, base: &str, page_url: Option<&str>) -> Result<String, String> {
    if is_valid_url(url) {
        return Ok(url.to_string());
    }
    let mut url = url.to_string();
    if let Some(page) = page_url.filter(|p| !p.is_empty())
        && !url.starts_with('/')
    {
        let resolved = Url::parse(&format!("http://example.com{page}"))
            .and_then(|b| join(&b, &url))
            .map_err(|_| format!("Invalid URL: {url}"))?;
        url = format!(
            "{}{}",
            resolved.path(),
            resolved
                .fragment()
                .map(|f| format!("#{f}"))
                .unwrap_or_default()
        );
        // `urlObj.hash` is `""` for an empty fragment (`#`).
        if url.ends_with('#') {
            url.pop();
        }
    }
    add_path_prefix_to_url(&url, Some(base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_like_path_join() {
        assert_eq!(join_url_parts("/"), "/");
        assert_eq!(join_url_parts("/a//b/"), "/a/b/");
        assert_eq!(join_url_parts("//a"), "/a");
        assert_eq!(join_url_parts("/a/./b/../c"), "/a/c");
        assert_eq!(join_url_parts("/.."), "/");
    }

    /// Probed: the RSS plugin's `htmlBaseUrl` and HTML transform in 11ty
    /// 3.1.8 with base `https://example.com/`, on page `/posts/a/`.
    #[test]
    fn transforms_like_the_base_plugin() {
        let base = "https://example.com/";
        let page = Some("/posts/a/");
        for (input, want) in [
            ("/i.png", "https://example.com/i.png"),
            ("x?a=1&b=2#h", "https://example.com/posts/a/x#h"),
            ("#top", "https://example.com/posts/a/#top"),
            ("", "https://example.com/posts/a/"),
            ("../b/", "https://example.com/posts/b/"),
            ("//cdn.example.org/x.js", "https://cdn.example.org/x.js"),
            ("mailto:me@example.com", "mailto:me@example.com"),
            ("///x", "https://x/"),
            ("https://other.org/a b", "https://other.org/a b"),
            ("/a b", "https://example.com/a%20b"),
            ("/q?x=1", "https://example.com/q?x=1"),
        ] {
            assert_eq!(transform_url(input, base, page).unwrap(), want, "{input}");
        }
        assert_eq!(
            add_path_prefix_to_url("https://example.com", None).unwrap(),
            "https://example.com/"
        );
        assert!(add_path_prefix_to_url("/", None).is_err());
    }
}
