// Regenerates tests/probes/regex.json: what JavaScript's String.prototype
// replace, split and RegExp.prototype.test give for the patterns below on
// text with emoji, CJK, combining marks and other characters outside ASCII.
// Run from crates/brrrty-config with Node 20 or later (the file was made
// with v26.2.0):
//
//   node tools/gen-regex-probes.cjs > tests/probes/regex.json
//
// Output is JSON: { "node": version, "patterns": [{ "p", "f", "astral" }],
// "cases": [{ "i": pattern index, "op", "s", "r"?, "out", "why"? }] }. "astral" is
// whether Brrrty accepts the pattern on text outside the BMP; when it is
// false, Brrrty must refuse such text. A case with "out" null is one
// Brrrty refuses, with "why" in the message. The text is made by a seeded generator,
// so the output is the same on every run.

// [pattern, flags, accepted on text outside the BMP]
const PATTERNS = [
  // Accepted: every character a match can end on is outside any surrogate
  // pair, and every class that matches surrogates is a greedy, unbounded
  // loop that only an ASCII-anchored element follows.
  ['(<([^>]+)>)', 'gi', true],
  ['(<([^>]+)>)', 'g', true],
  ['(<([^>]+)>)', '', true],
  ['\\s+', 'g', true],
  ['\\s+', '', true],
  ['[.,!?;:]$', '', true],
  ['<[^>]*>', 'g', true],
  ['[^<]+', 'g', true],
  ['\\S+', 'g', true],
  ['"[^"]*"', 'g', true],
  ['\\[[^\\]]+\\]\\([^)]+\\)', 'g', true],
  ['[^\\s]+', 'g', true],
  ['\\W+', 'g', true],
  ['[^a-z]+', 'g', true],
  ['a[^b]*b', 'g', true],
  ['(\\w+)\\s*=\\s*"([^"]*)"', 'g', true],
  ['^\\s+|\\s+$', 'g', true],
  ['\\s*,\\s*', 'g', true],
  ['-+', 'g', true],
  ['^#+\\s*', 'gm', true],
  ['([^,]+),?', 'g', true],
  ['.+', 'g', true],
  ['.*x', 'g', true],
  ['<.+>', 'g', true],
  ['[^>]+$', 'g', true],
  ['\\b\\w+\\b', 'g', true],
  ['[^ ]+ [^ ]+', 'g', true],
  ['x|[^x]+', 'g', true],
  ['(?:<[^>]*>)+', 'g', true],
  ['\\D+', 'g', true],
  // Refused: a match could start or end between the two halves of a
  // surrogate pair, or count them as two.
  ['.', 'g', false],
  ['^.$', '', false],
  ['a.b', 'g', false],
  ['[^a]', 'g', false],
  ['[^a]b', 'g', false],
  ['[^>]+?', 'g', false],
  ['.{2}', 'g', false],
  ['[^a]{1,3}', 'g', false],
  ['[^a]{2,}', 'g', false],
  ['([^a])+', 'g', false],
  ['[^a]+[^b]+', 'g', false],
  ['[^a]+\\S', 'g', false],
  ['[^a]*\\s*[^b]+', 'g', false],
  ['(?:[^a]+)+', 'g', false],
  ['x*', 'g', false],
  ['\\B', 'g', false],
  ['\\b|[^a]+', 'g', false],
  ['[\\uD7FF-\\uE000]', 'g', false],
  ['[^\\uD000-\\uE000]+', 'g', false],
  ['\\S?', 'g', false],
];

// Characters to build text from: ASCII, markup, whitespace, and text
// outside ASCII (emoji with modifiers and joiners, flags, CJK inside and
// outside the BMP, combining marks).
const PIECES = [
  'a', 'b', 'x', 'Z', '1', ' ', '  ', '\n', '\t', ',', '.', '!', '?', ';', ':', '-', '#', '"', '=',
  '<', '>', '<b>', '</b>', '<a href="x">', '</a>', '[', ']', '(', ')', '[t](u)',
  ' ', '　', ' ', 'é', 'é', 'ä', '‍',
  '\u{1F600}', '\u{1F44D}\u{1F3FD}', '\u{1F468}‍\u{1F469}‍\u{1F467}', '\u{1F1FA}\u{1F1F8}',
  '漢字', '中', '\u{2070E}', '\u{20BB7}', '\u{1D400}', '\u{10348}', '�', '', '퟿',
];

