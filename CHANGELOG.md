# Changelog

## 0.1.19 - 2026-11-28

- With `config.add_plugin("htmlbase")` (or 11ty's `HtmlBasePlugin` run natively under `set_js_config("node")`), the `transformWithHtmlBase` filter is now available in Liquid, Nunjucks and on JavaScript templates' `this`. With the default base `/` it returns its content unchanged, as in 11ty. Any other base is refused by name.
- A filter added with `addFilter` in a JS config and called as `this.f()` from a `.11ty.js` template is now refused by name. Before this, it failed with `this.f is not a function` and was misreported as an order-dependent data file.

## 0.1.18 - 2026-11-27

- With `config.add_plugin("htmlbase")` (or 11ty's `HtmlBasePlugin` run natively under `set_js_config("node")`), JavaScript templates (`.11ty.js`) now get `this.htmlBaseUrl` and `this.addPathPrefixToFullUrl`, as in 11ty. Before this, they were only there when the RSS plugin was on, and a template that called them threw.

## 0.1.17 - 2026-11-27

- 11ty's `InputPathToUrlTransformPlugin` is now supported as `config.add_plugin("inputpathtourl")`, and `brrrty init` ports `addPlugin(InputPathToUrlTransformPlugin)` without options to it. With `set_js_config("node")`, 11ty's own plugin added without options runs natively. On pages written to `.html`, a link that names a template's source file (`href="posts/a.md"`, `../b.njk#top`) becomes the URL of that template's first page, keeping its `?query` and `#hash`, as in 11ty. Absolute URLs, fragments and paths that aren't templates are left as written. A link with a malformed `%` escape, or one named like a JavaScript object property (`constructor`), is refused with the link and the cause. The plugin's `extensions` option is refused by name. `transformWithHtmlBase` and the `inputPathToUrl` filter aren't supported yet.

## 0.1.16 - 2026-11-27

- With `set_js_config("node")`, `addPlugin(IdAttributePlugin)` and `addPlugin(HtmlBasePlugin)` from `@11ty/eleventy` with no options now run as Brrrty's native `idattribute` and `htmlbase` plugins. Before this, the plugins' `htmlTransformer` calls were recorded as unsupported and every page was refused. With options, a function of the same name that isn't 11ty's, or (for `HtmlBasePlugin`) a `pathPrefix` other than `/`, the plugin runs as before and the build still refuses. `transformWithHtmlBase` and `InputPathToUrlTransformPlugin` aren't supported yet.

## 0.1.15 - 2026-11-27

- 11ty's `IdAttributePlugin` is now supported as `config.add_plugin("idattribute")`, and `brrrty init` ports `addPlugin(IdAttributePlugin)` without options to it. Headings without an `id` get one from their text (`slugify`, numbered when repeated) on pages written to `.html`, and those pages are printed the way 11ty's HTML transformer prints them. Its options are refused by name. `InputPathToUrlTransformPlugin` isn't supported yet.

## 0.1.14 - 2026-11-27

- With `set_js_config("node")`, plugins added with `addPlugin` now run after 11ty's config merge, as in 11ty. They see `eleventyConfig.pathPrefix` set to the config's `pathPrefix` (default `/`). Before this, it was `undefined`, so `HtmlBasePlugin` threw ``The `baseHref` option is required`` and the whole config failed to load.

## 0.1.13 - 2026-11-27

- 11ty's `HtmlBasePlugin` (also exported as `EleventyHtmlBasePlugin`) is now supported as `config.add_plugin("htmlbase")`, and `brrrty init` ports `addPlugin(HtmlBasePlugin)` to it instead of leaving a TODO. Brrrty's `pathPrefix` is always `/`, so, as in 11ty, the plugin rewrites no URL in a page. It adds the `htmlBaseUrl` and `addPathPrefixToFullUrl` filters to Liquid and Nunjucks. The `baseHref` and `extensions` options are refused by name. 11ty's `IdAttributePlugin` and `InputPathToUrlTransformPlugin` aren't supported yet.

## 0.1.12 - 2026-11-27

- Passthrough copy now takes globs. `config.add_passthrough_copy("src/posts/**/*.{png,jpg}")` copies every matching file to its path minus the input folder, as in 11ty: braces, `**` and `?` work, matching ignores case and includes dotfiles, and a glob that matches nothing is fine. Before this, Brrrty refused the glob. `brrrty init` ports plain glob strings now; negated globs and extglob groups (`@(a|b)`) keep their TODO. A glob with a target (`#{ "src/**/*.png": "img" }`) is still refused, and the error says so.

## 0.1.11 - 2026-11-27

- With `set_js_config("node")`, 11ty's bundle plugin works in Nunjucks. `addBundle(name)` adds the `{% name %}...{% endname %}` paired tag, and `{% getBundle "name" %}` prints what the page collected, code added after the call included. Entries are trimmed, empty ones dropped, kept once each and joined with a newline; buckets can be comma lists, as in 11ty. Once any bundle is added, every `.html` page goes through the same pass as 11ty's html transformer, before the config's transforms. That pass drops an empty `<style>`, a `<script>` with no text and no `src`, and a `<link rel="stylesheet">` with no `href` (`eleventy:keep` keeps one), and re-serializes the page as 11ty does (`<br/>` becomes `<br>`, `disabled` becomes `disabled=""`). Still refused by name: `addBundle` with options, `getBundleFileUrl`, and bundle tags in Liquid.

## 0.1.10 - 2026-11-27

- `eleventyComputed` can now set `layout`. Brrrty used to refuse every page whose computed data set it. Now each page renders with the layout named in its final data, as in 11ty. The computed value may read other computed values, and the layout it names may have a layout of its own. A computed `layout` that is empty or `null` means no layout. The layout's front matter data still comes from the layout named before computing, as in 11ty. A computed layout that names a missing layout, or isn't a name, fails only that page.

## 0.1.9 - 2026-11-27

- `brrrty init` now writes `config.set_js_config("node")` when the 11ty config has something it can't port to Rhai. It no longer leaves blocking TODOs, or gives up on the whole config, for what the JS config runs: filters, shortcodes, transforms, plugins, passthrough copy, ignores, template formats, global data, and the folders and engines the config returns. What Rhai can port is still ported and wins. Collections, preprocessors, events, `amendLibrary` and the like keep their TODOs. At build time, any config method the JS config doesn't run now refuses every page, wherever it's called (a plugin or helper in `node_modules` included). The exceptions are the ones that never change output (watch targets, dev server options, quiet mode, linters), which stay warnings. `brrrty doctor` (text and `--json`) gains a `jsConfig` section: the mode, the config file, each JS function with its kind, name, calls and time (hottest first), and each unsupported method with file and line. When the config throws, `doctor --json` still reports, with the error as the blocker.

## 0.1.8 - 2026-11-26

- `dir.includes` and `dir.layouts` can now lead out of the input folder with leading `..` parts (like `../src`), resolved against the input folder as in 11ty. Before this, Brrrty refused such a site before building anything. A `..` path that leads back into the input folder (`../pages/_includes`) names the same folder as the plain one, so its files are never built as pages, and `eleventy.directories` shows the resolved folders. An absolute path, a `..` after a folder name, and a folder that contains the input folder are still refused, and so is `dir.data` outside the input folder.

## 0.1.7 - 2026-11-26

- With `set_js_config("node")`, plugins added in the 11ty config with `addPlugin` (a function, or an object with `configFunction`) now run, as in 11ty: after the config function returns, in the order added, or at once with `{ immediate: true }`. The filters, passthrough copies and transforms a plugin adds take effect, and a plugin's transforms run after the config's own. Before this, a plugin's transforms were dropped without a message. Brrrty's own stand-ins for the RSS and syntax highlight plugins are not part of this yet.

## 0.1.6 - 2026-11-26

- With `set_js_config("node")`, transforms added in the 11ty config (`addTransform`) now run in Node on every page Brrrty writes, after layouts. They run in 11ty's order: names that look like integers first, then the order they were added, and a name added again keeps its place. Each one gets 11ty's `this` (`inputPath`, `outputPath`, `url`, `page`) and `(content, outputPath)`, and async ones are awaited. A transform that throws or doesn't return text refuses that page, naming the transform and the config file. While the config has transforms, RSS feeds are refused: 11ty also runs the transforms on each feed entry (`renderTransforms`), which Brrrty doesn't do yet. Before this, Brrrty ignored these transforms without saying so.

## 0.1.5 - 2026-11-25

- With `set_js_config("node")`, shortcodes added in the 11ty config (`addShortcode`, `addPairedShortcode`, `addAsyncShortcode`, `addPairedAsyncShortcode`, and the Nunjucks- and Liquid-only forms) now run in Node from Nunjucks and Liquid templates. Nunjucks parses arguments like 11ty, with keyword arguments as a last object. Liquid uses 11ty's argument parsing, with values separated by commas or spaces. A paired shortcode gets its rendered body as the first argument. Return values print as 11ty prints them, including falsy and non-string returns. A shortcode that throws refuses its page, naming the shortcode and the config file and line. Shortcodes that read `this.ctx`/`this.env` refuse, as filters do.

## 0.1.4 - 2026-11-25

- With `set_js_config("node")`, filters added in the 11ty config (`addFilter`, `addAsyncFilter`, `addNunjucksFilter`, `addNunjucksAsyncFilter`, `addLiquidFilter`) now run in Node from Nunjucks and Liquid templates. Async filters are awaited. Inside a filter, `this` has `page` and `eleventy` (with `env.runMode` set to `serve` under `brrrty serve`). Engine-specific filters register only in their own engine, and a Rhai `add_filter` with the same name wins. A filter that throws refuses its page, naming the filter and its file and line. A filter that reads `this.ctx`/`this.env` (Nunjucks) or `this.context`/`this.liquid` (Liquid), or gets a `safe` string, refuses its page with a message naming the filter. Unless `--js-data` or `set_js_data` says otherwise, the Node that loaded the config runs JavaScript data files too.

## 0.1.3 - 2026-11-24

- `config.set_js_config("node")` (or `--js-config=node` on `build`, `serve` and `doctor`; off by default) runs the site's 11ty config file in Node against a recording `eleventyConfig`. Brrrty then applies passthrough copy, ignores, template formats, global data, `dir` and the template engines from it, along with what the config returns and an ESM `export const config`. Lines in `brrrty.config.rhai` win for the same setting. Config files are found in 11ty's order, `.eleventy.js` first. Brrrty doesn't call config-defined filters, shortcodes and transforms yet. Any method it skips gets a warning with the file and line, any returned key it skips gets a warning naming the file, and `brrrty doctor` lists them all.

## 0.1.2 - 2026-11-24

- Brrrty reads `templateFormats` from a config's returned object, plus `config.set_template_formats` and `config.add_template_formats`, and builds only the formats listed, as 11ty does. A file in a format that isn't listed is neither rendered nor copied. As in 11ty, the set call wins over the returned key, an empty string means the defaults, and names are lowercased. A layout named without an extension is looked up in the listed formats, in the listed order. A format Brrrty doesn't build (such as `css` or `webc`) is refused with a config error instead of being ignored. `brrrty init` ports all three instead of leaving a TODO.

## 0.1.1 - 2026-11-23

- A config can name its global data folder with `dir.data` (a folder inside the input folder, like `_includes`). Brrrty reads global data from it, doesn't build it as pages, and sets `eleventy.directories.data` to it. `brrrty init` ports the setting instead of leaving a TODO.

## 0.1.0 - 2026-11-22

The first milestone: the author's own 11ty sites, a blog and data-driven sites with thousands of pages, run on Brrrty with output byte-identical to 11ty's. From here, each minor version tracks how much of a public corpus of 11ty 3.x sites Brrrty builds identically with no hand edits; 1.0 is about 80% of it.

- `brrrty init` gives a `config.htmlTransformer` call (such as `addPosthtmlPlugin`) its own TODO instead of giving up the whole config. Brrrty doesn't run posthtml plugins; the rest of the config still ports.

## 0.0.30 - 2026-11-21

- `brrrty init` ports the rest of a config file whose `addPlugin` call takes its options from a `const` bound to a plain object (it used to give up the whole file). Options that run code, or hold `immediate`, still give it up.

## 0.0.29 - 2026-11-21

- When `brrrty init` can't port a whole config file, its message (and `brrrty doctor`'s `config-todo` blocker) now says which statement stopped it: the file, the line and the code, instead of only naming the config object.

## 0.0.28 - 2026-11-19

- `brrrty build --no-order-check` (and `serve`), or `config.set_order_check(false)`, builds without the order check: the site's JavaScript runs once, and Brrrty does not run it again in other orders to check that no page depends on the order pages are built in. The build says so in one line. The check stays on by default, and `brrrty doctor` always runs it.
- A build in which a function changes the data it is given always ends with that problem. Now and then it ended with "JavaScript data stopped: ... definition N: no kept value M" instead, when another page's request reached Node just before Brrrty heard of the change.

## 0.0.27 - 2026-11-19

- The Linux musl builds (the ones npm installs on Linux) use mimalloc instead of musl's own allocator, which is slow under threads. A 1,070-page site with JavaScript templates builds in 3.03 s instead of 3.26 s; the pages are the same. The glibc and macOS builds are unchanged.

## 0.0.26 - 2026-11-19

- JavaScript data, functions in it and JavaScript templates (`--js-data=node`) build much faster, with the same pages and the same refusals. A 1,070-page site built from `.11ty.js` templates over 15 MB of data went from 31.2 s to 2.6 s (11ty 3.1.8: 2.2 s) and from 3.9 GB to 0.4 GB of memory; 2,000 Nunjucks pages calling data functions beside a 30,000-item JavaScript list went from 130 s to 1.7 s (11ty: 1.5 s); 4,000 pages from one paginated `.11ty.js` went from 62 s to 2.5 s (11ty: 4.0 s). What changed:
  - A list or object that pages share crosses to Node once per build and is referred to after that; before, each request carried the page's data again. Which object is which stays as in 11ty: a JavaScript template or a computed data function sees its page's own copy of each plain object in the data, lists that every page shares, and a paginated page's data as 11ty wraps it (see "Differences from 11ty" for what `===` still answers differently).
  - The two extra Nodes of the replay check start with the first call and load the data while the build goes on, are sent each shared value as it is defined, and run the build's requests again by their recorded text, sent ahead of the replies. The check compares the same replies in the same two orders as before.
  - The render threads send their requests without waiting for each other's replies, and a page's computed data goes in three round trips instead of one per function.
  - Where every function that can run is in strict mode, the values the data files gave are frozen while pages call functions, instead of walked after every call. A build that tries to change one (the attempt throws, and Brrrty sees every error Node throws, caught or not) runs once more with the walk, which decides as before. ES modules and CommonJS files that start with `"use strict"` are strict; a CommonJS file that doesn't is loaded as strict when that changes nothing in it without an error, and an error only strict mode throws sends the build back too. Other sloppy CommonJS, `eval`, `new Function`, `vm`, native addons and Node before 22.15 keep the walk, and so does a site that throws more than 500 errors while its pages are built, or throws in 100 requests in a row (watching an error costs more than throwing it). A stack trace the site's code reads has the file's own columns, and `Error.prepareStackTrace`, `Function.prototype.toString` and the other built-ins Brrrty stands in for look and print as Node's own. See "Differences from 11ty".
  - A template or directory data file whose export isn't a function is asked for once, however many templates it applies to.
  - Data that is already in JavaScript key order stays one shared value when a template's data is put in that order (an object with a number-like key was copied for every template), and a value found in order isn't looked through again for the next template.
- `Object.isFrozen(eleventy)` and `Object.isFrozen(pkg)` are `true` in a JavaScript template or data function, as in 11ty, and two templates never share a list from a JSON directory data file or a `tags` list (11ty reads the file again, and cleans `tags` anew, for each template).
- A computed data function after one that throws no longer runs for that page, as in 11ty.
- `BRRRTY_JS_TRACE=1` says on stderr whether a build froze or walks the data files' values (and why), how many requests and bytes went to Node, and how long the replay check took after the build's own calls. `BRRRTY_JS_EXPORTS=walk` asks for the walk.

## 0.0.25 - 2026-11-19

- `config.add_random_filter(name, fn)` ports a filter that uses `Math.random`, such as a shuffle: inside it, `random()` gives a number in [0, 1), as `Math.random()` does. 11ty writes such a page differently on every build, so any result is one 11ty could have written: each page that calls the filter builds, with one `random-output` note saying 11ty writes it differently on every build. A page that shows a random page's output gets the note too: one that reads its content (`templateContent`, also through another page), and, when a random filter gives a page's data (its permalink or computed data), every page that reads `collections` (conservatively, as Brrrty doesn't track which items it reads). `brrrty doctor` lists the notes and counts the pages as building. `--verify` doesn't compare a random page's file, pairs the files a random permalink puts at different paths, and then says how many files it compared instead of "byte-compatible". A random filter called on `this` from a JavaScript template, permalink or computed function works too; the replay check is handed the filter's first answers. `random()` anywhere else (the script's top level, global data, a collection, a filter added with `add_filter`) is still an error, now naming `add_random_filter`.
- `BRRRTY_RANDOM_SEED` (Brrrty's own; 11ty has no seed) seeds random filters: each page draws from its own stream, so every build with the same seed and the same Brrrty version writes the same pages.
- `config.add_nondeterministic_filter(name)` still refuses the pages that use the filter, and its error now points to `add_random_filter`.

## 0.0.24 - 2026-11-18

- JavaScript templates: with `config.set_js_data("node")` (or `--js-data=node`), `.11ty.js`, `.11ty.cjs` and `.11ty.mjs` templates run in the site's own Node, in the same worker as JavaScript data, and their pages match 11ty 3.1.8 byte for byte. Every export 11ty takes works (a string, `Buffer` or promise; a function; a class with `render` and `data`; an object), in CommonJS and ESM projects, by a port of 11ty's own `_getInstance`, including its quirks (in a CommonJS project, an `.mjs` with `export default` and `export const data` renders empty, as in 11ty). `data` (an object, a getter, an async function) is the page's front matter; `render(data)` gets the page's data, `collections` included, with `this` as 11ty builds it: `this.page` is the first page's on every page of a paginated template, as in 11ty, and `this.slugify`, `this.url`, `this.log` and your config's filters run Brrrty's own filters (the worker asks Brrrty over a pipe of its own and waits; the replay check is handed the same answers). Permalink and computed strings are used as written, as 11ty's JavaScript engine uses them. A JavaScript template can use any layout, and a `.11ty.js` file in the layouts folder can be the layout of any page. Three new fixtures, built by 11ty 3.1.8 on node:22 and node:24, match byte for byte. Without the switch, a JavaScript template is refused as before, now naming it.
- What a JavaScript template does that Brrrty can't follow refuses its page, naming the file (and the line, for an error the code throws): an export 11ty can't make a template of, or `data` that isn't an object (`js-template-invalid`, where 11ty stops the build); `render` giving a number, `null`, an array or an object with no layout (`js-template-value`; 11ty stops the build there too, and with a layout Brrrty prints it as 11ty does); calling one of 11ty's filters Brrrty doesn't have on `this` (`getCollectionItem`, `getBundle`), or one that fails; `this.page` in a JavaScript layout; changing the data `render` is given; `templateEngineOverride` on a JavaScript template; `page.rawInput`. `render` giving `undefined` writes no file, with a `js-template-empty` warning (11ty says nothing). A render is a call: the replay check, the check of the data files' values after each call, and the recorded clock cover it.
- Collections on a site with JavaScript templates, or with template and directory JavaScript data that runs, now build. 11ty adds those templates to its template map once their files load, so that order is file-read timing, but each collection is sorted by date and input path and complete either way. Only what follows that order is refused: listing the names in `collections`, and a config collection built with `get_all()`. Before, every page that read `collections` on such a site was refused. Template and directory JavaScript data that doesn't run (JavaScript off) still refuses them, since it may set `tags`.
- 11ty runs each JavaScript template's and template or directory data file's module code as its file finishes loading, in an order of its own. The replay check now loads them again in two other orders (the reverse, and the build's order rotated by one) and compares every load and call, so a page that shows a module's place among them (a counter in a shared helper) stops the build (`js-data-impure`) instead of following Brrrty's order. A dependence that comes out the same in all three orders goes unseen. Template and directory data files therefore run twice more, even when no page calls a function.
- Only the JavaScript layouts a page's data names first (front matter, else template and directory data, else global data), and the layouts those name, are imported, as in 11ty; an unused or overridden one never runs.
- A permalink function that gives `false` (or `{ build: false }`) for any page of a paginated template now writes none of its pages, as 11ty does, with a `permalink-false-page` warning (11ty says nothing); this also fixes `.11tydata.js` permalink functions, which wrote the other pages. A computed permalink that does it is refused (11ty writes `false` as a path and stops).
- A computed or permalink function's `this` is 11ty's filters, which run Brrrty's own (`permalink(d) { return \`/${this.slugify(d.title)}/\` }`), for templates and data files; `this.tmpl` is refused. Before, any read of `this` there refused the page.
- When the collections can't be built, a JavaScript template or layout that reads `collections` is refused with the reason, where it read `undefined` before. A paginated page's data lists `pagination` first, as 11ty's does (`Object.keys(data)` in JavaScript). A filter a render calls from a timer that outlives it stops the build naming that template.
- A build with many JavaScript-rendered pages keeps its record of calls as text: 4,000 pages from one paginated template went from 1.04 GB to 605 MB.
- A paginated template whose `pagination.alias` is `layout` (each page its own layout, which 11ty follows) is refused, naming the alias; before, every page was written with the template's one layout, or none.
- `fileSlug`, `filePathStem`, the default permalink and the template's own data file (`c.11tydata.json` for `c.11ty.js`) of a JavaScript template take off the whole `.11ty.js`, as 11ty does.
- `brrrty init` writes `config.set_js_data("node");` when the site has JavaScript templates, listing them.
- For contributors: the Docker fixture refresh installs 11ty with `--ignore-scripts`.
- `brrrty doctor` counts a page refused because template or folder JavaScript data doesn't run (JavaScript off) as a blocker, with its fix: turn JavaScript data on, or port those files to `.11tydata.rhai` or `.11tydata.json`. Before, it was filed under "Waiting on the blockers above", so the verdict could read "0 blockers".

## 0.0.23 - 2026-11-18

- Fixed: a global JavaScript data file that changes another file's value while it loads (`require("./a").list.push(2)`) now changes what pages see, as in 11ty; before, pages showed the value as first loaded, with no warning (in 0.0.22 too). A template or directory data file that does it stops the build (`js-data-impure`), since what 11ty's pages see then depends on its load order.
- With JavaScript data on, functions in the data run. A function under `eleventyComputed` (nested and `async` too) runs on the page's data in 11ty's dependency order, found the way 11ty finds it: by running each function once on 11ty's proxy data, which also sets computed keys not set yet to `""`. A `permalink` function, plain or computed, places the page as 11ty does. A Nunjucks template calls a function with arguments (`{{ fmt.money(1234.5) }}`, `{{ i18n.en.label(name) }}`), and Liquid calls one it reads (`{{ site.year }}`) and awaits it, as LiquidJS does. Before, every page that used such a function was refused. The js-data-computed fixture, built by 11ty 3.1.8 on node:22 and node:24, matches byte for byte, and Brrrty's port of 11ty's computed data proxy gives 11ty's own results on 200 generated functions.
- What a function does that Brrrty can't follow refuses the page with the new `js-data-function` problem, naming the function's file and line: reading `this` where 11ty passes its own filters or template scope, reading a key Brrrty can't supply, changing the data it is given, returning a promise to Nunjucks or a value that can't cross, or being printed instead of called. Each call runs once, as in 11ty. Before writing anything, Brrrty loads the data again in two fresh Nodes (handed the clock and random values the first load read) and runs every page's calls again, in reverse page order and in name order; when a result differs, the function keeps state that follows the order of calls, and nothing is written (`js-data-impure`). Caches pass. The current time and random values a call reads (`Date`, `Intl` formatting with no date, `performance.now()`, `process.hrtime()`, `Math.random()`, `crypto`) are the call's own, as under 11ty, and the replays are handed the same values, so luxon's `fromISO` builds. A function that changes a list or object a data file exports (`all.sort()`, `styles.push()`, a property it sets or defines, a getter's value, a built-in prototype) stops the build, as 11ty's templates would show the change. A value saved from the first call that read the clock replays as it was. A function compared with `==`, `===`, `in` or `contains` is itself, as in 11ty; reading its `name` or `length`, or handing it to a filter other than `dump`, `json` and `default`, refuses the page.
- A JavaScript data value holding a function now crosses when templates only call it or leave it out: `dump` and `json` leave a function out as JSON does (the js-data-cjs-default `class` page and the js-data-values `function` page now build, matching 11ty).
- The lists and objects global data gives every page are sent to Node once per build and kept there, so a function's calls don't each copy the site's data (a 3,000-key strings object called 1,500 times: 20.7 s to 0.5 s). Pages share global data objects instead of copying them for each page, which Brrrty's values make safe; in Node, a computed key set inside one gets a copy for that page, as 11ty's merge gives each page its own copy.

## 0.0.22 - 2026-11-18

- JavaScript data also refuses a `Proxy`, an array with properties of its own (`list.total = 5`) and a `Date` with properties of its own, which templates could read but wouldn't cross. A data function that changes the argument 11ty hands it stops the build with `js-worker-failed`, since 11ty merges that object into every page's data. Files opened with `fs.open` or listed with `fs.glob` are watched by `serve` too. `brrrty init` never translates a data file that shares its name with another (`site.js` and `site.mjs`, which one `site.rhai` would replace together) or uses `__proto__`. Doctor says how a port differs: `key order`, `keys`, `length` or `value`.
- Docs: JavaScript data in the configuration page (the opt-in, which Node, what crosses, logging, errors, `serve`, security), in getting started, in the differences from 11ty, and in PORTING.md (checking a port with doctor).
- `brrrty doctor` starts with a JavaScript data section when the site has JavaScript data files: whether it's on and what turned it on, the Node and its time zone data (with a note when that isn't Brrrty's 2026b), and each file's state: runs in Node, throws, ported (its `.rhai` is identical, so delete the `.js`, or differs at a path, compared by running both), translatable by `init`, or waiting. Off, it says how many templates wait on the files and names both fixes. With JavaScript data on, doctor says on stderr that it runs the files before it does. `--json` has the same as `jsData`.
- `brrrty init` sorts the site's JavaScript data files into three groups and says which is which. A file whose export is a literal (`module.exports = { ... }`, `export default [...]`, or a function that only returns one) is translated to a `.rhai` file beside it, never over an existing one, when a Rhai map keeps its key order; delete the `.js` once `brrrty doctor` says the port is identical. A file that only reads files, parses JSON or CSV, joins paths, makes dates or compares strings is listed with the Brrrty helpers that would port it. Anything else (other packages, `fetch`, `process.env`, `child_process`) needs Node. When any file is left in JavaScript, `init` writes `config.set_js_data("node");` with a comment listing them, so they keep running in the site's own Node.
- With template or directory JavaScript data, a page that reads `collections` stays refused, and now says why in 11ty's terms: 11ty orders those pages by file-read timing, so its own collections change order from one build to the next (measured: 5 of 30 generated sites gave two orders over 20 runs of 11ty 3.1.8). Global JavaScript data files don't affect the order.
- `brrrty serve` runs JavaScript data again, in a fresh Node, on every rebuild, so nothing a data file computed survives a change. Every file the data read or loaded inside the project is watched, wherever it is (a CSV outside the input folder, a helper module, a package under `node_modules`), and so is `package-lock.json`. On Node older than 22.15, which can't report the ES modules it loads, any change under `node_modules` or to any project `.js`, `.cjs` or `.mjs` file counts. No Node outlives a build.
- A JavaScript data file that throws is reported where the error happened, with the source line and a caret: the first stack frame inside the project and outside `node_modules` (so an error inside a package is shown where your file calls it, and one in your own helper module in that module, naming the data file being loaded), or the line of a `SyntaxError`, for CommonJS and ES modules alike. The help lists the stack's frames, with project paths relative. A thrown non-Error value reads `Uncaught "x"`.
- With JavaScript data on, template and directory data files (`x.11tydata.js`, `posts/posts.11tydata.mjs`) run too. As in 11ty, a file exporting a function is called once for every template it applies to; when two calls give different data (a counter, a random value, the time), 11ty's result would depend on the order it loads templates, so the pages that use the file are refused with `js-data-impure`, naming two of them. A file that gives something other than an object is ignored, as 11ty ignores it, with a `js-data-not-object` warning (11ty says nothing). `tags` is cleaned as for JSON files.
- With JavaScript data on, every `_data/*.js`, `.cjs` and `.mjs` file runs in Node, one at a time in 11ty's order, and its value takes the place 11ty gives it. An exported function is called (and awaited) with what 11ty hands it: the config's global data and `eleventy`, frozen. A page that reads a key whose value can't cross exactly (a `Map`, a luxon `DateTime`, a function read as a value, ...) is refused with `js-data-value`, naming the file and the path inside it (`data.posts[3].date`); other pages build. A file that throws is `js-data-error` and refuses the pages that read its key; one that exits Node stops the build with `js-worker-failed`. A `.rhai` port beside a file still replaces it. With JavaScript data on and no Node, `js-data-node-missing` says what was tried and the pages that need the data are refused, as before. The hint on a refused page now names both fixes: `config.set_js_data("node")`, or a Rhai port.
- `config.set_js_data("node")` in `brrrty.config.rhai` turns on JavaScript data files, run in the site's own Node; `config.set_js_data("off")`, or no call, keeps them off, as before. Any other value is `config-js-data-invalid`, at its line. `--js-data=node` or `--js-data=off` on `build`, `serve` and `doctor` overrides the config for one run. The Node is `BRRRTY_NODE` when set, else `node` on `PATH`, version 18 or later. The npm package's launcher sets `BRRRTY_NODE` to the Node running it, unless it is set already.
- `brrrty-node` runs the data in one Node process per build, started with the worker module passed on the command line (nothing is written to disk). Requests go one at a time, in build order; replies come back in framed messages on a pipe of their own, and the site's own logging (`console.log`, `process.stdout.write`, and the output of processes a data file starts) goes to stderr, where Brrrty passes it through. A process a data file leaves running is stopped with the build unless it started a session of its own, and data nested more than 1000 levels deep is refused. A data file that takes more than 10 seconds gets a note naming it (then every 30 seconds) and is waited for, as 11ty waits. A Node that exits or writes something else to its output stops the build, with its exit status and the last 20 lines of its stderr. The worker records every file the data's code reads or loads.
- New crate `brrrty-node`: runs a site's JavaScript data files in the site's own Node. Its value encoding carries plain objects (in JavaScript key order), arrays, strings, numbers (`-0`, `NaN` and infinities included), `null`, `undefined` and `Date`s exactly, keeps functions as handles, and refuses everything else (a `Map`, a class instance such as a luxon `DateTime`, a `BigInt`, an invalid `Date`, a circular reference, a getter that changes between reads) naming the value's path.
- For contributors: `cargo xtask fixtures-refresh --docker <name>` builds a fixture's reference output with 11ty 3.1.8 inside Docker, once per `--image` (`node:22` and `node:24` by default), with no network, and keeps it only when every image gives the same bytes.

## 0.0.21 - 2026-11-18

- `brrrty init` follows the config object instead of giving up on the whole file when it goes somewhere else. It follows helper functions it's handed to, in the config file or in the site's own modules (`require("./x")`, `import`), local plugins (`addPlugin(require("./plugin.js"))`, `addPlugin(localFunction, options)`), which it ports after the config function as 11ty runs them, and loops (`forEach`, `for ... of`, `for ... in`) over literal lists and objects or over a module's exports (`Object.keys(filters).forEach(...)`, `Object.entries(namespace)`). What they set is ported where it can be; a filter or shortcode a loop adds gets a TODO that quotes its definition, in whichever file. An `if` that uses the config object is one TODO naming its condition, rather than a refusal of the whole file; one that sets a folder or template engine still refuses it. The rest of a helper after a possible early `return`, and a loop over something `init` can't read, are one TODO each. A value is ported only when nothing changes it afterwards (`push`, an assignment, being handed to a function). Code in a module that runs when it loads is a TODO, and so is a module's helper that a TODO's code calls. Each module read is recorded on an `// Also ported from:` line with its hash, and `build` and `doctor` warn (`config-stale`) when it changes. A package's function handed the config object still makes the whole file one TODO, as it could change any setting. So does a value worked out on the way that uses the config object: a helper's argument, a plugin's options, a default parameter or a loop's list (`h(c, c.setInputDirectory("x"))`). Such a value that runs other code (`h(c, fs.rmSync(...))`) gets a TODO on its statement, and a loop over it is one TODO. Taking one part of a list or object (`[fs.rmSync(...), "a"][1]`, `{ x: run(), y: "a" }.y`), or a key repeated after code, counts as running that code. Top-level code in the config file that runs when it loads is a TODO that says so, and a declaration's TODO no longer suggests dropping it. Modules that require each other aren't followed into each other, as one of them sees the other half loaded.
- A config that adds the `rss` plugin (feed or filters) is refused when the site installs a version of `@11ty/eleventy-plugin-rss` other than 3 (from `package-lock.json`, `node_modules` or `package.json`), as its output could differ. `brrrty init` records the version it saw on a `// RSS plugin: 3.1.0` line, and `build` and `doctor` warn (`config-stale`) when it changes.
- A feed's `input_path` is refused inside the configured includes or layouts folder, rather than inside `_includes` whatever the config says. The syntax highlighting plugin's grammar check and `brrrty serve`'s stylesheet-only reload use the configured folders too.
- `brrrty init` ports `addPlugin(pluginRss)` (the RSS plugin's default export, or `rssPlugin`) to `config.add_plugin("rss", #{ feed: false })` when the site installs version 3 of the plugin (from `package-lock.json`, the installed package or `package.json`). Version 2's `dateToRfc822` writes the time zone differently, so for version 2, or an unknown version, it stays a TODO that says why. `require("@11ty/eleventy-plugin-rss")` of version 3 gets a TODO saying 11ty stops there: the plugin is an ES module, so `require()` gives its namespace, which 11ty refuses.
- `brrrty init` ports `addLayoutAlias`, `dir.includes` and `dir.layouts` (and `setIncludesDirectory`, `setLayoutsDirectory`), now that builds read them. A plugin that sets a folder gets a TODO, as 11ty stops there.
- `dir.includes` and `dir.layouts` can name folders other than `_includes` inside the input folder, as in 11ty: includes and layouts are looked up there, the folders aren't built as pages (and `_includes` is, when it's no longer the includes folder), and `eleventy.directories` shows them. `config.add_layout_alias(name, file)` is 11ty's `addLayoutAlias`. Both were refused before. `dir.data` still has to be `_data`. The layouts-dir fixture is built by real 11ty and matches byte for byte.
- `brrrty init` reads a `/` inside a template literal's `${}` (`` `${s.replace(/x/g, "")}` ``, `` `${a / b}` ``) and non-ASCII characters inside a regular expression (`/[éà]/`). Each made it give up on the whole config before. A `/` it can't tell apart (a regular expression or a division) is still never guessed.
- `brrrty init` ports a CommonJS config that sets `module.exports.config = { ... }` after `module.exports = function ...`, as 11ty reads it. It gave up on the whole file, saying the file mixed `module.exports` with `export`. That message is now only for a file that really does.
- `config.add_plugin("rss", #{ feed: false })` adds the RSS plugin's filters without a feed: `dateToRfc3339`, `dateToRfc822`, `getNewestCollectionItemDate`, `absoluteUrl` and `htmlToAbsoluteUrls` in Nunjucks, and `htmlBaseUrl`, `addPathPrefixToFullUrl` and the `eleventyFeed*` filters everywhere. It is 11ty's `addPlugin(pluginRss)`, the plugin's default export, for a site with a feed template of its own. Before, the filters came only with a feed.

## 0.0.20 - 2026-11-18

- Brrrty formats locale dates (Liquid's `%c`, `%x` and `%X`, which use `toLocaleString()`) for en-US. Two things now get a TODO in front of the 11ty command, since init didn't carry them and carrying the locale wouldn't help:
  - an `LC_ALL`, `LC_MESSAGES` or `LANG` that doesn't make Node's default locale exactly en-US. That includes en_GB (`15/03/2024`), en_CA, en_AU and an empty value, which Node reads as `und` (`%B` gives `M03`);
  - a `NODE_PATH`.
- `brrrty build`, `doctor` and `serve` warn (`locale-differs`) when any of `LC_ALL`, `LC_MESSAGES` and `LANG` is set (empty counts) to something other than en_US, C or POSIX, since 11ty run there would format dates for it. `LC_TIME` and `LANGUAGE` don't set Node's default locale and are ignored.
- A script that runs 11ty gets a TODO when another step changes shell state that 11ty's step would inherit:
  - its folder: `cd` (other than to `.`), `pushd`, `popd`;
  - an environment variable that can change what 11ty writes, through `export`, `unset`, `declare`/`typeset`/`readonly`/`local`, a bare `NAME=value`, or `set -a`. Those variables are `TZ`, the locale (`LANG`, `LC_*`), `PATH`, `NODE_PATH`, `NODE_OPTIONS`, any variable the 11ty config reads through `process.env`, and any name computed at run time;
  - anything past reading: `source`, `.`, `exec`, `alias`, `shopt`, `hash -p`, a shell function, a command named by a variable, or an `eval` of any of these.

  Builtins that change nothing 11ty sees don't count: `set -e`, `umask`, `ulimit`, `trap`, or exporting a variable nothing reads. So does an 11ty step with `$(…)`, backticks or a `$VARIABLE` in it, and one run through `node` with an option other than a preload (`--env-file`, `--max-old-space-size`, `-C`). Other commands run in their own process and can't change 11ty's, so `sass … && eleventy`, `tailwindcss … && eleventy`, `node scripts/prep.js && eleventy` and the like are read as before. The same builtins on an earlier line of a CI job that runs 11ty, or a `working-directory:` other than `.`, get a TODO too. A JavaScript file that spawns 11ty is read only when:
  - it never writes `process.env`, calls `process.chdir`, or uses `fork`/`execArgv`;
  - each spawn's options are absent or an inline object with known keys: `stdio`, `encoding`, `shell: false`, or an `env` that only adds `ELEVENTY_*` to `...process.env`.

  A `.npmrc` `node-options` setting gets a TODO in a site that runs 11ty.
- `brrrty init` keeps a build script's `TZ` when it rewrites it, in `brrrty build` and in `brrrty serve`. It now rewrites a script with other commands beside 11ty too. It replaces only the 11ty step and keeps the rest in order, so `sass a b && eleventy` becomes `sass a b && brrrty build`. `"build": "TZ=Asia/Tokyo eleventy"` becomes `"TZ=Asia/Tokyo brrrty build"`, through `cross-env` when the original used it; `env TZ=…` is read too. Only `TZ` is carried, since Brrrty isn't Node. In 0.0.19 the `TZ` was dropped, so the dates on the pages could differ from 11ty's with no warning. A `TZ` that Brrrty's build doesn't get is a TODO: one in a deploy command, a CI job's `variables:` or `env:`, or a JavaScript spawn's `env:` (anything but `ELEVENTY_*` variables). A comparison run counts as one only when its Brrrty build has the same `TZ`.
- A preload runs code inside 11ty: `node -r`, `--require`, `--import`, `--loader`, `NODE_OPTIONS`, or `--require` in a JavaScript run list. It's now a TODO that names the file, except in a comparison run, which is never ported. So is a JavaScript spawn whose `cwd:` isn't the site's folder.
- A run of 11ty that only adds `--output` or `--dryrun` to the build's flags, such as `npm run build:11ty -- --output=_site-11ty` in CI to compare with Brrrty's output, is a comparison run. It's no longer a conflict, and it gets no TODO; before, it refused every page. This needs two things. The run must be in a CI file or a package.json script other than `build`; Netlify's and Vercel's commands are always the deploy build. The real build must also be Brrrty's: `"build": "brrrty build"`, a Brrrty build in the same CI job, or a Brrrty build in the same script (`brrrty build && eleventy --output=_site-11ty`). `brrrty init` decides this from `package.json` as it was before init set `build` to `brrrty build`. With Brrrty as the build, a comparison run's other flags are the ones 11ty builds with. Otherwise `--output` counts like any other flag. A CI-only 11ty build's `--output` (`public` for GitLab Pages) is ported. Two 11ty runs that differ only in their output, such as a `build: "eleventy"` script and a GitLab Pages job with `--output=public`, give a TODO that names both outputs and both places, and asks you to set `dir.output` to the one you deploy.
- A file a script runs that starts 11ty's command line (`execFileSync("node", ["node_modules/@11ty/eleventy/cmd.cjs", ...])`, `spawn("npx", ["@11ty/eleventy", ...])`, `execSync("npx @11ty/eleventy ...")`) no longer counts as using 11ty's JavaScript API. Its arguments are read from the file's string literals and handled like a script's, but only from an array passed straight to the call, or from a `const` array that is never changed afterwards. That includes an array inside a `const` list of objects that a loop spawns, such as `const tools = [{ cmd, args: [...] }]` with `spawnSync(tool.cmd, tool.args)`, but only when the list is looped over (or destructured into element names) and each element's `args` is passed whole as `spawnSync(tool.cmd, tool.args)`, or as `spawnSync(cmd, args)` with names the loop destructures. Any other use of the list or its `args` gets a TODO: an alias, a spread into a new array, `forEach`, or passing it to another function. Other properties, such as `tool.name` and `tool.times.push(...)`, may be read and written. A `const` argument array is likewise trusted only when every use passes it whole to a process call. A check such as `fs.existsSync("node_modules/.bin/eleventy")` isn't a spawn. An output folder set at run time (`` `--output=${OUT}` ``) is named that way in messages. When the program or the arguments come from anything else, the TODO says that the file starts 11ty's command line with arguments Brrrty couldn't read. That covers `path.join(...)`, an array that is pushed to, and a command joined with `+`.
- Messages name where the flags come from: `.gitlab-ci.yml, line 8 → ci/compare.mjs`, `package.json, scripts.build`, `netlify.toml, [build] command`. They no longer say "package.json's scripts" for flags package.json doesn't pass.
- A `// 11ty flags from package.json:` hash recorded by 0.0.19 stays valid: Brrrty also accepts the hash 0.0.19 would compute, so these changes raise no `config-stale` warning on their own.

## 0.0.19 - 2026-11-17

- `brrrty build` and `brrrty doctor` no longer treat a config `brrrty init` couldn't fully port as if it were ported. `init` now marks each statement it couldn't port with `// TODO(brrrty):`, and a config it couldn't port at all with `// TODO(brrrty-file):`. While one is left, every page is refused with `config-todo`, which names the lines, and nothing is written or copied; `doctor` lists it first, as a blocker of every page, and its verdict can't say the pages build. Before, Brrrty built with what was ported (11ty's default folders, when nothing was) and doctor said the pages would build, though an unported transform, Markdown library, template format or folder made them differ from 11ty's. Delete a marker once you've ported it, or decided it changes nothing Brrrty writes. A config written by an older `init` counts its `// TODO:` lines (with the `// See https://...` line under them) the same way.
- `brrrty init` records the 11ty config it read on a `// Ported from: eleventy.config.js sha256:...` line, and `build` and `doctor` warn (`config-stale`) when that file changes afterwards, giving the new hash to put there once the change is ported.
- `brrrty init` notes settings for 11ty's watcher and dev server (`addWatchTarget`, `setServerOptions`, `setQuietMode` and the like) as left out instead of as TODOs: they change nothing 11ty writes. One whose arguments run code is still a TODO, and so is an import or `require` of the site's own files (or a bare `import "x"`), whose code runs when 11ty loads the config.
- `brrrty init` reads the flags `package.json`'s build script passes to 11ty, which win over the config: `--input` (a folder) and `--output` become `dir.input` and `dir.output`, `--config=file.js` is the file ported, and `--pathprefix`, `--formats`, `--to`, `--dryrun`, `--incremental` and unknown arguments are TODOs that quote the script. It ignored them, so a site built with `eleventy --input=src` was built from the wrong folder with no warning.
- With an 11ty config next to a `brrrty.config.rhai` that has no `// Ported from:` line (one written by hand, or before the 11ty config was added), `build` and `doctor` warn (`config-unverified`) and give the line to add. `config-stale` also warns when 11ty would read another file than the one recorded (a rename to `.mjs`, or a new `.eleventy.js`), and the hash ignores line endings and a BOM.
- `brrrty init` reads the 11ty command however a script spells it: `npx eleventy`, `pnpm exec eleventy`, `yarn eleventy`, `bunx`, `npm exec`, `node .../@11ty/eleventy/cmd.cjs`, with redirections (`> log.txt`, `2>&1`) and subshells. A script that mentions 11ty in a way it can't read, and one that runs a file that imports `@11ty/eleventy`, gets a TODO naming the script instead of nothing. It records a hash of what the scripts pass to 11ty (`// 11ty flags from package.json:`), and `build` and `doctor` warn (`config-stale`) when that changes, when `--config` changes which file 11ty reads, or when 11ty reads no config file any more. `--config=` (no file) no longer warns that the default config is unverified, and a hand port of a config only `--config` names does. `bun run eleventy` is read; a file a script runs is found as Node finds it (`scripts/b` is `scripts/b.js`, a folder its `index`), run directly through its shebang, and followed through the site's own modules it loads, to see whether it uses 11ty's JavaScript API; and a site's own `build` script that doesn't run 11ty while `@11ty/eleventy` is a dependency is a TODO. Deploy configs are read the same way: `netlify.toml` `[build]` and `[context.production]` `command`, `vercel.json` `buildCommand`, GitHub Actions `run:` steps and GitLab CI `script:` lists; where they and the build script disagree on flags it is a TODO, and a change to any of them is `config-stale`. In CI files, a command continued on the next line and a quoted key are read, and any other line that mentions 11ty but isn't read as a command (an anchor, a `!reference`, a flow-style step) is a TODO naming the line; comments and `name:` lines don't count. Flag order no longer counts as a change, and an absolute `--config` path inside the site is that file. Once `@11ty/eleventy` is no longer a dependency and no 11ty config is left, the site has moved off 11ty and the checks say nothing.
- `brrrty doctor`'s text report shows each warning's help. `brrrty serve` shows a page naming `config-todo` while markers are left, instead of a bare 404. TODO markers match in any case and spacing (`// todo (brrrty):`), and after `//!`, `/*` or code.

## 0.0.18 - 2026-11-17

- A `config.add_*` or `config.ignores` call while pages render (from a filter, a collection or a global data function handed `config`) is ignored, as 11ty ignores it, and now comes with a warning (`config-late-call`), once for each place in the script that makes one.
- markdown-it renderer rules in Rhai. `config.amend_library("md", |md| ...)` is 11ty's `amendLibrary`, and its rules apply to every `.md` page and Markdown layout; `markdown_it(options)` makes an instance for data files and filters, rendered with `md.render(s)`. A rule is `md.renderer.rules.link_open = |tokens, idx, options, env, renderer| ...`, with tokens' `attr_get`, `attr_set`, `attr_join`, `attr_index`, `attr_push` and fields, `renderer.render_token`, `renderer.render_attrs` and `escape_html`; markdown-it's own rules (`md.renderer.rules.text`, `image`, ...) can be read and wrapped. Rules run for inline tokens, which Brrrty gives exactly as markdown-it does (checked against markdown-it 14.3.3 and 15.0.3 on generated inputs, with and without rules). A rule for a block token, reading `env` on 11ty's library, changing the token list or the options, or returning something that isn't a string is an error that names it, and a page whose rule fails is refused with `markdown-rule-failed`. `config.set_library` is an error that says it isn't ported. Rules can only be set while the config or a data file loads: changing one from a filter, a collection or a rule is an error, so output never depends on the order pages render in (an instance a filter makes for itself can still get rules). A token's type can't become a block type, and `md.render` takes only a string, as markdown-it's does. `render_token` follows markdown-it 14.3.3 on 11ty's library and 15.0.3 on `markdown_it()` instances.

## 0.0.17 - 2026-11-17

- `markdown(s, options)` in Rhai is `markdownIt({ html, linkify, typographer })` with `md.linkify.set({ fuzzyLink })`: `markdown(s, #{ linkify: true, typographer: true, fuzzy_link: true })`. Linkify is linkify-it 6.1.0, run on its own patterns, and the typographer is markdown-it 15's replacements and smart quotes; both match markdown-it on 40,000 generated inputs with each set of options. A paragraph with a run of 1,000 or more characters and no space (with linkify), and a quote right next to an image whose label can't be found in the source (with the typographer), are errors instead of guesses. Unknown options are errors.
- Markdown links normalize as markdown-it's do: a host outside ASCII is punycoded (`[x](http://é.com)` links to `http://xn--9ca.com`), an autolink's text is decoded (`<http://x.y/%C3%A9>` shows `http://x.y/é`), and a URL is parsed with mdurl first (`HTTP://~:8080` gives `HTTP://:8080~`). Brrrty only percent-encoded them.
- Markdown emphasis next to a symbol follows markdown-it: `_a_😀` is `<em>a</em>😀`, in 11ty's Markdown and in `markdown()`. Each instance uses its own markdown-it's Unicode tables (uc.micro 2.1.0 for 11ty's 14.3.3, 3.0.0 for 15).
- Code spans follow each markdown-it version's rule: in ``[a`b`c` `` the first two backticks are a code span, as in markdown-it. A backslash before a space leaves the space to the line break after it, so `a\` then a space at the end of a line gives `a\` and a soft break.
- Strikethrough follows markdown-it with three or more `~` in a row: `~~a~~~` is `<s>a</s>~` and `a ~~~~---~~` is `a ~~<s>---</s>`. Emphasis or strikethrough across a CRLF line break inside a list item or a blockquote no longer panics in a debug build (it refused the page with `markdown-render-failed`).
- Emphasis and strikethrough are paired as markdown-it pairs them (its delimiter list, `balance_pairs`, and post-processing). The port's own matching left some markers literal: `*Set `DEBUG` in settings_* then _restart_.` now has `<em>restart</em>`, and `*b~~*~~x~~` has `<s>x</s>`.
- In 11ty's Markdown, link URLs encode an IPv6 host's brackets, as markdown-it 14.3.3 does (`[x](http://[::1]:8080/)` links to `http://%5B::1%5D:8080/`); `markdown()` keeps them, as 15 does.
- Image `alt` text is markdown-it's: entities, escapes, line breaks, autolinks and inline HTML count, and code spans count in 15 but not in 11ty's 14.3.3 (`![Output of `ls -la`](a.png)` has `alt="Output of "` on an 11ty page). An image's label is parsed as its own text, as markdown-it does.
- A `<pre>`, `<script>`, `<style>`, `<textarea>` or comment in a list item keeps everything up to its closing tag across a blank line, as in markdown-it; Brrrty ended it at the blank line and wrote the rest as paragraphs.
- In a tight list item, a fence, an indented code block or an HTML block right after the item's text follows it with no line break, as markdown-it writes it. A last line of only spaces with no newline after it is ignored, as markdown-it ignores it.
- Tables follow markdown-it's `table` rule: a one-column table (`| Name |` then `---`), a header with an escaped `\|`, and the lines that end a table.
- `markdown(s, options)` with `linkify` or `typographer` no longer refuses a tight list item that holds a block (a fence, a heading, a quote).
- Markdown that Brrrty's parser is known to read differently from markdown-it is refused (an 11ty page with `markdown-render-failed`, a `markdown()` call with an error) instead of written: a reference definition whose destination ends in `\`, a list marker alone on a blockquote's lazy line, a table right after a blockquote's paragraph, a code span or inline HTML across an indented lazy line in a blockquote, and a tab after a blockquote's `>` where the parser counts its columns differently (in HTML blocks, fenced and indented code, and nested list items). The checks look at the parsed blocks, so code blocks, HTML blocks and link titles are never refused. PORTING.md lists them.
- In 11ty's Markdown, inline HTML follows markdown-it 14.3.3: `<!-->`, `<!--->` and a comment with `--` inside are HTML, and after a comment, processing instruction, declaration or CDATA section that is never closed, no more are taken in the same paragraph. A fence or HTML block that runs to the end of a file with no final newline no longer gets one added.

## 0.0.16 - 2026-11-17

- In `brrrty.config.rhai`, a constant number inside `${...}` prints as JavaScript prints it, as a variable already did: `${10.0}` and `const PER_PAGE = 10.0; `${PER_PAGE}`` give `10`, not `10.0`, and `${1e21}` gives `1e+21`. Rhai's optimizer folded constants into the string with Rhai's own number formatting; it is now off for config and data scripts.
- Rhai scripts get JavaScript's number formatting and string order, each checked against Node: `to_fixed(n, digits)` is V8's `toFixed`, rounding the number's exact value with ties away from zero (`to_fixed(1.005, 2)` is `"1.00"`, `to_fixed(2.5, 0)` is `"3"`); `to_locale_string(n)` is `toLocaleString('en-US')` (`1,234.568`); `locale_compare(a, b)` is `localeCompare` with the `en-US` locale. Other locales and options are refused by name, and so are Han characters, which Node sorts by radical and stroke count.
- Rhai scripts can build local dates as `new Date(y, m, d, h, mi, s, ms)` does: `date(2026, 4, 10)`, with `get_full_year()`, `get_month()`, `get_date()`, `get_day()`, `get_hours()`, `get_minutes()`, `get_seconds()`, `get_milliseconds()`, `get_timezone_offset()` and `to_iso_string()`.
- `random()` in a Rhai script is an error that explains why: a random value can never match 11ty's output. `config.add_nondeterministic_filter(name)` stays the way to stand in for a filter that shuffles.
- In Nunjucks templates, a number's `toLocaleString()` and a string's `localeCompare()` run as in Node with the `en-US` locale; another locale or options refuse the page.
- With no locale argument, `toLocaleString()` and `localeCompare()` (and `to_locale_string`, `locale_compare`) run only when Node's default locale would be `en-US`: the first of `LC_ALL`, `LC_MESSAGES` and `LANG` that is one Node maps to `en-US` (`en_US`, `C` or `POSIX`, with any codeset and no `@modifier`), or none is set. Otherwise they refuse, naming the variable.
- Local time uses the tz database Node 26 ships (2026b), built into Brrrty, instead of the computer's: with newer data on the machine (2026c makes Alberta UTC-6 from November 2026), local dates in a changed zone were an hour off. Luxon's `luxon_format` uses the same data, so zones with daylight saving work after 2099 instead of being refused.
- The local zone is found as Node's ICU finds it: `TZ`, else where `/etc/localtime` links to, else (a copied `/etc/localtime`, as in many Docker images) the zone file under `/usr/share/zoneinfo` with the same bytes, else UTC when there is none. A `TZ` that names no zone in the bundled data (a typo, a trailing space, a POSIX rule), a broken link, or a copy that matches no zone file stops the build with `tz-invalid`, where Node would quietly use UTC or guess.
- `String(n)` now prints the even digits when a number is exactly halfway between two shortest forms, as V8 does: `9302184022144.5625` prints `9302184022144.562`, not `...563`. Numbers in templates and Rhai print this way too.
- Local dates past the year 250,000 or before -250,000 use the time zone's offset (they used UTC's), so their hours and days match Node out to the end of JavaScript's date range.

## 0.0.15 - 2026-11-17

- Data files and the config can read files in the project: `read_text(path)` is `fs.readFileSync(path, 'utf8')`, `read_json(path)` is `JSON.parse` of it, `read_dir(path)` and `read_dir(path, #{ recursive: true })` are `fs.readdirSync` in Node's order, `file_exists(path)` is `fs.existsSync`, and `parse_csv(text, options)` is csv-parse 7.0.3's `parse`, quirks and error messages included, for the options `columns`, `delimiter`, `bom`, `trim`, `ltrim`, `rtrim`, `skip_empty_lines` and `relax_column_count`. Other csv-parse options are refused by name.
- Paths are relative to the project folder and can't leave it: an absolute path and a `..` are refused, and every path is opened through a handle on the project folder one component at a time (cap-std), so a symlink that leads outside is refused even if it is swapped in mid-build. Recursive listings enter symlinked folders as Node 26 does, refusing one that leads outside or loops. Only regular files are read (a FIFO is refused, not waited on), a file over 16 MiB is refused unread, and nothing writes.
- `brrrty serve` rebuilds when a file a script read changes, appears or goes away, including outside the input folder and under `node_modules`, when a folder a script listed gains or loses a file, and when a symlink on the way to a read is retargeted.
## 0.0.14 - 2026-11-17

- `brrrty doctor` lists everything that keeps pages from building, without writing anything. A build stops each page at its first problem; doctor keeps going and lists them all: every filter that a page's template, its layouts, the includes they name literally, its `permalink` and its `eleventyComputed` use that isn't defined (in every branch), every JavaScript data file they read, a page's own problem that a layout Brrrty can't render hid, and template languages Brrrty doesn't run. Problems are grouped by cause, with the pages and templates each one refuses, every file and line, and a fix. A page refused only because it shows the content of a refused page is listed under that page's blockers ("also blocks N pages that use its content"). A cause that stops a template's pagination before it makes its pages comes first, and every count that includes such a template says "at least". Unknown tags (shortcodes) are listed one per file, as the build reports them. The verdict reads `N of M pages will build; K blockers, grouped below.` `--json` prints the same as one object. Exit status 0 when every page builds, 1 when something is refused, 2 for usage errors.
- `brrrty doctor --verify` also builds the site with the project's own 11ty (`node_modules/@11ty/eleventy`) and with Brrrty, in the system temp folder, at one frozen time (`SOURCE_DATE_EPOCH`, or now), and compares every file with `brrrty-compat`. It prints the byte-identical, different, missing and extra counts, the first diffs, both timings and a line to paste: `Your site is byte-compatible on Brrrty: 2,272 files, 0.89 s vs 2.39 s`. It runs the site's JavaScript through 11ty, so it says so first, asks on a terminal and needs `--yes` off one. When Node or 11ty is missing, it says how to install it. Ctrl-C or SIGTERM stops 11ty and everything it started, removes the temporary folders and exits 130 or 143.

## 0.0.13 - 2026-11-17

- `brrrty init` sets up an existing 11ty site in one step: `npx brrrty init`. It adds `brrrty` to `devDependencies` at its own version and sets the `build` and `start` scripts to `brrrty build` and `brrrty serve`. Scripts that are just an 11ty command (`eleventy` or `npx @11ty/eleventy`, with flags) are kept as `build:11ty` and `start:11ty`; any other script is left alone, with a note saying what to change, down to which step of a script like `npm run css && eleventy` to swap. The rest of `package.json` keeps its indentation, line endings, key order and byte order mark, and the edit is checked to change nothing else.
- `brrrty init` also writes `brrrty.config.rhai` from `eleventy.config.js` (`.mjs`, `.cjs` or `.eleventy.js`), reading it without running it. Folders, `markdownTemplateEngine` and `htmlTemplateEngine`, passthrough copies and ignores with literal paths, and the syntax highlighting and RSS feed plugins with literal options are ported. Every other statement gets a `// TODO:` that quotes the JavaScript with its line numbers and links to the docs, and so does anything it can't be sure runs as written (code after a `return`, a name declared twice); a file it can't be sure of becomes a single TODO and no setting is ported: one that exports twice, has `return` before a line break, passes the config object to code it can't port (a helper, an `if`, a loop), or has a `/` that could be read as either a regular expression or division. It reads only regular files inside the site's folder, up to 1 MiB. It lists JavaScript data files that still need a `.rhai` port, and prints the next steps. It never overwrites `brrrty.config.rhai`, writes only `package.json` and `brrrty.config.rhai` in the site's folder (each atomically, and never through a symlink that leads outside it), and doesn't run `npm install`.
- `brrrty init` shows its changes as a diff and asks before writing. `--yes` skips asking, `--dry-run` writes nothing, and without a terminal to ask in it writes nothing unless given `--yes`. `--dir` points it at another folder. A folder that isn't an 11ty site exits with status 2.
- A bare `brrrty` in an 11ty site without `brrrty.config.rhai` prints a one-line hint to run `npx brrrty init` before building as before.

## 0.0.12 - 2026-11-17

- npm releases are published with npm trusted publishing: the release workflow proves its identity to npm through GitHub's OIDC token, so no npm token is stored in the repository.

## 0.0.11 - 2026-11-17

- With the syntax highlighting plugin, a page highlighted in a language that another page could change (a `rust` fence, say) is no longer refused because of `{% highlight %}` text that no template language reads. The check that runs before the build used to count such tags in every file, and treated any file mentioning `njk` as Nunjucks, so a Markdown changelog quoting `{% highlight js 1,3 %}` in inline code, with `markdownTemplateEngine: false`, refused every `rust` page. It now decides what reads each file the way the build does (its extension, its front matter's `templateEngineOverride`, and `markdownTemplateEngine` and `htmlTemplateEngine`, `false` included) and counts a tag only where Liquid or Nunjucks reads it and the plugin gives that language the tag. Where that can't be known before the build (data that could set the override, a file a template could include, from an include tag or a `{% liquid %}` tag's statements (nested ones too), or name as its layout, data files and `_includes`), it still counts the tag, so a page that 11ty would highlight differently is never written.
- A `{% highlight %}` tag with a line break after `{%` or inside its arguments, and a `highlight` statement in a `{% liquid %}` tag, count in that check too, as 11ty runs them. It used to be missed, and when the page with it was refused before reaching it, a page after it could be written with a grammar 11ty had changed.
- `date: Last Modified` is the file's change time (`ctime`), as in 11ty, not its modification time. They differ after anything that sets the modification time back or leaves it alone while changing the file's metadata, such as `touch -m`, a restore that keeps times, or `chmod`.
- When a `_data` file fails, its error now prints before the problems of the pages it stops, which say to fix the error "reported above". Before, a page whose path sorted before the data file (`src/About.md` before `src/_data/site.json`) printed first.
- The build summary says `Wrote 1 file` and `copied 1 file`, not `1 files`.

## 0.0.10 - 2026-11-17

- Brrrty is on npm: `npm install --save-dev brrrty`, then `brrrty build` in your scripts. The `brrrty` package is a small launcher that runs a prebuilt binary from `@brrrty/darwin-arm64`, `@brrrty/darwin-x64`, `@brrrty/linux-arm64-musl` or `@brrrty/linux-x64-musl`, whichever npm installed for the platform. No install scripts and no Rust toolchain needed. The Linux binaries are static, so they run on Alpine and glibc distributions alike. Arguments, output and exit codes pass through unchanged, and Ctrl-C under `npm run` or `npx` stops `brrrty serve` cleanly with exit 0. Ctrl-\\ and every other terminating signal Node can catch are passed on too, and `brrrty serve` stops cleanly by itself if the launcher is killed outright, so it never stays behind holding its port.
- Each release attaches binaries for those four platforms, with SHA-256 sums, to its GitHub release.
- Every npm package and release tarball carries NOTICE, with the copyright and license notices of the projects Brrrty ports code and data from, next to LICENSE.
- On Linux, a page without a `date` is dated by its file's creation time whatever the C library, as 11ty does. The static musl build used to fall back to the modification time, so a page edited after it was created got a different date (and a different place in date-sorted collections) than in 11ty. Where the creation time is missing, Brrrty now does what 11ty does there too: a file system that doesn't record it dates the page (and `date: Last Modified`) by the build time, and a system where `statx` is unavailable uses the file's change time.

## 0.0.9 - 2026-11-17

- A build into an empty or new output folder is about 20% faster on macOS. On APFS, many threads creating files under the same folders fight over folder locks, so pages and passthrough files are now written by 4 threads of their own, and passthrough files are copied in parallel on every system. Passthrough copies whose destinations overlap (a file and a folder mapped to one path, or names that differ only by case or Unicode normalization on macOS) still run one at a time in walk order, so the output and the problems reported are the same as before.
- Passthrough copies keep their source's access and modification times on every system, as 11ty's copy does, so rsync and other deploy tools that compare times no longer re-upload every passthrough file after a build on Linux. On macOS, copies already kept them. Rendered pages get the time they are written, as in 11ty.

## 0.0.8 - 2026-11-17

`brrrty serve`:

- `brrrty serve` builds the site, serves it, and rebuilds on every change, as `eleventy --serve` does. It takes `--port` (8080, moving up to 10 ports on when it is taken), `--host` (`localhost`), `--input`, `--output`, `--passthrough`, `--quiet` and `--strict`. It prints `Server at <url>`, `File changed: <path>` and `Watching…` as 11ty does.
- Live reload: pages get a script that patches in an edit to the page's own template, swaps an edited stylesheet, and reloads for anything else, and logs build errors in the browser console, as 11ty's dev server does.
- The output folder is kept up to date after every build, from the build's own list of what it wrote, so an unchanged file is never read again. Passthrough sources reached through a symlink are copied again on every edit. Builds are made in `<output>/.brrrty-serve/`, which is never served and is removed on exit; `brrrty build` removes one a killed session left behind.
- A running session holds a lock on `<output>/.brrrty-serve/` (an OS file lock, with its process id in the file). `brrrty build` into the same output folder leaves it alone and prints a `serve-staging-in-use` note, and a second `brrrty serve` there refuses to start. The OS releases the lock when the process ends, so a killed session's folder is still cleaned up.
- When no build ever fills the output folder (the first build failed, as under `--strict`), a folder the session created is removed on exit, as 11ty leaves none.
- Ctrl-C stops cleanly and exits 0, with the last good build in the output folder and the port released. A second Ctrl-C quits at once. SIGTERM stops the same way and then ends by the signal.
- `eleventy.env.runMode` is `"serve"` while serving.

Where `brrrty serve` differs from `eleventy --serve`:

- It listens on `localhost` only, unless `--host` says otherwise, and on that address answers only requests whose `Host` names this machine.
- It never serves a file outside the output folder: `..` in any encoding and symlinks that point outside get a 404, and malformed URLs a 400.
- A build with errors in some pages is still served, and its pages that built are written to the output folder, as `brrrty build` writes them. A page whose template has an error keeps the version last served (the rebuild says how many were kept), and the error shows in the browser console; a page that never built is a 404. A first build with errors keeps the server running. `--strict` serves no build with an error and exits with status 1 when the first build has one, as 11ty does.
- Any change under the input folder, a deleted file included, starts a rebuild.
- The live-reload socket refuses pages from other sites.
- Paths under `/.brrrty/` (`reload-client.js`, `morphdom.js`, `live`) are reserved while serving, as `/.11ty/` is in 11ty.
- `--quiet` prints only the `Server at` line, errors and warnings; 11ty's also prints the `Wrote` line, `Watching…` and changed files.
- An added file is reported as `File changed:` (11ty: `File added:`), and a changed file outside the project folder by its full path. When the port is taken, the `Server at` line adds `(port N was in use)`.
- `setServerOptions` and `serverPassthroughCopyBehavior` have no equivalent.

Building Brrrty now needs Rust 1.89 or later.

## 0.0.7 - 2026-11-17

Nunjucks:

- `.njk` templates are built, with a Nunjucks 3.2.4 engine configured as 11ty 3.1.8 configures it: autoescaping on, `_includes` then the project folder as include roots, and `./` names relative to the including template. This replaces the refusal for `.njk` files.
- Pages, layouts and includes can be Nunjucks, in any mix with Liquid and Markdown: a `.md` page in an `.njk` layout, and the reverse. An include of any file, such as `.js` or `.css`, is rendered as Nunjucks, as in 11ty; one without tags comes out byte for byte. Permalinks and computed data in `.njk` pages are Nunjucks, and `markdownTemplateEngine` and `htmlTemplateEngine` can be `"njk"`.
- Expressions follow JavaScript: truthiness, `==` and `===`, `+` on strings and numbers, how `undefined` and `null` print, and the string, array, number and date methods a template calls. `if`, `for` (with `else`, `loop` and objects), `set`, `macro`, `call` and `caller`, `import` and `from`, `include` (with `ignore missing`), `extends`, `block` and `super()`, `raw`, `verbatim`, `filter` and `switch` work, with whitespace control (`{%-`, `-%}`).
- Every Nunjucks built-in filter, 11ty's filters, the RSS plugin's filters and the config's Rhai filters run in Nunjucks. A `SafeString` stays safe through filters and macros as it does in Nunjucks.
- Regular expression literals (`r/.../gi`) work with `replace`, `replaceAll`, `split`, `test`, the `replace` filter and printing. They match as JavaScript's do, with the same rules as `regex_replace` in scripts.
- What Brrrty can't reproduce exactly is refused with the reason, never approximated: a method with no exact emulation, `random`, a `sort` or `dictsort` whose order would depend on V8's sort, async filters and tags, 11ty's `setAsync`, `getBundle` and `getBundleFileUrl`, printing a function (Nunjucks prints its JavaScript source), the syntax highlighting plugin's Nunjucks `{% highlight %}` shortcode, a `SafeString` passed to a config (Rhai) filter or to an RSS filter that every engine has (JavaScript would pass an object, not a string), options to `slug` and `slugify`, and a time zone argument to `dateToRfc822`.
- The engine has the Liquid engine's safety limits: nesting depth, loop and output size, and a parser that never backtracks. A crash in it refuses the page instead of stopping the build.
- A render may run 3,000,000 loop iterations in all, across nested loops, includes and macros, so nested loops are stopped in seconds.
- A filter that lists a collection item's properties (`urlencode`, `dictsort`, `list`, `dump`) is refused, as `for k, v in item` is: 11ty prints its `template` object and content there. Counting or testing the names in `collections` (`length`, `in`, `dictsort`) is refused when the site has an 11ty config that may add collections.

Config:

- `git_head()` gives the commit `git rev-parse HEAD` prints, for a config or data script that ports `execSync("git rev-parse HEAD")`. It reads the repository's files (`HEAD`, loose refs, `packed-refs`, and a linked worktree's `.git` file and `commondir`) from the project folder up, and starts no process. Outside a repository it is an error, which `try`/`catch` can turn into a default.
- A script filter that lists the names in `collections` (`keys()`, `len()`, `contains()`, `for k in`) under an 11ty config, or changes a collection item, is refused.
- `map`, `filter` and `reduce` with an anonymous function run about twice as fast, and `regex_split` with a pattern without groups is faster.
- The regular expression matcher behind `regex_replace`, `regex_split` and `regex_test` moved to `brrrty-jsregex`, where Nunjucks uses it too.

CLI:

- `brrrty build 2>&1 | head -1` no longer aborts with exit code 134 when the pipe closes: printing stops and the build's own exit code stands.

Syntax highlighting:

- Markdown front matter inside a fence is written highlighted as YAML when a page 11ty renders before it loaded YAML, and refused, as before, when none does. Brrrty replays the order 11ty renders pages' content in.
- The scan for languages that change grammars counts a Nunjucks `{% highlight %}` whose argument isn't one string, and any `{% highlight` whose arguments start on the next line, as any language.

## 0.0.6 - 2026-11-17

Syntax highlighting:

- Brrrty now highlights code the way `@11ty/eleventy-plugin-syntaxhighlight` does. Fenced code blocks in Markdown, the `{% highlight %}` Liquid tag and the plugin's options (`lineSeparator`, `errorOnInvalidLanguage`, `alwaysWrapLineHighlights`, `preAttributes`, `codeAttributes`, `trim`, `languages`, `templateFormats`) give the same HTML, byte for byte. `config.add_plugin("syntaxhighlight")` takes them as a map. This replaces the refusal for pages with a fenced code block or a `{% highlight %}` tag.
- It is a port of Prism 1.30.0 itself: the grammars' 464 distinct regular expressions are translated to Rust regular expressions that match as JavaScript's do, and Prism's tokenizer, hooks and line highlighting (`` ```js/1-2,4 ``, `{% highlight js 1,3 %}`) and `diff-` languages run as they do in Node.
- Brrrty has the grammars of 28 of Prism's 297 languages, the ones the test sites use: `bash` (`sh`, `shell`), `c`, `clike`, `css`, `diff`, `go`, `ini`, `javascript` (`js`), `json` (`webmanifest`), `jsx`, `less`, `liquid`, `lua`, `markdown` (`md`), `markup` (`html`, `xml`, `svg`, `mathml`, `ssml`, `atom`, `rss`), `markup-templating`, `nginx`, `perl`, `php`, `python` (`py`), `r`, `ruby` (`rb`), `rust`, `sql`, `toml`, `typescript` (`ts`), `vim` and `yaml` (`yml`), plus Prism's `plain` (`plaintext`, `txt`).
- A name that isn't a Prism language or alias, such as `conf`, `zsh`, `console`, `terraform` or `JS`, is written as the plugin writes it: the code as it is, with a `language-x` class. So is `text`. With `errorOnInvalidLanguage`, such a name other than `text` fails the page with `syntax-highlight-failed`, where 11ty stops the build.
- Highlighting is never approximated. A page whose highlighting Brrrty can't reproduce exactly is refused with the reason in a `syntax-highlight-unsupported` problem:
  - a fence or `{% highlight %}` in one of the other 269 Prism languages, such as `java`, `kotlin`, `swift`, `cpp`, `csharp`, `scss`, `graphql`, `docker` or `shell-session`, or `njk`, which the plugin highlights as `django`;
  - a page highlighting with a grammar that a language on another page changes. In 11ty, loading `regex`, `css-extras`, `js-extras`, `js-templates`, `jsdoc`, `javadoclike`, `phpdoc` or `php-extras` (among 21 such languages) changes the grammars of `javascript`, `css`, `php`, `markup` and what is built on them for every page rendered after. Brrrty scans every file under the input folder before it builds, Nunjucks pages and pages it refuses for other reasons included, and refuses the pages those grammars would highlight, naming the file and line that loads the language. A language that reaches the highlighter only as pages render, such as a fence built from data, is caught too: every page renders before any is written, and if one meets such a language the build is redone refusing what it changes. The list comes from Prism's `components.json` and from loading each language in Node;
  - a pattern that would backtrack more than a budget allows (on hostile input, reaching the budget can take a few seconds), and Markdown with a fenced block or front matter inside a fence, which 11ty highlights differently depending on the order it rendered pages in.

  `syntax-highlight-failed` is for where the plugin itself throws, in a fence or a `{% highlight %}` tag.
- Fixed: with the `code` rule off, Markdown still stops treating lines indented 4 or more spaces as continuing a list item, as markdown-it does.
- Building is faster for pages with code in languages with long keyword lists, such as SQL: translating a case-insensitive pattern no longer reads Prism's whole case table for every letter.

## 0.0.5 - 2026-11-16

Config written in Rhai:

- `brrrty.config.rhai` at the project root is Brrrty's version of `eleventy.config.js`. When it exists, Brrrty reads it and ignores the JavaScript config. It sets `dir`, passthrough copy, `ignores`, global data, filters, collections and plugins. `PORTING.md` in `brrrty-config` shows how to port a JavaScript config, call by call.
- Filters (`config.add_filter`) get their arguments the way 11ty passes them to a JavaScript filter, keyword arguments included. A filter that fails refuses only the page that used it, where 11ty stops the build.
- Collections (`config.add_collection`) get 11ty's collection API: `get_all`, `get_all_sorted`, `get_filtered_by_tag`, `get_filtered_by_tags` and `get_filtered_by_glob`, with 11ty's glob matching. Items are read-only.
- Helpers for what JavaScript does differently from Rhai: `object()` keeps keys in the order they were added, `truthy`, `js_string`, `js_concat`, `js_div`, `js_round`, `js_sort`, `js_sort_by`, `json_stringify`, and arrays get JavaScript's `sort`, `reverse` and `slice`. Sorting runs V8's own TimSort, so even a comparator that isn't a consistent ordering, such as `|a, b| a.date > b.date`, gives the order Node gives, at any length. `regex_replace`, `regex_split` and `regex_test` take JavaScript patterns.
- `luxon_format(date, format, zone)` formats dates as Luxon does, and `markdown(s)` and `markdown_inline(s)` render as `markdownIt()` does.
- Data files can be Rhai scripts: `_data/name.rhai`, `page.11tydata.rhai` and `folder.11tydata.rhai`. A script's last expression, or its `fn data(global)`, is the data. A `.rhai` file replaces a JavaScript data file of the same name, with a note saying so.
- Two global data files for one key (`a.json` and `a.rhai`) merge, as in 11ty.
- `config.add_plugin("rss", #{ ... })` is 11ty's RSS feed plugin: Atom, RSS and JSON feeds, with its filters for Liquid.
- `config.add_plugin("syntaxhighlight")` marks a site that uses 11ty's syntax highlighting plugin. Brrrty can't highlight yet, so pages with a fenced code block that names a language, or a `{% highlight %}` tag, are refused instead of written without highlighting.
- `SOURCE_DATE_EPOCH` sets the time for Rhai's `now()` and for Liquid's `'now'` and `'today'`.
- Scripts are limited in operations, time, call depth and size (what `object()`s hold included), and can't capture variables in closures or read the real clock through Rhai's `timestamp()`, so a script can't hang the build or give different output from one run to the next. Errors point at the script's file, line and column.
- A filter written in Rhai that would be given a Liquid `nil`, `null`, `empty`, `blank` or `forloop` refuses its page: 11ty passes LiquidJS's own objects for those, and no script value acts the same.

Regular expressions in scripts accept only what means the same in JavaScript and Rust. Text with emoji or other characters outside the BMP works with patterns where no match can stop between the two halves of such a character, such as `(<([^>]+)>)` or `\s+`. A pattern that could, or a repeated group that can match nothing, such as `(a*)?`, is refused with the reason.

A Markdown render that crashes refuses its page with `markdown-render-failed` instead of stopping the build. A crash while running a script is a problem too: `config-error` at the config's top level, `data-rhai-error` in a data file, and a refused page in a filter or collection.

A feed whose HTML nests elements more than 2000 deep is refused with `feed-failed`, where it used to crash Brrrty (11ty's posthtml runs out of stack a little deeper and stops the build).

Fenced code blocks whose language is only characters JavaScript's `trim` keeps, such as U+0085, count as naming a language, as in markdown-it.

New errors:

- `config-syntax`, `config-error`, `config-invalid`, `config-limit`, `config-unreadable` and `config-unsupported`: a problem in `brrrty.config.rhai`, with its line and column.
- `data-rhai-syntax`, `data-rhai-error` and `data-rhai-limit`: a problem in a `.rhai` data file. Only the pages that use its data are refused.
- `data-rhai-shadows-js` (a note): a `.rhai` data file replaces a JavaScript one.
- `syntax-highlight-unsupported`: a page needs syntax highlighting.
- `feed-failed` and `feed-unsupported`: a feed that can't be built, or one Brrrty doesn't support yet, such as an unknown option or a feed path that is also a file.
- `passthrough-unsupported`: a passthrough copy Brrrty can't do yet, such as a glob.
- `source-date-epoch-invalid`: `SOURCE_DATE_EPOCH` isn't a whole number of seconds.
- `markdown-render-failed`: Markdown rendering crashed on a page.

## 0.0.4 - 2026-11-16

Data, permalinks, pagination and collections:

- JSON data files in `_data` load as 11ty loads them, including folders of data files and symbolic links. JavaScript data files (`.js`, `.cjs`, `.mjs`) don't run yet: only the pages that use their data are refused.
- Template and directory data files (`page.json`, `page.11tydata.json`, `folder/folder.json` and the like) apply to their pages, and the whole data cascade merges deeply in 11ty's order: global data, layouts, data files, then front matter.
- `eleventyComputed` values are rendered in 11ty's dependency order.
- Permalinks can be Liquid templates, and `permalink` can be computed.
- `pagination` works over lists and objects, with `size`, `alias`, `reverse`, `filter`, `resolve: values` and `generatePageOnEmptyData`. Any `size` 11ty accepts works, such as `'3'` or `[2]`.
- `collections` are built from `tags`, with `collections.all`, `eleventyExcludeFromCollections` and `addAllPagesToCollections`, in 11ty's order, including the order `eleventyImport.collections` sets. Pages can paginate a collection.
- `templateContent` and `content` of collection items work, including pages that read each other's content, rendered in the order 11ty renders them.
- Nunjucks templates and pages under a Nunjucks layout are collection items. Their own pages aren't written yet, but pages that list them build.
- Front matter YAML reads values the way 11ty's YAML loader does: dates, `0123` as 123, `.inf`, and numbers too large for JavaScript stay strings.
- A file named like its folder (`posts/posts.md`) is written as the folder's `index.html`, as in 11ty.
- A `date` left empty, `""`, `0` or `false` counts as no date, as in 11ty: the page takes the date in its path, or its file's date. `Last Modified` and `Created` match in any case.
- `json` and `inspect` of collection items stop at once with a problem, as 11ty does, instead of working through every item first.

Where 11ty stops the whole build, Brrrty refuses only the page or data file at fault and builds the rest: a bad data file, pagination over missing data, two pages with one permalink, a `templateContent` circle, or a computed data loop. A page whose date Brrrty can't read is refused with the pages that read a collection it is in; other collections still work.

Front matter whose YAML aliases (`*name`) expand to more than a million values is refused as `front-matter-invalid`, at the alias that goes over, instead of taking the build down.

New errors:

- `data-file-invalid`: a data file that can't be read or parsed.
- `data-js-unsupported`: a page uses data from a JavaScript data file, which Brrrty can't run yet. Pages that don't use it still build.
- `computed-cycle` and `computed-unsupported`: computed values that depend on each other in a loop, or a computed key Brrrty can't set yet, such as `layout`.
- `pagination-invalid` and `pagination-unsupported`: pagination 11ty can't build, or that Brrrty can't follow yet.
- `content-cycle` and `content-unavailable`: a page reads `templateContent` in a circle, or content that 11ty wouldn't have rendered yet.
- `tags-invalid`: `tags` that are neither a string nor a list.
- `permalink-unsupported`: `permalinkBypassOutputDir`, which would write outside the output folder.

Removed:

- The `permalink-templated` error. Templated permalinks now render.

## 0.0.3 - 2026-11-15

Liquid templates and layouts:

- `.liquid`, `.md` and `.html` pages render through Liquid, as 11ty does by default. `.md` pages then go through Markdown. The Liquid engine matches LiquidJS as 11ty configures it, filters included, plus 11ty's `slug`, `slugify`, `url` and `log`.
- `{% include %}` and `{% render %}` find files in `_includes`, then in the input folder. A name can't lead outside those folders, through `..`, an absolute path or a symbolic link.
- The front matter `layout` key works: layouts are looked up in `_includes` with 11ty's extension order, can have their own `layout`, and their front matter merges under the page's. `.md` layouts run Markdown too.
- Pages get 11ty's `page`, `eleventy` and `pkg` data, and `templateEngineOverride` is honored.
- Markdown is rendered by a port of markdown-it, configured like 11ty's.
- Dates print in the machine's local time zone, as in 11ty, and `TZ` is honored. Set `TZ` to get the same output on every machine.

New errors:

- `liquid-syntax`, `liquid-unknown-filter`, `liquid-render` and `liquid-limit`: a Liquid problem, reported in the file it comes from (the page, a layout or an include) at that file's line and column. The rest of the site still builds.
- `layout-missing`, `layout-cycle`, `layout-invalid` and `layout-unsupported`: a layout that can't be found, a chain of layouts that loops (reported once, naming every layout in the loop), a layout that isn't a template file, and a Nunjucks or JavaScript layout.
- `data-unsupported`: a page uses something Brrrty doesn't read yet, such as `_data` files, template and directory data files, or `collections`. The page is not written, instead of being written without that data.
- `data-reserved`: data sets a name 11ty sets itself, such as `content` or `page.url`.
- `template-engine-override-invalid` and `template-engine-unsupported`: a `templateEngineOverride` that isn't valid, or that names Nunjucks.

Removed:

- The `template-syntax-unsupported` error. Template syntax in `.md` and `.html` pages now renders.

## 0.0.2 - 2026-11-15

Behavior change:

- Ignore files now follow 11ty's rules. Each line in `.gitignore`, `.eleventyignore` and `<input>/.eleventyignore` is a path from that file's own folder, not a gitignore pattern that matches at any depth. So a root `.gitignore` line `drafts` ignores `./drafts` but not `src/drafts`. A new `ignore-line-anchored` warning points at lines that would have matched under the old rules, with the full path to write instead.

Changes:

- `<input>/.eleventyignore` is now read.
- Symlinked files and folders in the input folder are followed and built at the link's path.
- Page dates and slugs come from the whole path, like 11ty: a date in a folder name counts, and `index` takes its slug from its folder. An impossible path date like `2026-13-45` is a `date-invalid` error for that page, and the rest of the site still builds.
- Permalinks with no file extension are allowed when the page sets `eleventyAllowMissingExtension` to a truthy value, and for `_redirects`, `.htaccess` and `_headers`.
- Two passthrough sources that copy to the same output file are now a `passthrough-collision` error.
- An input folder outside the project root, through `--input ../x` or a symlink, now builds and honors its `.eleventyignore`.
- Problem paths are shown relative to the project root.

New warnings:

- `ignore-line-anchored`: an ignore line matches nothing where it is anchored, but would have matched pages as a gitignore pattern.
- `ignore-line-invalid`: an ignore line is not a valid pattern, so it is skipped and the build goes on.
- `template-plugin-only`: files such as `.webc`, `.pug` or `.mdx` are skipped because they are templates only when a plugin adds them.
- `symlink-loop` and `symlink-broken`: a link that points back into its own folder, or at a missing file, is skipped.

## 0.0.1 - 2026-11-15

First release: builds plain Markdown and HTML sites the way 11ty does. Templates coming next.
