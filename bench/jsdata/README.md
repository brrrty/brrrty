# JavaScript data benchmark

Sites that lean on what Brrrty runs in Node under `--js-data=node`, timed against 11ty 3.1.8. Each build is checked file by file against 11ty's output.

- `calls`: a global data file that exports a 30,000-item list and a 3,000-key lookup table next to its functions, and 2,000 Nunjucks pages that call four of them each. After every call Brrrty checks that the call left the exported data as it loaded, and at the end it replays every call in two other orders.
- `templates`: 4,000 pages from one paginated `.11ty.js` template (a permalink function, `this.slugify`, `this.url`, a Nunjucks layout), plus 200 single `.11ty.js` pages with data of their own.

`generate.mjs` writes both, with no randomness. `PAGES`, `ITEMS`, `TEMPLATE_PAGES` and `SINGLES` change their size.

## Running it

```sh
cargo build --release -p brrrty
node bench/jsdata/run.mjs /tmp/jsdata                      # Brrrty only
ELEVENTY=docker node bench/jsdata/run.mjs /tmp/jsdata      # and 11ty, compared
```

11ty runs in Docker (`node:22`): `eleventy-docker.sh` copies the site into the container and runs `npm ci` there from `bench/site`'s lockfile, so nothing is installed on the host. `RUNS` (default 3) sets the runs per tool; the table shows the fastest of each.
