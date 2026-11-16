// Regenerates tests/probes/filters.json: the filter and date probes, each
// run through the real LiquidJS 10.30.1 with strictFilters on.
//
//   LIQUIDJS=/path/to/node_modules/liquidjs \
//     node tools/gen-filter-probes.cjs > tests/probes/filters.json
//
// LIQUIDJS is the liquidjs package directory (version 10.30.1, as 11ty 3.1.8
// installs it). Run it from crates/brrrty-liquid with Node 20 or later (the
// file was made with v26.2.0).
//
// Templates and data are in filter-probe-sources.cjs. The "plain" and
// "dates_utc" lists run with TZ=UTC. The "zonal" list runs in each zone in
// ZONES below, and "zonal_parse" in the zones in PARSE_ZONES, each with its
// own TZ. The test (tests/filters.rs) replays them: UTC in process, the
// other zones in a child process with that TZ. Regenerating gives the
// same bytes (apart from the zone names, which come from this Node's ICU).

const fs = require('fs');
const { execFileSync } = require('child_process');
if (process.argv[2] === '--child') {
  const { Liquid } = require(process.env.LIQUIDJS || (() => { throw new Error('set LIQUIDJS to the liquidjs package directory'); })());
  const l = new Liquid({ strictFilters: true });
  const revive = (v) => {
    if (v && typeof v === 'object' && !Array.isArray(v)) {
      const k = Object.keys(v);
      if (k.length === 1 && k[0] === '$date') return new Date(v.$date);
      const o = {}; for (const x of k) o[x] = revive(v[x]); return o;
    }
    return Array.isArray(v) ? v.map(revive) : v;
  };
  const { shared, probes } = JSON.parse(fs.readFileSync(process.argv[3], 'utf8'));
  const res = [];
  for (const p of probes) {
    const data = p.data ?? shared;
    const probe = p.data ? { src: p.src, data: p.data } : { src: p.src };
    try {
      const r = l.parseAndRenderSync(p.src, revive(data));
      res.push({ ...probe, out: r.toWellFormed() });
    } catch (e) { res.push({ ...probe, err: e.message.toWellFormed() }); }
  }
  fs.writeFileSync(process.argv[4], JSON.stringify(res));
} else {
  const srcs = require('./filter-probe-sources.cjs');
  const tmp = require('path').join(require('os').tmpdir(), 'filter-probes-' + process.pid);
  const runIn = (tz, probes) => {
    fs.writeFileSync(tmp + '.in', JSON.stringify({ shared: srcs.data, probes }));
    execFileSync('node', [__filename, '--child', tmp + '.in', tmp + '.out'], { env: { ...process.env, TZ: tz }, stdio: 'inherit' });
    return JSON.parse(fs.readFileSync(tmp + '.out', 'utf8'));
  };
  const uniq = (a) => { const seen = new Set(); return a.filter((p) => { const k = p.src + JSON.stringify(p.data ?? 0); if (seen.has(k)) return false; seen.add(k); return true; }); };
  const plain = uniq(srcs.plain);
  const ZONES = ['UTC', 'America/New_York', 'Europe/London', 'Asia/Kolkata', 'Australia/Sydney', 'Pacific/Auckland', 'Asia/Kathmandu', 'America/St_Johns', 'Europe/Berlin', 'Asia/Tokyo', 'America/Sao_Paulo', 'Australia/Lord_Howe', 'America/Los_Angeles'];
  const PARSE_ZONES = ['UTC', 'America/New_York', 'Australia/Sydney', 'Asia/Kolkata', 'Europe/London'];
  const out = { data: srcs.data, plain: runIn('UTC', plain), dates_utc: runIn('UTC', uniq(srcs.dates_utc)), zones: {} };
  for (const z of ZONES) {
    out.zones[z] = { zonal: runIn(z, srcs.zonal) };
    if (PARSE_ZONES.includes(z)) out.zones[z].zonal_parse = runIn(z, srcs.zonal_parse);
  }
  fs.unlinkSync(tmp + '.in'); fs.unlinkSync(tmp + '.out');
  process.stdout.write(JSON.stringify(out).replace(/\},\{"src"/g, '},\n{"src"'));
}
