//! A discovered template file: its slug, date, data, and body.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use chrono::{DateTime, NaiveDate, Utc};
use regex::Regex;
use serde_json::{Map, Value};

use crate::problem::{Problem, codes};

/// A template file with its front matter parsed.
#[derive(Debug, Clone)]
pub struct Page {
    /// Path relative to the input folder.
    pub input_path: PathBuf,
    pub file_slug: String,
    pub date: DateTime<Utc>,
    pub data: Map<String, Value>,
    pub body: String,
}

static SLUG_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}-(.*)").expect("valid regex"));
static PATH_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4}-\d{2}-\d{2})").expect("valid regex"));

fn stem(path: &Path) -> &str {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("")
}

/// 11ty's date strip: the text after the first `YYYY-MM-DD-` anywhere in `name`.
fn strip_date(name: &str) -> &str {
    SLUG_DATE
        .captures(name)
        .and_then(|c| c.get(1))
        .map_or(name, |m| m.as_str())
}

/// The slug of a file like 11ty: its stem with everything up to and including the first
/// `YYYY-MM-DD-` removed, or the parent folder name (same strip) for `index.*` files
/// (`""` at the input root).
pub fn file_slug(rel_to_input: &Path) -> String {
    let stem = strip_date(stem(rel_to_input));
    if stem == "index" {
        return rel_to_input
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .map_or("", strip_date)
            .to_string();
    }
    stem.to_string()
}

fn midnight(date: NaiveDate) -> DateTime<Utc> {
    date.and_hms_opt(0, 0, 0)
        .expect("midnight is valid")
        .and_utc()
}

/// Parses RFC 3339 or YAML-style timestamps: `T` or space separator,
/// optional fractional seconds, optional zone (none means UTC).
fn parse_timestamp(text: &str) -> Option<DateTime<Utc>> {
    if let Ok(d) = DateTime::parse_from_rfc3339(text) {
        return Some(d.with_timezone(&Utc));
    }
    let normalized = match text.as_bytes().get(10) {
        Some(b' ') => format!("{}T{}", &text[..10], text[11..].trim_start()),
        _ => text.to_string(),
    };
    // Allow a space before the zone: `10:00:00 +02:00`.
    let normalized = match normalized.rfind(" ") {
        Some(i) => format!("{}{}", &normalized[..i], &normalized[i + 1..]),
        None => normalized,
    };
    if let Ok(d) = DateTime::parse_from_rfc3339(&normalized) {
        return Some(d.with_timezone(&Utc));
    }
    chrono::NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .map(|d| d.and_utc())
}

