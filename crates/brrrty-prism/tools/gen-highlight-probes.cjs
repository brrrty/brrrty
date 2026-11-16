// What Prism 1.30.0 highlights, the way @11ty/eleventy-plugin-syntaxhighlight
// 5.0.2 calls it, for brrrty-prism's tests. Three modes:
//
//   node tools/gen-highlight-probes.cjs <node_modules> fixtures <prism-tests> <out-dir>
//     Writes tests/probes/highlight/<language>.json from the code of Prism's
//     own test fixtures (<prism-tests> is tests/languages of a Prism 1.30.0
//     checkout; the files are MIT, as Prism is) and tests/fuzz/corpus.txt.
//
//   node tools/gen-highlight-probes.cjs <node_modules> sites <dir> <out-file>
//     Highlights every fenced code block in <dir>/*/src/**/*.md (rendered by
//     11ty's markdown-it with the plugin's own highlighter) and every
//     {% highlight %} body in <dir>/*/src (through the plugin's own paired
//     shortcode), and writes each Prism.highlight call they make as a JSON
//     line. For private site code: write it under target/ only.
//
//   node tools/gen-highlight-probes.cjs <node_modules> serve
//     Reads {"lang", "code"} JSON lines on stdin and writes one JSON line
//     each: {"html", "grammar"} (the grammar's Prism.languages key),
//     {"plain": true} (Prism has no grammar: the plugin writes the code as
//     is) or {"error"} (Prism threw).
//
// Every language the grammars were made with (gen-grammars.cjs) is loaded
// first, in that order, so Prism.languages holds what brrrty-prism's
// `AllLoaded` assumes. A name a highlight adds to Prism.languages (the
// diff-highlight hook, `diff-` languages) is removed again after it, so
// each highlight starts from the same state. Lone surrogates in the output
// are written as U+FFFD, as 11ty writes them to a file.

'use strict';

const fs = require('fs');
const path = require('path');
const readline = require('readline');

const FENCES = [
  'shell', 'php', 'javascript', 'sql', 'bash', 'conf', 'yaml', 'typescript',
  'json', 'ruby', 'python', 'markdown', 'jql', 'ini', 'html', 'css',
  'console', 'vim', 'liquid', 'c', 'xml', 'nginx', 'config', 'text', 'perl',
  'less', 'jsx', 'go', 'zsh', 'toml', 'terraform', 'rust', 'r', 'lua', 'js',
  'fstab', 'sh',
];

function load(nm) {
  const plugin = path.join(nm, '@11ty/eleventy-plugin-syntaxhighlight');
  const prismDir = path.dirname(require.resolve('prismjs/package.json', { paths: [plugin] }));
  const versions = {
    prism: require(path.join(prismDir, 'package.json')).version,
    plugin: require(path.join(plugin, 'package.json')).version,
  };
  if (versions.prism !== '1.30.0' || versions.plugin !== '5.0.2') {
    throw new Error(`need prismjs 1.30.0 and the plugin 5.0.2, found ${JSON.stringify(versions)}`);
  }
  const Prism = require(require.resolve('prismjs', { paths: [plugin] }));
  const PrismLoader = require(path.join(plugin, 'src/PrismLoader.js'));
  const normalize = require(path.join(plugin, 'src/PrismNormalizeAlias.js'));
  for (const f of FENCES) PrismLoader(f);
  return { Prism, PrismLoader, normalize, plugin };
}

// The text as 11ty writes it: lone surrogates become U+FFFD.
function wellFormed(s) {
  return Buffer.from(s, 'utf8').toString('utf8');
}

function grammarKey(normalize, lang) {
  return lang.startsWith('diff-') ? 'diff' : normalize(lang);
}

// The state is restored after each call: names added to Prism.languages
// are removed.
function withRestore(Prism, f) {
  const before = new Set(Object.keys(Prism.languages));
  try {
    return f();
  } finally {
    for (const k of Object.keys(Prism.languages)) {
      if (!before.has(k)) delete Prism.languages[k];
    }
  }
}

