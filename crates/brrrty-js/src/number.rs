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
    let (digits, n10) = shortest_digits(n);
    let k = digits.len() as i32;
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

/// The shortest digits that read back as `|n|`, and where the decimal
/// point goes: `|n|` is `0.digits x 10^point`. `n` is finite and not zero.
fn shortest_digits(n: f64) -> (String, i32) {
    // Rust's `{:e}` yields the shortest round-trip digits, as JS does, and
    // the closest of them to the exact value. When the exact value lies
    // exactly halfway between two of them, JS takes the even one; Rust may
    // not, so that case is settled here.
    let sci = format!("{:e}", n.abs());
    let (mantissa, exp) = sci.split_once('e').expect("exponent present");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let point = exp.parse::<i32>().expect("integer exponent") + 1;
    match even_at_tie(n.abs(), &digits, point) {
        Some(even) => (even, point),
        None => (digits, point),
    }
}

/// When `x`'s exact value is the shortest digits plus a final 5 (a tie
/// between `digits` and its neighbour), the even one of the two, if it
/// reads back as `x` too and isn't `digits` already.
fn even_at_tie(x: f64, digits: &str, point: i32) -> Option<String> {
    let k = digits.len();
    // The exact value as an integer and the count of its decimals. A tie
    // needs at most k + 1 <= 18 significant digits, which only a double
    // with a few binary places, or a small integer, has.
    let bits = x.to_bits();
    let biased = (bits >> 52) as i32;
    let fraction = bits & ((1 << 52) - 1);
    let (mut m, mut e) = if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1 << 52), biased - 1075)
    };
    let z = m.trailing_zeros() as i32;
    m >>= z;
    e += z;
    let (exact, decimals) = if e < 0 {
        // m odd: the last decimal is a 5 at place -e, and m * 5^-e has
        // more than 0.69 * -e digits.
        if -e > 25 {
            return None;
        }
        (u128::from(m) * 5u128.pow((-e) as u32), -e)
    } else {
        // An integer ending in 5 then e zeros needs m divisible by 5^(e+1).
        if e > 22 {
            return None;
        }
        (u128::from(m) << e, 0)
    };
    let text = exact.to_string();
    let sig = text.trim_end_matches('0');
    if sig.len() != k + 1 || !sig.ends_with('5') {
        return None;
    }
    // Same decade as `digits`: the exact value's point must match.
    let exact_point = text.len() as i32 - decimals;
    if exact_point != point {
        return None;
    }
    let low: u64 = sig[..k].parse().ok()?;
    let even = if low.is_multiple_of(2) { low } else { low + 1 };
    let even_text = even.to_string();
    if even_text.len() != k || even_text == digits {
        return None;
    }
    let reads_back = format!("0.{even_text}e{point}").parse::<f64>().ok()? == x;
    reads_back.then_some(even_text)
}

/// `Number.prototype.toFixed(digits)`, as V8 does it.
///
/// The result is the integer `m` closest to the double's exact value times
/// `10^digits`, the larger one on a tie, so a tie goes away from zero and
/// is judged on the exact binary value: `(2.5).toFixed(0)` is `"3"`, but
/// `(1.005).toFixed(2)` is `"1.00"`, because 1.005 is stored as
/// 1.00499999999999989... A magnitude of `1e21` or more prints as
/// `String(n)`. `-0` gives `"0.00"`; a negative number that rounds to zero
/// keeps its sign (`"-0.00"`).
///
/// `digits` above 100 is an error, V8's `RangeError` message.
pub fn js_to_fixed(n: f64, digits: u32) -> Result<String, &'static str> {
    if digits > 100 {
        return Err("toFixed() digits argument must be between 0 and 100");
    }
    if n.is_nan() {
        return Ok("NaN".into());
    }
    if n.abs() >= 1e21 {
        return Ok(js_number_to_string(n));
    }
    let bits = n.abs().to_bits();
    let biased = (bits >> 52) as i32;
    let fraction = bits & ((1 << 52) - 1);
    // |n| = mantissa * 2^exp exactly.
    let (mantissa, exp) = if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1 << 52), biased - 1075)
    };
    let mut big = Big::from(mantissa);
    for _ in 0..digits {
        big.mul_small(10);
    }
    if exp >= 0 {
        big.shl(exp as u32);
    } else {
        let k = (-exp) as u32;
        let round_up = big.bit(k - 1);
        big.shr(k);
        if round_up {
            big.add_one();
        }
    }
    let mut text = big.to_decimal();
    let digits = digits as usize;
    if text.len() <= digits {
        text.insert_str(0, &"0".repeat(digits + 1 - text.len()));
    }
    if digits > 0 {
        text.insert(text.len() - digits, '.');
    }
    if n < 0.0 {
        text.insert(0, '-');
    }
    Ok(text)
}

