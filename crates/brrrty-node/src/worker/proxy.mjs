// The keys a computed data function reads, and lodash's get and set as 11ty
// uses them on page data.
//
// `ComputedDataProxy` is ported from Eleventy 3.1.8
// (src/Data/ComputedDataProxy.js), `isPlainObject` from @11ty/eleventy-utils
// and `get`/`set` from @11ty/lodash-custom (lodash 4), all MIT licensed;
// see NOTICE. Computed data keys are dotted paths of plain names (Brrrty
// refuses `[`, `]` and empty names), so paths are split on dots.

const UNDEFINED_VALUE = "__11TY_UNDEFINED__";

function isPlainObject11ty(value) {
  if (value === null || typeof value !== "object") return false;
  const proto = Object.getPrototypeOf(value);
  return !proto || proto === Object.prototype;
}

function lodashIsObject(value) {
  const type = typeof value;
  return value != null && (type === "object" || type === "function");
}

function lodashIsIndex(value) {
  return /^(?:0|[1-9]\d*)$/.test(value) && Number(value) < Number.MAX_SAFE_INTEGER;
}

function splitPath(key) {
  return typeof key === "string" ? key.split(".") : key;
}

// `_.get(object, path, defaultValue)`.
function lodashGet(object, key, defaultValue) {
  const path = splitPath(key);
  let o = object;
  let i = 0;
  while (o != null && i < path.length) {
    o = o[path[i++]];
  }
  const result = i && i === path.length ? o : undefined;
  return result === undefined ? defaultValue : result;
}

// `_.set(object, path, value)`.
function lodashSet(object, key, value) {
  if (!lodashIsObject(object)) return object;
  const path = splitPath(key);
  let nested = object;
  for (let i = 0; nested != null && i < path.length; i++) {
    const k = path[i];
    if (k === "__proto__" || k === "constructor" || k === "prototype") return object;
    let newValue = value;
    if (i !== path.length - 1) {
      const objValue = nested[k];
      newValue = lodashIsObject(objValue) ? objValue : lodashIsIndex(path[i + 1]) ? [] : {};
    }
    // `assignValue`: skip only an equal own value that is already there.
    const objValue = nested[k];
    const same = objValue === newValue || (objValue !== objValue && newValue !== newValue);
    if (!(Object.prototype.hasOwnProperty.call(nested, k) && same) || (newValue === undefined && !(k in nested))) {
      nested[k] = newValue;
    }
    nested = nested[k];
  }
  return object;
}

class ComputedDataProxy {
  constructor(computedKeys) {
    if (Array.isArray(computedKeys)) {
      this.computedKeys = new Set(computedKeys);
    } else {
      this.computedKeys = computedKeys;
    }
  }

  isArrayOrPlainObject(data) {
    return Array.isArray(data) || isPlainObject11ty(data);
  }

  getProxyData(data, keyRef) {
    // WARNING: SIDE EFFECTS (as in 11ty): keys not set yet become "".
    if (this.computedKeys) {
      for (const key of this.computedKeys) {
        if (lodashGet(data, key, UNDEFINED_VALUE) === UNDEFINED_VALUE) {
          lodashSet(data, key, "");
        }
      }
    }
    return this._getProxyData(data, keyRef);
  }

  _getProxyForObject(dataObj, keyRef, parentKey = "") {
    return new Proxy(
      {},
      {
        get: (obj, key) => {
          if (typeof key !== "string") {
            return obj[key];
          }
          const newKey = `${parentKey ? `${parentKey}.` : ""}${key}`;
          if (newKey === "collections") {
            keyRef.add(newKey);
            return new Proxy(
              {},
              {
                get: (target, k) => {
                  if (typeof k === "string") {
                    keyRef.add(`collections.${k}`);
                    return [];
                  }
                  return target[k];
                },
              },
            );
          }
          const newData = this._getProxyData(dataObj[key], keyRef, newKey);
          if (!this.isArrayOrPlainObject(newData)) {
            keyRef.add(newKey);
          }
          return newData;
        },
      },
    );
  }

  _getProxyForArray(dataArr, keyRef, parentKey = "") {
    return new Proxy(new Array(dataArr.length), {
      get: (obj, key) => {
        if (Array.prototype.hasOwnProperty(key)) {
          keyRef.add(parentKey);
          return obj[key];
        }
        if (key === "then") {
          keyRef.add(parentKey);
          return;
        }
        const newKey = `${parentKey}[${key}]`;
        const newData = this._getProxyData(dataArr[key], keyRef, newKey);
        if (!this.isArrayOrPlainObject(newData)) {
          keyRef.add(newKey);
        }
        return newData;
      },
    });
  }

  _getProxyData(data, keyRef, parentKey = "") {
    if (isPlainObject11ty(data)) {
      return this._getProxyForObject(data, keyRef, parentKey);
    } else if (Array.isArray(data)) {
      return this._getProxyForArray(data, keyRef, parentKey);
    }
    return data;
  }

  async findVarsUsed(fn, data = {}) {
    const keyRef = new Set();
    const proxyData = this.getProxyData(data, keyRef);
    await fn(proxyData);
    return Array.from(keyRef);
  }
}
