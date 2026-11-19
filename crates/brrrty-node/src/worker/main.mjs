// Brrrty's JavaScript data worker: the protocol, the loader and the reads
// it records. Runs as `node --input-type=module --eval <encode.mjs + this>`
// with the project folder as its working directory.
//
// Requests are JSON lines on stdin, one at a time. Replies go as frames,
// "\x1eBRRRTY <byte length>\n<json>", to their own descriptor, which Brrrty
// names in BRRRTY_FRAMES_FD: a pipe of their own on Unix, where stdout joins stderr so a
// child process writing to fd 1 is only noise. Where the frames share
// stdout, nothing else may reach it, so before any site code loads, the
// worker keeps its own writer for the frames and points
// `process.stdout.write` (and so `console.log`) at stderr.
//
// The import logic is ported from Eleventy 3.1.8 (src/Util/Require.js and
// src/Data/TemplateData.js), MIT licensed, copyright Zach Leatherman and
// contributors; see NOTICE.

import fs from "node:fs";
import path from "node:path";
import util from "node:util";
import nodeModule, { createRequire } from "node:module";
import { spawnSync } from "node:child_process";
import { Worker as Thread } from "node:worker_threads";
import vm from "node:vm";
import inspector from "node:inspector";
import { AsyncLocalStorage } from "node:async_hooks";
import nodeCrypto from "node:crypto";
import { fileURLToPath, pathToFileURL } from "node:url";

// --- Frames, written before anything else can touch stdout. ---

const writeFd = fs.writeSync;
const framesFd = closeOnExec(Number(process.env.BRRRTY_FRAMES_FD) || 1);
delete process.env.BRRRTY_FRAMES_FD;
// Brrrty's answers to the worker's own questions (a filter a JavaScript
// template calls on `this`) come on a pipe of their own, read blocking.
const answersFd = process.env.BRRRTY_ANSWERS_FD ? closeOnExec(Number(process.env.BRRRTY_ANSWERS_FD), fs.constants.O_RDONLY) : null;
delete process.env.BRRRTY_ANSWERS_FD;
const sleepCell = new Int32Array(new SharedArrayBuffer(4));

// The replies' pipe comes in without close-on-exec (it had to survive
// Node's own exec), so every process Node starts would inherit it, and one
// left running would hold it open. Open it again, as Node opens every file,
// with close-on-exec, and close the inherited descriptor.
function closeOnExec(fd, flags = fs.constants.O_WRONLY) {
  if (fd <= 2) return fd;
  for (const dir of ["/proc/self/fd", "/dev/fd"]) {
    let again;
    try {
      again = fs.openSync(`${dir}/${fd}`, flags);
    } catch {
      continue;
    }
    fs.closeSync(fd);
    return again;
  }
  return fd;
}

// Replies wait here while more requests of the same read are still to
// run, and go out in one write: Brrrty sends the requests of a page
// together and reads their replies together.
let held = [];
let heldSince = 0;

function out(text, hold = false) {
  // Not for long: Brrrty takes a slow reply for a stuck worker.
  if (hold && (held.length === 0 || Date.now() - heldSince < 50)) {
    if (held.length === 0) heldSince = Date.now();
    held.push(text);
    return;
  }
  if (held.length) {
    text = held.join("") + text;
    held = [];
  }
  const bytes = Buffer.from(text, "utf8");
  let done = 0;
  while (done < bytes.length) {
    try {
      done += writeFd(framesFd, bytes, done, bytes.length - done);
    } catch (e) {
      if (e && e.code === "EAGAIN") {
        Atomics.wait(sleepCell, 0, 0, 1);
        continue;
      }
      throw e;
    }
  }
}

// One frame: `kind` is "r" for a reply, "f" for a reply that mentions a
// function a call made, "q" for a question of the worker's own. The
// header carries what differs between two runs of the same request (its
// id, and the clock and random values it read); the body is the rest, so
// the replay check compares bodies as text.
function frame(kind, id, obj, clock) {
  // A value already encoded as JSON text goes in as it is.
  let json;
  if (obj.okText !== undefined) {
    const { okText, ...rest } = obj;
    const head = JSON.stringify(rest);
    json = `${head.slice(0, -1)}${head.length > 2 ? "," : ""}"ok":${okText}}`;
  } else {
    json = JSON.stringify(obj);
  }
  const clockText = clock && clock.length ? JSON.stringify(clock) : "";
  // A question goes at once: the worker waits for its answer.
  out(
    `\x1eBRRRTY ${kind} ${id === null ? 0 : id} ${Buffer.byteLength(clockText, "utf8")} ${Buffer.byteLength(json, "utf8")}\n${clockText}${json}`,
    kind !== "q" && queued > 0,
  );
}

process.stdout.write = process.stderr.write.bind(process.stderr);

// --- What the site's code reads. ---

const root = process.cwd();
const reads = new Set();

// `process.argv` as `npx @11ty/eleventy` (with `--serve` under `brrrty
// serve`) gives it: data files written for older 11ty check it for
// `--serve`. 11ty's other flags aren't known here.
process.argv = [
  process.execPath,
  path.join(root, "node_modules", ".bin", "eleventy"),
  ...(process.env.ELEVENTY_RUN_MODE === "serve" ? ["--serve"] : []),
];

function note(p) {
  try {
    if (p instanceof URL) {
      if (p.protocol === "file:") reads.add(fileURLToPath(p));
      return;
    }
    if (Buffer.isBuffer(p)) p = p.toString("utf8");
    if (typeof p === "string") {
      if (p.startsWith("file:")) {
        reads.add(fileURLToPath(p));
      } else {
        reads.add(path.resolve(root, p));
      }
    }
  } catch {
    // A path the site can't use either; nothing to watch.
  }
}

const readFileRaw = fs.readFileSync;

function wrap(obj, name) {
  const original = obj[name];
  if (typeof original !== "function") return;
  const wrapped = function (p, ...rest) {
    note(p);
    return original.call(this, p, ...rest);
  };
  Object.defineProperty(wrapped, "name", { value: original.name });
  obj[name] = wrapped;
}

for (const name of ["readFileSync", "readFile", "readdirSync", "readdir", "existsSync", "statSync", "stat", "lstatSync", "lstat", "opendir", "opendirSync", "createReadStream", "accessSync", "access"]) {
  wrap(fs, name);
}
for (const name of ["open", "openSync", "glob", "globSync"]) {
  wrap(fs, name);
}
for (const name of ["readFile", "readdir", "stat", "lstat", "opendir", "access", "open", "glob"]) {
  wrap(fs.promises, name);
}
nodeModule.syncBuiltinESMExports();

const requireHere = createRequire(path.join(root, "brrrty-worker.js"));
const loadHooks = typeof nodeModule.registerHooks === "function";

// --- Whether every function that can run is in strict mode. ---
//
// The worker can freeze the data files' values instead of walking them
// after every call (see "Freezing"), but only where a write to a frozen
// object can't pass in silence: in strict code it throws, in sloppy code
// it does nothing. So the worker follows every way code gets in: an ES
// module is strict; a CommonJS file is strict when it starts with "use
// strict"; `eval`, `new Function`, `vm`, a native addon, a preload, the
// module hooks and the inspector are each taken as code the worker can't
// vouch for. `unprovable` is why, once there is a reason: the worker then
// walks (and a build that already froze runs again, walking).
let unprovable = null;
// Why the build must run again with the walk: set once frozen.
let again = null;
// "walk": the data files' values are walked after each call. "frozen":
// they are frozen, and a failed write is what the worker looks for.
let exportsMode = "walk";

function cantVouch(why) {
  if (unprovable === null) unprovable = why;
  if (exportsMode === "frozen" && again === null) again = why;
}

// The build has to run again with every file as it was written, and the
// walk: strict mode may have changed what a file loaded as strict did.
function redo(why) {
  if (again === null) again = why;
}

// Brrrty asked for the walk (this folder's last build asked to run
// again, or `BRRRTY_JS_EXPORTS=walk`): nothing is frozen, and no file is
// loaded as strict that wasn't written so.
const walkOnly = process.env.BRRRTY_JS_EXPORTS === "walk";
if (walkOnly) cantVouch("BRRRTY_JS_EXPORTS=walk");
delete process.env.BRRRTY_JS_EXPORTS;
if (!loadHooks) cantVouch("this Node has no module load hooks (before 22.15), so the worker can't see every file that loads");
if (/(^|\s)(-r|--require|--import|--loader|--experimental-loader|--experimental-(default-)?config-file|--inspect[\w-]*)([=\s]|$)/.test(`${process.execArgv.join(" ")} ${process.env.NODE_OPTIONS || ""}`)) {
  cantVouch("Node was started with a preload, a loader or the inspector");
}

// Whether CommonJS source text is strict as a whole: its directive
// prologue (after a hashbang, white space and comments) has "use strict".
function isStrictSource(text) {
  let i = 0;
  const n = text.length;
  if (text.charCodeAt(0) === 0xfeff) i = 1;
  if (text.startsWith("#!", i)) {
    const nl = text.indexOf("\n", i);
    i = nl === -1 ? n : nl + 1;
  }
  for (;;) {
    // White space and comments.
    for (;;) {
      const c = text.charCodeAt(i);
      if (c === 32 || c === 9 || c === 10 || c === 13 || c === 11 || c === 12 || c === 0xa0 || c === 0x2028 || c === 0x2029 || c === 59) {
        i++;
      } else if (c === 47 && text.charCodeAt(i + 1) === 47) {
        const nl = text.indexOf("\n", i);
        i = nl === -1 ? n : nl + 1;
      } else if (c === 47 && text.charCodeAt(i + 1) === 42) {
        const end = text.indexOf("*/", i + 2);
        if (end === -1) return false;
        i = end + 2;
      } else {
        break;
      }
    }
    // One directive: a string literal standing as a statement.
    const q = text[i];
    if (q !== '"' && q !== "'") return false;
    const end = text.indexOf(q, i + 1);
    if (end === -1) return false;
    const body = text.slice(i + 1, end);
    if (body.includes("\\") || body.includes("\n")) return false;
    i = end + 1;
    // What follows must end the statement.
    let j = i;
    while (text.charCodeAt(j) === 32 || text.charCodeAt(j) === 9) j++;
    const after = text.charCodeAt(j);
    if (!(j >= n || after === 59 || after === 10 || after === 13 || after === 125 || (after === 47 && (text.charCodeAt(j + 1) === 47 || text.charCodeAt(j + 1) === 42)))) return false;
    if (body === "use strict") return true;
  }
}

// --- Sloppy CommonJS, run as strict where that changes nothing. ---
//
// Most CommonJS data files have no "use strict", and the worker can't
// freeze under sloppy code (a write to a frozen object does nothing,
// silently). But strict mode only changes what a file means in a few
// ways (ECMAScript's Annex C), and the worker can tell, from the file's
// syntax tree, a file where every one of them is loud:
// - the constructs strict mode forbids don't compile, so the file is
//   compiled both ways and left sloppy if strict fails;
// - a write that sloppy mode drops, an assignment to a name nothing
//   declares, `caller` and `callee`, a property read on a `this` that
//   sloppy mode would have made the global object: each throws a
//   TypeError or ReferenceError, which the watcher thread sees, caught or
//   not, and the build runs again with every file as it was written;
// - what strict mode changes without a sound is refused up front: the
//   `arguments` object (it stops tracking the parameters), `eval` (its
//   variables stop leaking), `this` used as a value in a function (it
//   would be `undefined` or a primitive, not the global object or a
//   wrapper), and a function declared inside a block (its name stops
//   leaking out).
// A file that passes is loaded with "use strict" before its first
// character, on the same line, so every line number stays.
const rawBinding = typeof process.binding === "function" ? process.binding.bind(process) : null;
const RawFunction = Function;
let acornModule;

// Node's own copy of the acorn parser.
function acornParser() {
  if (acornModule !== undefined) return acornModule;
  acornModule = null;
  try {
    const source = rawBinding && rawBinding("natives")["internal/deps/acorn/acorn/dist/acorn"];
    if (typeof source === "string") {
      const m = { exports: {} };
      new RawFunction("exports", "module", source)(m.exports, m);
      if (typeof m.exports.parse === "function") acornModule = m.exports;
    }
  } catch {
    acornModule = null;
  }
  return acornModule;
}

const FUNCTIONS = new Set(["FunctionDeclaration", "FunctionExpression"]);

// Whether the syntax tree of a sloppy script has nothing in it that strict
// mode changes in silence.
function sameInStrict(ast) {
  // (node, the node holding it, the key it is held under, whether inside
  // a function that has its own `this`)
  const stack = [[ast, null, "", false]];
  while (stack.length) {
    const [node, parent, key, inFunction] = stack.pop();
    let inside = inFunction;
    switch (node.type) {
      case "ClassDeclaration":
      case "ClassExpression":
        // Strict already, all of it.
        continue;
      case "WithStatement":
        return false;
      case "Identifier":
        if (node.name === "arguments" || node.name === "eval") {
          const named =
            parent &&
            !parent.computed &&
            ((parent.type === "MemberExpression" && key === "property") ||
              ((parent.type === "Property" || parent.type === "MethodDefinition" || parent.type === "PropertyDefinition") && key === "key"));
          if (!named) return false;
        }
        continue;
      case "ThisExpression":
        if (inFunction && !(parent && parent.type === "MemberExpression" && key === "object" && !parent.optional)) return false;
        continue;
      case "FunctionDeclaration":
        // Only at the top of a script or of a function's body.
        if (!(parent && (parent.type === "Program" || parent.functionBody === true))) return false;
        inside = true;
        break;
      case "FunctionExpression":
        inside = true;
        break;
      default:
        break;
    }
    if (FUNCTIONS.has(node.type) || node.type === "ArrowFunctionExpression") {
      const body = node.body;
      if (body && body.type === "BlockStatement") {
        // Strict already: nothing in it changes.
        const first = body.body[0];
        if (first && first.type === "ExpressionStatement" && first.directive === "use strict") continue;
        body.functionBody = true;
      }
    }
    for (const k of Object.keys(node)) {
      const child = node[k];
      if (child === null || typeof child !== "object") continue;
      if (Array.isArray(child)) {
        for (let i = child.length - 1; i >= 0; i--) {
          const c = child[i];
          if (c !== null && typeof c === "object" && typeof c.type === "string") stack.push([c, node, k, inside]);
        }
      } else if (typeof child.type === "string") {
        stack.push([child, node, k, inside]);
      }
    }
  }
  return true;
}

