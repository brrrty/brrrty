//! `HighlightLines.js` and `HighlightLinesGroup.js`: which lines a range
//! like `1-2,4` marks, and the markup around each line.

use super::Error;

/// The most lines one range may mark. 11ty sets one object key per line,
/// so a range of millions takes Node seconds and more; Brrrty refuses.
const MAX_RANGE: f64 = 1_000_000.0;

/// `parseInt(text, 10)`: leading whitespace, a sign, then the longest run
/// of digits; `NaN` without one.
fn parse_int(text: &str) -> f64 {
    let s = text.trim_start_matches(brrrty_js::is_js_whitespace);
    let (negative, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return f64::NAN;
    }
    let n: f64 = s[..digits].parse().unwrap_or(f64::NAN);
    if negative { -n } else { n }
}

/// `HighlightLines`: the lines a comma-separated list of ranges marks,
/// counted from 0.
#[derive(Debug, Clone, Default)]
pub(crate) struct HighlightLines {
    /// Inclusive ranges: `for (j = start; j <= end; j++) hash[j] = true`.
    ranges: Vec<(f64, f64)>,
}

impl HighlightLines {
    pub(crate) fn new(range_str: &str) -> Result<Self, Error> {
        let mut ranges = Vec::new();
        if range_str.is_empty() {
            return Ok(Self { ranges });
        }
        for range in range_str.split(',') {
            let range = range.trim_matches(brrrty_js::is_js_whitespace);
            let mut start_finish = range.split('-');
            let start = parse_int(start_finish.next().unwrap_or(""));
            // `parseInt(startFinish[1] || start, 10)`: a missing or empty
            // end parses the start's own text.
            let end = match start_finish.next() {
                Some(e) if !e.is_empty() => parse_int(e),
                _ => parse_int(&brrrty_js::Value::Number(start).to_js_string()),
            };
            // `j <= k` is false when either is NaN.
            if start.partial_cmp(&end).is_none_or(|o| o.is_gt()) {
                continue;
            }
            if end >= 9_007_199_254_740_992.0 {
                return Err(Error::Unsupported(format!(
                    "the line range `{range}` never ends in 11ty: past 2^53, adding 1 to a line number leaves it as it was"
                )));
            }
            if end - start >= MAX_RANGE {
                return Err(Error::Unsupported(format!(
                    "the line range `{range}` marks more than {MAX_RANGE} lines, which takes 11ty too long to reproduce"
                )));
            }
            ranges.push((start, end));
        }
        Ok(Self { ranges })
    }

    pub(crate) fn is_highlighted(&self, line: usize) -> bool {
        let n = line as f64;
        self.ranges.iter().any(|&(a, b)| a <= n && n <= b)
    }
}

/// `HighlightLinesGroup`: one range list marks lines, or two (split by
/// `delimiter`) mark added and removed lines.
#[derive(Debug, Clone)]
pub(crate) struct HighlightLinesGroup {
    highlights: HighlightLines,
    add: HighlightLines,
    remove: HighlightLines,
}

impl HighlightLinesGroup {
    pub(crate) fn new(text: &str, delimiter: &str) -> Result<Self, Error> {
        let split: Vec<&str> = text.split(delimiter).collect();
        let one = |n: usize, i: usize| if split.len() == n { split[i] } else { "" };
        Ok(Self {
            highlights: HighlightLines::new(one(1, 0))?,
            add: HighlightLines::new(one(2, 0))?,
            remove: HighlightLines::new(one(2, 1))?,
        })
    }

    /// `getLineMarkup(lineNumber, line)`.
    pub(crate) fn line_markup(&self, number: usize, line: &str) -> String {
        let (before, after) = if self.highlights.is_highlighted(number) {
            (
                r#"<mark class="highlight-line highlight-line-active">"#,
                "</mark>",
            )
        } else if self.add.is_highlighted(number) {
            (
                r#"<ins class="highlight-line highlight-line-add">"#,
                "</ins>",
            )
        } else if self.remove.is_highlighted(number) {
            (
                r#"<del class="highlight-line highlight-line-remove">"#,
                "</del>",
            )
        } else {
            (r#"<span class="highlight-line">"#, "</span>")
        };
        // A line with more `<span` than `</span` (or fewer), where a token
        // runs over lines, is left as it is.
        if line.matches("<span").count() != line.matches("</span").count() {
            return line.to_string();
        }
        format!("{before}{line}{after}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(ranges: &str) -> Vec<usize> {
        let h = HighlightLines::new(ranges).unwrap();
        (0..12).filter(|&n| h.is_highlighted(n)).collect()
    }

    /// What `new HighlightLines(ranges)` marked in Node, lines 0 to 11.
    #[test]
    fn ranges_parse_as_node_parses_them() {
        assert_eq!(marked(""), Vec::<usize>::new());
        assert_eq!(marked("0"), [0]);
        assert_eq!(marked("1-2,4"), [1, 2, 4]);
        assert_eq!(marked(" 3 , 5-"), [3, 5]);
        assert_eq!(marked("x,2-1, 1"), [1]);
        assert_eq!(marked("2a-3b"), [2, 3]);
        assert_eq!(marked("+1"), [1]);
        assert_eq!(marked("-3"), Vec::<usize>::new());
        assert_eq!(marked("1.9"), [1]);
        assert_eq!(marked("4-6-1"), [4, 5, 6]);
        assert_eq!(marked("1e1"), [1]);
        assert_eq!(marked("1000000000000000000000"), Vec::<usize>::new());
        assert_eq!(marked("\u{a0}7"), [7]);
    }

    #[test]
    fn huge_ranges_are_refused() {
        assert!(HighlightLines::new("0-5000000").is_err());
        assert!(HighlightLines::new("9007199254740993").is_err());
        assert!(HighlightLines::new("5-1-5000000").is_ok());
    }

    #[test]
    fn a_group_splits_once() {
        let g = HighlightLinesGroup::new("0 2", " ").unwrap();
        assert!(g.line_markup(0, "a").starts_with("<ins"));
        assert!(g.line_markup(2, "a").starts_with("<del"));
        assert_eq!(
            g.line_markup(1, "a"),
            r#"<span class="highlight-line">a</span>"#
        );
        let three = HighlightLinesGroup::new("0/1/2", "/").unwrap();
        assert!(three.line_markup(0, "a").starts_with("<span"));
        let g = HighlightLinesGroup::new("0", " ").unwrap();
        assert_eq!(g.line_markup(0, "<span>x"), "<span>x");
        assert_eq!(
            g.line_markup(0, "</span><span>x"),
            r#"<mark class="highlight-line highlight-line-active"></span><span>x</mark>"#
        );
    }
}
