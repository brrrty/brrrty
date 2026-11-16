//! A discovered template file: its slug, date, data, and body.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use brrrty_js::Value;
use chrono::{DateTime, NaiveDate, Utc};
use regex::Regex;

use crate::frontmatter::Data;
use crate::problem::{Problem, codes};

/// A template file with its front matter parsed.
#[derive(Debug, Clone)]
pub struct Page {
    /// Path relative to the input folder.
    pub input_path: PathBuf,
    pub file_slug: String,
    pub date: DateTime<Utc>,
    pub data: Data,
    pub body: String,
}

// JavaScript's `\d` is ASCII-only; Rust's is Unicode-aware, so spell the digits out.
static SLUG_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9]{4}-[0-9]{2}-[0-9]{2}-(.*)").expect("valid regex"));
static PATH_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([0-9]{4}-[0-9]{2}-[0-9]{2})").expect("valid regex"));

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

/// 11ty's `page.filePathStem`: the path from the input folder without its
/// extension, with the file name's date prefix removed as in [`file_slug`]
/// (but `index` kept).
pub fn file_path_stem(rel_to_input: &Path) -> String {
    let mut parts: Vec<String> = rel_to_input
        .parent()
        .into_iter()
        .flat_map(|p| p.components())
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.push(strip_date(stem(rel_to_input)).to_string());
    format!("/{}", parts.join("/"))
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

/// Resolves a page's date as 11ty's `getMappedDate` does: a truthy `date` in
/// the data (a date, a timestamp, or one of the `Last Modified` / `Created`
/// keywords, in any case), then the first `YYYY-MM-DD` anywhere in the
/// project-root-relative `input_path`, then `created`. A falsy `date` (`null`,
/// `""`, `0`, `false`, `NaN`) counts as none. A date that is not a real date
/// is a `date-invalid` problem for this page (11ty aborts the whole build).
pub fn resolve_date(
    data: &Data,
    input_path: &Path,
    created: SystemTime,
    modified: SystemTime,
) -> Result<DateTime<Utc>, Problem> {
    if let Some(value) = data.get("date").filter(|v| v.js_truthy()) {
        let invalid = || {
            Problem::error(
                codes::DATE_INVALID,
                format!("cannot parse date {}", value.to_js_string()),
            )
            .with_help("use YYYY-MM-DD or an RFC 3339 timestamp")
        };
        let text = match value {
            // An unquoted YAML timestamp is already an instant.
            Value::Date(d) => return Ok(d.with_timezone(&Utc)),
            // Luxon's ISO parse allows no space around the date, and the
            // keywords are exact: 11ty refuses ` 2020-01-01` and `Created `.
            Value::String(s) if s.trim() != s.as_ref() => return Err(invalid()),
            Value::String(s) => s.as_ref(),
            _ => return Err(invalid()),
        };
        // 11ty keywords, in any case. Git history lookups come later, so the
        // `git` forms use file times.
        match text.to_lowercase().as_str() {
            "last modified" | "git last modified" => return Ok(modified.into()),
            "created" | "git created" => return Ok(created.into()),
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

    fn fm_date(s: &str) -> Data {
        let mut m = Data::new();
        m.insert("date".into(), Value::String(s.into()));
        m
    }

    #[test]
    fn slugs() {
        assert_eq!(
            file_slug(Path::new("posts/2026/2026-11-08-hello-world.md")),
            "hello-world"
        );
        assert_eq!(file_slug(Path::new("sub/deep/index.html")), "deep");
        assert_eq!(file_slug(Path::new("index.md")), "");
        assert_eq!(file_slug(Path::new("about.md")), "about");
    }

    /// Probed: `/index`, `/posts/index`, `/posts/2026/first-post`.
    #[test]
    fn file_path_stems() {
        assert_eq!(file_path_stem(Path::new("index.liquid")), "/index");
        assert_eq!(
            file_path_stem(Path::new("posts/index.liquid")),
            "/posts/index"
        );
        assert_eq!(
            file_path_stem(Path::new("posts/2026/2026-03-04-first-post.md")),
            "/posts/2026/first-post"
        );
    }

    #[test]
    fn date_precedence() {
        let epoch = SystemTime::UNIX_EPOCH;
        let fm = fm_date("2020-01-02");
        assert_eq!(
            resolve_date(&fm, Path::new("2026-11-08-x.md"), epoch, epoch)
                .unwrap()
                .to_rfc3339(),
            "2020-01-02T00:00:00+00:00"
        );
        assert_eq!(
            resolve_date(&Data::new(), Path::new("2026-11-08-x.md"), epoch, epoch)
                .unwrap()
                .to_rfc3339(),
            "2026-11-08T00:00:00+00:00"
        );
        assert_eq!(
            resolve_date(&Data::new(), Path::new("x.md"), epoch, epoch)
                .unwrap()
                .timestamp(),
            0
        );
    }

    #[test]
    fn rfc3339_and_invalid_dates() {
        let epoch = SystemTime::UNIX_EPOCH;
        let fm = fm_date("2020-01-02T03:04:05+02:00");
        assert_eq!(
            resolve_date(&fm, Path::new("x.md"), epoch, epoch)
                .unwrap()
                .to_rfc3339(),
            "2020-01-02T01:04:05+00:00"
        );
        let bad = fm_date("nope");
        assert_eq!(
            resolve_date(&bad, Path::new("x.md"), epoch, epoch)
                .unwrap_err()
                .code,
            codes::DATE_INVALID
        );
    }

    /// Probed: 11ty skips a falsy `date` (`getMappedDate` tests
    /// `if (dateValue)`): the path date, else the file's creation time.
    #[test]
    fn falsy_dates_count_as_none() {
        let created = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400);
        let modified = SystemTime::UNIX_EPOCH;
        for v in [
            Value::Null,
            Value::String("".into()),
            Value::Number(0.0),
            Value::Number(f64::NAN),
            Value::Bool(false),
        ] {
            let mut d = Data::new();
            d.insert("date".into(), v.clone());
            let got = |p: &str| {
                resolve_date(&d, Path::new(p), created, modified)
                    .unwrap()
                    .to_rfc3339()
            };
            assert_eq!(got("2021-03-04/x.md"), "2021-03-04T00:00:00+00:00", "{v:?}");
            assert_eq!(got("x.md"), "1970-01-02T00:00:00+00:00", "{v:?}");
        }
        // A truthy non-date is still refused, as 11ty does.
        let mut d = Data::new();
        d.insert("date".into(), Value::Number(1_700_000_000_000.0));
        let err = resolve_date(&d, Path::new("2021-03-04/x.md"), created, modified).unwrap_err();
        assert_eq!(err.code, codes::DATE_INVALID);
    }

    /// Probed: the keywords match in any case, and nothing is trimmed (11ty
    /// refuses ` 2020-01-01` and `Last Modified `).
    #[test]
    fn keywords_match_in_any_case_and_untrimmed() {
        let created = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400);
        let modified = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(172_800);
        let got = |s: &str| resolve_date(&fm_date(s), Path::new("x.md"), created, modified);
        assert_eq!(got("last modified").unwrap().timestamp(), 172_800);
        assert_eq!(got("CREATED").unwrap().timestamp(), 86_400);
        assert_eq!(got("Git Created").unwrap().timestamp(), 86_400);
        for bad in [" 2020-01-01", "2020-01-01 ", "Last Modified "] {
            assert_eq!(got(bad).unwrap_err().code, codes::DATE_INVALID, "{bad:?}");
        }
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
            file_slug(Path::new("posts/2026/2026-11-08-hello-world.md")),
            "hello-world"
        );
    }

    #[test]
    fn dates_from_anywhere_in_path() {
        let t = SystemTime::UNIX_EPOCH;
        let none = Data::new();
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
    fn only_ascii_digits_make_a_date() {
        let t = SystemTime::UNIX_EPOCH;
        let none = Data::new();
        // Arabic-Indic and fullwidth digits are not `\d` in JavaScript.
        for p in [
            "posts/\u{662}\u{660}\u{662}\u{666}-\u{660}\u{661}-\u{660}\u{662}-hello.md",
            "posts/\u{ff12}\u{ff10}\u{ff12}\u{ff16}-\u{ff10}\u{ff11}-\u{ff10}\u{ff12}-hello.md",
        ] {
            let d = resolve_date(&none, Path::new(p), t, t).unwrap();
            assert_eq!(d.to_rfc3339(), "1970-01-01T00:00:00+00:00", "{p}");
            let stem = Path::new(p).file_stem().unwrap().to_str().unwrap();
            assert_eq!(file_slug(Path::new(p)), stem, "{p}");
        }
    }

    #[test]
    fn front_matter_date_beats_bad_path_date() {
        let t = SystemTime::UNIX_EPOCH;
        let fm = fm_date("2020-01-01");
        let d = resolve_date(&fm, Path::new("src/2026-13-45-x/page.md"), t, t).unwrap();
        assert_eq!(d.to_rfc3339(), "2020-01-01T00:00:00+00:00");
    }

    #[test]
    fn date_keywords_use_file_times() {
        let created = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
        let modified = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2000);
        let secs = |kw: &str| {
            let fm = fm_date(kw);
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
            let fm = fm_date(s);
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
