// Regenerates tests/probes/markdown_it.json: `markdownIt().render(s)` and
// `markdownIt().renderInline(s)` (markdown-it with no options, so HTML is
// off) for the inputs below. Run from crates/brrrty-markdown with Node 20 or later
// (the file was made with v26.2.0):
//
//   MARKDOWN_IT=/path/to/node_modules/markdown-it node tools/gen-markdown-probes.cjs > tests/probes/markdown_it.json
//
// MARKDOWN_IT is the markdown-it package directory (version 15.0.3). MARKDOWN_IT_11TY is the
// 14.3.3 copy under @11ty/eleventy/node_modules.
const markdownIt = require(process.env.MARKDOWN_IT || (() => { throw new Error('set MARKDOWN_IT to the markdown-it package directory'); })());
const md = markdownIt();
// 11ty 3.1.8's own instance: its bundled markdown-it 14.3.3, html on, `code` rule off.
const eleventy = require(process.env.MARKDOWN_IT_11TY || (() => { throw new Error('set MARKDOWN_IT_11TY to 11ty\'s bundled markdown-it directory (14.3.3)'); })())({ html: true }).disable('code');

// Found by tools/gen-markdown-fuzz.cjs and fixed.
const FUZZ_BLOCK = ['>', '> ', '> >', '>  ', '* > ', '> [x]: /u', '> a\n\n>', '\n\n>', 'a\n|--', '[* \n|--', "' \r:-:", '## h\n|--', '2) x\n---:', 'a | b\n---|---', '>|\n|--', '```js\r\n\n  ', '```js\r><div>+ + \n ', '```\nx\n  ', '```\n  '];
const FUZZ_INLINE = ['`    `', '`   `', '`  `', '` `', '`   a   `', '` a `', '`\n`', '`\n\n`', '`    `__# ', '\u00a0', '\u00a0x', 'x\u00a0', '\u00a0x\u00a0', ' \u00a0 ', '*\u00a0a*', '\u00a0*a*\u00a0'];

const INLINE = [
  '', 'plain', '*em*', '**strong**', '***both***', '_em_ and __strong__', '`code`', '``a ` b``', '` a `', '`  `',
  '<b>bold</b>', '<script>alert(1)</script>', 'a < b > c & d', '&amp; &lt; &copy; &#35; &#x41; &bogus; &#0;', '&nbsp;x',
  '[link](http://example.com)', '[link](http://example.com "title")', '[link](<a b> "t")', '[ref][r]\n\n[r]: /url', '[x](javascript:alert(1))',
  '![img](a.png)', '![alt *em*](a.png "t")', '<http://example.com>', '<me@example.com>', 'http://bare.example.com',
  'line one\nline two', 'line one  \nline two', 'line one\\\nline two', '  leading', 'trailing  ', 'a\n   b\n  c', '\n', 'a\n',
  '\\*not em\\*', '\\\\ \\` \\a', '~~strike~~', '~single~', '* not a list', '# not a heading', '> not a quote', '- not a list', '1. not a list', '    not code',
  '"quotes" and \'single\'', '(c) (tm) +- ... --', '"smart"', 'a  b   c', 'tab\there', 'a\r\nb', 'a\rb', 'nul\0char', 'éñ 中文 😀',
  '*a **b** c*', '**a *b* c**', '*a*b*', 'a*b*c', 'snake_case_word', '_a_b_', '[a *b*](c)', '[a](b)[c](d)', '[a]()', '[a](<>)',
  '<span class="x">hi</span>', '<!-- comment -->', '<a href="x">y</a>', '&lt;b&gt;', 'x<y>z',
  '[link](http://example.com/a b)', '[link](/a%20b)', '[l](http://ex.com/?a=1&b=2)', '[l](http://ex.com/é)', '<http://ex.com/a&b>',
  '||', '| a | b |\n|---|---|\n| 1 | 2 |', '```js\ncode\n```', '---', '***', '[ ] [x]', '[^1]', 'foo[bar', '*', '**', '`', '\\', '&', '#', '1) x',
  '  \nfoo', 'foo  \n  ', ' \t a \t ', '\n\n', '  *a*  ', '  _a_  ', 'a\n\n\nb', '\n\na', '*a*\n', ' \n', '\u00a0x\u00a0', '  `x`  ', '  **x**', '__x__  ',
  'Josh\'s "blog" post -- really... (c)', 'a *b* `c` [d](e) ![f](g) <h> &i;',
];

