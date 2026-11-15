//! Front matter splitting and YAML parsing.

use serde_json::{Map, Value};

use crate::problem::Problem;

/// Splits `source` into its front matter map and the body that follows.
///
/// Front matter is a first line of `---` (optionally followed by whitespace)
/// through the next `---` line. Both `\n` and `\r\n` are accepted; the body is
/// returned untouched. Without front matter the map is empty and the body is
/// the whole source.
pub fn split_front_matter(source: &str) -> Result<(Map<String, Value>, &str), Problem> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let Some((first, rest)) = next_line(source) else {
        return Ok((Map::new(), source));
    };
    if !is_delimiter(first) {
        return Ok((Map::new(), source));
    }

    let mut offset = 0;
    let mut remaining = rest;
    while let Some((line, after)) = next_line(remaining) {
        if is_delimiter(line) {
            let yaml = &rest[..offset];
            return Ok((parse_yaml(yaml)?, after));
        }
        offset += remaining.len() - after.len();
        remaining = after;
    }
    Err(Problem::error(
        "front-matter-invalid",
        "front matter is missing its closing `---` line",
    )
    .at(1, 1)
    .with_help("add a line containing only `---` after the front matter"))
}

/// Returns the next line (without its line ending) and the text after it.
fn next_line(text: &str) -> Option<(&str, &str)> {
    if text.is_empty() {
        return None;
    }
    match text.find('\n') {
        Some(i) => Some((text[..i].trim_end_matches('\r'), &text[i + 1..])),
        None => Some((text, "")),
    }
}

fn is_delimiter(line: &str) -> bool {
    line.strip_prefix("---")
        .is_some_and(|tail| tail.chars().all(char::is_whitespace))
}

fn parse_yaml(yaml: &str) -> Result<Map<String, Value>, Problem> {
    if yaml.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_norway::from_str::<Value>(yaml) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(Value::Null) => Ok(Map::new()),
        // Lists and scalars are not data; 11ty renders the page with none.
        Ok(_) => Ok(Map::new()),
        Err(e) => {
            // +1 for the opening `---` line.
            let (line, column) = e.location().map_or((2, 1), |l| (l.line() + 1, l.column()));
            Err(Problem::error(
                "front-matter-invalid",
                format!("invalid YAML front matter: {}", e.to_string().trim()),
            )
            .at(line, column))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_lf_and_crlf_identically() {
        let (a, body_a) = split_front_matter("---\ntitle: Hi\n---\nBody\n").unwrap();
        let (b, body_b) = split_front_matter("---\r\ntitle: Hi\r\n---\r\nBody\r\n").unwrap();
        assert_eq!(a, b);
        assert_eq!(a["title"], "Hi");
        assert_eq!(body_a, "Body\n");
        assert_eq!(body_b, "Body\r\n");
    }

    #[test]
    fn no_front_matter_returns_whole_source() {
        let (m, body) = split_front_matter("no front matter\n").unwrap();
        assert!(m.is_empty());
        assert_eq!(body, "no front matter\n");
    }

    #[test]
    fn invalid_yaml_reports_file_line() {
        let p = split_front_matter("---\ntitle: [unclosed\n---\nbad\n").unwrap_err();
        assert_eq!(p.code, "front-matter-invalid");
        assert_eq!(p.line, Some(3));
    }

    #[test]
    fn ignores_unknown_keys_like_disqus() {
        let (m, _) = split_front_matter("---\ndisqus: true\ntitle: X\n---\n").unwrap();
        assert_eq!(m["disqus"], true);
    }

    #[test]
    fn yaml_dates_become_strings() {
        let (m, _) = split_front_matter("---\ndate: 2020-01-02\n---\n").unwrap();
        assert_eq!(m["date"], "2020-01-02");
    }

    #[test]
    fn empty_front_matter_and_trailing_whitespace_delimiter() {
        let (m, body) = split_front_matter("---  \n---\nx").unwrap();
        assert!(m.is_empty());
        assert_eq!(body, "x");
    }

    #[test]
    fn unclosed_front_matter_is_an_error() {
        let p = split_front_matter("---\ntitle: x\n").unwrap_err();
        assert_eq!(p.code, "front-matter-invalid");
        assert_eq!(p.line, Some(1));
        assert!(p.message.contains("closing"));
    }

    #[test]
    fn non_mapping_yaml_is_empty_data() {
        let (m, body) = split_front_matter("---\n- a\n- b\n---\nlist fm\n").unwrap();
        assert!(m.is_empty());
        assert_eq!(body, "list fm\n");
        let (m, body) = split_front_matter("---\njust a scalar\n---\nx").unwrap();
        assert!(m.is_empty());
        assert_eq!(body, "x");
    }

    #[test]
    fn bom_is_stripped() {
        let (m, body) = split_front_matter("\u{feff}---\ntitle: bom\n---\nbom body").unwrap();
        assert_eq!(m["title"], "bom");
        assert_eq!(body, "bom body");
        let (m, body) = split_front_matter("\u{feff}plain").unwrap();
        assert!(m.is_empty());
        assert_eq!(body, "plain");
    }

    #[test]
    fn closing_delimiter_without_body_newline() {
        let (m, body) = split_front_matter("---\na: 1\n---").unwrap();
        assert_eq!(m["a"], 1);
        assert_eq!(body, "");
    }
}
