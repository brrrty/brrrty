module.exports = function (arg) {
  return {
    layout: "base.njk",
    section: "pages",
    tags: "page",
    version: arg.eleventy.version,
    nested: { list: [1, 2], when: new Date(Date.UTC(2025, 6, 4)) },
  };
};