const BLOCK = [
  '', '\n', 'plain', 'a\n\nb', '# H1', '## H2 ##', 'Setext\n===', 'Setext 2\n---', '###### H6', '####### not',
  '> quote\n> more', '> a\n\n> b', '- a\n- b', '* a\n* b\n\n  c', '1. one\n2. two', '3. three\n4. four', '- a\n  - b\n    - c',
  '- [ ] task', '    indented code', 'para\n\n    code\n    more', '\tcode with tab', '```\nfence\n```', '```js\nlet a = 1;\n```', '~~~\ntilde\n~~~', '````\n```\n````',
  '```\nunclosed', '```\nunclosed\n', '```', '```\n', '> ```\n> x', '- ```\n  x', '```\na\n```', '```\na\n``` ', '````\na\n```', '~~~\na\n```', '```\n\n', '---', '***', '___', '- - -', '<div>html</div>', '<div>\nhtml block\n</div>', '<script>x</script>', '<!-- c -->', '<br>', 'text <b>x</b> text',
  '| a | b |\n|---|---|\n| 1 | 2 |', 'a | b\n--|--\n1 | 2', '| a |\n|:-:|\n| `x` |', '~~strike~~ and *em*',
  '[ref]\n\n[ref]: http://example.com "Title"', '[a](b)', '![i](a.png)', 'http://example.com', '<http://example.com>',
  'hard  \nbreak', 'soft\nbreak', '&amp; &copy; &#35;', 'a & b < c > d', '"quotes" -- (c) ...',
  'Line 1\r\nLine 2\r\n\r\nPara 2', 'é 中文 😀', '1. a\n\n   b\n2. c', '- a\n\n- b', '* a\n- b\n+ c', '10. ten', '1. a\n1. b\n1. c', '2. a\n3. b',
  '> - a\n> - b', '> # h\n> text', '- ```\n  code\n  ```', '    \n', '  \n\n  text', '\n\n\ntext\n\n\n', 'a\n    b', 'a\n\n\n\nb',
  '# Title\n\nSome *emphasis*, **strong**, and `code`.\n\n- item one\n- item two\n\n> A quote.\n',
  '```html\n<div class="x">&amp;</div>\n```', '`<b>` and <b>', '<img src="x.png">', '<a href="x">link</a> text',
  'Term\n: Definition', '* * *', '+ plus', 'a\\\nb', '\\# not heading', '1\\. not list', '[link]( <a b> )', '&#0; &#xD800; &#1234567;',
];

// Found by the fuzz runs of Plan 5 and fixed: emphasis next to a symbol, code spans after an
// unclosed run of backticks, `\ ` before a newline, inline HTML comments, links with a host
// outside ASCII or escapes in an autolink, blocks at the end of the text, runs of `~`.
const FIXED = ['_a_😀', '😀*![i](u)10.~~😀__*', '__😀_!---__', '[a`b`c`', '[a`http://x.y`(C)`', 'a\\ \nb', 'a\\  \nb',
  'a <!--> b', 'a <!---> b', 'a <!-- x -- y --> b', '<!-- *x* b', '[x](http://é.com/é)', '<http://xn--p1ai/%C3%A9%2F>', '[x](HTTP://~:8080)',
  '<div>', '> <div>', '```js\ncode', '`` a ` b ``', '` `` `',
  'a ~~~~---~~)', '~~a~~~', '~~~a~~', '~~a~~~~b~~', '~~a ~~b~~~~', 'x ~~a~~~~~ b', '~~a ~~~b~~~ c~~', '~~a~~~~~~', '~~~~~a~~~~~', '~~a~~b~~c~~d~~~'];
