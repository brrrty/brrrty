//! JavaScript regular expressions translated to fancy-regex, checked
//! against what Node's `exec` gives (with the `g` flag and `lastIndex`
//! set), in UTF-16 units.

use brrrty_prism::regex::{JsText, translate, translate_spans};

/// `[index, end, group 1 start, group 1 end, ...]`, -1 for an unset group.
fn exec(source: &str, flags: &str, text: &str, last: usize) -> Option<Vec<i64>> {
    exec_on(source, flags, &JsText::new(text), last)
}

fn exec_on(source: &str, flags: &str, t: &JsText, last: usize) -> Option<Vec<i64>> {
    let re = translate(source, flags).unwrap_or_else(|e| panic!("/{source}/{flags}: {}", e.0));
    re.exec(t, last).unwrap().map(|m| {
        let mut out = vec![m.index as i64, m.end as i64];
        for g in &m.groups {
            match g {
                Some((a, b)) => out.extend([*a as i64, *b as i64]),
                None => out.extend([-1, -1]),
            }
        }
        out
    })
}

fn refused(source: &str, flags: &str) -> String {
    match translate(source, flags) {
        Ok(_) => panic!("/{source}/{flags} was translated"),
        Err(e) => e.0,
    }
}

#[test]
fn classes_are_javascripts() {
    // \w and \b are ASCII; é is not a word character.
    assert_eq!(exec(r"\b\w+\b", "", "é abc", 0), Some(vec![2, 5]));
    assert_eq!(exec(r"\w+", "", "éa", 0), Some(vec![1, 2]));
    // \s has U+FEFF and U+00A0, and not U+0085.
    assert_eq!(
        exec(r"\s+", "", "a\u{85}\u{a0}\u{feff}b", 0),
        Some(vec![2, 4])
    );
    // `.` stops at \r and U+2028, and not at U+0085.
    assert_eq!(exec(r".+", "", "a\u{85}b\rc", 0), Some(vec![0, 3]));
    assert_eq!(exec(r"[^]+", "", "a\r\n", 0), Some(vec![0, 3]));
    assert_eq!(exec(r"[]", "", "a", 0), None);
    assert_eq!(exec(r"a.b", "s", "a\nb", 0), Some(vec![0, 3]));
}

#[test]
fn positions_are_utf16_units() {
    // An emoji is two units, and `.` matches each.
    assert_eq!(exec(r".", "", "😀", 0), Some(vec![0, 1]));
    assert_eq!(exec(r".", "", "😀", 1), Some(vec![1, 2]));
    assert_eq!(exec(r"b", "", "😀b", 0), Some(vec![2, 3]));
    assert_eq!(exec(r"[\uD800-\uDBFF]", "", "a😀", 0), Some(vec![1, 2]));
    assert_eq!(exec(r"\uDE00", "", "😀", 0), Some(vec![1, 2]));
    // A literal outside the BMP is two units; `+` repeats the second.
    let t = JsText::from_units(&[0xD83D, 0xDE00, 0xDE00, 0x61]);
    assert_eq!(exec_on("😀+", "", &t, 0), Some(vec![0, 3]));
    // A lone surrogate is a unit like any other.
    assert_eq!(exec_on(r"[^a]+", "", &t, 1), Some(vec![1, 3]));
    // lastIndex past the end matches nothing; at the end, an empty match.
    assert_eq!(exec(r"$", "", "ab", 2), Some(vec![2, 2]));
    assert_eq!(exec(r"$", "", "ab", 3), None);
}

#[test]
fn case_folding_is_javascripts() {
    assert_eq!(exec(r"k", "i", "\u{212a}K", 0), Some(vec![1, 2]));
    assert_eq!(exec(r"s", "i", "\u{17f}s", 0), Some(vec![1, 2]));
    assert_eq!(exec(r"σ", "i", "Σ", 0), Some(vec![0, 1]));
    assert_eq!(exec(r"[^a-z]", "i", "A1", 0), Some(vec![1, 2]));
    assert_eq!(exec(r"\W", "i", "s\u{17f}", 0), Some(vec![1, 2]));
}

#[test]
fn anchors_and_lookaround() {
    // `^` is the start of the text, not of lastIndex.
    assert_eq!(exec(r"^a", "", "aa", 1), None);
    // With `m`, lines break at \r and U+2028 too.
    assert_eq!(exec(r"^b$", "m", "a\rb\u{2028}c", 0), Some(vec![2, 3]));
    // A lookbehind sees the text before lastIndex.
    assert_eq!(exec(r"(?<=a)b", "", "ab", 1), Some(vec![1, 2]));
    assert_eq!(exec(r"a(?=(b))", "", "ab", 0), Some(vec![0, 1, 1, 2]));
    assert_eq!(exec(r"a(?!(c))", "", "ab", 0), Some(vec![0, 1, -1, -1]));
}