/// `n.toLocaleString('en-US')` with no options, as Node's ICU prints it:
/// the shortest digits that read back as `n` (those `String(n)` uses),
/// rounded half away from zero to at most 3 decimals, trailing zeros
/// dropped, and the whole part in groups of three with `,`. `-0` (and a
/// negative number that rounds to zero) keeps its `-`; infinity is `∞`.
pub fn js_to_locale_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    let sign = if n.is_sign_negative() { "-" } else { "" };
    if n.is_infinite() {
        return format!("{sign}∞");
    }
    if n == 0.0 {
        return format!("{sign}0");
    }
    let (digits, mut point) = shortest_digits(n);
    let mut d: Vec<u8> = digits.into_bytes();
    // Keep the digits down to the third decimal, rounding half up on the
    // shortest digits (ICU's half-even never sees a tie it rounds down:
    // its rounding mode for this format is half-expand).
    let keep = point + 3;
    if keep < 0 {
        d.clear();
    } else if (keep as usize) < d.len() {
        let up = d[keep as usize] >= b'5';
        d.truncate(keep as usize);
        if up {
            let mut i = d.len();
            loop {
                if i == 0 {
                    d.insert(0, b'1');
                    point += 1;
                    break;
                }
                i -= 1;
                if d[i] == b'9' {
                    d[i] = b'0';
                } else {
                    d[i] += 1;
                    break;
                }
            }
        }
    }
    while d.last() == Some(&b'0') {
        d.pop();
    }
    if d.is_empty() {
        return format!("{sign}0");
    }
    let (int, frac): (Vec<u8>, Vec<u8>) = if point <= 0 {
        (
            Vec::new(),
            std::iter::repeat_n(b'0', (-point) as usize)
                .chain(d)
                .collect(),
        )
    } else if point as usize >= d.len() {
        let mut int = d;
        int.resize(point as usize, b'0');
        (int, Vec::new())
    } else {
        let frac = d.split_off(point as usize);
        (d, frac)
    };
    let mut out = String::from(sign);
    if int.is_empty() {
        out.push('0');
    }
    for (i, c) in int.iter().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c as char);
    }
    if !frac.is_empty() {
        out.push('.');
        out.extend(frac.iter().map(|c| *c as char));
    }
    out
}

/// A small unsigned big integer, little-endian 32-bit limbs: enough for a
/// double times `10^100`.
struct Big(Vec<u32>);

impl Big {
    fn from(n: u64) -> Self {
        Big(vec![n as u32, (n >> 32) as u32])
    }
    fn mul_small(&mut self, m: u32) {
        let mut carry = 0u64;
        for limb in &mut self.0 {
            let v = u64::from(*limb) * u64::from(m) + carry;
            *limb = v as u32;
            carry = v >> 32;
        }
        if carry > 0 {
            self.0.push(carry as u32);
        }
    }
    fn shl(&mut self, k: u32) {
        let (words, bits) = ((k / 32) as usize, k % 32);
        let mut out = vec![0u32; words];
        let mut carry = 0u32;
        for &limb in &self.0 {
            if bits == 0 {
                out.push(limb);
            } else {
                out.push((limb << bits) | carry);
                carry = limb >> (32 - bits);
            }
        }
        out.push(carry);
        self.0 = out;
    }
    fn shr(&mut self, k: u32) {
        let (words, bits) = ((k / 32) as usize, k % 32);
        if words >= self.0.len() {
            self.0 = vec![0];
            return;
        }
        let src = &self.0[words..];
        let out = (0..src.len())
            .map(|i| {
                let hi = src.get(i + 1).copied().unwrap_or(0);
                if bits == 0 {
                    src[i]
                } else {
                    (src[i] >> bits) | (hi << (32 - bits))
                }
            })
            .collect();
        self.0 = out;
    }
    fn bit(&self, k: u32) -> bool {
        self.0
            .get((k / 32) as usize)
            .is_some_and(|limb| (limb >> (k % 32)) & 1 == 1)
    }
    fn add_one(&mut self) {
        for limb in &mut self.0 {
            let (v, overflow) = limb.overflowing_add(1);
            *limb = v;
            if !overflow {
                return;
            }
        }
        self.0.push(1);
    }
    fn to_decimal(&self) -> String {
        let mut limbs = self.0.clone();
        let mut chunks = Vec::new();
        loop {
            while limbs.last() == Some(&0) {
                limbs.pop();
            }
            if limbs.is_empty() {
                break;
            }
            let mut rem = 0u64;
            for limb in limbs.iter_mut().rev() {
                let v = (rem << 32) | u64::from(*limb);
                *limb = (v / 1_000_000_000) as u32;
                rem = v % 1_000_000_000;
            }
            chunks.push(rem as u32);
        }
        match chunks.split_last() {
            None => "0".into(),
            Some((top, rest)) => {
                let mut s = top.to_string();
                for c in rest.iter().rev() {
                    s.push_str(&format!("{c:09}"));
                }
                s
            }
        }
    }
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
            // Exactly halfway between ...562 and ...563: JS takes the even one.
            // (written as text: the literal has more digits than the double keeps)
            ("9302184022144.5625".parse().unwrap(), "9302184022144.562"),
            (1234567890123.4565, "1234567890123.4565"),
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
