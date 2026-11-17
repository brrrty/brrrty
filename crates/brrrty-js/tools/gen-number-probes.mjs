// Regenerates tests/probes/number.json: what V8 gives for
// Number.prototype.toFixed and Number.prototype.toLocaleString('en-US'), and
// String(x) for the numbers toLocaleString gets.
// Run from crates/brrrty-js with Node 20 or later (the file was made with
// v26.2.0, ICU 78.3):
//
//   node tools/gen-number-probes.mjs > tests/probes/number.json
//   node tools/gen-number-probes.mjs dump to_fixed 17   # one batch, readable
//
// The file has hand-picked cases with their results, written out, and
// seeded fuzz batches stored as hashes: tests/number_probes.rs makes the same
// values from the same seeds and hashes its own results. A batch that
// differs is printed by its number; `dump` shows its cases.
//
// Numbers are written as their 64-bit patterns in hex, so -0 and NaN
// survive. A batch's hash is FNV-1a (32-bit) over the UTF-8 of each result
// followed by "\n".

const BATCH = 1000;
const FIXED_BATCHES = 600; // 600,000 toFixed cases
const LOCALE_BATCHES = 300; // 300,000 toLocaleString cases

function xorshift(seed) {
  let x = seed >>> 0 || 1;
  return () => {
    x ^= x << 13;
    x >>>= 0;
    x ^= x >>> 17;
    x ^= x << 5;
    x >>>= 0;
    return x;
  };
}

const view = new DataView(new ArrayBuffer(8));
const fromBits = (hi, lo) => {
  view.setUint32(0, hi >>> 0);
  view.setUint32(4, lo >>> 0);
  return view.getFloat64(0);
};
const hex = (x) => {
  view.setFloat64(0, x);
  return (
    view.getUint32(0).toString(16).padStart(8, '0') + view.getUint32(4).toString(16).padStart(8, '0')
  );
};
const bitsOf = (x) => {
  view.setFloat64(0, x);
  return [view.getUint32(0), view.getUint32(4)];
};
// The next double away from zero (or toward it with step -1).
const step = (x, by) => {
  const [hi, lo] = bitsOf(x);
  let l = lo + by;
  let h = hi;
  if (l > 0xffffffff) {
    l -= 0x100000000;
    h += 1;
  } else if (l < 0) {
    l += 0x100000000;
    h -= 1;
  }
  return fromBits(h, l);
};

// A whole number below 2^53.
const int53 = (r) => (r() & 0x1fffff) * 4294967296 + r();
const below = (r, n) => int53(r) % n;
// Exact powers of ten.
const POW10 = Array.from({ length: 23 }, (_, i) => Number(`1e${i}`));
// A random sign.
const signed = (r, x) => (r() & 1 ? -x : x);

// One toFixed case: [x, digits]. Each `kind` aims at a different corner.
function fixedCase(r, kind) {
  switch (kind) {
    case 0: // any double at all
      return [fromBits(r(), r()), r() % 101];
    case 1: {
      // |x| below about 4.7e21, any bits
      const e = 1023 - 80 + (r() % 153);
      return [fromBits(((r() & 1) << 31) | (e << 20) | (r() & 0xfffff), r()), r() % 21];
    }
    case 2: {
      // decimal ties that aren't exact in binary: m.5 x 10^-d
      const d = r() % 18;
      const m = below(r, POW10[1 + (r() % 15)]);
      return [signed(r, (m * 10 + 5) / POW10[d + 1]), d];
    }
    case 3: {
      // exact binary ties and halves: n / 2^j
      const j = 1 + (r() % 60);
      return [signed(r, int53(r) / 2 ** j), r() % 31];
    }
    case 4: {
      // whole dollars scaled down, the way prices are shown
      const v = below(r, 1e12);
      const k = [3, 6, 9][r() % 3];
      return [signed(r, v / POW10[k]), r() % 4];
    }
    case 5: {
      // tiny numbers, subnormals included
      const e = r() % 60;
      return [fromBits(((r() & 1) << 31) | (e << 20) | (r() & 0xfffff), r()), r() % 101];
    }
    case 6: {
      // the doubles either side of a decimal tie
      const [x, d] = fixedCase(r, 2);
      return [step(x, r() & 1 ? 1 : -1), d];
    }
    case 7: {
      // results that carry: 9.99...5 and friends
      const d = r() % 12;
      const nines = POW10[1 + (r() % 12)] - 1;
      return [signed(r, (nines * 10 + 5 + (r() % 5)) / POW10[d + 1]), d];
    }
    default: {
      // near 1e21, where toFixed switches to String(x)
      const x = 1e21 * (0.9 + (r() % 2000) / 10000);
      return [signed(r, step(x, (r() % 9) - 4)), r() % 4];
    }
  }
}
const FIXED_KINDS = 9;

// One toLocaleString case.
function localeCase(r, kind) {
  switch (kind) {
    case 0:
      return fromBits(r(), r());
    case 1: {
      const e = 1023 - 40 + (r() % 120);
      return fromBits(((r() & 1) << 31) | (e << 20) | (r() & 0xfffff), r());
    }
    case 2: // whole numbers
      return signed(r, below(r, POW10[1 + (r() % 16)]));
    case 3: {
      // up to six decimals
      const d = r() % 7;
      return signed(r, below(r, POW10[1 + (r() % 15)]) / POW10[d]);
    }
    case 4: {
      // ties at the third decimal: m.5 x 10^-3
      const m = below(r, POW10[1 + (r() % 12)]);
      return signed(r, (m * 10 + 5) / 1e4);
    }
    case 5: {
      // the doubles either side of those ties
      return step(localeCase(r, 4), r() & 1 ? 1 : -1);
    }
    case 6: {
      // rounding that carries into more digits
      const nines = POW10[1 + (r() % 12)] - 1;
      return signed(r, (nines * 10 + 5 + (r() % 5)) / 1e4);
    }
    default: {
      // tiny numbers
      const e = r() % 1023;
      return fromBits(((r() & 1) << 31) | (e << 20) | (r() & 0xfffff), r());
    }
  }
}
const LOCALE_KINDS = 8;

