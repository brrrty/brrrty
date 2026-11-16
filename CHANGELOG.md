# Changelog

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
