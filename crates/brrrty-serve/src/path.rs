//! Turning a request target into a file inside the output folder.
//!
//! The rules follow `@11ty/eleventy-dev-server` 2.1.2 (`mapUrlToFilePath`),
//! with stricter path handling: the dev server leaves a gap where `..%2f`
//! crashes it, accepts a sibling folder that shares the output folder's name
//! as a prefix, and follows symlinks out of the folder.

use std::fs;
use std::path::{Path, PathBuf};

/// The folder being served, with its resolved location.
#[derive(Debug)]
pub(crate) struct Root {
    pub(crate) canonical: PathBuf,
}

impl Root {
    pub(crate) fn new(path: &Path) -> std::io::Result<Root> {
        let canonical = fs::canonicalize(path)?;
        if !canonical.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                "not a directory",
            ));
        }
        Ok(Root { canonical })
    }

    /// The file at `segments` (with `suffix` appended to the last one, or as a
    /// new segment when `as_segment`), only when it is a regular file that
    /// still lies inside the root once symlinks are followed.
    fn file(&self, segments: &[String], suffix: Option<(&str, bool)>) -> Option<Found> {
        let mut p = self.canonical.clone();
        let mut parts: Vec<String> = segments.to_vec();
        match suffix {
            Some((s, true)) => parts.push(s.to_string()),
            Some((s, false)) => parts.last_mut()?.push_str(s),
            None => {}
        }
        if parts.is_empty() {
            return None;
        }
        for part in &parts {
            p.push(part);
        }
        let real = fs::canonicalize(&p).ok()?;
        if !real.starts_with(&self.canonical) || !real.is_file() {
            return None;
        }
        Some(Found {
            real,
            name: parts.pop().unwrap_or_default(),
        })
    }
}

/// A file inside the root: where it really is, and the name the request used
/// (which decides the content type, as in the dev server).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) real: PathBuf,
    pub(crate) name: String,
}

/// What a request path maps to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Resolved {
    File(Found),
    Redirect(String),
    NotFound,
    /// The target is malformed (bad percent-encoding or not UTF-8).
    BadRequest,
}

/// Percent-decode one time. `None` for a bad escape or a result that is not
/// UTF-8.
fn decode(path: &str) -> Option<String> {
    let b = path.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hi = hex(*b.get(i + 1)?)?;
            let lo = hex(*b.get(i + 2)?)?;
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Percent-encode one path segment for a `Location` header. Leaves the same
/// characters alone that a browser leaves in a path.
pub(crate) fn encode_segment(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for &b in seg.as_bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
            );
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn location(segments: &[String], trailing_slash: bool) -> String {
    let mut s = String::from("/");
    s.push_str(
        &segments
            .iter()
            .map(|p| encode_segment(p))
            .collect::<Vec<_>>()
            .join("/"),
    );
    if trailing_slash && !segments.is_empty() {
        s.push('/');
    }
    s
}

/// Map the path part of a request target (no query) to a file.
///
/// In order, as the dev server does:
/// 1. a file that exists at the path (not for a trailing slash);
/// 2. `/dir/` serves `/dir/index.html`; `/page` serves `/page.html`;
/// 3. `/dir` with only `dir/index.html` redirects to `/dir/` (301);
/// 4. `/page/` with only `page.html` redirects to `/page` (301);
/// 5. otherwise 404.
pub(crate) fn resolve(root: &Root, raw_path: &str) -> Resolved {
    let Some(decoded) = decode(raw_path) else {
        return Resolved::BadRequest;
    };
    if decoded.contains('\0') || decoded.contains('\\') {
        return Resolved::NotFound;
    }
    let trailing_slash = decoded.ends_with('/');
    let mut segments: Vec<String> = Vec::new();
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => return Resolved::NotFound,
            s => segments.push(s.to_string()),
        }
    }
    // Compared without case: the folder may be on a case-insensitive file system.
    if segments
        .first()
        .is_some_and(|s| s.eq_ignore_ascii_case(crate::HIDDEN_DIR))
    {
        return Resolved::NotFound;
    }

    if !trailing_slash && let Some(f) = root.file(&segments, None) {
        return Resolved::File(f);
    }
    let index = root.file(&segments, Some(("index.html", true)));
    let html = root.file(&segments, Some((".html", false)));

    match (index, html) {
        (Some(i), _) if trailing_slash => Resolved::File(i),
        (_, Some(h)) if !trailing_slash => Resolved::File(h),
        (Some(_), _) if !trailing_slash => Resolved::Redirect(location(&segments, true)),
        (_, Some(_)) => Resolved::Redirect(location(&segments, false)),
        _ => Resolved::NotFound,
    }
}

/// The 404 page the output folder provides, if it has one.
pub(crate) fn not_found_page(root: &Root) -> Option<Found> {
    root.file(&["404.html".to_string()], None)
}
