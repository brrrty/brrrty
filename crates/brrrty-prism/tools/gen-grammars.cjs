// Regenerates grammars/prism.json: the Prism 1.30.0 grammars that
// @11ty/eleventy-plugin-syntaxhighlight 5.0.2 highlights with, for the fence
// languages below and everything they load.
//
//   node tools/gen-grammars.cjs <node_modules> > grammars/prism.json
//
// <node_modules> is a directory with prismjs 1.30.0 and
// @11ty/eleventy-plugin-syntaxhighlight 5.0.2 installed. Run from
// crates/brrrty-prism with Node 20 or later (the file was made with v26.2.0).
//
// The languages are loaded the way the plugin loads them: `require("prismjs")`
// (core, markup, css, clike, javascript), then the plugin's own PrismLoader.js,
// which loads diff and the diff-highlight plugin and then, for each name,
// normalizes the alias and calls Prism's `loadLanguages`. So `require`
// dependencies, `extend` and `insertBefore` are resolved by Prism itself.
//
// Grammars are written as they stand once loaded, before any tokenizing (the
// tokenizer merges `rest` into its grammar on first use). Each grammar is
// {"tokens": [[name, [pattern, ...]], ...], "rest": id | null}, its tokens in
// Prism's `for...in` order, and each pattern is
// {"source", "flags", "lookbehind", "greedy", "alias": [..], "inside": id | null}.
// `inside` and `rest` are ids into "grammars", so shared and cyclic grammars
// stay shared and cyclic.
//
// Which grammar a language gets can depend on the order languages are loaded
// in (a later language can modify or reload an earlier one). Each fence
// language is therefore also loaded on its own, and all of them in three
// more orders, and every grammar compared with this one. "orderDependent"
// lists, per language, the patterns that differ. A pattern whose `inside` is
// there in one order and missing in another (markdown's front matter, which
// is YAML only once yaml is loaded) is recorded for the tokenizer to refuse
// where it matters; any other difference fails the generator.
//
// "baseKeys" are the grammars Prism.languages has before any language is
// loaded (Prism's core, then the plugin's diff), and "keysByLanguage" the
// ones it has once a fence language is loaded on its own: what a hook that
// looks a language up surely finds, whatever order 11ty renders pages in.
//
// "hooks" has the source of every hook Prism has once loaded, and
// "hooksByLanguage" how many of each a fence language adds alone; the
// tokenizer has to port them. "components" is Prism's components.json
// languages and aliases, for the plugin's alias normalization.

'use strict';

const { execFileSync } = require('child_process');
const path = require('path');

// Every fence language on the test sites, as written in the info strings.
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
  const components = require(path.join(prismDir, 'components.json'));
  return { Prism, PrismLoader, normalize, components, versions };
}

function isGrammar(v) {
  return v !== null && typeof v === 'object' && !(v instanceof RegExp) && !Array.isArray(v);
}

const PATTERN_KEYS = new Set(['pattern', 'lookbehind', 'greedy', 'alias', 'inside']);

// The grammars reachable from `roots` (objects), numbered in the order they
// are first reached, depth first.
function snapshot(roots) {
  const ids = new Map();
  const grammars = [];
  function visit(g) {
    if (!isGrammar(g)) throw new Error('not a grammar: ' + String(g));
    if (ids.has(g)) return ids.get(g);
    const id = grammars.length;
    ids.set(g, id);
    const out = { tokens: [], rest: null };
    grammars.push(out);
    for (const name in g) {
      // Exactly what Prism's matchGrammar skips.
      if (!Object.prototype.hasOwnProperty.call(g, name) || !g[name]) continue;
      if (name === 'rest') {
        out.rest = visit(g.rest);
        continue;
      }
      const list = Array.isArray(g[name]) ? g[name] : [g[name]];
      const patterns = list.map((p) => {
        const re = p instanceof RegExp ? p : p.pattern;
        if (!(re instanceof RegExp)) throw new Error(`token ${name}: no pattern`);
        if (!(p instanceof RegExp)) {
          for (const k of Object.keys(p)) {
            if (!PATTERN_KEYS.has(k)) throw new Error(`token ${name}: unknown key ${k}`);
          }
        }
        const alias = p instanceof RegExp || !p.alias ? [] : Array.isArray(p.alias) ? p.alias : [p.alias];
        for (const a of alias) if (typeof a !== 'string') throw new Error(`token ${name}: alias`);
        return {
          source: re.source,
          flags: re.flags,
          lookbehind: !(p instanceof RegExp) && !!p.lookbehind,
          greedy: !(p instanceof RegExp) && !!p.greedy,
          alias,
          inside: !(p instanceof RegExp) && p.inside ? p.inside : null,
        };
      });
      out.tokens.push([name, patterns]);
    }
    // Number nested grammars after this one's tokens are listed.
    for (const [, patterns] of out.tokens) {
      for (const p of patterns) if (p.inside) p.inside = visit(p.inside);
    }
    return id;
  }
  const rootIds = roots.map(visit);
  return { rootIds, grammars };
}

