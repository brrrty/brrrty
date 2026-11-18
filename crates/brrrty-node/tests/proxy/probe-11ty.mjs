// Runs Eleventy 3.1.8's own ComputedDataProxy over `cases.json` and prints
// what each function reads, for `expected.json`:
//
//   docker run --rm -v "$PWD:/p" -w /p node:22 sh -c \
//     'mkdir -p /e && cd /e && npm init -y >/dev/null &&
//      npm install --ignore-scripts --silent @11ty/eleventy@3.1.8 >/dev/null &&
//      cd /p && node probe-11ty.mjs' > expected.json
//
// (Brrrty's port must give the same lists, in the same order.)

import fs from "node:fs";

// The package doesn't export its source files, so import the file itself.
const { default: ComputedDataProxy } = await import(
  "/e/node_modules/@11ty/eleventy/src/Data/ComputedDataProxy.js"
);

const cases = JSON.parse(fs.readFileSync("cases.json", "utf8"));
const out = [];
for (const c of cases) {
  // The cases are generated text, run here as 11ty runs a data file's code.
  const fn = new Function(`return ${c.fn};`)();
  const proxy = new ComputedDataProxy(c.keys);
  try {
    out.push({ vars: await proxy.findVarsUsed(fn, structuredClone(c.data)) });
  } catch (e) {
    out.push({ throws: String(e && e.name) });
  }
}
process.stdout.write(JSON.stringify(out, null, 1) + "\n");
