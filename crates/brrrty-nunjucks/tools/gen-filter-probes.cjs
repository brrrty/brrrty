// Regenerates tests/probes/filters.json: every Nunjucks 3.2.4 built-in
// filter, with edge-case arguments, applied to values of every type, as
// Nunjucks renders it for 11ty (`new Environment(loader, { dev: true })`),
// plus Eleventy's own filters (slug, slugify, url, log) registered the way
// Eleventy registers them.
//
//   NUNJUCKS=/path/to/node_modules/nunjucks \
//   ELEVENTY=/path/to/node_modules/@11ty/eleventy \
//   TZ=UTC node tools/gen-filter-probes.cjs > tests/probes/filters.json
//
// Run it from crates/brrrty-nunjucks with Node 20 or later (the file was
// made with v26.2.0, whose V8 words the error messages).
//
// Each probe renders `X | f(args)` twice: printed, which shows the
// autoescaping, and through `dump`, which shows whether the result is a
// SafeString (`{"val":...,"length":...}`) or a plain value. All probes
// share one data object, so the file holds it once.

const nunjucks = require(process.env.NUNJUCKS || (() => { throw new Error('set NUNJUCKS to the nunjucks package directory'); })());
const path = require('path');

const data = {
  n: null, f: false, t: true, z: 0, i: 5, d: -3.7, big: 1e21, nan: 'abc',
  e: '', s: 'abc', w: ' a b  c ', h: 'Hello World', html: '<b>x</b> & "y"',
  arr: [1, 2, 3], sarr: ['b', 'a', 'C', 'd'], narr: [3, 1, 2, 10, -1], mixed: [1, 'a', null],
  ea: [], objs: [{ n: 'b', v: 2, g: 'x' }, { n: 'a', v: 1, g: 'y' }, { n: 'c', v: 3, g: 'x' }],
  o: { b: 1, a: 2, C: 3 }, eo: {}, no: { z: 'x', y: 1 }, nums: { a: 3, b: 1, c: 2 },
  dt: '__DATE__2020-05-05T10:20:30.123Z',
  nested: { a: { b: [1, { c: 2 }] }, k: [null, 'x'] },
  emoji: '😀 hi', ws: '  a  \n  b  \n\n\n\nc  ', crlf: 'a\r\nb\nc\n\nd\re',
  urls: 'see http://a.com/x, www.b.org. mail me@x.io <a.net> (c.com) end&gt;',
  tags: '<p>Hi <b>there</b></p><!-- c --> <br/> x < y <a href="q">l</a>',
  posts: [{ d: '__DATE__2021-03-01T00:00:00Z', t: 'b' }, { d: '__DATE__2020-01-01T00:00:00Z', t: 'a' }, { d: '__DATE__2021-03-01T00:00:00Z', t: 'c' }],
  up: 'ÀbÇ ß', num: '  42abc', hex: '0x1F', flt: '3.14e2xyz',
};

const inputs = ['u', 'n', 'f', 't', 'z', 'i', 'd', 'big', 'nan', 'e', 's', 'w', 'h', 'html', 'arr', 'sarr',
  'narr', 'mixed', 'ea', 'objs', 'o', 'eo', 'nums', 'dt', 'nested', 'emoji', 'ws', 'crlf', 'urls', 'tags', 'up', 'num', 'hex', 'flt'];
const safeInputs = ['s', 'e', 'w', 'h', 'html', 'i', 'arr', 'o', 'ws', 'crlf', 'urls', 'tags', 'up', 'emoji'];

