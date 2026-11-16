# Porting an `eleventy.config.js` to `brrrty.config.rhai`

A port is authored code. Brrrty checks it the way it checks everything:
build the site with 11ty and with Brrrty and compare the output
(`cargo xtask scoreboard`). This guide says what Brrrty guarantees, where
Rhai is not JavaScript, and which helper to call when you need the
JavaScript behavior.

## What Brrrty guarantees

**Data crossing the boundary is exact, in both directions.** Page data,
collection items and arguments arrive in your script as the values 11ty
would give a JS filter, and what a filter returns goes back out as data:

- Numbers are floats (JS numbers). Strings, booleans and arrays keep their
  shape. Dates are dates (`get_time(d)`, `year(d)`).
- `undefined` is `()`. `null` is `null_value()`; it
  equals `()`, is falsy, and comes back out as `null`. This holds for
  scalars, array items and object keys, so copying a value keeps its null.
  `x ?? default` only replaces `()`, so test `x == ()` to catch both.
- Objects are `object()`s and keep JS key order (below).
- A returned value is converted by data rules, never by `Debug` or Rhai's
  own printing. Text made from an object or null (`` `${o}` ``, `"a" + o`,
  `to_string`) is JavaScript's: `[object Object]` and `null`.
- Arguments arrive in the order the template wrote them. A keyword
  argument arrives in place as a two-item array `[name, value]`.

**Nothing carries state from one filter call to the next.** 11ty runs
filters in page order, so a JS filter can keep a counter or a cache in a
variable it closes over. Brrrty renders pages in parallel, so that could
never give the same output. Instead of checking for it, Brrrty makes it
impossible:

