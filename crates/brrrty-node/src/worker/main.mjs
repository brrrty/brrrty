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
// Values (lists and objects) kept between requests, by Brrrty's id, and
// every list and object inside them: pages share them.
let persistent = new Map();
let keptObjects = new WeakSet();

function markKept(v) {
  const stack = [v];
  while (stack.length) {
    let x = stack.pop();
    if (x !== null && typeof x === "object" && guardTarget.has(x)) x = guardTarget.get(x);
    if (x === null || typeof x !== "object" || keptObjects.has(x)) continue;
    keptObjects.add(x);
    for (const k of Object.keys(x)) {
      const d = Object.getOwnPropertyDescriptor(x, k);
      if (d && "value" in d) stack.push(d.value);
    }
  }
}

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
      Object.defineProperties(copy, Object.getOwnPropertyDescriptors(next));
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
  keptObjects = new WeakSet();
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
    const d = Object.getOwnPropertyDescriptor(obj, name);
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
  const formatGetter = Object.getOwnPropertyDescriptor(proto, "format").get;
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
      const d = Reflect.getOwnPropertyDescriptor(p, k);
      out.push(k, d.value, d.get, d.set, d.enumerable, d.writable);
    }
    out.push(Object.getPrototypeOf(p));
  }
  return out;
}
const startProtos = protosRecord();

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
      const d = Reflect.getOwnPropertyDescriptor(x, k);
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

