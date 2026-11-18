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
// imports or exports of its own.

const LONE_SURROGATE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;

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

function pathText(path) {
  let out = "data";
  for (const part of path) {
    out += typeof part === "number" ? `[${part}]` : /^[A-Za-z_$][\w$]*$/.test(part) ? `.${part}` : `[${JSON.stringify(part)}]`;
  }
  return out;
}

// Encode `value`. `register(fn)` returns a handle for a function value.
function encodeValue(value, register) {
  const stack = [];
  const stackPaths = [];
  const path = [];

  function encodeString(s) {
    return LONE_SURROGATE.test(s) ? { $x: "a string with a lone surrogate" } : s;
  }

  function enc(v) {
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
    const at = stack.indexOf(v);
    if (at !== -1) {
      return { $x: `a circular reference to ${pathText(stackPaths[at])}` };
    }
    const proto = Object.getPrototypeOf(v);
    if (proto === Date.prototype) {
      const ms = Date.prototype.getTime.call(v);
      return Number.isNaN(ms) ? { $x: "an invalid Date" } : { $d: ms };
    }
    if (Array.isArray(v) && proto === Array.prototype) {
      stack.push(v);
      stackPaths.push(path.slice());
      const out = [];
      for (let i = 0; i < v.length; i++) {
        if (!Object.prototype.hasOwnProperty.call(v, i)) {
          out.push({ $hole: 1 });
          continue;
        }
        path.push(i);
        out.push(enc(v[i]));
        path.pop();
      }
      stack.pop();
      stackPaths.pop();
      return out;
    }
    if (proto === Object.prototype || proto === null) {
      stack.push(v);
      stackPaths.push(path.slice());
      const entries = [];
      for (const key of Object.keys(v)) {
        path.push(key);
        let item;
        if (LONE_SURROGATE.test(key)) {
          item = { $x: "a key with a lone surrogate" };
        } else {
          const desc = Object.getOwnPropertyDescriptor(v, key);
          if (desc && desc.get) {
            // 11ty reads a getter as many times as it copies the data, so
            // one that changes between reads has no single value.
            const first = enc(v[key]);
            const second = enc(v[key]);
            item = JSON.stringify(first) === JSON.stringify(second) ? first : { $x: "a getter that returns a different value each time it is read" };
          } else {
            item = enc(desc ? desc.value : v[key]);
          }
        }
        entries.push([key, item]);
        path.pop();
      }
      stack.pop();
      stackPaths.pop();
      return { $o: entries };
    }
    return { $x: describeObject(v) };
  }

  return enc(value);
}
