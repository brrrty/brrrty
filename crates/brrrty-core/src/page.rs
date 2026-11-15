//! A discovered template file: its slug, date, data, and body.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value};

use crate::problem::Problem;

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

/// Splits a leading `YYYY-MM-DD-` off `stem`, returning the date text and the rest.
fn split_date_prefix(stem: &str) -> Option<(&str, &str)> {
    let prefix = stem.get(..11)?;
    let b = prefix.as_bytes();
    let shaped = b[10] == b'-'
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..10]
            .iter()
            .enumerate()
            .all(|(i, c)| matches!(i, 4 | 7) || c.is_ascii_digit());
    shaped.then(|| (&prefix[..10], &stem[11..]))
}

fn stem(path: &Path) -> &str {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("")
}

/// The slug of a file: its stem without a leading date, or the parent folder
/// name for `index.*` files (`""` at the input root).
pub fn file_slug(rel_to_input: &Path) -> String {
    let stem = stem(rel_to_input);
    if stem == "index" {
        return rel_to_input
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
    }
    split_date_prefix(stem)
        .map_or(stem, |(_, rest)| rest)
        .to_string()
}

fn midnight(date: NaiveDate) -> DateTime<Utc> {
    date.and_hms_opt(0, 0, 0)
        .expect("midnight is valid")
        .and_utc()
}

/// Resolves a page's date: front matter `date`, then a filename date prefix,
/// then `created`.
pub fn resolve_date(
    front_matter: &Map<String, Value>,
    rel_to_input: &Path,
    created: SystemTime,
) -> Result<DateTime<Utc>, Problem> {
    if let Some(value) = front_matter.get("date") {
        let invalid = || {
            Problem::error("date-invalid", format!("cannot parse date {value}"))
                .with_help("use YYYY-MM-DD or an RFC 3339 timestamp")
        };
        let text = value.as_str().ok_or_else(invalid)?.trim();
        if let Ok(d) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
            return Ok(midnight(d));
        }
        return DateTime::parse_from_rfc3339(text)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|_| invalid());
    }
    if let Some((prefix, _)) = split_date_prefix(stem(rel_to_input))
        && let Ok(d) = NaiveDate::parse_from_str(prefix, "%Y-%m-%d")
    {
        return Ok(midnight(d));
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
            resolve_date(&fm, Path::new("2026-11-08-x.md"), epoch)
                .unwrap()
                .to_rfc3339(),
            "2020-01-02T00:00:00+00:00"
        );
        assert_eq!(
            resolve_date(&Map::new(), Path::new("2026-11-08-x.md"), epoch)
                .unwrap()
                .to_rfc3339(),
            "2026-11-08T00:00:00+00:00"
        );
        assert_eq!(
            resolve_date(&Map::new(), Path::new("x.md"), epoch)
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
            resolve_date(&fm, Path::new("x.md"), epoch)
                .unwrap()
                .to_rfc3339(),
            "2020-01-02T01:04:05+00:00"
        );
        let bad: Map<_, _> = serde_json::from_str(r#"{"date":"nope"}"#).unwrap();
        assert_eq!(
            resolve_date(&bad, Path::new("x.md"), epoch)
                .unwrap_err()
                .code,
            "date-invalid"
        );
    }
}
