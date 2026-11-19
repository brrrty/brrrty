// Brrrty's JavaScript data worker: the value encoding.
//
// Every value either crosses exactly or is refused with what it is. Wire forms:
//   plain object   {"$o": [[key, value], ...]}  (Object.keys order)
//   undefined      {"$u": 1}
//   array hole     {"$hole": 1}
//   -0, NaN, +-Inf {"$n": "-0" | "NaN" | "Infinity" | "-Infinity"}
//   Date           {"$d": <ms since 1970>}
//   function       {"$f": <handle>}
//   refused        {"$x": "<what it is>"}
// Strings, booleans, null, finite numbers and arrays are themselves.
//
// This file is concatenated ahead of the worker's main module, so it has no
// exports of its own.

import { types as utilTypes } from "node:util";

// Reflection as JavaScript has it, kept from before the worker's main
// module wraps these functions for the site's code (see "Freezing" there).
const ownDescriptor = Object.getOwnPropertyDescriptor;
const ownDescriptors = Object.getOwnPropertyDescriptors;
const reflectOwnDescriptor = Reflect.getOwnPropertyDescriptor;
const objectIsFrozen = Object.isFrozen;
const objectFreeze = Object.freeze;
const defineProperty = Object.defineProperty;
// The objects the worker froze itself, to see what a call changes: to the
// site's code and to 11ty's rules they are as they were.
const frozenByUs = new WeakSet();

// How many objects and arrays deep a value may nest. Brrrty's templates and
// filters walk values recursively, so a deeper one is refused, never a crash.
const MAX_DEPTH = 1000;

const LONE_SURROGATE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;

// `String.prototype.isWellFormed` where Node has it (20 and later).
const wellFormed =
  typeof String.prototype.isWellFormed === "function"
    ? (s) => s.isWellFormed()
    : (s) => !LONE_SURROGATE.test(s);

const LONE_SURROGATES = new RegExp(LONE_SURROGATE.source, "g");

// `s` with each lone surrogate replaced by U+FFFD, so it can cross as
// JSON text Brrrty reads.
function toWellFormed(s) {
  return typeof s.toWellFormed === "function" ? s.toWellFormed() : s.replace(LONE_SURROGATES, "\uFFFD");
}

function article(word) {
  // "a Uint8Array", "a URL", "an Uncommon".
  const vowel = /^[AEIOaeio]/.test(word) || (/^[Uu]/.test(word) && !/^U(int|RL|rl|ni|s)/.test(word));
  return vowel ? "an" : "a";
}

// "a Map object", "a Foo object", "an object with a null-less prototype".
function describeObject(value) {
  let name;
  try {
    const proto = Object.getPrototypeOf(value);
    const ctor = proto && Object.prototype.hasOwnProperty.call(proto, "constructor") ? proto.constructor : undefined;
    name = typeof ctor === "function" && ctor.name ? ctor.name : undefined;
  } catch {
    name = undefined;
  }
  if (!name) {
    return "an object that isn't a plain object";
  }
  return `${article(name)} ${name} object`;
}

// Symbols that change how JavaScript reads an object: printing it, listing
// it, iterating it.
const WELL_KNOWN = new Set(
  Object.getOwnPropertyNames(Symbol)
    .map((name) => Symbol[name])
    .filter((v) => typeof v === "symbol"),
);

// What a template could see on `v` that its enumerable string keys don't
// carry: own non-enumerable string keys (other than an array's indexes and
// `length`) and own well-known symbols. 11ty hands some objects to
// templates as they are, where these stay readable.
function unseen(v, isArray) {
  const symbols = Object.getOwnPropertySymbols(v);
  const names = Object.getOwnPropertyNames(v);
  // The common case, quickly: every string key enumerable (an array's
  // `length` aside) and no symbols.
  if (!symbols.length && names.length === Object.keys(v).length + (isArray ? 1 : 0)) return [];
  const hidden = names.filter((k) => {
    if (isArray && (k === "length" || (String(k >>> 0) === k && (k >>> 0) !== 4294967295))) return false;
    const d = ownDescriptor(v, k);
    return d && !d.enumerable;
  });
  const known = symbols.filter((s) => WELL_KNOWN.has(s)).map((s) => s.description);
  return [...hidden, ...known];
}

function pathText(path) {
  return toWellFormed(pathTextRaw(path));
}

function pathTextRaw(path) {
  let out = "data";
  for (const part of path) {
    out += typeof part === "number" ? `[${part}]` : /^[A-Za-z_$][\w$]*$/.test(part) ? `.${part}` : `[${JSON.stringify(part)}]`;
  }
  return out;
}

