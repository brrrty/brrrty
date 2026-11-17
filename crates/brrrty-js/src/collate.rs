//! `String.prototype.localeCompare` with no locale argument, in an English
//! locale: ICU's root collation at tertiary strength, with normalization
//! on, as V8 sets it up.
//!
//! The data is icu4x's (CLDR 49, Unicode 18). Node 26 ships ICU 78.3
//! (CLDR 48, Unicode 17). Two kinds of character are refused, because the
//! order differs:
//!
//! - Han ideographs (any character whose compatibility decomposition has
//!   one: CJK ideographs, compatibility ideographs, Kangxi and CJK
//!   radicals, `々`). Node's ICU sorts them by radical and stroke count
//!   from Unihan; icu4x's data sorts them by code point within blocks.
//!   4,274 adjacent pairs of Node's order of every code point come out the
//!   other way.
//! - Characters new in Unicode 18, which Node's ICU leaves unassigned.
//!
//! Every other code point sorts as in Node: the review sorted all of them
//! with Node and checked each neighbouring pair, alone and in 14 contexts.

use std::cmp::Ordering;
use std::sync::OnceLock;

use icu_collator::options::CollatorOptions;
use icu_collator::{Collator, CollatorBorrowed};

fn collator() -> &'static CollatorBorrowed<'static> {
    static C: OnceLock<CollatorBorrowed<'static>> = OnceLock::new();
    C.get_or_init(|| {
        Collator::try_new(Default::default(), CollatorOptions::default())
            .expect("the root collation is compiled in")
    })
}

/// Code points assigned in Unicode 18 that Node 26's Unicode 17 leaves
/// unassigned: icu4x 2.4's general category against Node's `/\p{Cn}/u`,
/// over every code point (13,007 of them).
pub const NEW_IN_UNICODE_18: &[(u32, u32)] = &[
    (0x558, 0x558),
    (0x58B, 0x58C),
    (0x5C8, 0x5C9),
    (0xB53, 0xB54),
    (0x1ADE, 0x1ADF),
    (0x1AEC, 0x1AF0),
    (0x208F, 0x208F),
    (0x209D, 0x209F),
    (0x20C2, 0x20C4),
    (0x2E60, 0x2E63),
    (0xA7DD, 0xA7DD),
    (0xA7E2, 0xA7E2),
    (0xAB6C, 0xAB6D),
    (0x107BB, 0x107BF),
    (0x10EC9, 0x10ECF),
    (0x10ED9, 0x10EEE),
    (0x10EF0, 0x10EF9),
    (0x11B0A, 0x11B0A),
    (0x11DF0, 0x11DF1),
    (0x1246F, 0x1246F),
    (0x12475, 0x1247F),
    (0x12550, 0x12686),
    (0x18CD6, 0x18CDA),
    (0x18D1F, 0x18D20),
    (0x18E00, 0x19191),
    (0x191A0, 0x191D2),
    (0x1B123, 0x1B128),
    (0x1B168, 0x1B168),
    (0x1CED2, 0x1CED4),
    (0x1CEDD, 0x1CEDF),
    (0x1CEF1, 0x1CEFD),
    (0x1D127, 0x1D128),
    (0x1D1EB, 0x1D1FF),
    (0x1D250, 0x1D281),
    (0x1D6A6, 0x1D6A6),
    (0x1DB00, 0x1DB1C),
    (0x1DF1F, 0x1DF24),
    (0x1DF2B, 0x1DF81),
    (0x1DF90, 0x1DF96),
    (0x1DFCD, 0x1DFFF),
    (0x1F1AE, 0x1F1AE),
    (0x1F6D9, 0x1F6D9),
    (0x1F7DA, 0x1F7DB),
    (0x1F7F1, 0x1F7FF),
    (0x1FA8B, 0x1FA8D),
    (0x1FACC, 0x1FACC),
    (0x1FADD, 0x1FADD),
    (0x1FAEB, 0x1FAEB),
    (0x1FAF9, 0x1FAFA),
    (0x2B81E, 0x2B81E),
    (0x3D000, 0x3FC3F),
];

