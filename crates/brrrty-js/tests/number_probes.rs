//! `Number.prototype.toFixed`, `toLocaleString('en-US')` and `String(x)`
//! against V8:
//! `tests/probes/number.json`, made by `tools/gen-number-probes.mjs` (Node
//! v26.2.0, ICU 78.3). Hand-picked cases are compared one by one; the
//! seeded fuzz batches (600,000 `toFixed` cases, 300,000 each for
//! `toLocaleString` and `String`)
//! are made again here from the same seeds and compared by hash. A batch
//! that differs is named; `node tools/gen-number-probes.mjs dump to_fixed N`
//! (or `dump locale N`) prints its cases.

use brrrty_js::{js_number_to_string, js_to_fixed, js_to_locale_string};
use serde_json::Value as Json;

fn probes() -> Json {
    serde_json::from_str(include_str!("probes/number.json")).unwrap()
}

fn from_hex(s: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(s, 16).unwrap())
}

struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        Rng(if seed == 0 { 1 } else { seed })
    }
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    /// A whole number below 2^53.
    fn int53(&mut self) -> f64 {
        let hi = f64::from(self.next() & 0x1f_ffff);
        hi * 4_294_967_296.0 + f64::from(self.next())
    }
    fn below(&mut self, n: f64) -> f64 {
        self.int53() % n
    }
    fn signed(&mut self, x: f64) -> f64 {
        if self.next() & 1 == 1 { -x } else { x }
    }
    /// A double with a random sign and mantissa and the biased exponent `e`.
    fn with_exponent(&mut self, e: u32) -> f64 {
        let hi = ((self.next() & 1) << 31) | (e << 20) | (self.next() & 0xf_ffff);
        let lo = self.next();
        f64::from_bits((u64::from(hi) << 32) | u64::from(lo))
    }
    fn any(&mut self) -> f64 {
        let hi = self.next();
        let lo = self.next();
        f64::from_bits((u64::from(hi) << 32) | u64::from(lo))
    }
}

fn pow10(i: u32) -> f64 {
    format!("1e{i}").parse().unwrap()
}

/// The next double away from zero (`by` > 0) or toward it, by bit pattern.
fn step(x: f64, by: i64) -> f64 {
    f64::from_bits(x.to_bits().wrapping_add(by as u64))
}

/// `fixedCase` in the generator, call for call.
fn fixed_case(r: &mut Rng, kind: u32) -> (f64, u32) {
    match kind {
        0 => {
            let x = r.any();
            (x, r.next() % 101)
        }
        1 => {
            let e = 1023 - 80 + r.next() % 153;
            let x = r.with_exponent(e);
            (x, r.next() % 21)
        }
        2 => {
            let d = r.next() % 18;
            let p = pow10(1 + r.next() % 15);
            let m = r.below(p);
            let x = r.signed((m * 10.0 + 5.0) / pow10(d + 1));
            (x, d)
        }
        3 => {
            let j = 1 + r.next() % 60;
            let n = r.int53();
            let x = r.signed(n / 2f64.powi(j as i32));
            (x, r.next() % 31)
        }
        4 => {
            let v = r.below(1e12);
            let k = [3, 6, 9][(r.next() % 3) as usize];
            let x = r.signed(v / pow10(k));
            (x, r.next() % 4)
        }
        5 => {
            let e = r.next() % 60;
            let x = r.with_exponent(e);
            (x, r.next() % 101)
        }
        6 => {
            let (x, d) = fixed_case(r, 2);
            let by = if r.next() & 1 == 1 { 1 } else { -1 };
            (step(x, by), d)
        }
        7 => {
            let d = r.next() % 12;
            let nines = pow10(1 + r.next() % 12) - 1.0;
            let extra = f64::from(r.next() % 5);
            let x = r.signed((nines * 10.0 + 5.0 + extra) / pow10(d + 1));
            (x, d)
        }
        _ => {
            let x = 1e21 * (0.9 + f64::from(r.next() % 2000) / 10000.0);
            let by = i64::from(r.next() % 9) - 4;
            let x = r.signed(step(x, by));
            (x, r.next() % 4)
        }
    }
}

/// `localeCase` in the generator, call for call.
fn locale_case(r: &mut Rng, kind: u32) -> f64 {
    match kind {
        0 => r.any(),
        1 => {
            let e = 1023 - 40 + r.next() % 120;
            r.with_exponent(e)
        }
        2 => {
            let p = pow10(1 + r.next() % 16);
            let n = r.below(p);
            r.signed(n)
        }
        3 => {
            let d = r.next() % 7;
            let p = pow10(1 + r.next() % 15);
            let n = r.below(p) / pow10(d);
            r.signed(n)
        }
        4 => {
            let p = pow10(1 + r.next() % 12);
            let m = r.below(p);
            r.signed((m * 10.0 + 5.0) / 1e4)
        }
        5 => {
            let x = locale_case(r, 4);
            step(x, if r.next() & 1 == 1 { 1 } else { -1 })
        }
        6 => {
            let nines = pow10(1 + r.next() % 12) - 1.0;
            let extra = f64::from(r.next() % 5);
            r.signed((nines * 10.0 + 5.0 + extra) / 1e4)
        }
        _ => {
            let e = r.next() % 1023;
            r.with_exponent(e)
        }
    }
}