// What stands behind a value the worker wrapped to watch a call's writes;
// the worker sets it.
let unwrapHook = (v) => v;

// Encode `value`. `register(fn)` returns a handle for a function value.
function encodeValue(value, register) {
  const stack = [];
  const stackPaths = [];
  const path = [];

  function encodeString(s) {
    return wellFormed(s) ? s : { $x: "a string with a lone surrogate" };
  }

  // `shared`: 11ty hands this value to templates as it is rather than
  // copying its enumerable keys (`getMergedItem` in @11ty/eleventy-utils
  // keeps arrays, and frozen plain objects, by reference).
  function enc(v, shared) {
    switch (typeof v) {
      case "undefined":
        return { $u: 1 };
      case "boolean":
        return v;
      case "string":
        return encodeString(v);
      case "number":
        if (Object.is(v, -0)) return { $n: "-0" };
        if (Number.isNaN(v)) return { $n: "NaN" };
        if (v === Infinity) return { $n: "Infinity" };
        if (v === -Infinity) return { $n: "-Infinity" };
        return v;
      case "bigint":
        return { $x: "a BigInt" };
      case "symbol":
        return { $x: "a Symbol" };
      case "function":
        return { $f: register(v) };
    }
    if (v === null) return null;
    // The worker's own view of data it handed a function is the data.
    v = unwrapHook(v);
    // A proxy can answer reads its own properties don't hold.
    if (utilTypes.isProxy(v)) return { $x: "a Proxy" };
    const at = stack.indexOf(v);
    if (at !== -1) {
      return { $x: `a circular reference to ${pathText(stackPaths[at])}` };
    }
    const proto = Object.getPrototypeOf(v);
    if (proto === Date.prototype) {
      const extra = Object.keys(v);
      if (extra.length) return { $x: `a Date with other properties (${extra.join(", ")})` };
      const ms = Date.prototype.getTime.call(v);
      return Number.isNaN(ms) ? { $x: "an invalid Date" } : { $d: ms };
    }
    if (stack.length >= MAX_DEPTH) {
      return { $x: `a value nested more than ${MAX_DEPTH} objects and arrays deep` };
    }
    if (Array.isArray(v) && proto === Array.prototype) {
      // Templates can read a property set on an array (`list.total`).
      const extra = Object.keys(v).filter((k) => !(String(k >>> 0) === k && (k >>> 0) !== 4294967295));
      if (extra.length) return { $x: `an array with other properties (${extra.join(", ")})` };
      const hidden = unseen(v, true);
      if (hidden.length) return { $x: `an array with properties Brrrty can't list (${hidden.join(", ")}), which 11ty passes to templates as they are` };
      stack.push(v);
      stackPaths.push(path.slice());
      const out = [];
      for (let i = 0; i < v.length; i++) {
        if (!Object.prototype.hasOwnProperty.call(v, i)) {
          out.push({ $hole: 1 });
          continue;
        }
        path.push(i);
        out.push(enc(v[i], true));
        path.pop();
      }
      stack.pop();
      stackPaths.pop();
      return out;
    }
    if (proto === Object.prototype || proto === null) {
      const byReference = shared || (objectIsFrozen(v) && !frozenByUs.has(v));
      if (byReference) {
        const hidden = unseen(v, false);
        if (hidden.length) return { $x: `an object with properties Brrrty can't list (${hidden.join(", ")}), which 11ty passes to templates as they are` };
      }
      stack.push(v);
      stackPaths.push(path.slice());
      const entries = [];
      for (const key of Object.keys(v)) {
        path.push(key);
        let item;
        if (!wellFormed(key)) {
          item = { $x: "a key with a lone surrogate" };
        } else {
          const desc = ownDescriptor(v, key);
          if (desc && desc.get) {
            // 11ty reads a getter as many times as it copies the data, so
            // one that changes between reads has no single value.
            const first = enc(v[key], byReference);
            const second = enc(v[key], byReference);
            item = JSON.stringify(first) === JSON.stringify(second) ? first : { $x: "a getter that returns a different value each time it is read" };
          } else {
            item = enc(desc ? desc.value : v[key], byReference);
          }
        }
        // A key with a lone surrogate is refused, under a name that can
        // cross.
        entries.push([wellFormed(key) ? key : toWellFormed(key), item]);
        path.pop();
      }
      stack.pop();
      stackPaths.pop();
      return { $o: entries };
    }
    return { $x: describeObject(v) };
  }

  return enc(value, false);
}
