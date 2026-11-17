# Brrrty

**Eleventy, but make it go brrr.**

Brrrty is a fast, independent static site generator written in Rust that aims to build your existing 11ty site unchanged.

Three Rs, like "go brrr".

Brrrty exists because Eleventy is great. See NOTICE.

Status: early development. Not ready for real sites yet.

## Install

In an 11ty site's folder, from npm, with a prebuilt binary for macOS (arm64, x64) or Linux (arm64, x64; static, so Alpine works too):

```sh
npx brrrty init
```

`brrrty init` adds `brrrty` to `devDependencies`, sets the `build` and `start` scripts (keeping ones that are just an 11ty command as `build:11ty` and `start:11ty`), and writes `brrrty.config.rhai` from the 11ty config, with a `// TODO:` for each part it can't port. It reads the 11ty config without running it, shows the changes as a diff and asks first (`--yes` skips asking, `--dry-run` writes nothing), and never overwrites a `brrrty.config.rhai`. Then `npm install` and `npm run build`.

To set it up by hand instead, `npm install --save-dev brrrty`, then `"build": "brrrty build"` in your `package.json` scripts, or `npx brrrty build`. No Rust toolchain or install scripts are needed.

Anywhere Rust runs:

```sh
cargo install brrrty
```

Release binaries are also attached to each GitHub release.

## `brrrty doctor`

```sh
brrrty doctor [--json] [--verify [--yes]]
```

Lists what keeps your pages from building, without writing anything: filters that aren't defined, JavaScript data files to port, template languages Brrrty doesn't run, grouped by cause with the pages affected and each file and line. A build stops each page at its first problem; doctor also reads each refused page's template, layouts, includes, `permalink` and `eleventyComputed` for the rest, then a verdict such as `1,180 of 1,224 pages will build; 3 blockers, grouped below.`

`--verify` also builds the site with your project's own 11ty and with Brrrty, in the system temp folder at one frozen time, compares every file, and prints a line to paste: `Your site is byte-compatible on Brrrty: 2,272 files, 0.89 s vs 2.39 s`. It runs your site's JavaScript through 11ty, so it asks first (or needs `--yes` off a terminal). Brrrty itself never runs JavaScript.

Exit status: 0 when every page builds (and, with `--verify`, every file is byte-identical), 1 when something is refused or differs, 2 for usage errors or a missing Node or 11ty.

## Benchmark

`bench/` builds a generated 1,000-post blog with 11ty and with Brrrty, times both, and checks that every output file is identical. Run it with `node bench/run.mjs`, or from the **Bench** workflow on GitHub; [bench/README.md](bench/README.md) explains how to run and read it.

## `brrrty serve`

```sh
brrrty serve [--port 8080] [--host localhost] [--input DIR] [--output DIR] [--passthrough PATH] [--quiet] [--strict]
```

Builds the site, serves it at `http://localhost:8080/`, and rebuilds whenever a source file changes, as `eleventy --serve` does. Open pages update by themselves: an edit to the page's own template is patched in without a reload, a stylesheet edit swaps the stylesheet, and anything else reloads the page. Build errors print in the terminal and in the browser console. A page whose template has an error keeps the version last served, so a typo never turns the page you're editing into a 404, while the rest of the site goes on updating. A page that has never built is a 404 until it does.

- `--port`: when it is taken, the next free port up to 10 above it is used, and the startup line says so.
- `--host`: the address to listen on. The default only answers this machine; `--host 0.0.0.0` lets other devices on your network in.
- `--output` is kept up to date after every build, so it holds the site while you work and after you stop. It gets exactly what `brrrty build` would write: the pages that built, never the older copies kept for pages with errors. As with `brrrty build`, files it didn't write are left alone. The one removal: when a build writes a file where the folder holds a folder by that name, or a folder where it holds a file, the old one is replaced (`brrrty build` stops with an error there instead).
- `--quiet` prints only the `Server at` line, errors and warnings.
- `--strict` stops each build at its first error, and exits with status 1 if the first build has errors, as `eleventy --serve` does. With it, a build with an error is never served; the last one stays up.

Ctrl-C stops cleanly: a build already running finishes, the output folder holds the last good build, the port is released, and the exit status is 0. A second Ctrl-C quits at once. SIGTERM stops the same way and then ends by the signal.

While it runs, each build goes into `<output>/.brrrty-serve/`, which is removed when it stops. A leftover one from a session that was killed is removed by the next `brrrty serve` or `brrrty build`. A running session holds a lock on it: `brrrty build` into the same output folder leaves it alone and says so, and a second `brrrty serve` there refuses to start. It is never served. If the first build fails and no build ever fills the output folder, a folder the session created is removed when it stops.

Pages get a small script, `/.brrrty/reload-client.js`, so a site's own files at `/.brrrty/reload-client.js`, `/.brrrty/morphdom.js` and `/.brrrty/live` are hidden while serving.

Where it differs from `eleventy --serve`:

- It listens on `localhost` only, unless you pass `--host`, and on that address answers only requests that name this machine (`localhost`, `127.0.0.1`, `[::1]`), so another website can't reach it through a DNS trick.
- It never serves a file outside the output folder: `..` and its encoded forms get a 404, and so does a symlink that points outside. A malformed URL gets a 400.
- A build with errors in some pages still serves the rest, and its pages that built are written to the output folder, as `brrrty build` writes them. `eleventy --serve` writes nothing from a build with an error, and exits when the first build has one (see `--strict`).
- Any file under the input folder starts a rebuild, not only template, data and include files, and so does deleting one, so a deleted page disappears from what is served.
