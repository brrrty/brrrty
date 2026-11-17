// Writes the benchmark blog's content: Markdown posts in bench/site/src/posts
// and placeholder images in bench/site/src/images. Everything else in
// bench/site (configs, layouts, list pages) is checked in.
//
//   node bench/generate.mjs            # 1000 posts, 600 images
//   POSTS=200 node bench/generate.mjs  # a smaller site
//
// The output depends only on POSTS, IMAGES and SEED: a seeded random number
// generator picks every word, and dates are counted from a fixed day, so two
// runs anywhere give the same files. The last line printed is a hash of them.

import { createHash } from "node:crypto";
import { mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const POSTS = Number(process.env.POSTS ?? 1000);
const IMAGES = Number(process.env.IMAGES ?? 600);
const SEED = Number(process.env.SEED ?? 20140105);

const here = dirname(fileURLToPath(import.meta.url));
const src = join(here, "site", "src");
const postsDir = join(src, "posts");
const imagesDir = join(src, "images");

// mulberry32: small, fast, and the same in every JavaScript engine.
let state = SEED >>> 0;
function random() {
  state = (state + 0x6d2b79f5) >>> 0;
  let t = state;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const int = (lo, hi) => lo + Math.floor(random() * (hi - lo + 1));
const pick = (list) => list[Math.floor(random() * list.length)];
const chance = (p) => random() < p;
const cap = (s) => s[0].toUpperCase() + s.slice(1);
const a = (word) => (/^[aeiou]/.test(word) ? `an ${word}` : `a ${word}`);
// "parse" to "parses", "fetch" to "fetches", "retry" to "retries".
const s3 = (v) => (/(s|sh|ch|x|z)$/.test(v) ? v + "es" : /[^aeiou]y$/.test(v) ? v.slice(0, -1) + "ies" : v + "s");
// "parse" to "parsing", "ship" to "shipping".
const DOUBLED = new Set(["ship", "pin", "wrap", "unwrap", "trim", "log", "debug", "split", "stop"]);
const ing = (v) => (DOUBLED.has(v) ? v + v.at(-1) + "ing" : /[^e]e$/.test(v) ? v.slice(0, -1) + "ing" : v + "ing");

const NOUNS = (
  "cache build server request response buffer queue thread worker process " +
  "module package library compiler parser lexer token template layout page " +
  "route handler config setting option flag value index table column row " +
  "schema query record batch stream pipeline job task schedule timer event " +
  "signal socket port host client proxy gateway service cluster node replica " +
  "shard volume disk file folder path link image asset bundle chunk script " +
  "style sheet theme color font grid border margin layer shadow animation " +
  "frame budget metric chart report trace log alert dashboard release branch " +
  "commit patch review test fixture mock benchmark profile snapshot backup " +
  "migration deploy rollout feature toggle limit quota key secret token store"
).split(" ");
const VERBS = (
  "build cache render parse compile measure profile split merge sort filter " +
  "index store load fetch send queue retry batch trim shrink grow move copy " +
  "rename check test deploy ship release tune debug trace log watch serve " +
  "pin lock bump wrap unwrap stream compress encode decode hash sign verify"
).split(" ");
const ADJECTIVES = (
  "small large fast slow quiet busy simple plain careful steady stable fresh " +
  "stale empty full shared local remote static dynamic lazy eager strict " +
  "loose clean tidy narrow wide shallow deep early late warm cold safe " +
  "portable modest honest boring useful tiny sturdy reliable gentle flexible"
).split(" ");
const ADVERBS = (
  "quickly slowly carefully quietly rarely often usually always sometimes " +
  "mostly barely nearly simply plainly safely gently"
).split(" ");
const CONNECTORS = [
  "and then", "but", "so", "because", "while", "until", "after", "before",
  "unless", "since", "although", "once",
];
const OPENERS = [
  "In practice,", "Most of the time,", "On a good day,", "As a rule,",
  "For a while,", "In the end,", "At first,", "Later on,", "By default,",
  "To be fair,", "With some care,", "Every so often,",
];

const TAGS = [
  "javascript", "rust", "css", "html", "python", "bash", "performance",
  "testing", "tooling", "databases", "devops", "security", "accessibility",
  "design", "writing", "career", "networking", "linux", "macos", "editors",
  "git", "ci", "containers", "caching", "compilers", "parsing", "typography",
  "images", "fonts", "feeds", "static-sites", "markdown", "templates", "apis",
  "json", "yaml", "regex", "unicode", "dates", "logging",
];
const CATEGORIES = [
  "Tutorials", "Notes", "Reviews", "Essays", "Links", "Projects", "Releases",
  "Guides",
];
const LANGS = ["js", "rust", "bash", "css", "html", "json", "yaml", "python"];

function sentence() {
  const parts = [];
  if (chance(0.25)) parts.push(pick(OPENERS));
  parts.push(`the ${pick(ADJECTIVES)} ${pick(NOUNS)}`);
  parts.push(s3(pick(VERBS)));
  parts.push(chance(0.5) ? `the ${pick(NOUNS)}` : a(pick(NOUNS)));
  if (chance(0.5)) parts.push(pick(ADVERBS));
  if (chance(0.55)) {
    parts.push(pick(CONNECTORS));
    parts.push(`the ${pick(NOUNS)} ${s3(pick(VERBS))} ${pick(["it", "them", "nothing", "everything"])}`);
  }
  return cap(parts.join(" ")) + pick([".", ".", ".", ".", ".", ".", "!"]);
}

// A paragraph, with some inline Markdown mixed in.
function paragraph(links, min = 3, max = 6) {
  const out = [];
  for (let i = 0, n = int(min, max); i < n; i++) {
    let s = sentence();
    const roll = random();
    if (roll < 0.12) s = s.replace(/ (\w+)([.?!])$/, " `$1`$2");
    else if (roll < 0.22) s = s.replace(/ the (\w+) /, " the **$1** ");
    else if (roll < 0.3) s = s.replace(/ a (\w+)/, " a _$1_");
    else if (roll < 0.42 && links.length) {
      const [text, href] = pick(links);
      s = s.replace(/([.?!])$/, ` (see [${text}](${href}))$1`);
    }
    out.push(s);
  }
  return out.join(" ");
}

function ident() {
  return `${pick(VERBS)}${cap(pick(NOUNS))}`;
}
function snake() {
  return `${pick(VERBS)}_${pick(NOUNS)}`;
}

// One code block per language, filled in with random names. None contains
// `{{` or `{%`, since Markdown pages are run through Liquid first.
const CODE = {
  js() {
    const fn = ident();
    const a = pick(NOUNS);
    const b = pick(NOUNS);
    return [
      `// ${cap(sentence())}`,
      `export async function ${fn}(${a}s, options = {}) {`,
      `  const limit = options.limit ?? ${int(2, 64)};`,
      `  const seen = new Map();`,
      `  for (const ${a} of ${a}s.slice(0, limit)) {`,
      `    const key = \`\${${a}.id}:${b}\`;`,
      `    if (seen.has(key)) continue;`,
      `    seen.set(key, await fetch(\`/api/${b}s/\${${a}.id}\`).then((r) => r.json()));`,
      `  }`,
      `  return [...seen.values()].filter(Boolean);`,
      `}`,
    ].join("\n");
  },
  rust() {
    const ty = cap(pick(NOUNS));
    const field = pick(NOUNS);
    return [
      `/// ${cap(sentence())}`,
      `#[derive(Debug, Clone, PartialEq)]`,
      `pub struct ${ty} {`,
      `    pub ${field}: Vec<String>,`,
      `    pub limit: usize,`,
      `}`,
      ``,
      `impl ${ty} {`,
      `    pub fn ${snake()}(&self, input: &str) -> Option<usize> {`,
      `        let count = input.split_whitespace().filter(|w| w.len() > ${int(2, 9)}).count();`,
      `        if count > self.limit {`,
      `            return None;`,
      `        }`,
      `        println!("{} of {}", count, self.${field}.len());`,
      `        Some(count * ${int(2, 16)})`,
      `    }`,
      `}`,
    ].join("\n");
  },
  bash() {
    const dir = pick(NOUNS);
    return [
      `#!/usr/bin/env bash`,
      `set -euo pipefail`,
      ``,
      `# ${cap(sentence())}`,
      `${dir.toUpperCase()}_DIR="\${1:-./${dir}s}"`,
      `for f in "$${dir.toUpperCase()}_DIR"/*.txt; do`,
      `  lines=$(wc -l < "$f")`,
      `  if [ "$lines" -gt ${int(10, 500)} ]; then`,
      `    echo "trimming $f ($lines lines)"`,
      `    tail -n ${int(5, 50)} "$f" > "$f.tmp" && mv "$f.tmp" "$f"`,
      `  fi`,
      `done | tee -a ${pick(NOUNS)}.log`,
    ].join("\n");
  },
  css() {
    const cls = `${pick(ADJECTIVES)}-${pick(NOUNS)}`;
    return [
      `/* ${cap(sentence())} */`,
      `.${cls} {`,
      `  display: grid;`,
      `  grid-template-columns: repeat(auto-fill, minmax(${int(8, 20)}rem, 1fr));`,
      `  gap: ${int(1, 4)}rem;`,
      `  color: #${int(0, 0xffffff).toString(16).padStart(6, "0")};`,
      `}`,
      ``,
      `.${cls} > a:hover,`,
      `.${cls} > a:focus-visible {`,
      `  outline: ${int(1, 3)}px solid currentColor;`,
      `}`,
      ``,
      `@media (max-width: ${int(30, 60)}em) {`,
      `  .${cls} { grid-template-columns: 1fr; }`,
      `}`,
    ].join("\n");
  },
  html() {
    const n = pick(NOUNS);
    return [
      `<!-- ${cap(sentence())} -->`,
      `<section class="${n}-list" aria-label="${cap(n)}s">`,
      `  <h2>${cap(pick(ADJECTIVES))} ${n}s</h2>`,
      `  <ul>`,
      `    <li><a href="/${n}s/${int(1, 99)}/">${cap(pick(NOUNS))}</a></li>`,
      `    <li><a href="/${n}s/${int(100, 199)}/">${cap(pick(NOUNS))}</a></li>`,
      `  </ul>`,
      `  <button type="button" data-${pick(VERBS)}="true">More &amp; more</button>`,
      `</section>`,
    ].join("\n");
  },
  json() {
    const n = pick(NOUNS);
    return [
      `{`,
      `  "name": "${pick(ADJECTIVES)}-${n}",`,
      `  "version": "${int(0, 4)}.${int(0, 20)}.${int(0, 9)}",`,
      `  "private": ${chance(0.5)},`,
      `  "${n}s": [`,
      `    { "id": ${int(1, 999)}, "label": "${pick(NOUNS)}", "weight": ${(random() * 10).toFixed(2)} },`,
      `    { "id": ${int(1, 999)}, "label": "${pick(NOUNS)}", "weight": null }`,
      `  ],`,
      `  "scripts": { "${pick(VERBS)}": "node ${pick(VERBS)}.js --${pick(NOUNS)}" }`,
      `}`,
    ].join("\n");
  },
  yaml() {
    const n = pick(NOUNS);
    return [
      `# ${cap(sentence())}`,
      `name: ${pick(VERBS)}-${n}`,
      `on:`,
      `  push:`,
      `    branches: [main]`,
      `jobs:`,
      `  ${n}:`,
      `    runs-on: ubuntu-latest`,
      `    timeout-minutes: ${int(5, 60)}`,
      `    steps:`,
      `      - run: make ${pick(VERBS)}`,
      `      - run: ./scripts/${pick(VERBS)}-${pick(NOUNS)}.sh --limit ${int(1, 99)}`,
      `        env:`,
      `          ${pick(NOUNS).toUpperCase()}_MODE: "${pick(ADJECTIVES)}"`,
    ].join("\n");
  },
  python() {
    const fn = snake();
    const n = pick(NOUNS);
    return [
      `from collections import Counter`,
      ``,
      ``,
      `def ${fn}(${n}s, limit=${int(2, 50)}):`,
      `    """${cap(sentence())}"""`,
      `    counts = Counter(x.lower() for x in ${n}s if x)`,
      `    for name, total in counts.most_common(limit):`,
      `        if total < ${int(2, 9)}:`,
      `            break`,
      `        print(f"{name}: {total}")`,
      `    return dict(counts)`,
      ``,
      ``,
      `if __name__ == "__main__":`,
      `    ${fn}(["${pick(NOUNS)}", "${pick(NOUNS)}", "${pick(NOUNS)}"])`,
    ].join("\n");
  },
};

function list(links) {
  const ordered = chance(0.4);
  const items = [];
  for (let i = 0, n = int(3, 6); i < n; i++) {
    let item = `${cap(pick(VERBS))} the ${pick(ADJECTIVES)} ${pick(NOUNS)}`;
    if (chance(0.3)) item += ` before the ${pick(NOUNS)} ${s3(pick(VERBS))}`;
    if (chance(0.15) && links.length) {
      const [text, href] = pick(links);
      item += `, as in [${text}](${href})`;
    }
    items.push(`${ordered ? `${i + 1}.` : "-"} ${item}.`);
  }
  return items.join("\n");
}

function table() {
  const rows = [
    `| ${cap(pick(NOUNS))} | ${cap(pick(NOUNS))} | Time |`,
    `| --- | --- | ---: |`,
  ];
  for (let i = 0, n = int(2, 5); i < n; i++) {
    rows.push(`| ${pick(ADJECTIVES)} | \`${pick(NOUNS)}\` | ${int(1, 900)} ms |`);
  }
  return rows.join("\n");
}

function image(alt) {
  const n = int(1, IMAGES);
  return `![${alt}](/images/figure-${String(n).padStart(4, "0")}.svg)`;
}

function svg(n) {
  const w = pick([320, 480, 640]);
  const h = pick([180, 240, 320]);
  const hue = int(0, 359);
  return [
    `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">`,
    `  <rect width="${w}" height="${h}" fill="hsl(${hue} 45% 85%)"/>`,
    `  <circle cx="${int(40, w - 40)}" cy="${int(40, h - 40)}" r="${int(10, 40)}" fill="hsl(${(hue + 180) % 360} 50% 45%)"/>`,
    `  <text x="16" y="${h - 16}" font-family="sans-serif" font-size="16">Figure ${n}</text>`,
    `</svg>`,
    ``,
  ].join("\n");
}

// Tags follow a long tail, as on a real blog: a few are on many posts.
function tags() {
  const picked = new Set();
  for (let i = 0, n = int(2, 4); i < n; i++) {
    picked.add(TAGS[Math.floor(TAGS.length * random() ** 1.8)]);
  }
  return [...picked];
}

function title(used) {
  for (;;) {
    const forms = [
      () => `${cap(ing(pick(VERBS)))} the ${pick(ADJECTIVES)} ${pick(NOUNS)}`,
      () => `Why the ${pick(NOUNS)} ${s3(pick(VERBS))} ${pick(ADVERBS)}`,
      () => `${cap(a(pick(ADJECTIVES)))} ${pick(NOUNS)} for ${pick(ADJECTIVES)} ${pick(NOUNS)}s`,
      () => `Notes on ${pick(NOUNS)} ${pick(NOUNS)}s`,
      () => `How to ${pick(VERBS)} ${a(pick(NOUNS))} without ${a(pick(NOUNS))}`,
      () => `${cap(pick(ADJECTIVES))} ${pick(NOUNS)}s, ${pick(ADJECTIVES)} ${pick(NOUNS)}s`,
    ];
    const t = pick(forms)();
    const slug = t.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
    if (!used.has(slug)) {
      used.add(slug);
      return [t, slug];
    }
  }
}

function post(index, date, links) {
  const lines = [paragraph(links, 4, 6)];
  const langs = [pick(LANGS)];
  if (chance(0.45)) langs.push(pick(LANGS));
  const sections = int(2, 4);
  for (let s = 0; s < sections; s++) {
    lines.push(`## ${cap(ing(pick(VERBS)))} the ${pick(NOUNS)}`);
    lines.push(paragraph(links));
    const roll = random();
    if (roll < 0.35) lines.push(list(links));
    else if (roll < 0.5) lines.push(`> ${sentence()} ${sentence()}`);
    else if (roll < 0.6) lines.push(table());
    if (langs.length > s) lines.push("```" + langs[s] + "\n" + CODE[langs[s]]() + "\n```");
    if (chance(0.3)) lines.push(image(`${cap(pick(ADJECTIVES))} ${pick(NOUNS)} diagram`));
    if (chance(0.3)) {
      lines.push(`### ${cap(pick(ADJECTIVES))} ${pick(NOUNS)}s`);
      lines.push(paragraph(links, 2, 4));
    }
  }
  for (const lang of langs.slice(sections)) lines.push("```" + lang + "\n" + CODE[lang]() + "\n```");
  lines.push(
    `Further reading: [the ${pick(NOUNS)} guide](https://example.com/docs/${pick(NOUNS)}/) and ` +
      `[${pick(ADJECTIVES)} ${pick(NOUNS)}s](https://example.org/${pick(VERBS)}/${index}).`,
  );
  return lines.join("\n\n");
}

// Clear out an earlier run. posts/posts.11tydata.json is checked in, so only
// the Markdown goes.
mkdirSync(postsDir, { recursive: true });
for (const name of readdirSync(postsDir)) {
  if (name.endsWith(".md")) rmSync(join(postsDir, name));
}
rmSync(imagesDir, { recursive: true, force: true });
mkdirSync(imagesDir, { recursive: true });

const files = [];
function write(path, text) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, text);
  files.push([path.slice(src.length + 1), text]);
}

