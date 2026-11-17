// Regenerates tests/probes/dates.json: `new Date(y, m, d, h, mi, s, ms)` in
// the local zone, the local getters, and toISOString, as V8 gives them in
// several zones. Run from crates/brrrty-js with Node 20 or later (the file
// was made with v26.2.0):
//
//   node tools/gen-date-probes.mjs > tests/probes/dates.json
//
// five zones, each in its own process. Numbers that JSON can't hold (NaN) are written as null.
//
// `sweep` covers every zone Node knows: the offset every 3 days (at a
// different hour each time) from 1850 to 2045, and around 2100 and at the
// ends of the date range, as getTimezoneOffset() gives it (minutes west,
// seconds dropped), stored as one FNV-1a hash per zone. The offsets are read
// from ICU through Intl.DateTimeFormat rather than from Date: V8's date
// cache can answer from an earlier date near a transition once a process
// has made many dates (a fresh `new Date(t)` gives ICU's offset).

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

import { execFileSync } from 'node:child_process';
import { writeSync } from 'node:fs';

// One zone's sweep, run as `node gen-date-probes.mjs sweep-one` with TZ set.
function sweepInstants() {
  const out = [];
  const start = Date.UTC(1850, 0, 1);
  const end = Date.UTC(2045, 0, 1);
  for (let t = start, i = 0; t < end; t += 3 * 86400000, i++) out.push(t + (i % 24) * 3600000 + (i % 7) * 60000);
  for (let i = 0; i < 400; i++) out.push(Date.UTC(2099, 0, 1) + i * 86400000 * 1.37);
  for (let i = 0; i < 100; i++) out.push(8.64e15 - i * 86400000 * 3.3, -8.64e15 + i * 86400000 * 3.3);
  return out.map(Math.trunc);
}
function fnv(strings) {
  let h = 0x811c9dc5;
  for (const s of strings) {
    for (const c of s + '\n') {
      h ^= c.charCodeAt(0);
      h = Math.imul(h, 0x01000193) >>> 0;
    }
  }
  return h;
}
function zoneProbes(zone) {
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
  return { ctor, instants };
}
if (process.argv[2] === 'zone-one') {
  writeSync(1, JSON.stringify(zoneProbes(process.argv[3])) + '\n');
  process.exit(0);
}
const result = { node: process.version, tz: process.versions.tz, zones: {}, sweep: {} };
const links = ['US/Pacific', 'US/Eastern', 'Asia/Calcutta', 'EST5EDT', 'CET', 'NZ', 'Etc/GMT+5', 'GB', 'Japan'];
// ICU's offset at t, as getTimezoneOffset() reports it.
function offsetWest(fmt, t) {
  const name = fmt.formatToParts(t).find((p) => p.type === 'timeZoneName').value;
  const m = /^GMT(?:([+-])(\d\d):(\d\d)(?::(\d\d))?)?$/.exec(name);
  if (!m) throw new Error(`offset ${name}`);
  if (!m[1]) return 0;
  const east = (m[1] === '-' ? -1 : 1) * (Number(m[2]) * 3600 + Number(m[3]) * 60 + Number(m[4] || 0));
  return Math.trunc(-east / 60) + 0;
}
const instants = sweepInstants();
for (const zone of [...Intl.supportedValuesOf('timeZone'), ...links]) {
  const fmt = new Intl.DateTimeFormat('en-US', { timeZone: zone, timeZoneName: 'longOffset' });
  result.sweep[zone] = fnv(instants.map((t) => String(offsetWest(fmt, t))));
}
for (const zone of ZONES) {
  const out = execFileSync(process.execPath, [new URL(import.meta.url).pathname, 'zone-one', zone], {
    env: { ...process.env, TZ: zone },
    maxBuffer: 1 << 28,
  });
  result.zones[zone] = JSON.parse(String(out));
}
console.log(JSON.stringify(result));
