// Regenerates tests/probes/render.json: each template in
// render-probe-sources.cjs rendered by Nunjucks 3.2.4 as 11ty 3.1.8 runs
// it: `new Environment(new FileSystemLoader([includes, "."]), { dev: true })`
// and `new Template(source, env, "./cases/t.njk")`, with the case's
// includes in the includes folder.
//
//   NUNJUCKS=/path/to/node_modules/nunjucks TZ=UTC \
//     node tools/gen-render-probes.cjs > tests/probes/render.json
//
// Run it from crates/brrrty-nunjucks with Node 20 or later (the file was
// made with v26.2.0, whose V8 words the error messages). Data goes
// through JSON, as front matter does; a string starting `__DATE__` is
// made a Date.
//
// A template that renders gets `out`. One that fails gets `err`, the
// message of the innermost error, after Nunjucks's `(path) [Line, Column]`
// lines. An output holding a lone surrogate (a JavaScript string split
// inside a character) gets `lone_surrogate`, and its well-formed form.

const fs = require('fs');
const os = require('os');
const path = require('path');
const nunjucks = require(process.env.NUNJUCKS || (() => { throw new Error('set NUNJUCKS to the nunjucks package directory'); })());
const sources = require('./render-probe-sources.cjs');

const dates = (v) => {
  if (typeof v === 'string' && v.startsWith('__DATE__')) return new Date(v.slice(8));
  if (Array.isArray(v)) return v.map(dates);
  if (v && typeof v === 'object') {
    const o = {};
    for (const [k, x] of Object.entries(v)) o[k] = dates(x);
    return o;
  }
  return v;
};

const out = [];
for (const [template, data, includes] of sources) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'njk-probe-'));
  fs.mkdirSync(path.join(dir, '_includes'));
  for (const [name, text] of Object.entries(includes)) {
    const p = path.join(dir, '_includes', name);
    fs.mkdirSync(path.dirname(p), { recursive: true });
    fs.writeFileSync(p, text);
  }
  const cwd = process.cwd();
  process.chdir(dir);
  const env = new nunjucks.Environment(new nunjucks.FileSystemLoader(['_includes', '.']), { dev: true });
  const entry = { template, data: JSON.parse(JSON.stringify(data)), includes };
  try {
    const t = new nunjucks.Template(template, env, './cases/t.njk', false);
    const rendered = t.render(dates(entry.data));
    if (rendered.isWellFormed()) {
      entry.out = rendered;
    } else {
      // JSON can't carry a lone surrogate. Brrrty refuses these.
      entry.lone_surrogate = true;
      entry.out = rendered.toWellFormed();
    }
  } catch (e) {
    const parts = String(e.message).split('\n  ');
    entry.err = parts[parts.length - 1];
  }
  process.chdir(cwd);
  fs.rmSync(dir, { recursive: true });
  out.push(entry);
}
process.stdout.write(JSON.stringify(out, null, 1) + '\n');