// markdownIt({ linkify: true, typographer: true }) with md.linkify.set({ fuzzyLink }), as
// sites set up their own instance.
const LINKIFY = [
  'See http://example.com, or https://example.com/a?b=c&d=e#f.', 'Mail me@example.com or mailto:me@example.com', 'Founded Example.com in 1995.',
  'Visit www.example.com/path (and github.com/x).', 'Not a link: file.txt, v1.2.3 or e.g. this.', '[label](http://u.v) and http://w.x', '<http://a.b> then http://c.d',
  '`http://code.x` and ![http://alt.x](u)', 'пример.рф and é.com', 'http://xn--p1ai', '<a href="x">http://inside.a</a> http://out.side', 'a\\_b.com', '\\_b.com',
  '"Hello," she said. \'Hi\' -- "it\'s" 1990s... (c) (tm) (r) +- ??? !!!! ,,', '"*em*" \'`code`\' "![i](u)"', 'He said "a \'nested\' quote".',
  '"The Show" (2009--present) -- the "Mr. Fixit" era... Pat O\'Brien\'s 5\'11".', '6\'2" tall and "quoted"', '"a\nb"', 'x---y and a - b -- c --- d',
  '**Ada Lovelace** (born 1815) wrote [Note G](https://en.wikipedia.org/wiki/Note_G) for Babbage! See Example.org... "the notes".',
];
// Fix round 1 (from the review): IPv6 hosts (14.3.3 encodes the brackets), emphasis openers
// after another marker closes, image alt text per version, raw HTML blocks across a blank line
// in a list item, tight list items with blocks, image labels parsed as their own text.
const ROUND1 = ['[dev server](http://[::1]:8080/)', '<http://[::1]/>', '[x][r]\n\n[r]: //[2001:db8::1]', '<mailto:a@[IPv6:::1]>', '[x](http://é.com) [y](http://xn--p1ai/) <http://пример.рф>',
  '*Set `DEBUG` in settings_* then _restart_.', '_Price with [code](u) SAVE10*_ (*terms apply*)', '*b~~*~~x~~', 'I *really~~*~~like~~', '_a`c`d*_*b*', '*a `c` d_* _b_', '*a~~*~~c~~', '**a _b** c_', '~~a *b~~ c*',
  '![Josh &amp; Ana](a.jpg)', "![Don\\'t](u)", '![a\\\nb](u)', '![a  \nb](u)', '![a <http://x.y> b](u)', '![Output of `ls -la`](a.png)', '![a <b>x</b> c](u)', '![![n &copy;](v) &amp;](u)', '![*a*b](u)',
  '- <pre>\n  x\n\n  y\n  </pre>\n', '1. <script>\n   a\n\n   b\n   </script>\n', '- <!-- a\n\n  b -->\n', '<!x<?\n   ', '```\rcode\r```', '```**\r\n\r\n> ```',
  '- ```\n  x\n  ```\n  after', '- ## Step 1\n  Do this.', '- a\n  > q', '- a\n  ***', '1. text\n   ---\n   more', '- a\n  <div>', '- a\n      code',
  '![``a`b``![``a`b```', '> quote\n- item', '| a |\n| --- |\n| b |', '| Name |\n---\n| a |', '``````<div>>   a\\|<div>text\n---', '- _[a](b)- |\r---- ',
  '[note]: see below', '- a\n  ```\n  b', '```\n\n  x\n     ```',
  // Fix round 2: what the refusal checks must leave alone (code, HTML, titles, list text).
  '```\n[A-Za-z]:\\\\Users\n```', '```\n[a-z]:\\w+\n```', '    [A-Z]:\\d+', '```md\n[docs]: C:\\path\\\n```', '```diff\n>\tline\n```', '```\n>\tquoted\n```',
  '[x]: /u "a \\"b\\" c"\n\n[x]', '12 [a]: b\\c', '- [a]: b\\c', '> a\n> \n| b | c |\n|---|---|', '- \r\n- _~~|\t\n---', '>```x\n>   '];
const SPACE_CODE = ['`   `', '`    `', '` \n `', 'a `   ` b', '`  `', '` a `'];
const options = (o, fuzzyLink) => { const m = markdownIt(o); m.linkify.set({ fuzzyLink }); return m; };
const fuzzy = options({ linkify: true, typographer: true }, true);
const strict = options({ linkify: true, typographer: true }, false);
const html = options({ html: true, linkify: true, typographer: true }, true);
const run = (fn, list) => [...list, ...(list === BLOCK ? FUZZ_BLOCK : FUZZ_INLINE)].map((s) => ({ s, out: fn(s) }));
process.stdout.write(JSON.stringify({
  markdown_it: '15.0.3',
  render: run((s) => md.render(s), BLOCK),
  render_eleventy: [...FUZZ_BLOCK, ...SPACE_CODE, ...FIXED, 'a', '# h', '> q', '<div>x</div>', '    indented'].map((s) => ({ s, out: eleventy.render(s) })),
  render_fixed: FIXED.map((s) => ({ s, out: md.render(s) })),
  render_round1: ROUND1.map((s) => ({ s, out: md.render(s), inline: md.renderInline(s), eleventy: eleventy.render(s), html: html.render(s) })),
  render_linkify_typographer: LINKIFY.map((s) => ({ s, out: fuzzy.render(s), strict: strict.render(s), html: html.render(s) })),
  render_inline: run((s) => md.renderInline(s), INLINE),
}, null, 0).replace(/\},\{/g, '},\n{') + '\n');