// Child: load `names` in order and print the state.
function child(nm, names) {
  const { Prism, PrismLoader, normalize, components, versions } = load(nm);
  const hooksBefore = {};
  for (const [k, v] of Object.entries(Prism.hooks.all)) hooksBefore[k] = v.length;
  const baseKeys = Object.keys(Prism.languages).filter((k) => isGrammar(Prism.languages[k]));
  const fences = names.map((name) => {
    const id = normalize(name);
    PrismLoader(name);
    return [name, id, isGrammar(Prism.languages[id]) ? id : null];
  });
  const languages = Object.keys(Prism.languages).filter((k) => isGrammar(Prism.languages[k]));
  const snap = snapshot(languages.map((k) => Prism.languages[k]));
  const hooks = {};
  for (const [k, v] of Object.entries(Prism.hooks.all)) {
    hooks[k] = v.map((f) => String(f));
  }
  return {
    versions,
    fences,
    languages: languages.map((k, i) => [k, snap.rootIds[i]]),
    grammars: snap.grammars,
    hooks,
    hooksBefore,
    baseKeys,
    components: Object.entries(components.languages)
      .filter(([id]) => id !== 'meta')
      .map(([id, c]) => [id, c.alias === undefined ? [] : [].concat(c.alias)]),
  };
}

function run(nm, names) {
  const out = execFileSync(process.execPath, [__filename, '--child', nm, JSON.stringify(names)], {
    maxBuffer: 1 << 30,
  });
  return JSON.parse(out);
}

// Where the grammars reachable from `a` in state `sa` and from `b` in `sb`
// differ, walking both in step: "token/index > token/index: field".
function differences(sa, a, sb, b) {
  const out = [];
  const seen = new Set();
  function walk(x, y, at) {
    const key = x + ',' + y;
    if (seen.has(key)) return;
    seen.add(key);
    const gx = sa.grammars[x];
    const gy = sb.grammars[y];
    const names = (g) => g.tokens.map(([n, ps]) => n + '*' + ps.length).join(' ');
    if (names(gx) !== names(gy)) return out.push(`${at || '(root)'}: tokens`);
    if ((gx.rest === null) !== (gy.rest === null)) return out.push(`${at || '(root)'}: rest`);
    if (gx.rest !== null) walk(gx.rest, gy.rest, at + ' > rest');
    gx.tokens.forEach(([n, ps], t) => {
      ps.forEach((p, i) => {
        const q = gy.tokens[t][1][i];
        const here = (at ? at + ' > ' : '') + n + '/' + i;
        for (const k of ['source', 'flags', 'lookbehind', 'greedy', 'alias']) {
          if (JSON.stringify(p[k]) !== JSON.stringify(q[k])) out.push(`${here}: ${k}`);
        }
        if ((p.inside === null) !== (q.inside === null)) out.push(`${here}: inside`);
        else if (p.inside !== null) walk(p.inside, q.inside, here);
      });
    });
  }
  walk(a, b, '');
  return out;
}

function rootOf(state, lang) {
  const hit = state.languages.find(([k]) => k === lang);
  return hit ? hit[1] : null;
}