const USE_STRICT = '"use strict";';

// Where "use strict" goes in `source`: at its start, or after a hashbang
// line. The offset, and the line (from 0).
function useStrictAt(source) {
  let at = source.charCodeAt(0) === 0xfeff ? 1 : 0;
  let line = 0;
  if (source.startsWith("#!", at)) {
    const nl = source.indexOf("\n", at);
    at = nl === -1 ? source.length : nl + 1;
    line = 1;
  }
  return [at, line];
}

// `source` with "use strict" ahead of it.
function withUseStrict(source) {
  const [at] = useStrictAt(source);
  return `${source.slice(0, at)}${USE_STRICT}${source.slice(at)}`;
}

// The files loaded as strict that were written sloppy, and the line (from
// 0) "use strict" went on.
const strictLines = new Map();

// The files loaded as strict that were written sloppy, by the hash of
// their path (see `pathHash`), each with the places where strict mode
// throws an error sloppy mode wouldn't (see `strictPlaces`): an error the
// watcher saw at one of them may be strict mode's doing.
const strictified = new Map();

// Where, in a file loaded as strict, an error may be one only strict mode
// throws:
// - `this`: wherever a property of `this` is read, set or called. In a
//   plain call `this` is `undefined` in strict mode, and the global object
//   as written.
// - `assign`: wherever a plain `=` (or a `for...in`, `for...of`) assigns to
//   a name. A name nothing declares makes a global as written, and a
//   ReferenceError in strict mode. (`+=`, `++` and the like read the name
//   first, which throws either way.)
// Each place is the whole expression around it, so it errs toward
// counting. `starts` has where each line begins, for the inspector's
// lines and columns.
function strictPlaces(tree, text) {
  const places = { this: [], assign: [], starts: [0] };
  for (let i = text.indexOf("\n"); i !== -1; i = text.indexOf("\n", i + 1)) places.starts.push(i + 1);
  const named = (pattern) => {
    switch (pattern && pattern.type) {
      case "Identifier":
        return true;
      case "ArrayPattern":
        return pattern.elements.some(named);
      case "ObjectPattern":
        return pattern.properties.some((q) => named(q.type === "RestElement" ? q.argument : q.value));
      case "AssignmentPattern":
        return named(pattern.left);
      case "RestElement":
        return named(pattern.argument);
      default:
        return false;
    }
  };
  const stack = [[tree, null]];
  while (stack.length) {
    const [node, parent] = stack.pop();
    if (node.type === "MemberExpression" && node.object && node.object.type === "ThisExpression") {
      const around = parent || node;
      places.this.push(around.start, around.end);
    } else if (node.type === "AssignmentExpression" && node.operator === "=" && named(node.left)) {
      places.assign.push(node.start, node.end);
    } else if ((node.type === "ForInStatement" || node.type === "ForOfStatement") && named(node.left)) {
      places.assign.push(node.start, node.body ? node.body.start : node.end);
    }
    for (const k of Object.keys(node)) {
      const child = node[k];
      if (child === null || typeof child !== "object") continue;
      if (Array.isArray(child)) {
        for (const c of child) if (c !== null && typeof c === "object" && typeof c.type === "string") stack.push([c, node]);
      } else if (typeof child.type === "string") {
        stack.push([child, node]);
      }
    }
  }
  return places;
}

// Whether the inspector's (line, column) is in one of `ranges` (starts
// and ends, flat).
function inPlaces(places, ranges, line, column) {
  if (line < 0 || line >= places.starts.length) return true;
  const at = places.starts[line] + column;
  for (let i = 0; i < ranges.length; i += 2) {
    if (at >= ranges[i] && at <= ranges[i + 1]) return true;
  }
  return false;
}
// Whether the watcher is there to see what strict mode makes loud: set
// before the first file loads.
let canStrictify = false;

