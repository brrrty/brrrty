Measurement harness that scores candidate Liquid, Nunjucks and Markdown libraries against real Eleventy 3.1.8 output.
It is a workspace member but is never published (`publish = false`).
Run `cargo run -p brrrty-spike -- reference liquid` to regenerate expected files, then `-- score liquid` to measure candidates.
