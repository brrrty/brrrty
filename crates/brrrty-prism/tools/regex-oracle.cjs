// What Node's RegExp `exec` gives, for tests/fuzz.rs. Reads one JSON request
// per line on stdin and writes one JSON line per request on stdout:
//
//   {"source": "...", "flags": "...", "cases": [[text, lastIndex], ...]}
//   -> {"results": [null | [index, end, g1start, g1end, ...], ...]}
//
// Each case runs `exec` with the `g` flag (as Prism's greedy patterns do)
// and `lastIndex` set, the way Prism's tokenizer calls it. Positions are
// UTF-16 units; a group that took no part is -1, -1. Texts may hold lone
// surrogates (JSON `\uD800` escapes).

'use strict';

const readline = require('readline');

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on('line', (line) => {
  const req = JSON.parse(line);
  let flags = req.flags;
  if (!flags.includes('g')) flags += 'g';
  // `d` gives each group's indices; it changes nothing else.
  if (!flags.includes('d')) flags += 'd';
  const re = new RegExp(req.source, flags);
  const results = req.cases.map(([text, last]) => {
    re.lastIndex = last;
    const m = re.exec(text);
    if (!m) return null;
    const out = [m.index, m.index + m[0].length];
    for (let i = 1; i < m.indices.length; i++) {
      const g = m.indices[i];
      if (g) out.push(g[0], g[1]);
      else out.push(-1, -1);
    }
    return out;
  });
  process.stdout.write(JSON.stringify({ results }) + '\n');
});