function pathHash(file) {
  let h = 0x811c9dc5;
  for (let i = 0; i < file.length; i++) {
    h ^= file.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h | 0;
}

// Sloppy CommonJS source as strict, when that changes nothing in silence
// (see above); null when it must stay as written.
function asStrict(source, file) {
  if (!canStrictify) return null;
  const acorn = acornParser();
  if (acorn === null) return null;
  try {
    const options = { ecmaVersion: "latest", sourceType: "script", allowReturnOutsideFunction: true, allowHashBang: true };
    if (!sameInStrict(acorn.parse(source, options))) return null;
    const strict = withUseStrict(source);
    // It must still be a program, to acorn and to V8, as Node wraps it.
    const tree = acorn.parse(strict, options);
    const at = strict.indexOf('"use strict";');
    new vm.Script(`${strict.slice(0, at)}(function (exports, require, module, __filename, __dirname) {${strict.slice(at)}\n})`);
    if (typeof file === "string") {
      strictified.set(pathHash(file), strictPlaces(tree, strict));
      strictLines.set(file, useStrictAt(source)[1]);
    }
    return strict;
  } catch {
    return null;
  }
}

// Builtin modules that run code the worker can't see, or change how
// JavaScript runs.
const UNSEEN_BUILTINS = new Set(["node:vm", "node:inspector", "node:inspector/promises", "node:repl", "node:v8", "node:wasi"]);
const STRICT_FORMATS = new Set(["module", "json", "builtin", "wasm", "module-typescript"]);

if (loadHooks) {
  nodeModule.registerHooks({
    resolve(specifier, context, nextResolve) {
      const r = nextResolve(specifier, context);
      if (r && typeof r.url === "string" && UNSEEN_BUILTINS.has(r.url)) {
        cantVouch(`the site's code loads ${r.url}`);
      }
      return r;
    },
    load(url, context, nextLoad) {
      if (url.startsWith("file:")) note(url.split("?")[0]);
      const r = nextLoad(url, context);
      if (!walkOnly) {
        try {
          const format = r && r.format;
          if (!STRICT_FORMATS.has(format)) {
            let source = r && r.source;
            if (source === null || source === undefined) {
              source = url.startsWith("file:") ? readFileRaw(fileURLToPath(url.split("?")[0]), "utf8") : null;
            } else if (typeof source !== "string") {
              source = Buffer.from(source.buffer ? new Uint8Array(source.buffer, source.byteOffset, source.byteLength) : source).toString("utf8");
            }
            if (typeof source !== "string" || !isStrictSource(source)) {
              const file = url.startsWith("file:") ? fileURLToPath(url.split("?")[0]) : null;
              const strict = typeof source === "string" && file !== null ? asStrict(source, file) : null;
              if (strict !== null) return { ...r, source: strict };
              cantVouch(`${file !== null ? relFile(file) : url} is not in strict mode, and has code that strict mode changes`);
            }
          }
        } catch {
          cantVouch(`${url} can't be read to see whether it is in strict mode`);
        }
      }
      return r;
    },
  });
  // Hooks the site's code adds itself may hand Node source the worker
  // never sees.
  for (const name of ["registerHooks", "register"]) {
    const original = nodeModule[name];
    if (typeof original !== "function") continue;
    nodeModule[name] = function (...args) {
      cantVouch("the site's code adds module hooks of its own");
      return original.apply(this, args);
    };
  }
  // Source compiled by hand (`module._compile`), or by a require hook.
  const compile = nodeModule.prototype._compile;
  if (typeof compile === "function") {
    nodeModule.prototype._compile = function (content, ...rest) {
      if (!walkOnly && typeof content === "string" && !isStrictSource(content)) {
        const strict = asStrict(content, typeof rest[0] === "string" ? rest[0] : null);
        if (strict !== null) {
          content = strict;
        } else {
          cantVouch(`${typeof rest[0] === "string" ? relFile(rest[0]) : "a script"} is not in strict mode, and has code that strict mode changes`);
        }
      }
      return compile.call(this, content, ...rest);
    };
  }
  nodeModule.syncBuiltinESMExports();
}

// `eval` and the function constructors make code from text.
{
  const realEval = globalThis.eval;
  defineProperty(globalThis, "eval", {
    get() {
      cantVouch("the site's code uses `eval`");
      return realEval;
    },
    set(v) {
      defineProperty(globalThis, "eval", { value: v, writable: true, configurable: true, enumerable: false });
    },
    configurable: true,
    enumerable: false,
  });
  // `Function("return this")()` is how old libraries find the global
  // object: it writes nothing. Text that starts with "use strict" is
  // strict.
  const harmless = (args) => {
    const body = args.length ? String(args[args.length - 1]) : "";
    return /^\s*return this;?\s*$/.test(body) || isStrictSource(body);
  };
  const guard = (ctor) =>
    new Proxy(ctor, {
      apply(t, self, args) {
        if (!harmless(args)) cantVouch("the site's code makes a function from text (`new Function`)");
        return Reflect.apply(t, self, args);
      },
      construct(t, args, newTarget) {
        if (!harmless(args)) cantVouch("the site's code makes a function from text (`new Function`)");
        return Reflect.construct(t, args, newTarget);
      },
    });
  const guardedFunction = guard(Function);
  for (const sample of [function () {}, async function () {}, function* () {}, async function* () {}]) {
    const proto = Object.getPrototypeOf(sample);
    const ctor = proto.constructor;
    const d = ownDescriptor(proto, "constructor");
    defineProperty(proto, "constructor", { ...d, value: ctor === Function ? guardedFunction : guard(ctor) });
  }
  globalThis.Function = guardedFunction;
  for (const name of ["dlopen", "binding", "_linkedBinding"]) {
    const original = process[name];
    if (typeof original !== "function") continue;
    process[name] = function (...args) {
      cantVouch(name === "dlopen" ? "the site's code loads a native addon" : "the site's code reaches into Node's internals");
      return original.apply(this, args);
    };
  }
}

function allReads() {
  for (const key of Object.keys(requireHere.cache)) note(key);
  return [...reads].sort();
}

// --- Decoding values Brrrty sends (see `wire.rs`). ---

const handles = [];
const handleOf = new Map();
// How many functions the data files' values gave: one numbered from here
// on was made by a call.
let loadHandles = Infinity;
// Set when a reply mentions a function a call made.
let mentionedMade = false;

// The lists and objects Brrrty defined (`def`), by id: the data pages
// share. A request refers to them (`{"$R": id}`), so each is one object
// however many pages hold it.
let objects = [];
let nextObject = 0;
// The plain objects among them: a computed key set inside one copies it
// for that page first (see `ownPath`).
let keptObjects = new WeakSet();
let defProblem = null;

// An object Brrrty can't answer every question about: `collections` when
// the site's config may add collections (`missing`: a name it doesn't have
// is a trap) or when 11ty's order of its names isn't known (`list`: listing
// them is a trap). Reads give watched values, writes are noted, as the
// membrane does.
const guardTarget = new WeakMap();

function guarded(o, c) {
  const absent = (t, k) => typeof k === "string" && c.missing && !Object.prototype.hasOwnProperty.call(t, k) && !(k in Object.prototype);
  const p = new Proxy(o, {
    get(t, k, r) {
      if (absent(t, k)) throw refuse(String(c.missing).replaceAll("{key}", k));
      return watch(Reflect.get(t, k, r));
    },
    has(t, k) {
      if (absent(t, k)) throw refuse(String(c.missing).replaceAll("{key}", k));
      return Reflect.has(t, k);
    },
    ownKeys(t) {
      if (c.list) throw refuse(String(c.list));
      return Reflect.ownKeys(t);
    },
    getOwnPropertyDescriptor(t, k) {
      const d = reflectOwnDescriptor(t, k);
      if (d && "value" in d && d.configurable) d.value = watch(d.value);
      return d;
    },
    set(t, k, v) {
      noteWrite();
      return Reflect.set(t, k, unwrapProxy(v));
    },
    defineProperty(t, k, d) {
      noteWrite();
      return Reflect.defineProperty(t, k, d);
    },
    deleteProperty(t, k) {
      noteWrite();
      return Reflect.deleteProperty(t, k);
    },
  });
  guardTarget.set(p, o);
  return p;
}

// Before setting the dotted `key` of a page's data: an object on the way
// that pages share is replaced, in this page's data, by a copy of it, as
// 11ty's merge gives every page its own copy of a plain object.
function ownPath(data, key) {
  const path = key.split(".");
  let o = data;
  for (let i = 0; i < path.length - 1; i++) {
    if (o === null || typeof o !== "object") return;
    const next = o[path[i]];
    if (next !== null && typeof next === "object" && !Array.isArray(next) && keptObjects.has(next)) {
      const copy = Object.create(Object.getPrototypeOf(next));
      Object.defineProperties(copy, ownDescriptors(next));
      o[path[i]] = copy;
      o = copy;
    } else {
      o = next;
    }
  }
}

// --- What a call writes. ---
//
// Data handed to a function is wrapped in a membrane: reads give wrapped
// plain objects and lists, and any write through it is noted, so a call
// that changes its data is caught without walking the data before and
// after.

let wrote = false;
const membraneOf = new WeakMap();
const targetOf = new WeakMap();
let touchedDates = [];

function unwrapProxy(v) {
  return (v !== null && typeof v === "object" && targetOf.get(v)) || v;
}
unwrapHook = unwrapProxy;

function noteWrite() {
  wrote = true;
}

const membrane = {
  get(t, k, r) {
    const v = Reflect.get(t, k, r);
    if (v === null || typeof v !== "object") return v;
    const d = reflectOwnDescriptor(t, k);
    if (d && !d.configurable && "value" in d && !d.writable) return v;
    // A collection item's `data` is that page's data, and its `page` the
    // page's one `page` object.
    if (k === "data" && isItem(t)) return dataOfItem(v);
    if (k === "page") return pageObject(v);
    return watch(v);
  },
  getOwnPropertyDescriptor(t, k) {
    const d = reflectOwnDescriptor(t, k);
    if (d && "value" in d && d.configurable) d.value = watch(d.value);
    return d;
  },
  set(t, k, v) {
    noteWrite();
    return Reflect.set(t, k, unwrapProxy(v));
  },
  defineProperty(t, k, d) {
    noteWrite();
    if ("value" in d) d = { ...d, value: unwrapProxy(d.value) };
    return Reflect.defineProperty(t, k, d);
  },
  deleteProperty(t, k) {
    noteWrite();
    return Reflect.deleteProperty(t, k);
  },
  setPrototypeOf(t, p) {
    noteWrite();
    return Reflect.setPrototypeOf(t, p);
  },
  preventExtensions(t) {
    noteWrite();
    return Reflect.preventExtensions(t);
  },
};

function watch(v) {
  if (v === null || typeof v !== "object" || targetOf.has(v) || utilTypes.isProxy(v)) return v;
  if (v instanceof OriginalDate) {
    touchedDates.push([v, OriginalDate.prototype.getTime.call(v)]);
    return v;
  }
  if (!Array.isArray(v) && !isPlainObject11ty(v)) return v;
  let p = membraneOf.get(v);
  if (!p) {
    p = new Proxy(v, membrane);
    membraneOf.set(v, p);
    targetOf.set(p, v);
  }
  return p;
}

// --- Which object is which. ---
//
// 11ty merges the data files into each page's data (`Merge` in
// @11ty/eleventy-utils): every page gets its own copy of each plain
// object, down to the first list or other value, which pages share. So
// `site.author` is one object within a page and another in the next,
// while `site.posts` and what is in it are the same everywhere. Brrrty
// keeps one of each value, so a page's data is read through a view: a
// plain object reached from the page's data through plain objects only
// is that page's own (one view for each place), and anything else is one
// for everybody (`watch`).
//
// A paginated page's data is 11ty's `ProxyWrap` of the pagination's values
// over the template's data: reading a plain object that the template's
// data has gives a new wrapper every time, so `data.site !== data.site`
// there. `page`, the frozen `eleventy` and `pkg`, the alias, and computed
// values the template's data didn't have are the values themselves.

const SHARED_AT_ROOT = new Set(["page", "eleventy", "pkg", "collections"]);
const SAME_WHEN_PAGED = new Set(["page", "eleventy", "pkg"]);
// The live `page` objects by what they say, and the data of the pages
// being read, so that one page has one of each however it is reached.
const pageObjects = new Map();
const rootOfData = new WeakMap();
let liveRoot = null;

function pageKeyOf(page) {
  if (page === null || typeof page !== "object") return null;
  const d = ownDescriptor(page, "inputPath");
  if (!d || typeof d.value !== "string") return null;
  const text = (name) => {
    const x = ownDescriptor(page, name);
    return x && "value" in x ? `${typeof x.value}:${String(x.value)}` : "";
  };
  return `${d.value}\0${text("url")}\0${text("outputPath")}`;
}

function pageObject(v) {
  if (targetOf.has(v) || utilTypes.isProxy(v)) return v;
  const key = Array.isArray(v) ? null : pageKeyOf(v);
  if (key === null) return watch(v);
  let p = pageObjects.get(key);
  if (!p) {
    p = watch(v);
    pageObjects.set(key, p);
  }
  return p;
}

function isItem(t) {
  return hasOwn.call(t, "inputPath") && hasOwn.call(t, "url") && hasOwn.call(t, "page");
}

// What identifies a page's data: its `page`, and which of a pagination's
// pages it is.
function dataKeyOf(data) {
  const page = ownDescriptor(data, "page");
  const key = page && "value" in page ? pageKeyOf(unwrapProxy(page.value)) : null;
  if (key === null) return null;
  const pg = ownDescriptor(data, "pagination");
  const at = pg && pg.value !== null && typeof pg.value === "object" ? unwrapProxy(pg.value).pageNumber : undefined;
  return `${key}\0${typeof at === "number" ? at : ""}`;
}

// A collection item's `data`: the data of the page being built when it
// is that page's, and one view of it otherwise.
function dataOfItem(v) {
  if (targetOf.has(v) || utilTypes.isProxy(v) || Array.isArray(v) || !isPlainObject11ty(v)) return watch(v);
  if (liveRoot !== null && liveRoot.key !== null && liveRoot.key === dataKeyOf(v)) return liveRoot.view;
  let p = rootOfData.get(v);
  if (p) return p;
  p = rootView(v, null);
  rootOfData.set(v, p);
  return p;
}

const ownHandler = {
  __proto__: null,
  get(t, k, r) {
    const v = Reflect.get(t, k, r);
    if (v === null || typeof v !== "object") return v;
    const d = reflectOwnDescriptor(t, k);
    if (d && !d.configurable && "value" in d && !d.writable) return v;
    return this.child(k, v);
  },
  getOwnPropertyDescriptor(t, k) {
    const d = reflectOwnDescriptor(t, k);
    if (d && "value" in d && d.configurable && d.value !== null && typeof d.value === "object") d.value = this.child(k, d.value);
    return d;
  },
  set: membrane.set,
  defineProperty: membrane.defineProperty,
  deleteProperty: membrane.deleteProperty,
  setPrototypeOf: membrane.setPrototypeOf,
  preventExtensions: membrane.preventExtensions,
  // The view of the value at `k`.
  child(k, v) {
    if (targetOf.has(v) || utilTypes.isProxy(v)) {
      // An open `collections` is a wrapper already; a paginated page
      // reads a new one each time.
      if (this.paged && k === "collections" && typeof k === "string") return freshView(v, false);
      return v;
    }
    if (typeof k !== "string" || Array.isArray(v) || !isPlainObject11ty(v)) return watch(v);
    if (this.root) {
      if (k === "page") return pageObject(v);
      if (k === "eleventy" || k === "pkg") iceReserved(v);
      if (this.paged) {
        if (SAME_WHEN_PAGED.has(k) || this.same.has(k)) return watch(v);
        return freshView(v, k !== "pagination");
      }
      if (SHARED_AT_ROOT.has(k)) return watch(v);
    } else if (this.fresh) {
      return freshView(v, true);
    }
    const kids = (this.kids ??= new Map());
    const have = kids.get(k);
    if (have !== undefined && have.t === v) return have.p;
    const p = view(v, { __proto__: ownHandler, root: false, paged: false, fresh: false, same: null, kids: null });
    kids.set(k, { t: v, p });
    return p;
  },
};

// 11ty freezes `eleventy` and `pkg`, and what is in them.
const iced = new WeakSet();
function iceReserved(v) {
  if (v === null || typeof v !== "object" || iced.has(v) || utilTypes.isProxy(v)) return;
  iced.add(v);
  if (!Array.isArray(v) && !isPlainObject11ty(v)) return;
  for (const k of Object.keys(v)) {
    const d = ownDescriptor(v, k);
    if (d && "value" in d) iceReserved(d.value);
  }
  objectFreeze(v);
}

function view(v, handler) {
  const p = new Proxy(v, handler);
  targetOf.set(p, unwrapProxy(v));
  return p;
}

// A wrapper nothing else has: with `deep`, what is read through it is
// new each time as well; without, it is the values themselves.
function freshView(v, deep) {
  if (!deep) return view(v, membrane);
  return view(v, { __proto__: ownHandler, root: false, paged: false, fresh: true, same: null, kids: null });
}

// A page's data as its functions and templates see it. `same` names the
// keys of a paginated page's data that are the values themselves, when
// known (see `pageOpen`).
function rootView(data, same) {
  if (data === null || typeof data !== "object" || targetOf.has(data) || utilTypes.isProxy(data)) return watch(data);
  if (Array.isArray(data) || !isPlainObject11ty(data)) return watch(data);
  const pg = ownDescriptor(data, "pagination");
  const pgv = pg && "value" in pg ? unwrapProxy(pg.value) : null;
  const paged = pgv !== null && typeof pgv === "object" && Array.isArray(pgv.items) && typeof pgv.pageNumber === "number";
  let keys = null;
  if (paged) {
    keys = new Set(same || []);
    if (typeof pgv.alias === "string") keys.add(pgv.alias.split(".")[0]);
    if (same === null) {
      // Not known: a computed value is taken to be one the template's
      // data didn't have.
      const computed = ownDescriptor(data, "eleventyComputed");
      const cv = computed && "value" in computed ? unwrapProxy(computed.value) : null;
      if (cv !== null && typeof cv === "object") for (const k of Object.keys(cv)) keys.add(k);
    }
  }
  return view(data, { __proto__: ownHandler, root: true, paged, fresh: false, same: keys, kids: null });
}

// After a call: whether it wrote to its data.
function takeWrote() {
  for (const [d, t] of touchedDates) {
    if (OriginalDate.prototype.getTime.call(d) !== t && !Number.isNaN(t)) noteWrite();
  }
  touchedDates = [];
  const out = wrote;
  wrote = false;
  return out;
}

// Forget the kept values: one was changed. Brrrty defines what it needs
// again, under new ids.
function dropPersistent() {
  objects = [];
  keptObjects = new WeakSet();
  pageObjects.clear();
}

// --- The clock, timers and random numbers. ---
//
// `Date.now()`, `new Date()`, `Date()`, `Intl.DateTimeFormat`'s `format()`
// with no date, `performance.now()`, `process.hrtime()`, `process.uptime()`
// and the random sources of `Math` and `crypto` give what they always give,
// as under 11ty, and each value is recorded: while the data files load,
// for the whole load; during a page's call, for that call (sent back with
// its reply). The replay check (see `replay` in the client) hands each
// replayed load and call the same values, so the check only sees what the
// order of calls changes. A value read in a `vm` context isn't seen.

const OriginalDate = Date;
const originalNow = Date.now;
const originalRandom = Math.random;
let clockPhase = "load"; // "load" | "calls"
const clockRecord = [];
let callClock = [];
let clockFeed = null;

// One read of an ambient source: `real()` gives it, `toJson`/`fromJson`
// carry it in the record.
// Each value is recorded as `[kind, value]`. A replayed load or call is
// handed its own values first; a replayed call that reads more of a kind
// than it read the first time (a value it saves, read first by another
// page the first time: `t ??= new Date()`) gets the first run's values of
// that kind in the order the first run read them.
let globalFeed = null;
const globalCursor = new Map();

function ambient(kind, real, toJson = (v) => v, fromJson = (v) => v) {
  const record = clockPhase === "calls" ? callClock : clockRecord;
  if (clockFeed !== null && clockFeed.length && clockFeed[0][0] === kind) {
    const fed = clockFeed.shift();
    record.push(fed);
    return fromJson(fed[1]);
  }
  if (clockPhase === "calls" && globalFeed !== null && Array.isArray(globalFeed[kind])) {
    const at = globalCursor.get(kind) || 0;
    if (at < globalFeed[kind].length) {
      globalCursor.set(kind, at + 1);
      const v = globalFeed[kind][at];
      record.push([kind, v]);
      return fromJson(v);
    }
  }
  const v = real();
  record.push([kind, toJson(v)]);
  return v;
}

const nowMs = () => ambient("now", () => originalNow.call(OriginalDate));

// Replace a function property, whatever kind of property it is; one that
// can't be replaced stays as it is.
function setFn(obj, name, fn) {
  try {
    const d = ownDescriptor(obj, name);
    Object.defineProperty(obj, name, {
      value: fn,
      writable: true,
      configurable: true,
      enumerable: d ? d.enumerable : false,
    });
  } catch {
    // Left as it is.
  }
}

function PatchedDate(...args) {
  if (!new.target) return new OriginalDate(nowMs()).toString();
  return Reflect.construct(OriginalDate, args.length ? args : [nowMs()], new.target);
}
Object.setPrototypeOf(PatchedDate, OriginalDate);
Object.defineProperty(PatchedDate, "prototype", { value: OriginalDate.prototype, writable: false });
Object.defineProperty(PatchedDate, "name", { value: "Date" });
Object.defineProperty(PatchedDate, "length", { value: 7 });
PatchedDate.now = function now() {
  return nowMs();
};
Object.defineProperty(OriginalDate.prototype, "constructor", { value: PatchedDate, writable: true, configurable: true, enumerable: false });
globalThis.Date = PatchedDate;
Math.random = function random() {
  return ambient("random", () => originalRandom.call(Math));
};

// `Intl.DateTimeFormat`: with no date, `format` and `formatToParts` read
// the clock themselves.
{
  const proto = Intl.DateTimeFormat.prototype;
  const formatGetter = ownDescriptor(proto, "format").get;
  Object.defineProperty(proto, "format", {
    configurable: true,
    enumerable: false,
    get() {
      const bound = formatGetter.call(this);
      return (date) => bound(date === undefined ? nowMs() : date);
    },
  });
  const toParts = proto.formatToParts;
  proto.formatToParts = function formatToParts(date) {
    return toParts.call(this, date === undefined ? nowMs() : date);
  };
}

{
  const perf = globalThis.performance;
  const perfNow = perf.now.bind(perf);
  setFn(perf, "now", function now() {
    return ambient("perf", perfNow);
  });
  const hrtime = process.hrtime;
  const hrBig = process.hrtime.bigint;
  const patched = function hrtime(prev) {
    const t = ambient("hrtime", () => hrtime());
    if (!prev) return t;
    let s = t[0] - prev[0];
    let ns = t[1] - prev[1];
    if (ns < 0) {
      s -= 1;
      ns += 1e9;
    }
    return [s, ns];
  };
  patched.bigint = function bigint() {
    return ambient("hrbig", () => hrBig(), (v) => String(v), (v) => BigInt(v));
  };
  process.hrtime = patched;
  const uptime = process.uptime;
  process.uptime = function () {
    return ambient("uptime", () => uptime());
  };
}

{
  const hex = (b) => Buffer.from(b.buffer, b.byteOffset, b.byteLength).toString("hex");
  const fill = (target, h) => {
    Buffer.from(h, "hex").copy(Buffer.from(target.buffer, target.byteOffset, target.byteLength));
    return target;
  };
  const rb = nodeCrypto.randomBytes;
  setFn(nodeCrypto, "randomBytes", function randomBytes(size, cb) {
    const bytes = ambient("bytes", () => rb(size), hex, (h) => Buffer.from(h, "hex"));
    if (typeof cb === "function") {
      setImmediate(() => cb(null, bytes));
      return undefined;
    }
    return bytes;
  });
  const uuid = nodeCrypto.randomUUID;
  setFn(nodeCrypto, "randomUUID", function randomUUID(options) {
    return ambient("uuid", () => uuid(options));
  });
  const rint = nodeCrypto.randomInt;
  setFn(nodeCrypto, "randomInt", function randomInt(...args) {
    const cb = typeof args[args.length - 1] === "function" ? args.pop() : null;
    const n = ambient("int", () => rint(...args));
    if (cb) {
      setImmediate(() => cb(null, n));
      return undefined;
    }
    return n;
  });
  const rfs = nodeCrypto.randomFillSync;
  setFn(nodeCrypto, "randomFillSync", function randomFillSync(buf, offset = 0, size) {
    const view = ArrayBuffer.isView(buf) ? new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength) : new Uint8Array(buf);
    const n = size ?? view.byteLength - offset;
    const part = view.subarray(offset, offset + n);
    ambient("bytes", () => rfs(part), hex, (h) => fill(part, h));
    return buf;
  });
  const grv = nodeCrypto.getRandomValues;
  setFn(nodeCrypto, "getRandomValues", function getRandomValues(arr) {
    return ambient("bytes", () => grv.call(nodeCrypto.webcrypto, arr), hex, (h) => fill(arr, h));
  });
  const web = globalThis.crypto;
  if (web) {
    const wgrv = web.getRandomValues.bind(web);
    const wuuid = web.randomUUID.bind(web);
    setFn(web, "getRandomValues", function getRandomValues(arr) {
      return ambient("bytes", () => wgrv(arr), hex, (h) => fill(arr, h));
    });
    setFn(web, "randomUUID", function randomUUID() {
      return ambient("uuid", () => wuuid());
    });
  }
  nodeModule.syncBuiltinESMExports();
}

