---
title: brrrty doctor
order: 1.5
summary: Everything that keeps your pages from building, in one list, and proof that the output matches 11ty's.
description: brrrty doctor lists every blocker per page, grouped by cause, and --verify compares the output with your 11ty build file by file.
lead: <code>brrrty doctor</code> looks at your site the way <code>brrrty build</code> does and lists everything that stops a page from building, all of it at once. With <code>--verify</code>, it also builds with your own 11ty and checks that every file is byte-identical.
---

```sh
brrrty doctor [--json] [--verify [--yes]] [--input DIR] [--output DIR] [--passthrough PATH]
```

| Flag | What it does |
|---|---|
| `--json` | Print one JSON object instead of the text report. |
| `--verify` | Also build the site with its own 11ty and with Brrrty, and compare every file. See below. |
| `--yes` | Run 11ty for `--verify` without asking. Needed when not on a terminal. |
| `--input <dir>`, `--output <dir>`, `--passthrough <path>` | As for `brrrty build`. Nothing is written to the output folder. |

## What it reports

`brrrty doctor` runs the real build, so what it reports is exactly what `brrrty build` would refuse, and the pages it counts as building are exactly the ones `brrrty build` writes. It writes nothing: no pages, no passthrough copies.

A build stops each page at its first problem. Doctor keeps going: for every page that is refused, it also lists what that first problem hid. That includes every filter the page, its layouts and its includes use that isn't defined, in every branch, and every JavaScript data file they read. Each problem is grouped under its cause:

- **A filter** that neither Brrrty nor your `brrrty.config.rhai` defines, such as an 11ty filter you haven't ported yet.
- **A JavaScript data file** a page reads. Brrrty doesn't run JavaScript, so each one is ported to a `.rhai` data file (see [Configuration](/docs/configuration/)).
- **A template language** Brrrty doesn't run, such as `.11ty.js`.
- **Anything else**, by the problem itself: a syntax error, a feature that isn't supported yet.

Each group lists how many pages it refuses, every file and line where it shows up, and how to fix it. The blockers that affect the most pages come first. Pages that only wait on other pages (a sitemap that needs every page in `collections`) come last, under "Waiting on the blockers above", and aren't counted as blockers.

```text
1. JavaScript data file src/_data/catalog.js (at least 3 pages, 3 templates)
   src/index.njk:5:10: this template reads `catalog`, which src/_data/catalog.js, a JavaScript data file, sets
   src/item.njk: this page uses `catalog` from src/_data/catalog.js, a JavaScript data file
   Fix: Brrrty doesn't run JavaScript data files; port src/_data/catalog.js to src/_data/catalog.rhai, a Rhai script whose last expression is the data (see PORTING.md).

2. Filter `usd` isn't defined (2 pages, 2 templates)
   src/index.njk:12:31: Error: filter not found: usd
   src/item.njk:8:20: Error: filter not found: usd
   Fix: Port it to brrrty.config.rhai with `config.add_filter("usd", |value| ...)`; PORTING.md says how.

4 of at least 7 pages will build; 2 blockers, grouped below.
```

The last line is the verdict. "At least" means a template paginates data that a blocker stops, so Brrrty can't know how many pages it makes yet: it counts as one until that blocker is fixed. That blocker comes first.

Warnings and notes are listed after the groups. They stop nothing.

## `--verify`

```sh
npx brrrty doctor --verify
```

With `--verify`, doctor also builds your site twice, once with the 11ty installed in your project (`node_modules/@11ty/eleventy`) and once with Brrrty, and compares every file:

```text
Built your site with 11ty 3.1.8 and with Brrrty 0.0.13, then compared every file.
  2,272 files: 2,272 byte-identical, 0 different, 0 missing, 0 extra
  11ty: 2.39 s   Brrrty: 0.89 s   (2.7x faster)

Your site is byte-compatible on Brrrty: 2,272 files, 0.89 s vs 2.39 s
```

The last line is meant for pasting into an issue or a post. When files differ, the first diffs are shown, and the files only one build wrote are listed. A file that differs only in whitespace counts as different: the check is for byte-identical output.

- **Your 11ty runs your site's JavaScript:** its config, data files and plugins, with your permissions. Doctor says so before it starts. On a terminal it asks first; anywhere else it needs `--yes`. Brrrty itself never runs JavaScript.
- **Both builds write to the system temp folder,** which is removed afterwards. Your output folder is left alone. 11ty itself may still write caches in your project, as it does in a normal build.
- **The clock is frozen.** Both builds run at the same instant: `SOURCE_DATE_EPOCH` when it is set, otherwise the time doctor starts. Brrrty reads it itself. 11ty gets it through a small preload script that makes `Date.now()` and `new Date()` return it, so a page that prints the build time matches.
- **Node and 11ty are needed** only for this check. When either is missing, doctor says how to install it.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Every page builds and, with `--verify`, every file is byte-identical. |
| 1 | Something is refused, or with `--verify` some file differs, or 11ty's build failed. |
| 2 | A usage error: a bad flag, `--verify` off a terminal without `--yes`, or Node or 11ty not installed. |

## JSON

`--json` prints one object with the same findings: `verdict`, `builds`, `pages` (`total`, `building`, and `exact`, false when the total is a lower bound), `blockers`, `groups` (each with its `cause`, `title`, `help`, `pages`, `templates` and every `location`), `notes`, and `verify` (the counts, both timings in seconds, and every file that differs, with its diff), or `null` without `--verify`.