// What the plugin's highlighters do with `lang` and `code`, up to and
// including Prism.highlight.
function highlight(env, lang, code) {
  const { Prism, PrismLoader, normalize } = env;
  return withRestore(Prism, () => {
    try {
      const grammar = PrismLoader(lang);
      if (!grammar) return { plain: true };
      return { html: wellFormed(Prism.highlight(code, grammar, lang)), grammar: grammarKey(normalize, lang) };
    } catch (e) {
      return { error: String(e && e.message || e) };
    }
  });
}

function serve(env) {
  const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  rl.on('line', (line) => {
    const req = JSON.parse(line);
    process.stdout.write(JSON.stringify(highlight(env, req.lang, req.code)) + '\n');
  });
}

// The code of a Prism test fixture: everything before the line of dashes.
function fixtureCode(text) {
  const parts = text.split(/^-{10,}[ \t]*$/m);
  return parts[0].replace(/\r\n?/g, '\n').replace(/\s+$/, '');
}

// Sections of tests/fuzz/corpus.txt, by language.
function corpus(file) {
  const out = {};
  let lang = null;
  for (const line of fs.readFileSync(file, 'utf8').split(/(?<=\n)/)) {
    const m = /^=== (.+)$/.exec(line.trimEnd());
    if (m) {
      lang = m[1];
      (out[lang] = out[lang] || []).push('');
    } else if (lang) {
      const xs = out[lang];
      xs[xs.length - 1] += line;
    }
  }
  return out;
}

// Fixture directories (tests/languages/<dir>) and corpus sections, and the
// names each is highlighted as. A name's hooks (php, liquid, markdown/md,
// jsx, diff-) depend on the name, not just the grammar.
const FIXTURES = [
  ['bash', 'bash', ['bash']],
  ['php', 'php', ['php']],
  ['markup+php', 'php', ['php']],
  ['javascript', 'javascript', ['javascript']],
  ['typescript', 'typescript', ['typescript']],
  ['jsx', 'jsx', ['jsx']],
  ['sql', 'sql', ['sql']],
  ['yaml', 'yaml', ['yaml']],
  ['json', 'json', ['json']],
  ['ruby', 'ruby', ['ruby']],
  ['python', 'python', ['python']],
  ['markdown', 'markdown', ['markdown', 'md']],
  ['ini', 'ini', ['ini']],
  ['markup', 'markup', ['html', 'xml']],
  ['css', 'css', ['css']],
  ['less', 'less', ['less']],
  ['vim', 'vim', ['vim']],
  ['liquid', 'liquid', ['liquid']],
  ['c', 'c', ['c']],
  ['nginx', 'nginx', ['nginx']],
  ['perl', 'perl', ['perl']],
  ['go', 'go', ['go']],
  ['toml', 'toml', ['toml']],
  ['rust', 'rust', ['rust']],
  ['r', 'r', ['r']],
  ['lua', 'lua', ['lua']],
  ['diff', 'diff', ['diff', 'diff-javascript', 'diff-php']],
];

function fixtures(env, testsDir, outDir) {
  const corpusFile = path.join(__dirname, '../tests/fuzz/corpus.txt');
  const sections = corpus(corpusFile);
  fs.mkdirSync(outDir, { recursive: true });
  const byName = {};
  for (const [dir, section, names] of FIXTURES) {
    const codes = [];
    const full = path.join(testsDir, dir);
    for (const f of fs.readdirSync(full).sort()) {
      if (f.endsWith('.test')) {
        codes.push([`${dir}/${f}`, fixtureCode(fs.readFileSync(path.join(full, f), 'utf8'))]);
      }
    }
    (sections[section] || []).forEach((code, i) => codes.push([`corpus.txt ${section} ${i}`, code]));
    for (const name of names) {
      const cases = (byName[name] = byName[name] || []);
      for (const [from, code] of codes) {
        const r = highlight(env, name, code);
        if (r.error) throw new Error(`${from} as ${name}: ${r.error}`);
        cases.push({ from, code, ...r });
      }
    }
  }
  for (const [name, cases] of Object.entries(byName)) {
    const body = { prism: '1.30.0', plugin: '5.0.2', language: name, cases };
    fs.writeFileSync(path.join(outDir, `${name}.json`), JSON.stringify(body, null, 0) + '\n');
  }
}

