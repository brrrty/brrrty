# Brrrty

**Eleventy, but make it go brrr.**

Brrrty is a fast, independent static site generator written in Rust that aims to build your existing 11ty site unchanged.

Status: early development. Not ready for real sites yet.

## `brrrty serve`

```sh
brrrty serve [--port 8080] [--host localhost] [--input DIR] [--output DIR] [--passthrough PATH] [--quiet] [--strict]
```

Builds the site, serves it at `http://localhost:8080/`, and rebuilds whenever a source file changes, as `eleventy --serve` does. Open pages update by themselves: an edit to the page's own template is patched in without a reload, a stylesheet edit swaps the stylesheet, and anything else reloads the page. Build errors print in the terminal and in the browser console, and the last good build stays up until an edit fixes them.

- `--port`: when it is taken, the next free port up to 10 above it is used, and the startup line says so.
- `--host`: the address to listen on. The default only answers this machine; `--host 0.0.0.0` lets other devices on your network in.
- `--output` is kept up to date after every good build, so it holds the site while you work and after you stop. As with `brrrty build`, nothing is ever deleted from it.
- `--quiet` prints only errors and warnings.
- `--strict` stops each build at its first error, and exits with status 1 if the first build fails, as `eleventy --serve` does. Without it, a failed first build keeps serving whatever the output folder held, and the first edit that builds takes over.

Ctrl-C stops cleanly: a build already running finishes, the output folder holds the last good build, the port is released, and the exit status is 0. A second Ctrl-C quits at once. SIGTERM stops the same way and then ends by the signal.

While it runs, each build goes into `<output>/.brrrty-serve/`, which is removed when it stops. A leftover one from a session that was killed is removed by the next `brrrty serve` or `brrrty build`. Run one `brrrty serve` per output folder, and don't run `brrrty build` into the same folder while it runs.

Pages get a small script, `/.brrrty/reload-client.js`, so a site's own files at `/.brrrty/reload-client.js`, `/.brrrty/morphdom.js` and `/.brrrty/live` are hidden while serving.

Where it differs from `eleventy --serve`:

- It listens on `localhost` only, unless you pass `--host`, and on that address answers only requests that name this machine (`localhost`, `127.0.0.1`, `[::1]`), so another website can't reach it through a DNS trick.
- It never serves a file outside the output folder: `..` and its encoded forms get a 404, and so does a symlink that points outside. A malformed URL gets a 400.
- A first build that fails keeps the server running (see `--strict`).
- Any file under the input folder starts a rebuild, not only template, data and include files, and so does deleting one, so a deleted page disappears from what is served.

Three Rs, like "go brrr".

Brrrty exists because Eleventy is great. See NOTICE.
