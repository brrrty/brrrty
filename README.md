<p align="center">
  <a href="https://brrrty.com/"><img src="https://brrrty.com/assets/brand/bertie.svg" width="168" alt="Bertie, the Brrrty tortoise, with motion lines behind him"></a>
</p>

<h1 align="center">Brrrty</h1>

<p align="center"><strong>Eleventy, but make it go brrr.</strong></p>

<p align="center">
  <a href="https://github.com/brrrty/brrrty/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/brrrty/brrrty/ci.yml?branch=main&style=flat&label=CI" alt="CI"></a>
  <a href="https://crates.io/crates/brrrty"><img src="https://img.shields.io/crates/v/brrrty?style=flat" alt="crates.io"></a>
  <a href="https://www.npmjs.com/package/brrrty"><img src="https://img.shields.io/npm/v/brrrty?style=flat" alt="npm"></a>
  <a href="https://github.com/brrrty/brrrty/actions/workflows/coverage.yml"><img src="https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/brrrty/brrrty/badges/coverage.json&style=flat" alt="coverage"></a>
  <a href="https://github.com/brrrty/brrrty/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat" alt="license: MIT"></a>
</p>

Brrrty is a static site generator written in Rust that builds your existing 11ty site, with byte-compatible output, in a fraction of the time. Templates, Markdown and data stay as they are. The config moves to a small Rhai script, and `brrrty init` writes most of it for you.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://brrrty.com/assets/readme/bench-dark.svg">
  <img src="https://brrrty.com/assets/readme/bench-light.svg" width="880" alt="Clean build of a 2,272-file blog. MacBook Pro, Apple M4: Brrrty 0.85 s, 11ty 3.1.8 2.32 s, 2.7 times faster. GitLab shared runner: 1.47 s vs 3.82 s, 2.6 times. GitHub Actions runner: 1.87 s vs 4.80 s, 2.6 times. Every output file is byte-identical to 11ty's.">
</picture>

## Running in production

- [brrrty.com](https://brrrty.com/) is built with Brrrty, from [`site/`](site/) in this repo.
- [joshtronic.com](https://joshtronic.com/), a 2,272-file blog, is built with Brrrty in GitLab CI on every push. It moved over from 11ty with no changes to its templates, and its output was compared file by file with 11ty's before the switch.

It's early (0.0.x), so try it on a copy of your site first. `brrrty doctor --verify` builds the site with both and tells you whether every file matches.

## Try it on your site

In your 11ty site's folder:

```sh
npx brrrty init      # adds brrrty to package.json, ports your config to brrrty.config.rhai
npm install
npx brrrty doctor    # lists anything that keeps a page from building, with file and line
npm run build
```

`init` reads your 11ty config without running it, shows its changes as a diff and asks first. What it can't port becomes a `// TODO(brrrty):` with the original code quoted, and Brrrty won't build pages until those are dealt with. Prebuilt binaries cover macOS (arm64, x64) and Linux (arm64, x64, static, so Alpine works). Or, anywhere Rust 1.89+ runs: `cargo install brrrty`.

## The config

The same calls, in snake_case, with `config` for `eleventyConfig`:

```js
// eleventy.config.js
module.exports = function (eleventyConfig) {
  eleventyConfig.addPassthroughCopy("src/images");
  eleventyConfig.addFilter("shout", (s) => s.toUpperCase() + "!");
  eleventyConfig.addCollection("posts", (api) =>
    api.getFilteredByGlob("./src/posts/*.md").reverse()
  );
  return { dir: { input: "src" } };
};
```

```rust
// brrrty.config.rhai
config.add_passthrough_copy("src/images");
config.add_filter("shout", |s| s.to_upper() + "!");
config.add_collection("posts", |api|
    api.get_filtered_by_glob("./src/posts/*.md").reverse());

#{ dir: #{ input: "src" } }
```

JavaScript data files (`_data/*.js`, `*.11tydata.js`) can run in your site's own Node with `config.set_js_data("node")`, or be ported to `.rhai` data files. `init` translates the simple ones and `doctor` checks each port against the original.

## What it does

- **Templates:** Liquid, Nunjucks, Markdown and HTML in any mix, with 11ty's data cascade, layouts, pagination, permalinks and collections.
- **Plugins:** RSS and syntax highlighting, built in.
- **`brrrty serve`:** rebuilds on every change and updates open pages in place.
- **Refusals:** where Brrrty can't match 11ty exactly, it refuses that page, says why with the file and line, and builds the rest. It never writes a page that might differ.

The [differences from 11ty](https://brrrty.com/docs/differences/) page lists everything it refuses or does differently.

## How it's checked

Output is compared byte for byte against real 11ty 3.1.8. Test fixtures get their expected output from 11ty itself, run in Docker with no network, and the suite has over 1,400 tests. Parts of 11ty and its dependencies (LiquidJS, Nunjucks, markdown-it, Prism, Luxon and others) are ported closely enough to match their output, each credited in [NOTICE](NOTICE). Brrrty was built with [Claude Code](https://claude.com/claude-code): Claude wrote the code and Josh steered, with an independent review and a file-by-file comparison against 11ty before every merge. [How it was built](https://brrrty.com/#built) has the story and what it cost.

## Docs

- [Getting started](https://brrrty.com/docs/getting-started/): setting up, porting a config, checking the output
- [Configuration](https://brrrty.com/docs/configuration/): everything `brrrty.config.rhai` can do
- [Templates](https://brrrty.com/docs/templates/): what each language supports
- [`brrrty doctor`](https://brrrty.com/docs/doctor/) and [`brrrty serve`](https://brrrty.com/docs/serve/)
- [Changelog](CHANGELOG.md)
- [Benchmark](bench/README.md): a generated 1,000-post blog you can run yourself

## The name

Brrr + Eleventy = Brrrty, as in "money printer go brrr". Three Rs. It's also a nod to Bertie, who holds the Guinness World Record for the fastest tortoise.

Brrrty exists because Eleventy is great. Thank you to Zach Leatherman and the 11ty community. MIT licensed; see [LICENSE](LICENSE) and [NOTICE](NOTICE).
