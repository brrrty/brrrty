---
title: Getting started
order: 1
summary: Install Brrrty, port an 11ty config to Rhai, build, serve, and check the output against 11ty's.
description: Install Brrrty and move an existing 11ty site over to it.
lead: Brrrty builds an existing 11ty project. Your templates, Markdown, layouts and JSON data stay as they are. What changes is the config, and any data file written in JavaScript.
---

## Install

With Rust 1.89 or later:

```sh
cargo install brrrty
```

From 0.0.10, Brrrty is also on npm, for macOS and Linux. Add it to a project with `npm i -D brrrty` and run it with `npx brrrty`.

Check it works:

```sh
brrrty --version
```

## How a project is laid out

Brrrty works from the folder you run it in, the project root, as 11ty does. It reads `brrrty.config.rhai` there, if there is one, and builds the input folder (the root itself unless the config or `--input` says otherwise) into `_site`.

```text
my-site/
  brrrty.config.rhai     replaces eleventy.config.js
  src/
    _data/               global data: .json and .rhai
    _includes/           layouts and includes
    index.md
    posts/
```

Brrrty reads `_includes` and `_data` from the input folder, under those names. A config that moves them elsewhere is refused with a message saying so.

## 1. Port the config

11ty is configured in JavaScript, which Brrrty doesn't run. Its config is the same calls written in [Rhai](https://rhai.rs), a small scripting language, in a file called `brrrty.config.rhai` at the project root. Method names are 11ty's in snake_case, and `eleventyConfig` is `config`.

An 11ty config like this:

```js
module.exports = function (eleventyConfig) {
  eleventyConfig.addPassthroughCopy("src/images");
  eleventyConfig.addFilter("shout", (s) => s.toUpperCase() + "!");
  eleventyConfig.addCollection("posts", (api) =>
    api.getFilteredByGlob("./src/posts/*.md").reverse()
  );
  return { dir: { input: "src" } };
};
```

becomes:

```rust
config.add_passthrough_copy("src/images");
config.add_filter("shout", |s| s.to_upper() + "!");
config.add_collection("posts", |api| api.get_filtered_by_glob("./src/posts/*.md").reverse());

#{ dir: #{ input: "src" } }
```

The last expression is the object 11ty's config function returns. Every call is listed in the [configuration reference](/docs/configuration/).

Keep `eleventy.config.js` next to it while you work. Once `brrrty.config.rhai` exists, Brrrty ignores the JavaScript file, so the two can live side by side and 11ty keeps working.

Rhai is not JavaScript, and a few things work differently: `7 / 2` is `3`, `+` won't mix strings and numbers, `if` needs a real boolean, and `#{ }` maps sort their keys. Brrrty has a helper for each of these; see [Rhai is not JavaScript](/docs/configuration/#rhai-is-not-javascript).

## 2. Port JavaScript data files

Brrrty doesn't run JavaScript data files. Write each one as a Rhai script with the same name: `_data/site.js` becomes `_data/site.rhai`, and `post.11tydata.js` becomes `post.11tydata.rhai`. The script's last expression is the data:

```rust
// _data/site.rhai, from module.exports = { title: "My site", year: new Date().getFullYear() }
#{ title: "My site", year: year(now()) }
```

When a `.rhai` file sits next to a `.js` file of the same name, Brrrty uses the `.rhai` one and prints a note. JSON data files work unchanged.

A page that uses data from a `.js` file with no port is refused with `data-js-unsupported`; pages that don't use it still build.

## 3. Build

```sh
brrrty build
```

`brrrty` on its own does the same. It writes the site into `_site` and prints one line, like this one from building this site:

```text
Wrote 9 files, copied 6 files in 0.01 seconds (brrrty 0.0.9)
```

The flags are:

| Flag | What it does |
|---|---|
| `--input <dir>` | Folder with your source files. Default: `dir.input` from the config, or `.` |
| `--output <dir>` | Folder to write the site to. Default: `dir.output` from the config, or `_site` |
| `--passthrough <path>` | A file or folder to copy as is, relative to the current folder. Repeatable |
| `--strict` | Stop at the first error, for CI |
| `--quiet` | Don't print the summary line |

Flags on the command line win over the config, as they do in 11ty.

## When a page is refused

Brrrty never writes a page it can't write exactly as 11ty would. When it hits something it can't reproduce, or something 11ty would fail on, it skips that page, prints a problem with a code, the file, the line and a hint, and builds the rest of the site. Here, a page uses a filter the config doesn't add yet:

```text
Wrote 0 files, copied 0 files in 0.00 seconds (brrrty 0.0.9)
liquid-unknown-filter

  × undefined filter: shout
   ╭─[src/index.md:4:3]
 3 │ ---
 4 │ # {{ title | shout }}
   ·   ┬
   ·   ╰── here
   ╰────
  help: did you mean `shift`?

1 error, 0 warnings
```

A build with any error exits with status 1. Fix the cause, or port the missing piece, and build again.

## 4. Serve

```sh
brrrty serve
```

It builds the site, serves it at `http://localhost:8080/`, and rebuilds on every change, with live reload in the browser. See [`brrrty serve`](/docs/serve/).

## 5. Check the output against 11ty

The point of Brrrty is that the output is the same. Build with both and compare:

```sh
npx @11ty/eleventy --output=_site-11ty
brrrty build --output _site-brrrty
diff -r _site-11ty _site-brrrty
```

Run both on the same machine with the same `TZ`, since both print dates in local time. A page that shows the time of the build, such as a feed's `updated` date, will differ, since the two builds run at different moments.

If a file differs where 11ty built it fine, that is a bug in Brrrty. Please [open an issue](https://github.com/brrrty/brrrty/issues) with the smallest site that shows it.