fn is_new(c: char) -> bool {
    let c = c as u32;
    NEW_IN_UNICODE_18
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                Ordering::Less
            } else if lo > c {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

/// Why a pair of strings isn't compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollationRefusal {
    /// A Han character: Node sorts it by radical and stroke.
    Han(char),
    /// A character new in Unicode 18, unassigned in Node 26's ICU.
    Unicode18(char),
}

impl std::fmt::Display for CollationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CollationRefusal::Han(c) => write!(
                f,
                "`{c}` (U+{:04X}) is a Han character: Node sorts those by radical and stroke count, from data Brrrty doesn't have, so the order can't be matched",
                *c as u32
            ),
            CollationRefusal::Unicode18(c) => write!(
                f,
                "U+{:04X} is new in Unicode 18, which Node's ICU doesn't have yet, so its order there can't be matched",
                *c as u32
            ),
        }
    }
}

/// Whether `c`, or anything its compatibility decomposition (NFKD) gives,
/// is in the Han script.
fn is_han(c: char) -> bool {
    use icu_properties::CodePointMapData;
    use icu_properties::props::Script;
    use unicode_normalization::char::decompose_compatible;
    if c.is_ascii() {
        return false;
    }
    let script = CodePointMapData::<Script>::new();
    let mut han = script.get(c) == Script::Han;
    decompose_compatible(c, |d| han |= script.get(d) == Script::Han);
    han
}

/// `a.localeCompare(b)` in Node with an English locale (`en-US`, or `C`):
/// less, equal or greater, as its -1, 0 or 1. `Err` names a character
/// whose order in Node can't be matched.
pub fn js_locale_compare(a: &str, b: &str) -> Result<Ordering, CollationRefusal> {
    for c in a.chars().chain(b.chars()) {
        if is_new(c) {
            return Err(CollationRefusal::Unicode18(c));
        }
        if is_han(c) {
            return Err(CollationRefusal::Han(c));
        }
    }
    Ok(collator().compare(a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_han_characters_and_their_compatibility_forms() {
        // Node: "﨑".localeCompare("嵊") is -1, "缵".localeCompare("纓") is -1;
        // icu4x's data gives 1 for both.
        for (a, b, c) in [
            ("﨑", "嵊", '﨑'),
            ("缵", "纓", '缵'),
            ("a", "々", '々'),
            ("⼀", "b", '⼀'), // Kangxi radical one
            ("x", "⺠", '⺠'), // CJK radical
            ("㊀", "y", '㊀'), // NFKD gives 一
        ] {
            assert_eq!(
                js_locale_compare(a, b),
                Err(CollationRefusal::Han(c)),
                "{a} {b}"
            );
        }
        // Kana, Hangul and the rest still compare.
        assert!(js_locale_compare("あ", "ア").is_ok());
        assert!(js_locale_compare("가", "각").is_ok());
        assert!(js_locale_compare("〇", "z").is_err());
    }

    #[test]
    fn refuses_characters_node_does_not_know() {
        assert_eq!(js_locale_compare("a", "b"), Ok(Ordering::Less));
        assert_eq!(
            js_locale_compare("a\u{20C2}", "b"),
            Err(CollationRefusal::Unicode18('\u{20C2}'))
        );
        assert_eq!(
            js_locale_compare("a", "\u{3FC3F}"),
            Err(CollationRefusal::Unicode18('\u{3FC3F}'))
        );
        assert!(js_locale_compare("\u{20C1}", "\u{20C5}").is_ok());
        let mut last = 0;
        for &(lo, hi) in NEW_IN_UNICODE_18 {
            assert!(lo >= last && lo <= hi);
            last = hi + 2;
        }
        let count: u32 = NEW_IN_UNICODE_18.iter().map(|(lo, hi)| hi - lo + 1).sum();
        assert_eq!(count, 13_007);
    }
}
