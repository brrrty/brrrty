---
title: Configuration
order: 2
summary: Every call brrrty.config.rhai can make, the helpers scripts get, and where Rhai differs from JavaScript.
description: The brrrty.config.rhai reference.
lead: <code>brrrty.config.rhai</code> is a Rhai script at the project root. It calls methods on a <code>config</code> object shaped like 11ty's <code>eleventyConfig</code>, in snake_case, and ends with the object 11ty's config function returns.
---

```rust
config.add_passthrough_copy("src/images");
config.add_passthrough_copy(#{ "src/favicon.svg": "favicon.svg" });
config.ignores.add("src/drafts/**");
config.add_global_data("build", #{ year: year(now()) });
config.add_filter("shout", |s| s.to_upper() + "!");
config.add_collection("posts", |api|
    api.get_filtered_by_tag("post").reverse());
config.add_plugin("rss", #{
    type: "atom",
    output_path: "/feed.xml",
    collection: #{ name: "posts", limit: 10 },
    metadata: #{
        language: "en",
        title: "My blog",
        base: "https://example.com/",
        author: #{ name: "Me", email: "" },
    },
});

#{ dir: #{ input: "src", output: "_site" } }
```

Nothing runs until the whole script has. An error in the script stops the build before any page is written, with the line and column.

## The config object

| Call | What it does |
|---|---|
| `config.add_passthrough_copy(path)` | `addPassthroughCopy`. Copies a file or folder as is. The path is from the project root. |
| `config.add_passthrough_copy(#{ from: to })` | `addPassthroughCopy({...})`. Copies each source to the given output path. |
| `config.ignores.add(glob)` | `ignores.add`. Leaves matching files out of the build. |
| `config.ignores.delete(glob)` | `ignores.delete`. Removes a glob added earlier. |
| `config.add_global_data(name, value)` | `addGlobalData`. Adds global data. Two objects under one name merge deeply, as in 11ty. A function is called once, when the config loads. |
| `config.add_filter(name, fn)` | `addFilter`. Adds a filter to Liquid and Nunjucks. |
| `config.add_nondeterministic_filter(name)` | Stands in for a filter that uses `Math.random` in your JavaScript config. Its output can never match 11ty's, so pages that use it are refused. |
| `config.add_collection(name, fn)` | `addCollection`. Adds a collection. |
| `config.add_layout_alias(name, file)` | `addLayoutAlias`. `layout: name` uses `file`, relative to the layouts folder. |
| `config.add_plugin(name, options)` | `addPlugin`, for the plugins Brrrty has its own version of: `rss` and `syntaxhighlight`. `options` is optional. |

Passthrough copy takes a file or folder, not a glob; a glob is refused with `passthrough-unsupported`.

### What the script returns

The script's last expression is a map with these keys, all optional. A script can also end with a statement, which is the same as returning nothing.

| Key | Values |
|---|---|
| `dir` | `input` and `output` folders, and `includes` and `layouts`: folders inside the input folder, relative to it. `layouts` defaults to the includes folder. `data` is accepted only as `_data`. |
| `markdownTemplateEngine` | `"liquid"` (the default), `"njk"` or `false` |
| `htmlTemplateEngine` | `"liquid"` (the default), `"njk"` or `false` |

`dataTemplateEngine` is ignored with a warning, since 11ty 3 doesn't read it. Any other key is refused, so a setting Brrrty doesn't support never goes unnoticed.

## Filters

A filter gets the value, then the arguments in the order the template wrote them. A missing argument is `()`, so `n ?? 40` gives a default. A keyword argument arrives in place as a two-item array, `[name, value]`.

```rust
config.add_filter("money", |n, symbol| (symbol ?? "$") + js_string(n));
```

```liquid
{{ price | money: "€" }}
```

A filter that throws, returns something that isn't data, or runs too long refuses the page that called it, with the template's position and the script's. The rest of the site builds. (11ty stops the whole build there.)

