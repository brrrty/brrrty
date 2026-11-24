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

function configRecorder() {
  const values = { passthroughCopy: [], ignores: [] };
  const functions = [];
  const unsupported = [];
  const ignores = new Set();
  const notedAt = (method) => unsupported.push({ method, at: configPlace() });
  const methods = {};
  for (const [method, [kind, engine, asyncFlag]] of Object.entries(CONFIG_FUNCTIONS)) {
    methods[method] = (name, fn, flag) => {
      if (typeof fn !== "function") {
        throw new Error(`Unable to add ${kind} "${name}": the callback must be a function (\`${method}\`)`);
      }
      const isAsync = asyncFlag !== null ? asyncFlag : flag === true || util.types.isAsyncFunction(fn);
      functions.push({ kind, name: String(name), engine, async: isAsync, fn });
    };
  }
  methods.addPlugin = (plugin, options = {}) => {
    const fn = typeof plugin === "function" ? plugin : plugin && plugin.configFunction;
    if (typeof fn !== "function") return notedAt("addPlugin");
    functions.push({ kind: "plugin", name: fn.name || "", engine: "all", async: false, fn, options });
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
    done(returned) {
      values.ignores = [...ignores];
      return { returned, values, functions, unsupported };
    },
  };
}

async function loadConfig(file) {
  let fn;
  try {
    fn = await dynamicImportAbsolutePath(file, dataType(file));
  } catch (e) {
    throw placeSyntaxError(e, file);
  }
  if (typeof fn !== "function") {
    throw new Error(`The config ${relFile(file)} must export a function, as 11ty's does`);
  }
  const rec = configRecorder();
  const returned = await fn(rec.proxy);
  return rec.done(returned);
}
