# Changelog

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
