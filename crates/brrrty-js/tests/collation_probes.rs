//! `String.prototype.localeCompare` against V8 in Node's en-US locale:
//! `tests/probes/collation.json`, made by `tools/gen-collation-probes.mjs`
//! (Node v26.2.0, ICU 78.3). Hand-picked pairs are compared one by one;
//! 300,000 fuzzed pairs, drawn from the file's alphabet, are made again
//! here from the same seeds and compared by hash. `dump N` in the
//! generator prints a batch.

use std::cmp::Ordering;

use brrrty_js::js_locale_compare;
use serde_json::Value as Json;

fn probes() -> Json {
    serde_json::from_str(include_str!("probes/collation.json")).unwrap()
}

fn sign(o: Ordering) -> char {
    match o {
        Ordering::Less => '<',
        Ordering::Equal => '=',
        Ordering::Greater => '>',
    }
}

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

struct Gen<'a> {
    r: Rng,
    ascii: &'a [char],
    alphabet: &'a [char],
}

impl Gen<'_> {
    fn pick(&mut self) -> char {
        let v = self.r.next();
        if !v.is_multiple_of(4) {
            self.ascii[((v >> 2) as usize) % self.ascii.len()]
        } else {
            self.alphabet[((v >> 2) as usize) % self.alphabet.len()]
        }
    }

    /// `pair` in the generator, call for call.
    fn pair(&mut self) -> (String, String) {
        let len = self.r.next() % 9;
        let a: Vec<char> = (0..len).map(|_| self.pick()).collect();
        let mut b = a.clone();
        match self.r.next() % 4 {
            0 => {
                let n = self.r.next() % 9;
                b = (0..n).map(|_| self.pick()).collect();
            }
            1 => {
                if len > 0 {
                    let i = (self.r.next() % len) as usize;
                    b[i] = self.pick();
                }
            }
            2 => {
                let i = (self.r.next() % (len + 1)) as usize;
                let c = self.pick();
                b.insert(i, c);
            }
            _ => {
                if len > 0 {
                    b.remove((self.r.next() % len) as usize);
                }
            }
        }
        (a.into_iter().collect(), b.into_iter().collect())
    }
}

#[test]
fn hand_picked_pairs_match_v8() {
    let p = probes();
    let pairs = p["pairs"].as_array().unwrap();
    assert!(pairs.len() > 10_000);
    let mut bad = Vec::new();
    for c in pairs {
        let (a, b) = (c[0].as_str().unwrap(), c[1].as_str().unwrap());
        let want = c[2].as_str().unwrap().chars().next().unwrap();
        let got = sign(js_locale_compare(a, b).unwrap());
        if got != want {
            bad.push(format!(
                "{a:?}.localeCompare({b:?}): want {want}, got {got}"
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "{} differ:\n{}",
        bad.len(),
        bad[..bad.len().min(30)].join("\n")
    );
}

#[test]
fn fuzzed_pairs_match_v8() {
    let p = probes();
    let size = p["batch"].as_u64().unwrap();
    let alphabet: Vec<char> = p["alphabet"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| char::from_u32(c.as_u64().unwrap() as u32).unwrap())
        .collect();
    let ascii = &alphabet[..95];
    let hashes = p["batches"].as_array().unwrap();
    assert_eq!(hashes.len(), 300);
    let mut bad = Vec::new();
    for (b, want) in hashes.iter().enumerate() {
        let seed = 0xc011_a7e0_u32.wrapping_add((b as u32).wrapping_mul(7919));
        let mut g = Gen {
            r: Rng(if seed == 0 { 1 } else { seed }),
            ascii,
            alphabet: &alphabet,
        };
        let mut h: u32 = 0x811c_9dc5;
        for _ in 0..size {
            let (x, y) = g.pair();
            h ^= sign(js_locale_compare(&x, &y).unwrap()) as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
        if u64::from(h) != want.as_u64().unwrap() {
            bad.push(b);
        }
    }
    assert!(
        bad.is_empty(),
        "localeCompare batches differ from V8: {bad:?}"
    );
}
