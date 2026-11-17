// Regenerates tests/probes/csv.json: what csv-parse 7.0.3's `parse` from
// `csv-parse/sync` gives for the inputs and options below. Run from
// crates/brrrty-config with Node 20 or later (the file was made with
// v26.2.0), pointing at an installed csv-parse 7.0.3:
//
//   node tools/gen-csv-probes.mjs /path/to/node_modules/csv-parse > tests/probes/csv.json
//
// Output is JSON: { "node", "csv_parse", "options": [ {...} ], "cases": [{ "o":
// options index, "s": input, "out": records | null, "err": message | null }] }.
// A record made with `columns` is a list of [key, value] pairs in the
// object's key order. The fuzzed inputs come from a seeded generator, so the
// output is the same on every run.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const dir = process.argv[2];
if (!dir) {
  console.error('usage: node tools/gen-csv-probes.mjs /path/to/node_modules/csv-parse');
  process.exit(2);
}
const version = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8')).version;
if (version !== '7.0.3') {
  console.error(`expected csv-parse 7.0.3, found ${version}`);
  process.exit(2);
}
const { parse } = await import(pathToFileURL(join(dir, 'lib/sync.js')).href);

const OPTIONS = [
  {},
  { columns: true },
  { columns: true, skip_empty_lines: true, trim: true },
  { skip_empty_lines: true },
  { trim: true },
  { ltrim: true },
  { rtrim: true },
  { trim: true, ltrim: false },
  { trim: true, rtrim: false },
  { columns: true, bom: true },
  { bom: true },
  { delimiter: ';' },
  { delimiter: '::', trim: true },
  { columns: true, relax_column_count: true },
  { columns: true, relax_column_count_less: true },
  { columns: true, relax_column_count_more: true },
  { skipEmptyLines: true, columns: true },
  { columns: false, skip_empty_lines: false, trim: false, bom: false },
];

const FIXED = [
  '',
  '\n',
  '\n\n',
  'a',
  'a,b\n1,2',
  'a,b\n1,2\n',
  'a,b\r\n1,2\r\n',
  'a,b\r1,2\r',
  'a,b\n1,2\r\n3,4',
  'a,b\r\n1,2\n3,4',
  'a,b\n\n1,2\n\n',
  'a,b\n  \n1,2',
  'a,b\n,\n1,2',
  'a,b\n"",""\n',
  '"a","b"\n"1","2"',
  '"a""b",c',
  '"a\nb",c\nd,e',
  '"a\r\nb",c\r\nd,e',
  '"a" ,b',
  '"a"  ,b\n1,2',
  ' "a" ,b',
  '"a"b,c',
  'a"b,c',
  '"a',
  'a,"b',
  '" ",x',
  '"a" ,b',
  '﻿a,b\n1,2',
  '﻿"a",b\n1,2',
  ' ﻿a,b\n1,2',
  'a,a\n1,2',
  'b,a,b\n1,2,3',
  '2,1,x,__proto__\n1,2,3,4',
  'a,,b\n1,2,3',
  'a,b\n1,2,3',
  'a,b,c\n1,2',
  'a,b\n1',
  ' a , b \n 1 , 2 ',
  '\ta\t,\tb\t\n　x　,y',
  'a,b\n1,2\n ',
  'x ,y',
  'a;b\n1;2',
  'a::b\n1:: 2 ::3',
  '"a\u0000",b',
  '"a"\u0000b',
  'a\u0000,b',
  '""',
  '"""",x',
  '"a""",x',
  'a,b\n"x\ny",z\n"p""q", r ',
  'héllo,wörld\n日本,語\n😀,ok',
  '"é"x,y',
  'a,b\n1,2\n\n\n',
  '\r\n\r\na,b\r\n1,2',
  '\n\na,b\n1,2',
  '#a,b\n1,2',
  'a,b\n\r\n1,2',
  'a,b\r\n\n1,2',
];

function xorshift(seed) {
  let x = seed >>> 0 || 1;
  return () => {
    x ^= x << 13; x >>>= 0;
    x ^= x >>> 17;
    x ^= x << 5; x >>>= 0;
    return x;
  };
}
const ALPHABET = ['a', 'b', 'Z', '1', ',', ',', ';', ':', '"', '"', '\n', '\n', '\r', '\r\n', ' ', ' ', '\t', ' ', '﻿', '　', 'é', '😀', '\u0000', '#'];
const rnd = xorshift(0xc5a1);
const FUZZ = [];
for (let i = 0; i < 1500; i++) {
  const len = rnd() % 24;
  let s = '';
  for (let j = 0; j < len; j++) s += ALPHABET[rnd() % ALPHABET.length];
  FUZZ.push(s);
}

const cases = [];
OPTIONS.forEach((opts, o) => {
  for (const s of [...FIXED, ...FUZZ]) {
    let out = null;
    let err = null;
    try {
      const records = parse(s, { ...opts });
      out = records.map((r) => (Array.isArray(r) ? r : Object.keys(r).map((k) => [k, r[k]])));
    } catch (e) {
      err = e.message;
    }
    cases.push({ o, s, out, err });
  }
});
process.stdout.write(JSON.stringify({ node: process.version, csv_parse: version, options: OPTIONS, cases }));
