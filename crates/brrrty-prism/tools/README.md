# brrrty-prism tools

Node scripts that make brrrty-prism's data and check it against real Prism.
Everything they make is committed; nothing here runs during a build.

- `pins.cjs`: the Prism and plugin versions Brrrty reproduces, the fence
  languages its grammars are made for, and loading Prism the way
  `@11ty/eleventy-plugin-syntaxhighlight` does. The other scripts read it.
- `gen-grammars.cjs`: writes `grammars/prism.json`.
- `gen-highlight-probes.cjs`: writes `tests/probes/highlight/*.json`, and
  serves `tests/highlight_node.rs`.
- `regex-oracle.cjs`: runs regular expressions for `tests/fuzz.rs`.

## Upgrading Prism

Run these from `crates/brrrty-prism`, with Node 20 or later.

1. Set `PRISM` and `PLUGIN` in `pins.cjs` to the new versions. Add any new
   fence language to `FENCES`.
2. Install them somewhere outside the repo:

   ```sh
   npm install --prefix /tmp/prism prismjs@<PRISM> @11ty/eleventy-plugin-syntaxhighlight@<PLUGIN>
   ```

   Check that the plugin resolves the same `prismjs`
   (`npm ls --prefix /tmp/prism prismjs`).
3. Regenerate the grammars. The file is written only if every check
   passes: no grammar may depend on load order except an `inside`.

   ```sh
   node tools/gen-grammars.cjs /tmp/prism/node_modules grammars/prism.json
   ```

4. Regenerate the highlight probes. They need a Prism checkout at the same
   tag, for its test fixtures:

   ```sh
   git clone --depth 1 --branch v<PRISM> https://github.com/PrismJS/prism /tmp/prism-src
   node tools/gen-highlight-probes.cjs /tmp/prism/node_modules fixtures /tmp/prism-src/tests/languages tests/probes/highlight
   ```

5. Regenerate the regular expression probes:

   ```sh
   BRRRTY_WRITE_PROBES=1 cargo test -p brrrty-prism --test fuzz -- --ignored --nocapture
   ```

6. Compare with Node. Both should report 0 differences:

   ```sh
   BRRRTY_NODE_MODULES=/tmp/prism/node_modules cargo test -p brrrty-prism --release --test highlight_node fuzz -- --ignored --nocapture
   BRRRTY_NODE_MODULES=/tmp/prism/node_modules BRRRTY_HIGHLIGHT_SITES=<sites> cargo test -p brrrty-prism --release --test highlight_node sites -- --ignored --nocapture
   ```

7. Update the versions in the fixtures that install the plugin
   (`tests/fixtures/highlight*/site/package.json` and their lock files),
   refresh them with `cargo xtask fixtures-refresh <name>`, and run
   `cargo test --workspace` and `cargo xtask scoreboard`.
8. Update the versions named in docs and comments:
   `git grep -n -e <old PRISM> -e <old PLUGIN>`.
