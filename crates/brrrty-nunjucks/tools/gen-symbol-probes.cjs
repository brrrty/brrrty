// Regenerates tests/probes/symbols.json: the variables 11ty 3.1.8 finds
// in a Nunjucks template for computed data (`Nunjucks.parseForSymbols`,
// copied below from src/Engines/Nunjucks.js), for every template in
// parse-probe-sources.cjs and every conformance case, plus templates
// written for it below.
//
//   NUNJUCKS=/path/to/node_modules/nunjucks SMOL_TOML=/path/to/node_modules/smol-toml \
//     node tools/gen-symbol-probes.cjs > tests/probes/symbols.json
//
// Run it from crates/brrrty-nunjucks.

const fs = require('fs');
const path = require('path');
const NunjucksLib = require(process.env.NUNJUCKS || (() => { throw new Error('set NUNJUCKS to the nunjucks package directory'); })());
const toml = require(process.env.SMOL_TOML || (() => { throw new Error('set SMOL_TOML to the smol-toml package directory'); })());

// 11ty 3.1.8's `parseForSymbols`, with no custom tags.
function parseForSymbols(str) {
  if (!str) {
    return [];
  }
  const { parser, nodes } = NunjucksLib;
  let obj = parser.parse(str, []);
  if (!obj) {
    return [];
  }
  let linesplit = str.split("\n");
  let values = obj.findAll(nodes.Value);
  let symbols = obj.findAll(nodes.Symbol).map((entry) => {
    let name = [entry.value];
    let nestedIndex = -1;
    for (let val of values) {
      if (nestedIndex > -1) {
        /* deep.object.syntax */
        if (linesplit[val.lineno].charAt(nestedIndex) === ".") {
          name.push(val.value);
          nestedIndex += val.value.length + 1;
        } else {
          nestedIndex = -1;
        }
      } else if (
        val.lineno === entry.lineno &&
        val.colno === entry.colno &&
        val.value === entry.value
      ) {
        nestedIndex = entry.colno + entry.value.length;
      }
    }
    return name.join(".");
  });

  let uniqueSymbols = Array.from(new Set(symbols));
  return uniqueSymbols;
}

const written = [
  '{{ foo }}', '{{ foo.bar }}', '{{ foo.bar.baz }}', '{{ foo["bar"] }}', '{{ foo.bar | upper }}',
  '{{ a.b }}{{ a.b.c }}', '{{ x | f(y.z, k=w.v) }}', '{{ a if b.c else d.e }}',
  '{% for x in items.list %}{{ x.name }}{% endfor %}', '{% for k, v in obj.map %}{{ k }}{{ v.y }}{% endfor %}',
  '{% set y = page.url %}{{ y }}', '{% set y %}{{ title }}{% endset %}{{ y }}',
  '{% if a.b == c.d %}{{ e.f }}{% elif g.h %}i{% else %}{{ j.k }}{% endif %}',
  '{{ a.b < c.d < e.f }}', '{{ a ~ b.c }}', '{{ not a.b }}', '{{ -a.b }}', '{{ a.b is defined }}',
  '{{ a.b in c.d }}', '{{ a.b not in c.d }}', '{{ (a.b, c.d) }}', '{{ [a.b, c.d] }}', '{{ {k: a.b, "s": c.d} }}',
  '{% switch a.b %}{% case c.d %}{{ e.f }}{% default %}{{ g.h }}{% endswitch %}',
  '{% macro m(x, y=z.w) %}{{ x.q }}{% endmacro %}{{ m(a.b) }}',
  '{% call m(a.b) %}{{ c.d }}{% endcall %}', '{% call(x) m(a.b) %}{{ x.d }}{% endcall %}',
  '{% filter upper %}{{ a.b }}{% endfilter %}', '{% filter f(x.y) %}t{% endfilter %}',
  '{% include a.b %}', '{% import a.b as c %}{{ c.d }}', '{% from a.b import c, d as e %}',
  '{% block content %}{{ a.b }}{% endblock %}', '{% extends a.b %}',
  '{% raw %}{{ a.b }}{% endraw %}{{ c.d }}', '{{ a }}\n.b', '{{ a }}x\n{{ .5 }}', '{{a.b}}{{a.b.c}}',
  '{{ a.b.c.d.e }}', '{{ a .b }}', '{{ a. b }}', '{{ a.b(c).d }}', '{{ a[0].b }}', '{{ a.b[c.d] }}',
  'é {{ a.b }}', '😀{{ a.b }}', '{{ "é".x }}', '{{ a | b.c }}', '{{ a | b.c | d }}',
  '{{ super() }}', '{{ caller() }}', '{{ loop.index }}', '{{ x }}{# a.b #}{{ y.z }}',
  '{%- if a.b -%} {{ c.d }} {%- endif -%}', '  {{- a.b -}}  ', '{{ a.b }}\n{{ c.d }}\n{{ e.f.g }}',
  '{{ true }}{{ a }}', '{{ a }}{{ none }}', '{{ a.true }}', '{{ a }}.{{ b }}', '{{ a }}.{{ 1 }}',
  '{{a}}.{{true}}{{b.c}}', '{{ a.b }}.{{ c }}', '{{ page.url }}!', '{{ title }}-x', '{{ foo }}-y',
  '{{ collections.post | length }}', '{{ collections["post"] }}', '{% set a, b = c.d %}{{ a }}',
  '{% for x in y %}{% else %}{{ z.w }}{% endfor %}', '{% asyncEach x in y.z %}{{ x }}{% endeach %}',
  '{% ifAsync a.b %}c{% endif %}', '{{ r/a.b/g }}', '{{ x if y }}', '{{ {a.b: 1} }}',
  '{{ f(a=b) }}', '{{ f(**) }}', '{{', 'text only', '', '\n\n{{ a.b }}', '{{ a\n.b }}', '{{ a.\nb }}',
  '{{ a }}\r\n.b', '{{ a | default(b.c) }}', '{{ "x" ~ a.b ~ "y" }}', '{{ a.b.length }}',
];

const sources = [...written, ...require('./parse-probe-sources.cjs')];
const dir = path.join(__dirname, '../../../tests/conformance/nunjucks');
for (const file of fs.readdirSync(dir).filter((f) => f.endsWith('.toml')).sort()) {
  for (const c of toml.parse(fs.readFileSync(path.join(dir, file), 'utf8')).case || []) {
    sources.push(c.template);
    for (const t of Object.values(c.includes || {})) sources.push(t);
  }
}

const seen = new Set();
const out = [];
for (const src of sources) {
  if (seen.has(src)) continue;
  seen.add(src);
  try {
    out.push({ src, symbols: parseForSymbols(src) });
  } catch (e) {
    out.push({ src, error: String(e && e.message || e).split('\n')[0] });
  }
}
process.stdout.write(JSON.stringify(out, null, 1) + '\n');
