# Porting an `eleventy.config.js` to `brrrty.config.rhai`

A port is authored code. Brrrty checks it the way it checks everything:
build the site with 11ty and with Brrrty and compare the output
(`brrrty doctor --verify` in a site, `cargo xtask scoreboard` across the
test sites). This guide says what Brrrty guarantees, where Rhai is not
JavaScript, and which helper to call when you need the JavaScript
behavior.

Start with the `// TODO(brrrty):` lines `brrrty init` left: while one is
there, Brrrty builds no pages (`config-todo`), since each stands for
something the 11ty config does that could change any page. Port each one,
or decide it changes nothing Brrrty writes, and delete its line.

Then run `brrrty doctor`: it lists what is left to port, grouped by
cause, with every place each one shows up. A build stops each page at its
first problem; doctor also reads each refused page's template, layouts,
literal includes, `permalink` and `eleventyComputed` for every filter
that isn't defined and every JavaScript data file read. Unknown tags
(shortcodes) are listed one per file, as syntax errors, so run it again
after porting.

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
  `config.add_nondeterministic_filter("name")`. There's no `Math.random`:
  `random()` is an error. A random value can't give 11ty's bytes, and a
  seeded stand-in would silently differ from 11ty, so Brrrty doesn't offer
  one.
- `toLocaleString()` and `localeCompare()` without a locale follow Node's
  default locale, which it takes from the first of `LC_ALL`, `LC_MESSAGES`
  and `LANG` that is set (`de_DE` prints `1.234,5`, `en_IN` prints
  `12,34,567.891`, `sv_SE` sorts `ä` after `z`). Brrrty gives the `en-US`
  result, so these run only when that variable is one Node maps to
  `en-US`: `en_US` (any case, also `en-US` or `en_US_POSIX`), `C`, `c` or
  `POSIX`, with any codeset (`en_US.UTF-8`, `C.UTF-8`) and no
  `@modifier`, or when none is set. Any other value (`en_GB`,
  `en_US@euro`, `C.UTF-8@x`, which Node prints without digit grouping, an
  empty one) refuses the call, naming the variable. Passing `"en-US"`
  works everywhere.
- Local time comes from the tz database Brrrty bundles, release 2026b, the
  one Node 26.2.0 ships (`process.versions.tz`); the operating system's
  zone rules are never read. The zone is found as Node's ICU finds it:
  the one `TZ` names; with `TZ` unset, the one `/etc/localtime` links to
  (its real path after `/zoneinfo/`, without a `posix/` or `right/`
  prefix); when `/etc/localtime` is a copied file, as in many Docker
  images, the first file under `/usr/share/zoneinfo` with the same bytes;
  and UTC when there is no `/etc/localtime`. A `TZ` that names no zone in
  the bundled data (a typo, a POSIX rule such as `EST5EDT,M3.2.0,M11.1.0`),
  a broken link, or a copy that matches no zone file stops the build
  (`tz-invalid`): Node would quietly fall back to UTC or guess from the C
  library. A site built by a different Node version (with other tz data)
  can differ in zones whose rules changed between the two releases.
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

