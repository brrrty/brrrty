// The 11ty config, loaded against a recording `eleventyConfig`.
//
// The method names and which of them chain follow Eleventy 3.1.8's
// src/UserConfig.js (MIT, see NOTICE): `addPassthroughCopy` and
// `addGlobalData` return the config, every other method returns nothing.
// What Brrrty can apply is recorded; any other method or property write is
// recorded as unsupported with the config file and line, never ignored.

// [kind, engine, async] of each method that takes (name, callback).
const CONFIG_FUNCTIONS = {
  addFilter: ["filter", "all", null],
  addAsyncFilter: ["filter", "all", true],
  addLiquidFilter: ["filter", "liquid", false],
  addNunjucksFilter: ["filter", "nunjucks", null],
  addNunjucksAsyncFilter: ["filter", "nunjucks", true],
  addJavaScriptFilter: ["filter", "javascript", null],
  addShortcode: ["shortcode", "all", null],
  addAsyncShortcode: ["shortcode", "all", true],
  addLiquidShortcode: ["shortcode", "liquid", false],
  addNunjucksShortcode: ["shortcode", "nunjucks", null],
  addNunjucksAsyncShortcode: ["shortcode", "nunjucks", true],
  addJavaScriptShortcode: ["shortcode", "javascript", null],
  addPairedShortcode: ["pairedShortcode", "all", null],
  addPairedAsyncShortcode: ["pairedShortcode", "all", true],
  addPairedLiquidShortcode: ["pairedShortcode", "liquid", false],
  addPairedNunjucksShortcode: ["pairedShortcode", "nunjucks", null],
  addPairedNunjucksAsyncShortcode: ["pairedShortcode", "nunjucks", true],
  addPairedJavaScriptShortcode: ["pairedShortcode", "javascript", null],
  addTransform: ["transform", "all", null],
};

// Directory setters, by the `dir` key they set.
const CONFIG_DIRS = {
  setInputDirectory: "input",
  setOutputDirectory: "output",
  setDataDirectory: "data",
  setIncludesDirectory: "includes",
  setLayoutsDirectory: "layouts",
};

// Properties a config reads that are not methods.
const CONFIG_PROPS = new Set(["dir", "pathPrefix", "logger", "plugins", "events", "directories", "errorReporting", "templateHandling"]);

// Where the config (or a module it calls) called from: the first stack
// frame in a file, as `path:line` from the site's root.
function configPlace() {
  const lines = asWritten(String(new Error().stack)).split("\n").slice(1);
  for (const line of lines) {
    const m = /(?:\(|at )((?:file:\/\/)?\/[^()]*?):(\d+):\d+\)?$/.exec(line);
    if (!m || m[1].endsWith("[eval1]")) continue;
    const file = m[1].startsWith("file://") ? fileURLToPath(m[1]) : m[1];
    return `${relFile(file)}:${m[2]}`;
  }
  return "the config";
}

// The Brrrty-native name of 11ty 3.1.8's built-in plugins (src/Plugins/IdAttributePlugin.js,
// HtmlBasePlugin.js and InputPathToUrl.js), told by function name and the source each has; null for any other.
function nativePlugin(fn) {
  let src;
  try {
    src = Function.prototype.toString.call(fn);
  } catch {
    return null;
  }
  // The posthtml plugin's name is a module-level const, so match what its body has.
  if (fn.name === "IdAttributePlugin" && src.includes("[id],h1,h2,h3,h4,h5,h6") && src.includes("checkDuplicates")) return "idattribute";
  if (fn.name === "TransformPlugin" && src.includes("the `pathToUrl` Transform") && src.includes("eleventy.contentMap")) return "inputpathtourl";
  if (fn.name === "eleventyRenderPlugin" && src.includes("_renderStringShortcodeFn") && src.includes("tagNameFile")) return "render";
  if (fn.name === "eleventyHtmlBasePlugin" && src.includes("addPathPrefixToFullUrl")) return "htmlbase";
  return null;
}