// --- What the data's own code may not change. ---
//
// 11ty hands templates the values data files give as they are (lists, and
// what is in them; plain objects are copied into each page as it is
// placed), and its `getData` merges them only after every global data
// file has loaded. Brrrty's templates read each value as Brrrty received
// it. So:
// - each value a data file gives is recorded in full when it loads
//   ("complete": every own key, string or symbol, enumerable or not, each
//   getter read again, prototypes);
// - once the global data files have loaded, any of their values changed
//   since is sent again (`loadsChanged`), as 11ty's pages see it;
// - at the start of the pages' calls, a value changed since (by a template
//   or directory data file loading later) stops the build;
// - after each call, a quicker walk (values, list items, enumerable key
//   counts, prototypes, Dates, getters) of every value, plus the built-in
//   prototypes; a call that defines a property on one of these objects
//   gets the complete walk instead. Any change stops the build.
// A property added with a symbol key by plain assignment, and removed
// again before the end of the build, is seen only by the complete walk at
// the end (`exportsFinal`); templates can't read symbol keys.
//
// Freezing. The walk after each call costs as much as the data files'
// values weigh, every call. Where the worker can vouch that every
// function is strict (see `cantVouch`), it freezes those values instead
// when the pages' calls begin ("frozen" mode): a frozen object can't
// change, so there is nothing to walk. What a walk would have found is
// then a write that fails, and a failed write never passes unseen:
// - in strict code and in JavaScript's own functions it throws a
//   TypeError, and a thread of the worker's own watches every exception
//   the worker throws through the inspector, caught or not;
// - `Reflect.set` and its kin answer `false`, and are wrapped;
// - a Date's time isn't a property, so Date's setters are wrapped.
// Code that asks whether an object is frozen, or for a property's
// attributes, would see the worker's doing: those questions are wrapped
// too. Any of these, and anything else the worker can't vouch for, ends
// the build with `again`: Brrrty then runs it once more with the walk,
// which decides as it always has. So a build that stays frozen is one in
// which no call tried to change these values at all, and the walk would
// have passed after every call.
// What freezing can't hold still is walked as before: an object with a
// getter (its value may change), a Proxy, a RegExp (`lastIndex`), a
// typed array, and the built-in prototypes.

// Every value a data file gave: value -> file.
const loadedRoots = new Map();
// Complete records, by value, as of its load (or as last sent).
const loadRecords = new Map();
let quickRecords = null;
let finalRecords = null;
let replaying = false;
let exportChange = null;
// Every object inside a recorded value, and the built-in prototypes: a
// property defined on one of them marks the call for the complete walk.
let watchedObjects = new WeakSet();
let definedOnWatched = false;

// Wrap a function of the reflection API: `before` sees its first argument.
function wrapFirst(obj, name, before, after) {
  const original = obj[name];
  if (typeof original !== "function") return;
  const wrapped = function (target, ...rest) {
    before(target, rest);
    const r = original.call(this, target, ...rest);
    if (after) after(target, rest, r);
    return r;
  };
  defineProperty(wrapped, "name", { value: original.name, configurable: true });
  defineProperty(wrapped, "length", { value: original.length, configurable: true });
  setFn(obj, name, wrapped);
}

function isObject(v) {
  return v !== null && (typeof v === "object" || typeof v === "function");
}

// Questions that show whether the worker froze an object, and requests to
// freeze one that are no longer what they would be.
for (const [obj, name] of [
  [Object, "isFrozen"],
  [Object, "isSealed"],
  [Object, "isExtensible"],
  [Reflect, "isExtensible"],
  [Object, "getOwnPropertyDescriptor"],
  [Object, "getOwnPropertyDescriptors"],
  [Reflect, "getOwnPropertyDescriptor"],
  [Object, "freeze"],
  [Object, "seal"],
  [Object, "preventExtensions"],
  [Reflect, "preventExtensions"],
]) {
  wrapFirst(obj, name, (target) => {
    // A Proxy may stand in front of one of them.
    if (exportsMode === "frozen" && isObject(target) && (frozenByUs.has(target) || utilTypes.isProxy(target))) {
      cantVouch(`the site's code calls \`${obj === Object ? "Object" : "Reflect"}.${name}\` on a value a data file gave`);
    }
  });
}
// What a function can learn of the function that called it. A function in
// a file loaded as strict that wasn't written so (see `asStrict`) shows
// here as no caller at all.
{
  for (const name of ["caller", "arguments"]) {
    const d = ownDescriptor(Function.prototype, name);
    if (!d || typeof d.get !== "function") continue;
    const get = d.get;
    defineProperty(Function.prototype, name, {
      ...d,
      get() {
        if (strictified.size > 0) redo(`the site's code asks a function for its \`${name}\`, which a file loaded as strict hides or refuses`);
        return get.call(this);
      },
    });
  }
  // A stack trace's own objects give a frame's function and `this` only
  // for sloppy code.
  let prepare = Error.prepareStackTrace;
  defineProperty(Error, "prepareStackTrace", {
    get() {
      return prepare;
    },
    set(v) {
      if (typeof v === "function") {
        cantVouch("the site's code formats stack traces itself (`Error.prepareStackTrace`)");
        if (strictified.size > 0) redo("the site's code formats stack traces itself, where a file loaded as strict shows differently");
      }
      prepare = v;
    },
    configurable: true,
    enumerable: false,
  });
}
// A write that answers `false` instead of throwing.
wrapFirst(Reflect, "set", () => {}, (target, rest, r) => {
  const receiver = rest.length >= 3 ? rest[2] : target;
  if (r === false && exportsMode === "frozen" && isObject(receiver) && watchedObjects.has(receiver)) {
    cantVouch("the site's code sets a property of a value a data file gave (`Reflect.set`)");
  }
});
for (const name of ["__defineGetter__", "__defineSetter__"]) {
  const original = Object.prototype[name];
  if (typeof original !== "function") continue;
  setFn(Object.prototype, name, function (...args) {
    if (isObject(this) && watchedObjects.has(this)) definedOnWatched = true;
    return original.apply(this, args);
  });
}
// A Date's time: no property holds it, so freezing doesn't.
for (const name of Object.getOwnPropertyNames(Date.prototype)) {
  if (!/^set[A-Z]/.test(name) || typeof Date.prototype[name] !== "function") continue;
  const original = Date.prototype[name];
  setFn(Date.prototype, name, function (...args) {
    if (exportsMode === "frozen" && watchedObjects.has(this)) {
      cantVouch(`the site's code changes a Date a data file gave (\`${name}\`)`);
    }
    return original.apply(this, args);
  });
}

const BUILTIN_PROTOS = [
  Object.prototype, Array.prototype, String.prototype, Number.prototype, Boolean.prototype,
  Function.prototype, OriginalDate.prototype, RegExp.prototype, Map.prototype, Set.prototype,
  Promise.prototype, Symbol.prototype, Error.prototype,
];
for (const p of BUILTIN_PROTOS) watchedObjects.add(p);

function protosRecord() {
  const out = [];
  for (const p of BUILTIN_PROTOS) {
    for (const k of Reflect.ownKeys(p)) {
      const d = reflectOwnDescriptor(p, k);
      out.push(k, d.value, d.get, d.set, d.enumerable, d.writable);
    }
    out.push(Object.getPrototypeOf(p));
  }
  return out;
}
const startProtos = protosRecord();

// The same check, quickly, for a call that defined no property through
// `Object.defineProperty` and its kin (those are wrapped, and send the
// call to `protosRecord`). Without them a call can only assign to a
// property or delete one. So: each prototype's own prototype; each data
// property's value (an assignment changes it, a delete leaves
// `undefined`); each accessor still there (deleted and set again it would
// be a data property, and enumerable); and how many keys are enumerable
// (none of JavaScript's own are; any key an assignment adds, or adds
// again, is).
const protoQuick = BUILTIN_PROTOS.map((p) => {
  const keys = [];
  const vals = [];
  const accessors = [];
  for (const k of Reflect.ownKeys(p)) {
    const d = reflectOwnDescriptor(p, k);
    if ("value" in d && d.value !== undefined) {
      keys.push(k);
      vals.push(d.value);
    } else {
      accessors.push(k);
    }
  }
  return [p, keys, vals, accessors, Object.getPrototypeOf(p), Object.keys(p).length];
});
const hasOwn = Object.prototype.hasOwnProperty;

function protosSame() {
  for (let n = 0; n < protoQuick.length; n++) {
    const q = protoQuick[n];
    const p = q[0];
    if (Object.getPrototypeOf(p) !== q[4]) return false;
    const keys = q[1];
    const vals = q[2];
    for (let i = 0; i < keys.length; i++) {
      if (p[keys[i]] !== vals[i]) return false;
    }
    const accessors = q[3];
    for (let i = 0; i < accessors.length; i++) {
      if (!hasOwn.call(p, accessors[i])) return false;
    }
    if (Object.keys(p).length !== q[5]) return false;
  }
  return true;
}

function sameList(a, b) {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const x = a[i];
    const y = b[i];
    if (x !== y && !(x !== x && y !== y)) return false;
  }
  return true;
}

// The complete record of `v`: every object's identity (an object read
// through a getter by its shape only, since a getter may build it anew),
// prototype, and every own key with its kind, attributes and value; a
// getter is read, and what it gives is recorded too.
function completeRecord(v, mark, byShape = false) {
  const out = [];
  const seen = new Map();
  const stack = [[v, byShape]];
  while (stack.length) {
    const [x, viaGetter] = stack.pop();
    if (x === null || (typeof x !== "object" && typeof x !== "function")) {
      out.push(x);
      continue;
    }
    if (typeof x === "function") {
      out.push(x);
      continue;
    }
    if (seen.has(x)) {
      out.push(seen.get(x));
      continue;
    }
    seen.set(x, -1 - seen.size);
    if (mark) watchedObjects.add(x);
    out.push(viaGetter ? "object" : x, Object.getPrototypeOf(x));
    if (x instanceof OriginalDate) out.push(OriginalDate.prototype.getTime.call(x));
    if (utilTypes.isProxy(x)) continue;
    const keys = Reflect.ownKeys(x);
    out.push(keys.length);
    for (let i = keys.length - 1; i >= 0; i--) {
      const k = keys[i];
      const d = reflectOwnDescriptor(x, k);
      out.push(k, d.enumerable, d.configurable);
      if ("value" in d) {
        out.push(d.writable);
        stack.push([d.value, viaGetter]);
      } else {
        out.push(d.get, d.set);
        stack.push([getterValue(x, d.get), true]);
      }
    }
  }
  return out;
}

// Freeze `x` where that holds it still. False where it doesn't: `x` is
// then walked as before.
function hold(x) {
  if (utilTypes.isProxy(x) || utilTypes.isRegExp(x) || ArrayBuffer.isView(x) || utilTypes.isAnyArrayBuffer(x)) return false;
  if (objectIsFrozen(x)) return true;
  try {
    objectFreeze(x);
  } catch {
    return false;
  }
  frozenByUs.add(x);
  return true;
}

