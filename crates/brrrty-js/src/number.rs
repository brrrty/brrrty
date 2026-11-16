//! `Number.prototype.toString` for `f64`.

/// Formats `n` exactly as JavaScript's `String(n)` does.
pub fn js_number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    if n.is_infinite() {
        return if n < 0.0 { "-Infinity" } else { "Infinity" }.into();
    }
    // Rust's `{:e}` yields the shortest round-trip digits, which is what JS uses too.
    let sci = format!("{:e}", n.abs());
    let (mantissa, exp) = sci.split_once('e').expect("exponent present");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n10 = exp.parse::<i32>().expect("integer exponent") + 1;
    let mut out = String::new();
    if n < 0.0 {
        out.push('-');
    }
    if k <= n10 && n10 <= 21 {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (n10 - k) as usize));
    } else if 0 < n10 && n10 <= 21 {
        out.push_str(&digits[..n10 as usize]);
        out.push('.');
        out.push_str(&digits[n10 as usize..]);
    } else if -6 < n10 && n10 <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-n10) as usize));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let e = n10 - 1;
        out.push('e');
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&e.abs().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_javascript() {
        let cases: &[(f64, &str)] = &[
            (1.0, "1"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1e21, "1e+21"),
            (1e-7, "1e-7"),
            (-0.0, "0"),
            (0.0, "0"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
            (123456789012345680000.0, "123456789012345680000"),
            (1.5e-7, "1.5e-7"),
            (0.000001, "0.000001"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (-1.5, "-1.5"),
            (100.0, "100"),
            (1e20, "100000000000000000000"),
            (123.456, "123.456"),
            (-1e-7, "-1e-7"),
            (1.2345e25, "1.2345e+25"),
        ];
        for (n, want) in cases {
            assert_eq!(js_number_to_string(*n), *want, "for {n:?}");
        }
    }
}

/// JavaScript's whitespace and line terminators: what `\s` matches in a
/// regular expression, and what `Number(text)` and `trim()` remove.
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// JavaScript's `Number(text)`.
///
/// Leading and trailing whitespace is ignored and an empty string is `0`.
/// Accepts decimal literals with an optional sign and exponent,
/// `Infinity`, and unsigned `0x`, `0o` and `0b` integers. Anything else,
/// including `inf`, `NaN` and `1_000`, is `NaN`.
pub fn js_string_to_number(text: &str) -> f64 {
    let t = text.trim_matches(is_js_whitespace);
    if t.is_empty() {
        return 0.0;
    }
    let radix = match t.get(..2) {
        Some("0x" | "0X") => 16,
        Some("0o" | "0O") => 8,
        Some("0b" | "0B") => 2,
        _ => 10,
    };
    if radix != 10 {
        let digits = &t[2..];
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return f64::NAN;
        }
        return digits.chars().fold(0.0, |n, c| {
            n * radix as f64 + c.to_digit(radix).unwrap() as f64
        });
    }
    let (sign, body) = match t.as_bytes()[0] {
        b'+' => (1.0, &t[1..]),
        b'-' => (-1.0, &t[1..]),
        _ => (1.0, t),
    };
    if body == "Infinity" {
        return sign * f64::INFINITY;
    }
    if !is_decimal_literal(body) {
        return f64::NAN;
    }
    sign * body.parse::<f64>().unwrap_or(f64::NAN)
}

/// `digits [. digits] [e [+-] digits]`, where one side of the `.` may be
/// empty but not both.
fn is_decimal_literal(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - start
    };
    let mut n = digits(&mut i);
    if i < b.len() && b[i] == b'.' {
        i += 1;
        n += digits(&mut i);
    }
    if n == 0 {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return false;
        }
    }
    i == b.len()
}

#[cfg(test)]
mod to_number_tests {
    use super::js_string_to_number;

    #[test]
    fn matches_node() {
        // Each pair is [text, String(Number(text))] from node.
        let cases: &[(&str, &str)] = &[
            ("", "0"),
            (" ", "0"),
            (" 12 ", "12"),
            (" 12\u{feff}", "12"),
            ("1e3", "1000"),
            ("1E-2", "0.01"),
            ("+5", "5"),
            ("-5", "-5"),
            ("5.", "5"),
            (".5", "0.5"),
            (".", "NaN"),
            ("+.5", "0.5"),
            ("-.e1", "NaN"),
            ("1e", "NaN"),
            ("0x1F", "31"),
            ("0X1f", "31"),
            ("-0x10", "NaN"),
            ("0o17", "15"),
            ("0b101", "5"),
            ("0x", "NaN"),
            ("Infinity", "Infinity"),
            ("-Infinity", "-Infinity"),
            ("+Infinity", "Infinity"),
            ("infinity", "NaN"),
            ("inf", "NaN"),
            ("NaN", "NaN"),
            ("1_000", "NaN"),
            ("12abc", "NaN"),
            ("00012", "12"),
            ("1.2.3", "NaN"),
            ("\t\n7\r", "7"),
            ("\u{180e}1", "NaN"),
            ("\u{3000}1", "1"),
            ("0.1e+2", "10"),
            ("1e400", "Infinity"),
            ("-0", "0"),
        ];
        for (text, want) in cases {
            let got = crate::js_number_to_string(js_string_to_number(text));
            assert_eq!(&got, want, "Number({text:?})");
        }
    }
}