// [filter, [argument lists as template text]]
const cases = [
  ['abs', ['']],
  ['batch', ['(2)', '(2, "-")', '(0)', '(3, 0)', '("2")', '(-1)', '()', '(2, "")']],
  ['capitalize', ['']],
  ['center', ['', '(10)', '(3)', '("9")', '(0)', '(-5)', '(1e3)']],
  ['default', ['("x")', '("x", true)', '()', '(0)', '("x", false)', '(1, 1)']],
  ['d', ['("x")', '("x", true)']],
  ['dictsort', ['', '(true)', '(false, "value")', '(false, "bad")', '(true, "key")', '(false, "value", 1)']],
  ['dump', ['', '(2)', '("--")', '(20)', '(-1)', '("abcdefghijklmnop")', '(1.9)', '(true)']],
  ['escape', ['']], ['e', ['']], ['safe', ['']], ['forceescape', ['']],
  ['first', ['']], ['last', ['']],
  ['float', ['', '(7)', '(default=7)']],
  ['groupby', ['("g")', '("n")', '("missing")', '()', '(1)']],
  ['indent', ['', '(2)', '(2, true)', '("3")', '(0)', '(1.5)', '(-2)', '(2, 1)']],
  ['int', ['', '(7)', '(7, 16)', '(base=16)', '(default=3)', '(0, 2)', '(0, 36)', '(0, 1)', '(0, 37)', '("x", "2")']],
  ['join', ['', '(",")', '(", ", "n")', '("-", "v")', '(1)', '(0)', '(" ", "length")']],
  ['length', ['']], ['list', ['']], ['lower', ['']], ['upper', ['']], ['nl2br', ['']],
  ['random', ['']],
  ['reject', ['', '("odd")', '("even")', '("gt", 1)', '("string")', '("bogus")', '("divisibleby", 2)']],
  ['select', ['', '("odd")', '("even")', '("gt", 1)', '("string")', '("bogus")', '("number")', '("undefined")', '("null")']],
  ['rejectattr', ['("v")', '("zz")', '("n")', '()', '(0)']],
  ['selectattr', ['("v")', '("zz")', '("n")', '()', '(0)']],
  ['replace', ['("a", "X")', '("a", "X", 1)', '("", "-")', '(1, "x")', '("a")', '("a", 5)', '("a", "$&")', '("l", "L", 0)',
    '("l", "L", -1)', '("l", "L", "2")', '("l", "L", 1.5)', '(5, 7)', '("b", "<i>")', '(",", ";")', '("", "")', '("a", "", 1)']],
  ['reverse', ['']],
  ['round', ['', '(2)', '(1, "floor")', '(0, "ceil")', '(-1)', '("2")', '(2, "foo")', '(5)', '(20)', '(-5)', '(2, "ceil")']],
  ['slice', ['(2)', '(3, "-")', '(0)', '("2")', '(1)', '(4)', '(-1)', '()', '(2, 0)']],
  ['sum', ['', '("v")', '("v", 5)', '("", 5)', '(0, "x")']],
  ['sort', ['', '(true)', '(false, true)', '(false, false, "v")', '(true, false, "n")', '(attribute="v")', '(reverse=true, case_sensitive=true)', '(false, false, "g")', '(false, true, "n")']],
  ['string', ['']],
  ['striptags', ['', '(true)']],
  ['title', ['']], ['trim', ['']],
  ['truncate', ['(5)', '(5, true)', '(5, false, "!")', '(3, true, "")', '(100)', '()', '(0)', '("4")', '(5, true, null)', '(4, false, 1)', '(2, 1, "..")']],
  ['urlencode', ['']],
  ['urlize', ['', '(10)', '(10, true)', '(4, true)', '(0)', '("x")', '(-1)']],
  ['wordcount', ['']],
];

