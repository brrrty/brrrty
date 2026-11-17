// Random linkify-it differential corpus: N texts built from link fragments, with linkify-it
// 6.1.0's `test`, `match` and `matchAtStart`, `fuzzyLink` off and on. Deterministic for a seed.
//
//   LINKIFY_IT=/path/to/node_modules/linkify-it node tools/gen-linkify-fuzz.cjs [seed] [count] > linkify.json
//
// Replay it with `LINKIFY_FUZZ=linkify.json cargo test -p brrrty-markdown --lib linkify_corpus -- --ignored --nocapture`.
const { LinkifyIt } = require(process.env.LINKIFY_IT || (() => { throw new Error('set LINKIFY_IT to the linkify-it package directory'); })());
const strict = new LinkifyIt();
const fuzzy = new LinkifyIt().set({ fuzzyLink: true });
let seed = Number(process.argv[2] || 1) >>> 0;
const N = Number(process.argv[3] || 10000);
const rnd = () => { seed = (seed + 0x6D2B79F5) >>> 0; let t = seed; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
const pick = (a) => a[Math.floor(rnd() * a.length)];
const FRAGS = [
  'http://', 'https://', 'HTTP://', 'ftp://', 'mailto:', '//', 'xhttp://', 'http:', 'skype:', ':', '/', '//',
  'example.com', 'github.com', 'a.b', 'x.y.z', 'Example.com', 'www.', 'sub.example.org', 'ex.co.uk', 'b.c', 'a.io', 'xn--p1ai', 'рф', 'пример.рф', 'é.com', '中文.com',
  'localhost', '127.0.0.1', '[::1]', '[2001:db8::1]', ':8080', ':99999', ':80',
  '/path', '/a/b', '?q=1', '&b=2', '#frag', '(x)', '[y]', '{z}', '(', ')', '[', ']', '{', '}', '"q"', "'s'", "'", '"',
  '.', '..', '...', ',', ';', '!', '?', '-', '--', '---', '_', '*', '**', '~', '%20', '@', '|', '>', '<', '｜',
  'foo@bar.com', 'a.b@c.d', 'me@', '@x.com', 'user.name+tag@example.co', 'john@[IPv6:::1]',
  ' ', ' ', '  ', '\n', '\t', ' ', '　', 'word', 'a', 'B', '1', '42', 'é', '中', '😀', '—', '’', '«',
];
const cases = [];
for (let i = 0; i < N; i++) {
  const n = 1 + Math.floor(rnd() * 8);
  let s = '';
  for (let j = 0; j < n; j++) s += pick(FRAGS);
  const m = (l) => (l.match(s) || []).map((x) => [x.schema, x.index, x.lastIndex, x.text, x.url]);
  const a = (l) => { const x = l.matchAtStart(s); return x && [x.schema, x.index, x.lastIndex, x.text, x.url]; };
  cases.push({ s, test: [strict.test(s), fuzzy.test(s)], match: [m(strict), m(fuzzy)], at_start: a(fuzzy) });
}
process.stdout.write('[' + cases.map((c) => JSON.stringify(c)).join(',\n') + ']\n');
