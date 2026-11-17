# Changelog

## Unreleased

- markdown-it renderer rules in Rhai. `config.amend_library("md", |md| ...)` is 11ty's `amendLibrary`, and its rules apply to every `.md` page and Markdown layout; `markdown_it(options)` makes an instance for data files and filters, rendered with `md.render(s)`. A rule is `md.renderer.rules.link_open = |tokens, idx, options, env, renderer| ...`, with tokens' `attr_get`, `attr_set`, `attr_join`, `attr_index`, `attr_push` and fields, `renderer.render_token`, `renderer.render_attrs` and `escape_html`; markdown-it's own rules (`md.renderer.rules.text`, `image`, ...) can be read and wrapped. Rules run for inline tokens, which Brrrty gives exactly as markdown-it does (checked against markdown-it 14.3.3 and 15.0.3 on generated inputs, with and without rules). A rule for a block token, reading `env` on 11ty's library, changing the token list or the options, or returning something that isn't a string is an error that names it, and a page whose rule fails is refused with `markdown-rule-failed`. `config.set_library` is an error that says it isn't ported. Rules can only be set while the config or a data file loads: changing one from a filter, a collection or a rule is an error, so output never depends on the order pages render in. A token's type can't become a block type, and `md.render` takes only a string, as markdown-it's does. `render_token` follows markdown-it 14.3.3 on 11ty's library and 15.0.3 on `markdown_it()` instances.

## 0.0.17 - 2026-11-17

