// Regenerates tests/probes/collation.json: what V8 gives for
// String.prototype.localeCompare with no locale argument, in Node's en-US
// locale (ICU's root collation). Run from crates/brrrty-js with Node 20 or
// later (the file was made with v26.2.0, ICU 78.3, CLDR 48):
//
//   LC_ALL=en_US.UTF-8 node tools/gen-collation-probes.mjs > tests/probes/collation.json
//   LC_ALL=en_US.UTF-8 node tools/gen-collation-probes.mjs dump 17   # one batch
//
// The file has the alphabet the fuzz draws from, hand-picked pairs with
// their results, and seeded fuzz batches stored as hashes:
// tests/collation_probes.rs builds the same pairs from the same seeds. A
// batch's hash is FNV-1a (32-bit) over one byte per pair: `<`, `=` or `>`.

const BATCH = 1000;
const BATCHES = 300; // 300,000 pairs

if (new Intl.Collator().resolvedOptions().locale !== 'en-US') {
  throw new Error('run with LC_ALL=en_US.UTF-8: the default locale decides localeCompare');
}

function xorshift(seed) {
  let x = seed >>> 0 || 1;
  return () => {
    x ^= x << 13;
    x >>>= 0;
    x ^= x >>> 17;
    x ^= x << 5;
    x >>>= 0;
    return x;
  };
}

// ASCII printable first (95), then everything else.
const range = (a, b) => Array.from({ length: b - a + 1 }, (_, i) => a + i);
const ascii = range(0x20, 0x7e);
const alphabet = [
  ...ascii,
  0x09, 0x0a, 0x0d, 0x00, 0x7f, // controls
  ...range(0xa0, 0xff), // Latin-1
  ...range(0x100, 0x17f), // Latin Extended-A
  0x180, 0x18f, 0x1c4, 0x1c5, 0x1c6, 0x1f1, 0x237, 0x259, 0x292, // a few more Latin
  ...range(0x300, 0x36f), // combining marks
  ...range(0x391, 0x3a9), ...range(0x3b1, 0x3c9), 0x3c2, // Greek
  ...range(0x410, 0x44f), 0x401, 0x451, // Cyrillic
  0x5d0, 0x5d1, 0x5e9, 0x627, 0x628, 0x644, 0x660, 0x661, 0x669, // Hebrew, Arabic, Arabic-Indic digits
  0x905, 0x915, 0x93f, 0x966, 0x967, // Devanagari
  0xe01, 0xe40, 0xe48, // Thai
  0x1100, 0x1161, 0x11a8, 0xac00, 0xac01, 0xd7a3, // Hangul jamo and syllables
  0x3041, 0x3042, 0x30a2, 0x30fc, 0x4e00, 0x4e8c, 0x5b57, 0x9fa5, // kana, Han
  ...range(0x2010, 0x2027), 0x2030, 0x2032, 0x2039, 0x203a, 0x20ac, 0x2122, // punctuation
  0x2153, 0x2160, 0x2460, 0x24b6, 0x2603, 0x266a, // number forms, symbols
  0x200b, 0x200c, 0x200d, 0x2060, 0xfeff, 0xad, // ignorables
  0xfb01, 0xfb02, 0xff21, 0xff41, 0xff10, // ligatures, fullwidth
  0x1f600, 0x1f44d, 0x1f3fd, 0x1d400, 0x10400, 0x20000, // outside the BMP
  0xe000, 0xfffd, // private use, replacement
];

const pick = (r) => {
  const v = r();
  return v % 4 !== 0 ? ascii[(v >>> 2) % ascii.length] : alphabet[(v >>> 2) % alphabet.length];
};

function pair(r) {
  const len = r() % 9;
  const a = Array.from({ length: len }, () => pick(r));
  let b;
  switch (r() % 4) {
    case 0: {
      const n = r() % 9;
      b = Array.from({ length: n }, () => pick(r));
      break;
    }
    case 1:
      b = [...a];
      if (len > 0) {
        const i = r() % len;
        b[i] = pick(r);
      }
      break;
    case 2: {
      b = [...a];
      const i = r() % (len + 1);
      b.splice(i, 0, pick(r));
      break;
    }
    default:
      b = [...a];
      if (len > 0) b.splice(r() % len, 1);
  }
  return [String.fromCodePoint(...a), String.fromCodePoint(...b)];
}

const seed = (b) => (0xc011a7e0 + b * 7919) >>> 0;
function batch(b) {
  const r = xorshift(seed(b));
  return Array.from({ length: BATCH }, () => pair(r));
}

const sign = (n) => (n < 0 ? '<' : n > 0 ? '>' : '=');
function fnv(chars) {
  let h = 0x811c9dc5;
  for (const c of chars) {
    h ^= c.charCodeAt(0);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h;
}

const words = [
  '', ' ', 'a', 'A', 'b', 'B', 'á', 'Á', 'à', 'ä', 'å', 'æ', 'Æ', 'ab', 'aB', 'Ab', 'AB', 'résumé',
  'resume', 'Resume', 'résume', 'resumé', 'co-op', 'coop', 'co op', 'Co-op', 'co–op', "o'neil",
  'o’neil', 'oneil', 'O’Neil', '10', '9', '2', '02', '1.5', '1,5', '-1', '+1', '#1', '$5', '5$',
  '2025-01-02', '2025-01-02T03:04:05Z', '2025-1-2', '2024-12-31', 'ß', 'ss', 'SS', 'ẞ', 'ﬁ', 'fi',
  'ö', 'oe', 'ö', 'é', 'é', 'Å', 'Å', 'Å', 'ı', 'i', 'İ', 'I', 'ŉ', "ʼn",
  'ae', 'Ae', 'ǅ', 'Dž', 'dž', 'DŽ', 'x­y', 'xy', 'x​y', 'Ω', 'ω', 'Я', 'я', 'ё', 'е',
  'あ', 'ア', 'ｱ', '一', '二', '가', '각', 'ᄀ', '😀', '👍', '👍🏽', 'Ａ', 'ａ', '０', '0', '٠', '१',
  'Ⅰ', 'I', '①', '1', 'Ⓐ', '™', 'TM', '€', '¢', '£', '¥', '&', 'and', '_', '-', '.', '...', '…',
  'The Shop', 'the shop', 'theshop', 'The  Shop', 'café', 'Cafe', 'CAFÉ', 'naïve', 'naive', 'Zoë', 'Zoe',
];

const hand = [];
for (const a of words) for (const b of words) hand.push([a, b, sign(a.localeCompare(b))]);

const mode = process.argv[2];
if (mode === 'dump') {
  for (const [a, b] of batch(Number(process.argv[3]))) {
    console.log(JSON.stringify(a), JSON.stringify(b), sign(a.localeCompare(b)));
  }
} else {
  const lines = ['{'];
  lines.push(`  "node": ${JSON.stringify(process.version)},`);
  lines.push(`  "icu": ${JSON.stringify(process.versions.icu)},`);
  lines.push(`  "batch": ${BATCH},`);
  lines.push(`  "alphabet": ${JSON.stringify(alphabet)},`);
  lines.push('  "pairs": [');
  hand.forEach((c, i) => lines.push(`    ${JSON.stringify(c)}${i < hand.length - 1 ? ',' : ''}`));
  lines.push('  ],');
  const hashes = Array.from({ length: BATCHES }, (_, b) =>
    fnv(batch(b).map(([a, b]) => sign(a.localeCompare(b)))),
  );
  lines.push(`  "batches": ${JSON.stringify(hashes)}`);
  lines.push('}');
  console.log(lines.join('\n'));
}