**`config` while pages render.** `config` can only change while the script
runs. A `config.add_*` or `config.ignores` call after that (from a filter, a
collection or a global data function that was handed `config`) is ignored, as
11ty 3.1.8 ignores it: a filter added then is unknown to the templates, a
passthrough copy isn't made, and an ignore doesn't drop the page. So the pages
build as 11ty writes them, and Brrrty warns (`config-late-call`) once for each
place in the script that makes such a call, where 11ty says nothing. A warning
never fails a build, `--strict` included.

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
| `new Date(y, m, d, h, mi, s, ms)` | | `date(y, m, d, h, mi, s, ms)` with two to seven parts, in the local zone (`TZ`, with Node 26's tz data, below): parts roll over (`date(2026, 12, 0)` is 2026-12-31), fractions are dropped, a year 0-99 is 19xx, a time the clocks skip or repeat resolves as V8 does. Checked against Node in five zones, and every zone's offsets from 1850 to 2045 and beyond |
| `d.getFullYear()` ... `d.getMilliseconds()`, `d.getDay()`, `d.getTimezoneOffset()` | | `d.get_full_year()`, `get_month()`, `get_date()`, `get_day()`, `get_hours()`, `get_minutes()`, `get_seconds()`, `get_milliseconds()`, `get_timezone_offset()`: local time; `NaN` for an invalid date |
| `d.toISOString()` | | `d.to_iso_string()`; an invalid date is an error, as JS throws |
| `n.toFixed(d)` | | `to_fixed(n, d)` or `n.to_fixed(d)`: V8's exactly. It rounds the double's exact value and a tie goes away from zero (`2.5` gives `"3"`; `1.005` is 1.00499999..., so `to_fixed(1.005, 2)` is `"1.00"`); `1e21` and up print as `String(n)`; `-0` gives `"0.00"`, `-0.0001` gives `"-0.00"`. Digits outside 0-100 are an error (`RangeError`). `n` must be a number, as in JS: convert a string first. Checked against Node on 608,270 cases |
| `n.toLocaleString('en-US')` | | `to_locale_string(n)` or `to_locale_string(n, "en-US")`: ICU's default format, the shortest digits rounded half up to 3 decimals, grouped by 3 (`1,234.568`, `-0`, `∞`). Any other locale, or options, is an error. Checked against Node on 300,000 values |
| `a.localeCompare(b)` | | `locale_compare(a, b)`: -1, 0 or 1, ICU's root order (the `en-US` one: `a` < `A` < `á` < `b`). Another locale, or options, is an error. So is a Han character, or one whose compatibility form is Han (Node sorts them by radical and stroke count, from data Brrrty doesn't have), and a character new in Unicode 18 (Node 26's ICU is Unicode 17). Checked against Node on 316,129 pairs and on every other code point, sorted alone and with three suffixes |
| `Math.random()` | | not available. `random()` is an error that says why; for a filter that shuffles, use `config.add_nondeterministic_filter(name)` |
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
| `markdownIt({ linkify: true, typographer: true })`, then `md.linkify.set({ fuzzyLink: true })` and `md.render(s)` | `markdown(s, #{ linkify: true, typographer: true, fuzzy_link: true })` |

- `luxon_format` supports every Luxon token for `en-US`: the field tokens and
  the macro tokens (`D`, `DD`, `t`, `T`, `f`, `F` and their longer forms).
  Zones are `"utc"`, fixed offsets (`"UTC+5"`, `"utc-3:30"`) and IANA names.
  A value that isn't a `date`, or a zone Luxon doesn't know, gives `"Invalid
  DateTime"`, as Luxon does. `luxon_format(d, fmt, zone, locale)` takes a
  locale, and any but `en-US` is an error. So are the machine-dependent zones
  (`"local"`, `"system"`, `"default"`), and a zone *name* (`ZZZZ`, `ZZZZZ`,
  `ttt`, `tttt`, `fff`, ...) for an IANA zone before 1970 or at an instant
  where ICU's name can't be told from the table. Offsets come from the
  bundled tz data (2026b, Node 26's) for every instant. JavaScript's
  `format = 'dd-LLL-yyyy'` default is `format ?? "dd-LLL-yyyy"`.
- `markdown` and `markdown_inline` are `markdown-it` 15.0.3 with no options, so HTML
  is **off** (11ty's own instance has it on) and the indented code rule is
  **on**. `null`, `()`, `""`, `0` and `false` give `""`, as `s || ''` does;
  any other value that isn't a string is an error, as in markdown-it.
- `markdown(s, options)` takes markdown-it's `html`, `linkify` and `typographer`
  and linkify-it's `fuzzyLink` (as `fuzzy_link`), each `true` or `false`, in a
  map or an `object()`. Any other key is an error. Linkify is linkify-it 6.1.0
  (markdown-it 15's), so links without `http://` (`example.com`) need
  `fuzzy_link: true`, as in JavaScript. The typographer is markdown-it's:
  `(c)`, `+-`, `...`, `--`, `---`, and smart quotes `“”‘’`. markdown-it is
  15.0.3; a site with 15.0.2 installed differs from it only
  with `html: true`, on some HTML comments. For an instance with renderer
  rules, use `markdown_it(options)` (see below).

### markdown-it renderer rules

A renderer rule (`md.renderer.rules.link_open = (tokens, idx, options, env,
self) => ...`) ports to a Rhai function with the same five parameters. `self`
is a reserved word in Rhai, so call the last one `renderer`.

| JavaScript | In Rhai |
|---|---|
| `eleventyConfig.amendLibrary('md', (md) => plugin(md))` | `config.amend_library("md", \|md\| plugin(md));` |
| `const md = new MarkdownIt({ linkify: true }); plugin(md); md.render(s)` | `let md = markdown_it(#{ linkify: true }); plugin(md); md.render(s)` |
| `md.renderer.rules.link_open = (tokens, idx, options, env, self) => ...` | `md.renderer.rules.link_open = \|tokens, idx, options, env, renderer\| ...;` |
| `tokens[idx].attrGet('href')` | `tokens[idx].attr_get("href")` (`()` when there is none) |
| `attrSet`, `attrJoin`, `attrIndex`, `attrPush([n, v])` | `attr_set`, `attr_join`, `attr_index`, `attr_push([n, v])` |
| `self.renderToken(tokens, idx, options)` | `renderer.render_token(tokens, idx, options)` |
| `self.renderAttrs(token)`, `md.utils.escapeHtml(s)` | `renderer.render_attrs(token)`, `escape_html(s)` |
| `const base = md.renderer.rules.link_open \|\| renderToken` | `md.renderer.rules.link_open = Fn("my_rule").curry(md.renderer.rules.link_open);` |

Rhai closures can't capture variables, so a rule that wraps the rule before
it takes it as a curried first parameter: `fn my_rule(base, tokens, idx,
options, env, renderer)`, calling `base.call(tokens, idx, options, env,
renderer)`, or `renderer.render_token(tokens, idx, options)` when `base` is
`()` (no rule was set). Reading `md.renderer.rules.text` (and `image`,
`code_inline`, `softbreak`, `hardbreak`, `html_inline`) gives markdown-it's own
rule, which a wrapper calls the same way. Setting a rule to `()` removes it.
A closure that captures nothing works as JavaScript's `|| renderToken`:
`let base = md.renderer.rules.link_open ?? |t, i, o, e, r| r.render_token(t, i, o);`
then `Fn("my_rule").curry(base)`, with no `()` branch in `my_rule`.

- `amend_library("md", f)` runs `f` on 11ty's own markdown-it as the config
  loads; its rules apply to every `.md` template and Markdown layout, as in
  11ty. Other library names, and `set_library`, are errors.
- `markdown_it()` is `markdownIt()`; `markdown_it(options)` takes the options
  of `markdown(s, options)`. `md.render(s)` renders as `markdown()` does, with
  the instance's rules. `markdown()` and `markdown_inline()` never have rules.
- Rules run for inline tokens: `text`, `link_open`, `link_close`, `image`,
  `code_inline`, `softbreak`, `hardbreak`, `html_inline`, `em_open`,
  `em_close`, `strong_open`, `strong_close`, `s_open` and `s_close`. A rule
  for a block token (`paragraph_open`, `heading_open`, `fence`, `table_open`
  and the rest) is an error when it is set; one for `inline`, or a type
  markdown-it never makes, is never called, as in markdown-it.
- `tokens` is one inline block's token list, exactly as markdown-it hands it
  to `renderInline` (after linkify, the typographer and `text_join`). It is
  read-only as a list: `tokens[i]` (`()` past either end), `tokens.len`; adding,
  removing or replacing a token is an error. A token's `type`, `tag`,
  `attrs`, `nesting`, `level`, `content`, `markup`, `info`, `block` and
  `hidden` can be read and set, and the renderer sees the change. `map`,
  `meta` and `children` are `()`; reading an `image`'s `children` is an error.
  A token's `type` can't become a block type (`fence`, `code_block`,
  `html_block`, ...), and markdown-it's image rule on a token that isn't an
  image is an error, as markdown-it throws.
  Attribute names and values must be strings (JavaScript would turn a number
  into one).
- Rules are set while the config or a data file loads. Once pages render,
  every library is frozen: changing a rule from a filter, a collection or
  another rule is an error (in 11ty, pages render one at a time in a fixed
  order; Brrrty renders them in parallel, so such a change would make the
  output depend on timing). An instance a filter, collection or rule makes
  for itself (`markdown_it()` inside the function) belongs to that call, so
  its rules can be set there. Reading a block type's rule is an error too.
- `md.render(s)` takes a string only, as markdown-it's `render` throws on
  anything else; `markdown(s)` keeps the filters' `s || ''`.
- `amend_library` lowercases the name, as 11ty does (`"MD"` is `"md"`).
- `renderer.render_token` is markdown-it 14.3.3's on 11ty's library and
  15.0.3's on `markdown_it()` instances; they differ around hidden tokens.
- `options` is read-only: `html`, `xhtmlOut`, `breaks`, `linkify`,
  `typographer`, `langPrefix`, `quotes`. Others are errors.
- `env`: 11ty renders with the page's data as `env`, which rules can't read
  yet, so reading it on 11ty's library is an error; on a `markdown_it()`
  instance it is `{}`, so a read gives `()`. Writing to it is an error.
- A rule must return a string. A rule that fails, returns anything else, or
  runs markdown-it's image rule on a token without `alt` (markdown-it throws)
  refuses that page with `markdown-rule-failed`, or makes that `md.render` an
  error.

### What Markdown Brrrty refuses

Brrrty renders Markdown with a Rust port of markdown-it, corrected where it
differed, and checked against markdown-it (14.3.3 for 11ty's `.md` pages,
15.0.3 for `markdown()`) by differential fuzzing. It isn't proven identical for
every input. Where the port is known to differ, Brrrty refuses instead of
writing something else: an 11ty page with `markdown-render-failed` (the rest of
the site builds), and a `markdown()` call with an error. Refused today:

- a link reference definition (one the parser took, or a paragraph that
  starts like one) whose destination ends in `\` (`[docs]: C:\path\`), or
  one with only `[label]:` on its line and a list marker alone on the next;
- inside a blockquote, a lazy line (fewer `>` than the quote) that is only a
  list marker (`> quote` then `-`);
- a table right after a blockquote's paragraph, with no line between them (an
  empty `>` line ends the paragraph, and that is fine);
- inside a blockquote, a code span or inline HTML in a paragraph with a lazy
  line indented with spaces or tabs, or a line with a tab after its `>`;
- inside a blockquote, a tab after a `>` where markdown-it keeps it as
  written or counts its columns differently from Brrrty's parser: in an HTML
  block (`>\t<pre>`, unless the tab starts at a column of 3 modulo 4, and
  any extra indentation under two or more `>`), in a fenced code block's text
  (`>\t` or under two or more `>`), and any tab among the markers after a
  second `>` (`>>\t\tcode`, `>\t>\t* x`). A tab after `>` in a paragraph, a
  list item or a fence's opening line under one `>` (`> a\n>\tb`,
  `>\t- item`, `> \treturn` in a fence) renders;
- with `linkify`: a run of 1,000 or more characters with no space in a
  paragraph's source, link URLs included;
- with the typographer: a quote right next to an image whose label can't be
  found in the source.

The checks look at what the block parser made, not at raw lines: text in a
code block (fenced or indented) or an HTML block outside a blockquote is never
refused, and neither is a `\` in a definition's title, or text before a
definition on the same line (`12 [a]: b\c` is a paragraph to both). They are
still a little wider than the differences, so they can refuse text markdown-it
and Brrrty would write the same way (most often the tab checks under two `>`). Known, not
refused: a panic in the port (none known) makes the call an error with the
panic text and refuses an 11ty page.

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
- Not portable yet: data that runs commands other than `git rev-parse
  HEAD` (which is `git_head()`) or imports other modules. There is no
  `import` in a data script; files are read with the helpers below. Such a
  file can keep running in Node instead: `config.set_js_data("node");`.

### Checking a port with doctor

With JavaScript data on and Node there, `brrrty doctor` runs both the `.js`
file and its `.rhai` port and compares the two values: types, key order,
`-0`, and dates to the millisecond. It says `ported: site.rhai is identical,
delete site.js`, or names the first path where they differ
(`ported: nav.rhai differs at data.items[2] (value)`). `brrrty init` translates a
file whose value is a literal by itself, and lists the others with the
helpers that would port them.

### Reading files

A data file (or the config, or a filter) reads files in the project with:

| JavaScript | Rhai |
|---|---|
| `fs.readFileSync(p, 'utf8')` | `read_text(p)`: bytes that aren't UTF-8 become U+FFFD exactly as Node replaces them; a byte order mark is kept |
| `JSON.parse(fs.readFileSync(p, 'utf8'))`, `import x from './x.json'` | `read_json(p)`: keys in JavaScript's order (integer-like first), a repeated key keeps its first place and its last value; JSON that `JSON.parse` rejects is an error, and so are numbers out of a double's range (`1e400`) and lone surrogate escapes, which `JSON.parse` would accept |
| `fs.readdirSync(p)` | `read_dir(p)`: names without `.` and `..`, dotfiles included, sorted by their UTF-8 bytes as libuv sorts them (`10.md`, `2.md`, `Z.md`, `_index.md`, `a.md`, `é.md`). That is Node's order on macOS and Linux; on Windows Node gives the file system's order |
| `fs.readdirSync(p, { recursive: true })` | `read_dir(p, #{ recursive: true })`: the folder's names, then each subfolder's in that order, breadth first, as `sub/name`. As in Node 26, a symlink to a folder is entered (`link -> real` lists `link/x.md` and `real/x.md`) and a dangling one is just a name. A symlink to a folder outside the project is refused (Node would list it), and so is a loop such as `x/up -> ..` (Node follows it until the path is too long). `recursive` must be `true` or `false`; other options (`withFileTypes`, `encoding`) are refused |
| `fs.existsSync(p)` | `file_exists(p)` |
| `parse(text, opts)` from `csv-parse/sync` (7.0.3) | `parse_csv(text, opts)` |

Paths are relative to the project folder, 11ty's working directory, so
`join(__dirname, 'shop.csv')` in `src/_data/shop.js` is
`"src/_data/shop.csv"`. A script can't leave the project: an absolute path
and any `..` component are refused before anything is opened, and every path
is opened relative to a handle on the project folder one component at a
time (with cap-std: `openat2` with `RESOLVE_BENEATH` on Linux, `openat` with
`O_NOFOLLOW` elsewhere), following a symlink only when it stays inside. A
symlink that leads out, including one with an absolute target (even one
that points back inside), is refused, even for `file_exists`, and a symlink
swapped in while the build runs can't get around that. Only regular files
are read: a FIFO, socket or device is refused rather than waited on. A path
ending in `/` or `/.` needs a folder there, as in Node (`file_exists("a.txt/")`
is `false`). A file over 16 MiB is refused before it is read (and text over
16 MiB once invalid UTF-8 is replaced), `read_dir` stops at 100,000 names, and
`read_json` at 100,000 array items. Nothing writes. The helpers work on macOS
and Linux; elsewhere they refuse. A missing file is an error a `try { } catch { }` catches,
like Node's `ENOENT`: `read_text("x.txt"): no such file or directory`.

`parse_csv` is a byte-level port of csv-parse 7.0.3, checked against Node on
28,044 inputs. It takes the options `columns` (`true` or `false`; a list or a
function is refused), `delimiter` (one string), `bom`, `trim`, `ltrim`,
`rtrim`, `skip_empty_lines`, `relax_column_count`,
`relax_column_count_less` and `relax_column_count_more`, in snake_case or
camelCase. Any other csv-parse option is refused by name, and so is a key
csv-parse doesn't know (it would ignore a typo). An option given in both
spellings (`skip_empty_lines` and `skipEmptyLines`) is refused: csv-parse
takes whichever was written last, and a Rhai map doesn't keep that order.
csv-parse's quirks are kept:

- The first line ending (`\r\n`, `\n` or `\r`) decides the record
  delimiter; a different one later stays in the field.
- `trim` drops a byte order mark at the start of a field, so a line of only
  spaces is empty for `skip_empty_lines`.
- With `columns`, a repeated header keeps its first place and takes the
  last value, and integer-like headers come first, as in a JavaScript
  object.
- Errors are csv-parse's messages: `parse_csv: Invalid Record Length:
  columns length is 3, got 2 on line 4`.

Every path a script reads, lists or checks, found or not, is a dependency:
`brrrty serve` rebuilds when it changes, wherever it is in the project
(`node_modules` and `.git` included, which are otherwise never watched), when
a listed folder (or any folder a recursive listing entered) gains or loses a
name, and when a symlink on the way to a read is retargeted. Only the output
folder is never watched, so a script that reads the output folder isn't
rebuilt when it changes.

Things to watch when porting a reader:

- A regular expression with `[\s\S]` is `.` with the `s` flag:
  `regex_replace(raw, "^---\\n(.*?)\\n---\\n?(.*)$", "$2", "s")`.
- Rhai's `s.trim()` trims `s` in place and returns nothing: write
  `s.trim(); x.body = s;`, not `x.body = s.trim();`. It trims Unicode white
  space, which differs from JavaScript's at U+FEFF and U+0085.
- A Markdown file rendered with `markdownIt()` defaults is `markdown(s)`, and
  one with `html`, `linkify` or `typographer` is `markdown(s, options)`; one
  with a plugin or a rule can't be ported yet.

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

A site that writes its own feed template adds the plugin's default export,
which brings the filters and no feed. That is `feed: false`:

```js
const pluginRss = require("@11ty/eleventy-plugin-rss");
eleventyConfig.addPlugin(pluginRss);
```

```rhai
config.add_plugin("rss", #{ feed: false });
```

It takes no other options yet: `htmlBasePluginOptions` and
`posthtmlRenderOptions` are refused. The filters are version 3's
(`@11ty/eleventy-plugin-rss` 3.0.0 to 3.1.0 have the same ones);
version 2's `dateToRfc822` writes the time zone another way (`CST` for
`-0600`), so `brrrty init` ports the call only for a site that installs
version 3, and a config that adds the `rss` plugin is refused for a site
that installs another version (feeds too). `init` records the version on a
`// RSS plugin:` line, and a build warns (`config-stale`) when it changes.

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

## `brrrty serve`

`brrrty serve` stands in for `eleventy --serve`; no config change is needed.
The pages it writes are the same as `brrrty build` writes, and
`eleventy.env.runMode` is `"serve"`. What is different:

- It listens on `localhost` only. Pass `--host 0.0.0.0` to reach it from
  another device, where 11ty listens on every address.
- On a loopback address it answers only requests whose `Host` names this
  machine (`localhost`, `*.localhost`, `127.0.0.1`, `[::1]`), and others get a
  403. The live-reload WebSocket also needs an `Origin` on the same host.
  11ty checks neither.
- It refuses `..` (encoded or not) and symlinks that point outside the output
  folder. A malformed URL, or one that decodes to invalid UTF-8, gets a 400.
- `--quiet` prints only the `Server at` line, errors and warnings. 11ty's
  `--quiet` still prints the `Wrote` line, `Watching…` and `File changed`.
- An added file is reported as `File changed:`, where 11ty prints
  `File added:`.
- When the port is taken and a higher one is used, the `Server at` line ends
  with `(port N was in use)`.
- `setServerOptions` and `serverPassthroughCopyBehavior` have no equivalent:
  the server takes only `--port` and `--host`, and passthrough files are always
  copied into the output folder, as `brrrty build` copies them.
- A build with an error in a page still serves and writes the pages that
  built, as `brrrty build` does; 11ty's `--serve` writes none of that build. A page
  whose template has the error keeps serving its last version, and the
  output folder keeps its last file for it, so both stay as they were, as
  in 11ty. A page that never built is a 404. A first build with errors
  leaves the server running instead of exiting; use `--strict` for 11ty's
  exit, and to serve no build with an error.
- Every file under the input folder is watched, and deleting one rebuilds.
  So are passthrough sources outside it and the config and ignore files.
  `addWatchTarget` has no equivalent yet: a file outside those isn't watched,
  so save a file inside the input folder to rebuild after editing it.
  `setWatchJavaScriptDependencies` and `setWatchThrottleWaitTime` have nothing
  to port.
- Pages load `/.brrrty/reload-client.js` instead of `/.11ty/reload-client.js`,
  and stylesheets swapped on an edit get `?_brrrty=` instead of `?_11ty=`.
- While it runs, builds are made in `<output>/.brrrty-serve/`, which is never
  served. Leave that name out of anything that deploys the output folder by
  hand while a session runs. `brrrty build` into the same folder leaves it
  alone while a session holds it, and a second `brrrty serve` there refuses to
  start.

## Checklist

1. Port one filter or collection at a time. Keep the JS next to it while you work.
2. Use `object()` for anything built in a loop whose order can show.
3. Replace each JS operator in the table above with its helper.
4. Run the JS and the port on the same inputs in Node and compare. Include
   `undefined`, `null`, `0`, `""`, non-ASCII text and emoji.
5. Port every `addPlugin` call (see Plugins); a plugin left out is not
   noticed.
6. Run `brrrty doctor` until it has no blockers left, then
   `brrrty doctor --verify` (or `cargo xtask scoreboard` for the test
   sites). Every page that builds must be identical to 11ty's. A refused
   page names its reason; fix the port, or the pattern, until it builds.
