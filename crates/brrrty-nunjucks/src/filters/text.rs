//! The filters that Nunjucks writes with regular expressions, done by hand.
//!
//! Each function does exactly what one fixed pattern does in Nunjucks 3.2.4
//! (`filters.js`), so none needs a regular expression engine. The patterns
//! are quoted where they are used. `\s` is JavaScript's white space
//! ([`brrrty_js::is_js_whitespace`]), `\w` is `[A-Za-z0-9_]`, and the `i`
//! flag only widens a letter class to both cases of ASCII.

use brrrty_js::is_js_whitespace;

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `str.replace(/^\s*|\s*$/g, '')`.
pub(super) fn trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// `str.replace(/<\/?([a-z][a-z0-9]*)\b[^>]*>|<!--[\s\S]*?-->/gi, '')`.
pub(super) fn remove_tags(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(s.len());
    // Where the next `>` or `-->` is, found once: a failed attempt means
    // there is none further on, and every later attempt fails too.
    let mut no_more_gt = false;
    let mut no_more_comment_end = false;
    let mut i = 0;
    while i < n {
        if chars[i] == '<'
            && let Some(end) = tag_end(&chars, i, &mut no_more_gt, &mut no_more_comment_end)
        {
            i = end;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// The index after the tag or comment that starts at `i` (a `<`), if one
/// does.
fn tag_end(
    chars: &[char],
    i: usize,
    no_more_gt: &mut bool,
    no_more_comment_end: &mut bool,
) -> Option<usize> {
    let n = chars.len();
    // `<!--[\s\S]*?-->`
    if chars[i + 1..].starts_with(&['!', '-', '-']) {
        if *no_more_comment_end {
            return None;
        }
        let from = i + 4;
        let found = (from..n.saturating_sub(2)).find(|&j| chars[j..].starts_with(&['-', '-', '>']));
        return match found {
            Some(j) => Some(j + 3),
            None => {
                *no_more_comment_end = true;
                None
            }
        };
    }
    // `<\/?([a-z][a-z0-9]*)\b[^>]*>`
    let mut j = i + 1;
    if chars.get(j) == Some(&'/') {
        j += 1;
    }
    if !chars.get(j).is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    while chars.get(j).is_some_and(|c| c.is_ascii_alphanumeric()) {
        j += 1;
    }
    // `\b` after a word character: the next one must not be a word
    // character. Giving characters back never helps, since both sides of
    // the boundary would be word characters.
    if chars.get(j).is_some_and(|c| is_word(*c)) {
        return None;
    }
    if *no_more_gt {
        return None;
    }
    match chars[j..].iter().position(|c| *c == '>') {
        Some(p) => Some(j + p + 1),
        None => {
            *no_more_gt = true;
            None
        }
    }
}

/// `s.replace(/\s+/gi, ' ')`.
pub(super) fn squash_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_run = false;
    for c in s.chars() {
        if is_js_whitespace(c) {
            if !in_run {
                out.push(' ');
            }
            in_run = true;
        } else {
            in_run = false;
            out.push(c);
        }
    }
    out
}

/// The `preserveLinebreaks` chain of `striptags`:
///
/// ```text
/// .replace(/^ +| +$/gm, '')     // leading and trailing spaces of each line
/// .replace(/ +/g, ' ')
/// .replace(/(\r\n)/g, '\n')
/// .replace(/\n\n\n+/g, '\n\n')
/// ```
pub(super) fn preserve_linebreaks(s: &str) -> String {
    // Step 1: with `m`, `^` and `$` sit at line terminators. A run of
    // spaces is removed when it starts a line or ends one.
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut step1 = String::with_capacity(s.len());
    let mut i = 0;
    while i < n {
        if chars[i] == ' ' {
            let mut j = i;
            while j < n && chars[j] == ' ' {
                j += 1;
            }
            let at_line_start = i == 0 || is_line_terminator(chars[i - 1]);
            let at_line_end = j == n || is_line_terminator(chars[j]);
            if at_line_start || at_line_end {
                i = j;
                continue;
            }
            step1.extend(&chars[i..j]);
            i = j;
            continue;
        }
        step1.push(chars[i]);
        i += 1;
    }
    // Step 2: runs of spaces become one.
    let mut step2 = String::with_capacity(step1.len());
    let mut prev_space = false;
    for c in step1.chars() {
        if c == ' ' {
            if !prev_space {
                step2.push(' ');
            }
            prev_space = true;
        } else {
            prev_space = false;
            step2.push(c);
        }
    }
    // Step 3: CRLF becomes LF.
    let step3 = step2.replace("\r\n", "\n");
    // Step 4: three or more LFs become two.
    let mut out = String::with_capacity(step3.len());
    let mut run = 0;
    for c in step3.chars() {
        if c == '\n' {
            run += 1;
            if run <= 2 {
                out.push('\n');
            }
        } else {
            run = 0;
            out.push(c);
        }
    }
    out
}

/// `str.match(/\w+/g)`: how many runs of word characters.
pub(super) fn word_runs(s: &str) -> usize {
    let mut count = 0;
    let mut in_run = false;
    for c in s.chars() {
        if is_word(c) {
            if !in_run {
                count += 1;
            }
            in_run = true;
        } else {
            in_run = false;
        }
    }
    count
}

/// `str.replace(/\r\n|\n/g, '<br />\n')`.
pub(super) fn nl2br(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                out.push_str("<br />\n");
            }
            '\n' => out.push_str("<br />\n"),
            c => out.push(c),
        }
    }
    out
}

