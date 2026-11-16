// Regenerates tests/probes/sort.json: what V8's Array.prototype.sort does
// with consistent, inconsistent, random and throwing comparators, for every
// length from 0 to 2000. Run from crates/brrrty-config with Node 20 or later
// (the file was made with v26.2.0):
//
//   node tools/gen-sort-probes.cjs > tests/probes/sort.json
//
// Arrays are made by a seeded generator, so the output is the same on every
// run. Each item is { k, i }: a key the comparators look at, and its index
// before the sort. For each case the file has
// [length, comparator, order hash, calls hash, calls, threw]:
// FNV-1a hashes (32-bit) of the items' `i` in sorted order, and of every
// comparator call's (a.i, b.i) in order; how many calls there were; and 1
// when the comparator threw (then the order hash is 0).

"use strict";

function xorshift(seed) {
  let x = seed | 0 || 1;
  return () => {
    x ^= x << 13;
    x ^= x >>> 17;
    x ^= x << 5;
    return x >>> 0;
  };
}

// The array for length n. Patterns rotate with n: random keys with many
// repeats, ascending runs, descending runs, nearly sorted, and three keys.
function makeArray(n) {
  const r = xorshift(0x9e3779b9 ^ Math.imul(n + 1, 2654435761));
  const keys = [];
  const pattern = n % 5;
  if (pattern === 0) {
    const max = Math.max(1, n >> 2);
    for (let i = 0; i < n; i++) keys.push(r() % max);
  } else if (pattern === 1 || pattern === 2) {
    while (keys.length < n) {
      const len = Math.min(n - keys.length, (r() % 100) + 1);
      let k = r() % 1000;
      for (let j = 0; j < len; j++) {
        keys.push(k);
        k = pattern === 1 ? k + (r() % 3) : k - (r() % 3);
      }
    }
  } else if (pattern === 3) {
    for (let i = 0; i < n; i++) keys.push(i);
    for (let s = 0; s < n / 20; s++) {
      const a = r() % n;
      const b = r() % n;
      const t = keys[a];
      keys[a] = keys[b];
      keys[b] = t;
    }
  } else {
    for (let i = 0; i < n; i++) keys.push(r() % 3);
  }
  return keys.map((k, i) => ({ k, i }));
}

function pairHash(a, b) {
  return ((Math.imul(a.k * 31 + a.i, 2654435761) ^ (b.k * 17 + b.i)) >>> 0);
}

// The comparators, by index. Keep in step with tests/sort_probes.rs.
const COMPARATORS = [
  (a, b) => a.k - b.k, // 0: consistent
  (a, b) => a.k > b.k, // 1: a boolean, never negative
  (a, b) => a.k < b.k, // 2: a boolean, never negative
  () => 1, // 3: constant
  () => -1, // 4: constant
  (a, b) => (pairHash(a, b) % 3) - 1, // 5: random, but the same for a pair
  (a, b) => ((a.i + b.i) % 3 === 0 ? NaN : a.k - b.k), // 6: NaN is 0
  (a, b) => {
    // 7: random, and throws on some pairs
    const h = pairHash(a, b);
    if (h % 211 === 5) throw new Error("stop");
    return (h % 3) - 1;
  },
];

function fnv(h, x) {
  for (let s = 0; s < 32; s += 8) {
    h ^= (x >>> s) & 0xff;
    h = Math.imul(h, 16777619) >>> 0;
  }
  return h;
}

const cases = [];
for (let n = 0; n <= 2000; n++) {
  const base = makeArray(n);
  COMPARATORS.forEach((f, c) => {
    const items = base.slice();
    let calls = 0;
    let callsHash = 2166136261;
    let threw = 0;
    try {
      items.sort((a, b) => {
        calls++;
        callsHash = fnv(fnv(callsHash, a.i), b.i);
        return f(a, b);
      });
    } catch (e) {
      threw = 1;
    }
    let orderHash = 0;
    if (!threw) {
      orderHash = 2166136261;
      for (const it of items) orderHash = fnv(orderHash, it.i);
    }
    cases.push([n, c, orderHash, callsHash, calls, threw]);
  });
}

process.stdout.write(
  JSON.stringify({ node: process.version, cases }).replace(/\],\[/g, "],\n[") + "\n"
);
