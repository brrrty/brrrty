const { DateTime } = require("luxon");
const slugify = require("slugify");
const yaml = require("js-yaml");

const base = DateTime.fromISO("2026-03-01T12:00:00", { zone: "America/Chicago" });
const when = DateTime.fromISO("2026-02-14T09:30:00", { zone: "America/Chicago" });

module.exports = {
  formatted: when.toFormat("cccc d LLLL yyyy, HH:mm ZZZZ"),
  french: when.setLocale("fr").toFormat("cccc d LLLL yyyy"),
  relative: when.toRelative({ base }),
  iso: when.toISO(),
  slug: slugify("Hello, World & Friends!", { lower: true, strict: true }),
  slugPlain: slugify("Crème Brûlée ☃ 2026"),
  parsed: yaml.load("title: Hello\nlist:\n  - 1\n  - two\nnested:\n  a: true\n"),
  dumped: yaml.dump({ b: 1, a: [1, "x"], s: "multi\nline" }),
};