// The quick record of `v`, as `quickSame` checks it. With `freezing`,
// each object in it is frozen, and only what freezing can't hold still
// is recorded.
function quickRecord(v, freezing = false) {
  const out = [];
  const seen = new Set();
  const stack = [v];
  while (stack.length) {
    const x = stack.pop();
    if (x === null || typeof x !== "object" || seen.has(x)) continue;
    seen.add(x);
    watchedObjects.add(x);
    const proto = Object.getPrototypeOf(x);
    const held = freezing && hold(x);
    if (Array.isArray(x)) {
      if (!held) out.push(x, proto, 0, x.slice(), Object.keys(x).length);
      // A key this walk doesn't follow (a symbol, one that isn't
      // enumerable) may lead to an object that isn't frozen.
      if (freezing && Reflect.ownKeys(x).length !== Object.keys(x).length + 1) out.soft = true;
      for (let i = 0; i < x.length; i++) stack.push(x[i]);
      continue;
    }
    if (x instanceof OriginalDate) {
      // Frozen or not, its time changes only through Date's setters,
      // which say so.
      if (!freezing) out.push(x, proto, 1, OriginalDate.prototype.getTime.call(x), 0);
      continue;
    }
    const keys = Object.keys(x);
    const vals = new Array(keys.length);
    const getters = [];
    for (let i = 0; i < keys.length; i++) {
      const d = ownDescriptor(x, keys[i]);
      if (d && !("value" in d)) {
        // What a getter gives is compared by its shape: it may build it anew.
        getters.push(i);
        vals[i] = completeRecord(readGetter(x, keys[i]), false, true);
      } else {
        vals[i] = d ? d.value : undefined;
        stack.push(vals[i]);
      }
    }
    if (!held || getters.length) out.push(x, proto, 2, [keys, vals, getters], keys.length);
    if (freezing && Reflect.ownKeys(x).length !== keys.length) out.soft = true;
  }
  // `soft`: something in the value can still change (what is recorded
  // here, or behind a key this walk doesn't follow).
  if (out.length) out.soft = true;
  return out;
}

// A getter's value, read twice as the encoding reads it: one that gives a
// different number or string each time is refused where it crosses, so it
// counts as one fixed token here; one that builds a new object each time
// is compared by what is in it.
const UNSTABLE = "a getter whose value changes as it is read";

function getterValue(x, get) {
  if (!get) return undefined;
  try {
    const a = get.call(x);
    const b = get.call(x);
    if (a === b || (a !== a && b !== b)) return a;
    if (a !== null && b !== null && typeof a === "object" && typeof b === "object") return a;
    return UNSTABLE;
  } catch {
    return "a getter that throws";
  }
}

function readGetter(x, k) {
  const d = ownDescriptor(x, k);
  return d && !("value" in d) ? getterValue(x, d.get) : x[k];
}

// Whether every value still is as `rec` (from `quickRecord`) has it.
function quickSame(rec) {
  for (let j = 0; j < rec.length; j += 5) {
    const x = rec[j];
    if (Object.getPrototypeOf(x) !== rec[j + 1]) return false;
    const kind = rec[j + 2];
    if (kind === 0) {
      const items = rec[j + 3];
      if (x.length !== items.length) return false;
      for (let i = 0; i < items.length; i++) {
        const a = x[i];
        const b = items[i];
        if (a !== b && !(a !== a && b !== b)) return false;
      }
      if (Object.keys(x).length !== rec[j + 4]) return false;
    } else if (kind === 1) {
      const t = OriginalDate.prototype.getTime.call(x);
      if (t !== rec[j + 3] && !(t !== t && rec[j + 3] !== rec[j + 3])) return false;
    } else {
      const [keys, vals, getters] = rec[j + 3];
      let gi = 0;
      for (let i = 0; i < keys.length; i++) {
        const was = vals[i];
        if (gi < getters.length && getters[gi] === i) {
          gi++;
          if (!sameList(completeRecord(readGetter(x, keys[i]), false, true), was)) return false;
          continue;
        }
        const now = x[keys[i]];
        if (now !== was && !(now !== now && was !== was)) return false;
      }
      // The keys in their order: 11ty's templates list them in it.
      const nowKeys = Object.keys(x);
      if (nowKeys.length !== keys.length) return false;
      for (let i = 0; i < keys.length; i++) if (nowKeys[i] !== keys[i]) return false;
    }
  }
  return true;
}

let realRoot = root;
try {
  realRoot = fs.realpathSync(root);
} catch {
  realRoot = root;
}

function relFile(file) {
  let real = file;
  try {
    real = fs.realpathSync(file);
  } catch {
    real = file;
  }
  const base = real.startsWith(realRoot + path.sep) ? realRoot : root;
  return path.relative(base, base === realRoot ? real : file).split(path.sep).join("/");
}

// After a call from a page: whether it changed what the data files gave.
// Run once the call's reply is sent, before the next request.
function checkAfterCall(labelOf) {
  if (quickRecords === null || exportChange !== null) return;
  const complete = definedOnWatched;
  definedOnWatched = false;
  if (complete && exportsMode === "frozen") {
    // Whether it changed anything, the walk says.
    cantVouch("the site's code defines a property on a value a data file gave, or on a built-in prototype");
    return;
  }
  if (complete ? !sameList(protosRecord(), startProtos) : !protosSame()) {
    exportChange = `${labelOf()} changes one of JavaScript's built-in prototypes (\`Array.prototype\` and the like), which 11ty's templates use and Brrrty's don't`;
    return;
  }
  for (const [v, rec] of quickRecords) {
    const changed = complete ? !sameList(completeRecord(v, false), loadRecords.get(v)) : !quickSame(rec);
    if (changed) {
      exportChange = `${labelOf()} changes the data ${relFile(loadedRoots.get(v))} gives (an \`all.sort()\`, a \`list.push()\` or a property set on a value the file exports), which 11ty's templates then show changed from that call on; Brrrty's templates show the data as it loaded, so the pages would differ. Sort or add to a copy instead (\`[...all].sort()\`)`;
      return;
    }
  }
}

// Mark a call that defines a property on a watched object.
for (const [obj, name] of [
  [Object, "defineProperty"],
  [Object, "defineProperties"],
  [Reflect, "defineProperty"],
  [Object, "setPrototypeOf"],
  [Reflect, "setPrototypeOf"],
  [Reflect, "deleteProperty"],
  [Object, "freeze"],
  [Object, "seal"],
]) {
  wrapFirst(obj, name, (target) => {
    if (isObject(target) && watchedObjects.has(target)) definedOnWatched = true;
  });
}
// --- The thread that watches the worker's exceptions. ---
//
// It asks the inspector to stop this thread at every exception, caught or
// not, counts the ones a failed write to a frozen object throws, and lets
// the thread go on. Nothing else slows down: code that throws nothing
// runs as before.
// How many of the last errors the watcher keeps the places of.
const RING = 64;
const WATCHER = `
const { workerData, parentPort } = require("node:worker_threads");
const inspector = require("node:inspector");
const flags = workerData;
const session = new inspector.Session();
session.connectToMainThread();
// The text of V8's errors for a write a frozen object refuses, and for a
// Proxy whose target's frozen properties it must show as they are.
const WRITE = /read only|not extensible|Cannot delete|Cannot redefine|Cannot define|on proxy/;
// The other errors only strict mode throws, whatever is around them.
const STRICT_ONLY = /which has only a getter|Cannot create property|may not be accessed on strict mode|Assignment to constant variable/;
const RING = ${RING};
function pathHash(file) {
  let h = 0x811c9dc5;
  for (let i = 0; i < file.length; i++) {
    h ^= file.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h | 0;
}
// Every script's file, by the id the inspector gives it.
const scripts = new Map();
session.on("Debugger.scriptParsed", (m) => {
  if (m.params && m.params.url) scripts.set(m.params.scriptId, m.params.url);
});
session.on("Debugger.paused", (m) => {
  try {
    const p = m.params || {};
    if (p.reason === "exception" || p.reason === "promiseRejection") {
      const d = p.data;
      const text = d && typeof d.description === "string" ? d.description.split("\\n")[0] : null;
      if (text === null ? !d || d.type === "object" : WRITE.test(text)) Atomics.add(flags, 0, 1);
      // What kind of error, for a file loaded as strict that wasn't
      // written so: 1, one only strict mode throws; 2, a TypeError that
      // strict mode throws where \`this\` is \`undefined\`; 3, a
      // ReferenceError, which strict mode throws where a name nothing
      // declares is assigned to; 4, one the watcher can't place. Any
      // other error is the same either way.
      const kind =
        text === null
          ? 4
          : WRITE.test(text) || STRICT_ONLY.test(text)
            ? 1
            : /^TypeError\\b/.test(text)
              ? 2
              : /^ReferenceError\\b/.test(text)
                ? 3
                : /^(SyntaxError|EvalError)\\b/.test(text)
                  ? 4
                  : 0;
      if (kind !== 0) {
        // Where it was thrown, for the worker to tell whether a file it
        // loaded as strict threw it.
        const top = p.callFrames && p.callFrames[0];
        let file = (top && (top.url || scripts.get(top.location && top.location.scriptId))) || "";
        const query = file.indexOf("?");
        if (query !== -1) file = file.slice(0, query);
        if (file.startsWith("file:")) {
          try {
            file = require("node:url").fileURLToPath(file);
          } catch {}
        }
        const n = Atomics.load(flags, 2);
        const at = 4 + (n % RING) * 4;
        Atomics.store(flags, at, pathHash(file));
        Atomics.store(flags, at + 1, top && top.location ? top.location.lineNumber : -1);
        Atomics.store(flags, at + 2, top && top.location ? top.location.columnNumber || 0 : 0);
        Atomics.store(flags, at + 3, kind);
        Atomics.store(flags, 2, n + 1);
      }
    }
  } finally {
    session.post("Debugger.resume");
  }
});
parentPort.on("message", (m) => {
  if (m === "on") {
    session.post("Debugger.setPauseOnExceptions", { state: "all" }, (e) => parentPort.postMessage(e ? "failed" : "on"));
  }
  if (m === "bye") {
    try {
      session.disconnect();
    } catch {}
    Atomics.store(flags, 1, 1);
    Atomics.notify(flags, 1);
    process.exit(0);
  }
});
setInterval(() => {}, 1 << 30);
session.post("Debugger.enable", (e) => parentPort.postMessage(e ? "failed" : "ready"));
`;

let watcher = null;
// A frozen object of the worker's own, for one last failed write (see
// `exportsFinal`), and how Node wraps a CommonJS module.
const WATCHED_SAMPLE = objectFreeze({ a: 1 });
const WRAP_SAMPLE = typeof nodeModule.wrap === "function" ? nodeModule.wrap("X") : undefined;

// The thread, started while the data files load (the build's own worker
// only: a replay's compares replies), so it is there when the calls begin.
function launchWatcher() {
  try {
    const flags = new Int32Array(new SharedArrayBuffer(4 * (4 + RING * 4)));
    // It takes none of this process's own flags (`--eval` among them).
    const thread = new Thread(WATCHER, { eval: true, workerData: flags, stdout: true, stderr: true, execArgv: [], argv: [] });
    thread.unref();
    const ready = new Promise((resolve) => {
      thread.once("message", (m) => resolve(m === "ready"));
      thread.once("error", () => resolve(false));
      thread.once("exit", () => resolve(false));
    });
    return { thread, flags, ready };
  } catch {
    return null;
  }
}
const launched = unprovable === null ? launchWatcher() : null;
delete process.env.BRRRTY_ROLE;

// Turn the watcher on and try it: each kind of write a frozen object
// refuses must be counted, on this Node. False when it can't be relied on.
async function startWatcher() {
  if (launched === null) return false;
  const { thread, flags } = launched;
  try {
    const timeout = new Promise((resolve) => {
      const timer = setTimeout(() => resolve(false), 3000);
      timer.unref();
    });
    if (!(await Promise.race([launched.ready, timeout]))) {
      thread.terminate();
      return false;
    }
    const on = new Promise((resolve) => thread.once("message", (m) => resolve(m === "on")));
    thread.postMessage("on");
    if (!(await Promise.race([on, timeout]))) {
      thread.terminate();
      return false;
    }
    watcher = { thread, flags, seen: 0, strictSeen: 0 };
    const o = objectFreeze({ a: 1, list: objectFreeze([2, 1]) });
    const tries = [
      () => {
        o.a = 2;
      },
      () => {
        o.b = 1;
      },
      () => {
        delete o.a;
      },
      () => {
        o.a++;
      },
      () => {
        o.list[0] = 2;
      },
      () => {
        o.list.length = 0;
      },
      () => o.list.push(1),
      () => o.list.pop(),
      () => o.list.sort(),
      () => Object.assign(o, { a: 2 }),
      () => defineProperty(o, "a", { value: 2 }),
      () => defineProperty(o, "c", { value: 2 }),
      () => Object.setPrototypeOf(o, null),
      () => {
        o.__proto__ = null;
      },
      () => new Proxy(o, { get: () => 2 }).a,
    ];
    for (const attempt of tries) {
      const before = Atomics.load(flags, 0);
      let threw = false;
      try {
        attempt();
      } catch {
        threw = true;
      }
      if (!threw || Atomics.load(flags, 0) !== before + 1) {
        stopWatcher();
        return false;
      }
    }
    watcher.seen = Atomics.load(flags, 0);
    watcher.strictSeen = Atomics.load(flags, 2);
    canStrictify = !walkOnly && loadHooks;
    return true;
  } catch {
    thread.terminate();
    watcher = null;
    return false;
  }
}

