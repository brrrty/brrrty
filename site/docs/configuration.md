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
| `config.add_plugin(name, options)` | `addPlugin`, for the plugins Brrrty has its own version of: `rss` and `syntaxhighlight`. `options` is optional. |

Passthrough copy takes a file or folder, not a glob; a glob is refused with `passthrough-unsupported`.

### What the script returns

The script's last expression is a map with these keys, all optional. A script can also end with a statement, which is the same as returning nothing.

| Key | Values |
|---|---|
| `dir` | `input` and `output` folders. `includes`, `data` and `layouts` are accepted only with their defaults, `_includes`, `_data` and `_includes`. |
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

- **`rss`** is `@11ty/eleventy-plugin-rss`'s feed plugin, one feed per call. Options: `type` (`"atom"`, `"rss"` or `"json"`), `output_path`, `input_path`, `collection` (`name`, `limit`, `sort`), `metadata`, `template_data`, `stylesheet` and `script`. Its filters work in Liquid and Nunjucks as they do in 11ty.
- **`syntaxhighlight`** is `@11ty/eleventy-plugin-syntaxhighlight` with Prism 1.30.0, for Markdown fences and the Liquid `{% highlight %}` tag. Options: `line_separator`, `error_on_invalid_language`, `always_wrap_line_highlights`, `trim`, `languages`, `template_formats`, `pre_attributes` and `code_attributes`. It has 28 of Prism's 297 languages, including `bash`, `css`, `go`, `javascript`, `json`, `liquid`, `markdown`, `markup` (HTML), `php`, `python`, `ruby`, `rust`, `sql`, `toml`, `typescript` and `yaml`. A page in a Prism language Brrrty doesn't have yet is refused.

The full lists, and the cases each plugin refuses, are in [PORTING.md](https://github.com/brrrty/brrrty/blob/main/crates/brrrty-config/PORTING.md#plugins).

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
| `new Date(ms)` | | `date(ms)` |
| `Date.now()` | | `get_time(now())` |
| `new Date().getFullYear()` | | `year(now())` |
| `d.toUTCString()` | | `date_to_utc_string(d)` |
| `d.toLocaleDateString("en-US", { year: "numeric", month: "long", day: "numeric" })` | | `to_locale_date_string(d)` |
| `new URL(u).hostname` | | `url_hostname(u)`, or `()` when `u` isn't a full URL |
| `s.replace(/re/g, r)`, `s.split(/re/)`, `/re/.test(s)` | | `regex_replace(s, pattern, r, "g")`, `regex_split(s, pattern)`, `regex_test(s, pattern)` |
| `DateTime.fromJSDate(d, { zone }).toFormat(fmt)` (Luxon) | | `luxon_format(d, fmt, zone)` |
| `markdownIt().render(s)`, `.renderInline(s)` | | `markdown(s)`, `markdown_inline(s)` |
| `execSync("git rev-parse HEAD")` | scripts can't run commands | `git_head()` |

Sorting runs V8's own TimSort, so even a comparator that isn't a consistent ordering gives the order Node gives. Regular expressions accept only syntax that means the same in JavaScript and in Rust: no lookaround and no backreferences, among a few others, and a pattern outside that is an error naming it.

`object()` has `keys()`, `values()`, `entries()`, `len()`, `contains(k)` and `remove(k)`, and `for k in o` walks its keys in order. `object(map)` copies a map in. Objects that come from Brrrty, such as page data, are `object()`s already, so returning one as it came keeps its order.

## Time

`now()` reads the system clock. Set `SOURCE_DATE_EPOCH` to a whole number of seconds since 1970, and `now()`, Liquid's `'now'` and `'today'`, and the RSS feeds' dates all use that time instead, so two builds of the same source give the same files. Rhai's own `timestamp()` is an error that points to `now()`.

## Printing

`print(x)` writes to standard output, and `debug(x)` writes the script's file, line and `x` to standard error. Pages render in parallel, so lines from different pages can come out in any order. Neither changes what is built.

## Limits

A script may run 50 million operations and 10 seconds, nest 64 calls deep, and hold strings up to 16 MB and arrays and maps up to 100,000 items. Past a limit it stops with a problem that points at the file, so a script can't hang the build. There is no `import` and no `eval`, and files are only read through the helpers above.

The full porting guide, with every edge case, is [PORTING.md](https://github.com/brrrty/brrrty/blob/main/crates/brrrty-config/PORTING.md).