// Inputs for filters that only mean something on certain data.
const extra = [
  '{{ "a_b <a_x> <ab_c>" | striptags }}', '{{ "<a\n>x</A\n>" | striptags }}', '{{ "<<a>>" | striptags }}',
  '{{ "x <!-- a -- b --> y <!--> z -->" | striptags }}', '{{ "<!-- unterminated" | striptags }}',
  '{{ "a < b > c" | striptags }}', '{{ "<script>1</script>" | striptags }}', '{{ "<1>x</1><é>y" | striptags }}',
  '{{ "  a   b  \\n  c  \\r\\n\\r\\n\\r\\n d  " | striptags(true) }}', '{{ "a \\u2028 b \\u2029 c" | striptags(true) }}',
  '{{ "\\u00a0 x \\u00a0" | trim }}|{{ "\\ufeff x \\ufeff" | trim }}|{{ "\\u200b x" | trim | length }}',
  '{{ "(http://a.com)" | urlize }}|{{ "<www.a.com>" | urlize }}|{{ "&lt;a.org&gt;" | urlize }}|{{ "x.net/y" | urlize }}|{{ "a@b.c" | urlize }}',
  '{{ "a@b" | urlize }}|{{ "a@b.c-d.e" | urlize }}|{{ ".com" | urlize }}|{{ "a.comx" | urlize }}|{{ "a.com:80" | urlize }}|{{ "HTTP://A.COM" | urlize }}',
  '{{ "a.com.\\nb.org" | urlize }}|{{ "a.com,b.org" | urlize }}|{{ "x\\u00a0a.com" | urlize }}|{{ "a.com)" | urlize(3) }}',
  '{{ "a b" | wordcount }}|{{ "a_b-c" | wordcount }}|{{ "é a" | wordcount }}|{{ "---" | wordcount }}|{{ "" | wordcount }}|{{ " " | wordcount }}',
  '{{ "ǅ ǆ İ ı ſ K ﬃ" | capitalize }}|{{ "ǅ ǆ İ ı ſ K ﬃ" | title }}|{{ "σας ΑΣ" | lower }}|{{ "ß ŉ" | upper }}',
  '{{ "a\\tb" | indent(2) }}|{{ "\\na" | indent(2) }}|{{ "a\\n" | indent(2, true) }}',
  '{{ "hello world foo" | truncate(11) }}|{{ "hello world foo" | truncate(12) }}|{{ "hello" | truncate(5) }}|{{ "hello" | truncate(4) }}|{{ "😀😀😀" | truncate(2, true) }}',
  '{{ 1.005 | round(2) }}|{{ 2.5 | round }}|{{ -2.5 | round }}|{{ -0.4 | round }}|{{ 0.49999999999999994 | round }}|{{ 1234.5678 | round(-2) }}|{{ 5e-324 | round(2) }}|{{ 1e21 | round(2) }}',
  '{{ 0.1 | round(1) }}|{{ 1.45 | round(1) }}|{{ 8.345 | round(2) }}|{{ 1.255 | round(2) }}|{{ 10.075 | round(2) }}|{{ 123456.789 | round(3) }}|{{ 0.000001234 | round(8) }}',
  '{{ 7 | round(1, "ceil") }}|{{ 7.001 | round(2, "ceil") }}|{{ -7.001 | round(2, "floor") }}|{{ 0.1 | round(20) }}|{{ 1 | round(400) }}|{{ 1 | round(-400) }}',
  '{{ "  12px" | int }}|{{ "-0" | int }}|{{ "0x1f" | int }}|{{ "0x1f" | int(0, 16) }}|{{ "1e3" | int }}|{{ 1e21 | int }}|{{ 0.0000001 | int }}|{{ "9007199254740993" | int }}|{{ "zz" | int(0, 36) }}|{{ "12345678901234567890123" | int }}',
  '{{ "  3.14abc" | float }}|{{ ".5" | float }}|{{ "5." | float }}|{{ "-.5e1" | float }}|{{ "Infinity" | float }}|{{ "-Infinity" | float }}|{{ "infinity" | float(1) }}|{{ "1e" | float }}|{{ "1e+" | float }}|{{ "0x10" | float }}|{{ "1_0" | float }}|{{ "+1" | float }}|{{ "." | float(9) }}',
  '{{ [1, 2, 3] | sum }}|{{ ["a", "b"] | sum }}|{{ [1, "2", 3] | sum }}|{{ [[1], [2]] | sum }}|{{ [] | sum }}|{{ [true, 1] | sum }}|{{ [null, 1] | sum }}',
  '{{ "ab" | list | dump }}|{{ o | list | dump }}|{{ [1] | list | dump }}',
  '{{ x | length }}|{{ [1, 2] | length }}|{{ "ab" | length }}|{{ {a: 1, b: 2} | length }}|{{ 12 | length }}|{{ true | length }}|{{ (1 | safe) | length }}',
  '{{ {a: 1, b: [1, 2, {c: undefined}], d: undefined, e: null, f: "x\\"y\\n\\u0001\\u2028"} | dump }}|{{ [undefined, null, 1.5, -0, 1e21, 1e-7] | dump }}',
  '{{ {a: 1, b: [1, 2], c: {}, d: []} | dump(2) }}|{{ [[], {}, [[]]] | dump(1) }}|{{ "x" | dump }}|{{ 1 | dump }}|{{ undefined | dump }}|{{ null | dump }}',
  '{{ ("a" | safe) | dump }}|{{ [("a" | safe)] | dump }}|{{ {k: ("a" | safe)} | dump }}|{{ dt | dump }}|{{ [dt] | dump(2) }}|{{ {d: dt} | dump }}',
  '{% macro m() %}x{% endmacro %}{{ m() | dump }}|{{ {f: m, g: 1} | dump }}|{{ [m, 1] | dump }}',
  '{% set a = [1] %}{{ a.push(a) | dump }}',
  '{{ [3, 1, 2] | sort | dump }}|{{ [3, 1, 2] | sort(true) | dump }}|{{ ["b", "B", "a", "A"] | sort | dump }}|{{ ["b", "B", "a", "A"] | sort(false, true) | dump }}|{{ "cba" | sort | dump }}',
  '{{ [{a: 1}, {a: 1, b: 1}, {a: 0}] | sort(false, false, "a") | dump }}|{{ [{a: {b: 2}}, {a: {b: 1}}] | sort(false, false, "a.b") | dump }}',
  '{{ [10, 9, 1, 2] | sort | dump }}|{{ ["10", "9", "1", "2"] | sort | dump }}|{{ [1.5, -1, 0, -0, 1e21] | sort | dump }}|{{ ["é", "z", "a", "😀", "\\uffff"] | sort | dump }}',
  '{{ [1, "a"] | sort | dump }}|{{ [2, undefined, 1] | sort | dump }}|{{ [null, 2, 1] | sort | dump }}|{{ [{}, {}] | sort | dump }}|{{ [[2], [1]] | sort | dump }}',
  '{{ posts | sort(false, false, "d") | dump }}|{{ posts | sort(true, false, "d") | dump }}|{{ posts | sort(attribute="d") | dump }}|{{ [posts[0].d, posts[1].d] | sort | dump }}|{{ [posts[0].d, 5] | sort | dump }}|{{ posts | sort(false, false, "t") | dump }}|{{ posts | groupby("d") | dump }}|{{ posts | dictsort | dump }}',
  '{{ [3, 1] | sort(attribute="a") | dump }}|{{ [{a: 1}] | sort(attribute="a") | dump }}|{{ [null, {a: 1}] | sort(attribute="a") | dump }}|{{ [] | sort | dump }}|{{ undefined | sort | dump }}|{{ 5 | sort | dump }}',
  '{{ {b: 1, a: 2, "B": 0} | dictsort | dump }}|{{ {b: 1, a: 2, "B": 0} | dictsort(true) | dump }}|{{ {b: "x", a: "Y"} | dictsort(false, "value") | dump }}|{{ {b: "x", a: "Y"} | dictsort(true, "value") | dump }}',
  '{{ {b: 2, a: 1, 10: 3, 9: 4} | dictsort | dump }}|{{ {b: 2, a: 1, 10: 3, 9: 4} | dictsort(false, "value") | dump }}|{{ {} | dictsort | dump }}|{{ [] | dictsort | dump }}|{{ "s" | dictsort | dump }}|{{ ("s" | safe) | dictsort | dump }}',
  '{{ [{g: "a", v: 1}, {g: "b", v: 2}, {g: "a", v: 3}] | groupby("g") | dump }}|{{ [{g: 1}, {g: 2}, {g: 1}] | groupby("g") | dump }}|{{ [{g: 10}, {g: 9}, {g: "x"}] | groupby("g") | dump }}',
  '{{ [{g: {h: 1}}, {g: {h: 2}}] | groupby("g.h") | dump }}|{{ [{g: {h: 1}}, {}] | groupby("g.h") | dump }}|{{ ["a", "bb", "cc"] | groupby("length") | dump }}|{{ ["a", "b"] | groupby | dump }}|{{ [] | groupby("x") | dump }}',
  '{{ [{g: "constructor"}] | groupby("g") | dump }}|{{ [{g: "__proto__"}] | groupby("g") | dump }}|{{ [{g: "toString"}] | groupby("g") | dump }}|{{ [{g: "hasOwnProperty"}] | groupby("g") | dump }}',
  '{{ [null] | groupby("g") | dump }}|{{ [1, 2] | groupby("g") | dump }}|{{ "ab" | groupby("g") | dump }}|{{ undefined | groupby("g") | dump }}|{{ {length: 1, 0: {g: 1}} | groupby("g") | dump }}',
  '{{ [1, 2, 3, 4, 5] | batch(2, 0) | dump }}|{{ [1, 2, 3] | batch(2, "") | dump }}|{{ "abcde" | batch(2) | dump }}|{{ undefined | batch(2) | dump }}|{{ {length: 3, 0: "a", 1: "b"} | batch(2) | dump }}|{{ ("abc" | safe) | batch(2) | dump }}',
  '{{ [1, 2, 3, 4, 5] | slice(3) | dump }}|{{ [1, 2, 3, 4, 5] | slice(3, "x") | dump }}|{{ [1, 2] | slice(3, "x") | dump }}|{{ "abcde" | slice(2) | dump }}|{{ "abcde" | slice(2, "x") | dump }}|{{ undefined | slice(2) | dump }}|{{ [] | slice(2, 1) | dump }}',
  '{{ [1, 2, 3] | select("odd") | dump }}|{{ [1, 2, 3] | reject("odd") | dump }}|{{ "abc" | select | dump }}|{{ undefined | select | dump }}|{{ 5 | select | dump }}|{{ [0, 1, "", "a", null] | select | dump }}|{{ [0, 1, "", "a", null] | reject | dump }}|{{ {length: 2, 0: 1, 1: 0} | select | dump }}|{{ ("ab" | safe) | select | dump }}',
  '{{ [1, 2, 3] | select("equalto", 2) | dump }}|{{ [1, 2, 3] | select("ne", 2) | dump }}|{{ [1, 2, 3] | select("lt", 3) | dump }}|{{ ["a", "B"] | select("lower") | dump }}|{{ ["a", "B"] | select("upper") | dump }}|{{ [1, 2, 3] | select("callable") | dump }}|{{ ["a"] | select("iterable") | dump }}|{{ [{}] | select("mapping") | dump }}',
  '{{ [{a: 1}, {a: 0}, {}] | selectattr("a") | dump }}|{{ [{a: 1}, {a: 0}, {}] | rejectattr("a") | dump }}|{{ [null] | selectattr("a") | dump }}|{{ "ab" | selectattr("a") | dump }}|{{ undefined | selectattr("a") | dump }}|{{ [[1], []] | selectattr("length") | dump }}',
  '{{ "a-b-c" | replace("-", "+") }}|{{ "aaa" | replace("a", "b", 2) }}|{{ "aaa" | replace("aa", "b") }}|{{ "abc" | replace("", "-") }}|{{ "abc" | replace("", "-", 1) }}|{{ "" | replace("", "-") }}|{{ "😀" | replace("😀", "x") }}|{{ "a😀b" | replace("b", "x") }}|{{ "a😀b" | replace("", "") }}',
  '{{ "x" | replace("x", undefined) }}|{{ "x" | replace("x", null) }}|{{ "x" | replace("x", 1.5) }}|{{ "x" | replace("x", [1, 2]) }}|{{ "x" | replace("x", {}) }}|{{ "x" | replace("x", ("<" | safe)) }}|{{ "x" | replace("x", "<") }}|{{ ("x" | safe) | replace("x", "<") }}|{{ "<x>" | replace("x", "&") }}',
  '{{ 12345 | replace(3, "x") }}|{{ 12345 | replace("3", "x") | dump }}|{{ 12345 | replace("9", "x") | dump }}|{{ true | replace("t", "x") | dump }}|{{ undefined | replace("t", "x") | dump }}|{{ [1] | replace("t", "x") | dump }}|{{ "abc" | replace(undefined, "x") }}|{{ "abc" | replace(null, "x") }}|{{ "abc" | replace(("a" | safe), "x") }}',
  '{{ "abab" | replace("a", "x", "1") }}|{{ "abab" | replace("a", "x", null) }}|{{ "abab" | replace("a", "x", undefined) }}|{{ "abab" | replace("a", "x", true) }}|{{ "abab" | replace("a", "x", "a") }}|{{ "abab" | replace("a", "x", -5) }}|{{ "abab" | replace("a", "x", 0) }}|{{ "abab" | replace("a", "x", false) }}',
  '{{ "%20&=" | urlencode }}|{{ "é😀 ~!*()\'" | urlencode }}|{{ [["a", 1], ["b c", "d&"]] | urlencode }}|{{ {a: 1, "b c": "d&", e: [1, 2], f: null, g: undefined} | urlencode }}|{{ [] | urlencode }}|{{ {} | urlencode }}|{{ undefined | urlencode }}|{{ 5 | urlencode }}|{{ [["a"], []] | urlencode }}|{{ ("a b" | safe) | urlencode }}',
  '{{ [1, null] | sum(attribute="v") }}', '{{ [null] | selectattr("a", 1) | dump }}', '{{ [1] | join(attribute="v") }}', '{{ [["😀", "a"]] | urlencode }}|{{ ["😀a"] | urlencode }}', '{{ ["😀"] | urlencode }}',
  '{{ ["ab", "c"] | urlencode }}|{{ [1] | urlencode }}|{{ [null] | urlencode }}|{{ [["a", {}]] | urlencode }}|{{ [["a", dt]] | urlencode }}|{{ [[1, [2, 3]]] | urlencode }}',
  '{{ "a\\nb\\r\\nc\\rd" | nl2br | dump }}|{{ ("a\\nb" | safe) | nl2br | dump }}|{{ 1 | nl2br }}|{{ null | nl2br | dump }}|{{ false | nl2br }}|{{ [1] | nl2br }}',
  '{{ x | default("d") }}|{{ null | default("d") }}|{{ false | default("d") }}|{{ false | default("d", true) }}|{{ "" | default("d", true) }}|{{ 0 | default("d", true) }}|{{ [] | default("d", true) | dump }}|{{ ("" | safe) | default("d", true) | dump }}|{{ undefined | default | dump }}',
  '{{ "a" | center(5) | dump }}|{{ "ab" | center(5) | dump }}|{{ ("ab" | safe) | center(7) | dump }}|{{ "abc" | center(2) | dump }}|{{ 5 | center(4) | dump }}|{{ undefined | center(4) | dump }}|{{ "a" | center(4.5) | dump }}|{{ "a" | center("x") | dump }}',
  '{{ "abc" | first }}|{{ "abc" | last }}|{{ [] | first }}|{{ [] | last }}|{{ undefined | first }}|{{ {a: 1} | first }}|{{ {0: "z", length: 1} | last }}|{{ ("abc" | safe) | first }}|{{ 12 | first }}|{{ null | last }}',
  '{{ "ab" | join("-") }}|{{ [1, null, undefined, "a", [2, 3], {}] | join("|") }}|{{ undefined | join }}|{{ undefined | join("-", "x") }}|{{ "ab" | join("-", "x") }}|{{ {a: 1} | join }}|{{ [("<" | safe), "<"] | join(",") }}|{{ [1, 2] | join(("<" | safe)) }}',
  '{{ [1, 2, 3] | reverse | dump }}|{{ "abc" | reverse | dump }}|{{ ("abc" | safe) | reverse | dump }}|{{ undefined | reverse | dump }}|{{ 5 | reverse | dump }}|{{ {length: 2, 0: "a", 1: "b"} | reverse | dump }}|{{ "😀a" | reverse | dump }}|{{ "ab😀" | reverse | dump }}',
  '{{ x | string | dump }}|{{ 5 | string | dump }}|{{ [1, [2]] | string | dump }}|{{ {} | string | dump }}|{{ true | string | dump }}|{{ null | string | dump }}|{{ ("a" | safe) | string | dump }}|{{ dt | string | dump }}|{{ -0 | string | dump }}',
  '{{ "a" | safe | upper | dump }}|{{ "a" | safe | lower | dump }}|{{ "a" | safe | capitalize | dump }}|{{ "a b" | safe | title | dump }}|{{ " a " | safe | trim | dump }}|{{ "a" | safe | truncate(0) | dump }}|{{ "abc" | safe | truncate(1) | dump }}|{{ "a" | safe | replace("a", "b") | dump }}',
  '{{ "a" | safe | indent | dump }}|{{ "a" | safe | striptags | dump }}|{{ "a" | safe | center(3) | dump }}|{{ "a" | safe | string | dump }}|{{ "a" | safe | nl2br | dump }}|{{ "a" | safe | urlize | dump }}|{{ "a" | safe | wordcount | dump }}|{{ "a" | safe | list | dump }}',
  '{{ [1, 2, 3] | random }}|{{ [7] | random }}|{{ [] | random | dump }}|{{ "x" | random }}|{{ undefined | random }}|{{ {length: 0} | random | dump }}|{{ {length: 1, 0: "q"} | random }}',
  '{{ 3 | abs }}|{{ -3 | abs }}|{{ "-3" | abs }}|{{ undefined | abs }}|{{ null | abs }}|{{ true | abs }}|{{ [] | abs }}|{{ [-2] | abs }}|{{ {} | abs }}|{{ "" | abs }}|{{ dt | abs }}|{{ -0 | abs }}|{{ ("-4" | safe) | abs }}',
  '{{ [1, 2, 3] | sum("x") }}|{{ [{x: 1}, {x: 2}] | sum("x") }}|{{ [{x: "a"}, {x: "b"}] | sum("x") }}|{{ [{x: 1}, null] | sum("x") }}|{{ "ab" | sum("x") }}|{{ undefined | sum("x") }}|{{ undefined | sum }}|{{ "ab" | sum }}|{{ {} | sum }}|{{ [1, 2] | sum(null, "a") }}|{{ [1] | sum(0, null) }}',
  '{{ "Ab Cd" | title }}|{{ "ab  cd" | title }}|{{ "aB-cD eF" | title }}|{{ "" | title }}|{{ " a" | title }}|{{ 5 | title }}|{{ null | title }}|{{ [1] | title }}|{{ false | title | dump }}|{{ true | title }}',
  '{{ "Ab" | capitalize }}|{{ "" | capitalize | dump }}|{{ 5 | capitalize }}|{{ null | capitalize | dump }}|{{ false | capitalize | dump }}|{{ true | capitalize }}|{{ [1] | capitalize }}|{{ "😀" | capitalize }}|{{ "ǆa" | capitalize }}',
  '{{ "a" | lower }}|{{ 5 | lower }}|{{ null | lower }}|{{ false | lower | dump }}|{{ true | lower }}|{{ [1] | lower }}|{{ {} | lower }}|{{ ("A" | safe) | lower | dump }}',
  '{{ "a" | trim }}|{{ 5 | trim }}|{{ null | trim }}|{{ false | trim }}|{{ [" a "] | trim }}|{{ undefined | trim }}',
  '{{ "x" | truncate(1, false, undefined) }}|{{ "xy" | truncate(1, false, ("<" | safe)) }}|{{ ("xy" | safe) | truncate(1, true, "<") }}|{{ [1, 2, 3] | truncate(2) | dump }}|{{ 12345 | truncate(2) }}|{{ null | truncate(2) | dump }}|{{ false | truncate(2) | dump }}|{{ true | truncate(2) }}',
  '{{ "ab cd ef" | truncate(5) }}|{{ "ab cd ef" | truncate(6) }}|{{ "ab cd ef" | truncate(7) }}|{{ "abcdef" | truncate(3) }}|{{ "ab cd ef" | truncate(5, true) }}|{{ "a b c" | truncate(2, false, "") }}|{{ "ab cd" | truncate(-1) }}|{{ "ab cd" | truncate(1.5) }}',
  '{{ x | urlize }}|{{ 5 | urlize }}|{{ null | urlize }}|{{ [1] | urlize }}|{{ ("a.com" | safe) | urlize | dump }}',
  '{{ "x" | batch(2) | dump }}|{{ 5 | batch(2) | dump }}|{{ null | batch(2) | dump }}|{{ true | slice(2) | dump }}',
  '{{ x | wordcount | dump }}|{{ 5 | wordcount | dump }}|{{ [1] | wordcount | dump }}|{{ ("a b" | safe) | wordcount | dump }}|{{ ("" | safe) | wordcount | dump }}|{{ true | wordcount | dump }}|{{ false | wordcount | dump }}',
  '{{ x | striptags | dump }}|{{ 5 | striptags | dump }}|{{ null | striptags | dump }}|{{ false | striptags | dump }}|{{ [1] | striptags | dump }}|{{ ("<b>a</b>" | safe) | striptags | dump }}',
  '{{ "a" | e }}|{{ ("<" | e) | e }}|{{ ("<" | safe) | e }}|{{ ("<" | e) | forceescape }}|{{ 5 | e }}|{{ [1, "<"] | e }}|{{ {} | e }}|{{ undefined | e | dump }}|{{ null | safe | dump }}|{{ false | safe | dump }}|{{ 0 | e | dump }}',
  '{{ "<" | safe | escape }}|{{ "<" | escape | safe }}|{{ "<" | escape | escape }}|{{ "&amp;" | forceescape | forceescape }}',
  '{{ "x" | foo }}', '{{ "x" | upperr }}', '{{ "x" | Upper }}',
  '{{ "ab" | upper | lower | title | length }}|{{ [1, 2] | join("-") | upper }}|{{ "a b" | title | replace(" ", "_") | trim }}',
  '{% filter upper %}a<b{% endfilter %}|{% filter title %}hi there{% endfilter %}|{% filter replace("a", "b") %}aa{% endfilter %}|{% filter safe %}<{% endfilter %}|{% filter escape %}<{% endfilter %}',
  '{% filter nosuch %}a{% endfilter %}', '{% filter truncate(3) %}abcdef{% endfilter %}|{% filter default("d") %}{% endfilter %}',
  '{% for x in [3, 1, 2] | sort %}{{ x }}{% endfor %}|{% for k, v in {b: 1, a: 2} | dictsort %}{{ k }}{{ v }}{% endfor %}|{% for g, items in [{c: "x"}, {c: "y"}, {c: "x"}] | groupby("c") %}{{ g }}:{{ items | length }};{% endfor %}',
  '{% for row in [1, 2, 3, 4, 5] | batch(2, "-") %}[{{ row | join(",") }}]{% endfor %}|{% for col in [1, 2, 3, 4, 5] | slice(2) %}[{{ col | join(",") }}]{% endfor %}',
  '{% for i in "abc" | list %}{{ i }}.{% endfor %}|{% for p in {a: 1} | list %}{{ p.key }}={{ p.value }}{% endfor %}',
  '{{ "a" | join("-") }}|{{ "a,b" | replace(",", ", ") | wordcount }}|{{ [1, 2, 3] | first + 1 }}|{{ ([1, 2, 3] | last) * 2 }}|{{ "5" | int + 1 }}|{{ "5.5" | float + 1 }}',
  '{{ "A" | lower == "a" }}|{{ [1, 2] | length == 2 }}|{{ "x" | upper ~ "y" }}|{{ ("x" | upper) ~ "y" }}|{{ 1 + 2 | abs }}',
  '{{ x | length > 0 }}|{{ (x | length) > 0 }}|{{ "ab" | length > 1 and "c" | length < 2 }}',
  '{% set s = "<b>" | safe | upper %}{{ s }}|{% set t = "<b>" | upper | safe %}{{ t }}',
  '{% macro m(a) %}{{ a | upper }}{% endmacro %}{{ m("<x>") }}|{{ m("<x>") | lower }}|{{ m("<x>") | length }}|{{ m("<x>") | replace("X", "y") }}|{{ m("a b") | title | dump }}',
  '{% macro m() %}<i>{% endmacro %}{{ m() | trim | dump }}|{{ m() | nl2br | dump }}|{{ m() | striptags | dump }}|{{ m() | truncate(2) | dump }}|{{ m() | indent | dump }}|{{ m() | center(5) | dump }}|{{ m() | capitalize | dump }}|{{ m() | string | dump }}',
  '{{ "ab" | length(1, 2, 3) }}|{{ "ab" | upper(1) }}|{{ "ab" | trim(true) }}|{{ [1, 2] | join(",", null, 5) }}',
  '{{ "ab" | truncate(length=1) }}|{{ "ab" | truncate(1, end="!") }}|{{ "abc" | truncate(2, killwords=true, end="~") }}|{{ "abc" | center(width=7) | dump }}|{{ "ab" | replace(old="a", new_="x") }}',
  '{{ [3, 1, 2] | sort(reverse=true) | dump }}|{{ ["b", "A"] | sort(case_sensitive=true) | dump }}|{{ [{a: 2}, {a: 1}] | sort(attribute="a") | dump }}|{{ [{a: 2}, {a: 1}] | sort(true, false, "a", 1) | dump }}|{{ [2, 1] | sort(undefined, undefined, undefined) | dump }}',
  '{{ "11" | int(base=2) }}|{{ "x" | int(default=9) }}|{{ "x" | int(9, 10, 11) }}|{{ "11" | int(0, 2, 99) }}|{{ "x" | int("d") }}|{{ "x" | int(default="d", base=16) }}',
  '{{ "1" | float(default=2) }}|{{ "x" | float(default=2) }}|{{ "x" | float(2, 3) }}',
  '{{ 5 | default(1, 2, 3) }}|{{ undefined | default(default=3) }}|{{ undefined | default(boolean=true) | dump }}',
];