- `markdown(s, options)` in Rhai is `markdownIt({ html, linkify, typographer })` with `md.linkify.set({ fuzzyLink })`: `markdown(s, #{ linkify: true, typographer: true, fuzzy_link: true })`. Linkify is linkify-it 6.1.0, run on its own patterns, and the typographer is markdown-it 15's replacements and smart quotes; both match markdown-it on 40,000 generated inputs with each set of options. A paragraph with a run of 1,000 or more characters and no space (with linkify), and a quote right next to an image whose label can't be found in the source (with the typographer), are errors instead of guesses. Unknown options are errors.
- Markdown links normalize as markdown-it's do: a host outside ASCII is punycoded (`[x](http://é.com)` links to `http://xn--9ca.com`), an autolink's text is decoded (`<http://x.y/%C3%A9>` shows `http://x.y/é`), and a URL is parsed with mdurl first (`HTTP://~:8080` gives `HTTP://:8080~`). Brrrty only percent-encoded them.
- Markdown emphasis next to a symbol follows markdown-it: `_a_😀` is `<em>a</em>😀`, in 11ty's Markdown and in `markdown()`. Each instance uses its own markdown-it's Unicode tables (uc.micro 2.1.0 for 11ty's 14.3.3, 3.0.0 for 15).
- Code spans follow each markdown-it version's rule: in ``[a`b`c` `` the first two backticks are a code span, as in markdown-it. A backslash before a space leaves the space to the line break after it, so `a\` then a space at the end of a line gives `a\` and a soft break.
- Strikethrough follows markdown-it with three or more `~` in a row: `~~a~~~` is `<s>a</s>~` and `a ~~~~---~~` is `a ~~<s>---</s>`. Emphasis or strikethrough across a CRLF line break inside a list item or a blockquote no longer panics in a debug build (it refused the page with `markdown-render-failed`).
- Emphasis and strikethrough are paired as markdown-it pairs them (its delimiter list, `balance_pairs`, and post-processing). The port's own matching left some markers literal: `*Set `DEBUG` in settings_* then _restart_.` now has `<em>restart</em>`, and `*b~~*~~x~~` has `<s>x</s>`.
- In 11ty's Markdown, link URLs encode an IPv6 host's brackets, as markdown-it 14.3.3 does (`[x](http://[::1]:8080/)` links to `http://%5B::1%5D:8080/`); `markdown()` keeps them, as 15 does.
- Image `alt` text is markdown-it's: entities, escapes, line breaks, autolinks and inline HTML count, and code spans count in 15 but not in 11ty's 14.3.3 (`![Output of `ls -la`](a.png)` has `alt="Output of "` on an 11ty page). An image's label is parsed as its own text, as markdown-it does.
- A `<pre>`, `<script>`, `<style>`, `<textarea>` or comment in a list item keeps everything up to its closing tag across a blank line, as in markdown-it; Brrrty ended it at the blank line and wrote the rest as paragraphs.
- In a tight list item, a fence, an indented code block or an HTML block right after the item's text follows it with no line break, as markdown-it writes it. A last line of only spaces with no newline after it is ignored, as markdown-it ignores it.
- Tables follow markdown-it's `table` rule: a one-column table (`| Name |` then `---`), a header with an escaped `\|`, and the lines that end a table.
- `markdown(s, options)` with `linkify` or `typographer` no longer refuses a tight list item that holds a block (a fence, a heading, a quote).
- Markdown that Brrrty's parser is known to read differently from markdown-it is refused (an 11ty page with `markdown-render-failed`, a `markdown()` call with an error) instead of written: a reference definition whose destination ends in `\`, a list marker alone on a blockquote's lazy line, a table right after a blockquote's paragraph, a code span or inline HTML across an indented lazy line in a blockquote, and a tab after a blockquote's `>` where the parser counts its columns differently (in HTML blocks, fenced and indented code, and nested list items). The checks look at the parsed blocks, so code blocks, HTML blocks and link titles are never refused. PORTING.md lists them.
- In 11ty's Markdown, inline HTML follows markdown-it 14.3.3: `<!-->`, `<!--->` and a comment with `--` inside are HTML, and after a comment, processing instruction, declaration or CDATA section that is never closed, no more are taken in the same paragraph. A fence or HTML block that runs to the end of a file with no final newline no longer gets one added.

## 0.0.16 - 2026-11-17

- In `brrrty.config.rhai`, a constant number inside `${...}` prints as JavaScript prints it, as a variable already did: `${10.0}` and `const PER_PAGE = 10.0; `${PER_PAGE}`` give `10`, not `10.0`, and `${1e21}` gives `1e+21`. Rhai's optimizer folded constants into the string with Rhai's own number formatting; it is now off for config and data scripts.
- Rhai scripts get JavaScript's number formatting and string order, each checked against Node: `to_fixed(n, digits)` is V8's `toFixed`, rounding the number's exact value with ties away from zero (`to_fixed(1.005, 2)` is `"1.00"`, `to_fixed(2.5, 0)` is `"3"`); `to_locale_string(n)` is `toLocaleString('en-US')` (`1,234.568`); `locale_compare(a, b)` is `localeCompare` with the `en-US` locale. Other locales and options are refused by name, and so are Han characters, which Node sorts by radical and stroke count.
- Rhai scripts can build local dates as `new Date(y, m, d, h, mi, s, ms)` does: `date(2026, 4, 10)`, with `get_full_year()`, `get_month()`, `get_date()`, `get_day()`, `get_hours()`, `get_minutes()`, `get_seconds()`, `get_milliseconds()`, `get_timezone_offset()` and `to_iso_string()`.
- `random()` in a Rhai script is an error that explains why: a random value can never match 11ty's output. `config.add_nondeterministic_filter(name)` stays the way to stand in for a filter that shuffles.
- In Nunjucks templates, a number's `toLocaleString()` and a string's `localeCompare()` run as in Node with the `en-US` locale; another locale or options refuse the page.
- With no locale argument, `toLocaleString()` and `localeCompare()` (and `to_locale_string`, `locale_compare`) run only when Node's default locale would be `en-US`: the first of `LC_ALL`, `LC_MESSAGES` and `LANG` that is one Node maps to `en-US` (`en_US`, `C` or `POSIX`, with any codeset and no `@modifier`), or none is set. Otherwise they refuse, naming the variable.
- Local time uses the tz database Node 26 ships (2026b), built into Brrrty, instead of the computer's: with newer data on the machine (2026c makes Alberta UTC-6 from November 2026), local dates in a changed zone were an hour off. Luxon's `luxon_format` uses the same data, so zones with daylight saving work after 2099 instead of being refused.
- The local zone is found as Node's ICU finds it: `TZ`, else where `/etc/localtime` links to, else (a copied `/etc/localtime`, as in many Docker images) the zone file under `/usr/share/zoneinfo` with the same bytes, else UTC when there is none. A `TZ` that names no zone in the bundled data (a typo, a trailing space, a POSIX rule), a broken link, or a copy that matches no zone file stops the build with `tz-invalid`, where Node would quietly use UTC or guess.
- `String(n)` now prints the even digits when a number is exactly halfway between two shortest forms, as V8 does: `9302184022144.5625` prints `9302184022144.562`, not `...563`. Numbers in templates and Rhai print this way too.
- Local dates past the year 250,000 or before -250,000 use the time zone's offset (they used UTC's), so their hours and days match Node out to the end of JavaScript's date range.

## 0.0.15 - 2026-11-17

- Data files and the config can read files in the project: `read_text(path)` is `fs.readFileSync(path, 'utf8')`, `read_json(path)` is `JSON.parse` of it, `read_dir(path)` and `read_dir(path, #{ recursive: true })` are `fs.readdirSync` in Node's order, `file_exists(path)` is `fs.existsSync`, and `parse_csv(text, options)` is csv-parse 7.0.3's `parse`, quirks and error messages included, for the options `columns`, `delimiter`, `bom`, `trim`, `ltrim`, `rtrim`, `skip_empty_lines` and `relax_column_count`. Other csv-parse options are refused by name.
- Paths are relative to the project folder and can't leave it: an absolute path and a `..` are refused, and every path is opened through a handle on the project folder one component at a time (cap-std), so a symlink that leads outside is refused even if it is swapped in mid-build. Recursive listings enter symlinked folders as Node 26 does, refusing one that leads outside or loops. Only regular files are read (a FIFO is refused, not waited on), a file over 16 MiB is refused unread, and nothing writes.
- `brrrty serve` rebuilds when a file a script read changes, appears or goes away, including outside the input folder and under `node_modules`, when a folder a script listed gains or loses a file, and when a symlink on the way to a read is retargeted.
## 0.0.14 - 2026-11-17

