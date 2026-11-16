//! `Array.prototype.sort` against V8: `tests/probes/sort.json` (made by
//! `tools/gen-sort-probes.cjs` in Node) has, for every length from 0 to 2000
//! and eight comparators (consistent, two that return booleans, two
//! constants, a random one, one that returns `NaN`, and a random one that
//! throws), a hash of V8's result, a hash of every comparator call's
//! arguments in order, and the number of calls. The port has to match all
//! three, so it makes the same calls as V8 and gives V8's order even for a
//! comparator that isn't an ordering.

use std::cmp::Ordering;

use brrrty_config::timsort;
use brrrty_js::Value;
use serde_json::Value as Json;

#[derive(Clone, Copy, Debug)]
struct Item {
    k: i64,
    i: u32,
}

fn xorshift(seed: u32) -> impl FnMut() -> u32 {
    let mut x = if seed == 0 { 1 } else { seed };
    move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    }
}

/// `makeArray` in the generator.
fn make_array(n: usize) -> Vec<Item> {
    let mut r = xorshift(0x9e37_79b9 ^ ((n as u32 + 1).wrapping_mul(2_654_435_761)));
    let mut keys: Vec<i64> = Vec::new();
    match n % 5 {
        0 => {
            let max = (n as u32 >> 2).max(1);
            for _ in 0..n {
                keys.push((r() % max) as i64);
            }
        }
        p @ (1 | 2) => {
            while keys.len() < n {
                let len = (n - keys.len()).min((r() % 100) as usize + 1);
                let mut k = (r() % 1000) as i64;
                for _ in 0..len {
                    keys.push(k);
                    let step = (r() % 3) as i64;
                    k = if p == 1 { k + step } else { k - step };
                }
            }
        }
        3 => {
            keys.extend(0..n as i64);
            let mut s = 0.0;
            while s < n as f64 / 20.0 {
                let a = (r() % n as u32) as usize;
                let b = (r() % n as u32) as usize;
                keys.swap(a, b);
                s += 1.0;
            }
        }
        _ => {
            for _ in 0..n {
                keys.push((r() % 3) as i64);
            }
        }
    }
    keys.into_iter()
        .enumerate()
        .map(|(i, k)| Item { k, i: i as u32 })
        .collect()
}

fn pair_hash(a: &Item, b: &Item) -> u32 {
    ((a.k * 31 + a.i as i64) as u32).wrapping_mul(2_654_435_761) ^ ((b.k * 17 + b.i as i64) as u32)
}

fn fnv(mut h: u32, x: u32) -> u32 {
    for s in (0..32).step_by(8) {
        h ^= (x >> s) & 0xff;
        h = h.wrapping_mul(16_777_619);
    }
    h
}

const FNV_START: u32 = 2_166_136_261;

/// The comparators of the generator, as the number they return (`None`
/// when they throw).
fn compare(c: u64, a: &Item, b: &Item) -> Option<f64> {
    Some(match c {
        0 => (a.k - b.k) as f64,
        1 => f64::from(u8::from(a.k > b.k)),
        2 => f64::from(u8::from(a.k < b.k)),
        3 => 1.0,
        4 => -1.0,
        5 => (pair_hash(a, b) % 3) as f64 - 1.0,
        6 if (a.i + b.i).is_multiple_of(3) => f64::NAN,
        6 => (a.k - b.k) as f64,
        7 => {
            let h = pair_hash(a, b);
            if h % 211 == 5 {
                return None;
            }
            (h % 3) as f64 - 1.0
        }
        _ => unreachable!(),
    })
}

fn order(n: f64) -> Ordering {
    if n < 0.0 {
        Ordering::Less
    } else if n > 0.0 {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

fn probes() -> Vec<Vec<u64>> {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/probes/sort.json"
    ))
    .unwrap();
    let json: Json = serde_json::from_str(&text).unwrap();
    json["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            c.as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_u64().unwrap())
                .collect()
        })
        .collect()
}