// A small seeded generator (mulberry32), so the file is reproducible.
let seed = 0x6272_7274;
const rand = () => {
  seed = (seed + 0x6d2b79f5) | 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const pick = (xs) => xs[Math.floor(rand() * xs.length)];

const FIXED = ['', 'a', '\u{1F600}', 'a\u{1F600}b', '<b>\u{1F600}</b>', '\u{1F600}<i>x</i>\u{1F600}', 'x\u{1F600}x', '\u{1F600}\u{1F600}', ' \u{1F600} ', '\u{1F600},\u{1F600}'];
const texts = [...FIXED];
for (let n = 0; n < 60; n++) {
  let s = '';
  const len = Math.floor(rand() * 14);
  for (let k = 0; k < len; k++) s += pick(PIECES);
  texts.push(s);
}

// Random patterns, to test the rule rather than the shapes above. Brrrty
// may accept or refuse each ("astral" null); what it accepts must give
// JavaScript's answer.
const ATOMS = ['a', 'b', 'x', '<', '>', ' ', '\\s', '\\S', '\\w', '\\W', '\\d', '.', '[^>]', '[^a]', '[a-z]', '[<>]', '[^\\s]'];
const QUANTS = ['', '', '', '*', '+', '?', '{1,2}', '{2}', '*?', '+?', '{1,}'];
const randomPattern = (depth) => {
  const parts = [];
  const n = 1 + Math.floor(rand() * 4);
  for (let k = 0; k < n; k++) {
    const r = rand();
    // Groups take at most `?`, so JavaScript's backtracking stays fast.
    if (depth < 2 && r < 0.15) parts.push(`(${randomPattern(depth + 1)})` + pick(['', '?']));
    else if (depth < 2 && r < 0.25) parts.push(`(?:${randomPattern(depth + 1)}|${randomPattern(depth + 1)})` + pick(['', '?']));
    else if (r < 0.3) parts.push(pick(['^', '$', '\\b', '\\B']));
    else parts.push(pick(ATOMS) + pick(QUANTS));
  }
  return parts.join('');
};
const SHAPES = PATTERNS.length;
const seen = new Set(PATTERNS.map(([p, f]) => p + '/' + f));
while (PATTERNS.length < SHAPES + 300) {
  const p = randomPattern(0);
  const f = rand() < 0.7 ? 'g' : '';
  if (seen.has(p + '/' + f)) continue;
  seen.add(p + '/' + f);
  PATTERNS.push([p, f, null]);
}

// JavaScript's answer has half a surrogate pair in it.
const broken = (x) => (Array.isArray(x) ? x.some(broken) : typeof x === 'string' && !x.isWellFormed());

const cases = [];
const ASTRAL = /[\u{10000}-\u{10FFFF}]/u;
PATTERNS.forEach(([p, f, astral], i) => {
  for (const s of i < SHAPES ? texts : texts.slice(0, 30)) {
    // Brrrty refuses this text for this pattern, and JavaScript's answer
    // may have half a surrogate pair in it, so there's nothing to compare.
    if (astral === false && ASTRAL.test(s)) {
      cases.push({ i, op: 'test', s, out: null, why: 'outside the BMP' });
      continue;
    }
    // Brrrty also refuses `^`/`$` with `m` on text with U+2028 (and \r,
    // U+2029), where JavaScript breaks lines too.
    if (f.includes('m') && /[\^$]/.test(p) && /[\r\u2028\u2029]/.test(s)) {
      cases.push({ i, op: 'test', s, out: null, why: '`m` flag' });
      continue;
    }
    const re = () => new RegExp(p, f);
    const outs = [
      { op: 'replace', r: '[$&]', out: s.replace(re(), '[$&]') },
      { op: 'split', out: s.split(re()).map((x) => (x === undefined ? null : x)) },
      { op: 'test', out: re().test(s) },
    ];
    for (const c of outs) {
      if (broken(c.out)) cases.push({ i, op: c.op, s, r: c.r, out: null, why: 'outside the BMP' });
      else cases.push({ i, s, ...c });
    }
  }
});

process.stdout.write(JSON.stringify({
  node: process.version,
  patterns: PATTERNS.map(([p, f, astral]) => ({ p, f, astral })),
  cases,
}, null, 0) + '\n');
