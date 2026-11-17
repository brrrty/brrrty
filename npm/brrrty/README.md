# Brrrty

**Eleventy, but make it go brrr.**

Brrrty is a fast, independent static site generator written in Rust that aims to build your existing 11ty site unchanged. Three Rs, like "go brrr".

Status: early development. See the [repository](https://github.com/brrrty/brrrty) for what works today.

## Install

```sh
npm install --save-dev brrrty
```

Then point your scripts at it:

```json
"scripts": {
  "build": "brrrty build",
  "start": "brrrty serve"
}
```

Or run it once with `npx brrrty build`.

## How it is installed

This package is a small launcher. The Brrrty binary comes prebuilt in a package for your platform, which npm picks from this package's optional dependencies:

| Platform | Package |
|---|---|
| macOS, Apple Silicon | `@brrrty/darwin-arm64` |
| macOS, Intel | `@brrrty/darwin-x64` |
| Linux, arm64 | `@brrrty/linux-arm64-musl` |
| Linux, x64 | `@brrrty/linux-x64-musl` |

The Linux binaries are static, so they run on Alpine and on glibc distributions alike. Nothing is downloaded or compiled at install time and there are no install scripts, so `npm ci --ignore-scripts` works. Installs that skip optional dependencies (`--omit=optional`) leave the binary out, and `brrrty` says so when it runs.

On other platforms, build Brrrty from source with `cargo install brrrty`.

The launcher passes arguments, input and output through unchanged and exits with the binary's exit status. Ctrl-C, Ctrl-\\ and SIGTERM reach the binary, so `brrrty serve` stops cleanly under `npm run` and `npx`, and if the launcher itself is killed, `brrrty serve` stops on its own. Ctrl-Z suspends npm and the launcher but not `brrrty serve`, which keeps serving until you resume and stop it.

## License

MIT