#[test]
fn the_port_makes_v8s_calls_and_gives_v8s_order() {
    let cases = probes();
    assert_eq!(cases.len(), 2001 * 8);
    let mut threw = 0;
    for case in &cases {
        let [n, c, want_order, want_calls_hash, want_calls, want_threw] = case[..] else {
            panic!("{case:?}")
        };
        let mut items = make_array(n as usize);
        let mut calls = 0u64;
        let mut calls_hash = FNV_START;
        let result = timsort::sort(&mut items, &mut |a: &Item, b: &Item| {
            calls += 1;
            calls_hash = fnv(fnv(calls_hash, a.i), b.i);
            compare(c, a, b).map(order).ok_or(())
        });
        let got_order = match result {
            Ok(()) => items.iter().fold(FNV_START, |h, it| fnv(h, it.i)),
            Err(()) => 0,
        };
        let got = (
            got_order as u64,
            calls_hash as u64,
            calls,
            result.is_err() as u64,
        );
        assert_eq!(
            got,
            (want_order, want_calls_hash, want_calls, want_threw),
            "length {n}, comparator {c}"
        );
        threw += want_threw;
    }
    // The throwing comparator throws in most cases, so the calls up to a
    // throw are checked too.
    assert!(threw > 1500, "{threw}");
}

/// The same probes through Rhai's `sort(fn)` and `js_sort_by`, with the
/// comparators written in Rhai: what a port author writes.
#[test]
fn rhai_sort_gives_v8s_order() {
    let cases = probes();
    let rhai_cmp = [
        "|a, b| a[0] - b[0]",
        "|a, b| a[0] > b[0]",
        "|a, b| a[0] < b[0]",
        "|a, b| 1",
        "|a, b| -1",
        "|a, b| ((((a[0] * 31 + a[1]) * 2654435761) & 0xFFFFFFFF) ^ (b[0] * 17 + b[1])) % 3 - 1",
    ];
    let mut script = String::from("let out = object();\n");
    let lengths = [
        0usize, 1, 2, 31, 32, 63, 64, 65, 127, 128, 200, 641, 1000, 1999, 2000,
    ];
    for n in lengths {
        let items: Vec<String> = make_array(n)
            .iter()
            .map(|it| format!("[{}, {}]", it.k, it.i))
            .collect();
        script.push_str(&format!("let a{n} = [{}];\n", items.join(", ")));
        for (c, f) in rhai_cmp.iter().enumerate() {
            let call = if c % 2 == 0 {
                format!("let s = a{n}; s.sort({f});")
            } else {
                format!("let s = js_sort_by(a{n}, {f});")
            };
            script.push_str(&format!(
                "{{ {call} let r = []; for x in s {{ r.push(x[1]); }} out[\"{n}/{c}\"] = r; }}\n"
            ));
        }
    }
    script.push_str("config.add_global_data(\"out\", out);\n");
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join(brrrty_config::CONFIG_FILE), script).unwrap();
    let cfg = brrrty_config::load_with(t.path(), brrrty_config::Clock::Fixed(0))
        .unwrap()
        .unwrap();
    let Value::Object(out) = &cfg.global_data["out"] else {
        panic!("not an object")
    };
    for n in lengths {
        for c in 0..rhai_cmp.len() {
            let Some(Value::Array(sorted)) = out.get(&format!("{n}/{c}")) else {
                panic!("no result for {n}/{c}")
            };
            let got = sorted.iter().fold(FNV_START, |h, i| match i {
                Value::Number(i) => fnv(h, *i as u32),
                other => panic!("{other:?}"),
            });
            let want = cases
                .iter()
                .find(|x| x[0] == n as u64 && x[1] == c as u64)
                .unwrap();
            assert_eq!(got as u64, want[2], "length {n}, comparator {c}");
        }
    }
}