#[test]
fn groups_and_backreferences() {
    assert_eq!(
        exec(r#"(["'])(?:\\.|(?!\1)[^\\])*\1"#, "", r#"x "a'b" y"#, 0),
        Some(vec![2, 7, 2, 3])
    );
    assert_eq!(exec(r"(a)|(b)", "", "b", 0), Some(vec![0, 1, -1, -1, 0, 1]));
    assert_eq!(exec(r"(?<q>a)\k<q>", "", "aa", 0), Some(vec![0, 2, 0, 1]));
}

#[test]
fn backreferences_with_i_compare_as_javascript_does() {
    // ASCII groups compare by ASCII case, in both.
    assert_eq!(
        exec(r"([a-z]+)-\1", "i", "Ab-aB", 0),
        Some(vec![0, 5, 0, 2])
    );
    // The Kelvin sign doesn't match `k` in JavaScript.
    assert_eq!(exec(r"([a-z]+)-\1", "i", "k-\u{212a}", 0), None);
    // A group that can hold letters outside ASCII is compared the same way
    // only on text without them; otherwise the match is refused.
    let re = translate(r#"<<<"([^"]+)"\1"#, "i").unwrap();
    assert!(re.exec(&JsText::new(r#"<<<"éa"ÉA"#), 0).is_err());
    assert!(re.exec(&JsText::new("<<<\"k\"\u{212a}"), 0).is_err());
    // Only units at or after where a match could start matter.
    assert_eq!(
        exec(r#"<<<"([^"]+)"\1"#, "i", r#"é <<<"ab"AB"#, 0),
        Some(vec![2, 11, 6, 8])
    );
    assert!(re.exec(&JsText::new("é <<<"), 1).unwrap().is_none());
    assert_eq!(
        exec(r#"<<<"([^"]+)"\1"#, "i", r#"— <<<"ab"AB"#, 0),
        Some(vec![2, 11, 6, 8])
    );
}

#[test]
fn what_cannot_be_translated_exactly_is_refused() {
    assert!(refused("(?:a|)*", "").contains("can match nothing"));
    assert!(refused("(?:(a)|b)+", "").contains("repetition"));
    assert!(refused(r"(a)?\1", "").contains("backreference"));
    assert!(refused(r"(?<=(a))b", "").contains("lookbehind"));
    assert!(refused("a", "u").contains("`u`"));
    assert!(refused("a", "y").contains("`y`"));
    assert!(refused(r"\1", "").contains("octal"));
    assert!(refused(r"(?i:a)", "").contains("(?"));
}

#[test]
fn the_php_heredoc_guard_applies_only_to_quoted_labels() {
    // Prism's php heredoc: a quoted label (any units, compared without
    // case) or a bare one (ASCII only, exact in both engines).
    let re = translate(
        r#"<<<(?:"([^"]+)"[\r\n](?:.*[\r\n])*?\1;|([a-z_]\w*)[\r\n](?:.*[\r\n])*?\2;)"#,
        "i",
    )
    .unwrap();
    let ok = |s: &str| re.exec(&JsText::new(s), 0).is_ok();
    // A bare label with letters outside ASCII anywhere: exact.
    assert!(ok("$a = <<<EOT\ncafé ſ \u{212a}\nEOT;\n"));
    assert!(ok("// café\n$a = <<<eot\nx\nEOT;\n"));
    // A quoted label with such a letter after it: refused.
    assert!(!ok("$a = <<<\"EOT\"\ncafé\nEOT;\n"));
    // ... and before it: exact.
    assert!(ok("// café\n$a = <<<\"EOT\"\nx\nEOT;\n"));
    // Bare first, quoted later with the letter between them: the quoted
    // one is still after it, so exact.
    assert!(ok("<<<A\né\nA;\n<<<\"B\"\nx\nB;\n"));
}

#[test]
fn span_only_translation() {
    // php's placeholder pattern: group 1 sits out some passes, so its value
    // can differ, but `\1` always follows it in the same pass.
    let php = r#"<\?(?:[^"'/#]|\/(?![*/])|("|')(?:\\[\s\S]|(?!\1)[^\\])*\1|(?:\/\/|#(?!\[))(?:[^?\n\r]|\?(?!>))*(?=$|\?>|[\r\n])|#\[|\/\*(?:[^*]|\*(?!\/))*(?:\*\/|$))*?(?:\?>|$)"#;
    assert!(refused(php, "g").contains("repetition"));
    let re = translate_spans(php, "g").unwrap();
    let m = re
        .exec(&JsText::new(r#"a <?php echo "?>" . 'x'; ?> b"#), 0)
        .unwrap()
        .unwrap();
    assert_eq!((m.index, m.end), (2, 27));
    assert!(m.groups.is_empty());
    // A backreference after the repetition reads the stale value: refused.
    assert!(translate_spans("(?:(a)|b)+\\1", "").is_err());
    // A backreference to a group set before the repetition: refused.
    assert!(translate_spans("(a)(?:(b)\\1|c)+", "").is_err());
}
