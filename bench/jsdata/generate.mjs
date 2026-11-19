// Writes the JavaScript data benchmark sites into a folder: each one leans on
// what Brrrty runs in Node under `--js-data=node`.
//
//   node bench/jsdata/generate.mjs <out-folder>
//
// - `calls/`: a global data file that exports a large list (ITEMS items) and
//   a lookup table next to its functions, and PAGES Nunjucks pages that call
//   four of them each. Brrrty checks after every call that the call left the
//   exported data as it was.
// - `templates/`: TEMPLATE_PAGES pages from one paginated `.11ty.js`
//   template (a permalink function, `this.slugify`, `this.url`, a Nunjucks
//   layout), plus 200 single `.11ty.js` pages with data of their own.
//
// Each site gets the package.json and package-lock.json of bench/site, so
// `npm ci` installs the same 11ty 3.1.8, and a `brrrty.config.rhai` that
// turns JavaScript data on. No randomness: the output is the same on every
// machine.

import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const out = resolve(process.argv[2] ?? join(here, "out"));

const PAGES = Number(process.env.PAGES ?? 2000);
const ITEMS = Number(process.env.ITEMS ?? 30000);
const TEMPLATE_PAGES = Number(process.env.TEMPLATE_PAGES ?? 4000);
const SINGLES = Number(process.env.SINGLES ?? 200);

function site(name, files) {
  const dir = join(out, name);
  rmSync(dir, { recursive: true, force: true });
  for (const [path, text] of Object.entries(files)) {
    const file = join(dir, path);
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, text);
  }
  for (const f of ["package.json", "package-lock.json"]) {
    copyFileSync(join(here, "..", "site", f), join(dir, f));
  }
  return dir;
}

const CONFIG_11TY = `export default function () {
  return { dir: { input: "src" } };
}
`;
// The port of CONFIG_11TY, marked as checked against it.
const CONFIG_BRRRTY = `// Ported from: eleventy.config.js sha256:${createHash("sha256").update(CONFIG_11TY).digest("hex")}
config.set_js_data("node");
#{ dir: #{ input: "src" } }
`;

// --- calls ---

{
  const files = {
    "eleventy.config.js": CONFIG_11TY,
    "brrrty.config.rhai": CONFIG_BRRRTY,
    "src/_data/site.js": `// A lookup table and a large list, with the functions pages call.
const strings = {};
for (let i = 0; i < 3000; i++) strings["key" + i] = "value " + i;
const posts = [];
for (let i = 0; i < ${ITEMS}; i++) posts.push({ title: "t" + i, body: "x".repeat(200), tags: ["a", "b"] });
export default {
  strings,
  posts,
  t(k) {
    return this.strings[k];
  },
  up(s) {
    return String(s).toUpperCase();
  },
  money(n) {
    return "$" + Number(n).toFixed(2);
  },
};
`,
    "src/_includes/base.njk": `<!doctype html>
<title>{{ title }}</title>
<main>{{ content | safe }}</main>
`,
  };
  for (let i = 0; i < PAGES; i++) {
    files[`src/posts/p${i}.njk`] = `---
title: post ${i}
layout: base.njk
---
<h1>{{ site.up(title) }}</h1>
<p>{{ site.t("key${i % 3000}") }} {{ site.t("key1") }} {{ site.money(${i}.5) }}</p>
<p>{{ site.posts | length }} posts, the ${i}th is {{ site.posts[${i}].title }}</p>
`;
  }
  site("calls", files);
}

// --- templates ---

{
  const files = {
    "eleventy.config.js": CONFIG_11TY,
    "brrrty.config.rhai": CONFIG_BRRRTY,
    "src/_data/items.js": `const items = [];
for (let i = 0; i < ${TEMPLATE_PAGES}; i++) {
  items.push({ name: "Item " + i + " of the list", price: (i * 7) % 100, tags: ["t" + (i % 5), "u" + (i % 3)] });
}
export default items;
`,
    "src/_includes/base.njk": `<!doctype html>
<title>{{ title }}</title>
<main>{{ content | safe }}</main>
`,
    "src/items.11ty.js": `export const data = {
  title: "An item",
  layout: "base.njk",
  pagination: { data: "items", size: 1, alias: "item" },
  permalink(data) {
    return "/items/" + this.slugify(data.item.name) + "/";
  },
};

export function render(data) {
  const item = data.item;
  const tags = item.tags.map((t) => \`<li>\${t}</li>\`).join("");
  return \`<h1>\${item.name}</h1>
<p>Price: \${item.price}</p>
<ul>\${tags}</ul>
<a href="\${this.url("/")}">Home</a>
<p>Page \${data.pagination.pageNumber + 1} of \${data.pagination.pages.length}</p>
\`;
}
`,
  };
  for (let i = 0; i < SINGLES; i++) {
    files[`src/single/s${i}.11ty.js`] = `export const data = { title: "Single ${i}", layout: "base.njk", n: ${i} };

export function render(data) {
  return \`<h1>\${data.title}</h1><p>\${data.n * 2} \${this.slugify(data.title)}</p>\`;
}
`;
  }
  site("templates", files);
}

console.log(`wrote calls/ (${PAGES} pages, ${ITEMS} items) and templates/ (${TEMPLATE_PAGES} + ${SINGLES} pages) in ${out}`);
