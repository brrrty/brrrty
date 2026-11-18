// Generates the computed-data proxy probe: 200 functions over generated
// page data, each with the computed keys of its page. Deterministic (a
// seeded generator), so `cases.json` can be regenerated and reviewed.
//
//   node gen.mjs > cases.json
//
// `probe-11ty.mjs` runs Eleventy 3.1.8's own ComputedDataProxy over these
// cases (in Docker, see its header) and writes `expected.json`; the
// `proxy` test in brrrty-node runs Brrrty's port over the same cases.

let seed = 17;
function rand(n) {
  seed = (seed * 1103515245 + 12345) % 2147483648;
  return seed % n;
}
function pick(list) {
  return list[rand(list.length)];
}

const reads = [
  "d.title",
  "d.title.toUpperCase()",
  "d.site.name",
  "d.site.links[0].href",
  "d.site.links.length",
  "d.site.links.map((l) => l.href).join()",
  "d.tags",
  "d.tags[1]",
  "d.tags.includes('a')",
  "d.page.url",
  "d.page.fileSlug",
  "d.missing",
  "d.missing && d.missing.deep",
  "d.missing?.deep",
  "d.count + 1",
  "`${d.title}-${d.count}`",
  "typeof d.site",
  "'name' in d.site",
  "Object.keys(d.site).length",
  "JSON.stringify(d.site)",
  "d.collections.post",
  "d.collections.all.length",
  "d.a",
  "d.b",
  "d.nested.x",
  "d.nested",
  "d.flag ? d.site.name : d.title",
  "d.list[2].v",
  "d.list.filter(Boolean).length",
  "String(d.when)",
  "d.n === null",
  "(({ title, count }) => title + count)(d)",
  "[...d.tags].length",
  "d.site.links[1]?.href",
];

const computedKeys = ["a", "b", "nested.x", "nested.y", "title"];

const cases = [];
for (let i = 0; i < 200; i++) {
  const n = 1 + rand(3);
  const parts = [];
  for (let j = 0; j < n; j++) parts.push(pick(reads));
  const body = parts.length === 1 ? parts[0] : `[${parts.join(", ")}]`;
  const asyncFn = rand(4) === 0;
  const fn = `${asyncFn ? "async " : ""}(d) => ${body}`;
  const keys = ["page.url", "page.outputPath"];
  for (const k of computedKeys) if (rand(2)) keys.push(k);
  const data = {
    title: pick(["Hello", "x"]),
    count: rand(10),
    flag: rand(2) === 1,
    n: rand(2) ? null : 0,
    when: "2026-01-02",
    site: { name: "S", links: [{ href: "/a" }, { href: "/b" }] },
    tags: ["a", "b"],
    list: [1, 0, { v: 3 }],
    page: { fileSlug: "f", inputPath: "./f.md" },
  };
  if (rand(2)) data.a = "A";
  if (rand(3) === 0) data.nested = { y: 1 };
  if (rand(4) === 0) data.collections = { post: [] };
  cases.push({ fn, keys, data });
}
process.stdout.write(JSON.stringify(cases, null, 1) + "\n");
