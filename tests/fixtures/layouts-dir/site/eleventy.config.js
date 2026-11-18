module.exports = function (eleventyConfig) {
  eleventyConfig.addLayoutAlias("post", "post.njk");
  eleventyConfig.addLayoutAlias("base", "nested/base.liquid");

  return {
    dir: {
      input: "src",
      includes: "parts",
      layouts: "layouts",
    },
  };
};
