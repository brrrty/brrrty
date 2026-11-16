// The templates and data behind tests/probes/filters.json. Used by gen-filter-probes.cjs.
const D = {
  s: "Hello, World", e: "", ws: "  \t x y \n ", emoji: "😀abc", cjk: "日本語テキスト", mixed: "héllo wörld 😀 ok",
  n: 5, f: -2.5, big: 1e21, t: true, fl: false, nul: null,
  a: [1, 2, 3], ea: [], na: [3, "10", 2, "9", "a", null, true], strs: ["banana", "Apple", "cherry", "apple"],
  nested: [[1, [2]], "x"], o: { a: 1, b: [1, 2] },
  people: [{ name: "Ann", age: 30, tags: ["x"], meta: { ok: true } }, { name: "Bo", age: 25, meta: { ok: false } }, { name: "Cy", age: 30 }, { name: "Di" }],
  d: { $date: "2024-03-05T12:34:56.789Z" }, html: "<p>Hi & <b>\"there\"</b> 'x'</p>",
  dups: [1, 2, 2, 3, 1, "1", null, null], withnil: [1, null, 2],
  dupsort: [{ k: 1, v: "a" }, { k: 0, v: "b" }, { k: 1, v: "c" }, { k: 0, v: "d" }],
  words: ["banana", "Apple", "cherry"], suffix: "!!", url: "a b&c=d/e?f=é~!*'()[]#", b64: "aGVsbG8=", pct: "a%20b%2Bc+d%C3%A9",
  tags: "a,b,,c,,", sp: "a b  c   d e f g", long: "The quick brown fox jumps over the lazy dog and keeps on running far away",
  num: "3.7", numneg: "-3.2", sci: "1e3", hex: "0x10", obj2: { length: 3, x: 1 }, strlen: { size: 9 }, cjkmix: "Hello 日本語 world テスト",
};
const plain = [];
const add = (s) => plain.push({ src: s });
const out = (e) => `{{ ${e} }}`;

const STR_IN = ["s", "e", "emoji", "cjk", "n", "nul", "zz", "a", "ws", "html", "mixed", "d", "t", "o", "'a-b'"];
const ARR_IN = ["a", "ea", "na", "strs", "nested", "s", "nul", "zz", "o", "n", "dups", "withnil", "people", "emoji", "d"];
const NUM_IN = ["n", "f", "big", "t", "nul", "zz", "num", "'abc'", "' 12 '", "a", "o", "hex", "sci", "numneg", "0.5", "2.5", "-0.5", "1234.5678", "0", "-0", "d", "''", "emoji"];
const STR_ARG = ["'l'", "'o'", "'xyz'", '""', "s", "n", "nul", "zz", "a", "emoji", "'😀'", "'Hello'"];
const NUM_ARG = ["0", "1", "2", "3", "-1", "-3", "1.5", "'2'", "nul", "zz", "true", "''", "'x'", "big", "a", "10", "100", "0.1"];

