const syntaxHighlight = require("@11ty/eleventy-plugin-syntaxhighlight");

module.exports = function (eleventyConfig) {
  eleventyConfig.addPlugin(syntaxHighlight);

  eleventyConfig.addCollection("code", (api) =>
    api.getFilteredByGlob("./src/posts/*.md"),
  );

  return {
    dir: {
      input: "src",
    },
  };
};
