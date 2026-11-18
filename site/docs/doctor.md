---
title: brrrty doctor
order: 1.5
summary: Everything that keeps your pages from building, in one list, and proof that the output matches 11ty's.
description: brrrty doctor lists every blocker per page, grouped by cause, and --verify compares the output with your 11ty build file by file.
lead: <code>brrrty doctor</code> looks at your site the way <code>brrrty build</code> does and lists what stops each page from building, not just the first problem. With <code>--verify</code>, it also builds with your own 11ty and checks that every file is byte-identical.
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

A build stops each page at its first problem. Doctor keeps going: for every page that is refused, it also lists what that first problem hid. For that it reads, without rendering them, the page's template, its layouts, the includes they name by a literal name (`{% include "nav.njk" %}`, not one built from a variable), its `permalink` and every string in its `eleventyComputed`, and lists:

- every filter used there that isn't defined, in every branch. A Nunjucks filter in a branch the page never takes is listed too, though only reaching it would stop the page;
- every read of a global data key that a JavaScript data file sets.

Unknown tags (a shortcode, say) are syntax errors to Liquid and Nunjucks: only the first one in each file is listed, as the build reports it. Other front matter values aren't templates in 11ty, so they aren't read. Each problem is grouped under its cause:

- **Your 11ty config isn't fully ported.** While `brrrty.config.rhai` still has a `// TODO(brrrty)` line that `brrrty init` left, it blocks every page and comes first, with its line numbers: each TODO is something your 11ty config does that could change any page, so no page can be said to build. Until those are done, the other groups may be knock-ons of them (a layout not found because the folders weren't ported, say). See [Port the config](/docs/getting-started/#1-port-the-config).
- **A filter** that neither Brrrty nor your `brrrty.config.rhai` defines, such as an 11ty filter you haven't ported yet.
- **A JavaScript data file** a page reads. Brrrty doesn't run JavaScript, so each one is ported to a `.rhai` data file (see [Configuration](/docs/configuration/)).
- **A template language** Brrrty doesn't run, such as `.11ty.js`.
- **Anything else**, by the problem itself: a syntax error, a feature that isn't supported yet.

Each group lists the refused pages of the templates it affects, every file and line where it shows up, and how to fix it. A page can be refused for several causes, so groups can share pages. The blockers that affect the most pages come first.

Pages that only wait on other pages aren't blockers of their own. A page that shows another page's content (a home page listing posts with their `templateContent`) is listed under the blockers of the pages it reads, as "also blocks N pages that use its content". A page that needs every page in `collections` (a sitemap) comes last, under "Waiting on the blockers above".

```text
1. JavaScript data file src/_data/catalog.js (at least 3 pages, 3 templates)
   src/index.njk:5:10: this template reads `catalog`, which src/_data/catalog.js, a JavaScript data file, sets
   src/item.njk: this page uses `catalog` from src/_data/catalog.js, a JavaScript data file
   Fix: Turn on JavaScript data with config.set_js_data("node") in brrrty.config.rhai, which runs it in your own Node (18 or later), or port src/_data/catalog.js to src/_data/catalog.rhai, a Rhai script whose last expression is the data (see PORTING.md).

2. Filter `usd` isn't defined (2 pages, 2 templates)
   src/index.njk:12:31: Error: filter not found: usd
   src/item.njk:8:20: Error: filter not found: usd
   Fix: Port it to brrrty.config.rhai with `config.add_filter("usd", |value| ...)`; PORTING.md says how.

4 of at least 7 pages will build; 2 blockers, grouped below.
```

The last line is the verdict. "At least" means a template paginates data that a blocker stops, so Brrrty can't know how many pages it makes yet: it counts as one until that blocker is fixed. Every group with such a template says "at least" too, and the blockers that stop a pagination come first.

Warnings and notes are listed after the groups, each with its help. They stop nothing. Two of them are about the config: `config-stale` says your 11ty config changed, or 11ty now reads another file, since `brrrty.config.rhai` recorded it as ported, and `config-unverified` says nothing records your 11ty config as ported. Each gives the `// Ported from:` line to put in `brrrty.config.rhai` once your port covers it.

## JavaScript data

When the site has JavaScript data files (`_data/*.js`, `*.11tydata.js`), the report starts with a section about them:

```
JavaScript data: on (brrrty.config.rhai line 4), Node v24.22.3 at /usr/local/bin/node, tz 2026e
  src/_data/links.js           ported: links.rhai is identical, delete links.js
  src/_data/nav.js             ported: nav.rhai differs at data.items[2]
  src/_data/site.js            runs in Node
  src/posts/posts.11tydata.js  runs in Node
```

The first line says whether JavaScript data is on, what turned it on (`brrrty.config.rhai` and its line, `--js-data`, or nothing), and the Node that runs it with its time zone data. When that differs from Brrrty's own (2026b), a note says so: dates the data computes in your zone differ only if its rules changed between the two. With JavaScript data on, doctor runs the files, as a build would, and says so on stderr first.

Each file then has its state: it runs in Node, it throws (the error is grouped below), a `.rhai` port stands in for it and gives the same value (delete the `.js`) or differs at a path, or it waits on JavaScript data. A port is compared only when Node can run the `.js` too.

With JavaScript data off, the section says how many files there are, how many templates wait on them, and both fixes: `config.set_js_data("node")`, with the Node that was found, or a port to Rhai. A file `brrrty init` can translate says so.

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
- **11ty runs with its config file and no other flags,** as `npx @11ty/eleventy` would, apart from the output folder. If your build script passes flags such as `--input` or `--pathprefix`, put them in the config instead, or the two builds won't match.
- **Ctrl-C or SIGTERM** stops 11ty and everything it started, removes the temporary folders and exits with status 130 or 143.
- **Node and 11ty are needed** only for this check. When either is missing, doctor says how to install it.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Every page builds and, with `--verify`, every file is byte-identical. |
| 1 | Something is refused, or with `--verify` some file differs, or 11ty's build failed (the report is still printed). |
| 2 | A usage error: a bad flag, `--verify` off a terminal without `--yes`, or Node or 11ty not installed. |

## JSON

`--json` prints one object with the same findings:

- `schema` (1, raised when a field changes meaning), `brrrty` (the version), `verdict`, `builds`, `blockers`;
- `pages`: `total`, `building`, and `exact`, false when the total is a lower bound;
- `groups`, each with its `cause` (`kind`: `filter`, `js-data`, `template-language`, `problem` or `follows`; an unported config is a `problem` with `code` `config-todo`), `blocker`, `title`, `help`, `pages`, `pages_exact`, `hides_pages`, `templates`, `also_blocks` (`pages`, `pages_exact`, `templates`) and every `location` (`path`, `line`, `column`, `message`);
- `notes`: warnings and notes;
- `jsData`: `mode` (`node` or `off`), `source` (`config`, `flag` or `default`), `node` (`path`, `version`, `tz`, or `null`), and `files`, each with its `path` and `state` (`node`, `error`, `ported`, `ported-identical`, `ported-differs` with `differsAt`, `translatable` or `blocked`);
- `verify`: `null` without `--verify`; `{ "error": ... }` when 11ty's build failed; otherwise the counts, `eleventy_seconds` and `brrrty_seconds`, `byte_compatible`, `verdict`, and every file that differs, with its diff.