function fam(filter, inputs, args) {
  for (const i of inputs) {
    add(out(`${i} | ${filter}`));
    for (const a of args) add(out(`${i} | ${filter}: ${a}`));
  }
}
// no-arg families
for (const f of ["upcase", "downcase", "capitalize", "lstrip", "rstrip", "strip", "strip_newlines", "squish", "normalize_whitespace", "strip_html",
  "escape", "xml_escape", "escape_once", "newline_to_br", "url_encode", "cgi_escape", "uri_escape", "slugify", "base64_encode", "sha256", "number_of_words", "size", "to_integer", "json", "jsonify", "inspect", "raw", "array_to_sentence_string", "split: ','"]) {
  for (const i of [...STR_IN, "url", "pct", "tags", "sp", "long", "cjkmix", "num", "people", "nested", "dups", "'  A\\r\\nB  '"]) add(out(`${i} | ${f}`));
}
for (const i of [...STR_IN, "url", "pct", "b64", "'%'", "'%E0%A4%A'", "'%C3'", "'a+b'", "'%F0%9F%98%80'", "'%ED%A0%80'"]) add(out(`${i} | url_decode`));
for (const i of [...STR_IN, "b64", "'aGVsbG8'", "'aGVsbG8=='", "' aG Vs\\nbG8= '", "'a'", "'ab'", "'abc'", "'abcd'", "'ab=c'", "'-_-_'", "'////'", "'8J+YgA=='", "'/w=='", "'AAAA'"]) add(out(`${i} | base64_decode`));
add(out(`'a' | hmac_sha256: 'k'`)); add(out(`s | hmac_sha256: s`)); add(out(`emoji | hmac_sha256: cjk`)); add(out(`n | hmac_sha256: nul`)); add(out(`'a' | hmac_sha256`));
add(out(`long | hmac_sha256: long`));
// slugify modes
for (const m of ["'default'", "'raw'", "'pretty'", "'ascii'", "'latin'", "'none'", "'bogus'", "nul", "zz", "1"]) for (const c of ["", ", true", ", false", ", 'x'", ", nul", ", 0"]) for (const i of ["'Hello, Wörld! ñandú -- 日本 é'", "'  --A_b.c~d!e$f&g(h)i+j,k;l=m@n  '", "'ß æ œ ð þ Ÿ'", "'x'", "''", "'-a-'", "'--a--'"]) add(out(`${i} | slugify: ${m}${c}`));
add(out(`'Hello' | slugify: 'default', true`));
// escapes & html
for (const i of ["'<script>a<b</script>x'", "'<style>x</style>y'", "'a<!-- c -->b'", "'<p>a<b'", "'a<script>b'", "'<!-- unclosed'", "'<script>unclosed'", "'a < b > c'", "'<a href=\"x>y\">t</a>'", "'<<>>'", "'x<>y'", "'<style>a<script>b</style>c</script>d'", "'&amp; &lt; &gt; &#34; &#39; &quot; &#x27; &amp;lt;'", "'a\\r\\nb\\nc\\rd'"]) {
  add(out(`${i} | strip_html`)); add(out(`${i} | escape_once`)); add(out(`${i} | escape`)); add(out(`${i} | newline_to_br`)); add(out(`${i} | strip_newlines`));
}
// string filters with one arg
for (const f of ["append", "prepend", "remove", "remove_first", "remove_last", "split", "lstrip", "rstrip", "strip", "array_to_sentence_string", "number_of_words"]) fam(f, STR_IN.slice(0, 12), STR_ARG);
for (const f of ["replace", "replace_first", "replace_last"]) {
  for (const i of ["s", "e", "emoji", "cjk", "nul", "zz", "n", "a", "mixed"]) for (const p of ["'l'", '""', "'o'", "'😀'", "nul", "n"]) for (const r of ["'L'", '""', "'$&'", "'$1'", "nul", "'$$'", "'😀'"]) add(out(`${i} | ${f}: ${p}, ${r}`));
}
add(out(`s | replace: 'l'`)); add(out(`s | replace`)); add(out(`s | replace: 'l', 'L', 'x'`)); add(out(`s | replace_first: 'l'`));
for (const i of ["s", "e", "emoji", "cjk", "ws", "tags", "'a,b'", "','", "',,'", "'abc'", "mixed"]) for (const sep of ["','", '""', "nul", "zz", "' '", "'b'", "'😀'", "',,'", "'abc'", "n"]) add(out(`${i} | split: ${sep} | join: '|'`)), add(out(`${i} | split: ${sep} | size`));
// truncate etc
for (const i of ["s", "e", "emoji", "cjk", "mixed", "n", "nul", "long", "'😀😀😀😀😀'", "'abc'"]) {
  for (const l of ["0", "1", "2", "3", "4", "5", "8", "50", "-1", "2.5", "'3'", "'x'", "nul", "zz", "true", "a"]) {
    add(out(`${i} | truncate: ${l}`));
    for (const o of ["''", "'--'", "'😀'", "nul", "n", "'....'"]) add(out(`${i} | truncate: ${l}, ${o}`));
  }
  add(out(`${i} | truncate`));
}
for (const i of ["s", "e", "ws", "sp", "long", "cjkmix", "n", "nul", "'a '", "' a b'", "'a\\nb\\tc'", "'😀 😀 😀'"]) {
  for (const w of ["0", "1", "2", "3", "5", "15", "-1", "1.5", "'2'", "'x'", "nul", "zz", "true", "a"]) {
    add(out(`${i} | truncatewords: ${w}`));
    for (const o of ["''", "'--'", "nul", "n"]) add(out(`${i} | truncatewords: ${w}, ${o}`));
  }
  add(out(`${i} | truncatewords`));
}
// trims with chars
for (const f of ["strip", "lstrip", "rstrip"]) for (const i of ["'xxhixx'", "'  hi  '", "'😀hi😀'", "'abcba'", "'--a--'", "ws", "e", "n"]) for (const c of ["'x'", "'😀'", "'ab'", "'-'", '""', "' '", "nul", "zz", "0", "1", "true", "a"]) add(out(`${i} | ${f}: ${c}`));
// number_of_words
for (const i of ["cjkmix", "s", "e", "ws", "'  a  b  '", "cjk", "'テスト test'", "'a'", "n", "nul", "zz", "'한국어 text'", "'汉字'", "emoji"]) for (const m of ["", ": 'cjk'", ": 'auto'", ": 'x'", ": nul", ": zz", ": 1"]) add(out(`${i} | number_of_words${m}`));
// array_to_sentence_string
for (const i of ["a", "ea", "strs", "withnil", "'x'", "nul", "zz", "n", "o", "nested", "[1]", "people", "d", "na"]) for (const c of ["", ": 'or'", ": ''", ": nul", ": zz", ": n", ": a"]) add(out(`${i} | array_to_sentence_string${c}`));
add(`{% assign two = a | slice: 0, 2 %}{{ two | array_to_sentence_string }}{% assign one = a | slice: 0, 1 %}|{{ one | array_to_sentence_string }}`);
// capitalize misc
for (const i of ["'ǆungla'", "'ßtraße'", "'éCOLE'", "'😀ABC'", "'ΑΣ'", "'İSTANBUL'", "'ﬁne'", "'  x'", "'ΌΣΟΣ'", "'i̇'"]) { add(out(`${i} | capitalize`)); add(out(`${i} | upcase`)); add(out(`${i} | downcase`)); }
// arity of append/prepend
add(out(`s | append`)); add(out(`s | prepend`)); add(out(`s | append: 'a', 'b'`)); add(out(`s | append: k: 1`)); add(out(`s | prepend: 'a', k: 1`)); add(out(`s | append: 'a', 'b' | upcase`));
// url filters
for (const i of ["url", "'a b+c'", "'é😀'", "'~!*()'''", "'[]'", "'%'", "'%41'", "'a/b?c=d&e=f#g'", "'日本'", "n", "nul", "zz", "a", "o", "e"]) { add(out(`${i} | url_encode`)); add(out(`${i} | cgi_escape`)); add(out(`${i} | uri_escape`)); }
// ---- math
for (const f of ["abs", "ceil", "floor", "round"]) for (const i of NUM_IN) add(out(`${i} | ${f}`));
for (const f of ["plus", "minus", "times", "divided_by", "modulo", "at_least", "at_most", "round"]) for (const i of NUM_IN.slice(0, 16)) for (const a of NUM_ARG) add(out(`${i} | ${f}: ${a}`));
for (const f of ["plus", "minus", "times", "divided_by", "modulo", "at_least", "at_most", "abs", "ceil", "floor"]) add(out(`5 | ${f}`)), add(out(`5 | ${f}: 2, 3`)), add(out(`5 | ${f}: 2, k: 3`));
for (const i of ["10", "10.5", "-10", "7", "0", "1e21", "nul", "'x'"]) for (const j of ["4", "0", "-4", "0.3", "nul", "'x'"]) for (const k of ["", ", true", ", false", ", 1", ", 0", ", 'true'", ", nul", ", 'x'", ", a", ", allow: 1"]) add(out(`${i} | divided_by: ${j}${k}`));
for (const i of ["1.005", "2.675", "1.45", "-1.5", "-2.5", "0.5", "1.5", "2.5", "1234.5678", "0.000001234", "123456789.987654321", "1e21", "1.7976931348623157e308", "9007199254740993", "0.1", "-0.0001", "5e-324", "99.995", "1.255", "8.345", "0.285", "10.075"]) for (const p of ["", ": 0", ": 1", ": 2", ": 3", ": 4", ": -1", ": -2", ": 10", ": 20", ": 100", ": 400", ": 1.5", ": 'x'", ": nul", ": '2'"]) add(out(`${i} | round${p}`));
for (const e of ["0.1 | plus: 0.2", "1.1 | times: 3", "3 | times: 1.1", "0.7 | plus: 0.1", "1 | divided_by: 3", "-7 | modulo: 3", "7 | modulo: -3", "-7 | modulo: -3", "7 | modulo: 0", "7.5 | modulo: 2", "0 | minus: 0", "-0 | times: 1", "2 | times: -0", "1e308 | times: 10", "5 | divided_by: 0 | plus: 1", "'5' | plus: '3'", "'a' | plus: 'b'", "true | plus: 1", "nul | plus: 1", "a | plus: 1", "'1e3' | plus: 0", "' 5 ' | plus: 0", "'0b11' | plus: 0", "'0o17' | plus: 0", "'1_000' | plus: 0", "'Infinity' | plus: 0", "'-Infinity' | plus: 0", "'.5' | plus: 0", "'5.' | plus: 0", "'+5' | plus: 0", "d | plus: 0", "2 | at_least: 1, 9, 4", "2 | at_most: 1, 9, 4"]) add(out(e));
// ---- arrays
const SZ = [...ARR_IN, "obj2", "strlen", "'😀abc'", "cjk"];
for (const f of ["size", "first", "last", "reverse", "sort", "sort_natural", "compact", "uniq", "pop", "shift", "join", "sum", "array_to_sentence_string", "json"]) for (const i of SZ) add(out(`${i} | ${f}`));
for (const i of SZ) { add(out(`${i} | reverse | join: ','`)); add(out(`${i} | sort | join: ','`)); add(out(`${i} | sort_natural | join: ','`)); add(out(`${i} | compact | join: ','`)); add(out(`${i} | uniq | join: ','`)); add(out(`${i} | pop | join: ','`)); add(out(`${i} | shift | join: ','`)); add(out(`${i} | first | json`)); add(out(`${i} | last | json`)); }
for (const i of ARR_IN) for (const a of ["''", "', '", "'-'", "nul", "zz", "n", "a", "o", "true", "'😀'"]) add(out(`${i} | join: ${a}`));
for (const i of ARR_IN.slice(0, 10)) for (const a of ["a", "ea", "zz", "nul", "n", "s", "o", "strs", "na", "emoji"]) { add(out(`${i} | concat: ${a} | join: ','`)); add(out(`${i} | push: ${a} | json`)); add(out(`${i} | unshift: ${a} | json`)); }
add(out(`a | push`)); add(out(`a | unshift | json`)); add(out(`a | concat`)); add(out(`a | concat | json`));
for (const i of ["a", "ea", "strs", "s", "emoji", "cjk", "nul", "zz", "n", "o", "'a😀b😀c'", "mixed", "t"]) for (const b of ["0", "1", "2", "-1", "-3", "5", "1.5", "'1'", "'x'", "nul", "zz", "true"]) {
  add(out(`${i} | slice: ${b} | json`));
  for (const l of ["0", "1", "2", "-1", "1.5", "'2'", "nul", "zz", "100"]) add(out(`${i} | slice: ${b}, ${l} | json`));
}
add(out(`a | slice`));
for (const p of ["'name'", "'age'", "'meta.ok'", "'tags'", "'tags.first'", "'tags.size'", "'size'", "'first'", "''", "nul", "zz", "'nosuch'", "'name.size'", "'meta'", "0", "true", "'meta[\"ok\"]'", "'[0]'", "'age foo'", "'-x'", "n"]) {
  add(out(`people | map: ${p} | json`)); add(out(`people | sort: ${p} | map: 'name' | join: ','`)); add(out(`people | sort_natural: ${p} | map: 'name' | join: ','`)); add(out(`people | sum: ${p}`));
  add(out(`people | where: ${p} | map: 'name' | join: ','`)); add(out(`people | reject: ${p} | map: 'name' | join: ','`)); add(out(`people | find: ${p} | json`)); add(out(`people | find_index: ${p}`)); add(out(`people | has: ${p}`));
  add(out(`people | group_by: ${p} | json`));
  for (const x of ["30", "'Ann'", "true", "nul", "zz", "25", "empty", "nil", "''"]) {
    add(out(`people | where: ${p}, ${x} | map: 'name' | join: ','`)); add(out(`people | reject: ${p}, ${x} | map: 'name' | join: ','`)); add(out(`people | find: ${p}, ${x} | json`)); add(out(`people | find_index: ${p}, ${x}`)); add(out(`people | has: ${p}, ${x}`));
  }
}
for (const i of ["a", "na", "strs", "ea", "nul", "zz", "s", "o", "nested", "dupsort", "d"]) { add(out(`${i} | map: 'k' | json`)); add(out(`${i} | sort: 'k' | json`)); add(out(`${i} | sum: 'k'`)); add(out(`${i} | where: 'k', 1 | json`)); add(out(`${i} | group_by: 'k' | json`)); add(out(`${i} | map: 'length' | json`)); add(out(`${i} | sum: 'length'`)); add(out(`${i} | map: 'size' | json`)); add(out(`${i} | map: 'first' | json`)); }
for (const x of ["p.age > 26", "p.age == 30", "p.name", "p.name == 'Bo'", "p.tags", "p.meta.ok", "p.meta.ok == true", "p.nosuch", "p.age > 26 and p.age < 35", "true", "false", "nil", "1", "0", "''", "'x'", "p | json", "p.age | at_least: 28", "p.age | at_least: 28 | plus: 1 | minus: 29", "p.name contains 'n'", "p.age >= 30 or p.name == 'Di'", "p", "p.age | nosuch", "p.age ==", "", "(1..p.age) | size", "q.age", "p['name'] == 'Ann'"]) {
  for (const f of ["where_exp", "reject_exp", "find_exp", "find_index_exp", "has_exp", "group_by_exp"]) add(out(`people | ${f}: 'p', ${JSON.stringify(x).replace(/"/g, "'")} | json`));
}
for (const nm of ["'p'", "'x'", "nul", "zz", "1", "''", "'p.q'"]) for (const f of ["where_exp", "find_exp", "group_by_exp", "has_exp"]) { add(out(`people | ${f}: ${nm}, 'p.age > 26' | json`)); add(out(`people | ${f}: ${nm}, 'x.age > 26' | json`)); }
for (const i of ["a", "na", "ea", "nul", "zz", "s", "o", "nested", "strs", "dups", "d", "emoji"]) for (const f of ["where_exp", "reject_exp", "find_exp", "find_index_exp", "has_exp", "group_by_exp"]) { add(out(`${i} | ${f}: 'x', 'x > 1' | json`)); add(out(`${i} | ${f}: 'x', 'x' | json`)); add(out(`${i} | ${f}: 'x', 'x == nil' | json`)); }
add(out(`people | where_exp: 'p'`)); add(out(`people | where_exp: 'p', 'p.age > 26', 'z'`)); add(out(`people | where_exp | json`));
add(out(`people | where_exp: 'p', 'p.age > 26' | where_exp: 'p', 'p.age < 30' | map: 'name' | json`));
add(`{% assign g = people | group_by: 'age' %}{% for x in g %}{{ x.name }}={{ x.items | map: 'name' | join: ',' }};{% endfor %}`);
add(`{% assign g = people | group_by_exp: 'p', 'p.age | at_least: 28' %}{% for x in g %}{{ x.name }}={{ x.items | size }};{% endfor %}`);
add(`{% assign g = o | group_by: '0' %}{{ g | json }}`); add(`{% assign g = s | group_by: 'length' %}{{ g | json }}`);
add(`{% assign g = dupsort | group_by: 'k' %}{{ g | json: 2 }}`);
add(`{% assign g = dups | group_by: 'size' %}{{ g | json }}`);
add(`{% for x in people %}{% assign r = people | where_exp: 'q', 'q.name == x.name' %}{{ r | size }}{% endfor %}`);
add(`{% assign v = 'Ann' %}{{ people | where_exp: 'p', 'p.name == v' | map: 'name' | join: ',' }}`);
add(`{{ people | where_exp: 'p', 'p.name == name' | size }}`);
add(`{% assign p = 'outer' %}{{ people | where_exp: 'p', 'p.age > 26' | size }}{{ p }}`);
// sorting
add(out(`na | sort | json`)); add(out(`na | sort_natural | json`)); add(out(`strs | sort | json`)); add(out(`strs | sort_natural | json`));
add(out(`dupsort | sort: 'k' | map: 'v' | join: ''`)); add(out(`dupsort | sort_natural: 'k' | map: 'v' | join: ''`));
add(out(`people | sort: 'age' | map: 'name' | join: ','`)); add(out(`people | sort: 'name' | map: 'name' | join: ','`)); add(out(`people | sort: 'tags' | map: 'name' | join: ','`));
add(`{% assign m = "b,a,C,B,10,9,1" | split: ',' %}{{ m | sort | join: ',' }}|{{ m | sort_natural | join: ',' }}`);
add(`{% assign m = "3,1,2,10,20,1.5,-1,abc,ABC,xyz" | split: ',' %}{{ m | sort | json }}`);
// large sort
{ const big = Array.from({ length: 100 }, (_, i) => (i * 37) % 53); add(`{% assign m = "${big.join(',')}" | split: ',' %}{{ m | sort | join: ' ' }}|{{ m | sort_natural | join: ' ' }}`);
  add(`{% assign m = "${big.map(x => 'k' + (x % 7)).join(',')}" | split: ',' %}{{ m | sort | join: ' ' }}`); }
