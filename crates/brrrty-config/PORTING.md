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
script's top level runs.)

**Other things that would make output nondeterministic are refused**, and
only the pages that hit them:

- A filter that uses randomness: declare it with
  `config.add_nondeterministic_filter("name")`.
- A regular expression or text where Rust and JavaScript would differ
  (below).

A filter that throws, returns something that isn't data, or runs too long
refuses its page, with the template's position and the script's.

## Rhai is not JavaScript

Brrrty doesn't change Rhai's operators. Where they differ, call a helper.

| JavaScript | In Rhai | Use |
|---|---|---|
| `7 / 2` is `3.5` | `7 / 2` is `3` (integer literals) | `js_div(7, 2)`, or write `7.0 / 2`. Numbers that come from data are floats already. |
| `a + b` | `+` is Rhai's: strings join, numbers add, mixed is an error | `js_concat(a, b)` (string if either side is a string, array, object or date; else the numeric sum, `undefined` is `NaN`) |
| `String(v)`, `` `${v}` `` | `to_string()` is Rhai's for arrays and maps | `js_string(v)` (`[1,null]` is `1,`, objects are `[object Object]`) |
| `arr.sort()` | `sort()` compares values, in place | `js_sort(arr)`: by `String(item)` in UTF-16 order, `undefined` last, returns a new array |
| `arr.sort((a, b) => ...)` | `sort(fn)` needs `-1/0/1` integers and sorts in place | `js_sort_by(arr, \|a, b\| ...)`: any number (fractions, `NaN` is 0) or a bool; same result as V8, including for short arrays with sloppy comparators |
| `Math.round(x)` | `round()` rounds halves away from zero | `js_round(x)`: halves round up (`-4.5` is `-4`), `-0` stays `-0` |
| `if (x)` | Only `bool` is allowed | `if truthy(x)` |
| `x \|\| y` | `\|\|` needs bools | `if truthy(x) { x } else { y }` |
| `new Date(ts)` | | `date(ts)` takes a number, a date, `()` (invalid) or null (the epoch) |
| `new URL(u).hostname` | | `url_hostname(u)`, or `()` when `u` isn't an absolute URL |
| `d.toLocaleDateString('en-US', {year:'numeric', month:'long', day:'numeric'})` | | `to_locale_date_string(d)` |
| `Date.now()` | | `get_time(now())` (follows `SOURCE_DATE_EPOCH`) |
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
  where ICU's name can't be told from the table. JavaScript's
  `format = 'dd-LLL-yyyy'` default is `format ?? "dd-LLL-yyyy"`.
- `markdown` and `markdown_inline` are `markdown-it` with no options, so HTML
  is **off** (11ty's own instance has it on) and the indented code rule is
  **on**. `null`, `()`, `""`, `0` and `false` give `""`, as `s || ''` does;
  any other value that isn't a string is an error, as in markdown-it.

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
  class; no flags other than `g i m s u`.

Some text is refused, since the answer would differ:

- non-ASCII text when `i` is used and the pattern has letters (Rust folds
  more characters, such as the Kelvin sign);
- text with characters outside the BMP (emoji) when the pattern could split
  a surrogate pair (`.`, a negated class, an empty match), unless you add
  `u`;
- text with `\r`, U+2028 or U+2029 when `m` is used with `^` or `$`.

## Checklist

1. Port one filter at a time. Keep the JS next to it while you work.
2. Use `object()` for anything built in a loop whose order can show.
3. Replace each JS operator in the table above with its helper.
4. Run the JS and the port on the same inputs in Node and compare. Include
   `undefined`, `null`, `0`, `""`, non-ASCII text and emoji.
5. Run `cargo xtask scoreboard`. Every page that builds must be identical to
   11ty's. A refused page names its reason; fix the port, or the pattern,
   until it builds.
