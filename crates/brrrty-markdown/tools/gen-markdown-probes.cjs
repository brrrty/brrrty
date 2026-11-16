// Regenerates tests/probes/markdown_it.json: `markdownIt().render(s)` and
// `markdownIt().renderInline(s)` (markdown-it with no options, so HTML is
// off) for the inputs below. Run from crates/brrrty-markdown with Node 20 or later
// (the file was made with v26.2.0):
//
//   MARKDOWN_IT=/path/to/node_modules/markdown-it node tools/gen-markdown-probes.cjs > tests/probes/markdown_it.json
//
// MARKDOWN_IT is the markdown-it package directory (version 15.0.3).
const markdownIt = require(process.env.MARKDOWN_IT || (() => { throw new Error('set MARKDOWN_IT to the markdown-it package directory'); })());
const md = markdownIt();

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

const run = (fn, list) => [...list, ...(list === BLOCK ? FUZZ_BLOCK : FUZZ_INLINE)].map((s) => ({ s, out: fn(s) }));
process.stdout.write(JSON.stringify({
  markdown_it: '15.0.3',
  render: run((s) => md.render(s), BLOCK),
  render_inline: run((s) => md.renderInline(s), INLINE),
}, null, 0).replace(/\},\{/g, '},\n{') + '\n');