fn fnv(h: &mut u32, s: &str) {
    for b in s.bytes().chain(std::iter::once(b'\n')) {
        *h ^= u32::from(b);
        *h = h.wrapping_mul(0x0100_0193);
    }
}

fn fixed(x: f64, d: u32) -> String {
    js_to_fixed(x, d).unwrap()
}

#[test]
fn to_fixed_hand_picked_cases_match_v8() {
    let p = probes();
    let cases = p["to_fixed"].as_array().unwrap();
    assert!(cases.len() > 8_000);
    let mut bad = Vec::new();
    for c in cases {
        let x = from_hex(c[0].as_str().unwrap());
        let d = c[1].as_u64().unwrap() as u32;
        let want = c[2].as_str().unwrap();
        let got = fixed(x, d);
        if got != want {
            bad.push(format!("({x:?}).toFixed({d}): want {want}, got {got}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} differ:\n{}",
        bad.len(),
        bad[..bad.len().min(20)].join("\n")
    );
}

#[test]
fn to_fixed_fuzz_matches_v8() {
    let p = probes();
    let size = p["batch"].as_u64().unwrap() as u32;
    let hashes = p["to_fixed_batches"].as_array().unwrap();
    assert_eq!(hashes.len(), 600);
    let mut bad = Vec::new();
    for (b, want) in hashes.iter().enumerate() {
        let b = b as u32;
        let mut r = Rng::new(0x1f00_d000_u32.wrapping_add(b.wrapping_mul(7919)));
        let mut h = 0x811c_9dc5;
        for i in 0..size {
            let (x, d) = fixed_case(&mut r, (b + i) % 9);
            fnv(&mut h, &fixed(x, d));
        }
        if u64::from(h) != want.as_u64().unwrap() {
            bad.push(b);
        }
    }
    assert!(bad.is_empty(), "toFixed batches differ from V8: {bad:?}");
}

#[test]
fn to_fixed_refuses_digits_v8_refuses() {
    assert!(js_to_fixed(1.0, 101).is_err());
    assert_eq!(js_to_fixed(1.0, 100).unwrap().len(), 102);
}

#[test]
fn to_locale_string_hand_picked_cases_match_v8() {
    let p = probes();
    for c in p["to_locale_string"].as_array().unwrap() {
        let x = from_hex(c[0].as_str().unwrap());
        assert_eq!(
            js_to_locale_string(x),
            c[1].as_str().unwrap(),
            "({x:?}).toLocaleString('en-US')"
        );
    }
}

#[test]
fn to_locale_string_fuzz_matches_v8() {
    let p = probes();
    let size = p["batch"].as_u64().unwrap() as u32;
    let hashes = p["to_locale_string_batches"].as_array().unwrap();
    assert_eq!(hashes.len(), 300);
    let mut bad = Vec::new();
    for (b, want) in hashes.iter().enumerate() {
        let b = b as u32;
        let mut r = Rng::new(0x10ca_1e00_u32.wrapping_add(b.wrapping_mul(7919)));
        let mut h = 0x811c_9dc5;
        for i in 0..size {
            let x = locale_case(&mut r, (b + i) % 8);
            fnv(&mut h, &js_to_locale_string(x));
        }
        if u64::from(h) != want.as_u64().unwrap() {
            bad.push(b);
        }
    }
    assert!(
        bad.is_empty(),
        "toLocaleString batches differ from V8: {bad:?}"
    );
}

#[test]
fn to_string_fuzz_matches_v8() {
    let p = probes();
    let size = p["batch"].as_u64().unwrap() as u32;
    let hashes = p["to_string_batches"].as_array().unwrap();
    assert_eq!(hashes.len(), 300);
    let mut bad = Vec::new();
    for (b, want) in hashes.iter().enumerate() {
        let b = b as u32;
        let mut r = Rng::new(0x10ca_1e00_u32.wrapping_add(b.wrapping_mul(7919)));
        let mut h = 0x811c_9dc5;
        for i in 0..size {
            let x = locale_case(&mut r, (b + i) % 8);
            fnv(&mut h, &js_number_to_string(x));
        }
        if u64::from(h) != want.as_u64().unwrap() {
            bad.push(b);
        }
    }
    assert!(bad.is_empty(), "String(x) batches differ from V8: {bad:?}");
}