/// `str.split(/(\s+)/)` without the empty pieces: runs of white space and
/// runs of everything else, in order.
pub(super) fn words_and_gaps(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut cur: Option<bool> = None;
    for (i, c) in s.char_indices() {
        let ws = is_js_whitespace(c);
        match cur {
            Some(prev) if prev != ws => {
                out.push(&s[start..i]);
                start = i;
            }
            _ => {}
        }
        cur = Some(ws);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// What `puncRe` takes out of a word that has no white space:
/// `/^(?:\(|<|&lt;)?(.*?)(?:\.|,|\)|\n|&gt;)?$/`, group 1.
pub(super) fn strip_punctuation(word: &str) -> &str {
    let rest = if let Some(r) = word.strip_prefix('(') {
        r
    } else if let Some(r) = word.strip_prefix('<') {
        r
    } else if let Some(r) = word.strip_prefix("&lt;") {
        r
    } else {
        word
    };
    // The lazy group ends at the first place the rest is one optional
    // closer: the longest one at the end, `&gt;` before the others.
    if let Some(r) = rest.strip_suffix("&gt;") {
        r
    } else if let Some(r) = rest
        .strip_suffix('.')
        .or_else(|| rest.strip_suffix(','))
        .or_else(|| rest.strip_suffix(')'))
    {
        r
    } else {
        rest
    }
}

/// `/^https?:\/\/.*$/`, for text with no line terminator.
pub(super) fn is_http(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// `/^www\./`.
pub(super) fn is_www(s: &str) -> bool {
    s.starts_with("www.")
}

/// ``/^[\w.!#$%&'*+\-\/=?\^`{|}~]+@[a-z\d\-]+(\.[a-z\d\-]+)+$/i``.
pub(super) fn is_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    let local_ok = !local.is_empty()
        && local.chars().all(|c| {
            is_word(c)
                || matches!(
                    c,
                    '.' | '!'
                        | '#'
                        | '$'
                        | '%'
                        | '&'
                        | '\''
                        | '*'
                        | '+'
                        | '-'
                        | '/'
                        | '='
                        | '?'
                        | '^'
                        | '`'
                        | '{'
                        | '|'
                        | '}'
                        | '~'
                )
        });
    let labels: Vec<&str> = domain.split('.').collect();
    local_ok
        && labels.len() >= 2
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// `/\.(?:org|net|com)(?:\:|\/|$)/`.
pub(super) fn has_tld(s: &str) -> bool {
    ["org", "net", "com"].iter().any(|tld| {
        let needle = format!(".{tld}");
        s.match_indices(&needle).any(|(i, _)| {
            let after = &s[i + needle.len()..];
            after.is_empty() || after.starts_with(':') || after.starts_with('/')
        })
    })
}