- `brrrty doctor` lists everything that keeps pages from building, without writing anything. A build stops each page at its first problem; doctor keeps going and lists them all: every filter that a page's template, its layouts, the includes they name literally, its `permalink` and its `eleventyComputed` use that isn't defined (in every branch), every JavaScript data file they read, a page's own problem that a layout Brrrty can't render hid, and template languages Brrrty doesn't run. Problems are grouped by cause, with the pages and templates each one refuses, every file and line, and a fix. A page refused only because it shows the content of a refused page is listed under that page's blockers ("also blocks N pages that use its content"). A cause that stops a template's pagination before it makes its pages comes first, and every count that includes such a template says "at least". Unknown tags (shortcodes) are listed one per file, as the build reports them. The verdict reads `N of M pages will build; K blockers, grouped below.` `--json` prints the same as one object. Exit status 0 when every page builds, 1 when something is refused, 2 for usage errors.
- `brrrty doctor --verify` also builds the site with the project's own 11ty (`node_modules/@11ty/eleventy`) and with Brrrty, in the system temp folder, at one frozen time (`SOURCE_DATE_EPOCH`, or now), and compares every file with `brrrty-compat`. It prints the byte-identical, different, missing and extra counts, the first diffs, both timings and a line to paste: `Your site is byte-compatible on Brrrty: 2,272 files, 0.89 s vs 2.39 s`. It runs the site's JavaScript through 11ty, so it says so first, asks on a terminal and needs `--yes` off one. When Node or 11ty is missing, it says how to install it. Ctrl-C or SIGTERM stops 11ty and everything it started, removes the temporary folders and exits 130 or 143.

## 0.0.13 - 2026-11-17

- `brrrty init` sets up an existing 11ty site in one step: `npx brrrty init`. It adds `brrrty` to `devDependencies` at its own version and sets the `build` and `start` scripts to `brrrty build` and `brrrty serve`. Scripts that are just an 11ty command (`eleventy` or `npx @11ty/eleventy`, with flags) are kept as `build:11ty` and `start:11ty`; any other script is left alone, with a note saying what to change, down to which step of a script like `npm run css && eleventy` to swap. The rest of `package.json` keeps its indentation, line endings, key order and byte order mark, and the edit is checked to change nothing else.
- `brrrty init` also writes `brrrty.config.rhai` from `eleventy.config.js` (`.mjs`, `.cjs` or `.eleventy.js`), reading it without running it. Folders, `markdownTemplateEngine` and `htmlTemplateEngine`, passthrough copies and ignores with literal paths, and the syntax highlighting and RSS feed plugins with literal options are ported. Every other statement gets a `// TODO:` that quotes the JavaScript with its line numbers and links to the docs, and so does anything it can't be sure runs as written (code after a `return`, a name declared twice); a file it can't be sure of becomes a single TODO and no setting is ported: one that exports twice, has `return` before a line break, passes the config object to code it can't port (a helper, an `if`, a loop), or has a `/` that could be read as either a regular expression or division. It reads only regular files inside the site's folder, up to 1 MiB. It lists JavaScript data files that still need a `.rhai` port, and prints the next steps. It never overwrites `brrrty.config.rhai`, writes only `package.json` and `brrrty.config.rhai` in the site's folder (each atomically, and never through a symlink that leads outside it), and doesn't run `npm install`.
- `brrrty init` shows its changes as a diff and asks before writing. `--yes` skips asking, `--dry-run` writes nothing, and without a terminal to ask in it writes nothing unless given `--yes`. `--dir` points it at another folder. A folder that isn't an 11ty site exits with status 2.
- A bare `brrrty` in an 11ty site without `brrrty.config.rhai` prints a one-line hint to run `npx brrrty init` before building as before.

## 0.0.12 - 2026-11-17

- npm releases are published with npm trusted publishing: the release workflow proves its identity to npm through GitHub's OIDC token, so no npm token is stored in the repository.

## 0.0.11 - 2026-11-17

