// Random markdown-it differential corpus: N inputs built from markdown fragments, run through
// `markdownIt().render` and `.renderInline`. Deterministic for a seed.
//
//   MARKDOWN_IT=/path/to/node_modules/markdown-it node tools/gen-markdown-fuzz.cjs [seed] [count] > fuzz.json
//
// Replay it with `MARKDOWN_FUZZ=fuzz.json cargo test -p brrrty-markdown --test fuzz -- --ignored`.
// The differences it found are pinned in tests/probes/markdown_it.json.
const markdownIt = require(process.env.MARKDOWN_IT || (() => { throw new Error('set MARKDOWN_IT to the markdown-it package directory'); })());
const md = markdownIt();
let seed = Number(process.argv[2] || 1) >>> 0;
const N = Number(process.argv[3] || 6000);
const rnd = () => { seed = (seed + 0x6D2B79F5) >>> 0; let t = seed; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
const pick = (a) => a[Math.floor(rnd() * a.length)];
const FRAGS = ['a', 'b c', 'word', '*', '**', '_', '__', '`', '``', ' ', '  ', '\t', '\n', '\n\n', '> ', '>', '- ', '-', '* ', '+ ', '1. ', '2) ', '# ', '## ', '#', '---', '***', '___', '===', '|', '| a | b |', '|--', '|---|---|', ':-:', '```', '~~~', '```js', '    ', '<b>', '</b>', '<div>', '<!--', '-->', '[', ']', '(', ')', '[a](b)', '![i](u)', '<http://x.y>', '&amp;', '&', '<', '>', '\\', '\\*', '~~', '~', '"', "'", ' ', 'é', '中', '😀', '[x]: /u', '[x]', '\r\n', '\r', '1.', '10.', '0.', '    code', 'http://a.b', ':', '!', '#hash'];
const cases = [];
for (let i = 0; i < N; i++) {
  const n = 1 + Math.floor(rnd() * 9);
  let s = '';
  for (let j = 0; j < n; j++) s += pick(FRAGS);
  cases.push({ s, render: md.render(s), render_inline: md.renderInline(s) });
}
process.stdout.write(JSON.stringify(cases).replace(/\},\{/g, '},\n{') + '\n');