// The quick record of `v`, as `quickSame` checks it.
function quickRecord(v) {
  const out = [];
  const seen = new Set();
  const stack = [v];
  while (stack.length) {
    const x = stack.pop();
    if (x === null || typeof x !== "object" || seen.has(x)) continue;
    seen.add(x);
    watchedObjects.add(x);
    const proto = Object.getPrototypeOf(x);
    if (Array.isArray(x)) {
      out.push(x, proto, 0, x.slice(), Object.keys(x).length);
      for (let i = 0; i < x.length; i++) stack.push(x[i]);
      continue;
    }
    if (x instanceof OriginalDate) {
      out.push(x, proto, 1, OriginalDate.prototype.getTime.call(x), 0);
      continue;
    }
    const keys = Object.keys(x);
    const vals = new Array(keys.length);
    const getters = [];
    for (let i = 0; i < keys.length; i++) {
      const d = Object.getOwnPropertyDescriptor(x, keys[i]);
      if (d && !("value" in d)) {
        // What a getter gives is compared by its shape: it may build it anew.
        getters.push(i);
        vals[i] = completeRecord(readGetter(x, keys[i]), false, true);
      } else {
        vals[i] = d ? d.value : undefined;
        stack.push(vals[i]);
      }
    }
    out.push(x, proto, 2, [keys, vals, getters], keys.length);
  }
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
  const d = Object.getOwnPropertyDescriptor(x, k);
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
  if (!sameList(protosRecord(), startProtos)) {
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
]) {
  const original = obj[name];
  setFn(obj, name, function (target, ...rest) {
    if (target !== null && (typeof target === "object" || typeof target === "function") && watchedObjects.has(target)) {
      definedOnWatched = true;
    }
    return original.call(this, target, ...rest);
  });
}

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
    return w.$c ? guarded(o, w.$c) : o;
  }
  if ("$S" in w) {
    // A value Brrrty sends once and refers to in later requests: data
    // pages share.
    const [id, inner] = w.$S;
    if (persistent.has(id)) return persistent.get(id);
    const v = fromWire(inner, shared);
    markKept(v);
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
  const shown = relFile(req.path);
  let mod;
  try {
    mod = await importTemplate(req.path);
  } catch (e) {
    throw placeSyntaxError(e, req.path);
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
  const root = Object.assign({}, data);
  loadedRoots.set(root, req.path);
  loadRecords.set(root, completeRecord(root, false));
  templates.push({ inst, file: req.path, layout: req.layout === true, pageSet: false });
  return { template: templates.length - 1, okText: JSON.stringify(encodeValue(root, register)) };
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

// Ask Brrrty to run one of its filters, as 11ty's own would run in
// JavaScript, and wait for the result.
function askFilter(name, args) {
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
  frame({ filter: name, okText: text });
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
  const wdata = watch(data);
  let value;
  try {
    if (t.layout) {
      if (!t.pageSet) {
        t.pageSet = true;
        if (!inst.page) trapKey(inst, "page", LAYOUT_PAGE);
      }
    } else if (!inst.page) {
      const page = first !== undefined ? watch(first) : wdata.page;
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
    const failed = renderRefusal(inst);
    if (failed) return failed;
    throw e;
  }
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
      const root = rootValue(value);
      if (root !== null && typeof root === "object" && !loadedRoots.has(root)) {
        loadedRoots.set(root, req.path);
        loadRecords.set(root, completeRecord(root, false));
      }
      // A file loaded per template often gives the same data each time:
      // say so instead of sending it again.
      const text = JSON.stringify(encodeValue(root, register));
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
      const changed = [];
      let protos = false;
      if (!replaying) {
        for (const [v, file] of loadedRoots) {
          if (!sameList(completeRecord(v, false), loadRecords.get(v))) changed.push(relFile(file));
        }
        protos = !sameList(protosRecord(), startProtos);
        for (const v of loadedRoots.keys()) {
          loadRecords.set(v, completeRecord(v, true));
        }
        quickRecords = new Map([...loadedRoots.keys()].map((v) => [v, quickRecord(v)]));
      }
      return { ok: { clock: clockRecord, handles: handles.length, changed, protos } };
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
      if (exportChange !== null) return { refused: exportChange, impure: true };
      if (quickRecords === null) return { ok: null };
      for (const [v, file] of loadedRoots) {
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
      for (const key of req.defaults || []) {
        if (lodashGet(data, key, UNDEFINED_VALUE) === UNDEFINED_VALUE) {
          if (throughList(data, key)) return { refused: `computed key \`${key}\` would be set inside a list, which Brrrty doesn't do` };
          ownPath(data, key);
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
    case "pageClose": {
      pages.delete(req.page);
      return { ok: null };
    }
    case "templateSetup": {
      filterNames = (req.filters || []).map(String);
      return { ok: null };
    }
    case "templateLoad":
      return loadTemplate(req);
    case "render": {
      const t = templates[req.template];
      if (!t) throw new Error(`no template ${req.template}`);
      const shared = new Map();
      const data = fromWire(req.data, shared);
      for (const [k, why] of req.traps || []) trapKey(data, k, why);
      const first = req.first === undefined ? undefined : fromWire(req.first, shared);
      return renderTemplate(t, data, first);
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
    // A call's clock and random reads, recorded, or handed back on a replay.
    const calling = clockPhase === "calls";
    if (calling) {
      callClock = [];
      clockFeed = Array.isArray(req.feed) ? req.feed.slice() : null;
    }
    try {
      reply = await handle(req);
    } catch (e) {
      reply = e && e.brrrtyRefusal
        ? { refused: `a function ${e.brrrtyRefusal}` }
        : e && e.brrrtyInvalid
          ? { invalid: toWellFormed(String(e.message)) }
          : { thrown: thrown(e) };
    }
    if (calling) {
      clockFeed = null;
      if (callClock.length) reply = { ...reply, clock: callClock };
      callClock = [];
    }
    frame({ id: req.id, ...reply });
    // What the call changed, checked once its reply is on its way.
    if (calling && (req.op === "call" || req.op === "pageCall" || req.op === "pageVars")) {
      checkAfterCall(() =>
        typeof req.handle === "number" && typeof handles[req.handle] === "function" ? describeFn(handles[req.handle]) : "a function from the data",
      );
    }
    if (calling && req.op === "render") {
      checkAfterCall(() => (templates[req.template] ? describeRender(templates[req.template].inst) : "a JavaScript template"));
    }
  });
});
lines.on("close", () => process.exit(0));
