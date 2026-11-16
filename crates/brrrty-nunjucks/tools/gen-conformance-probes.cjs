// Regenerates tests/probes/conformance_parse_errors.json: every
// conformance case template, and every include a case defines, that
// Nunjucks 3.2.4's parser rejects, with the message, line and column it
// reports. Includes are named `case::file`.
//
//   NUNJUCKS=/path/to/node_modules/nunjucks SMOL_TOML=/path/to/node_modules/smol-toml \
//     node tools/gen-conformance-probes.cjs > tests/probes/conformance_parse_errors.json
//
// Run it from crates/brrrty-nunjucks. SMOL_TOML is any install of the
// smol-toml package, used only to read the case files.

const fs = require('fs');
const path = require('path');
const toml = require(process.env.SMOL_TOML || (() => { throw new Error('set SMOL_TOML to the smol-toml package directory'); })());
const { probe } = require('./gen-parse-probes.cjs');

const dir = path.join(__dirname, '../../../tests/conformance/nunjucks');
const out = [];
for (const file of fs.readdirSync(dir).filter((f) => f.endsWith('.toml')).sort()) {
  const cases = toml.parse(fs.readFileSync(path.join(dir, file), 'utf8')).case || [];
  for (const c of cases) {
    const sources = [[c.name, c.template], ...Object.entries(c.includes || {}).map(([f, t]) => [`${c.name}::${f}`, t])];
    for (const [name, src] of sources) {
      const r = probe(src);
      if (!r.ok) {
        const e = { name, msg: r.msg };
        if (r.line !== undefined) e.line = r.line;
        if (r.col !== undefined) e.col = r.col;
        out.push(e);
      }
    }
  }
}
process.stdout.write(JSON.stringify(out, null, 1) + '\n');
