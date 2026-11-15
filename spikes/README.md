Measurement harness that scores candidate Liquid, Nunjucks and Markdown libraries against real Eleventy 3.1.8 output.
It is a workspace member but is never published (`publish = false`).
Run `cargo run -p brrrty-spike -- reference liquid` to regenerate expected files, then `-- score liquid` to measure candidates.
Point it at an Eleventy 3.1.8 install with `--eleventy <path to @11ty/eleventy/cmd.cjs>` or the `BRRRTY_ELEVENTY` environment variable. The `markdown` command falls back to the first `--site`'s own install.
