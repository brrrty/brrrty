// Random markdown-it differential corpus: N inputs built from markdown fragments, run through
// `markdownIt().render` and `.renderInline`. Deterministic for a seed.
//
//   [MARKDOWN_MODE=eleventy] MARKDOWN_IT=/path/to/node_modules/markdown-it node tools/gen-markdown-fuzz.cjs [seed] [count] > fuzz.json
//
// MARKDOWN_OPTIONS='{"linkify":true,"typographer":true,"fuzzyLink":true,"html":false}' renders with
// `markdownIt({ html, linkify, typographer })` and `md.linkify.set({ fuzzyLink })` instead (no
// `renderInline`), from fragments that add links, emails, quotes, dashes and the like.
//
// Replay it with `MARKDOWN_FUZZ=fuzz.json cargo test -p brrrty-markdown --test fuzz -- --ignored`
// (with the same MARKDOWN_MODE or MARKDOWN_OPTIONS).
// The differences it found are pinned in tests/probes/markdown_it.json.
const markdownIt = require(process.env.MARKDOWN_IT || (() => { throw new Error('set MARKDOWN_IT to the markdown-it package directory'); })());
const options = process.env.MARKDOWN_OPTIONS && JSON.parse(process.env.MARKDOWN_OPTIONS);
// MARKDOWN_MODE=eleventy: markdownIt({ html: true }).disable('code'), as 11ty 3.1.8 builds it (with its bundled 14.3.3).
let md;
if (options) {
  md = markdownIt({ html: !!options.html, linkify: !!options.linkify, typographer: !!options.typographer });
  md.linkify.set({ fuzzyLink: !!options.fuzzyLink });
} else {
  md = process.env.MARKDOWN_MODE === 'eleventy' ? markdownIt({ html: true }).disable('code') : markdownIt();
}
let seed = Number(process.argv[2] || 1) >>> 0;
const N = Number(process.argv[3] || 6000);
const rnd = () => { seed = (seed + 0x6D2B79F5) >>> 0; let t = seed; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
const pick = (a) => a[Math.floor(rnd() * a.length)];
const FRAGS = ['a', 'b c', 'word', '*', '**', '_', '__', '`', '``', ' ', '  ', '\t', '\n', '\n\n', '> ', '>', '- ', '-', '* ', '+ ', '1. ', '2) ', '# ', '## ', '#', '---', '***', '___', '===', '|', '| a | b |', '|--', '|---|---|', ':-:', '```', '~~~', '```js', '    ', '<b>', '</b>', '<div>', '<!--', '-->', '[', ']', '(', ')', '[a](b)', '![i](u)', '<http://x.y>', '&amp;', '&', '<', '>', '\\', '\\*', '~~', '~', '"', "'", ' ', 'é', '中', '😀', '[x]: /u', '[x]', '\r\n', '\r', '1.', '10.', '0.', '    code', 'http://a.b', ':', '!', '#hash'];
// Around linkify and the typographer.
const LINKS = ['http://', 'https://', 'HTTP://', 'ftp://', 'mailto:', '//', 'www.', 'example.com', 'github.com/x', 'Example.com', 'a.b', 'x.io', 'пример.рф', 'é.com', 'xn--p1ai', '127.0.0.1', ':8080', '/path', '?q=1&b=2', '#frag', '(x)', '[y]', 'foo@bar.com', 'me@x.co', '@', '.', ',', ';', '!', '?', '*', '_', '%20', '<a href="x">', '</a>', '<http://x.y/z>', '[l](http://u.v)', '[http://in.label](u)', '![http://alt.text](u)', '`http://code.x`', '\\_', '&#58;', '&amp;', '"', '"', "'", "'", '“', "'s", '"q"', "'q'", '(c)', '(C)', '(tm)', '(r)', '+-', '..', '...', '....', '?..', '!!!!', '????', ',,', '--', '---', '----', ' -- ', ' ', '😀', '中'];
const ALL = options ? FRAGS.concat(LINKS, LINKS) : FRAGS;
const cases = [];
for (let i = 0; i < N; i++) {
  const n = 1 + Math.floor(rnd() * (options ? 12 : 9));
  let s = '';
  for (let j = 0; j < n; j++) s += pick(ALL);
  cases.push(options ? { s, render: md.render(s) } : { s, render: md.render(s), render_inline: md.renderInline(s) });
}
process.stdout.write('[' + cases.map((c) => JSON.stringify(c)).join(',\n') + ']\n');