// mixed-type sort (small n uses exact algorithm)
add(`{% assign m = "5,a,3,b,1,c,2" | split: ',' %}{{ m | sort | join: ',' }}`);
for (const lst of ["[3,'10',2,'9','a',null,true]", "[null,3,1,null,2]", "[true,false,1,0,'1','0']", "['b',1,'a',2,null,undefined]"]) add(`{% assign z = ${lst.replace(/'/g, "'")} %}`);
add(out(`dups | uniq | json`)); add(out(`nested | uniq | json`)); add(out(`people | uniq | size`)); add(out(`people | concat: people | uniq | size`));
add(`{% assign p2 = people | first %}{{ people | push: p2 | uniq | size }}`);
add(out(`withnil | compact | json`)); add(out(`nested | compact | json`));
add(out(`o | first`)); add(out(`o | last`)); add(out(`o | size`)); add(out(`o | map: 'a' | json`)); add(out(`o | sort | json`)); add(out(`o | reverse | json`)); add(out(`o | join: '-'`));
add(out(`obj2 | first`)); add(out(`obj2 | last`)); add(out(`obj2 | size`));
add(out(`strlen | size`)); add(out(`strlen.size`)); add(out(`s | first`)); add(out(`s | last`)); add(out(`'' | first | json`)); add(out(`emoji | first`)); add(out(`emoji | last`)); add(out(`cjk | last`));
add(out(`a | sample: 2 | size`)); add(out(`a | sample | size`)); add(out(`a | sample: 5 | size`)); add(out(`a | sample: 0 | size`)); add(out(`a | sample: -1 | size`)); add(out(`s | sample: 3 | size`)); add(out(`emoji | sample: 10 | size`)); add(out(`nul | sample | json`)); add(out(`ea | sample | json`)); add(out(`a | sample: '2' | size`)); add(out(`a | sample: 1.5 | size`));
add(out(`n | sample | json`));
// ---- misc
for (const i of [...STR_IN, "num", "people", "nested", "dups", "'a\\\"b\\\\c\\n\\u0001'", "big", "f", "0.1", "-0", "1e-7", "[1]"]) for (const sp of ["", ": 0", ": 2", ": 4", ": '--'", ": 'abcdefghijklmnop'", ": 20", ": -1", ": nul", ": zz", ": true", ": '2'", ": 1.5", ": ''"]) { add(out(`${i} | json${sp}`)); }
for (const i of ["o", "a", "ea", "people", "nested", "withnil", "d", "s"]) for (const sp of [": 0", ": 2", ": '\\t'"]) add(out(`${i} | inspect${sp}`)), add(out(`${i} | jsonify${sp}`));
add(`{% assign x = 'a' %}{% capture c %}{{ x }}{% endcapture %}{{ c | json }}`);
add(`{% for i in (1..2) %}{{ forloop | json }}|{{ forloop | json: 2 }}{% endfor %}`);
add(`{% for i in (1..2) %}{{ forloop | inspect }}{% endfor %}`);
add(`{{ nil | json }}|{{ empty | json }}|{{ blank | json }}|{{ nil | inspect }}`);
add(`{{ o | json: 2 }}`);
for (const i of ["s", "e", "nul", "zz", "n", "0", "fl", "t", "a", "ea", "o", "d", "nil", "empty", "blank", "'0'", "'false'"]) for (const dv of ["", ": 'dflt'", ": zz", ": nul", ": 5", ": a", ": 'dflt', allow_false: true", ": 'dflt', allow_false: false", ": 'dflt', allow_false: nil", ": 'dflt', allow_false: 0", ": 'dflt', allow_false: ''", ": 'dflt', allow_false", ": 'dflt', 'allow_false': true", ": 'dflt', other: true", ": 'dflt', true", ": 'dflt', 'x'", ": 'dflt', a"]) add(out(`${i} | default${dv}`));
add(out(`fl | default: 'x', allow_false: true | json`)); add(out(`s | default: 'x', 1`)); add(out(`fl | default: 'x', 1`)); add(out(`fl | default: 'x', a`)); add(out(`fl | default: 'x', 'ab'`)); add(out(`fl | default: 'x', s`)); add(out(`fl | default: 'x', n`)); add(out(`fl | default: 'x', nul`));
for (const i of ["s", "n", "num", "numneg", "sci", "hex", "'abc'", "''", "' 5 '", "t", "fl", "nul", "zz", "a", "o", "d", "f", "big", "'1e400'", "'-0'", "'12px'", "'0.1e1'", "'Infinity'"]) { add(out(`${i} | to_integer`)); add(out(`${i} | to_integer | plus: 1`)); add(out(`${i} | raw`)); }
// pipelines
for (const e of ["s | downcase | replace: ' ', '-' | remove: ','", "long | truncatewords: 5 | upcase | append: suffix", "people | map: 'name' | join: ', ' | prepend: '[' | append: ']'", "s | split: ', ' | first | upcase", "s | size | plus: 1", "a | size | times: 2 | divided_by: 4.0 | round: 1", "a | sum | divided_by: 3, true", "s | slice: 1, 3 | upcase", "s | append: n | append: f | append: t | append: nul | append: zz", "s | append: a | append: o | append: d", "d | append: ''", "d | prepend: 'x'", "d | date: '%Y' | append: d", "a | join: s", "o | json | escape", "html | escape | escape_once | strip_html"]) add(out(e));
const plainSrcs = plain;