function walk(dir, out) {
  let entries;
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const e of entries) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) {
      if (e.name !== 'node_modules' && !e.name.startsWith('.')) walk(p, out);
    } else {
      out.push(p);
    }
  }
}

function sites(env, nm, dir, outFile) {
  const { Prism, plugin } = env;
  const markdownIt = require(require.resolve('markdown-it', { paths: [path.join(nm, '@11ty/eleventy')] }));
  const mdOptions = require(path.join(plugin, 'src/markdownSyntaxHighlightOptions.js'));
  const paired = require(path.join(plugin, 'src/HighlightPairedShortcode.js'));
  const md = markdownIt({ html: true });
  md.set({ highlight: mdOptions({}) });

  // Record each top-level Prism.highlight call the plugin makes.
  const calls = [];
  const realHighlight = Prism.highlight;
  let depth = 0;
  let where = '';
  Prism.highlight = function (text, grammar, language) {
    depth++;
    try {
      const html = realHighlight.call(this, text, grammar, language);
      if (depth === 1) {
        calls.push({ where, lang: language, grammar: grammarKey(env.normalize, language), code: text, html: wellFormed(html) });
      }
      return html;
    } finally {
      depth--;
    }
  };

  const files = [];
  for (const site of fs.readdirSync(dir).sort()) walk(path.join(dir, site, 'src'), files);
  files.sort();
  let fences = 0;
  let tags = 0;
  const errors = [];
  for (const f of files) {
    if (!/\.(md|liquid|html|njk)$/.test(f)) continue;
    let text = fs.readFileSync(f, 'utf8');
    where = path.relative(dir, f);
    if (f.endsWith('.md')) {
      const body = text.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n/, '');
      const before = calls.length;
      try {
        withRestore(Prism, () => md.render(body));
      } catch (e) {
        errors.push(`${where}: ${e.message}`);
      }
      fences += calls.length - before;
    }
    const tag = /\{%-?\s*highlight\s+([^%]*?)\s*-?%\}([\s\S]*?)\{%-?\s*endhighlight\s*-?%\}/g;
    let m;
    while ((m = tag.exec(text))) {
      const split = m[1].split(' ');
      const language = split.shift();
      const before = calls.length;
      try {
        withRestore(Prism, () => paired(m[2].trim(), language, split.join(' '), {}));
      } catch (e) {
        errors.push(`${where}: ${e.message}`);
      }
      tags += calls.length - before;
    }
  }
  fs.mkdirSync(path.dirname(outFile), { recursive: true });
  fs.writeFileSync(outFile, calls.map((c) => JSON.stringify(c)).join('\n') + '\n');
  process.stderr.write(`${calls.length} highlights (${fences} from fences, ${tags} from tags), ${errors.length} errors\n`);
  for (const e of errors) process.stderr.write(`  ${e}\n`);
}

function main() {
  const [nm, mode, ...rest] = process.argv.slice(2);
  if (!nm || !mode) {
    process.stderr.write('usage: gen-highlight-probes.cjs <node_modules> fixtures|sites|serve ...\n');
    process.exit(2);
  }
  const env = load(nm);
  if (mode === 'serve') serve(env);
  else if (mode === 'fixtures') fixtures(env, rest[0], rest[1]);
  else if (mode === 'sites') sites(env, nm, rest[0], rest[1]);
  else {
    process.stderr.write(`unknown mode ${mode}\n`);
    process.exit(2);
  }
}

main();
