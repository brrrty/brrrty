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
import readline from "node:readline";
import { spawnSync } from "node:child_process";
import inspector from "node:inspector";
import { createHash } from "node:crypto";
import { fileURLToPath, pathToFileURL } from "node:url";

// --- Frames, written before anything else can touch stdout. ---

const writeFd = fs.writeSync;
const framesFd = closeOnExec(Number(process.env.BRRRTY_FRAMES_FD) || 1);
delete process.env.BRRRTY_FRAMES_FD;
const sleepCell = new Int32Array(new SharedArrayBuffer(4));

// The replies' pipe comes in without close-on-exec (it had to survive
// Node's own exec), so every process Node starts would inherit it, and one
// left running would hold it open. Open it again, as Node opens every file,
// with close-on-exec, and close the inherited descriptor.
function closeOnExec(fd) {
  if (fd <= 2) return fd;
  for (const dir of ["/proc/self/fd", "/dev/fd"]) {
    let again;
    try {
      again = fs.openSync(`${dir}/${fd}`, fs.constants.O_WRONLY);
    } catch {
      continue;
    }
    fs.closeSync(fd);
    return again;
  }
  return fd;
}

function out(text) {
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

function frame(obj) {
  // A value already encoded as JSON text goes in as it is.
  let json;
  if (obj.okText !== undefined) {
    const { okText, ...rest } = obj;
    const head = JSON.stringify(rest);
    json = `${head.slice(0, -1)}${head.length > 2 ? "," : ""}"ok":${okText}}`;
  } else {
    json = JSON.stringify(obj);
  }
  out(`\x1eBRRRTY ${Buffer.byteLength(json, "utf8")}\n${json}`);
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
if (loadHooks) {
  nodeModule.registerHooks({
    load(url, context, nextLoad) {
      if (url.startsWith("file:")) note(url.split("?")[0]);
      return nextLoad(url, context);
    },
  });
}

function allReads() {
  for (const key of Object.keys(requireHere.cache)) note(key);
  return [...reads].sort();
}

// --- Decoding values Brrrty sends (the same wire forms as encode.mjs). ---

const handles = [];
const handleOf = new Map();

function register(fn) {
  const at = handleOf.get(fn);
  if (at !== undefined) return at;
  handles.push(fn);
  handleOf.set(fn, handles.length - 1);
  return handles.length - 1;
}

// A value from its wire form. `shared` holds the objects sent once and
// referred to again (`$s` and `$r`) within one request.
function fromWire(w, shared = new Map()) {
  if (w === null || typeof w !== "object") return w;
  if (Array.isArray(w)) {
    const outList = new Array(w.length);
    for (let i = 0; i < w.length; i++) {
      if (w[i] && typeof w[i] === "object" && !Array.isArray(w[i]) && "$hole" in w[i]) continue;
      outList[i] = fromWire(w[i], shared);
    }
    return outList;
  }
  if ("$o" in w) {
    const o = {};
    for (const [k, v] of w.$o) {
      if (v && typeof v === "object" && !Array.isArray(v) && "$t" in v) {
        trapKey(o, k, v.$t);
      } else {
        // A key named `__proto__` is an own property, as in parsed JSON.
        Object.defineProperty(o, k, { value: fromWire(v, shared), writable: true, enumerable: true, configurable: true });
      }
    }
    return o;
  }
  if ("$s" in w) {
    const [id, inner] = w.$s;
    const v = fromWire(inner, shared);
    shared.set(id, v);
    return v;
  }
  if ("$r" in w) {
    if (!shared.has(w.$r)) throw new Error(`no shared value ${w.$r}`);
    return shared.get(w.$r);
  }
  if ("$u" in w) return undefined;
  if ("$n" in w) return Number(w.$n);
  if ("$d" in w) return new Date(w.$d);
  if ("$f" in w) {
    if (typeof handles[w.$f] !== "function") throw new Error(`no function ${w.$f}`);
    return handles[w.$f];
  }
  throw new Error(`not a wire form: ${JSON.stringify(w)}`);
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
  if (o && typeof o === "object" && !Object.isFrozen(o)) {
    Object.freeze(o);
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

// `getDataValue` from src/Data/TemplateData.js, for JavaScript files.
// `changed` is set when a data function changed its argument.
async function getDataValue(absolutePath, call, state) {
  if (!fs.existsSync(absolutePath)) return {};
  const ext = path.extname(absolutePath).slice(1);
  let type = "cjs";
  if (ext === "mjs" || (ext === "js" && isEsm)) type = "esm";
  let value = await dynamicImportAbsolutePath(absolutePath, type);
  if (call && typeof value === "function") {
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

// --- Errors. ---

function thrown(e) {
  if (e instanceof Error || (e && typeof e === "object" && typeof e.stack === "string")) {
    return {
      name: toWellFormed(String(e.name || "Error")),
      message: toWellFormed(String(e.message ?? "")),
      stack: toWellFormed(String(e.stack || "")),
      inspect: toWellFormed(String(e.brrrtyPlace || util.inspect(e))),
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
    return `${shown.split(path.sep).join("/")}:${loc.lineNumber + 1}:${loc.columnNumber + 1}`;
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
function describeFn(fn) {
  const name = fn && fn.name ? ` \`${toWellFormed(String(fn.name))}\`` : "";
  const place = fnPlace(fn);
  return `the function${name}${place ? ` (${place})` : ""}`;
}

// One inspector session for `fnKey`, kept open: it uses only the Runtime
// domain, which costs nothing while nothing asks.
let keySession = null;
const fnKeys = new WeakMap();

// A function's identity as code: where it was defined (script, line and
// column), so two closures made by the same code at different calls are
// the same function, and two functions with the same text elsewhere are
// not. Its text where the inspector can't say.
function fnKey(fn) {
  const known = fnKeys.get(fn);
  if (known !== undefined) return known;
  let key;
  try {
    if (!keySession) {
      keySession = new inspector.Session();
      keySession.connect();
    }
    globalThis.__brrrtyFunction = fn;
    let objectId;
    keySession.post("Runtime.evaluate", { expression: "globalThis.__brrrtyFunction", objectGroup: "brrrty" }, (e, r) => {
      objectId = r && r.result && r.result.objectId;
    });
    if (objectId) {
      keySession.post("Runtime.getProperties", { objectId, ownProperties: true }, (e, r) => {
        const p = r && r.internalProperties && r.internalProperties.find((x) => x.name === "[[FunctionLocation]]");
        const loc = p && p.value && p.value.value;
        if (loc) key = `@${loc.scriptId}:${loc.lineNumber}:${loc.columnNumber}`;
      });
    }
    keySession.post("Runtime.releaseObjectGroup", { objectGroup: "brrrty" });
  } catch {
    key = undefined;
  } finally {
    delete globalThis.__brrrtyFunction;
  }
  if (key === undefined) {
    try {
      key = `#${Function.prototype.toString.call(fn)}`;
    } catch {
      key = `h${register(fn)}`;
    }
  }
  fnKeys.set(fn, key);
  return key;
}

// A value as code would see it, to compare two results or two calls'
// arguments: functions by `fnKey`, accessors as such.
function canonical(v) {
  return fingerprint(v, (f) => `f${fnKey(f)}`, false);
}

// A value's state, to tell whether a call changed it: every own property
// (getters by identity, never called), in order, objects seen again by
// their first place. `fnToken` names a function; `accessors` says whether
// a getter's own identity counts.
function fingerprint(v, fnToken = (f) => `f${register(f)}`, accessors = true) {
  const seen = new Map();
  const parts = [];
  const walk = (x) => {
    switch (typeof x) {
      case "undefined":
        parts.push("u");
        return;
      case "string":
        parts.push(JSON.stringify(x));
        return;
      case "number":
        parts.push(Object.is(x, -0) ? "-0" : String(x));
        return;
      case "boolean":
      case "bigint":
        parts.push(`${typeof x}:${String(x)}`);
        return;
      case "symbol":
        parts.push("sym");
        return;
      case "function":
        parts.push(fnToken(x));
        return;
    }
    if (x === null) {
      parts.push("null");
      return;
    }
    if (seen.has(x)) {
      parts.push(`#${seen.get(x)}`);
      return;
    }
    seen.set(x, seen.size);
    if (utilTypes.isProxy(x)) {
      parts.push("proxy");
      return;
    }
    if (x instanceof Date) parts.push(`d${Date.prototype.getTime.call(x)}`);
    parts.push(Array.isArray(x) ? "[" : "{");
    for (const k of Reflect.ownKeys(x)) {
      const d = Object.getOwnPropertyDescriptor(x, k);
      parts.push(typeof k === "symbol" ? "sym" : JSON.stringify(k));
      if (d.get || d.set) {
        parts.push(accessors ? `acc${d.get ? register(d.get) : ""}/${d.set ? register(d.set) : ""}` : "acc");
      } else {
        walk(d.value);
      }
    }
    parts.push("}");
  };
  walk(v);
  return parts.join(",");
}

// Run one call; the reply's `refused` when it can't be used exactly.
async function runCall(fn, makeArgs, settle, label) {
  const { thisArg, args, watched } = makeArgs();
  const before = fingerprint(watched);
  refusedInCall = null;
  let value;
  try {
    value = fn.apply(thisArg, args);
    if (settle === "await") {
      value = await value;
    } else if (value && typeof value.then === "function" && typeof value.catch === "function") {
      // Not awaited, as Nunjucks doesn't: don't let a rejection end Node.
      value.catch(() => {});
    }
  } catch (e) {
    if (refusedInCall !== null) return { refused: refusedInCall };
    throw e;
  }
  if (refusedInCall !== null) return { refused: refusedInCall };
  if (fingerprint(watched) !== before) {
    return { changed: true };
  }
  return { value };
}

// Each call's result by the call: a later call with the same function and
// arguments must give the same result, or the function keeps state and
// its results follow the order Brrrty builds pages in.
const memo = new Map();

function digest(text) {
  return createHash("sha1").update(text).digest("base64");
}

function impure(fn) {
  return {
    refused: `${describeFn(fn)} returns something different when called again with the same data, so its results depend on the order pages are built in, which differs between 11ty and Brrrty`,
    impure: true,
  };
}

// Call twice, compare, and check against earlier calls. `memoKey` is the
// call's arguments as text, or null to skip that check.
async function callTwice(fn, makeArgs, settle, memoKey) {
  const first = await runCall(fn, makeArgs, settle);
  if (first.refused !== undefined) return { refused: `${describeFn(fn)} ${first.refused}` };
  if (first.changed) {
    return { refused: `${describeFn(fn)} changes the data it is given, which 11ty keeps for later pages and templates; Brrrty can't follow that, so return a new value instead` };
  }
  let text;
  try {
    text = JSON.stringify(encodeValue(first.value, register));
  } catch (e) {
    if (e && e.brrrtyRefusal) return { refused: `${describeFn(fn)} returns something that ${e.brrrtyRefusal}` };
    throw e;
  }
  const promise = settle !== "await" && first.value instanceof Promise;
  if (promise) return { okText: text, value: first.value };
  const result = canonical(first.value);
  // A second call, with the same data, must give the same result.
  const second = await runCall(fn, makeArgs, settle);
  if (second.refused !== undefined || second.changed || canonical(second.value) !== result) {
    return impure(fn);
  }
  if (memoKey !== null) {
    const key = digest(`${fnKey(fn)}\0${settle}\0${memoKey}`);
    const got = digest(result);
    const earlier = memo.get(key);
    if (earlier === undefined) memo.set(key, got);
    else if (earlier !== got) return impure(fn);
  }
  return { okText: text, value: first.value };
}

const pages = new Map();
let nextPage = 0;

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
      // A file loaded per template often gives the same data each time:
      // say so instead of sending it again.
      const text = JSON.stringify(encodeValue(rootValue(value), register));
      const key = `${req.call ? "call" : "raw"}:${req.path}`;
      if (lastSent.get(key) === text) return { same: true };
      lastSent.set(key, text);
      return { okText: text };
    }
    case "call": {
      const fn = getHandle(req.handle);
      const settle = req.await ? "await" : "asIs";
      // Fresh copies for each run: a call can't see what an earlier run did
      // to its arguments.
      const makeArgs = () => {
        const shared = new Map();
        const thisArg = req.opaque !== undefined ? opaqueThis(req.opaque) : fromWire(req.this, shared);
        const args = (req.args || []).map((a) => fromWire(a, shared));
        return { thisArg, args, watched: req.opaque !== undefined ? args : [thisArg, args] };
      };
      const { thisArg, args } = makeArgs();
      const memoKey = canonical([req.opaque !== undefined ? `opaque:${req.opaque}` : thisArg, args]);
      const r = await callTwice(fn, makeArgs, settle, memoKey);
      if (r.refused !== undefined) return { refused: r.refused, impure: r.impure };
      return { okText: r.okText };
    }
    case "pageOpen": {
      const data = fromWire(req.data);
      for (const [k, why] of req.traps || []) trapKey(data, k, why);
      for (const key of req.defaults || []) {
        if (lodashGet(data, key, UNDEFINED_VALUE) === UNDEFINED_VALUE) lodashSet(data, key, "");
      }
      nextPage += 1;
      pages.set(nextPage, { data });
      return { ok: nextPage };
    }
    case "pageVars": {
      const p = getPage(req.page);
      const fn = getHandle(req.handle);
      const proxy = new ComputedDataProxy(req.keys);
      const thisArg = opaqueThis(FILTERS_THIS);
      refusedInCall = null;
      let vars;
      try {
        vars = await proxy.findVarsUsed((d) => fn.call(thisArg, d), p.data);
      } catch (e) {
        if (refusedInCall === null) throw e;
      }
      if (refusedInCall !== null) return { refused: `${describeFn(fn)} ${refusedInCall}` };
      return { ok: vars.map((v) => toWellFormed(String(v))) };
    }
    case "pageCall": {
      const p = getPage(req.page);
      const fn = getHandle(req.handle);
      const makeArgs = () => ({ thisArg: opaqueThis(FILTERS_THIS), args: [p.data], watched: p.data });
      const r = await callTwice(fn, makeArgs, "await", null);
      if (r.refused !== undefined) return { refused: r.refused, impure: r.impure };
      if (typeof req.set === "string") lodashSet(p.data, req.set, r.value);
      return { okText: r.okText };
    }
    case "pageSet": {
      const p = getPage(req.page);
      lodashSet(p.data, req.key, fromWire(req.value));
      return { ok: null };
    }
    case "pageClose": {
      pages.delete(req.page);
      return { ok: null };
    }
    case "reads":
      return { ok: allReads() };
    case "info":
      return { ok: { loadHooks } };
    default:
      return { protocol: `unknown request ${JSON.stringify(req.op)}` };
  }
}

let queue = Promise.resolve();
const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
lines.on("line", (line) => {
  queue = queue.then(async () => {
    let req;
    try {
      req = JSON.parse(line);
    } catch {
      frame({ id: null, protocol: "a request that isn't JSON" });
      return;
    }
    if (req.op === "bye") process.exit(0);
    let reply;
    try {
      reply = await handle(req);
    } catch (e) {
      reply = e && e.brrrtyRefusal ? { refused: `a function ${e.brrrtyRefusal}` } : { thrown: thrown(e) };
    }
    frame({ id: req.id, ...reply });
  });
});
lines.on("close", () => process.exit(0));