Nothing carries over from one filter call to the next. Brrrty renders pages in parallel, so a counter or cache in a JavaScript filter could never give 11ty's output. To make that impossible rather than check for it, a function can't see variables outside itself, and every argument is a copy. To use a value set up at the top of the script, curry it into a named function:

```rust
let rates = #{ a: 2, b: 3 };
fn rate(rates, value, key) { rates[key] * value }
config.add_filter("rate", Fn("rate").curry(rates));   // {{ 5 | rate: "b" }}
```

## Collections

`config.add_collection(name, |api| ...)` runs where 11ty runs it, and `api` has 11ty's collection API in snake_case:

| Rhai | 11ty |
|---|---|
| `api.get_all()` | `getAll()`, in the order pages were added |
| `api.get_all_sorted()` | `getAllSorted()`, by date, then input path |
| `api.get_filtered_by_tag("t")` | `getFilteredByTag("t")` |
| `api.get_filtered_by_tags("a", "b")` | `getFilteredByTags("a", "b")`, up to four tags |
| `api.get_filtered_by_glob("./src/posts/**/*.md")` | `getFilteredByGlob(...)`, one glob or an array of them |

Globs are read from the project root, as 11ty reads them. Brrrty supports `*`, `?`, `**` as a whole folder name, and literal text; braces, `[ab]`, `!` and extglobs are an error rather than a guess.

Items are the pages themselves, and read-only: setting a property on one is an error, and so is changing an array read through one, like `post.data.tags` (`to_array()` copies it). Return any mix of arrays, maps and `object()`s holding items, and templates still read their `url`, `data` and `templateContent`. Reading an item's `content` or `templateContent` inside the collection refuses the pages that read the collection, because 11ty hasn't rendered it yet at that point.

Arrays have JavaScript's `sort()`, `sort(|a, b| ...)`, `reverse()` and `slice(start, end)`, and `map`, `filter` and `reduce` take a function:

```rust
config.add_collection("byOrder", |api|
    api.get_filtered_by_tag("docs").sort(|a, b| a.data.order - b.data.order));
```

## Data files

Global data in `_data/name.rhai`, and template and folder data in `page.11tydata.rhai` and `folder/folder.11tydata.rhai`, are Rhai scripts. The last expression is the data. Folders nest keys as JSON files do: `_data/a/b.rhai` is `a.b`.

```rust
// _data/site.rhai
#{ name: "Mine", url: "https://example.com", year: year(now()) }
```

If the script defines `fn data()` or `fn data(global)`, Brrrty calls it the way 11ty calls a function a data file exports, with the config's global data and 11ty's `eleventy` data as the argument, and the rest of the script doesn't run.

Each file runs once per build, on its own, and can't see `config`. A `.rhai` file and a `.json` file for the same key merge, the `.rhai` one winning, and a `.js` file next to a `.rhai` port is ignored. An error refuses only the pages that read that key.

## JavaScript data

`config.set_js_data("node");` runs the site's JavaScript data files (`_data/*.js`, `.cjs`, `.mjs`, and `*.11tydata.js` and the like) in your own Node, as 11ty runs them, so a site that uses luxon, `fetch` or `@11ty/eleventy-fetch` in its data builds without a port. `config.set_js_data("off")`, or no call, keeps them off: a page that reads their data is refused, and the rest build. `--js-data=node` or `--js-data=off` on `brrrty build`, `serve` and `doctor` overrides the config for one run; `off` shows what still depends on JavaScript. `brrrty init` writes the line when it finds files it can't translate to Rhai.

