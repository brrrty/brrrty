//! The character classes JavaScript defines, and case folding with `i`.

use crate::canon_table::CANON;
use std::sync::OnceLock;

use crate::set::UnitSet;

/// JavaScript's `\s`, in Rust regex syntax.
pub const SPACE: &str =
    r"\t\n\x0B\x0C\r \u00A0\u1680\u2000-\u200A\u2028\u2029\u202F\u205F\u3000\uFEFF";
/// JavaScript's `\w` (ASCII only), in Rust regex syntax.
pub const WORD: &str = "A-Za-z0-9_";
/// JavaScript's `\d` (ASCII only), in Rust regex syntax.
pub const DIGIT: &str = "0-9";

const SPACE_RANGES: &[(u16, u16)] = &[
    (0x09, 0x0D),
    (0x20, 0x20),
    (0xA0, 0xA0),
    (0x1680, 0x1680),
    (0x2000, 0x200A),
    (0x2028, 0x2029),
    (0x202F, 0x202F),
    (0x205F, 0x205F),
    (0x3000, 0x3000),
    (0xFEFF, 0xFEFF),
];
const WORD_RANGES: &[(u16, u16)] = &[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)];
const DIGIT_RANGES: &[(u16, u16)] = &[(0x30, 0x39)];
const LINE_TERMINATOR_RANGES: &[(u16, u16)] = &[(0x0A, 0x0A), (0x0D, 0x0D), (0x2028, 0x2029)];

/// `\s`.
pub fn space() -> UnitSet {
    UnitSet::from_ranges(SPACE_RANGES.iter().copied())
}

/// `\w`.
pub fn word() -> UnitSet {
    UnitSet::from_ranges(WORD_RANGES.iter().copied())
}

/// `\d`.
pub fn digit() -> UnitSet {
    UnitSet::from_ranges(DIGIT_RANGES.iter().copied())
}

/// `\n`, `\r`, U+2028 and U+2029: what `.` (without `s`) doesn't match, and
/// where `^` and `$` (with `m`) break lines.
pub fn line_terminators() -> UnitSet {
    UnitSet::from_ranges(LINE_TERMINATOR_RANGES.iter().copied())
}

/// The class escape `\d`, `\D`, `\s`, `\S`, `\w` or `\W`.
pub fn escape_class(letter: char) -> Option<UnitSet> {
    let set = match letter.to_ascii_lowercase() {
        'd' => digit(),
        's' => space(),
        'w' => word(),
        _ => return None,
    };
    Some(if letter.is_ascii_uppercase() {
        set.complement()
    } else {
        set
    })
}

/// Canonicalize (ECMA-262 22.2.2.7.3) for the `i` flag without `u` or `v`:
/// the unit's upper case when that is one unit, unless that takes a unit
/// outside ASCII into it. Two units match each other under `i` exactly when
/// their canonical units are equal.
pub fn canonicalize(u: u16) -> u16 {
    match CANON.binary_search_by_key(&u, |&(k, _)| k) {
        Ok(i) => CANON[i].1,
        Err(_) => u,
    }
}

/// The units whose canonical unit differs from themselves.
fn keys() -> &'static UnitSet {
    static KEYS: OnceLock<UnitSet> = OnceLock::new();
    KEYS.get_or_init(|| UnitSet::from_ranges(CANON.iter().map(|&(k, _)| (k, k))))
}

/// Every unit that matches a member of `set` under `i`: those whose
/// canonical unit is the canonical unit of a member.
pub fn case_closure(set: &UnitSet) -> UnitSet {
    let keys = keys();
    // The canonical units of the members.
    let mut canon = set.minus(keys);
    let mapped: Vec<(u16, u16)> = CANON
        .iter()
        .filter(|&&(k, _)| set.contains(k))
        .map(|&(_, c)| (c, c))
        .collect();
    canon = canon.union(&UnitSet::from_ranges(mapped));
    // Units that are their own canonical unit match when they are one;
    // the others when theirs is.
    let others: Vec<(u16, u16)> = CANON
        .iter()
        .filter(|&&(_, c)| canon.contains(c))
        .map(|&(k, _)| (k, k))
        .collect();
    canon.minus(keys).union(&UnitSet::from_ranges(others))
}

/// The units that `i` lets match some other unit.
pub fn cased_units() -> &'static UnitSet {
    static CASED: OnceLock<UnitSet> = OnceLock::new();
    CASED.get_or_init(|| {
        let keys = keys();
        let mut out = Vec::new();
        for &(k, c) in CANON {
            // k's class is c (if c is its own canonical unit) and every
            // other unit whose canonical unit is c.
            let others = CANON.iter().filter(|&&(x, y)| y == c && x != k).count();
            if others > 0 || !keys.contains(c) {
                out.push((k, k));
                if !keys.contains(c) {
                    out.push((c, c));
                }
            }
        }
        UnitSet::from_ranges(out)
    })
}

/// Whether `i` changes what `set` matches.
pub fn is_cased(set: &UnitSet) -> bool {
    case_closure(set) != *set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_units_follow_javascript() {
        assert_eq!(canonicalize(u16::from(b'a')), u16::from(b'A'));
        assert_eq!(canonicalize(u16::from(b'A')), u16::from(b'A'));
        // ſ upper-cases to S, which is ASCII: it stays itself.
        assert_eq!(canonicalize(0x017F), 0x017F);
        // The Kelvin sign is already upper case.
        assert_eq!(canonicalize(0x212A), 0x212A);
        // ß upper-cases to two units.
        assert_eq!(canonicalize(0x00DF), 0x00DF);
        assert_eq!(canonicalize(0x03C3), 0x03A3);
        assert_eq!(canonicalize(0x03C2), 0x03A3);
    }

    #[test]
    fn case_closure_adds_every_unit_with_the_same_canonical_unit() {
        let k = case_closure(&UnitSet::unit(u16::from(b'k')));
        assert_eq!(k.ranges(), &[(0x4B, 0x4B), (0x6B, 0x6B)]);
        let sigma = case_closure(&UnitSet::unit(0x03C2));
        assert!(sigma.contains(0x03A3) && sigma.contains(0x03C3) && sigma.contains(0x03C2));
        assert!(!is_cased(&digit()));
        // `\w` has both cases already.
        assert!(!is_cased(&word()));
        assert!(is_cased(&UnitSet::unit(u16::from(b'a'))));
        let cased = cased_units();
        assert!(cased.contains(0x61) && cased.contains(0x41) && cased.contains(0x3C2));
        assert!(!cased.contains(0x17F) && !cased.contains(0x212A) && !cased.contains(0x31));
        for u in [0x61, 0x17F, 0x3C2, 0x1C5, 0xDF, 0x130, 0x131] {
            assert_eq!(
                cased.contains(u),
                case_closure(&UnitSet::unit(u)).len() > 1,
                "{u:x}"
            );
        }
        // A negated class keeps its negation outside the closure.
        assert_eq!(case_closure(&UnitSet::full()), UnitSet::full());
    }
}