function configRecorder() {
  const values = { passthroughCopy: [], ignores: [] };
  const functions = [];
  const unsupported = [];
  const ignores = new Set();
  const notedAt = (method) => unsupported.push({ method, at: configPlace() });
  const methods = {};
  // 11ty's `eleventyConfig.htmlTransformer` (plugins call `addUrlTransform` and
  // `addPosthtmlPlugin`): not run by Brrrty's JS config, so each call is recorded as
  // unsupported instead of throwing a TypeError at config load.
  const htmlTransformer = new Proxy({}, {
    get(_t, key) {
      if (typeof key === "symbol" || key === "then") return undefined;
      return () => {
        notedAt(`htmlTransformer.${key}`);
      };
    },
  });
  for (const [method, [kind, engine, asyncFlag]] of Object.entries(CONFIG_FUNCTIONS)) {
    methods[method] = (name, fn, flag) => {
      if (typeof fn !== "function") {
        throw new Error(`Unable to add ${kind} "${name}": the callback must be a function (\`${method}\`)`);
      }
      const isAsync = asyncFlag !== null ? asyncFlag : flag === true || util.types.isAsyncFunction(fn);
      functions.push({ kind, name: String(name), engine, async: isAsync, fn });
    };
  }
  // 11ty 3.1.8 `UserConfig.addPlugin`: a plugin (a function, or `{ configFunction }`) is
  // called with the config and its options after the config function returns, in the
  // order added, unless `options.immediate` runs it now.
  const queue = [];
  const runPlugin = (fn, options) => fn(proxy, options);
  methods.addPlugin = (plugin, options = {}) => {
    const fn = typeof plugin === "function" ? plugin : plugin && plugin.configFunction;
    if (typeof fn !== "function") return notedAt("addPlugin");
    // 11ty's own IdAttributePlugin, HtmlBasePlugin, InputPathToUrlTransformPlugin and RenderPlugin, with no options: Brrrty runs them
    // natively, so their `htmlTransformer` calls are not recorded (they would refuse every page).
    const native = nativePlugin(fn);
    if (native && !(options && Object.keys(options).length > 0)) {
      // Decided when plugins run, once `pathPrefix` is known.
      queue.push({ fn, options, native });
      return;
    }
    if (options && options.immediate) return runPlugin(fn, options);
    queue.push({ fn, options });
  };
  methods.addPassthroughCopy = function (input, options = {}) {
    if (typeof input === "string") {
      values.passthroughCopy.push({ input, output: true, options });
    } else {
      for (const [from, to] of Object.entries(input)) values.passthroughCopy.push({ input: from, output: to, options });
    }
    return this;
  };
  methods.addGlobalData = function (name, data) {
    values.globalData = values.globalData || {};
    values.globalData[name] = data;
    return this;
  };
  methods.setTemplateFormats = (formats) => {
    values.templateFormats = formats;
  };
  methods.addTemplateFormats = (formats) => {
    (values.templateFormatsAdded = values.templateFormatsAdded || []).push(formats);
  };
  // 11ty's bundle plugin: only `addBundle(name)`; options are recorded as unsupported.
  methods.addBundle = (name, options) => {
    if (options && Object.keys(options).length > 0) return notedAt("addBundle (with options)");
    (values.bundles = values.bundles || []).push(String(name));
  };
  methods.versionCheck = () => {};
  for (const [method, key] of Object.entries(CONFIG_DIRS)) {
    methods[method] = (dir) => {
      (values.dir = values.dir || {})[key] = dir;
    };
  }
  const target = { ignores, plugins: [], dir: undefined, pathPrefix: undefined };
  const proxy = new Proxy(target, {
    get(t, key) {
      if (typeof key === "symbol" || key === "then") return undefined;
      if (Object.hasOwn(methods, key)) return methods[key].bind(proxy);
      if (key === "ignores") return ignores;
      if (key === "htmlTransformer") return htmlTransformer;
      if (CONFIG_PROPS.has(key)) return t[key];
      return (...rest) => {
        notedAt(key);
        return undefined;
      };
    },
    set(t, key, value) {
      if (!CONFIG_PROPS.has(key)) notedAt(`${String(key)} = `);
      t[key] = value;
      return true;
    },
  });
  return {
    proxy,
    async runPlugins() {
      for (let i = 0; i < queue.length; i++) {
        const { fn, options, native } = queue[i];
        // Native HtmlBase assumes a `/` pathPrefix; with another the real plugin runs, so
        // its `addUrlTransform` call is recorded and the build refuses, as before.
        if (native && (native !== "htmlbase" || proxy.pathPrefix === "/")) {
          (values.nativePlugins = values.nativePlugins || []).push(native);
        } else {
          await runPlugin(fn, options);
        }
      }
    },
    done(returned) {
      values.ignores = [...ignores];
      return { returned, values, functions, unsupported };
    },
  };
}

// 11ty's `Merge(local, exported)`: plain objects recurse, the exported value wins.
function isPlain(v) {
  return v !== null && typeof v === "object" && Object.getPrototypeOf(v) === Object.prototype;
}
function deepMerge(target, source) {
  const out = { ...target };
  for (const [k, v] of Object.entries(source)) {
    out[k] = isPlain(v) && isPlain(out[k]) ? deepMerge(out[k], v) : v;
  }
  return out;
}

async function loadConfig(file) {
  let fn;
  let exported;
  try {
    fn = await dynamicImportAbsolutePath(file, dataType(file));
  } catch (e) {
    throw placeSyntaxError(e, file);
  }
  // 11ty merges an ESM `export const config = {...}` into what the function returns.
  if (fn && typeof fn === "object" && typeof fn.default === "function") {
    exported = fn.config;
    fn = fn.default;
  }
  if (typeof fn !== "function") {
    throw new Error(`The config ${relFile(file)} must export a function, as 11ty's does`);
  }
  const rec = configRecorder();
  let returned = await fn(rec.proxy);
  if (exported && typeof exported === "object") {
    returned = deepMerge(returned && typeof returned === "object" ? returned : {}, exported);
  }
  // 11ty's `processPlugins` runs after the config merged, with `userConfig.pathPrefix` set to
  // the merged value (default "/"); plugins such as HtmlBasePlugin read it.
  rec.proxy.pathPrefix = (returned && returned.pathPrefix) || "/";
  await rec.runPlugins();
  return rec.done(returned);
}