const toFixed = (x, d) => x.toFixed(d);
const toLocale = (x) => x.toLocaleString('en-US');

const enc = new TextEncoder();
function fnv(strings) {
  let h = 0x811c9dc5;
  for (const s of strings) {
    for (const b of enc.encode(s + '\n')) {
      h ^= b;
      h = Math.imul(h, 0x01000193) >>> 0;
    }
  }
  return h;
}

const fixedSeed = (b) => (0x1f00d000 + b * 7919) >>> 0;
const localeSeed = (b) => (0x10ca1e00 + b * 7919) >>> 0;

function fixedBatch(b) {
  const r = xorshift(fixedSeed(b));
  const out = [];
  for (let i = 0; i < BATCH; i++) out.push(fixedCase(r, (b + i) % FIXED_KINDS));
  return out;
}
function localeBatch(b) {
  const r = xorshift(localeSeed(b));
  const out = [];
  for (let i = 0; i < BATCH; i++) out.push(localeCase(r, (b + i) % LOCALE_KINDS));
  return out;
}

// Hand-picked cases.
function fixedHand() {
  const xs = [
    0, -0, 0.5, 1.5, 2.5, -2.5, -0.5, 1.005, 1.045, 1.255, 0.05, 0.15, 0.25, 0.35, 0.45,
    -0.0001, -1e-30, 1e21, -1e21, 999999999999999900000, 1e21 - 65536 * 2, NaN, Infinity,
    -Infinity, 0.000001, 5e-324, -5e-324, 2.2250738585072014e-308, 1.7976931348623157e308,
    123.456, 0.1, 0.2, 0.3, 0.1 + 0.2, 1 / 3, 2 / 3, 10.235, 1.45, 8.345, 4.35, 9.995, 99.995,
    0.000005, 0.0000005, 1e-7, 1.5e-7, 123456789.125, 2 ** 52 + 0.5, 2 ** 53, 2 ** 53 + 2,
    2 ** 63, 2 ** 64, 2 ** 69, 4.35 * 100, 1234.5678, 1.6e6 / 1e6, 2.45e6 / 1e6, 12.5e9 / 1e9,
  ];
  const out = [];
  for (const x of xs) for (let d = 0; d <= 100; d++) out.push([x, d]);
  for (let n = -100; n <= 100; n++) {
    for (let d = 0; d <= 3; d++) out.push([n + 0.5, d], [n / 8, d], [n / 1000 + 0.0005, d]);
  }
  return out;
}
function localeHand() {
  return [
    0, -0, 1, -1, 1234.5, 1234.5678, -1234.5675, 0.0005, -0.0004, -0.0005, 1.0005, 2.0005,
    1e21, 1.5e21, 123456789.12345, NaN, Infinity, -Infinity, 1e-7, 0.9995, 0.99949999,
    9999.9995, 999.9995, 1e300, 1.7976931348623157e308, 5e-324, 2 ** 53, 2 ** 53 + 2,
    123456789012345680000, 1e16, 12345678901234567, 0.1 + 0.2, 1 / 3, 1000, 10000, 100000,
    999999.9999, 1234567.891, -987654.3215, 0.125, 0.0625, 1.2345e25, 4.35, 1e-300,
  ];
}

const mode = process.argv[2];
if (mode === 'dump') {
  const [which, n] = [process.argv[3], Number(process.argv[4])];
  if (which === 'to_fixed') {
    for (const [x, d] of fixedBatch(n)) console.log(hex(x), x, d, JSON.stringify(toFixed(x, d)));
  } else {
    for (const x of localeBatch(n)) console.log(hex(x), JSON.stringify(toLocale(x)), String(x));
  }
} else {
  const out = {
    node: process.version,
    icu: process.versions.icu,
    batch: BATCH,
    to_fixed: fixedHand().map(([x, d]) => [hex(x), d, toFixed(x, d)]),
    to_fixed_batches: Array.from({ length: FIXED_BATCHES }, (_, b) =>
      fnv(fixedBatch(b).map(([x, d]) => toFixed(x, d))),
    ),
    to_locale_string: localeHand().map((x) => [hex(x), toLocale(x)]),
    to_locale_string_batches: Array.from({ length: LOCALE_BATCHES }, (_, b) =>
      fnv(localeBatch(b).map(toLocale)),
    ),
    // String(x) on the same numbers: toLocaleString starts from its digits.
    to_string_batches: Array.from({ length: LOCALE_BATCHES }, (_, b) =>
      fnv(localeBatch(b).map(String)),
    ),
  };
  // One case per line keeps diffs readable.
  const lines = ['{'];
  const keys = Object.keys(out);
  keys.forEach((k, i) => {
    const comma = i < keys.length - 1 ? ',' : '';
    const v = out[k];
    if (Array.isArray(v) && Array.isArray(v[0])) {
      lines.push(`  ${JSON.stringify(k)}: [`);
      v.forEach((c, j) => lines.push(`    ${JSON.stringify(c)}${j < v.length - 1 ? ',' : ''}`));
      lines.push(`  ]${comma}`);
    } else {
      lines.push(`  ${JSON.stringify(k)}: ${JSON.stringify(v)}${comma}`);
    }
  });
  lines.push('}');
  console.log(lines.join('\n'));
}