- A function or closure can't see variables outside itself. `let n = 0;
  config.add_filter("f", |v| { n += 1; n })` is a syntax error at the line
  that uses `n`, with a pointer to this guide. (Rhai is built with
  `no_closure`, and a variable that isn't in scope is an error at load.)
- Every argument is a copy for the call. A filter that changes the object
  it was given changes only its copy; other pages (and the page's own data)
  never see it.
- Values the script gives a filter by currying are copied for each call too.
- There is no module-level state, and no `this`.

To read a value set up at the top of the script, curry it into a named
function. The curried value comes first, then the value being filtered,
then the template's arguments:

```rhai
let rates = #{ a: 2, b: 3 };
fn rate(rates, value, key) { rates[key] * value }
config.add_filter("rate", Fn("rate").curry(rates));   // {{ 5 | rate: "b" }}
```

(`global::NAME` constants can't be used: they exist only while the
script's top level runs. A script that names one still loads, and the
filter that reads it fails when it is called, with "Variable not found:
global::NAME", which refuses the page that used it.)

**Other things that would make output nondeterministic are refused**, and
only the pages that hit them:

- A filter that uses randomness: declare it with
  `config.add_nondeterministic_filter("name")`.
- Rhai's `timestamp()`, which reads the real clock: it throws an error that
  points to `now()`. Rhai's time functions (`elapsed` and the rest) aren't
  there either.
- A regular expression or text where Rust and JavaScript would differ
  (below).

A filter that throws, returns something that isn't data, or runs too long
refuses its page, with the template's position and the script's.

**Time.** `now()` and Liquid's `'now'` and `'today'` follow
`SOURCE_DATE_EPOCH` (whole seconds since 1970) when it is set, so every
call in a build gives the same time. Without it, `now()` reads the system
clock at each call, so two calls, or two filter calls on different pages,
can differ, as `new Date()` does in 11ty.

**`print` and `debug`.** `print(x)` writes `x` and a newline to standard
output, and `debug(x)` writes the script's file name, line and position and
`x` to standard error. Pages render in parallel, so lines from filters on
different pages can come out in any order, mixed with Brrrty's own output.
Neither changes what is built.

**Crashes.** If Brrrty itself panics while running a script (a bug in
Brrrty, never in the script), the panic is contained: at the top level of
the config the build stops with a `config-error`, in a data file the pages
that use its data are refused with `data-rhai-error`, and in a filter or
collection only the pages that use it are refused. The message gives the
panic's text; please report it.

## Rhai is not JavaScript

Brrrty doesn't change Rhai's operators. Where they differ, call a helper.

| JavaScript | In Rhai | Use |
|---|---|---|
| `7 / 2` is `3.5` | `7 / 2` is `3` (integer literals) | `js_div(7, 2)`, or write `7.0 / 2`. Numbers that come from data are floats already. |
| `a + b` | `+` is Rhai's: strings join, numbers add, mixed is an error | `js_concat(a, b)` (string if either side is a string, array, object or date; else the numeric sum, `undefined` is `NaN`) |
| `String(v)`, `` `${v}` `` | `to_string()` is Rhai's for arrays and maps | `js_string(v)` (`[1,null]` is `1,`, objects are `[object Object]`) |
| `arr.sort()` | Brrrty replaces Rhai's `sort()` with JavaScript's | `arr.sort()`: by `String(item)` in UTF-16 order (`[3, 10, 2]` gives `[10, 2, 3]`), `undefined` last, in place, and returns the array. `js_sort(arr)` gives the same as a new array |
| `arr.sort((a, b) => ...)` | Brrrty replaces Rhai's `sort(fn)` with JavaScript's | `arr.sort(\|a, b\| ...)` or `js_sort_by(arr, \|a, b\| ...)`: any number (fractions, `NaN` is 0) or a bool; the same result as V8 at every length, even for a comparator that isn't a consistent ordering, such as `\|a, b\| a.date > b.date`: Brrrty runs V8's own TimSort, which makes the same comparator calls in the same order (checked against Node for every length from 0 to 2000). A comparator that throws or calls a missing function is an error, as in JS |
| `JSON.stringify(v)` | | `json_stringify(v)`: no spacing, keys in `object()` order, `undefined` and `NaN` as JS gives them; `()` for `undefined` itself |
| `Math.round(x)` | `round()` rounds halves away from zero | `js_round(x)`: halves round up (`-4.5` is `-4`), `-0` stays `-0` |
| `if (x)` | Only `bool` is allowed | `if truthy(x)` |
| `x \|\| y` | `\|\|` needs bools | `if truthy(x) { x } else { y }` |
| `new Date(ts)` | | `date(ts)` takes a number, a date, `()` (invalid) or null (the epoch) |
| `new URL(u).hostname` | | `url_hostname(u)`, or `()` when `u` isn't an absolute URL |
| `d.toLocaleDateString('en-US', {year:'numeric', month:'long', day:'numeric'})` | | `to_locale_date_string(d)` |
| `Date.now()` | | `get_time(now())` (follows `SOURCE_DATE_EPOCH`) |
| `execSync('git rev-parse HEAD')` | Scripts can't run commands | `git_head()`: the same commit, read from the repository's files (`HEAD`, loose refs, `packed-refs`, and a linked worktree's `.git` file and `commondir`) found from the project folder up, as git finds and reads them. An error when the project isn't in a repository, a `.git` file names no repository, or `HEAD` has no commit yet. Unlike `execSync`, it starts from the project folder and not the current one, ignores `GIT_DIR` and git's other environment variables, and doesn't apply git's `safe.directory` ownership check |
| `obj.x` on a missing key | | `()`, as `undefined` |
| `arr[i]` with a float index | an error | `arr[n.to_int()]` |
| `str[0]` | a `char` | `str.sub_string(0, 1)` (Rhai counts characters, JS counts UTF-16 units) |
| `a.length` | `len()` | `a.len()` (characters for strings) |

Rhai's closures are `|a, b| ...` and can't capture variables (see above; use a loop instead of `arr.filter(|g| g.x == key)` when the test needs `key`). The last expression is the value, and a
missing argument is `()`, so `|s, n| { let n = n ?? 40; ... }` is
`(s, n = 40) =>`. A thrown error (`throw "msg"`) refuses the page.

## Built-in helpers for Luxon and markdown-it

A config that calls `DateTime.fromJSDate(d, { zone }).toFormat(fmt)` (Luxon
3.7.2) or `markdownIt().render()` has a helper to port it to:

| JavaScript | In Rhai |
|---|---|
| `DateTime.fromJSDate(d, { zone: 'utc' }).toFormat(fmt)` | `luxon_format(d, fmt, "utc")` |
| `md.render(s \|\| '')` | `markdown(s)` |
| `md.renderInline(s \|\| '')` | `markdown_inline(s)` |

- `luxon_format` supports every Luxon token for `en-US`: the field tokens and
  the macro tokens (`D`, `DD`, `t`, `T`, `f`, `F` and their longer forms).
  Zones are `"utc"`, fixed offsets (`"UTC+5"`, `"utc-3:30"`) and IANA names.
  A value that isn't a `date`, or a zone Luxon doesn't know, gives `"Invalid
  DateTime"`, as Luxon does. `luxon_format(d, fmt, zone, locale)` takes a
  locale, and any but `en-US` is an error. So are the machine-dependent zones
  (`"local"`, `"system"`, `"default"`), and a zone *name* (`ZZZZ`, `ZZZZZ`,
  `ttt`, `tttt`, `fff`, ...) for an IANA zone before 1970 or at an instant
  where ICU's name can't be told from the table, and any time from 2100 in a zone with
  daylight saving (the time zone data has no rules past 2099). JavaScript's
  `format = 'dd-LLL-yyyy'` default is `format ?? "dd-LLL-yyyy"`.
