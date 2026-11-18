---
title: Differences from 11ty
order: 5
summary: What Brrrty does differently from 11ty 3.1.8, what it doesn't do yet, and why.
description: Where Brrrty and 11ty 3.1.8 differ.
lead: Where Brrrty can't match 11ty 3.1.8 yet, and where it stops rather than guess.
---

## It refuses instead of guessing

When Brrrty can't reproduce something exactly, it doesn't write an approximation. It refuses that page with a code, the file and line, and the reason, and builds the rest. The build exits with status 1, so CI notices.

The same goes for the places where 11ty stops the whole build: a data file that won't parse, two pages with one permalink, a loop in computed data, a filter that throws. Brrrty refuses the pages at fault and writes the others. `--strict` stops at the first error, as 11ty does.

## The config is Rhai

Brrrty doesn't run JavaScript, so `eleventy.config.js` becomes `brrrty.config.rhai`; see [Getting started](/docs/getting-started/) and the [configuration reference](/docs/configuration/). Without a Rhai config, Brrrty builds with 11ty's defaults and warns that it isn't reading your JavaScript one. With one, the JavaScript file is ignored.

What isn't there yet:

- `.11ty.js` templates, and JavaScript data files without a `.rhai` port.
- Shortcodes, and plugins other than `rss` and `syntaxhighlight`.
- 11ty's `getCollectionItem`, `getPreviousCollectionItem`, `getNextCollectionItem`, `getCollectionItemIndex`, `inputPathToUrl` and `renderTransforms` filters.
- `dir.data` other than `_data`, and `dir.includes` or `dir.layouts` outside the input folder.
- Passthrough copy with a glob.
- `addWatchTarget`, `setServerOptions` and `serverPassthroughCopyBehavior`.

A plugin your JavaScript config adds and the Rhai port leaves out can't be noticed: Brrrty never sees the JavaScript. Port every `addPlugin` call.

## Filters don't share state

11ty renders pages one after another, so a JavaScript filter can keep a counter or a cache between calls. Brrrty renders pages in parallel, where that could never give the same output, so a Rhai function can't capture outside variables and every argument is a copy. Collection items are read-only for the same reason. The [configuration reference](/docs/configuration/#rhai-is-not-javascript) shows how to pass fixed values in.

## Syntax highlighting has 28 languages

Brrrty's port of Prism has 28 of Prism's 297 languages so far. A fence in one it doesn't have yet is refused rather than left plain.

In 11ty, Prism is shared across the whole build, and loading some languages changes others: `regex` changes `javascript`, `css-extras` changes `css`, and so on, for every page rendered after. Before it builds, Brrrty reads every fence's language and every `{% highlight %}` tag in the input folder, and when one of those languages is named anywhere, it refuses the pages whose highlighting would change, naming the file and line that loads it.

## Time can be pinned

`SOURCE_DATE_EPOCH`, a whole number of seconds since 1970, sets the time for `now()`, Liquid's `'now'` and `'today'`, and feed dates, so two builds of the same source give the same files. Without it, the clock is read as 11ty reads it.

## JavaScript data runs only when you turn it on

`config.set_js_data("node")` runs JavaScript data files in your own Node, so their output is 11ty's. A few things differ from 11ty there, on purpose:

- A data file's `console.log` goes to stderr, not stdout.
- A template or directory data function that returns different data for different templates (a counter, a random value, the time) refuses the pages that use it: 11ty's result would depend on the order it happens to load templates.
- A template or directory data file that gives something other than an object is ignored, as 11ty ignores it, but with a warning.
- A page that reads `collections` on a site with template or directory JavaScript data is refused: 11ty orders those pages by file-read timing, so its own order changes from one build to the next.

## brrrty serve is stricter about the network

It listens on `localhost` only, answers only requests that name this machine, and never serves a file outside the output folder. A build with errors still serves the pages that built. The full list is on the [brrrty serve](/docs/serve/) page.

## Small things

- The summary line is Brrrty's own: `Wrote 9 files, copied 6 files in 0.01 seconds (brrrty 0.0.10)`. Problems print with a code, such as `liquid-unknown-filter`, and a hint.
- Template files that 11ty only reads through a plugin (`.webc`, `.pug`, `.mdx` and others) are skipped with a warning.
- The live-reload script lives at `/.brrrty/` instead of `/.11ty/`.

Every detail, edge case and refusal is in [PORTING.md](https://github.com/brrrty/brrrty/blob/main/crates/brrrty-config/PORTING.md) and the [changelog](/changelog/).