// A seeded shuffle, so the output is the same on every run.
function shuffled(xs, seed) {
  const out = xs.slice();
  let s = seed;
  for (let i = out.length - 1; i > 0; i--) {
    s = (s * 1103515245 + 12345) % 2147483648;
    const j = s % (i + 1);
    [out[i], out[j]] = [out[j], out[i]];
  }
  return out;
}

// JSON with one line for anything holding no array or object of more than
// scalars (a pattern, a fence, a list of names), so diffs stay small.
function pretty(v, indent) {
  const flat = (x) => x === null || typeof x !== 'object';
  const simple = (x) => flat(x) || Object.values(x).every((y) => flat(y) || (Array.isArray(y) && y.every(flat)));
  if (simple(v)) return JSON.stringify(v);
  const inner = indent + ' ';
  if (Array.isArray(v)) {
    return '[\n' + v.map((x) => inner + pretty(x, inner)).join(',\n') + '\n' + indent + ']';
  }
  const entries = Object.entries(v).map(([k, x]) => inner + JSON.stringify(k) + ': ' + pretty(x, inner));
  return '{\n' + entries.join(',\n') + '\n' + indent + '}';
}

function main(nm) {
  const state = run(nm, FENCES);
  const others = [run(nm, FENCES.slice().reverse()), run(nm, shuffled(FENCES, 7)), run(nm, shuffled(FENCES, 11))];
  const langs = state.languages.map(([k]) => k);
  // language -> the patterns whose grammar differs between load orders.
  const orderDependent = {};
  const alone = new Map();
  for (const [name] of state.fences) {
    if (!alone.has(name)) alone.set(name, run(nm, [name]));
  }
  for (const lang of langs) {
    const states = others.slice();
    // Each fence loaded alone, where it loads `lang`.
    for (const solo of alone.values()) {
      if (rootOf(solo, lang) !== null) states.push(solo);
    }
    for (const s of states) {
      const r = rootOf(s, lang);
      if (r === null) throw new Error(`${lang} is missing in another load order`);
      for (const d of differences(state, rootOf(state, lang), s, r)) {
        const list = (orderDependent[lang] = orderDependent[lang] || []);
        if (!list.includes(d)) list.push(d);
      }
    }
  }
  const hooksByLanguage = {};
  for (const [name, id] of state.fences) {
    const solo = alone.get(name);
    const added = {};
    for (const [k, v] of Object.entries(solo.hooks)) {
      const n = v.length - (solo.hooksBefore[k] || 0);
      if (n > 0) added[k] = n;
    }
    if (id !== null && !(id in hooksByLanguage)) hooksByLanguage[id] = added;
  }
  // What Prism.languages surely has once a fence language is loaded,
  // whatever was loaded before: its keys when loaded alone.
  const keysByLanguage = {};
  for (const [name, id] of state.fences) {
    if (id !== null && !(id in keysByLanguage)) {
      keysByLanguage[id] = alone.get(name).languages.map(([k]) => k);
    }
  }
  const out = {
    generator: 'tools/gen-grammars.cjs',
    node: process.version,
    prism: state.versions.prism,
    plugin: state.versions.plugin,
    fences: state.fences.map(([name, normalized, grammar]) => ({ name, normalized, known: grammar !== null })),
    languages: Object.fromEntries(state.languages),
    orderDependent,
    hooksByLanguage,
    baseKeys: state.baseKeys,
    keysByLanguage,
    hooks: state.hooks,
    components: state.components,
    grammars: state.grammars,
  };
  process.stdout.write(pretty(out, '') + '\n');
  // A pattern whose `inside` is there in one order and not in another (a
  // language that embeds an optional one) is recorded: the tokenizer can
  // refuse code where that pattern matches. Any other difference fails.
  for (const [lang, ds] of Object.entries(orderDependent)) {
    for (const d of ds) {
      if (!d.endsWith(': inside')) {
        console.error(`${lang} depends on load order: ${d}`);
        process.exit(1);
      }
    }
  }
}

if (process.argv[2] === '--child') {
  process.stdout.write(JSON.stringify(child(process.argv[3], JSON.parse(process.argv[4]))));
} else if (process.argv.length === 3) {
  main(path.resolve(process.argv[2]));
} else {
  console.error('usage: node tools/gen-grammars.cjs <node_modules>');
  process.exit(2);
}
