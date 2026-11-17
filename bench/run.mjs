// Builds the benchmark blog with 11ty and with Brrrty, alternating, times each
// run, then compares every file of the last pair. Prints a Markdown table, and
// appends it to $GITHUB_STEP_SUMMARY when that is set. Exits 1 if a build
// fails or any file differs, is missing or is extra.
//
//   cargo build --release -p brrrty
//   npm ci --prefix bench/site
//   node bench/run.mjs
//
// Settings, from the environment:
//   RUNS=5               timed runs per tool
//   WARMUP=1             untimed runs per tool first, to warm the disk cache
//   BRRRTY=path          the Brrrty binary (default target/release/brrrty)
//   SOURCE_DATE_EPOCH=n  the frozen clock (default 2026-01-01T00:00:00Z)
//   SKIP_GENERATE=1      keep bench/site/src/posts and images as they are

import { spawnSync } from "node:child_process";
import { appendFileSync, existsSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { cpus, platform, arch, release } from "node:os";
import { dirname, join, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const site = join(here, "site");

const RUNS = Number(process.env.RUNS ?? 5);
const WARMUP = Number(process.env.WARMUP ?? 1);
const BRRRTY = resolve(root, process.env.BRRRTY ?? "target/release/brrrty");
const EPOCH = process.env.SOURCE_DATE_EPOCH ?? "1767225600";
const FREEZE = join(root, "xtask", "src", "freeze-time.cjs");
const ELEVENTY = join(site, "node_modules", "@11ty", "eleventy", "cmd.cjs");

function fail(message) {
  console.error(message);
  process.exit(1);
}
if (!Number.isInteger(RUNS) || RUNS < 1) fail(`RUNS must be a whole number from 1, got ${process.env.RUNS}`);
if (!Number.isInteger(WARMUP) || WARMUP < 0) fail(`WARMUP must be a whole number, got ${process.env.WARMUP}`);
if (!existsSync(BRRRTY)) fail(`No Brrrty binary at ${BRRRTY}. Run: cargo build --release -p brrrty`);
if (!existsSync(ELEVENTY)) fail(`11ty isn't installed. Run: npm ci --prefix bench/site`);

if (process.env.SKIP_GENERATE !== "1") {
  const gen = spawnSync(process.execPath, [join(here, "generate.mjs")], { stdio: "inherit" });
  if (gen.status !== 0) fail("bench/generate.mjs failed");
}

// The same clock and zone for both, every run, so the compared pair can't
// differ by when it ran. Brrrty reads SOURCE_DATE_EPOCH itself; 11ty gets it
// from the freeze-time preload.
const env = { ...process.env, SOURCE_DATE_EPOCH: EPOCH, TZ: "UTC" };

const version = (cmd, args) => {
  const r = spawnSync(cmd, args, { cwd: site, env, encoding: "utf8" });
  return (r.stdout || "").trim();
};
const eleventyVersion = JSON.parse(
  readFileSync(join(site, "node_modules", "@11ty", "eleventy", "package.json"), "utf8"),
).version;
const brrrtyVersion = version(BRRRTY, ["--version"]).replace(/^brrrty /, "");

const tools = [
  {
    name: "11ty",
    label: `11ty ${eleventyVersion}`,
    out: "_site-11ty",
    cmd: process.execPath,
    args: ["--require", FREEZE, ELEVENTY, "--output=_site-11ty", "--quiet"],
    times: [],
  },
  {
    name: "Brrrty",
    label: `Brrrty ${brrrtyVersion}`,
    out: "_site-brrrty",
    cmd: BRRRTY,
    args: ["build", "--strict", "--quiet", "--output=_site-brrrty"],
    times: [],
  },
];

function build(tool, label) {
  rmSync(join(site, tool.out), { recursive: true, force: true });
  const start = performance.now();
  const result = spawnSync(tool.cmd, tool.args, { cwd: site, env, stdio: "inherit" });
  const ms = performance.now() - start;
  if (result.error) throw result.error;
  if (result.status !== 0) fail(`${tool.name} build failed (${label}, exit ${result.status ?? result.signal})`);
  return ms;
}

console.log(`SOURCE_DATE_EPOCH=${EPOCH} (${new Date(Number(EPOCH) * 1000).toISOString()}), TZ=UTC`);
for (let run = 1; run <= WARMUP; run++) {
  for (const tool of tools) {
    const ms = build(tool, `warm-up ${run}`);
    console.log(`warm-up ${run}/${WARMUP}  ${tool.name.padEnd(6)}  ${(ms / 1000).toFixed(2)}s`);
  }
}
for (let run = 1; run <= RUNS; run++) {
  for (const tool of tools) {
    const ms = build(tool, `run ${run}`);
    tool.times.push(ms);
    console.log(`run ${run}/${RUNS}  ${tool.name.padEnd(6)}  ${(ms / 1000).toFixed(2)}s`);
  }
}

function walk(dir, rel = "", files = new Set()) {
  for (const entry of readdirSync(join(dir, rel), { withFileTypes: true })) {
    const path = rel ? `${rel}/${entry.name}` : entry.name;
    if (entry.isDirectory()) walk(dir, path, files);
    else files.add(path);
  }
  return files;
}

const [eleventy, brrrty] = tools;
const leftDir = join(site, eleventy.out);
const rightDir = join(site, brrrty.out);
const left = walk(leftDir);
const right = walk(rightDir);
const identical = [];
const different = [];
const missing = []; // written by 11ty, not by Brrrty
const extra = []; // written by Brrrty, not by 11ty
for (const file of [...left].sort()) {
  if (!right.has(file)) missing.push(file);
  else if (readFileSync(join(leftDir, file)).equals(readFileSync(join(rightDir, file)))) identical.push(file);
  else different.push(file);
}
for (const file of [...right].sort()) if (!left.has(file)) extra.push(file);

const mean = (xs) => xs.reduce((a, b) => a + b, 0) / xs.length;
const min = (xs) => Math.min(...xs);
const secs = (ms) => `${(ms / 1000).toFixed(2)} s`;
const times = (xs) => xs.map((ms) => (ms / 1000).toFixed(2)).join(", ");
const ok = different.length + missing.length + extra.length === 0;

const md = [
  `### 11ty and Brrrty on a ${left.size}-file blog`,
  "",
  `| | ${eleventy.label} | ${brrrty.label} |`,
  "|---|---:|---:|",
  `| mean | ${secs(mean(eleventy.times))} | ${secs(mean(brrrty.times))} |`,
  `| min | ${secs(min(eleventy.times))} | ${secs(min(brrrty.times))} |`,
  `| speedup (mean) | | ${(mean(eleventy.times) / mean(brrrty.times)).toFixed(1)}x |`,
  `| speedup (min) | | ${(min(eleventy.times) / min(brrrty.times)).toFixed(1)}x |`,
  `| runs (s) | ${times(eleventy.times)} | ${times(brrrty.times)} |`,
  "",
  "| identical | different | missing | extra |",
  "|---:|---:|---:|---:|",
  `| ${identical.length} | ${different.length} | ${missing.length} | ${extra.length} |`,
  "",
  `${RUNS} timed runs each after ${WARMUP} warm-up, alternating, clean output folder every run. ` +
    `Node ${process.version}, ${platform()} ${release()} ${arch()}, ${cpus().length} CPUs ` +
    `(${cpus()[0]?.model.trim() ?? "unknown"}). ` +
    (ok ? "Every file is identical." : "**The outputs differ.**"),
  "",
];

const problems = [
  ...different.map((f) => ["different", f]),
  ...missing.map((f) => ["missing", f]),
  ...extra.map((f) => ["extra", f]),
];
if (!ok) {
  md.push(`<details><summary>First ${Math.min(20, problems.length)} of ${problems.length} problem paths</summary>`, "");
  for (const [kind, file] of problems.slice(0, 20)) {
    md.push(`${kind}: \`${file}\``, "");
    if (kind !== "different") continue;
    const diff = spawnSync("diff", ["-u", join(leftDir, file), join(rightDir, file)], { encoding: "utf8" });
    const lines = (diff.stdout || diff.stderr || "").split("\n").slice(0, 20);
    md.push("```diff", ...lines, "```", "");
  }
  md.push("</details>", "");
}

const text = md.join("\n");
console.log("\n" + text);
if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, text + "\n");
process.exit(ok ? 0 : 1);