// Let go of the inspector before Node exits, or Node says it is waiting
// for a debugger.
function stopWatcher() {
  const live = watcher || launched;
  if (!live || live.stopped) return;
  live.stopped = true;
  const { thread, flags } = live;
  watcher = null;
  try {
    thread.postMessage("bye");
    Atomics.wait(flags, 1, 0, 500);
  } catch {
    // It is gone already.
  }
}
process.on("exit", stopWatcher);

// Whether a write to a frozen object failed since the last look, or a
// file loaded as strict threw what strict mode throws.
function checkWatcher() {
  if (watcher === null) return;
  const { flags } = watcher;
  const n = Atomics.load(flags, 0);
  if (n !== watcher.seen) {
    watcher.seen = n;
    if (exportsMode === "frozen") cantVouch("the site's code tries to change a value a data file gave, or shows it through a Proxy");
  }
  const m = Atomics.load(flags, 2);
  if (m !== watcher.strictSeen) {
    const from = watcher.strictSeen;
    watcher.strictSeen = m;
    if (strictified.size === 0) return;
    // More than the watcher keeps places for: one of them may be ours.
    let ours = m - from > RING;
    for (let i = from; i < m && !ours; i++) {
      const at = 4 + (i % RING) * 4;
      const places = strictified.get(Atomics.load(flags, at));
      if (places === undefined) continue;
      const line = Atomics.load(flags, at + 1);
      const column = Atomics.load(flags, at + 2);
      const kind = Atomics.load(flags, at + 3);
      ours = kind === 2 ? inPlaces(places, places.this, line, column) : kind === 3 ? inPlaces(places, places.assign, line, column) : true;
    }
    if (ours) redo("a CommonJS file with no \"use strict\" throws an error that strict mode may have caused");
  }
}

// The watcher, turned on and tried, before the first file loads.
const watching = startWatcher();

function register(fn) {
  let at = handleOf.get(fn);
  if (at === undefined) {
    handles.push(fn);
    at = handles.length - 1;
    handleOf.set(fn, at);
  }
  if (at >= loadHandles) mentionedMade = true;
  return at;
}

class Trap {
  constructor(why) {
    this.why = why;
  }
}

// A request refers to a kept value the worker no longer has: a call
// changed one since Brrrty wrote the request, and the worker forgot them
// all. Brrrty sends the request again, the values with it.
class Stale extends Error {
  constructor(id) {
    super(`no kept value ${id}`);
  }
}
// The requests Brrrty sent together with one that was stale: none of the
// rest runs.
let staleTicket = null;
const HOLE = new Trap("a hole");

// A value from its wire form, in place where it can be: what `JSON.parse`
// made is the value, with each marker in it replaced. With `def`, every
// list and object takes the next id, in the order Brrrty wrote them.
function fromWire(w, def = false) {
  if (w === null || typeof w !== "object") return w;
  if (Array.isArray(w)) {
    if (def) objects[nextObject++] = w;
    for (let i = 0; i < w.length; i++) {
      const x = w[i];
      if (x === null || typeof x !== "object") continue;
      const v = fromWire(x, def);
      if (v === HOLE) delete w[i];
      else if (v !== x) w[i] = v;
    }
    return w;
  }
  const keys = Object.keys(w);
  if (keys.length !== 0 && keys[0].charCodeAt(0) === 36) return fromMarker(w, keys[0], def);
  if (def) {
    objects[nextObject++] = w;
    keptObjects.add(w);
  }
  for (let i = 0; i < keys.length; i++) {
    const k = keys[i];
    const x = w[k];
    if (x === null || typeof x !== "object") continue;
    const v = fromWire(x, def);
    if (v instanceof Trap) trapKey(w, k, v.why);
    else if (v !== x) w[k] = v;
  }
  return w;
}

function fromMarker(w, tag, def) {
  switch (tag) {
    case "$R": {
      const v = objects[w.$R];
      if (v === undefined) throw new Stale(w.$R);
      return v;
    }
    case "$u":
      return undefined;
    case "$n":
      return Number(w.$n);
    case "$d":
      return new Date(w.$d);
    case "$f":
      if (typeof handles[w.$f] !== "function") throw new Error(`no function ${w.$f}`);
      return handles[w.$f];
    case "$t":
      return new Trap(w.$t);
    case "$hole":
      return HOLE;
    case "$o": {
      const o = {};
      const id = def ? nextObject++ : -1;
      if (def) keptObjects.add(o);
      for (const [k, x] of w.$o) {
        const v = fromWire(x, def);
        if (v instanceof Trap) {
          trapKey(o, k, v.why);
        } else {
          // A key named `__proto__` is an own property, as in parsed JSON.
          defineProperty(o, k, { value: v, writable: true, enumerable: true, configurable: true });
        }
      }
      const made = w.$c ? guarded(o, w.$c) : o;
      if (def) objects[id] = made;
      return made;
    }
    default:
      throw new Error(`not a wire form: ${JSON.stringify(w).slice(0, 200)}`);
  }
}

// One definition: the value takes `req.id`, and what is in it the ids
// after, `req.n` in all.
function define(req) {
  if (defProblem !== null) return;
  try {
    nextObject = req.id;
    fromWire(req.v, true);
    if (nextObject - req.id !== req.n) defProblem = `definition ${req.id} holds ${nextObject - req.id} values, not ${req.n}`;
  } catch (e) {
    defProblem = `definition ${req.id}: ${e && e.message}`;
  }
}

// The argument as text: 11ty merges the global data files over this same
// object, so a data function that changes it changes every page's data.
function argumentPrint() {
  return argument === undefined ? "" : JSON.stringify(encodeValue(argument, () => -1));
}

// The argument's text as of the last check: it only changes when a data
// function changes the argument, which stops the build.
let argumentText;

function deepFreeze(o) {
  if (o && typeof o === "object" && !objectIsFrozen(o)) {
    objectFreeze(o);
    for (const k of Object.keys(o)) deepFreeze(o[k]);
  }
  return o;
}

// --- Loading data files as Eleventy 3.1.8 does. ---

let isEsm = false;
try {
  isEsm = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8")).type === "module";
} catch {
  isEsm = false;
}

const requestPromiseCache = new Map();
// The JSON text last sent for each load, by call mode and path.
const lastSent = new Map();
let argument;

// `dynamicImportAbsolutePath` from src/Util/Require.js, without the JSON
// and cache-busting paths Brrrty never takes.
function dynamicImportAbsolutePath(absolutePath, type) {
  let urlPath;
  try {
    urlPath = new URL(`file:${absolutePath}`).toString();
  } catch {
    urlPath = absolutePath;
  }
  let promise;
  if (requestPromiseCache.has(urlPath)) {
    promise = requestPromiseCache.get(urlPath);
  } else {
    promise = import(urlPath);
    requestPromiseCache.set(urlPath, promise);
  }
  return promise.then((target) => {
    if (Object.keys(target).length === 1 && "default" in target) {
      return target.default;
    }
    if (type === "cjs" && "default" in target) {
      let match = true;
      for (const key in target) {
        if (key === "default") continue;
        if (key === "module.exports") continue;
        if (target[key] !== target.default[key]) {
          match = false;
        }
      }
      if (match) {
        return target.default;
      }
    }
    return Object.assign({}, target);
  });
}

// How 11ty imports a data file: ESM for `.mjs`, and `.js` in a
// `"type": "module"` project; CommonJS otherwise.
function dataType(absolutePath) {
  const ext = path.extname(absolutePath).slice(1);
  return ext === "mjs" || (ext === "js" && isEsm) ? "esm" : "cjs";
}

// `getDataValue` from src/Data/TemplateData.js, for JavaScript files.
// `changed` is set when a data function changed its argument.
async function getDataValue(absolutePath, call, state) {
  if (!fs.existsSync(absolutePath)) return {};
  let value = await dynamicImportAbsolutePath(absolutePath, dataType(absolutePath));
  if (call && typeof value === "function") {
    state.called = true;
    if (argumentText === undefined) argumentText = argumentPrint();
    value = await value(argument || {});
    state.changed = argumentPrint() !== argumentText;
  }
  return value;
}

// `isPlainObject` from @11ty/eleventy-utils, which sees through a Proxy.
function isPlainObject(value) {
  if (value === null || typeof value !== "object") return false;
  const proto = Object.getPrototypeOf(value);
  return !proto || proto === Object.prototype;
}

// A Proxy of a plain object, as 11ty's merge copies it (`for...in`, each
// key read once). Anything else stays as it is.
function rootValue(value) {
  if (!utilTypes.isProxy(value) || !isPlainObject(value)) return value;
  const copy = {};
  for (const key in value) copy[key] = value[key];
  return copy;
}

// --- JavaScript templates, as Eleventy 3.1.8's src/Engines/JavaScript.js,
// src/Util/GetJavaScriptData.js and src/Engines/Util/ContextAugmenter.js
// make and render them (MIT, see NOTICE). ---

// Every template and JavaScript layout loaded, by id: its instance, file,
// and whether a layout's `this.page` is set up.
const templates = [];
// The names 11ty puts on a template's `this` as functions, in its order:
// its own filters, then the config's (`templateSetup`).
let filterNames = [];

// `EleventyImport(inputPath, isEsm ? "esm" : "cjs", { cacheBust: true })`:
// the engine isn't cacheable, so each template's URL is busted, and the
// type is the project's, whatever the extension.
function importTemplate(absolutePath) {
  let urlPath;
  try {
    const u = new URL(`file:${absolutePath}`);
    u.searchParams.set("_cache_bust", String(originalNow.call(OriginalDate)));
    urlPath = u.toString();
  } catch {
    urlPath = absolutePath;
  }
  const type = isEsm ? "esm" : "cjs";
  let promise = requestPromiseCache.get(urlPath);
  if (!promise) {
    promise = import(urlPath);
    requestPromiseCache.set(urlPath, promise);
  }
  return promise.then((target) => {
    if (Object.keys(target).length === 1 && "default" in target) {
      return target.default;
    }
    if (type === "cjs" && "default" in target) {
      let match = true;
      for (const key in target) {
        if (key === "default") continue;
        if (key === "module.exports") continue;
        if (target[key] !== target.default[key]) {
          match = false;
        }
      }
      if (match) {
        return target.default;
      }
    }
    return Object.assign({}, target);
  });
}

// `JavaScript._getInstance`.
function getInstance(mod) {
  const noop = function () {
    return "";
  };
  const originalModData = mod?.data;
  if (typeof mod === "object" && mod.default && isEsm) {
    mod = mod.default;
  }
  if (typeof mod === "string" || mod instanceof Buffer || mod.then) {
    return { render: () => mod };
  } else if (typeof mod === "function") {
    if (mod.prototype?.data || mod.prototype?.render) {
      if (!("render" in mod.prototype)) {
        mod.prototype.render = noop;
      }
      if (!("data" in mod.prototype) && !mod.data && originalModData) {
        mod.prototype.data = originalModData;
      }
      return new mod();
    }
    return {
      ...(originalModData ? { data: originalModData } : undefined),
      render: mod,
    };
  } else if ("data" in mod || "render" in mod) {
    if (!mod.render) {
      mod.render = noop;
    }
    if (!mod.data && originalModData) {
      mod.data = originalModData;
    }
    return mod;
  }
  return undefined;
}

// 11ty's own refusal of a template: it stops the whole build.
function invalidTemplate(message) {
  const e = new Error(message);
  e.brrrtyInvalid = true;
  return e;
}

// Load a template or a JavaScript layout: its instance, then its data as
// `getJavaScriptData` reads it, copied as `#getFrontMatterData` copies it
// (`Object.assign({}, extraData)`).
async function loadTemplate(req) {
  let made;
  const early = preloaded.get(req.path);
  if (early) {
    preloaded.delete(req.path);
    if (early.error) throw early.error;
    made = early.made;
  } else {
    made = await makeTemplate(req.path);
  }
  const { inst, data } = made;
  const root = Object.assign({}, data);
  loadedRoots.set(root, req.path);
  loadRecords.set(root, completeRecord(root, false));
  templates.push({ inst, file: req.path, layout: req.layout === true, pageSet: false });
  return { template: templates.length - 1, okText: JSON.stringify(encodeValue(root, register)) };
}

// Templates and data files the replay check imported ahead of their
// loads, in another order than the build's (`preload`), by path.
const preloaded = new Map();

// Import a template and make its instance and data, as 11ty does when
// its file has loaded.
async function makeTemplate(filePath) {
  const shown = relFile(filePath);
  let mod;
  try {
    mod = await importTemplate(filePath);
  } catch (e) {
    throw placeSyntaxError(e, filePath);
  }
  let inst;
  try {
    inst = getInstance(mod);
  } catch (e) {
    throw invalidTemplate(`11ty can't make a template of what ${shown} exports: ${e && e.message}`);
  }
  if (!inst) {
    throw invalidTemplate(`No JavaScript template returned from ${shown}. Did you assign module.exports (CommonJS) or export (ESM)?`);
  }
  let data;
  if (inst && "data" in inst) {
    data = await (typeof inst.data === "function" ? inst.data() : inst.data);
    if (typeof data !== "object") {
      throw invalidTemplate(`Invalid data format returned from ${shown}: typeof ${typeof data}`);
    }
  }
  return { inst, data };
}

// One line from Brrrty's answers pipe, waiting for it.
const answerBuf = Buffer.alloc(65536);
let answerPending = Buffer.alloc(0);

function readAnswer() {
  for (;;) {
    const nl = answerPending.indexOf(10);
    if (nl !== -1) {
      const line = answerPending.subarray(0, nl).toString("utf8");
      answerPending = answerPending.subarray(nl + 1);
      return line;
    }
    let n;
    try {
      n = fs.readSync(answersFd, answerBuf, 0, answerBuf.length, null);
    } catch (e) {
      if (e && e.code === "EAGAIN") {
        Atomics.wait(sleepCell, 0, 0, 1);
        continue;
      }
      throw e;
    }
    if (n === 0) throw new Error("Brrrty closed its answers");
    answerPending = Buffer.concat([answerPending, answerBuf.subarray(0, n)]);
  }
}

