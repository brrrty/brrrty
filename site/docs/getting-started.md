---
title: Getting started
order: 1
summary: Set up Brrrty with npx brrrty init, finish porting the 11ty config to Rhai, build, serve, and check the output against 11ty's.
description: Install Brrrty and move an existing 11ty site over to it.
lead: Start from your existing 11ty site. Your templates, Markdown, layouts and JSON data stay as they are. What changes is the config, and any data file written in JavaScript.
---

## Set it up

With npm, on macOS or Linux (arm64 or x64), in your site's folder:

```sh
npx brrrty init
```

`brrrty init` shows what it will change as a diff and asks before writing anything. It changes two files:

- **`package.json`**: adds `brrrty` to `devDependencies`, and sets the scripts `"build": "brrrty build"` and `"start": "brrrty serve"`. A `build` or `start` script that is just an 11ty command (`eleventy` or `npx @11ty/eleventy`, with its flags) is kept as `build:11ty` or `start:11ty`, so you can still build with 11ty to compare. Any other script is left alone, and `init` says what to change instead: in `npm run css && eleventy`, which step to swap. The rest of the file keeps its formatting.
- **`brrrty.config.rhai`**: written from your 11ty config, which `init` reads without running it. Folders, template engines, passthrough copies and ignores with plain paths, and the syntax highlighting and RSS feed plugins with plain options are ported. Everything else, such as filters, collections, shortcodes and transforms, gets a `// TODO(brrrty):` line that quotes your JavaScript and links to the docs. So does anything `init` can't be sure 11ty runs as written, such as code after a `return`. When other code could change a setting, such as an `if` or a helper that gets the config object, the whole file is one `// TODO(brrrty-file):` and nothing is ported, since a wrong port would be worse than none. Settings for 11ty's watcher and dev server, and `dataTemplateEngine`, which 11ty 3 doesn't read, change nothing 11ty writes, so they are only noted as left out. A `// Ported from:` line in the header records your 11ty config's name and a hash of it, so Brrrty can warn when it changes later. `init` also reads the 11ty command in your `build` script (or, without one, the other scripts that run 11ty): `--input` (a folder) and `--output` become `dir.input` and `dir.output`, as they win over the config in 11ty; `--config=file.js` makes `init` port that file instead; and any other flag that changes what 11ty writes, such as `--pathprefix` or `--formats`, gets a `// TODO(brrrty):` that quotes the script. It reads `eleventy`, `npx eleventy`, `npx @11ty/eleventy`, `pnpm exec eleventy`, `yarn eleventy`, `bunx`, `npm exec` and `node .../@11ty/eleventy/cmd.cjs`, with redirections and `( ... )` around them. A script that mentions 11ty some other way (inside `concurrently`, after a `cd`, run twice), and one that runs a file that imports `@11ty/eleventy` (11ty's JavaScript API, found as Node finds the file, and through the site's own modules it loads), gets a `// TODO(brrrty):` instead, since Brrrty can't tell what it passes. So does a `build` script that doesn't run 11ty at all while `@11ty/eleventy` is a dependency. It reads deploy configs the same way, since they run the build too (Netlify's command overrides `package.json`'s): `netlify.toml`'s `[build]` and `[context.production]` `command`, `vercel.json`'s `buildCommand`, the `run:` steps of `.github/workflows/*.yml`, and the `script:` lists of `.gitlab-ci.yml`. In the CI files, a line that mentions 11ty but isn't read as a command (a YAML anchor, a `!reference`, a flow-style step) is a TODO naming the line, while comments and `name:` lines are ignored. One that runs `npm run build` adds nothing new; one whose flags differ from the build script's is a TODO naming both. A run that only adds `--output` or `--dryrun`, such as `npm run build:11ty -- --output=_site-11ty` to compare with Brrrty's output, is a comparison run and doesn't count. That holds when it's in CI or a script other than `build`, and the real build is clear: `brrrty build` as the `build` script or in the same CI file, or another 11ty run that writes to the usual output. Netlify's and Vercel's commands are always the build. A file that starts 11ty's command line (`cmd.cjs`, `npx @11ty/eleventy`) through `child_process` is read the same way, from its string literals. Without a `build` script or deploy command of its own, the site is taken to be built by hand with `npx @11ty/eleventy`. A `// 11ty flags from package.json:` line in the header records a hash of what `init` read there. `init` reads only files inside the site's folder, and never overwrites an existing `brrrty.config.rhai`.

It also lists the JavaScript data files that need a `.rhai` port (step 2 below). It doesn't run `npm install`; run that next, then `npm run build`.

| Flag | What it does |
|---|---|
| `--dir <dir>` | The site's folder. Default: the current folder |
| `--yes` | Write the changes without asking |
| `--dry-run` | Show the changes and write nothing |