- `markdown` and `markdown_inline` are `markdown-it` 15.0.3 with no options, so HTML
  is **off** (11ty's own instance has it on) and the indented code rule is
  **on**. `null`, `()`, `""`, `0` and `false` give `""`, as `s || ''` does;
  any other value that isn't a string is an error, as in markdown-it.
  They match markdown-it on about 200 pinned cases and on three random 6,000-input
  runs, apart from the known gaps below; they are not proven identical for every input.
  Known gaps (each a difference in the Rust port of markdown-it, left for later):
  an emphasis marker next to an emoji or other symbol (`_a_😀` is `<em>` in
  markdown-it); a single-column table whose header ends in `|` and whose delimiter
  row has no pipe; an empty list item right after a blockquote's lazy line; a
  reference definition with an odd destination (`[x]: /u[x]é`); a table after a
  blockquote's lazy line; a table indented four spaces; a `<div>` followed by a table;
  `> <!-- -->`; and input that makes the port panic (for example a strikethrough across
  a CRLF inside a list item, in a debug build of Brrrty, where the port's integer
  overflow is checked), which makes the call an error with the panic text. In 11ty's own
  Markdown (markdown-it 14.3.3, which `.md` pages use), the same port also differs on a
  fence that runs to the end of the text (it adds a final newline), on an HTML block at
  the end of the text with no newline, on lines indented four spaces or more that the
  `code` rule no longer takes, and on some raw-HTML edge cases. A panic there refuses the
  page with `markdown-render-failed`, and the rest of the site builds.

## `object()` and key order

A JavaScript object keeps its keys in insertion order, integer-like keys
first. Rhai's `#{ ... }` maps are sorted by key. Use `object()` wherever the
order can show in output:

```rhai
let o = object();
o.zebra = 1;          // or o["zebra"] = 1
o.apple = 2;
o.keys()              // ["zebra", "apple"]
for k in o { ... }    // keys, like for...in (for (k, v) gives a counter)
o.entries()           // [[k, v], ...]
```

`object(map)` copies a map in (its keys arrive sorted). Methods: `keys`,
`values`, `entries`, `len`, `contains`, `remove`. Data from Brrrty arrives as
`object()`s, so returning it as it came keeps its order.

## Collections

`eleventyConfig.addCollection(name, fn)` becomes
`config.add_collection(name, |api| ...)`, and the collection API is in
snake_case:

| JavaScript | Rhai |
|---|---|
| `getAll()` | `get_all()` (the order pages were added) |
| `getAllSorted()` | `get_all_sorted()` (date, then input path) |
| `getFilteredByTag("t")` | `get_filtered_by_tag("t")` |
| `getFilteredByTags("a", "b")` | `get_filtered_by_tags("a", "b")` (up to four) |
| `getFilteredByGlob("./src/posts/**/*.md")` | `get_filtered_by_glob("./src/posts/**/*.md")`, or an array of globs |
| `arr.reverse()` | `arr.reverse()` (returns the array, like JS) |
| `arr.slice(a, b)` | `arr.slice(a, b)` (JS's, negative indexes included) |
| `arr.map(f)`, `arr.filter(f)` | `arr.map(\|x\| ...)`, `arr.filter(\|x\| ...)` |
| `arr.sort(f)`, `arr.sort()` | `arr.sort(\|a, b\| ...)`, `arr.sort()` (JavaScript's; see above) |
| `arr.forEach(f)` | `for x in arr { ... }` |
| `{}` built in a loop | `object()` |

Globs are read from the project root (`./src/posts/**/*.md`, not
`posts/**/*.md`), as 11ty reads them. Brrrty supports `*`, `?`, `**` as a
whole folder name, and literal ASCII text; anything else (braces, `[ab]`,
`!`, extglobs) is an error rather than a guess.

**Items are the pages themselves, and read-only.** 11ty shares one item
with every page and collection, so changing one in JS changes it
everywhere. Brrrty can't follow that, so setting a property of an item
(or of anything read through it, like `post.data`) is an error, and an
array read through an item (`post.data.tags`) can be looped over, indexed,
measured with `len()` and searched with `contains`, but not changed (no
`push`, `sort` or `reverse`): `to_array()` copies it. Whatever you return (an array, an `object()`, a map,
nested any way) keeps the items as they are, so a template still reads
their `url`, `data` and `templateContent`.

Some things are refused, and only the pages that read the collection are:

- reading an item's `templateContent` or `content` inside the collection
  (11ty hasn't rendered it yet and stops the build), or its `template`;
- returning an array holding an object you made with an `inputPath` key
  (11ty would give it that page's content);
- a result that would hold a page whose date Brrrty can't read.

A name your JS config adds that the port doesn't is refused where a page
reads it, so a missing collection shows up as a refusal, never as an empty
list.

## Data files

`_data/site.js` becomes `_data/site.rhai`, and `x.11tydata.js` becomes
`x.11tydata.rhai`. Folders nest the key the way `.json` does (`_data/a/b.rhai`
is `a.b`). The script's last expression is the value:

```rhai
// _data/site.rhai, from `module.exports = () => ({ name, url })`
#{ name: "Mine", url: "https://example.com" }
```

A map's keys come out sorted; use `object()` where order shows. If the
script defines `fn data()` (or `fn data(global)`), Brrrty calls it the way
11ty calls a function a data file exports, with the config's
`add_global_data` values and 11ty's `eleventy` data as the argument, and
the script's own statements don't run.

- Each file runs once per build on a fresh engine: nothing carries from one
  file to the next, and a script can't see `config`.
- `now()` follows `SOURCE_DATE_EPOCH`, and reads the system clock at each call
  when it isn't set. `new Date().getFullYear()` is `year(now())`.
- The Rhai limits apply. An error is a problem with the file, line and
  column, and only the pages that read the key are refused.
- Precedence: a `.rhai` file wins over a `.json` or `.js` of the same key.
  In directory and template data it holds the slot `.11tydata.js` holds.
  When a `.js` file has a `.rhai` port beside it, Brrrty ignores the `.js`
  and prints one note.
- Template and directory data must give a map or `object()`. A value with
  functions in it (a JavaScript `eleventyComputed` of functions) can't be
  data; port the computation into the template or a filter.
- Not portable yet: data that reads files (`readFileSync`, `import` of a
  JSON or CSV file), runs commands other than `git rev-parse HEAD` (which
  is `git_head()`), or imports other modules. There is no
  `import` or file access in a data script.

## Plugins

`eleventyConfig.addPlugin(plugin, options)` becomes
`config.add_plugin("name", #{ ... })` for the plugins Brrrty has its own
version of. Any other name is an error where it is added.

**Port every `addPlugin` line.** Brrrty reads only the Rhai port, so it
can't see a plugin the JS config adds and the port leaves out. A missing
plugin is not refused: the pages it would change are written as if it
weren't there, and they differ from 11ty's. The scoreboard shows them as
`different`; nothing else does.

### `syntaxhighlight`

`@11ty/eleventy-plugin-syntaxhighlight` 5.0.2, with Prism 1.30.0:

```js
const syntaxHighlight = require("@11ty/eleventy-plugin-syntaxhighlight");
eleventyConfig.addPlugin(syntaxHighlight, { preAttributes: { tabindex: 0 } });
```

becomes

```rhai
let pre = object();
pre.tabindex = 0;
config.add_plugin("syntaxhighlight", #{ pre_attributes: pre });
```

Markdown fences that name a language (`` ```js ``, `~~~php`, `` ```js/1-2 ``
to mark lines) and the Liquid tag (`{% highlight js 1,3 %}...{% endhighlight %}`)
are written as 11ty writes them, byte for byte: Prism's tokens, the
language aliases (`sh`, `ts`, `html`, ...), `diff-` languages and line
marks. The plugin highlights 11ty's own Markdown, not the output of a
`markdown` filter a config adds.

Brrrty has the grammars of 28 of Prism's 297 languages:

| Language | Aliases |
|---|---|
| `bash` | `sh`, `shell` |
| `c` | |
| `clike` | |
| `css` | |
| `diff` | |
| `go` | |
| `ini` | |
| `javascript` | `js` |
| `json` | `webmanifest` |
| `jsx` | |
| `less` | |
| `liquid` | |
| `lua` | |
| `markdown` | `md` |
| `markup` | `html`, `xml`, `svg`, `mathml`, `ssml`, `atom`, `rss` |
| `markup-templating` | |
| `nginx` | |
| `perl` | |
| `php` | |
| `python` | `py` |
| `r` | |
| `ruby` | `rb` |
| `rust` | |
| `sql` | |
| `toml` | |
| `typescript` | `ts` |
| `vim` | |
| `yaml` | `yml` |

and Prism's own `plain` (`plaintext`, `txt`). `text`, and a name that is
neither a Prism language nor an alias of one (`conf`, `zsh`, `console`,
`terraform`, `JS`, `DIFF-js`, ...), are written as the plugin writes
them: the code as it is, not escaped, with its `language-` class. Every
other Prism language (`java`, `kotlin`, `cpp`, `scss`, `docker`, ...,
and `njk`, which the plugin highlights as `django`) refuses the page.

The options are 11ty's, in snake_case: `line_separator`,
`error_on_invalid_language`, `always_wrap_line_highlights`, `trim`,
`languages`, `template_formats`, `pre_attributes` and `code_attributes`.
The attributes are an `object()`, so they keep the order written (a
`#{...}` map sorts its keys, so one with more than one key is refused);
each value is a string or a number. Brrrty can't call a function, so a
function for an attribute, and `init`, are refused.

A page is refused, with the reason, when Brrrty can't highlight its code
the way 11ty does:

- a language Prism has that Brrrty doesn't have yet (above);
- a grammar that a language another page names would change. In 11ty,
  Prism keeps every language any page loaded for the rest of the build,
  and loading some languages changes others: `regex` reloads `javascript`
  and everything built on it (`markup`, `css`, `php`, `markdown`,
  `typescript`, `jsx`, `liquid`, `less`), `css-extras` and `js-extras` do
  the same, `php-extras` changes `php`, `opencl` and `cilkc` change `c`,
  `graphql` reloads `typescript` and `jsx`, and `java` reloads `jsx`. 21
  languages do this (the
  `mutators` in brrrty-prism's `grammars/prism.json`, from Prism's
  `components.json` and from loading each one in Node). Before it builds,
  Brrrty reads every fence's language and every `{% highlight %}`
  argument in every file under the input folder, whether it renders the
  file or not (Nunjucks pages, includes, pages refused for another
  reason), and a language a template writes (`` ```{{ lang }} ``) counts
  as any language, as does a `{% highlight` whose arguments start on the
  next line. If one of them changes a grammar, every page that
  highlights with that grammar is refused, naming the file and line.
  Every page is rendered before any is written, and a language that only
  shows up then (a fence built from data) redoes the build with it, so
  it is refused the same way;
- Prism looking a language up that 11ty may or may not have loaded by then,
  depending on the order it renders pages in. This happens in
  Markdown code (`` ```md ``) that has a fenced code block in a language,
  and in `diff-x` code where `x` isn't surely loaded. Front matter in
  Markdown code is highlighted as YAML only once YAML is loaded: Brrrty
  replays the order 11ty renders pages' content in (one at a time), and
  writes the page when a page 11ty renders before it loaded YAML (or the
  same page did, earlier), and refuses it otherwise. That order is 11ty's
  default, one page at a time; `eleventyConfig.setConcurrency(n)` with `n`
  above 1 renders several at once and can't be ported, so a site whose
  11ty config sets it may get different front matter from Brrrty;
- code where Prism's patterns would take too long (on hostile input,
  reaching the limit can take a few seconds), or where a Prism hook
  would run out of stack in Node;
- a line range of a million lines or more.

A page that reads a refused page's `templateContent` or `content` is
refused too. Where the plugin throws (an attribute that isn't a string or
a number, or `error_on_invalid_language` and a language Prism doesn't
have), 11ty stops the build; Brrrty fails that page with
`syntax-highlight-failed`, in a fence or a `{% highlight %}` tag.

In Nunjucks the plugin adds a paired shortcode, `{% highlight "js 1,3" %}`,
which Brrrty doesn't run yet: a Nunjucks page that uses it is refused.
Its argument is an expression, so the build-wide check above counts any
argument other than one string as any language. Without `add_plugin("syntaxhighlight")` code is written plain,
which is wrong for a site whose 11ty config adds the plugin: add it.

### `rss`

`@11ty/eleventy-plugin-rss`'s `feedPlugin`, once per feed. The options
are 11ty's, in snake_case:

```js
eleventyConfig.addPlugin(feedPlugin, {
  type: "atom",
  outputPath: "/atom.xml",
  collection: { name: "posts", limit: 10 },
  metadata: { language: "en", title: "My Blog", base: "https://example.com/",
              author: { name: "Me", email: "" } },
});
```

```rhai
config.add_plugin("rss", #{
    type: "atom",
    output_path: "/atom.xml",
    collection: #{ name: "posts", limit: 10 },
    metadata: #{ language: "en", title: "My Blog", base: "https://example.com/",
                 author: #{ name: "Me", email: "" } },
});
```

`type` (`atom`, `rss` or `json`), `output_path`, `input_path`,
`collection` (`name`, `limit`, `sort`), `metadata`, `template_data`,
`stylesheet` and `script` work as in 11ty, with the same defaults. Keys
inside `metadata` and `template_data` are data, so keep their own names.
The feed is a page of the site, as in 11ty: it is in `collections.all`,
dated when the build runs (`SOURCE_DATE_EPOCH`).

Brrrty renders the plugin's feed templates itself, byte for byte; the post
HTML in them gets 11ty's `renderTransforms`, which makes every link and
image URL a full URL on `metadata.base`. What it refuses, where 11ty would
build something Brrrty can't check: `htmlBasePluginOptions` and
`rssPluginOptions`; a `limit` that isn't a whole number; a collection name
with characters that would change the template (`'`, `{`, `%` and the
like, and for `rss` anything but letters, digits, `_` and `$`); an
`input_path` that isn't a `.njk` file in the input folder; and an HTML
element or attribute named like a JavaScript built-in (`toString`,
`constructor`) in a post the feed shows. Where 11ty stops the build
(`metadata.base` not a full URL, a collection that doesn't exist), Brrrty
refuses only the feed.

The plugin's filters for every template language work in Liquid too:
`htmlBaseUrl`, `addPathPrefixToFullUrl`, `eleventyFeedHasUrl`,
`eleventyFeedSortByDate` and `eleventyFeedHead`. Its other filters
(`dateToRfc3339`, `dateToRfc822`, `getNewestCollectionItemDate`,
`absoluteUrl`, `htmlToAbsoluteUrls`) are Nunjucks-only in 11ty, so a
Liquid template can't use them there either.

## Regular expressions

`regex_replace(s, pattern, replacement, flags)`, `regex_split(s, pattern)`
and `regex_test(s, pattern)` (each also takes `flags`) use the `regex`
crate, with JavaScript's `\s`, `\d`, `\w`, `\b` and `.`, JavaScript's `$1`
`$&` `$<name>` replacements, and JavaScript's `split` (captures included).

Only syntax that means exactly the same in both is accepted. Everything
else is an error naming the pattern, with the script's line:

- no lookaround (`(?=`, `(?!`, `(?<=`, `(?<!`) and no backreferences
  (`\1`, `\k<n>`);
- no `\p{..}`, `\a`, `\A`, `\z`, `\c`, `\u{..}`; no `(?i)`-style groups;
  no `[]` or `[^]`; no bare `{`, `}` or `]`; no `--` or `\w` range ends in a
  class; no flags other than `g i m s u`;
- no repeated or optional group that can match nothing, such as `(a*)?`
  or `(?:x|)*`: JavaScript skips a repetition that matches nothing and
  Rust takes it, so the match or its groups would differ;
- no capturing group inside a repeated group that can sit out a pass, such
  as `(?:(a)|b)+` or `(?:(a)?b)*`: JavaScript clears it at the start of each
  pass and Rust keeps its last value. A group every pass sets, such as
  `(\w)+` or `(?:([a-z])\d)+`, is fine.

Some text is refused, since the answer would differ:

- non-ASCII text when `i` is used and the pattern has letters (Rust folds
  more characters, such as the Kelvin sign);
- text with characters outside the BMP (emoji) when the pattern could split
  a surrogate pair, unless you add `u`. JavaScript sees such a character as
  two units, and `.`, a negated class (`[^>]`) and `\S`, `\D`, `\W` match
  each unit alone. These are fine as greedy loops (`*` or `+`) that nothing
  of the same kind follows, such as `(<([^>]+)>)`, `\S+` or `<.+>`. A single
  one (`^.$`, `[^a]b`), a lazy or counted one (`[^>]+?`, `.{2}`), two in a
  row (`[^a]+[^b]+`), `\B`, a range across U+D800 to U+DFFF, and a pattern
  that can match nothing are refused;
- text with `\r`, U+2028 or U+2029 when `m` is used with `^` or `$`.

## Checklist

1. Port one filter or collection at a time. Keep the JS next to it while you work.
2. Use `object()` for anything built in a loop whose order can show.
3. Replace each JS operator in the table above with its helper.
4. Run the JS and the port on the same inputs in Node and compare. Include
   `undefined`, `null`, `0`, `""`, non-ASCII text and emoji.
5. Port every `addPlugin` call (see Plugins); a plugin left out is not
   noticed.
6. Run `cargo xtask scoreboard`. Every page that builds must be identical to
   11ty's. A refused page names its reason; fix the port, or the pattern,
   until it builds.