// The request each piece of code runs for (timers and promises keep the
// one they were made in), and the one in flight: a filter called from a
// timer a render left behind is not that request's.
const requestContext = new AsyncLocalStorage();
let activeRequest = null;
let strayFilter = null;

// Ask Brrrty to run one of its filters, as 11ty's own would run in
// JavaScript, and wait for the result.
function askFilter(name, args) {
  const ctx = requestContext.getStore();
  if (!ctx || ctx.id !== activeRequest) {
    // Its answer would be counted against another request: say so at the
    // end of the build instead, naming where it came from.
    if (strayFilter === null) {
      const where = ctx && typeof ctx.template === "number" && templates[ctx.template] ? ` in ${relFile(templates[ctx.template].file)}` : "";
      strayFilter = `\`this.${name}\` is called${where} after its render ended (from a timer or a promise it didn't wait for), where Brrrty can't tell which page it belongs to`;
    }
    return undefined;
  }
  if (answersFd === null) {
    throw refuse(`calls \`this.${name}\`, one of the filters 11ty gives a JavaScript template, which Brrrty can't run here`);
  }
  let text;
  try {
    text = JSON.stringify(encodeValue(args.map(unwrapProxy), register));
  } catch (e) {
    if (e && e.brrrtyRefusal) throw refuse(`passes \`this.${name}\` something that ${e.brrrtyRefusal}`);
    throw e;
  }
  frame("q", activeRequest, { filter: name, okText: text });
  const answer = JSON.parse(readAnswer());
  if (answer.refused !== undefined) throw refuse(String(answer.refused));
  return fromWire(answer.ok);
}

// The functions 11ty puts on `this` for a render (`getJavaScriptFunctions`):
// one new wrapper per name, each render; each runs Brrrty's own filter.
function filterFunctions() {
  const out = {};
  for (const name of filterNames) {
    out[name] = function (...args) {
      return askFilter(name, args);
    };
  }
  return out;
}

const LAYOUT_PAGE =
  "reads `this.page` in a JavaScript layout, which 11ty sets from whichever page renders the layout first, and its pages render at the same time";

// One render, as the function `JavaScript.compile()` returns: `page` and
// `eleventy` added to the instance unless it has them (so `this.page` is
// the first page's: Brrrty sends page 0's page as `first`), the filters
// assigned, then `render` called and awaited.
async function renderTemplate(t, data, first) {
  refusedInCall = null;
  takeWrote();
  const inst = t.inst;
  const wdata = rootView(data, null);
  const before = liveRoot;
  liveRoot = { key: dataKeyOf(data), view: wdata };
  let value;
  try {
    if (t.layout) {
      if (!t.pageSet) {
        t.pageSet = true;
        if (!inst.page) trapKey(inst, "page", LAYOUT_PAGE);
      }
    } else if (!inst.page) {
      const page = first !== undefined && first !== null && typeof first === "object" ? pageObject(first) : wdata.page;
      if (page) inst.page = page;
    }
    if (!inst.eleventy) {
      const eleventy = wdata.eleventy;
      if (eleventy) inst.eleventy = eleventy;
    }
    Object.assign(inst, filterFunctions());
    value = inst.render.call(inst, wdata);
    if (Buffer.isBuffer(value)) value = value.toString();
    if (value && typeof value.then === "function") {
      value = await value;
      if (Buffer.isBuffer(value)) {
        throw refuse("returns a promise of a Buffer, which 11ty keeps as a Buffer");
      }
    }
  } catch (e) {
    liveRoot = before;
    const failed = renderRefusal(inst);
    if (failed) return failed;
    throw e;
  }
  liveRoot = before;
  const failed = renderRefusal(inst);
  if (failed) return failed;
  let text;
  try {
    text = JSON.stringify(encodeValue(unwrapProxy(value), register));
  } catch (e) {
    if (e && e.brrrtyRefusal) return { refused: `${describeRender(inst)} returns something that ${e.brrrtyRefusal}` };
    throw e;
  }
  return { okText: text };
}

function describeRender(inst) {
  return typeof inst.render === "function" ? describeFn(inst.render) : "the template's render";
}

// Why a render that just ran can't be used, if it can't.
function renderRefusal(inst) {
  const wroteData = takeWrote();
  if (refusedInCall !== null) return { refused: `${describeRender(inst)} ${refusedInCall}` };
  if (wroteData) {
    dropPersistent();
    return {
      refused: `${describeRender(inst)} changes the data it is given (a \`list.sort()\` or a property set on it), which 11ty's later pages then see changed; Brrrty can't follow that, so sort or change a copy instead (\`[...list].sort()\`)`,
      dropShared: true,
    };
  }
  return null;
}

// --- Errors. ---

// Places in `text` (a stack trace) as the files are written: on the line
// "use strict" was put on, a file loaded as strict has every column that
// much further right.
function asWritten(text) {
  for (const [file, line] of strictLines) {
    for (const name of [file, pathToFileURL(file).href]) {
      if (!text.includes(name)) continue;
      text = text.replaceAll(new RegExp(`(${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}:${line + 1}:)(\\d+)`, "g"), (all, head, column) =>
        Number(column) > USE_STRICT.length ? `${head}${Number(column) - USE_STRICT.length}` : all,
      );
    }
  }
  return text;
}

function thrown(e) {
  if (e instanceof Error || (e && typeof e === "object" && typeof e.stack === "string")) {
    return {
      name: toWellFormed(String(e.name || "Error")),
      message: toWellFormed(String(e.message ?? "")),
      stack: asWritten(toWellFormed(String(e.stack || ""))),
      inspect: asWritten(toWellFormed(String(e.brrrtyPlace || util.inspect(e)))),
    };
  }
  const text = typeof e === "string" ? JSON.stringify(e) : util.inspect(e);
  return { name: "", message: toWellFormed(`Uncaught ${text}`), stack: "", inspect: "" };
}

// An ES module's SyntaxError from import() carries no file or line (Node
// prints them only for an uncaught one), so ask `node --check`, which does.
function placeSyntaxError(e, file) {
  if (!(e instanceof SyntaxError) || String(e.stack).includes(file)) return e;
  try {
    const r = spawnSync(process.execPath, ["--check", file], { encoding: "utf8", timeout: 10000 });
    const header = String(r.stderr || "").split("\n")[0];
    if (/^(file:\/\/|\/)\S*:\d+$/.test(header)) {
      Object.defineProperty(e, "brrrtyPlace", { value: header, enumerable: false });
    }
  } catch {
    // Keep the error as it is.
  }
  return e;
}

// --- Calling functions from data. ---

// A read Brrrty can't answer: a property whose value it doesn't have, or
// `this` where 11ty passes an object Brrrty doesn't model. The call is
// refused even when the site's code catches the error.
let refusedInCall = null;

function refuse(why) {
  if (refusedInCall === null) refusedInCall = why;
  const e = new Error(`Brrrty: this function ${why}`);
  e.brrrtyRefusal = why;
  return e;
}

function trapKey(o, k, why) {
  Object.defineProperty(o, k, {
    get() {
      throw refuse(why);
    },
    set(v) {
      Object.defineProperty(o, k, { value: v, writable: true, enumerable: true, configurable: true });
    },
    enumerable: true,
    configurable: true,
  });
}

// `this` for a call where 11ty passes something Brrrty doesn't model.
function opaqueThis(what) {
  const fail = (key) => {
    throw refuse(`reads \`this${typeof key === "string" ? `.${key}` : ""}\`, which is ${what} in 11ty`);
  };
  return new Proxy(Object.create(null), {
    get: (_t, k) => fail(k),
    set: (_t, k) => fail(k),
    has: (_t, k) => fail(k),
    deleteProperty: (_t, k) => fail(k),
    defineProperty: (_t, k) => fail(k),
    getOwnPropertyDescriptor: (_t, k) => fail(k),
    ownKeys: () => fail(),
    getPrototypeOf: () => fail(),
    setPrototypeOf: () => fail(),
    isExtensible: () => fail(),
    preventExtensions: () => fail(),
  });
}

// Where a function is defined, as `file:line:column` relative to the
// project, found with the inspector (only when a message needs it).
function fnPlace(fn) {
  let session;
  try {
    session = new inspector.Session();
    session.connect();
    const scripts = new Map();
    session.on("Debugger.scriptParsed", (m) => scripts.set(m.params.scriptId, m.params.url));
    session.post("Debugger.enable");
    globalThis.__brrrtyFunction = fn;
    let objectId;
    session.post("Runtime.evaluate", { expression: "globalThis.__brrrtyFunction" }, (e, r) => {
      objectId = r && r.result && r.result.objectId;
    });
    let loc;
    if (objectId) {
      session.post("Runtime.getProperties", { objectId, ownProperties: true }, (e, r) => {
        const p = r && r.internalProperties && r.internalProperties.find((x) => x.name === "[[FunctionLocation]]");
        loc = p && p.value && p.value.value;
      });
    }
    session.post("Debugger.disable");
    if (!loc) return null;
    let file = scripts.get(loc.scriptId) || "";
    if (file.startsWith("file:")) file = fileURLToPath(file);
    if (!file) return null;
    const rel = path.relative(root, file);
    const shown = rel && !rel.startsWith("..") && !path.isAbsolute(rel) ? rel : file;
    // In a file loaded as strict, the line "use strict" went on is
    // that much longer than the file's own.
    const shift = strictLines.get(file) === loc.lineNumber && loc.columnNumber >= USE_STRICT.length ? USE_STRICT.length : 0;
    return `${shown.split(path.sep).join("/")}:${loc.lineNumber + 1}:${loc.columnNumber + 1 - shift}`;
  } catch {
    return null;
  } finally {
    delete globalThis.__brrrtyFunction;
    try {
      session && session.disconnect();
    } catch {
      // Nothing to disconnect.
    }
  }
}

// "the function `money` (src/_data/fmt.js:3:10)".
const described = new WeakMap();

function describeFn(fn) {
  let text = described.get(fn);
  if (text === undefined) {
    const name = fn && fn.name ? ` \`${toWellFormed(String(fn.name))}\`` : "";
    const place = fnPlace(fn);
    text = `the function${name}${place ? ` (${place})` : ""}`;
    described.set(fn, text);
  }
  return text;
}

// Run one call, once, as 11ty does. The reply's `refused` when it can't
// be used exactly.
async function runOnce(fn, thisArg, args, settle, page = null) {
  refusedInCall = null;
  takeWrote();
  let value;
  const before = liveRoot;
  if (page !== null) liveRoot = { key: dataKeyOf(page.data), view: page.view };
  try {
    value = fn.apply(watch(thisArg), page !== null ? [page.view] : args.map(watch));
    if (settle === "await") {
      value = await value;
    } else if (value && typeof value.then === "function" && typeof value.catch === "function") {
      // Not awaited, as Nunjucks doesn't: don't let a rejection end Node.
      value.catch(() => {});
    }
  } catch (e) {
    liveRoot = before;
    const failed = callRefusal(fn);
    if (failed) return failed;
    throw e;
  }
  liveRoot = before;
  const failed = callRefusal(fn);
  if (failed) return failed;
  let text;
  try {
    text = JSON.stringify(encodeValue(unwrapProxy(value), register));
  } catch (e) {
    if (e && e.brrrtyRefusal) return { refused: `${describeFn(fn)} returns something that ${e.brrrtyRefusal}` };
    throw e;
  }
  return { okText: text, value: unwrapProxy(value) };
}

// Why a call that just ran can't be used, if it can't.
function callRefusal(fn) {
  const wroteData = takeWrote();
  if (refusedInCall !== null) return { refused: `${describeFn(fn)} ${refusedInCall}` };
  if (wroteData) {
    // Kept values may be what it changed: Brrrty sends them again.
    dropPersistent();
    return {
      refused: `${describeFn(fn)} changes the data it is given, which 11ty keeps for later pages and templates; Brrrty can't follow that, so return a new value instead`,
      dropShared: true,
    };
  }
  return null;
}

const pages = new Map();

// Whether setting the dotted `key` would go through a list (or a
// function): lodash would set a property on it, and lists are shared
// between pages.
function throughList(data, key) {
  const path = key.split(".");
  let o = data;
  for (let i = 0; i < path.length - 1; i++) {
    if (o === null || typeof o !== "object") return false;
    o = o[path[i]];
    if (Array.isArray(o) || typeof o === "function" || o instanceof Date) return true;
  }
  return false;
}

function getHandle(h) {
  const fn = handles[h];
  if (typeof fn !== "function") throw new Error(`no function ${h}`);
  return fn;
}

function getPage(id) {
  const p = pages.get(id);
  if (!p) throw new Error(`no page ${id}`);
  return p;
}

// The `this` of a computed or permalink function: 11ty binds it to its
// JavaScript filters and shortcodes (and `tmpl`, its Template).
const FILTERS_THIS = "11ty's filters and shortcodes";

// What 11ty hands a computed or permalink function as `this`
// (`_renderFunction`): `Object.assign({}, javascriptFunctions)`, the
// filters and shortcodes by name, which run Brrrty's own.
// A computed function's also has `tmpl` (`ComputedData.add`), 11ty's
// Template object, which Brrrty has no copy of; a plain permalink
// function's (`renderPermalink`) doesn't.
function filtersThis(computed) {
  const out = filterFunctions();
  if (computed) trapKey(out, "tmpl", "reads `this.tmpl`, 11ty's internal Template object");
  return out;
}

// --- The request loop. ---