// ---- dates
const INST = ["2024-03-05T12:34:56.789Z", "2024-01-01T00:00:00Z", "2024-12-31T23:59:59.999Z", "2024-03-10T06:59:59Z", "2024-03-10T07:00:00Z", "2024-03-31T00:59:59Z", "2024-03-31T01:00:00Z", "2024-11-03T05:59:59Z", "2024-11-03T06:00:00Z", "2024-10-27T00:59:59Z", "2024-10-27T01:00:00Z", "2024-07-04T04:00:00Z", "1999-12-31T23:59:59Z", "1970-01-01T00:00:00Z", "1969-07-20T20:17:40Z", "2038-01-19T03:14:08Z", "2000-02-29T12:00:00Z", "2026-11-15T00:00:00Z", "2020-02-29T23:30:00Z", "2021-01-03T05:00:00Z", "2022-01-01T05:00:00Z", "2023-01-01T00:00:00Z", "2024-04-07T15:59:59Z", "2024-04-06T16:00:00Z", "2024-10-06T15:59:59Z", "2012-06-15T12:00:00Z", "2100-12-31T23:59:59Z", "1950-06-15T12:00:00Z", "2262-04-11T23:47:16Z", "2024-09-29T13:59:59Z", "2024-09-28T14:00:00Z", "2024-04-06T14:59:59Z", "2024-04-06T15:00:00Z"];
const DIRS = ["a", "A", "b", "B", "h", "c", "C", "d", "e", "H", "I", "j", "k", "l", "L", "m", "M", "N", "p", "P", "q", "s", "S", "u", "U", "w", "W", "x", "X", "y", "Y", "z", ":z", "Z", "t", "n", "%", "D", "F", "T", "R", "r", "v", "G", "V", "g", "E", "i", "Q", "f", "o", "O", "1"];
const dates_utc = [];
const dadd = (arr, s, x) => arr.push(x ? { src: s, data: { x: { $date: x } } } : { src: s });
const ZD = "%a %A %b %B %d %e %H %I %j %k %l %L %m %M %p %P %q %s %S %u %U %w %W %y %Y %z %:z %Z|%c|%x|%X|%C";
const zonal = [];
for (const iso of INST) {
  dadd(zonal, `{{ x | date: '${ZD}' }}`, iso);
  // Before 1980 the zone names ICU gives follow the history of each zone; the table has modern names only.
  if (+iso.slice(0, 4) >= 1980) dadd(zonal, `{{ x }}`, iso);
  dadd(zonal, `{{ x | date }}`, iso);
  dadd(zonal, `{{ x | date_to_xmlschema }}|{{ x | date_to_rfc822 }}|{{ x | date_to_string }}|{{ x | date_to_long_string }}|{{ x | date_to_string: 'ordinal', 'US' }}`, iso);
  dadd(zonal, `{{ '${iso}' | date: '${ZD}' }}`);
  dadd(zonal, `{{ '${iso}' | date: '%Y-%m-%d %H:%M:%S %z' }}`);
}
for (const iso of INST.slice(0, 12)) {
  dadd(zonal, `{{ x | json }}|{{ x | append: '' }}|{{ x | date: '%s' }}`, iso);
  dadd(zonal, `{{ x | date: '%Y-%m-%d %H:%M:%S %z %Z', -300 }}|{{ x | date: '%H %z %Z', 330 }}|{{ x | date: '%H %z %Z', 'UTC' }}|{{ x | date: '%H %z', 0 }}`, iso);
}
for (const iso of INST.slice(0, 12)) for (const dir of DIRS) dadd(dates_utc, `{{ '${iso}' | date: '[%${dir}]' }}`);
for (const iso of INST.slice(0, 3)) {
  dadd(dates_utc, `{{ '${iso}' | date: '%A, %B %-e, %Y at %-l:%M %P %z' }}`);
  dadd(dates_utc, `{{ '${iso}' | date: nil }}`); dadd(dates_utc, `{{ '${iso}' | date: '' }}`);
  for (const t of ["'ordinal'", "'ordinal', 'US'", "'x', 'US'", "'ordinal', 'x'", "nul", "'ordinal', nul"]) { dadd(dates_utc, `{{ '${iso}' | date_to_string: ${t} }}|{{ '${iso}' | date_to_long_string: ${t} }}`); }
}
const FLAGS = ["-", "_", "0", "^", "#", ":", "-^", "_#", "0_", "::", "^_", "-0"];
for (const iso of ["2024-03-05T12:34:56.789Z", "2020-02-29T23:30:00Z"]) for (const dir of ["a", "A", "b", "B", "c", "C", "d", "e", "H", "I", "j", "k", "l", "L", "m", "M", "N", "p", "P", "q", "s", "S", "u", "U", "w", "W", "x", "X", "y", "Y", "z", "Z", "t", "%"]) {
  for (const fl of FLAGS) dadd(dates_utc, `{{ '${iso}' | date: '[%${fl}${dir}]' }}`);
  for (const w of ["1", "3", "5", "12", "0"]) { dadd(dates_utc, `{{ '${iso}' | date: '[%${w}${dir}]|[%-${w}${dir}]|[%_${w}${dir}]|[%0${w}${dir}]' }}`); }
  dadd(dates_utc, `{{ '${iso}' | date: '[%E${dir}|%O${dir}]' }}`);
}
for (const f of ["%", "%%", "100%", "%-", "%5", "%-5", "%^", "% d", "%\\n", "a%\\nb%d", "%E", "%Ey", "%OH", "%::z", "%:::z", "%:-z", "%10N", "%1N", "%0N", "%N", "%6N", "%12N", "%3L", "%_3S", "%^B", "%^b", "%#B", "%#p", "%#Z", "%^Z", "%^c", "%-m/%-d/%y", "%d/%m/%Y", "%e", "%-e", "%_e", "%0e", "plain", "", "%q", "%-q", "%^q", "%5q", "%%d", "%%%d", "%d%", "é%Yé😀%y"]) dadd(dates_utc, `{{ '2024-03-05T12:34:56.789Z' | date: '${f}' }}`);
const PARSE = ["2024-03-05", "2024-03-05T12:00:00", "2024-03-05T12:00:00Z", "2024-03-05T12:00:00z", "2024-03-05T12:00:00.5Z", "2024-03-05T12:00:00.123456Z", "2024-03-05T12:00Z", "2024-03-05T12Z", "2024-03-05T12:00:00+05:30", "2024-03-05T12:00:00-0800", "2024-03-05T12:00:00+0530", "2024-03-05T24:00:00Z", "2024-03-05T24:00:01Z", "2024-03-05 12:00:00", "2024-03-05 12:00:00Z", "2024-03-05 12:00:00 UTC", "2024-03-05 12:00:00 +0100", "2024-03-05 12:00:00 GMT+0100", "2024-03-05 12:00:00 EST", "2024-03-05 12:00:00 PDT", "2024-03", "2024", "2024-3-5", "2024-03-5", "2024-3-05", "2024/03/05", "2024/3/5 1:02", "03/05/2024", "3/5/2024", "3/5/24", "3/5/49", "3/5/50", "03/05/2024 12:34 PM", "03/05/2024 12:34 am", "12/31/1999 11:59:59 PM", "March 5, 2024", "March 5, 2024 12:00", "March 5, 2024 12:00:00 GMT", "Mar 5 2024", "5 Mar 2024", "5 March 2024 10:00", "Tue Mar 05 2024", "Tue, 05 Mar 2024 12:00:00 GMT", "Tue, 05 Mar 2024 12:00:00 +0000", "Tue, 05 Mar 2024 12:00:00 -0500", "Tue Mar 05 2024 12:00:00 GMT-0500 (Eastern Standard Time)", "Mar 5", "March", "5 Mar", "Sept 5 2024", "Sep 5, 2024", "sept 5 2024", "Marchxx 5 2024", "Mon 2024-03-05", "2024-03-05 Mon", "  2024-03-05  ", "2024-03-05T", "T12:00", "12:00", "12:00:00", "1:02 PM", "2024-02-30", "2024-02-31T00:00:00Z", "2024-13-01", "2024-00-10", "2024-12-32", "2023-02-29", "0000-01-01", "0001-01-01T00:00:00Z", "99-01-01", "+002024-03-05", "-000001-01-01T00:00:00Z", "+275760-09-13T00:00:00Z", "+275760-09-13T00:00:00.001Z", "-271821-04-20T00:00:00Z", "-000000-01-01T00:00:00Z", "2024-03-05T12:00:00+25:00", "2024-03-05T12:60:00Z", "2024-03-05T12:00:60Z", "2024-03-05T25:00:00Z", "20240305", "2024.03.05", "2024-03-05T12:00:00.", "2024-03-05T12:00:00,5Z", "1/2/3", "1 2 3", "1-2-3", "31-12-2024", "12-31-2024", "Dec 31 2024 23:59:59 UTC+1", "Dec 31 2024 23:59:59 UTC-8", "Dec 31 2024 23:59:59 GMT+01:30", "Dec 31 2024 23:59:59 +05:30", "Dec 31 2024 23:59:59 Z", "Dec 31 2024 23:59:59 (comment) GMT", "Dec 31 2024 (a(b)c) 23:59", "foo 2024-03-05", "2024-03-05 foo", "garbage", "", " ", "Z", "12345", "1234567890", "0", "-1", "1e3", "1.5", "0x10", "2024-03-05T12:00:00Z extra", "Tuesday March 5 2024", "Thu, 01 Jan 1970 00:00:00 GMT", "Thursday, January 1, 1970 12:00 AM", "1970-01-01T00:00:00.000+00:00", "2024-06-15T12:00:00-07:00", "2024-03-05T02:30:00", "2024-03-10T02:30:00", "2024-11-03T01:30:00", "2024-03-31T01:30:00", "2024-10-27T01:30:00", "2024-10-27T02:30:00", "99999999999", "99999999999999999999", "00012345", "12:00 2024-03-05", "AM 12:00", "12 AM", "12:00 PM 3/5/2024", "13:00 PM 3/5/2024", "0:30 AM 3/5/2024", "3/5/2024 24:00", "3/5/2024 24:01", "1:2:3:4", "1:2:3.4", "1:2:3.45678", "1:2:3.", "1::", "1:", "1::2", "-", "+", "2024-", "2024-03-", "-2024", "+2024", "(2024)", "2024)", ")2024"];
const zonal_parse = [];
for (const p of PARSE) zonal_parse.push({ src: `{{ ${JSON.stringify(p).replace(/^"|"$/g, "'").replace(/\\"/g, '"')} | date: '%Y-%m-%d %H:%M:%S.%L %z' }}` });
for (const p of ["2024-03-05T12:00:00Z", "2024-03-05"]) for (const tzo of ["0", "-300", "300", "330", "-330", "60", "-60", "720", "'UTC'", "'GMT'", "'Z'", "'utc'", "nul", "zz", "'x'", "true", "''", "1.5", "'60'", "-1000000", "1e9"]) dadd(zonal_parse, `{{ '${p}' | date: '%Y-%m-%d %H:%M:%S %z|%:z|%Z', ${tzo} }}`);
for (const v of ["0", "1", "86400", "1709642096", "1709642096.789", "-1", "-86400", "1e10", "1e11", "1e12", "253402300800", "8640000000000", "8640000000001", "-8640000000000", "0.001", "nul", "zz", "true", "false", "'0'", "'1709642096'", "'-1'", "'1.5'", "'1e3'", "''", "' '", "'00'", "n", "f", "a", "o", "s", "ea"]) {
  dadd(zonal_parse, `{{ ${v} | date: '%Y-%m-%d %H:%M:%S.%L' }}`); dadd(zonal_parse, `[{{ ${v} | date_to_string }}]`);
}
dadd(dates_utc, `{{ 'now' | date: '%Y' | size }}|{{ 'today' | date: '%j' | size }}|{{ 'NOW' | date: '%Y' }}`);
dadd(dates_utc, `{{ a | date: '%Y' }}`); dadd(dates_utc, `{{ '2024' | date: '%Y' }}`);
for (const e of ["d", "d | append: ''", "d | prepend: ''", "d | json", "d | size", "d | upcase", "d | split: ' ' | first", "d | slice: 0, 3", "d | truncate: 20", "d | date: '%s'", "d | default: 1", "d | join: ','", "d | first", "d | strip", "d | escape", "d | url_encode", "d | sha256 | size", "d | to_integer", "d | plus: 0", "d | minus: 1", "d | inspect", "d | jsonify: 2", "d | array_to_sentence_string", "d | compact | size", "d | uniq | size", "d | sort | size", "d | sum", "d | map: 'x' | size", "d | number_of_words", "d | base64_encode", "d | truncatewords: 3", "d | replace: 'GMT', '-'", "d | remove: ' '", "d | capitalize", "d | date_to_xmlschema", "d | date_to_rfc822", "d | date_to_string", "d | date_to_long_string", "d | date: '%Y-%m-%d'", "d | date: '%c'", "d | date"]) dadd(dates_utc, out(e));
dadd(dates_utc, `{% assign x = d %}{{ x }}|{{ x | date: '%Y' }}`); dadd(dates_utc, `{% capture c %}{{ d }}{% endcapture %}{{ c }}`); dadd(dates_utc, `{% if d == d %}y{% endif %}{% if d > '1' %}z{% endif %}`);
dadd(dates_utc, `{{ d | date: "%s" | plus: 0 }}`);
for (const iso of INST) dadd(dates_utc, `{{ '${iso}' | date: '${ZD}' }}`);
module.exports = { data: D, plain: plainSrcs, dates_utc, zonal, zonal_parse };