/// Resolves a page's date: front matter `date` (a date, a timestamp, or one of 11ty's
/// `Last Modified` / `Created` keywords), then the first `YYYY-MM-DD` anywhere in the
/// project-root-relative `input_path`, then `created`. A path date that is not a real
/// date is a `date-invalid` problem for this page (11ty aborts the whole build).
pub fn resolve_date(
    front_matter: &Map<String, Value>,
    input_path: &Path,
    created: SystemTime,
    modified: SystemTime,
) -> Result<DateTime<Utc>, Problem> {
    if let Some(value) = front_matter.get("date") {
        let invalid = || {
            Problem::error(codes::DATE_INVALID, format!("cannot parse date {value}"))
                .with_help("use YYYY-MM-DD or an RFC 3339 timestamp")
        };
        let text = value.as_str().ok_or_else(invalid)?.trim();
        // 11ty keywords. Git history lookups come later, so the `git` forms use file times.
        match text {
            "Last Modified" | "git Last Modified" => return Ok(modified.into()),
            "Created" | "git Created" => return Ok(created.into()),
            _ => {}
        }
        if let Ok(d) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
            return Ok(midnight(d));
        }
        return parse_timestamp(text).ok_or_else(invalid);
    }
    let joined = input_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if let Some(m) = PATH_DATE.find(&joined) {
        return NaiveDate::parse_from_str(m.as_str(), "%Y-%m-%d")
            .map(midnight)
            .map_err(|_| {
                Problem::error(
                    codes::DATE_INVALID,
                    format!("date `{}` in the file path is not a real date", m.as_str()),
                )
                .with_help("add a date: to the front matter, or rename the file")
            });
    }
    Ok(created.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(
            file_slug(Path::new("posts/2026/2026-11-08-introducing-cuppa.md")),
            "introducing-cuppa"
        );
        assert_eq!(file_slug(Path::new("sub/deep/index.html")), "deep");
        assert_eq!(file_slug(Path::new("index.md")), "");
        assert_eq!(file_slug(Path::new("about.md")), "about");
    }

    #[test]
    fn date_precedence() {
        let epoch = SystemTime::UNIX_EPOCH;
        let fm: Map<_, _> = serde_json::from_str(r#"{"date":"2020-01-02"}"#).unwrap();
        assert_eq!(
            resolve_date(&fm, Path::new("2026-11-08-x.md"), epoch, epoch)
                .unwrap()
                .to_rfc3339(),
            "2020-01-02T00:00:00+00:00"
        );
        assert_eq!(
            resolve_date(&Map::new(), Path::new("2026-11-08-x.md"), epoch, epoch)
                .unwrap()
                .to_rfc3339(),
            "2026-11-08T00:00:00+00:00"
        );
        assert_eq!(
            resolve_date(&Map::new(), Path::new("x.md"), epoch, epoch)
                .unwrap()
                .timestamp(),
            0
        );
    }

    #[test]
    fn rfc3339_and_invalid_dates() {
        let epoch = SystemTime::UNIX_EPOCH;
        let fm: Map<_, _> =
            serde_json::from_str(r#"{"date":"2020-01-02T03:04:05+02:00"}"#).unwrap();
        assert_eq!(
            resolve_date(&fm, Path::new("x.md"), epoch, epoch)
                .unwrap()
                .to_rfc3339(),
            "2020-01-02T01:04:05+00:00"
        );
        let bad: Map<_, _> = serde_json::from_str(r#"{"date":"nope"}"#).unwrap();
        assert_eq!(
            resolve_date(&bad, Path::new("x.md"), epoch, epoch)
                .unwrap_err()
                .code,
            codes::DATE_INVALID
        );
    }

    #[test]
    fn slugs_match_11ty() {
        assert_eq!(file_slug(Path::new("posts/foo2024-03-04-bar.md")), "bar");
        assert_eq!(
            file_slug(Path::new("blog/2025-05-05-trip/index.md")),
            "trip"
        );
        assert_eq!(file_slug(Path::new("posts/2026-01-01-index.md")), "posts");
        assert_eq!(file_slug(Path::new("2026-01-01-index.md")), "");
        assert_eq!(file_slug(Path::new("2026-13-45-bad.md")), "bad");
        assert_eq!(
            file_slug(Path::new("posts/2026/2026-11-08-introducing-cuppa.md")),
            "introducing-cuppa"
        );
    }

    #[test]
    fn dates_from_anywhere_in_path() {
        let t = SystemTime::UNIX_EPOCH;
        let none = Map::new();
        let d = |p: &str| resolve_date(&none, Path::new(p), t, t).map(|d| d.to_rfc3339());
        assert_eq!(
            d("src/posts/2026-01-02/hello.md").unwrap(),
            "2026-01-02T00:00:00+00:00"
        );
        assert_eq!(
            d("src/posts/foo2024-03-04-bar.md").unwrap(),
            "2024-03-04T00:00:00+00:00"
        );
        assert_eq!(
            d("src/2020-01-01/b/2021-02-02-x.md").unwrap(),
            "2020-01-01T00:00:00+00:00"
        );
        assert_eq!(
            d("src/bad/2026-13-45-x/page.md").unwrap_err().code,
            codes::DATE_INVALID
        );
        assert_eq!(d("src/plain.md").unwrap(), "1970-01-01T00:00:00+00:00");
    }

    #[test]
    fn front_matter_date_beats_bad_path_date() {
        let t = SystemTime::UNIX_EPOCH;
        let fm: Map<_, _> = serde_json::from_str(r#"{"date":"2020-01-01"}"#).unwrap();
        let d = resolve_date(&fm, Path::new("src/2026-13-45-x/page.md"), t, t).unwrap();
        assert_eq!(d.to_rfc3339(), "2020-01-01T00:00:00+00:00");
    }

    #[test]
    fn date_keywords_use_file_times() {
        let created = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
        let modified = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2000);
        let secs = |kw: &str| {
            let fm: Map<_, _> = serde_json::from_value(serde_json::json!({ "date": kw })).unwrap();
            resolve_date(&fm, Path::new("x.md"), created, modified)
                .unwrap()
                .timestamp()
        };
        assert_eq!(secs("Last Modified"), 2000);
        assert_eq!(secs("git Last Modified"), 2000);
        assert_eq!(secs("Created"), 1000);
        assert_eq!(secs("git Created"), 1000);
    }

    #[test]
    fn timestamps_with_time() {
        let epoch = SystemTime::UNIX_EPOCH;
        let date = |s: &str| {
            let fm: Map<_, _> = serde_json::from_value(serde_json::json!({ "date": s })).unwrap();
            resolve_date(&fm, Path::new("x.md"), epoch, epoch)
                .unwrap()
                .to_rfc3339()
        };
        assert_eq!(date("2020-01-02 10:00:00"), "2020-01-02T10:00:00+00:00");
        assert_eq!(date("2020-01-02T10:00:00"), "2020-01-02T10:00:00+00:00");
        assert_eq!(
            date("2020-01-02 10:00:00.5"),
            "2020-01-02T10:00:00.500+00:00"
        );
        assert_eq!(
            date("2020-01-02 10:00:00 +02:00"),
            "2020-01-02T08:00:00+00:00"
        );
        assert_eq!(
            date("2020-01-02T10:00:00+02:00"),
            "2020-01-02T08:00:00+00:00"
        );
        assert_eq!(date("2020-01-02T10:00:00Z"), "2020-01-02T10:00:00+00:00");
    }
}