async function handle(req) {
  switch (req.op) {
    case "setArg": {
      const arg = fromWire(req.value);
      if (arg && typeof arg === "object" && arg.eleventy) deepFreeze(arg.eleventy);
      argument = arg;
      argumentText = undefined;
      return { ok: null };
    }
    case "load": {
      const state = { changed: false };
      let value;
      try {
        value = await getDataValue(req.path, req.call, state);
      } catch (e) {
        throw placeSyntaxError(e, req.path);
      }
      if (state.changed) {
        return { changedArgument: true };
      }
      const root = rootValue(value);
      if (root !== null && typeof root === "object" && !loadedRoots.has(root)) {
        loadedRoots.set(root, req.path);
        loadRecords.set(root, completeRecord(root, false));
      }
      // A file loaded per template often gives the same data each time:
      // say so instead of sending it again.
      encodedLive = false;
      const text = JSON.stringify(encodeValue(root, register));
      // What the module itself exports, with no getter in it, is the
      // same every time: Brrrty needn't ask again.
      const fixed = !state.called && !encodedLive ? { fixed: true } : {};
      const key = `${req.call ? "call" : "raw"}:${req.path}`;
      if (lastSent.get(key) === text) return { same: true, ...fixed };
      lastSent.set(key, text);
      return { okText: text, ...fixed };
    }
    case "call": {
      const fn = getHandle(req.handle);
      const thisArg = req.opaque !== undefined ? opaqueThis(req.opaque) : fromWire(req.this);
      const args = (req.args || []).map((a) => fromWire(a));
      const r = await runOnce(fn, thisArg, args, req.await ? "await" : "asIs");
      if (r.refused !== undefined) return { refused: r.refused, impure: r.impure, dropShared: r.dropShared };
      return { okText: r.okText };
    }
    case "callsBegin": {
      // The data files are loaded: from here on the clock marks a call.
      clockPhase = "calls";
      clockFeed = null;
      const changed = [];
      let protos = false;
      if (!replaying) {
        for (const [v, file] of loadedRoots) {
          if (!sameList(completeRecord(v, false), loadRecords.get(v))) changed.push(relFile(file));
        }
        protos = !sameList(protosRecord(), startProtos);
        // Freeze where the worker can vouch for every function, and a
        // failed write will be seen; walk otherwise.
        const freezing = changed.length === 0 && !protos && unprovable === null && (await watching);
        if (freezing) {
          quickRecords = new Map([...loadedRoots.keys()].map((v) => [v, quickRecord(v, true)]));
          exportsMode = "frozen";
        }
        for (const v of loadedRoots.keys()) {
          if (freezing && quickRecords.get(v).soft !== true) {
            // Frozen through and through: there is nothing the walk at
            // the end of the build could find changed.
            loadRecords.delete(v);
            continue;
          }
          loadRecords.set(v, completeRecord(v, true));
        }
        if (!freezing) quickRecords = new Map([...loadedRoots.keys()].map((v) => [v, quickRecord(v)]));
      }
      loadHandles = handles.length;
      return { ok: null, handles: handles.length, changed, protos, exports: exportsMode, walks: unprovable };
    }
    case "loadsChanged": {
      // The global data files have loaded: their values as 11ty's pages
      // will see them, where one file changed another's.
      const out = [];
      for (const [v, file] of loadedRoots) {
        const now = completeRecord(v, false);
        if (!sameList(now, loadRecords.get(v))) {
          loadRecords.set(v, now);
          out.push({ path: file, value: v });
        }
      }
      return { okText: JSON.stringify(encodeValue(out, register)) };
    }
    case "exportsFinal": {
      checkWatcher();
      // The watcher must still be there: one more write it has to see.
      if (watcher !== null && again === null && (exportsMode === "frozen" || strictified.size > 0)) {
        const before = Atomics.load(watcher.flags, 0);
        try {
          WATCHED_SAMPLE.a = 2;
        } catch {
          // As it should.
        }
        if (Atomics.load(watcher.flags, 0) !== before + 1) {
          redo("the thread that watches for failed writes stopped");
        } else {
          watcher.seen = before + 1;
          watcher.strictSeen = Atomics.load(watcher.flags, 2);
        }
      }
      if (nodeModule.wrap("X") !== WRAP_SAMPLE && (exportsMode === "frozen" || strictified.size > 0)) {
        redo("the site's code changes how Node wraps CommonJS modules");
      }
      if (again !== null) return { again };
      if (strayFilter !== null) return { refused: strayFilter, impure: true };
      if (exportChange !== null) return { refused: exportChange, impure: true };
      if (quickRecords === null) return { ok: null };
      for (const [v, file] of loadedRoots) {
        if (!loadRecords.has(v)) continue;
        if (!sameList(completeRecord(v, false), loadRecords.get(v))) {
          return {
            refused: `a function from the data changes the data ${relFile(file)} gives (a property it defines, or one with a symbol key), which 11ty's templates then show changed; Brrrty's templates show the data as it loaded, so the pages would differ`,
            impure: true,
          };
        }
      }
      if (!sameList(protosRecord(), startProtos)) {
        return { refused: "a function from the data changes one of JavaScript's built-in prototypes (`Array.prototype` and the like), which 11ty's templates use and Brrrty's don't", impure: true };
      }
      return { ok: null };
    }
    case "describe":
      return { ok: describeFn(getHandle(req.handle)) };
    case "feedCalls": {
      globalFeed = req.values || {};
      return { ok: null };
    }
    case "feedClock": {
      // A replay check's worker: it compares replies; the first run
      // already watched the data files' values.
      replaying = true;
      clockFeed = (req.values || []).slice();
      return { ok: null };
    }
    case "pageOpen": {
      const data = fromWire(req.data);
      for (const [k, why] of req.traps || []) trapKey(data, k, why);
      // The computed names the template's data doesn't have: on a
      // paginated page their values are the values themselves.
      const same = [];
      for (const key of req.defaults || []) {
        const top = key.split(".")[0];
        if (!hasOwn.call(data, top)) same.push(top);
      }
      for (const key of req.defaults || []) {
        if (lodashGet(data, key, UNDEFINED_VALUE) === UNDEFINED_VALUE) {
          if (throughList(data, key)) return { refused: `computed key \`${key}\` would be set inside a list, which Brrrty doesn't do` };
          ownPath(data, key);
          lodashSet(data, key, "");
        }
      }
      // Brrrty numbers the pages, so a replay's are the first run's.
      pages.set(req.page, { data, view: rootView(data, (req.defaults || []).length ? same : null) });
      return { ok: null };
    }
    case "pageVars": {
      const p = getPage(req.page);
      const fn = getHandle(req.handle);
      const proxy = new ComputedDataProxy(req.keys);
      const thisArg = filtersThis(true);
      refusedInCall = null;
      let vars;
      try {
        vars = await proxy.findVarsUsed((d) => fn.call(thisArg, d), p.data);
      } catch (e) {
        const failed = callRefusal(fn);
        if (failed) return failed;
        throw e;
      }
      const failed = callRefusal(fn);
      if (failed) return failed;
      return { ok: vars.map((v) => toWellFormed(String(v))) };
    }
    case "pageCall": {
      const p = getPage(req.page);
      const fn = getHandle(req.handle);
      // A computed value is set where it goes; a plain permalink isn't.
      const r = await runOnce(fn, filtersThis(typeof req.set === "string"), [p.data], "await", p);
      if (r.refused !== undefined) return { refused: r.refused, impure: r.impure, dropShared: r.dropShared };
      if (typeof req.set === "string") {
        if (throughList(p.data, req.set)) return { refused: `computed key \`${req.set}\` would be set inside a list, which Brrrty doesn't do` };
        ownPath(p.data, req.set);
        lodashSet(p.data, req.set, r.value);
      }
      return { okText: r.okText };
    }
    case "pageSet": {
      const p = getPage(req.page);
      if (throughList(p.data, req.key)) return { refused: `computed key \`${req.key}\` would be set inside a list, which Brrrty doesn't do` };
      ownPath(p.data, req.key);
      lodashSet(p.data, req.key, fromWire(req.value));
      return { ok: null };
    }
    case "templateSetup": {
      filterNames = (req.filters || []).map(String);
      return { ok: null };
    }
    case "templateLoad":
      return loadTemplate(req);
    case "preload": {
      // The replay check: run the module code of these templates and data
      // files now, in this order (each handed the clock and random values
      // it read the first time), as 11ty runs each when its file finishes
      // loading. Their loads then take what this made.
      const consumed = [];
      for (const item of req.items || []) {
        const saved = clockFeed;
        clockFeed = Array.isArray(item.feed) ? item.feed.slice() : [];
        const before = clockFeed.length;
        try {
          if (item.kind === "template") {
            try {
              preloaded.set(item.path, { made: await makeTemplate(item.path) });
            } catch (e) {
              preloaded.set(item.path, { error: e });
            }
          } else if (fs.existsSync(item.path)) {
            try {
              await dynamicImportAbsolutePath(item.path, dataType(item.path));
            } catch {
              // Its load reports it.
            }
          }
        } finally {
          consumed.push(before - clockFeed.length);
          clockFeed = saved;
        }
      }
      return { ok: null, consumed };
    }
    case "render": {
      const t = templates[req.template];
      if (!t) throw new Error(`no template ${req.template}`);
      const data = fromWire(req.data);
      for (const [k, why] of req.traps || []) trapKey(data, k, why);
      const first = req.first === undefined ? undefined : fromWire(req.first);
      return renderTemplate(t, data, first);
    }
    case "resetDefs":
      // The replay check: a call changed a kept value, so Brrrty sends
      // every definition again.
      dropPersistent();
      return { ok: null };
    case "reads":
      return { ok: allReads() };
    case "info":
      return { ok: { loadHooks } };
    default:
      return { protocol: `unknown request ${JSON.stringify(req.op)}` };
  }
}

let queue = Promise.resolve();
// How many requests have been read and not yet run.
let queued = 0;

function handleLine(line) {
  queued += 1;
  queue = queue.then(async () => {
    queued -= 1;
    try {
      await handleRequest(line);
    } finally {
      // Whatever this request left unsent (it had no reply, or threw).
      if (queued === 0 && held.length) out("");
    }
  });
}

async function handleRequest(line) {
  {
    let req;
    try {
      req = JSON.parse(line);
    } catch {
      frame("r", null, { protocol: "a request that isn't JSON" });
      return;
    }
    if (req.op === "bye") process.exit(0);
    if (req.op === "def") {
      define(req);
      return;
    }
    if (req.op === "pageClose") {
      // Nothing to say: Brrrty doesn't wait for it.
      pages.delete(req.page);
      return;
    }
    if (again !== null) {
      // Nothing more is of use: Brrrty runs the build again, walking.
      frame("r", req.id, { again }, null);
      return;
    }
    if (req.t !== undefined && req.t === staleTicket) {
      frame("r", req.id, { stale: true }, null);
      return;
    }
    // Site code runs from here on: the watcher is up first, or isn't
    // coming.
    if (req.op === "load" || req.op === "templateLoad" || req.op === "preload") await watching;
    let reply;
    // A call's clock and random reads, recorded, or handed back on a replay.
    const calling = clockPhase === "calls";
    if (calling) {
      callClock = [];
      clockFeed = Array.isArray(req.feed) ? req.feed.slice() : null;
    }
    // A load's own reads, recorded with its reply, and handed back to it
    // alone when the replay check sends them.
    const loadStart = clockRecord.length;
    const savedFeed = clockFeed;
    if (!calling && Array.isArray(req.feed)) clockFeed = req.feed.slice();
    mentionedMade = false;
    try {
      activeRequest = req.id;
      if (defProblem !== null) {
        reply = { protocol: defProblem };
      } else {
        reply = await requestContext.run({ id: req.id, template: req.template }, () => handle(req));
      }
    } catch (e) {
      if (e instanceof Stale && req.t !== undefined) staleTicket = req.t;
      reply = e instanceof Stale
        ? { stale: true }
        : e && e.brrrtyRefusal
        ? { refused: `a function ${e.brrrtyRefusal}` }
        : e && e.brrrtyInvalid
          ? { invalid: toWellFormed(String(e.message)) }
          : { thrown: thrown(e) };
    }
    if (watcher !== null) {
      checkWatcher();
      if (again !== null) reply = { again };
    }
    let clock = null;
    if (calling) {
      clockFeed = null;
      clock = callClock;
      callClock = [];
    } else {
      if (Array.isArray(req.feed)) clockFeed = savedFeed;
      if (clockRecord.length > loadStart && req.op !== "callsBegin") clock = clockRecord.slice(loadStart);
    }
    activeRequest = null;
    frame(mentionedMade ? "f" : "r", req.id, reply, clock);
    // What the call changed, checked once its reply is on its way.
    if (calling && (req.op === "call" || req.op === "pageCall" || req.op === "pageVars")) {
      checkAfterCall(() =>
        typeof req.handle === "number" && typeof handles[req.handle] === "function" ? describeFn(handles[req.handle]) : "a function from the data",
      );
    }
    if (calling && req.op === "render") {
      checkAfterCall(() => (templates[req.template] ? describeRender(templates[req.template].inst) : "a JavaScript template"));
    }
  }
}

// Requests are lines of JSON on stdin. A definition can be many megabytes
// long, so the pieces of a line are kept as they come and joined once.
{
  let pieces = [];
  process.stdin.on("data", (chunk) => {
    let from = 0;
    for (;;) {
      const nl = chunk.indexOf(10, from);
      if (nl === -1) break;
      const tail = chunk.subarray(from, nl);
      const line = pieces.length ? Buffer.concat([...pieces, tail]).toString("utf8") : tail.toString("utf8");
      pieces = [];
      from = nl + 1;
      if (line.length) handleLine(line);
    }
    if (from < chunk.length) pieces.push(from === 0 ? chunk : chunk.subarray(from));
  });
  process.stdin.on("end", () => process.exit(0));
}