const used = new Set();
const links = []; // [text, href] of posts written so far, so links point back
const start = Date.UTC(2014, 0, 5);
for (let i = 0; i < POSTS; i++) {
  // About one post every four days, never two on one day.
  const date = new Date(start + (i * 4 + int(0, 2)) * 86400000).toISOString().slice(0, 10);
  const [t, slug] = title(used);
  const body = post(i + 1, date, links.slice(-60));
  const fm = [
    "---",
    `title: "${t}"`,
    `date: ${date}`,
    `category: ${pick(CATEGORIES)}`,
    "tags:",
    ...tags().map((tag) => `  - ${tag}`),
    `description: "${sentence().replace(/"/g, "")}"`,
    "---",
    "",
  ].join("\n");
  write(join(postsDir, `${date}-${slug}.md`), fm + body + "\n");
  links.push([t, `/posts/${slug}/`]);
}
for (let n = 1; n <= IMAGES; n++) {
  write(join(imagesDir, `figure-${String(n).padStart(4, "0")}.svg`), svg(n));
}

const hash = createHash("sha256");
for (const [path, text] of files.sort((a, b) => (a[0] < b[0] ? -1 : 1))) {
  hash.update(path).update("\0").update(text).update("\0");
}
console.log(`Generated ${POSTS} posts and ${IMAGES} images (seed ${SEED}).`);
console.log(`content sha256 ${hash.digest("hex")}`);
