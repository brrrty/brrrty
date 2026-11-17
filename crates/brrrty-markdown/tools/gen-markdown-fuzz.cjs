// Random markdown-it differential corpus: N inputs built from markdown fragments, run through
// `markdownIt().render` and `.renderInline`. Deterministic for a seed.
//
//   [MARKDOWN_MODE=eleventy] MARKDOWN_IT=/path/to/node_modules/markdown-it node tools/gen-markdown-fuzz.cjs [seed] [count] > fuzz.json
//
// MARKDOWN_OPTIONS='{"linkify":true,"typographer":true,"fuzzyLink":true,"html":false}' renders with
// `markdownIt({ html, linkify, typographer })` and `md.linkify.set({ fuzzyLink })` instead (no
// `renderInline`), from fragments that add links, emails, quotes, dashes and the like.
//
// Replay it with `MARKDOWN_FUZZ=fuzz.json cargo test -p brrrty-markdown --test fuzz -- --ignored`
// (with the same MARKDOWN_MODE or MARKDOWN_OPTIONS).
// The differences it found are pinned in tests/probes/markdown_it.json.
const markdownIt = require(process.env.MARKDOWN_IT || (() => { throw new Error('set MARKDOWN_IT to the markdown-it package directory'); })());
const options = process.env.MARKDOWN_OPTIONS && JSON.parse(process.env.MARKDOWN_OPTIONS);
// MARKDOWN_MODE=eleventy: markdownIt({ html: true }).disable('code'), as 11ty 3.1.8 builds it (with its bundled 14.3.3).
let md;
if (options) {
  md = markdownIt({ html: !!options.html, linkify: !!options.linkify, typographer: !!options.typographer });
  md.linkify.set({ fuzzyLink: !!options.fuzzyLink });
} else {
  md = process.env.MARKDOWN_MODE === 'eleventy' ? markdownIt({ html: true }).disable('code') : markdownIt();
}
// MARKDOWN_TOKENS=1 adds `tokens`: each inline token's children, as renderer rules see them.
// MARKDOWN_RULES=1 adds `ruled`: the render with the generic renderer rules of
// `markdown-rules.cjs` installed on a second instance set up the same way.
const dumpTokens = !!process.env.MARKDOWN_TOKENS;
let ruledMd = null;
if (process.env.MARKDOWN_RULES) {
  const install = require('./markdown-rules.cjs');
  if (options) {
    ruledMd = markdownIt({ html: !!options.html, linkify: !!options.linkify, typographer: !!options.typographer });
    ruledMd.linkify.set({ fuzzyLink: !!options.fuzzyLink });
  } else {
    ruledMd = process.env.MARKDOWN_MODE === 'eleventy' ? markdownIt({ html: true }).disable('code') : markdownIt();
  }
  install(ruledMd);
}
const tokenDump = (s) => md.parse(s, {}).filter((t) => t.type === 'inline').map((t) => t.children.map((c) => ({
  type: c.type, tag: c.tag, attrs: c.attrs, nesting: c.nesting, level: c.level, content: c.content, markup: c.markup, info: c.info, block: c.block, hidden: c.hidden,
})));
let seed = Number(process.argv[2] || 1) >>> 0;
const N = Number(process.argv[3] || 6000);
const rnd = () => { seed = (seed + 0x6D2B79F5) >>> 0; let t = seed; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
const pick = (a) => a[Math.floor(rnd() * a.length)];
const FRAGS = ['a', 'b c', 'word', '*', '**', '_', '__', '`', '``', ' ', '  ', '\t', '\n', '\n\n', '> ', '>', '- ', '-', '* ', '+ ', '1. ', '2) ', '# ', '## ', '#', '---', '***', '___', '===', '|', '| a | b |', '|--', '|---|---|', ':-:', '```', '~~~', '```js', '    ', '<b>', '</b>', '<div>', '<!--', '-->', '[', ']', '(', ')', '[a](b)', '![i](u)', '<http://x.y>', '&amp;', '&', '<', '>', '\\', '\\*', '~~', '~', '"', "'", ' ', 'é', '中', '😀', '[x]: /u', '[x]', '\r\n', '\r', '1.', '10.', '0.', '    code', 'http://a.b', ':', '!', '#hash'];
// Around linkify and the typographer.
const LINKS = ['http://', 'https://', 'HTTP://', 'ftp://', 'mailto:', '//', 'www.', 'example.com', 'github.com/x', 'Example.com', 'a.b', 'x.io', 'пример.рф', 'é.com', 'xn--p1ai', '127.0.0.1', ':8080', '/path', '?q=1&b=2', '#frag', '(x)', '[y]', 'foo@bar.com', 'me@x.co', '@', '.', ',', ';', '!', '?', '*', '_', '%20', '<a href="x">', '</a>', '<http://x.y/z>', '[l](http://u.v)', '[http://in.label](u)', '![http://alt.text](u)', '`http://code.x`', '\\_', '&#58;', '&amp;', '"', '"', "'", "'", '“', "'s", '"q"', "'q'", '(c)', '(C)', '(tm)', '(r)', '+-', '..', '...', '....', '?..', '!!!!', '????', ',,', '--', '---', '----', ' -- ', ' ', '😀', '中'];
// MARKDOWN_FAMILY=<name> builds inputs from one of these instead, around the constructs the
// review of Plan 5 found differences in (emphasis, links, alt text, HTML blocks, tight lists,
// tabs, blockquotes, CRLF, emoji, IDN hosts, and the shapes Brrrty refuses).
const FAMILIES={
 emph:['*','_','**','__','***','~~','~','~~~','~~~~','`c`','`` ` ``','`','a','b c','settings_','DEBUG','x_y','[l](u)','[*a*](u)','![i](u)',' ','  ','\n','.','!','(',')','😀','中','-',',','"','\'','\\*','\\_','&amp;','<b>','*a*','_a_','**b**','~~s~~'],
 links:['[x](',')','<','>','http://','https://','HTTP://','mailto:','//','[::1]','[2001:db8::1]',':8080','/','é.com','xn--p1ai','пример.рф','a@','[IPv6:::1]','example.com','?q=[1]','#x','%5B','%zz',' ','\n','[r]: ','[r]','"t"','foo','ß.de','İ.com','ｅｘ.com'],
 alt:['![','](u)','](u "t")','a','&amp;','&copy;','&#35;','\\\'','\\*','\\q','`ls -la`','``a`b``','<b>x</b>','<http://x.y>','<a@b.co>','\\\n','  \n','\n','*e*','**s**','~~d~~','[l](v)','![n](w)',' ','http://z.io','"q"','(c)','😀'],
 html:['- ','1. ','* ','  ','   ','> ','<pre>','</pre>','<script>','</script>','<style>','</style>','<textarea>','</textarea>','<!--','-->','<?','?>','<!X','<!x','>','<![CDATA[',']]>','<div>','</div>','<PRE>','x','y','\n','\n\n','\n  ','\n   ','\t','b'],
 tight:['- ','1. ','  ','   ','```\n','```','~~~\n','## h','# h','> q','***','---','text','"q"','http://a.b','a.com','(c)','--','...','\n','\n  ','\n   ','<div>','| a |','|---|','*e*'],
 tabs:['\t','>','> ','- ','* ','1. ','  ','\n','x','`c`','<pre>','```','a b','\t\t','>>','<!--','-->'],
 quote:['> ','>','\n','  ','    ','`a','b`','<b','>','a','- ','1. ','---','| a |','|---|','*','_','[x]: /u','\n\n','<!x','?>'],
 mix:['*','_','~~','`','[a](b)','![i](u)','<http://x.y>','http://a.b','a.com','"','\'','--','...','(c)','> ','- ','1. ','```','<div>','<!--','\n','\n\n','  ','\t','|','|---|','&amp;','\\','x','é','😀','# ','[x]: /u','[x]','<b>','</b>'],
 crlf:['\r\n','\r','\n','*a*','**','_','~~','`c`','> ','- ','1. ','  ','```','<div>','[a](b)','text','a\\','\t','---','===','|','|---|'],
 emoji:['😀','👍🏽','👨‍👩‍👧','🇺🇸','1️⃣','❤️','✨','*','_','**','__','~~','a','.','!',' ','\u200d','\ufe0f','中'],
 idn:['[x](http://',')','<http://','>','é','ß','ς','Σ','İ','ı','ﬁ','K','.com','.рф','xn--','xn--p1ai','中文','ｅ','。','．','@','mailto:','a','-','..','😀','http://','/p',' '],
 shapes:['| Name |','---','| a |','|---|','[docs]: ','C:\\path\\','[docs]','> quote','> a','-','- ','1.','+','| b | c |','|---|---|','\n','\n\n',' ','text','> ','x'],
};
const family = process.env.MARKDOWN_FAMILY && FAMILIES[process.env.MARKDOWN_FAMILY];
if (process.env.MARKDOWN_FAMILY && !family) throw new Error(`no family ${process.env.MARKDOWN_FAMILY}: ${Object.keys(FAMILIES)}`);
const ALL = family || (options ? FRAGS.concat(LINKS, LINKS) : FRAGS);
const cases = [];
for (let i = 0; i < N; i++) {
  const n = 1 + Math.floor(rnd() * (family ? 14 : options ? 12 : 9));
  let s = '';
  for (let j = 0; j < n; j++) s += pick(ALL);
  const c = options ? { s, render: md.render(s) } : { s, render: md.render(s), render_inline: md.renderInline(s) };
  if (dumpTokens) c.tokens = tokenDump(s);
  if (ruledMd) c.ruled = ruledMd.render(s);
  cases.push(c);
}
process.stdout.write('[' + cases.map((c) => JSON.stringify(c)).join(',\n') + ']\n');