- With the syntax highlighting plugin, a page highlighted in a language that another page could change (a `rust` fence, say) is no longer refused because of `{% highlight %}` text that no template language reads. The check that runs before the build used to count such tags in every file, and treated any file mentioning `njk` as Nunjucks, so a Markdown changelog quoting `{% highlight js 1,3 %}` in inline code, with `markdownTemplateEngine: false`, refused every `rust` page. It now decides what reads each file the way the build does (its extension, its front matter's `templateEngineOverride`, and `markdownTemplateEngine` and `htmlTemplateEngine`, `false` included) and counts a tag only where Liquid or Nunjucks reads it and the plugin gives that language the tag. Where that can't be known before the build (data that could set the override, a file a template could include, from an include tag or a `{% liquid %}` tag's statements (nested ones too), or name as its layout, data files and `_includes`), it still counts the tag, so a page that 11ty would highlight differently is never written.
- A `{% highlight %}` tag with a line break after `{%` or inside its arguments, and a `highlight` statement in a `{% liquid %}` tag, count in that check too, as 11ty runs them. It used to be missed, and when the page with it was refused before reaching it, a page after it could be written with a grammar 11ty had changed.
- `date: Last Modified` is the file's change time (`ctime`), as in 11ty, not its modification time. They differ after anything that sets the modification time back or leaves it alone while changing the file's metadata, such as `touch -m`, a restore that keeps times, or `chmod`.
- When a `_data` file fails, its error now prints before the problems of the pages it stops, which say to fix the error "reported above". Before, a page whose path sorted before the data file (`src/About.md` before `src/_data/site.json`) printed first.
- The build summary says `Wrote 1 file` and `copied 1 file`, not `1 files`.

## 0.0.10 - 2026-11-17

- Brrrty is on npm: `npm install --save-dev brrrty`, then `brrrty build` in your scripts. The `brrrty` package is a small launcher that runs a prebuilt binary from `@brrrty/darwin-arm64`, `@brrrty/darwin-x64`, `@brrrty/linux-arm64-musl` or `@brrrty/linux-x64-musl`, whichever npm installed for the platform. No install scripts and no Rust toolchain needed. The Linux binaries are static, so they run on Alpine and glibc distributions alike. Arguments, output and exit codes pass through unchanged, and Ctrl-C under `npm run` or `npx` stops `brrrty serve` cleanly with exit 0. Ctrl-\\ and every other terminating signal Node can catch are passed on too, and `brrrty serve` stops cleanly by itself if the launcher is killed outright, so it never stays behind holding its port.
- Each release attaches binaries for those four platforms, with SHA-256 sums, to its GitHub release.
- Every npm package and release tarball carries NOTICE, with the copyright and license notices of the projects Brrrty ports code and data from, next to LICENSE.
- On Linux, a page without a `date` is dated by its file's creation time whatever the C library, as 11ty does. The static musl build used to fall back to the modification time, so a page edited after it was created got a different date (and a different place in date-sorted collections) than in 11ty. Where the creation time is missing, Brrrty now does what 11ty does there too: a file system that doesn't record it dates the page (and `date: Last Modified`) by the build time, and a system where `statx` is unavailable uses the file's change time.

## 0.0.9 - 2026-11-17

- A build into an empty or new output folder is about 20% faster on macOS. On APFS, many threads creating files under the same folders fight over folder locks, so pages and passthrough files are now written by 4 threads of their own, and passthrough files are copied in parallel on every system. Passthrough copies whose destinations overlap (a file and a folder mapped to one path, or names that differ only by case or Unicode normalization on macOS) still run one at a time in walk order, so the output and the problems reported are the same as before.
- Passthrough copies keep their source's access and modification times on every system, as 11ty's copy does, so rsync and other deploy tools that compare times no longer re-upload every passthrough file after a build on Linux. On macOS, copies already kept them. Rendered pages get the time they are written, as in 11ty.

## 0.0.8 - 2026-11-17

`brrrty serve`:

- `brrrty serve` builds the site, serves it, and rebuilds on every change, as `eleventy --serve` does. It takes `--port` (8080, moving up to 10 ports on when it is taken), `--host` (`localhost`), `--input`, `--output`, `--passthrough`, `--quiet` and `--strict`. It prints `Server at <url>`, `File changed: <path>` and `Watching…` as 11ty does.
- Live reload: pages get a script that patches in an edit to the page's own template, swaps an edited stylesheet, and reloads for anything else, and logs build errors in the browser console, as 11ty's dev server does.
- The output folder is kept up to date after every build, from the build's own list of what it wrote, so an unchanged file is never read again. Passthrough sources reached through a symlink are copied again on every edit. Builds are made in `<output>/.brrrty-serve/`, which is never served and is removed on exit; `brrrty build` removes one a killed session left behind.
- A running session holds a lock on `<output>/.brrrty-serve/` (an OS file lock, with its process id in the file). `brrrty build` into the same output folder leaves it alone and prints a `serve-staging-in-use` note, and a second `brrrty serve` there refuses to start. The OS releases the lock when the process ends, so a killed session's folder is still cleaned up.
- When no build ever fills the output folder (the first build failed, as under `--strict`), a folder the session created is removed on exit, as 11ty leaves none.
- Ctrl-C stops cleanly and exits 0, with the last good build in the output folder and the port released. A second Ctrl-C quits at once. SIGTERM stops the same way and then ends by the signal.
- `eleventy.env.runMode` is `"serve"` while serving.

Where `brrrty serve` differs from `eleventy --serve`:

- It listens on `localhost` only, unless `--host` says otherwise, and on that address answers only requests whose `Host` names this machine.
- It never serves a file outside the output folder: `..` in any encoding and symlinks that point outside get a 404, and malformed URLs a 400.
- A build with errors in some pages is still served, and its pages that built are written to the output folder, as `brrrty build` writes them. A page whose template has an error keeps the version last served (the rebuild says how many were kept), and the error shows in the browser console; a page that never built is a 404. A first build with errors keeps the server running. `--strict` serves no build with an error and exits with status 1 when the first build has one, as 11ty does.
- Any change under the input folder, a deleted file included, starts a rebuild.
- The live-reload socket refuses pages from other sites.
- Paths under `/.brrrty/` (`reload-client.js`, `morphdom.js`, `live`) are reserved while serving, as `/.11ty/` is in 11ty.
- `--quiet` prints only the `Server at` line, errors and warnings; 11ty's also prints the `Wrote` line, `Watching…` and changed files.
- An added file is reported as `File changed:` (11ty: `File added:`), and a changed file outside the project folder by its full path. When the port is taken, the `Server at` line adds `(port N was in use)`.
- `setServerOptions` and `serverPassthroughCopyBehavior` have no equivalent.

Building Brrrty now needs Rust 1.89 or later.

## 0.0.7 - 2026-11-17

Nunjucks:

- `.njk` templates are built, with a Nunjucks 3.2.4 engine configured as 11ty 3.1.8 configures it: autoescaping on, `_includes` then the project folder as include roots, and `./` names relative to the including template. This replaces the refusal for `.njk` files.
- Pages, layouts and includes can be Nunjucks, in any mix with Liquid and Markdown: a `.md` page in an `.njk` layout, and the reverse. An include of any file, such as `.js` or `.css`, is rendered as Nunjucks, as in 11ty; one without tags comes out byte for byte. Permalinks and computed data in `.njk` pages are Nunjucks, and `markdownTemplateEngine` and `htmlTemplateEngine` can be `"njk"`.
- Expressions follow JavaScript: truthiness, `==` and `===`, `+` on strings and numbers, how `undefined` and `null` print, and the string, array, number and date methods a template calls. `if`, `for` (with `else`, `loop` and objects), `set`, `macro`, `call` and `caller`, `import` and `from`, `include` (with `ignore missing`), `extends`, `block` and `super()`, `raw`, `verbatim`, `filter` and `switch` work, with whitespace control (`{%-`, `-%}`).
- Every Nunjucks built-in filter, 11ty's filters, the RSS plugin's filters and the config's Rhai filters run in Nunjucks. A `SafeString` stays safe through filters and macros as it does in Nunjucks.
- Regular expression literals (`r/.../gi`) work with `replace`, `replaceAll`, `split`, `test`, the `replace` filter and printing. They match as JavaScript's do, with the same rules as `regex_replace` in scripts.
- What Brrrty can't reproduce exactly is refused with the reason, never approximated: a method with no exact emulation, `random`, a `sort` or `dictsort` whose order would depend on V8's sort, async filters and tags, 11ty's `setAsync`, `getBundle` and `getBundleFileUrl`, printing a function (Nunjucks prints its JavaScript source), the syntax highlighting plugin's Nunjucks `{% highlight %}` shortcode, a `SafeString` passed to a config (Rhai) filter or to an RSS filter that every engine has (JavaScript would pass an object, not a string), options to `slug` and `slugify`, and a time zone argument to `dateToRfc822`.
- The engine has the Liquid engine's safety limits: nesting depth, loop and output size, and a parser that never backtracks. A crash in it refuses the page instead of stopping the build.
- A render may run 3,000,000 loop iterations in all, across nested loops, includes and macros, so nested loops are stopped in seconds.
- A filter that lists a collection item's properties (`urlencode`, `dictsort`, `list`, `dump`) is refused, as `for k, v in item` is: 11ty prints its `template` object and content there. Counting or testing the names in `collections` (`length`, `in`, `dictsort`) is refused when the site has an 11ty config that may add collections.

Config:

- `git_head()` gives the commit `git rev-parse HEAD` prints, for a config or data script that ports `execSync("git rev-parse HEAD")`. It reads the repository's files (`HEAD`, loose refs, `packed-refs`, and a linked worktree's `.git` file and `commondir`) from the project folder up, and starts no process. Outside a repository it is an error, which `try`/`catch` can turn into a default.
- A script filter that lists the names in `collections` (`keys()`, `len()`, `contains()`, `for k in`) under an 11ty config, or changes a collection item, is refused.
- `map`, `filter` and `reduce` with an anonymous function run about twice as fast, and `regex_split` with a pattern without groups is faster.
- The regular expression matcher behind `regex_replace`, `regex_split` and `regex_test` moved to `brrrty-jsregex`, where Nunjucks uses it too.

CLI:

- `brrrty build 2>&1 | head -1` no longer aborts with exit code 134 when the pipe closes: printing stops and the build's own exit code stands.

Syntax highlighting:

- Markdown front matter inside a fence is written highlighted as YAML when a page 11ty renders before it loaded YAML, and refused, as before, when none does. Brrrty replays the order 11ty renders pages' content in.
- The scan for languages that change grammars counts a Nunjucks `{% highlight %}` whose argument isn't one string, and any `{% highlight` whose arguments start on the next line, as any language.

## 0.0.6 - 2026-11-17

Syntax highlighting:

- Brrrty now highlights code the way `@11ty/eleventy-plugin-syntaxhighlight` does. Fenced code blocks in Markdown, the `{% highlight %}` Liquid tag and the plugin's options (`lineSeparator`, `errorOnInvalidLanguage`, `alwaysWrapLineHighlights`, `preAttributes`, `codeAttributes`, `trim`, `languages`, `templateFormats`) give the same HTML, byte for byte. `config.add_plugin("syntaxhighlight")` takes them as a map. This replaces the refusal for pages with a fenced code block or a `{% highlight %}` tag.
- It is a port of Prism 1.30.0 itself: the grammars' 464 distinct regular expressions are translated to Rust regular expressions that match as JavaScript's do, and Prism's tokenizer, hooks and line highlighting (`` ```js/1-2,4 ``, `{% highlight js 1,3 %}`) and `diff-` languages run as they do in Node.
- Brrrty has the grammars of 28 of Prism's 297 languages, the ones the test sites use: `bash` (`sh`, `shell`), `c`, `clike`, `css`, `diff`, `go`, `ini`, `javascript` (`js`), `json` (`webmanifest`), `jsx`, `less`, `liquid`, `lua`, `markdown` (`md`), `markup` (`html`, `xml`, `svg`, `mathml`, `ssml`, `atom`, `rss`), `markup-templating`, `nginx`, `perl`, `php`, `python` (`py`), `r`, `ruby` (`rb`), `rust`, `sql`, `toml`, `typescript` (`ts`), `vim` and `yaml` (`yml`), plus Prism's `plain` (`plaintext`, `txt`).
- A name that isn't a Prism language or alias, such as `conf`, `zsh`, `console`, `terraform` or `JS`, is written as the plugin writes it: the code as it is, with a `language-x` class. So is `text`. With `errorOnInvalidLanguage`, such a name other than `text` fails the page with `syntax-highlight-failed`, where 11ty stops the build.
- Highlighting is never approximated. A page whose highlighting Brrrty can't reproduce exactly is refused with the reason in a `syntax-highlight-unsupported` problem:
  - a fence or `{% highlight %}` in one of the other 269 Prism languages, such as `java`, `kotlin`, `swift`, `cpp`, `csharp`, `scss`, `graphql`, `docker` or `shell-session`, or `njk`, which the plugin highlights as `django`;
  - a page highlighting with a grammar that a language on another page changes. In 11ty, loading `regex`, `css-extras`, `js-extras`, `js-templates`, `jsdoc`, `javadoclike`, `phpdoc` or `php-extras` (among 21 such languages) changes the grammars of `javascript`, `css`, `php`, `markup` and what is built on them for every page rendered after. Brrrty scans every file under the input folder before it builds, Nunjucks pages and pages it refuses for other reasons included, and refuses the pages those grammars would highlight, naming the file and line that loads the language. A language that reaches the highlighter only as pages render, such as a fence built from data, is caught too: every page renders before any is written, and if one meets such a language the build is redone refusing what it changes. The list comes from Prism's `components.json` and from loading each language in Node;
  - a pattern that would backtrack more than a budget allows (on hostile input, reaching the budget can take a few seconds), and Markdown with a fenced block or front matter inside a fence, which 11ty highlights differently depending on the order it rendered pages in.

  `syntax-highlight-failed` is for where the plugin itself throws, in a fence or a `{% highlight %}` tag.
- Fixed: with the `code` rule off, Markdown still stops treating lines indented 4 or more spaces as continuing a list item, as markdown-it does.
- Building is faster for pages with code in languages with long keyword lists, such as SQL: translating a case-insensitive pattern no longer reads Prism's whole case table for every letter.

## 0.0.5 - 2026-11-16

Config written in Rhai:

- `brrrty.config.rhai` at the project root is Brrrty's version of `eleventy.config.js`. When it exists, Brrrty reads it and ignores the JavaScript config. It sets `dir`, passthrough copy, `ignores`, global data, filters, collections and plugins. `PORTING.md` in `brrrty-config` shows how to port a JavaScript config, call by call.
- Filters (`config.add_filter`) get their arguments the way 11ty passes them to a JavaScript filter, keyword arguments included. A filter that fails refuses only the page that used it, where 11ty stops the build.
- Collections (`config.add_collection`) get 11ty's collection API: `get_all`, `get_all_sorted`, `get_filtered_by_tag`, `get_filtered_by_tags` and `get_filtered_by_glob`, with 11ty's glob matching. Items are read-only.
- Helpers for what JavaScript does differently from Rhai: `object()` keeps keys in the order they were added, `truthy`, `js_string`, `js_concat`, `js_div`, `js_round`, `js_sort`, `js_sort_by`, `json_stringify`, and arrays get JavaScript's `sort`, `reverse` and `slice`. Sorting runs V8's own TimSort, so even a comparator that isn't a consistent ordering, such as `|a, b| a.date > b.date`, gives the order Node gives, at any length. `regex_replace`, `regex_split` and `regex_test` take JavaScript patterns.
- `luxon_format(date, format, zone)` formats dates as Luxon does, and `markdown(s)` and `markdown_inline(s)` render as `markdownIt()` does.
- Data files can be Rhai scripts: `_data/name.rhai`, `page.11tydata.rhai` and `folder.11tydata.rhai`. A script's last expression, or its `fn data(global)`, is the data. A `.rhai` file replaces a JavaScript data file of the same name, with a note saying so.
- Two global data files for one key (`a.json` and `a.rhai`) merge, as in 11ty.
- `config.add_plugin("rss", #{ ... })` is 11ty's RSS feed plugin: Atom, RSS and JSON feeds, with its filters for Liquid.
- `config.add_plugin("syntaxhighlight")` marks a site that uses 11ty's syntax highlighting plugin. Brrrty can't highlight yet, so pages with a fenced code block that names a language, or a `{% highlight %}` tag, are refused instead of written without highlighting.
- `SOURCE_DATE_EPOCH` sets the time for Rhai's `now()` and for Liquid's `'now'` and `'today'`.
- Scripts are limited in operations, time, call depth and size (what `object()`s hold included), and can't capture variables in closures or read the real clock through Rhai's `timestamp()`, so a script can't hang the build or give different output from one run to the next. Errors point at the script's file, line and column.
- A filter written in Rhai that would be given a Liquid `nil`, `null`, `empty`, `blank` or `forloop` refuses its page: 11ty passes LiquidJS's own objects for those, and no script value acts the same.

Regular expressions in scripts accept only what means the same in JavaScript and Rust. Text with emoji or other characters outside the BMP works with patterns where no match can stop between the two halves of such a character, such as `(<([^>]+)>)` or `\s+`. A pattern that could, or a repeated group that can match nothing, such as `(a*)?`, is refused with the reason.

A Markdown render that crashes refuses its page with `markdown-render-failed` instead of stopping the build. A crash while running a script is a problem too: `config-error` at the config's top level, `data-rhai-error` in a data file, and a refused page in a filter or collection.

A feed whose HTML nests elements more than 2000 deep is refused with `feed-failed`, where it used to crash Brrrty (11ty's posthtml runs out of stack a little deeper and stops the build).

Fenced code blocks whose language is only characters JavaScript's `trim` keeps, such as U+0085, count as naming a language, as in markdown-it.

New errors:

- `config-syntax`, `config-error`, `config-invalid`, `config-limit`, `config-unreadable` and `config-unsupported`: a problem in `brrrty.config.rhai`, with its line and column.
- `data-rhai-syntax`, `data-rhai-error` and `data-rhai-limit`: a problem in a `.rhai` data file. Only the pages that use its data are refused.
- `data-rhai-shadows-js` (a note): a `.rhai` data file replaces a JavaScript one.
- `syntax-highlight-unsupported`: a page needs syntax highlighting.
- `feed-failed` and `feed-unsupported`: a feed that can't be built, or one Brrrty doesn't support yet, such as an unknown option or a feed path that is also a file.
- `passthrough-unsupported`: a passthrough copy Brrrty can't do yet, such as a glob.
- `source-date-epoch-invalid`: `SOURCE_DATE_EPOCH` isn't a whole number of seconds.
- `markdown-render-failed`: Markdown rendering crashed on a page.

## 0.0.4 - 2026-11-16

Data, permalinks, pagination and collections:

- JSON data files in `_data` load as 11ty loads them, including folders of data files and symbolic links. JavaScript data files (`.js`, `.cjs`, `.mjs`) don't run yet: only the pages that use their data are refused.
- Template and directory data files (`page.json`, `page.11tydata.json`, `folder/folder.json` and the like) apply to their pages, and the whole data cascade merges deeply in 11ty's order: global data, layouts, data files, then front matter.
- `eleventyComputed` values are rendered in 11ty's dependency order.
- Permalinks can be Liquid templates, and `permalink` can be computed.
- `pagination` works over lists and objects, with `size`, `alias`, `reverse`, `filter`, `resolve: values` and `generatePageOnEmptyData`. Any `size` 11ty accepts works, such as `'3'` or `[2]`.
- `collections` are built from `tags`, with `collections.all`, `eleventyExcludeFromCollections` and `addAllPagesToCollections`, in 11ty's order, including the order `eleventyImport.collections` sets. Pages can paginate a collection.
- `templateContent` and `content` of collection items work, including pages that read each other's content, rendered in the order 11ty renders them.
- Nunjucks templates and pages under a Nunjucks layout are collection items. Their own pages aren't written yet, but pages that list them build.
- Front matter YAML reads values the way 11ty's YAML loader does: dates, `0123` as 123, `.inf`, and numbers too large for JavaScript stay strings.
- A file named like its folder (`posts/posts.md`) is written as the folder's `index.html`, as in 11ty.
- A `date` left empty, `""`, `0` or `false` counts as no date, as in 11ty: the page takes the date in its path, or its file's date. `Last Modified` and `Created` match in any case.
- `json` and `inspect` of collection items stop at once with a problem, as 11ty does, instead of working through every item first.

Where 11ty stops the whole build, Brrrty refuses only the page or data file at fault and builds the rest: a bad data file, pagination over missing data, two pages with one permalink, a `templateContent` circle, or a computed data loop. A page whose date Brrrty can't read is refused with the pages that read a collection it is in; other collections still work.

Front matter whose YAML aliases (`*name`) expand to more than a million values is refused as `front-matter-invalid`, at the alias that goes over, instead of taking the build down.

New errors:

- `data-file-invalid`: a data file that can't be read or parsed.
- `data-js-unsupported`: a page uses data from a JavaScript data file, which Brrrty can't run yet. Pages that don't use it still build.
- `computed-cycle` and `computed-unsupported`: computed values that depend on each other in a loop, or a computed key Brrrty can't set yet, such as `layout`.
- `pagination-invalid` and `pagination-unsupported`: pagination 11ty can't build, or that Brrrty can't follow yet.
- `content-cycle` and `content-unavailable`: a page reads `templateContent` in a circle, or content that 11ty wouldn't have rendered yet.
- `tags-invalid`: `tags` that are neither a string nor a list.
- `permalink-unsupported`: `permalinkBypassOutputDir`, which would write outside the output folder.

Removed:

- The `permalink-templated` error. Templated permalinks now render.

## 0.0.3 - 2026-11-15

Liquid templates and layouts:

- `.liquid`, `.md` and `.html` pages render through Liquid, as 11ty does by default. `.md` pages then go through Markdown. The Liquid engine matches LiquidJS as 11ty configures it, filters included, plus 11ty's `slug`, `slugify`, `url` and `log`.
- `{% include %}` and `{% render %}` find files in `_includes`, then in the input folder. A name can't lead outside those folders, through `..`, an absolute path or a symbolic link.
- The front matter `layout` key works: layouts are looked up in `_includes` with 11ty's extension order, can have their own `layout`, and their front matter merges under the page's. `.md` layouts run Markdown too.
- Pages get 11ty's `page`, `eleventy` and `pkg` data, and `templateEngineOverride` is honored.
- Markdown is rendered by a port of markdown-it, configured like 11ty's.
- Dates print in the machine's local time zone, as in 11ty, and `TZ` is honored. Set `TZ` to get the same output on every machine.

New errors:

- `liquid-syntax`, `liquid-unknown-filter`, `liquid-render` and `liquid-limit`: a Liquid problem, reported in the file it comes from (the page, a layout or an include) at that file's line and column. The rest of the site still builds.
- `layout-missing`, `layout-cycle`, `layout-invalid` and `layout-unsupported`: a layout that can't be found, a chain of layouts that loops (reported once, naming every layout in the loop), a layout that isn't a template file, and a Nunjucks or JavaScript layout.
- `data-unsupported`: a page uses something Brrrty doesn't read yet, such as `_data` files, template and directory data files, or `collections`. The page is not written, instead of being written without that data.
- `data-reserved`: data sets a name 11ty sets itself, such as `content` or `page.url`.
- `template-engine-override-invalid` and `template-engine-unsupported`: a `templateEngineOverride` that isn't valid, or that names Nunjucks.

Removed:

- The `template-syntax-unsupported` error. Template syntax in `.md` and `.html` pages now renders.

## 0.0.2 - 2026-11-15

Behavior change:

- Ignore files now follow 11ty's rules. Each line in `.gitignore`, `.eleventyignore` and `<input>/.eleventyignore` is a path from that file's own folder, not a gitignore pattern that matches at any depth. So a root `.gitignore` line `drafts` ignores `./drafts` but not `src/drafts`. A new `ignore-line-anchored` warning points at lines that would have matched under the old rules, with the full path to write instead.

Changes:

- `<input>/.eleventyignore` is now read.
- Symlinked files and folders in the input folder are followed and built at the link's path.
- Page dates and slugs come from the whole path, like 11ty: a date in a folder name counts, and `index` takes its slug from its folder. An impossible path date like `2026-13-45` is a `date-invalid` error for that page, and the rest of the site still builds.
- Permalinks with no file extension are allowed when the page sets `eleventyAllowMissingExtension` to a truthy value, and for `_redirects`, `.htaccess` and `_headers`.
- Two passthrough sources that copy to the same output file are now a `passthrough-collision` error.
- An input folder outside the project root, through `--input ../x` or a symlink, now builds and honors its `.eleventyignore`.
- Problem paths are shown relative to the project root.

New warnings:

- `ignore-line-anchored`: an ignore line matches nothing where it is anchored, but would have matched pages as a gitignore pattern.
- `ignore-line-invalid`: an ignore line is not a valid pattern, so it is skipped and the build goes on.
- `template-plugin-only`: files such as `.webc`, `.pug` or `.mdx` are skipped because they are templates only when a plugin adds them.
- `symlink-loop` and `symlink-broken`: a link that points back into its own folder, or at a missing file, is skipped.

## 0.0.1 - 2026-11-15

First release: builds plain Markdown and HTML sites the way 11ty does. Templates coming next.
