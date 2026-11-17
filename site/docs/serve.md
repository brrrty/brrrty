---
title: brrrty serve
order: 4
summary: The development server. Builds, serves, watches, rebuilds and reloads the browser.
description: brrrty serve, Brrrty's development server with live reload.
lead: <code>brrrty serve</code> stands in for <code>eleventy --serve</code>. It builds the site, serves it at <code>http://localhost:8080/</code>, and rebuilds whenever a file changes. No config change is needed.
---

```sh
brrrty serve [--port 8080] [--host localhost] [--input DIR] [--output DIR] [--passthrough PATH] [--quiet] [--strict]
```

| Flag | What it does |
|---|---|
| `--port <n>` | The port. When it is taken, the next free one up to 10 above it is used, and the startup line says so. Default 8080. |
| `--host <addr>` | The address to listen on. The default, `localhost`, only answers this machine; `--host 0.0.0.0` lets other devices on your network in. |
| `--input <dir>`, `--output <dir>`, `--passthrough <path>` | As for `brrrty build`. |
| `--quiet` | Print only the `Server at` line, errors and warnings. |
| `--strict` | Stop each build at its first error, and exit with status 1 if the first build has one, as `eleventy --serve` does. A build with an error is never served; the last good one stays up. |

## Live reload

Open pages update by themselves. An edit to the page's own template is patched into the page without a reload, an edited stylesheet is swapped in place, and anything else reloads the page. Build errors print in the terminal and in the browser console.

## When a build has errors

A page whose template has an error keeps the version last served, so a typo never turns the page you are editing into a 404, while the rest of the site goes on updating. A page that has never built is a 404 until it does.

## The output folder

`--output` is kept up to date after every build, with exactly what `brrrty build` would write: the pages that built, never the older copies kept for pages with errors. As with `brrrty build`, files Brrrty didn't write are left alone.

While it runs, each build goes into `<output>/.brrrty-serve/`, which is never served and is removed when the server stops. A running server holds a lock on it: `brrrty build` into the same output folder leaves it alone and says so, and a second `brrrty serve` there refuses to start. Leave that folder out of anything that deploys the output folder by hand while a session runs.

## Stopping

Ctrl-C stops cleanly: a build already running finishes, the output folder holds the last good build, the port is released, and the exit status is 0. A second Ctrl-C quits at once.

## Where it differs from eleventy --serve

- It listens on `localhost` only unless you pass `--host`, and there it answers only requests that name this machine (`localhost`, `127.0.0.1`, `[::1]`), so another website can't reach it through a DNS trick. The live-reload socket also checks the page's origin.
- It never serves a file outside the output folder: `..` in any encoding, and symlinks that point outside, get a 404. A malformed URL gets a 400.
- A build with errors still serves, and writes, the pages that built. Use `--strict` for 11ty's behavior.
- Every file under the input folder is watched, and deleting one rebuilds, so a deleted page disappears. So are the config, the ignore files, and passthrough sources outside the input folder.
- `addWatchTarget`, `setServerOptions` and `serverPassthroughCopyBehavior` have no equivalent yet. Passthrough files are always copied into the output folder.
- Pages load `/.brrrty/reload-client.js` instead of `/.11ty/reload-client.js`, so a site's own files under `/.brrrty/` are hidden while serving.
- An added file is reported as `File changed:`, where 11ty prints `File added:`.
