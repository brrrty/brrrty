module.exports = function (arg) {
  const root = arg.eleventy.env.root;
  return {
    json: JSON.stringify(arg)
      .split(root).join("<root>")
      .split(JSON.stringify(arg.eleventy.directories.output)).join("\"<output>\""),
    frozen: Object.isFrozen(arg.eleventy),
    frozenEnv: Object.isFrozen(arg.eleventy.env),
    thisType: typeof this,
    thisIsGlobal: this === globalThis,
    keys: Object.keys(arg),
    envKeys: Object.keys(arg.eleventy.env),
    dirKeys: Object.keys(arg.eleventy.directories),
    serverless: arg.eleventy.env.isServerless,
    serverlessListed: Object.keys(arg.eleventy.env).includes("isServerless"),
  };
};