Without a terminal to ask in, as in CI, `init` shows the changes and writes nothing unless you pass `--yes`.

### Setting it up by hand

Instead of `init`, add the package yourself:

```sh
npm i -D brrrty
```

Then point your scripts at it in `package.json`:

```json
{
  "scripts": {
    "build": "brrrty build",
    "start": "brrrty serve"
  }
}
```

The package runs a prebuilt binary for your platform, with no install scripts and no Rust toolchain.

Nothing checks a `brrrty.config.rhai` you write yourself against your 11ty config. While one sits next to an 11ty config with no `// Ported from:` line, `build` and `doctor` warn (`config-unverified`) and give the line to add once you've checked your port covers it; from then on they warn when the 11ty config changes.

Or, with Rust 1.89 or later, install it with Cargo:

```sh
cargo install brrrty
```

Check it works with `npx brrrty --version`, or `brrrty --version` after `cargo install`.

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

`brrrty init` ports what it can and leaves a `// TODO(brrrty):` line for the rest. This section is how to finish those by hand.

Each of those lines stands for something your 11ty config does that Brrrty's doesn't yet, and any of them could change any page: a transform rewrites every page, a Markdown library changes every Markdown page, and a whole-file TODO means Brrrty doesn't even know your folders. So while one is left, `brrrty build` writes no pages and copies no files, and `brrrty doctor` lists it first, as a blocker of every page, with its line numbers (`config-todo`). Port each one, or decide it changes nothing Brrrty writes, then delete its `TODO(brrrty)` line: that is how you tell Brrrty it's done. `grep -n 'TODO(brrrty' brrrty.config.rhai` lists what's left. A config written by an older `brrrty init`, whose lines read `// TODO:` followed by a `// See https://...` line, counts the same way; your own `// TODO:` comments don't count.

After deleting markers, `brrrty doctor --verify` (or [step 5](#5-check-the-output-against-11ty)) shows whether any page differs from 11ty's.

When your 11ty config changes after `init` read it, `build` and `doctor` warn (`config-stale`), and the warning gives the new hash. So they do when 11ty would now read another file: a rename from `eleventy.config.js` to `eleventy.config.mjs`, or a `.eleventy.js` added beside it, which 11ty reads first. Port what changed, then put the line the warning gives in place of the `// Ported from:` line. The hash ignores line endings and a leading BOM, so a Windows checkout doesn't warn. When the flags `package.json`'s scripts pass to 11ty change after that (or appear, in a config with no `// 11ty flags` line), they warn too, as `config-stale`, with the line to put there once the change is ported. So they do when 11ty reads no config file any more. Their help never asks you to take ported settings out of `brrrty.config.rhai`: run `brrrty init` in a copy of the site to see the port it gives now, or check yours against 11ty's output and update the line. Once you've moved off 11ty, with `@11ty/eleventy` gone from `package.json` and no 11ty config file left, the checks stop, and the two header lines can stay or go. Deleting the `// Ported from:` line doesn't stop the checks: with an 11ty config there and no `// Ported from:` line, they warn that the port is unverified (`config-unverified`).

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

Once `brrrty.config.rhai` exists, Brrrty ignores `eleventy.config.js`, so both can stay and 11ty keeps working.

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
npm run build
```

That runs `brrrty build`; `brrrty` on its own does the same. It writes the site into `_site` and prints one line:

```text
Wrote 9 files, copied 6 files in 0.01 seconds (brrrty 0.0.10)
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
Wrote 0 files, copied 0 files in 0.00 seconds (brrrty 0.0.10)
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

A build reports each page's first problem only. To see everything that stands in the way at once, grouped by cause (every missing filter, every JavaScript data file to port), run `npx brrrty doctor`. See [`brrrty doctor`](/docs/doctor/).

## 4. Serve

```sh
npm start
```

`brrrty serve` builds the site, serves it at `http://localhost:8080/`, and rebuilds on every change, with live reload in the browser. See [`brrrty serve`](/docs/serve/).

## 5. Check the output against 11ty

`brrrty doctor --verify` builds with your project's own 11ty and with Brrrty, at one frozen time, compares every file, and prints the result with both timings:

```sh
npx brrrty doctor --verify
```

It runs your site's JavaScript through 11ty, so it asks first. See [`brrrty doctor`](/docs/doctor/). To do the same by hand, build with both and compare:

```sh
npx @11ty/eleventy --output=_site-11ty
npx brrrty build --output _site-brrrty
diff -r _site-11ty _site-brrrty
```

Run both on the same machine with the same `TZ`, since both print dates in local time. A page that shows the time of the build, such as a feed's `updated` date, will differ, since the two builds run at different moments.

If a file differs where 11ty built it fine, that is a bug in Brrrty. Please [open an issue](https://github.com/brrrty/brrrty/issues) with the smallest site that shows it.
