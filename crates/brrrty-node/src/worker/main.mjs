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

// Lists kept between requests, by Brrrty's id, and every object and list
// inside them.
// Values (lists and objects) kept between requests, by Brrrty's id.
let persistent = new Map();

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
    const d = Reflect.getOwnPropertyDescriptor(t, k);
    if (d && !d.configurable && "value" in d && !d.writable) return v;
    return watch(v);
  },
  getOwnPropertyDescriptor(t, k) {
    const d = Reflect.getOwnPropertyDescriptor(t, k);
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

// Forget the kept values: one was changed. Brrrty sends them again.
function dropPersistent() {
  persistent = new Map();
}

// --- The clock and random numbers. ---
//
// While data files load, `Date.now()`, `new Date()`, `Date()` and
// `Math.random()` give what they always give, and each value is recorded,
// so a second worker checking the build (see `replay` in the client) can be
// handed the same ones. During a call from a page they mark the call: its
// result would be the moment or the luck of that call, so it is refused,
// every time, rather than only when two runs happen to differ.

const OriginalDate = Date;
const originalNow = Date.now;
const originalRandom = Math.random;
let clockPhase = "load"; // "load" | "calls"
const clockRecord = [];
let clockFeed = null;
let clockInCall = null;

function clockValue(kind, real) {
  if (clockPhase === "calls") {
    if (clockInCall === null) clockInCall = kind;
    return real();
  }
  if (clockFeed !== null && clockFeed.length) return clockFeed.shift();
  const v = real();
  clockRecord.push(v);
  return v;
}

function PatchedDate(...args) {
  const now = () => clockValue("the current time", () => originalNow.call(OriginalDate));
  if (!new.target) return new OriginalDate(now()).toString();
  return Reflect.construct(OriginalDate, args.length ? args : [now()], new.target);
}
Object.setPrototypeOf(PatchedDate, OriginalDate);
Object.defineProperty(PatchedDate, "prototype", { value: OriginalDate.prototype, writable: false });
Object.defineProperty(PatchedDate, "name", { value: "Date" });
Object.defineProperty(PatchedDate, "length", { value: 7 });
PatchedDate.now = function now() {
  return clockValue("the current time", () => originalNow.call(OriginalDate));
};
Object.defineProperty(OriginalDate.prototype, "constructor", { value: PatchedDate, writable: true, configurable: true, enumerable: false });
globalThis.Date = PatchedDate;
Math.random = function random() {
  return clockValue("a random number", () => originalRandom.call(Math));
};

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
  if ("$S" in w) {
    // A value Brrrty sends once and refers to in later requests: data
    // pages share.
    const [id, inner] = w.$S;
    if (persistent.has(id)) return persistent.get(id);
    const v = fromWire(inner, shared);
    persistent.set(id, v);
    return v;
  }
  if ("$R" in w) {
    if (!persistent.has(w.$R)) throw new Error(`no kept value ${w.$R}`);
    return persistent.get(w.$R);
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
async function runOnce(fn, thisArg, args, settle) {
  refusedInCall = null;
  clockInCall = null;
  takeWrote();
  let value;
  try {
    value = fn.apply(watch(thisArg), args.map(watch));
    if (settle === "await") {
      value = await value;
    } else if (value && typeof value.then === "function" && typeof value.catch === "function") {
      // Not awaited, as Nunjucks doesn't: don't let a rejection end Node.
      value.catch(() => {});
    }
  } catch (e) {
    const failed = callRefusal(fn);
    if (failed) return failed;
    throw e;
  }
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
  const clock = clockInCall;
  clockInCall = null;
  if (refusedInCall !== null) return { refused: `${describeFn(fn)} ${refusedInCall}` };
  if (clock !== null) {
    return {
      refused: `${describeFn(fn)} reads ${clock}, so its result is the moment (or the luck) of the call, and depends on when pages are built`,
      impure: true,
    };
  }
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
let nextPage = 0;

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
      const shared = new Map();
      const thisArg = req.opaque !== undefined ? opaqueThis(req.opaque) : fromWire(req.this, shared);
      const args = (req.args || []).map((a) => fromWire(a, shared));
      const r = await runOnce(fn, thisArg, args, req.await ? "await" : "asIs");
      if (r.refused !== undefined) return { refused: r.refused, impure: r.impure, dropShared: r.dropShared };
      return { okText: r.okText };
    }
    case "callsBegin": {
      // The data files are loaded: from here on the clock marks a call.
      clockPhase = "calls";
      clockFeed = null;
      return { ok: { clock: clockRecord, handles: handles.length } };
    }
    case "describe":
      return { ok: describeFn(getHandle(req.handle)) };
    case "feedClock": {
      clockFeed = (req.values || []).slice();
      return { ok: null };
    }
    case "pageOpen": {
      const data = fromWire(req.data);
      for (const [k, why] of req.traps || []) trapKey(data, k, why);
      for (const key of req.defaults || []) {
        if (lodashGet(data, key, UNDEFINED_VALUE) === UNDEFINED_VALUE) {
          if (throughList(data, key)) return { refused: `computed key \`${key}\` would be set inside a list, which Brrrty doesn't do` };
          lodashSet(data, key, "");
        }
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
      clockInCall = null;
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
      const r = await runOnce(fn, opaqueThis(FILTERS_THIS), [p.data], "await");
      if (r.refused !== undefined) return { refused: r.refused, impure: r.impure, dropShared: r.dropShared };
      if (typeof req.set === "string") {
        if (throughList(p.data, req.set)) return { refused: `computed key \`${req.set}\` would be set inside a list, which Brrrty doesn't do` };
        lodashSet(p.data, req.set, r.value);
      }
      return { okText: r.okText };
    }
    case "pageSet": {
      const p = getPage(req.page);
      if (throughList(p.data, req.key)) return { refused: `computed key \`${req.key}\` would be set inside a list, which Brrrty doesn't do` };
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
