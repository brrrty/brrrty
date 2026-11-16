// Logs when 11ty 3.1.8 adds each template to its map, measured in
// microtask rounds, by patching its classes in-process.
//
// usage: ELEVENTY_DIR=/path/to/node_modules/@11ty/eleventy node events.mjs SITE_DIR [-v]
//
// Prints one line per template in 11ty's map order: "B A start inputPath".
//   A      the microtask round (counted from the first template's start) at
//          which the template asked for global data: the barrier
//   B      the round, counted from when global data settled, at which its
//          map entry was added
//   start  the template's place in the glob order
// With -v it also prints every engine and layout promise call and settle,
// which is where the step counts in src/order.rs come from.
// Run it on a site whose pages all have a `date` (an undated page reads the
// disk, so its B is the cap, 100000).
import path from "node:path";
import { pathToFileURL } from "node:url";

if (!process.env.ELEVENTY_DIR) {
  console.error("Set ELEVENTY_DIR to the @11ty/eleventy package folder.");
  process.exit(2);
}
const E = pathToFileURL(path.join(process.env.ELEVENTY_DIR, "src") + "/").href;
const { default: TemplateMap } = await import(E + "TemplateMap.js");
const { default: TemplateData } = await import(E + "Data/TemplateData.js");
const { default: Eleventy } = await import(E + "Eleventy.js");
const { default: TemplateLayout } = await import(E + "TemplateLayout.js");
const events = [];
const verbose = process.argv[3] === "-v";
function note(what) { if (verbose && runB) events.push(`${genB} ${what}`); }
const { default: EngineManager } = await import(E + "Engines/TemplateEngineManager.js");
const og = EngineManager.prototype.getEngine;
const seenEng = new WeakSet();
EngineManager.prototype.getEngine = function (name, m) {
  note("call engine(" + name + ")");
  const r = og.call(this, name, m);
  if (!seenEng.has(r)) { seenEng.add(r); r.then(() => note("settle engine(" + name + ")")); }
  return r;
};
const seenInner = new WeakSet();
function inner(obj, field, label) {
  const p = obj[field];
  if (p && typeof p.then === "function" && !seenInner.has(p)) { seenInner.add(p); p.then(() => note("settle " + label)); }
}
for (const name of ["getData", "getTemplateLayoutMap", "getFrontMatterData"]) {
  const o = TemplateLayout.prototype[name];
  TemplateLayout.prototype[name] = function (...a) {
    const key = `${name}(${this.inputPath})`;
    note("call " + key);
    const r = o.apply(this, a);
    inner(this, {getData: "dataCache", getTemplateLayoutMap: "cachedLayoutMap", getFrontMatterData: "_frontMatterDataCache"}[name], "inner " + key);
    r.then(() => note("done " + key));
    return r;
  };
}
let genA = 0, genB = -1, runA = false, runB = false, starts = 0, asked = 0;
const arrive = new Map();
const rows = [];
function tickA() { genA++; if (runA && genA < 100000) queueMicrotask(tickA); }
function tickB() { genB++; if (runB && genB < 100000) queueMicrotask(tickB); }
const origGlobal = TemplateData.prototype.getGlobalData;
let started = new WeakSet();
TemplateData.prototype.getGlobalData = function (inputPath) {
  if (inputPath && !arrive.has(inputPath)) {
    arrive.set(inputPath, genA);
    if (++asked === starts) runA = false;
  }
  const p = origGlobal.call(this, inputPath);
  if (!started.has(p)) {
    started.add(p);
    p.then(() => { if (!runB) { runB = true; genB = 0; queueMicrotask(tickB); } });
  }
  return p;
};
const orig = TemplateMap.prototype.add;
TemplateMap.prototype.add = async function (template) {
  if (starts === 0) { runA = true; genA = 0; queueMicrotask(tickA); }
  const start = starts++;
  await orig.call(this, template);
  rows.push([genB, arrive.get(template.inputPath), start, template.inputPath]);
};
process.chdir(process.argv[2]);
const elev = new Eleventy(".", path.join(process.env.TMPDIR || "/tmp", "order-calibration-out-" + process.pid), { quietMode: true });
await elev.toJSON();
runB = false;
for (const r of rows) console.log(r.join(" "));
if (verbose) for (const e of events) console.log("  " + e);