const tmplsFor = (name, args) => {
  const out = [];
  for (const a of args) {
    const f = `${name}${a}`;
    for (const x of inputs) out.push(`{{ ${x} | ${f} }}|{{ ${x} | ${f} | dump }}`);
    for (const x of safeInputs) out.push(`{{ (${x} | safe) | ${f} }}|{{ (${x} | safe) | ${f} | dump }}`);
  }
  return out;
};

const dates = (v) => {
  if (typeof v === 'string' && v.startsWith('__DATE__')) return new Date(v.slice(8));
  if (Array.isArray(v)) return v.map(dates);
  if (v && typeof v === 'object') {
    const o = {};
    for (const [k, x] of Object.entries(v)) o[k] = dates(x);
    return o;
  }
  return v;
};

(async () => {
  const env = new nunjucks.Environment(null, { dev: true });
  if (process.env.ELEVENTY) {
    const base = path.join(process.env.ELEVENTY, 'src/Filters');
    const url = (await import(path.join(base, 'Url.js'))).default;
    const slug = (await import(path.join(base, 'Slug.js'))).default;
    const slugify = (await import(path.join(base, 'Slugify.js'))).default;
    // As Eleventy's default config adds them, with the path prefix `/`.
    env.addFilter('slug', slug);
    env.addFilter('slugify', slugify);
    env.addFilter('url', function (u, over) { return url.call(this, u, over && typeof over === 'string' ? over : '/'); });
    env.addFilter('log', (input) => input);
    const ex = [];
    const vs = ['u', 'n', 'f', 's', 'e', 'html', 'urls', 'i', 'arr', 'o', 'emoji', 'up', 'w'];
    for (const f of ['slug', 'slugify', 'url', 'log']) {
      for (const a of ['', '("/p/")', '("")', '(("/q/" | safe))', '({lower: false})', '(lower=false)', '(1)']) {
        for (const x of vs) ex.push(`{{ ${x} | ${f}${a} }}|{{ ${x} | ${f}${a} | dump }}`);
        for (const x of ['s', 'e', 'html', 'w', 'up']) ex.push(`{{ (${x} | safe) | ${f}${a} }}|{{ (${x} | safe) | ${f}${a} | dump }}`);
      }
    }
    for (const u of ['http://a.b/<c>', '//x/<y>', '//', '/a/<b>', 'a/<b>', '', ' ', 'mailto:a@b.c', 'ftp://x']) {
      const lit = JSON.stringify(u);
      ex.push(`{{ (${lit} | safe) | url }}|{{ (${lit} | safe) | url | dump }}|{{ ${lit} | url }}|{{ ${lit} | url | dump }}|{{ ${lit} | url(("/z" | safe)) }}`);
    }
    extra.push(...ex);
    extra.push('{{ "x" | slug | upper }}', '{{ ("<A B>" | safe) | slug | dump }}', '{{ ("<A B>" | safe) | slugify | dump }}', '{{ ("Hi <b>" | safe) | log | dump }}', '{{ "Hi <b>" | log }}');
  }
  const templates = [];
  for (const [name, args] of cases) templates.push(...tmplsFor(name, args));
  templates.push(...extra);
  const probes = [];
  for (const t of templates) {
    const entry = { t };
    try {
      const rendered = env.renderString(t, dates(JSON.parse(JSON.stringify(data))));
      if (rendered.isWellFormed()) {
        entry.out = rendered;
      } else {
        entry.lone_surrogate = true;
        entry.out = rendered.toWellFormed();
      }
    } catch (e) {
      const parts = String(e.message).split('\n  ');
      entry.err = parts[parts.length - 1];
    }
    probes.push(entry);
  }
  process.stdout.write(JSON.stringify({ data, probes }, null, 0).replace(/\},\{"t"/g, '},\n{"t"') + '\n');
})();
