module.exports = function (c) {
  c.addPassthroughCopy("src/images");
  return { dir: { input: "src" } };
};
