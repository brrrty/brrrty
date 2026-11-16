// Regenerates tests/probes/parse.json: each template in
// parse-probe-sources.cjs run through Nunjucks 3.2.4's parser, configured
// as 11ty 3.1.8 configures it (`new Environment(loader, { dev: true })`).
//
//   NUNJUCKS=/path/to/node_modules/nunjucks \
//     node tools/gen-parse-probes.cjs > tests/probes/parse.json
//
// Run it from crates/brrrty-nunjucks with Node 20 or later (the file was
// made with v26.2.0, whose V8 words the TypeError messages).
//
// A template that parses gets `ast`, the tree in the compact form that
// tests/probes.rs prints Brrrty's tree in. One that fails gets `msg`, the
// message Nunjucks puts after its position line (a JavaScript error keeps
// its name, as `TypeError: ...`), and `line` and `col` when Nunjucks
// gives them.
//
// The tree form: `Type@line:col(field, field, ...)` with 0-based
// positions as Nunjucks stores them, `[...]` for node lists, `~` for
// null or undefined, JSON for strings. Text (an Output holding only
// TemplateData) prints as `Text@line:col("...")`. Argument lists print as
// `Args(...)` and keyword arguments as `KW(...)`, without positions, and
// cases as `Case(...)`, as Nunjucks gives them none. A regular expression
// prints its flags only, as `Regex("gi")`.

const nunjucks = require(process.env.NUNJUCKS || (() => { throw new Error('set NUNJUCKS to the nunjucks package directory'); })());
const sources = require('./parse-probe-sources.cjs');

const env = new nunjucks.Environment(null, { dev: true });

const pos = (n) => `@${n.lineno === undefined ? '?' : n.lineno}:${n.colno === undefined ? '?' : n.colno}`;
const list = (xs) => '[' + xs.map(dump).join(' ') + ']';
const flag = (v) => (v === undefined || v === null ? '~' : String(v));

function args(n) {
  return 'Args(' + n.children.map((c) =>
    c.typename === 'KeywordArgs' ? 'KW(' + c.children.map(dump).join(' ') + ')' : dump(c)).join(' ') + ')';
}

function value(v) {
  if (v === null) return 'null';
  if (v instanceof RegExp) return `Regex(${JSON.stringify(v.flags)})`;
  if (typeof v === 'string') return JSON.stringify(v);
  return String(v);
}

function dump(n) {
  if (n === null || n === undefined) return '~';
  if (Array.isArray(n)) return list(n);
  const t = n.typename;
  const p = t + pos(n);
  switch (t) {
    case 'Root':
    case 'NodeList':
      return list(n.children);
    case 'Output': {
      const c = n.children[0];
      if (n.children.length === 1 && c.typename === 'TemplateData') return `Text${pos(n)}(${JSON.stringify(c.value)})`;
      return `${p}(${n.children.map(dump).join(' ')})`;
    }
    case 'If':
    case 'IfAsync':
    case 'InlineIf':
      return `${p}(${dump(n.cond)}, ${dump(n.body)}, ${dump(n.else_)})`;
    case 'For':
    case 'AsyncEach':
    case 'AsyncAll':
      return `${p}(${dump(n.arr)}, ${dump(n.name)}, ${dump(n.body)}, ${dump(n.else_)})`;
    case 'Set':
      return `${p}(${list(n.targets)}, ${dump(n.value)}, ${dump(n.body)})`;
    case 'Capture':
      return `${p}(${dump(n.body)})`;
    case 'Block':
      return `${p}(${dump(n.name)}, ${dump(n.body)})`;
    case 'Extends':
      return `${p}(${dump(n.template)})`;
    case 'Include':
      return `${p}(${dump(n.template)}, ${!!n.ignoreMissing})`;
    case 'Macro':
    case 'Caller':
      return `${p}(${dump(n.name)}, ${args(n.args)}, ${dump(n.body)})`;
    case 'Import':
      return `${p}(${dump(n.template)}, ${dump(n.target)}, ${flag(n.withContext)})`;
    case 'FromImport':
      return `${p}(${dump(n.template)}, ${dump(n.names)}, ${flag(n.withContext)})`;
    case 'Switch':
      return `${p}(${dump(n.expr)}, ${list(n.cases)}, ${dump(n.default)})`;
    case 'Case':
      return `Case(${dump(n.cond)}, ${dump(n.body)})`;
    case 'Literal':
      return `${p}(${value(n.value)})`;
    case 'Symbol':
      return `${p}(${JSON.stringify(n.value)})`;
    case 'Group':
    case 'Array':
    case 'Dict':
      return `${p}(${n.children.map(dump).join(' ')})`;
    case 'Pair':
      return `${p}(${dump(n.key)}, ${dump(n.value)})`;
    case 'LookupVal':
      return `${p}(${dump(n.target)}, ${dump(n.val)})`;
    case 'FunCall':
    case 'Filter':
      return `${p}(${dump(n.name)}, ${args(n.args)})`;
    case 'Compare':
      return `${p}(${dump(n.expr)}, ${list(n.ops)})`;
    case 'CompareOperand':
      return `${p}(${dump(n.expr)}, ${JSON.stringify(n.type)})`;
    case 'Not':
    case 'Neg':
    case 'Pos':
      return `${p}(${dump(n.target)})`;
    case 'In': case 'Is': case 'Or': case 'And': case 'Add': case 'Concat': case 'Sub':
    case 'Mul': case 'Div': case 'FloorDiv': case 'Mod': case 'Pow':
      return `${p}(${dump(n.left)}, ${dump(n.right)})`;
    default:
      throw new Error('no dump for ' + t);
  }
}

// What `Environment.getTemplate` reports: a TemplateError keeps its
// message; any other error is wrapped as `Name: message`.
function failure(e) {
  const t = e.Update ? e : new nunjucks.lib.TemplateError(e);
  const out = { msg: t.message.toWellFormed() };
  if (t.lineno !== undefined) out.line = t.lineno;
  if (t.colno !== undefined) out.col = t.colno;
  return out;
}

function probe(src) {
  try {
    return { src, ok: true, ast: dump(nunjucks.parser.parse(src, env.extensionsList, env.opts)) };
  } catch (e) {
    return { src, ok: false, ...failure(e) };
  }
}

module.exports = { probe };

if (require.main === module) {
  process.stdout.write(JSON.stringify(sources.map(probe), null, 1) + '\n');
}
