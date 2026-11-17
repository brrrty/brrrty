// Regenerates tests/probes/dates.json: `new Date(y, m, d, h, mi, s, ms)` in
// the local zone, the local getters, and toISOString, as V8 gives them in
// several zones. Run from crates/brrrty-js with Node 20 or later (the file
// was made with v26.2.0):
//
//   node tools/gen-date-probes.mjs > tests/probes/dates.json
//
// Node rereads TZ when process.env.TZ is assigned, so one run covers every
// zone. Numbers that JSON can't hold (NaN) are written as null.

const ZONES = ['America/Chicago', 'Europe/London', 'Australia/Lord_Howe', 'Asia/Kolkata', 'UTC'];

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

const num = (x) => (Number.isNaN(x) ? null : Object.is(x, -0) ? 0 : x);
const getters = (d) =>
  [
    d.getTime(),
    d.getFullYear(),
    d.getMonth(),
    d.getDate(),
    d.getDay(),
    d.getHours(),
    d.getMinutes(),
    d.getSeconds(),
    d.getMilliseconds(),
    d.getTimezoneOffset(),
  ].map(num);
const iso = (d) => {
  try {
    return d.toISOString();
  } catch (e) {
    return null;
  }
};

// Hand-picked constructor arguments: rollover, two-digit years, the edges
// of the date range, fractions, NaN, and clock changes in each zone.
const hand = [
  [2026, 0, 1], [2026, 4, 1], [2026, 4, 10], [2026, 5, 21], [2026, 11, 25], [2026, 1, 29],
  [2024, 1, 29], [2026, 12, 1], [2026, 13, 1], [2026, -1, 1], [2026, -13, 1], [2026, 0, 0],
  [2026, 0, 32], [2026, 0, -400], [2026, 0, 1, 25], [2026, 0, 1, -1], [2026, 0, 1, 0, 90],
  [2026, 0, 1, 0, 0, 3600], [2026, 0, 1, 0, 0, 0, 86400001], [0, 0, 1], [99, 11, 31], [100, 0, 1],
  [-1, 0, 1], [-271821, 3, 20], [-271821, 3, 19], [275760, 8, 13], [275760, 8, 13, 0, 0, 0, 1],
  [275760, 8, 14], [1e9, 0, 1], [2026.7, 0.9, 1.9, 1.5, 1.5, 1.5, 1.5], [-0.5, 0, 1],
  [NaN, 0, 1], [2026, NaN, 1], [2026, 0, NaN], [2026, 0, 1, 0, 0, 0, NaN], [Infinity, 0, 1],
  [2026, 0], [1970, 0, 1], [1969, 11, 31, 23, 59, 59, 999],
  [2026, 2, 8, 1, 59], [2026, 2, 8, 2, 0], [2026, 2, 8, 2, 30], [2026, 2, 8, 3, 0],
  [2026, 10, 1, 0, 59], [2026, 10, 1, 1, 0], [2026, 10, 1, 1, 30], [2026, 10, 1, 2, 0],
  [2026, 2, 29, 0, 30], [2026, 2, 29, 1, 30], [2026, 2, 29, 2, 30], [2026, 9, 25, 0, 30],
  [2026, 9, 25, 1, 30], [2026, 9, 25, 2, 30], [2026, 3, 5, 1, 30], [2026, 3, 5, 1, 45],
  [2026, 3, 5, 2, 0], [2026, 9, 4, 2, 0], [2026, 9, 4, 2, 15], [2026, 9, 4, 2, 30],
  [1883, 10, 18, 11, 0], [1883, 10, 18, 12, 30], [1900, 0, 1], [1847, 11, 1], [1941, 9, 1],
  [2100, 6, 23], [2200, 2, 14, 2, 30], [2038, 0, 19, 3, 14, 8],
];

function fuzz(r, n) {
  const out = [];
  for (let i = 0; i < n; i++) {
    const k = 2 + (r() % 6);
    const args = [1890 + (r() % 230), (r() % 43) - 15, (r() % 46) - 5, (r() % 33) - 2, (r() % 70) - 5, (r() % 70) - 5, (r() % 2200) - 100];
    if (r() % 8 === 0) args[r() % 7] += (r() % 1000) / 1000;
    out.push(args.slice(0, k));
  }
  return out;
}

const result = { node: process.version, zones: {} };
for (const zone of ZONES) {
  process.env.TZ = zone;
  const r = xorshift(0xda7e0000 + zone.length * 7919);
  const ctor = [...hand, ...fuzz(r, 3000)].map((args) => {
    const d = new Date(...args);
    return [args.map(num), getters(d), iso(d)];
  });
  const instants = [];
  for (let i = 0; i < 1500; i++) {
    // Across 1800-2200 and, now and then, the far edges of the range.
    const t =
      r() % 10 === 0
        ? (r() % 2 ? 1 : -1) * (8.64e15 + 1000 - (r() % 1e9) * 4096)
        : -5364662400000 + ((r() * 4096 + (r() % 4096)) % 12623040000000);
    const ms = Math.trunc(t);
    const d = new Date(ms);
    instants.push([num(ms), getters(d), iso(d)]);
  }
  result.zones[zone] = { ctor, instants };
}
console.log(JSON.stringify(result));
