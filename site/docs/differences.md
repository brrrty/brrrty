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

Brrrty doesn't run your JavaScript config, so `eleventy.config.js` becomes `brrrty.config.rhai`; see [Getting started](/docs/getting-started/) and the [configuration reference](/docs/configuration/). Without a Rhai config, Brrrty builds with 11ty's defaults and warns that it isn't reading your JavaScript one. With one, the JavaScript file is ignored.

What isn't there yet:

- `.11ty.js` templates and JavaScript data files without a `.rhai` port, unless `config.set_js_data("node")` runs them in your own Node.
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

## Random filters can be seeded

A filter that uses `Math.random`, ported with `config.add_random_filter`, gives a different page on every build, in 11ty and in Brrrty, and each such page gets a `random-output` note. Pages that show a random page's output get the note too. `BRRRTY_RANDOM_SEED`, which 11ty doesn't have, makes Brrrty's draws the same on every build with the same seed and the same Brrrty version. The numbers are Brrrty's own, so a seeded page is still one 11ty could have written, not the one a given 11ty run wrote. `brrrty doctor --verify` doesn't compare a random page's file, and says how many it skipped. See [Random filters](/docs/configuration/#random-filters).

## JavaScript data runs only when you turn it on

`config.set_js_data("node")` runs JavaScript data files in your own Node, so their output is 11ty's. A few things differ from 11ty there, on purpose:

- A data file's `console.log`, and anything else it writes to stdout, goes to stderr.
- A process a data file starts and leaves running is stopped when the build ends, unless it started a session of its own (`detached`).
- A getter in the data is read twice, to refuse one whose value changes; one with side effects runs once more than under 11ty.
- A template or directory data function that returns different data for different templates (a counter, a random value, the time) refuses the pages that use it: 11ty's result would depend on the order it happens to load templates.
- A template or directory data file that gives something other than an object is ignored, as 11ty ignores it, but with a warning.
- On a site with JavaScript templates, or template or directory JavaScript data, 11ty adds those templates to its map as their files load, so that order is file-read timing. Each collection is sorted and complete either way and builds, but a page that lists the names in `collections`, and a config collection built with `get_all()`, are refused. (Brrrty 0.0.23 refused every page that read `collections` on a site with template or directory JavaScript data.)
- 11ty runs each JavaScript template's and template or directory data file's module code (and a template's `data()`) as its file finishes loading, in an order of its own, so module state they share (a counter or registry in a helper they import) follows that order. Brrrty can't know it, so it checks two other orders: before writing anything, it loads them again in the reverse of its own order and, in a second Node, in its own order rotated by one, and compares every load and every call. A page that shows a module's place among them (a shared counter read at load) differs in one of the two and stops the build (`js-data-impure`). A dependence that comes out the same in Brrrty's order, its reverse and its rotation goes unseen. Template and directory data files therefore run twice more than under 11ty, even when no page calls a function.
- A JavaScript layout is imported only when a page's data names it (front matter, else template and directory data, else global data) or a layout it uses names it, as in 11ty.
- In a JavaScript template, `this.page` is the first page's `page` on every page of a paginated template, as in 11ty; in a JavaScript layout, reading it refuses the page, since 11ty keeps whichever page rendered the layout first. A filter called on `this` is Brrrty's own; one Brrrty doesn't have, or one that fails, refuses the page. A `render` that gives `undefined` writes no file with a warning, where 11ty says nothing.
- When a page calls a function from the data, Brrrty checks the build by running the data files and every call again, twice, in other orders, before it writes anything. Data files and functions run three times in all, so their side effects (a log line, an uncached fetch) happen three times, and an uncached fetch that gives different data the second time stops the build. (The two extra Nodes start with the first call and load the data while the build goes on; a build that is sent back, as below, runs everything once more.) A function whose results depend on the order pages call it stops the build (`js-data-impure`): 11ty builds pages one after another in its own order and Brrrty in parallel. The check compares three orders (the build's, the reverse of it with calls interleaved, and one page at a time in the order 11ty reaches them, as near as the templates' names tell: computed data and permalinks, then content, then layouts, and the content by template language, Liquid, Nunjucks, HTML, Markdown, then JavaScript templates, each in input order); order-dependence that gives the same results in all three but not in 11ty's exact order (which also follows when 11ty reads each layout file) would go unseen. A helper that resets and counts within each page, which 11ty builds since it renders one page at a time, is refused. The check can be skipped with `--no-order-check` or `config.set_order_check(false)`: the site's JavaScript then runs once, nothing is run again, and the build says the check was skipped. A build like that is faster, and a page whose result depends on the order pages are built in is written as Brrrty's order happens to give it. Changes to the data files' values are still caught. `brrrty doctor` always runs the check.
- `SOURCE_DATE_EPOCH` doesn't pin the time JavaScript data reads in Node, as 11ty doesn't. A call that reads the clock inside a `vm` context stops the build.
- A function that changes a list or object a data file exports stops the build, even where 11ty's output wouldn't show it (an object 11ty had already copied into every page). How Brrrty sees a change depends on the site's code:
  - Where every function that can run is in strict mode, Brrrty freezes those values while the pages' calls run. A change can't happen then, and an attempt at one can't pass unseen: it throws, and Brrrty sees every error Node throws, caught or not (through Node's inspector, from a thread of its own). An attempt sends the build back: it runs once more, with the walk below, which decides. A build that is never sent back made no attempt, and costs nothing per call. ES modules are strict, and so is a CommonJS file that starts with `"use strict"`.
  - A CommonJS file with no `"use strict"` is loaded as strict when that changes nothing in it without an error: it must not use `arguments`, `eval`, `this` as a value inside a function (`return this`, `typeof this`; `this.name` is fine), or a function declared inside a block, and it must compile in strict mode. What strict mode alone turns into an error there (an assignment to a name nothing declares, a write a frozen object drops, a property of `this` in a plain call) sends the build back too, and the next run loads every file as written. Positions are the file's own, in Brrrty's messages and in a stack trace the site's code reads (`new Error().stack`). A formatter of the site's own (`Error.prepareStackTrace`), `fn.caller` and `util.getCallSites()` see what a stack's text doesn't show (a frame's function is hidden in strict code), so each sends the build back as well.
  - Otherwise Brrrty walks every data file's values after each call, as every build did before: with any other CommonJS file that has no `"use strict"` (older libraries often are), `eval`, `new Function` (other than `Function("return this")` or a body that starts with `"use strict"`), `vm`, `v8` or `inspector` however the code came by them, a native addon, module hooks of the site's own, a Node preload, or a Node before 22.15 (which can't show Brrrty every file that loads). A site that throws more than 500 errors while its pages are built, or an error in each of 100 calls in a row, is walked too: Brrrty stops at every error to look at it, which costs more than the walk there. The walk costs what the data weighs, per call: 2,000 pages, each making five calls, next to a 30,000-item list take about 2 minutes that way, against under 2 seconds frozen. `BRRRTY_JS_TRACE=1` says which a build does, and why; `BRRRTY_JS_EXPORTS=walk` asks for the walk.
  - Frozen or walked, a getter's value, a `Proxy`, a `RegExp`'s `lastIndex`, a typed array and JavaScript's built-in prototypes are checked after each call. A change made with a symbol key by plain assignment is seen only by the walk at the end of the build when values are walked, so one undone before then goes unseen; templates can't read symbol keys.
- Which object is which (`===`, a `Map` or `WeakMap` keyed by an object, `indexOf`) is as in 11ty where a JavaScript template or a computed data or permalink function reads its page's data:
  - Each page has its own copy of every plain object in its data, down to the first list: `data.site` is one object within a page and another on the next (`collections.all[0].data.site !== data.site`, unless that item is the page itself), while a list from a global data file, and everything in it, is the same object on every page. `eleventy` and `pkg` are frozen and shared. A list from a JSON template or directory data file, and a `tags` list, belong to one template.
  - A collection item's `data` is the page's data, and its `page` is `data.page`.
  - On a paginated page 11ty wraps the data, and reading a plain object the template's data has gives a new wrapper every time: `data.site !== data.site` there. `page`, the pagination alias, `eleventy`, `pkg` and lists are the same each time.
  - Still different: a computed function that returns an object from its data (`author: (d) => d.site.authors[d.key]`) gives the page a copy, so `data.author !== data.site.authors[data.key]`, where 11ty keeps the same object. On a paginated page, a computed value that front matter also sets is the same object each time it is read, where 11ty wraps it anew. And an object a Nunjucks or Liquid template hands to a function from the data (`{{ h.f(site.author) }}`) is the same object from every page, where 11ty hands each page's own copy; a function that keeps one and compares it on another page, or compares one page's with another's (`h.same(site.author, collections.all[0].data.site.author)`), answers `true` where 11ty answers `false`. None of these can be seen from the outside (no order of pages changes them), so nothing refuses them.
- A function that changes the data or arguments it is given is refused, including a method that caches on `this` (`this._cache ??= ...`). So is one that reads `this` where 11ty hands it its own filters, or that receives data through `structuredClone`: Brrrty watches what the function writes through a wrapper, which `structuredClone` can't copy. Comparing a list from the data with `===` against a list a data module kept for itself (`data.posts === require("./posts")`) gives `false`, where 11ty gives `true`.
- An error thrown by a computed or permalink function refuses that page, where 11ty stops the whole build.

## brrrty serve is stricter about the network

It listens on `localhost` only, answers only requests that name this machine, and never serves a file outside the output folder. A build with errors still serves the pages that built. The full list is on the [brrrty serve](/docs/serve/) page.

## Small things

- The summary line is Brrrty's own: `Wrote 9 files, copied 6 files in 0.01 seconds (brrrty 0.0.10)`. Problems print with a code, such as `liquid-unknown-filter`, and a hint.
- Template files that 11ty only reads through a plugin (`.webc`, `.pug`, `.mdx` and others) are skipped with a warning.
- The live-reload script lives at `/.brrrty/` instead of `/.11ty/`.

Every detail, edge case and refusal is in [PORTING.md](https://github.com/brrrty/brrrty/blob/main/crates/brrrty-config/PORTING.md) and the [changelog](/changelog/).
