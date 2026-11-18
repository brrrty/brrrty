---
title: Templates
order: 3
summary: Liquid, Nunjucks, Markdown and HTML, the filters each has, layouts, data, and what Brrrty refuses.
description: Template languages, filters and shortcodes in Brrrty.
lead: Brrrty renders the template languages 11ty has built in, configured the way 11ty 3.1.8 configures them, and mixes them the way 11ty does.
---

| Files | Rendered as |
|---|---|
| `.liquid` | Liquid (LiquidJS 10.30.1, as 11ty sets it up) |
| `.njk` | Nunjucks 3.2.4, as 11ty sets it up |
| `.md` | Liquid, then Markdown. `markdownTemplateEngine` can make it Nunjucks or nothing. |
| `.html` | Liquid. `htmlTemplateEngine` can make it Nunjucks or nothing. |
| `.11ty.js`, `.11ty.cjs`, `.11ty.mjs` | Not yet: refused with `template-unsupported` |
| `.webc`, `.pug`, `.hbs`, `.mustache`, `.ejs`, `.haml`, `.jsx`, `.tsx`, `.mdx` | Skipped with a warning: in 11ty these are templates only when a plugin adds them |

Pages, layouts and includes can be in any mix: a Markdown page in a Nunjucks layout, a Liquid layout around a Nunjucks page. `templateEngineOverride` works as in 11ty. Markdown is a port of markdown-it, set up like 11ty's, with HTML allowed.

## What works across all of them

- **Layouts.** `layout` in front matter or data, looked up in the layouts folder (`_includes`, or `dir.layouts`/`dir.includes`) with 11ty's extension order, after `add_layout_alias` aliases. Layouts can have their own layout, and their front matter merges under the page's.
- **The data cascade.** Global data from `_data` (JSON and `.rhai`), layout data, folder and template data files, front matter and `eleventyComputed`, merged deeply in 11ty's order. Pages get `page`, `eleventy`, `pkg` and `collections`.
- **Permalinks**, including templated and computed ones.
- **Pagination** over lists and objects, with `size`, `alias`, `reverse`, `filter`, `resolve: values` and `generatePageOnEmptyData`.
- **Collections** from `tags`, `collections.all`, `eleventyExcludeFromCollections`, and the order `eleventyImport.collections` sets. `templateContent` and `content` of collection items work, including pages that read each other's content.
- **Dates** print in the machine's local time zone, as in 11ty. Set `TZ` to get the same output on every machine.

## Liquid

The tags are LiquidJS's: `assign`, `capture`, `case`, `comment`, `cycle`, `decrement`, `echo`, `for` (with `break`, `continue` and `else`), `if`, `include`, `increment`, `layout` and `block`, `liquid`, `raw`, `render`, `tablerow` and `unless`.

The filters are LiquidJS's standard set: string filters such as `append`, `capitalize`, `downcase`, `escape`, `remove`, `replace`, `split`, `strip_html`, `truncate` and `truncatewords`; array filters such as `concat`, `first`, `group_by`, `join`, `map`, `reverse`, `sort`, `sort_natural`, `uniq`, `where` and their `_exp` forms; math filters such as `plus`, `minus`, `times`, `divided_by`, `round` and `at_most`; and `date`, `default`, `json`, `url_encode` and the rest.

`{% include %}` and `{% render %}` find files in the includes folder (`_includes`, or `dir.includes`), then in the input folder. A name can't reach outside those folders through `..`, an absolute path or a symbolic link.

## Nunjucks

Autoescaping is on, and includes are looked up in the includes folder, then in the project folder, with `./` relative to the including template, as in 11ty. Expressions follow JavaScript: truthiness, `==` and `===`, `+` on strings and numbers, how `undefined` and `null` print, and the string, array, number and date methods a template calls.

`if`, `for` (with `else`, `loop` and objects), `set`, `macro`, `call` and `caller`, `import` and `from`, `include` (with `ignore missing`), `extends`, `block` and `super()`, `raw`, `verbatim`, `filter` and `switch` all work, with whitespace control. So does every built-in Nunjucks filter, with the exceptions below, and regular expression literals such as `r/\d+/g`.

## 11ty's filters

`slug`, `slugify`, `url` and `log` work in both languages. With `config.add_plugin("rss", ...)`, so do the feed plugin's filters: `htmlBaseUrl`, `addPathPrefixToFullUrl`, `eleventyFeedHasUrl`, `eleventyFeedSortByDate` and `eleventyFeedHead` everywhere, and `dateToRfc3339`, `dateToRfc822`, `getNewestCollectionItemDate`, `absoluteUrl` and `htmlToAbsoluteUrls` in Nunjucks, as in 11ty.

11ty's `getCollectionItem`, `getPreviousCollectionItem`, `getNextCollectionItem`, `getCollectionItemIndex`, `inputPathToUrl` and `renderTransforms` aren't there yet. A page that uses one is refused with an unknown filter error. For previous and next links, loop over the collection and compare `url` to `page.url`.

Filters you add in `brrrty.config.rhai` work in both languages.

## Shortcodes

Not yet. `brrrty.config.rhai` has no `add_shortcode` or `add_paired_shortcode`, so a page that uses a shortcode from your 11ty config is refused with a syntax error for the unknown tag, at its line. The one shortcode Brrrty has is the syntax highlighting plugin's `{% highlight %}`, in Liquid.

## What is refused

Brrrty writes a page only when it can write exactly what 11ty would. Otherwise it refuses that page with a code and the reason, and builds the rest. The refusals you are most likely to meet in templates:

- **JavaScript templates and data.** `.11ty.js` templates, and data from a `.js` data file with no `.rhai` port.
- **Shortcodes and missing filters**, as above.
- **Randomness.** Nunjucks' `random` filter, and a filter your config marks with `add_nondeterministic_filter`.
- **Sorting that depends on V8.** A Nunjucks `sort` or `dictsort` whose order would depend on how V8 sorts ties.
- **Async.** Async filters and tags, `setAsync`, `getBundle` and `getBundleFileUrl`.
- **Printing a function**, which Nunjucks prints as its JavaScript source.
- **Listing a collection item's properties**, with `for k, v in item`, `dictsort`, `dump` or similar, where 11ty prints its internal objects.
- **Options to `slug` and `slugify`**, and a time zone argument to `dateToRfc822`.
- **A `SafeString` passed to a Rhai filter**, since JavaScript would pass an object, not a string.
- **Highlighting** in a Prism language Brrrty doesn't have yet, and the plugin's `{% highlight %}` in Nunjucks.
- **Runaway templates.** Limits on nesting, loop iterations (3,000,000 per render) and output size stop a template that would run for minutes.

Where 11ty would stop the whole build, for a broken data file, two pages with one permalink, or a loop in computed data, Brrrty refuses only the pages at fault. Pass `--strict` to stop at the first error instead.