- **Which Node:** `BRRRTY_NODE` when set, else `node` on `PATH`; 18 or later, 11ty 3's own floor. The npm package sets `BRRRTY_NODE` to the Node that runs it. With no Node, the pages that need the data are refused with `js-data-node-missing`, which says what was tried.
- **How it runs:** one Node per build, given the worker on its command line (nothing is written to disk). Files load one at a time in 11ty's order, with 11ty's import rules (a lone `default` export, CommonJS's duplicated `default`, `"type": "module"`), and an exported function is called with the config's global data and `eleventy`, frozen. A template or directory data function runs once for every template it applies to, as in 11ty; if two calls give different data, the pages that use it are refused (`js-data-impure`), since 11ty's result would depend on its load order. `ELEVENTY_ROOT`, `ELEVENTY_SOURCE`, `ELEVENTY_RUN_MODE` and `ELEVENTY_VERSION` are set, and `process.argv` is `npx @11ty/eleventy`'s, with `--serve` under `brrrty serve` (11ty's other flags aren't). A process the data starts and leaves running is stopped with the build, unless it started a session of its own (`detached`).
- **What crosses:** plain objects (in JavaScript key order), arrays (a hole reads as `undefined`, which no template output tells apart), strings, numbers (`-0`, `NaN` and infinities too), booleans, `null`, `undefined` and `Date`s, exactly. Symbol keys and non-enumerable keys are left out, as 11ty's copy of the data leaves them out; on an object 11ty passes to templates as it is (inside an array, or frozen), where they stay readable, they are refused instead. A `Proxy` of a plain object at the top of a file is copied key by key, as 11ty copies it. Anything else is refused with `js-data-value`, naming the file and the path inside it, and only the pages that read that key, directly or through another page's `data` in a collection or pagination, are refused: a `Map`, `Set`, `RegExp`, `BigInt`, typed array, class instance (a luxon `DateTime` included), an invalid `Date`, a circular reference, a getter that changes between reads (a getter is read twice to tell), a value nested more than 1000 objects and arrays deep, or a function read as a value.
- **Logging:** a data file's `console.log`, and anything it or a process it starts writes to stdout, goes to stderr, where Brrrty passes it through, not stdout as under 11ty: stdout carries Brrrty's own output and `--json`.
- **Errors:** a file that throws is `js-data-error`, at the first line of your own code in the stack, with the source line. One that never settles gets a note after 10 seconds, and is waited for, as 11ty waits.
- **`brrrty serve`** runs the data again in a fresh Node on every rebuild, and rebuilds when any file it read or loaded inside the project changes, `node_modules` and `package-lock.json` included. A file it read outside the project isn't watched.
- **Security:** it's your site's own code, run with your rights, as under 11ty, with network and file access. Brrrty doesn't sandbox it. Brrrty itself still writes only the output folder; the data's code can write elsewhere, as `@11ty/eleventy-fetch` writes its `.cache/` folder.

`brrrty doctor` lists each file and its state. A port to Rhai is still the long-term path: it needs no Node, and doctor checks a port against the file it replaces.

## Reading files

Data files and the config can read files in the project, as a JavaScript data file does with `fs` and csv-parse:

| JavaScript | In Rhai |
|---|---|
| `fs.readFileSync(path, "utf8")` | `read_text(path)` |
| `JSON.parse(fs.readFileSync(path, "utf8"))` | `read_json(path)` |
| `fs.readdirSync(path)` | `read_dir(path)` |
| `fs.readdirSync(path, { recursive: true })` | `read_dir(path, #{ recursive: true })` |
| `fs.existsSync(path)` | `file_exists(path)` |
| `parse(text, options)` from `csv-parse/sync` | `parse_csv(text, options)` |

```rust
// _data/products.rhai
let products = parse_csv(read_text("data/products.csv"), #{
    columns: true,
    skip_empty_lines: true,
    trim: true,
});
let notes = [];
for file in read_dir("src/_data/notes") {
    if file.ends_with(".md") {
        notes.push(read_text("src/_data/notes/" + file));
    }
}
#{ products: products, notes: notes }
```

- Paths start at the project folder (where `brrrty.config.rhai` is), so `join(__dirname, "list.csv")` in `src/_data/x.js` is `"src/_data/list.csv"`. An absolute path, a `..`, and a symlink that leads outside the project are refused: every path is opened through a handle on the project folder, so not even a symlink swapped in mid-build gets out. Only regular files are read, and nothing writes.
- `read_text` replaces bytes that aren't UTF-8 as Node does and keeps a byte order mark. `read_dir` lists names in Node's order on macOS and Linux (sorted by their bytes, so `10.md` comes before `2.md` and `Zebra.md` before `apple.md`); `recursive` lists the folder, then each subfolder in turn as `sub/name`, entering symlinked folders as Node 26 does; a symlink that leads outside the project, or a loop, is refused.
- `parse_csv` is csv-parse 7.0.3, quirks and error messages included, with the options `columns` (`true`), `delimiter`, `bom`, `trim`, `ltrim`, `rtrim`, `skip_empty_lines` and `relax_column_count`. Any other option is refused. With `columns`, each record is an `object()` in the header's order.
- A file over 16 MB is refused unread. A missing file is an error that `try { ... } catch { ... }` catches, as in JavaScript.
- `brrrty serve` rebuilds when a file a script read changes, wherever it is in the project.

## Plugins

`config.add_plugin(name, #{ ... })` adds Brrrty's version of an 11ty plugin, with 11ty's options in snake_case. Any other name is an error. Port every `addPlugin` call: Brrrty only reads the Rhai config, so it can't tell when one is missing, and the pages that plugin would have changed come out as if it weren't there.

- **`rss`** is `@11ty/eleventy-plugin-rss`'s feed plugin, one feed per call. Options: `type` (`"atom"`, `"rss"` or `"json"`), `output_path`, `input_path`, `collection` (`name`, `limit`, `sort`), `metadata`, `template_data`, `stylesheet` and `script`. Its filters work in Liquid and Nunjucks as they do in 11ty. For the filters alone, with no feed (11ty's `addPlugin(pluginRss)`, for a site that writes its own feed template), use `config.add_plugin("rss", #{ feed: false })`.
- **`syntaxhighlight`** is `@11ty/eleventy-plugin-syntaxhighlight` with Prism 1.30.0, for Markdown fences and the Liquid `{% highlight %}` tag. Options: `line_separator`, `error_on_invalid_language`, `always_wrap_line_highlights`, `trim`, `languages`, `template_formats`, `pre_attributes` and `code_attributes`. It has 28 of Prism's 297 languages, including `bash`, `css`, `go`, `javascript`, `json`, `liquid`, `markdown`, `markup` (HTML), `php`, `python`, `ruby`, `rust`, `sql`, `toml`, `typescript` and `yaml`. A page in a Prism language Brrrty doesn't have yet is refused.

