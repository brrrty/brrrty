# Benchmark

A reproducible comparison of 11ty and Brrrty on a synthetic blog shaped like a real long-running one. Anyone can rerun it, locally or on a GitHub runner, and get the same input.

Each run builds the blog with both tools, times them, and checks that Brrrty's output is byte-compatible with 11ty's: every file is compared, and any file that differs, is missing or is extra fails the run.

## The blog

`generate.mjs` writes the content from a fixed seed, so every machine gets the same files (it prints a hash of them to check):

- 1,000 Markdown posts over about 11 years, with front matter (title, date, category, tags, description), headings, lists, tables, blockquotes, links between posts, and fenced code in JavaScript, Rust, Bash, CSS, HTML, JSON, YAML and Python
- 600 small SVG images, copied through as passthrough files

The rest of `site/` is checked in: Liquid and Nunjucks layouts and includes, a paginated home page, tag and category pages paginated through collections the config builds, an Atom feed (`@11ty/eleventy-plugin-rss`), a sitemap, and highlighting (`@11ty/eleventy-plugin-syntaxhighlight`). `eleventy.config.js` has a few filters and collections, and `brrrty.config.rhai` is its port. The site comes to 2,069 output files.

The text is generated from a small word list. It doesn't describe anything real.

The generator is a Node script with no dependencies, since the benchmark needs Node for 11ty anyway: one more `node` command, no extra build step, and nothing to compile before the timings start.

## Running it

From the repository root, with Rust and Node 24:

```sh
cargo build --release --locked -p brrrty
npm ci --prefix bench/site
node bench/run.mjs
```

Brrrty is the binary just built from this checkout, and 11ty comes from `site/package-lock.json` (11ty 3.1.8, feed plugin 3.1.0, highlighting plugin 5.0.2).

| Variable | Default | |
|---|---|---|
| `RUNS` | `5` | Timed runs per tool |
| `WARMUP` | `1` | Untimed runs per tool first |
| `BRRRTY` | `target/release/brrrty` | The Brrrty binary |
| `SOURCE_DATE_EPOCH` | `1767225600` | The frozen clock |
| `POSTS`, `IMAGES`, `SEED` | `1000`, `600`, `20140105` | Size and seed of the generated blog |
| `SKIP_GENERATE` | | `1` keeps the content already in `site/src` |

On GitHub, run the **Bench** workflow from the Actions tab (it also runs on every release tag). It builds Brrrty from source at that commit, installs 11ty from the lockfile, and writes the table to the job summary.

## Reading the result

```
| | 11ty 3.1.8 | Brrrty 0.0.11 |
|---|---:|---:|
| mean | 2.89 s | 0.86 s |
| min | 2.83 s | 0.81 s |
| speedup (mean) | | 3.4x |
| speedup (min) | | 3.5x |
| runs (s) | 2.94, 2.97, 2.84, 2.88, 2.83 | 0.91, 0.88, 0.86, 0.84, 0.81 |

| identical | different | missing | extra |
|---:|---:|---:|---:|
| 2069 | 0 | 0 | 0 |
```

- **mean** and **min** are wall-clock times for a whole build, from starting the process to its exit, so they include Node's and 11ty's startup. **min** is the least noisy number on a shared runner.
- **speedup** is 11ty's time divided by Brrrty's.
- **identical** counts files whose bytes match. **different**, **missing** (written by 11ty, not by Brrrty) and **extra** (written by Brrrty, not by 11ty) should all be 0; if not, the run lists the first paths with a diff and exits 1.

How the runs are made fair:

- The tools alternate, 11ty then Brrrty, so a slowdown on the machine hits both.
- The output folder is deleted before every run, so neither tool gets an incremental build.
- Both run with the same frozen clock (`SOURCE_DATE_EPOCH`, which Brrrty reads; 11ty gets it from `xtask/src/freeze-time.cjs` through `node --require`) and `TZ=UTC`.
- Brrrty runs with `--strict`, so a page it can't build exactly fails the run instead of being left out.

Timings depend on the machine: a GitHub runner has 4 CPUs, and Brrrty builds pages in parallel, so expect a different speedup than on a laptop. The file counts shouldn't change anywhere.
