// Times Brrrty on the JavaScript data sites generate.mjs writes, and checks
// its output against 11ty's.
//
//   cargo build --release -p brrrty
//   node bench/jsdata/run.mjs <work-folder>
//
// It generates the sites into <work-folder>, builds each RUNS times with
// Brrrty (`--js-data=node`), and prints the times. With ELEVENTY=docker it
// also times 11ty 3.1.8 in Docker (node:22, `npm ci` inside the container;
// see eleventy-docker.sh) and compares every file the two wrote.
//
// Settings, from the environment:
//   RUNS=3                 timed runs per tool
//   BRRRTY=path            the Brrrty binary (default target/release/brrrty)
//   SITES=calls,templates  which sites
//   ELEVENTY=docker        also run 11ty
// PAGES, ITEMS, TEMPLATE_PAGES and SINGLES pass through to generate.mjs.

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..", "..");
const work = resolve(process.argv[2] ?? join(here, "out"));
const RUNS = Number(process.env.RUNS ?? 3);
const BRRRTY = resolve(root, process.env.BRRRTY ?? "target/release/brrrty");
const SITES = (process.env.SITES ?? "calls,templates").split(",");

if (!existsSync(BRRRTY)) {
  console.error(`No Brrrty binary at ${BRRRTY}. Run: cargo build --release -p brrrty`);
  process.exit(1);
}
const gen = spawnSync(process.execPath, [join(here, "generate.mjs"), work], { stdio: "inherit" });
if (gen.status !== 0) process.exit(1);

function files(dir) {
  const out = [];
  const walk = (d) => {
    for (const name of readdirSync(d)) {
      const p = join(d, name);
      if (statSync(p).isDirectory()) walk(p);
      else out.push(relative(dir, p));
    }
  };
  if (existsSync(dir)) walk(dir);
  return out.sort();
}

const rows = [];
let failed = false;
for (const name of SITES) {
  const site = join(work, name);
  const times = [];
  for (let i = 0; i < RUNS; i++) {
    rmSync(join(site, "_site"), { recursive: true, force: true });
    const start = performance.now();
    const r = spawnSync(BRRRTY, ["build", "--js-data=node", "--quiet"], { cwd: site, stdio: ["ignore", "inherit", "inherit"] });
    times.push((performance.now() - start) / 1000);
    if (r.status !== 0) {
      console.error(`Brrrty failed on ${name}`);
      process.exit(1);
    }
  }
  const row = { name, brrrty: Math.min(...times), eleventy: null, same: null };
  if (process.env.ELEVENTY === "docker") {
    const out11 = join(work, "_11ty", name);
    const r = spawnSync(join(here, "eleventy-docker.sh"), [site, out11, String(RUNS)], { encoding: "utf8" });
    process.stderr.write(r.stderr);
    if (r.status !== 0) {
      console.error(`11ty failed on ${name}`);
      process.exit(1);
    }
    const runs = [...r.stdout.matchAll(/11ty run \d+: ([\d.]+) s/g)].map((m) => Number(m[1]));
    row.eleventy = Math.min(...runs);
    const a = files(join(out11, "_site"));
    const b = files(join(site, "_site"));
    row.same = a.length === b.length && a.every((f, i) => f === b[i] && readFileSync(join(out11, "_site", f)).equals(readFileSync(join(site, "_site", f))));
    if (!row.same) failed = true;
  }
  rows.push(row);
}

console.log("\n| site | Brrrty (min of " + RUNS + ") | 11ty (min of " + RUNS + ") | same output |");
console.log("|---|---:|---:|---|");
for (const r of rows) {
  console.log(`| ${r.name} | ${r.brrrty.toFixed(2)} s | ${r.eleventy === null ? "-" : r.eleventy.toFixed(2) + " s"} | ${r.same === null ? "-" : r.same ? "yes" : "NO"} |`);
}
process.exit(failed ? 1 : 0);