The full lists, and the cases each plugin refuses, are in [PORTING.md](https://github.com/brrrty/brrrty/blob/main/crates/brrrty-config/PORTING.md#plugins).

## Markdown renderer rules

`config.amend_library("md", |md| ...)` is 11ty's `amendLibrary`: it changes how 11ty's own markdown-it writes your `.md` pages and Markdown layouts. Brrrty runs markdown-it renderer rules for inline tokens (links, images, code, text, emphasis):

```rhai
fn tag_shop_links(base, tokens, idx, options, env, renderer) {
    let href = tokens[idx].attr_get("href") ?? "";
    if href.starts_with("https://shop.example/") {
        tokens[idx].attr_set("rel", "sponsored nofollow");
        tokens[idx].attr_set("target", "_blank");
    }
    base.call(tokens, idx, options, env, renderer)
}

config.amend_library("md", |md| {
    let base = md.renderer.rules.link_open ?? |t, i, o, e, r| r.render_token(t, i, o);
    md.renderer.rules.link_open = Fn("tag_shop_links").curry(base);
});
```

A rule gets markdown-it's five arguments (`self` is `renderer`, since `self` is a reserved word in Rhai) and returns the HTML for that token. Tokens have `attr_get`, `attr_set`, `attr_join`, `attr_index` and `attr_push`, and their fields (`type`, `tag`, `content`, `markup`, `info`, `level`, ...) can be read and set. `renderer.render_token(tokens, idx, options)` is markdown-it's default. Rhai closures can't capture variables, so a rule that wraps the one before it takes it curried, as above.

A data file or filter with its own markdown-it uses `markdown_it(options)`: `let md = markdown_it(#{ linkify: true }); md.renderer.rules.link_open = ...; md.render(text)`.

The tokens and the HTML are markdown-it's, checked against markdown-it 14.3.3 (11ty's) and 15.0.3. What Brrrty can't give a rule exactly is an error that names it, and the page is refused with `markdown-rule-failed`: a rule for a block token (`paragraph_open`, `fence`, ...), reading `env` (the page's data in 11ty), adding or removing tokens, changing a token's type to a block type, changing a rule while pages render, or returning something that isn't a string. The details are in [PORTING.md](https://github.com/brrrty/brrrty/blob/main/crates/brrrty-config/PORTING.md#markdown-it-renderer-rules).

## Rhai is not JavaScript

Values cross into the script as 11ty would give them to a JavaScript filter, and come back out as data. Numbers are floats, `undefined` is `()`, `null` is `null_value()`, and dates are dates. Rhai's own operators aren't changed, so where they differ from JavaScript's, use a helper:

| JavaScript | In Rhai | Use |
|---|---|---|
| `7 / 2` is `3.5` | `7 / 2` is `3` | `js_div(7, 2)`, or `7.0 / 2`. Numbers from data are floats already. |
| `a + b` | strings join, numbers add, a mix is an error | `js_concat(a, b)` |
| `String(v)`, `` `${v}` `` | `to_string()` prints arrays and maps Rhai's way | `js_string(v)` |
| `if (x)`, `x \|\| y` | only a `bool` is allowed | `truthy(x)` |
| `Math.round(x)` | `round()` rounds halves away from zero | `js_round(x)` |
| `JSON.stringify(v)` | | `json_stringify(v)` |
| `{}` built up key by key | `#{ }` maps sort their keys | `object()`, which keeps insertion order |
| `arr.sort()`, `arr.sort(fn)` | replaced with JavaScript's | `arr.sort()`, `arr.sort(\|a, b\| ...)`, or `js_sort(arr)` and `js_sort_by(arr, fn)` for a sorted copy |
| `n.toFixed(digits)` | | `to_fixed(n, digits)`, V8's rounding exactly |
| `n.toLocaleString("en-US")` | | `to_locale_string(n)` |
| `a.localeCompare(b)` | | `locale_compare(a, b)` |
| `Math.random()` | | not available: see below |
| `new Date(ms)` | | `date(ms)` |
| `new Date(y, m, d, h, mi, s, ms)` | | `date(y, m, d, h, mi, s, ms)`, from two to seven parts, in the local time zone |
| `d.getFullYear()`, `d.getMonth()`, `d.getDate()`, `d.getDay()`, `d.getHours()`, ... | | `d.get_full_year()`, `d.get_month()`, `d.get_date()`, `d.get_day()`, `d.get_hours()`, ... (local time) |
| `d.toISOString()` | | `d.to_iso_string()` |
| `Date.now()` | | `get_time(now())` |
| `new Date().getFullYear()` | | `year(now())` |
| `d.toUTCString()` | | `date_to_utc_string(d)` |
| `d.toLocaleDateString("en-US", { year: "numeric", month: "long", day: "numeric" })` | | `to_locale_date_string(d)` |
| `new URL(u).hostname` | | `url_hostname(u)`, or `()` when `u` isn't a full URL |
| `s.replace(/re/g, r)`, `s.split(/re/)`, `/re/.test(s)` | | `regex_replace(s, pattern, r, "g")`, `regex_split(s, pattern)`, `regex_test(s, pattern)` |
| `DateTime.fromJSDate(d, { zone }).toFormat(fmt)` (Luxon) | | `luxon_format(d, fmt, zone)` |
| `markdownIt().render(s)`, `.renderInline(s)` | | `markdown(s)`, `markdown_inline(s)` |
| `markdownIt({ linkify: true, typographer: true })`, `md.linkify.set({ fuzzyLink: true })`, `md.render(s)` | | `markdown(s, #{ linkify: true, typographer: true, fuzzy_link: true })` |
| `execSync("git rev-parse HEAD")` | scripts can't run commands | `git_head()` |

Sorting runs V8's own TimSort, so even a comparator that isn't a consistent ordering gives the order Node gives. Regular expressions accept only syntax that means the same in JavaScript and in Rust: no lookaround and no backreferences, among a few others, and a pattern outside that is an error naming it.

`to_fixed`, `to_locale_string` and `locale_compare` give what Node gives, checked against it on hundreds of thousands of values. `to_fixed` rounds the number's exact binary value, as V8 does: `to_fixed(2.5, 0)` is `"3"`, but `to_fixed(1.005, 2)` is `"1.00"`, because 1.005 is stored as 1.00499999... `to_locale_string(n)` is the `en-US` format: up to three decimals, thousands grouped with commas (`1,234.568`). `locale_compare(a, b)` is the order Node sorts strings in with an English locale: case and accents count only after the letters (`a`, `A`, `b`, `é`, `f`). In Node, `toLocaleString()` and `localeCompare()` without a locale follow the language set in `LC_ALL`, `LC_MESSAGES` or `LANG` (the first that is set). Brrrty matches the `en-US` result, so without a locale they run only when that is `en_US` (such as `en_US.UTF-8`), `C` or `POSIX`, or nothing is set; another language, another locale argument, or options is an error rather than a guess. `locale_compare` also refuses Chinese and Japanese ideographs (Han): Node sorts them by radical and stroke count, from data Brrrty doesn't have.

`markdown(s, options)` takes markdown-it's `html`, `linkify` and `typographer` options and linkify-it's `fuzzyLink` (as `fuzzy_link`), each `true` or `false`; any other option is an error. It is markdown-it 15 with linkify-it 6, so links without `http://` (`example.com`) need `fuzzy_link: true`, as in JavaScript. A markdown-it plugin or rule can't be added yet.

`Math.random()` has no stand-in: a page built from a random number differs on every build, so it can never be the same file as 11ty's. Calling `random()` is an error that says so. If your JavaScript config has a filter that shuffles or picks at random, declare it with `config.add_nondeterministic_filter(name)`: pages that use it aren't written, and the rest of the site builds.

Local dates (`date(y, m, d)`, the `get_*` methods, and dates printed in templates) use the time zone in the `TZ` environment variable, or the computer's, as Node does, with the time zone data Node 26 ships (release 2026b) built into Brrrty, so the computer's own, older or newer, data doesn't change your pages. Build with the same zone as your 11ty builds, or set `TZ`. A `TZ` that isn't a zone name (a typo, or a rule like `EST5EDT,M3.2.0,M11.1.0`) stops the build, where Node would quietly use UTC. A site built with another Node version can differ in a zone whose rules changed in between.

`object()` has `keys()`, `values()`, `entries()`, `len()`, `contains(k)` and `remove(k)`, and `for k in o` walks its keys in order. `object(map)` copies a map in. Objects that come from Brrrty, such as page data, are `object()`s already, so returning one as it came keeps its order.

## Time

`now()` reads the system clock. Set `SOURCE_DATE_EPOCH` to a whole number of seconds since 1970, and `now()`, Liquid's `'now'` and `'today'`, and the RSS feeds' dates all use that time instead, so two builds of the same source give the same files. Rhai's own `timestamp()` is an error that points to `now()`.

## Printing

`print(x)` writes to standard output, and `debug(x)` writes the script's file, line and `x` to standard error. Pages render in parallel, so lines from different pages can come out in any order. Neither changes what is built.

## Limits

A script may run 50 million operations and 10 seconds, nest 64 calls deep, and hold strings up to 16 MB and arrays and maps up to 100,000 items. Past a limit it stops with a problem that points at the file, so a script can't hang the build. There is no `import` and no `eval`, and files are only read through the helpers above.

The full porting guide, with every edge case, is [PORTING.md](https://github.com/brrrty/brrrty/blob/main/crates/brrrty-config/PORTING.md).
