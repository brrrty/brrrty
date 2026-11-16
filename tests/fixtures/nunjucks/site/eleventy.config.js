const { feedPlugin } = require("@11ty/eleventy-plugin-rss");

module.exports = function (eleventyConfig) {
  eleventyConfig.addPlugin(feedPlugin, {
    type: "atom",
    outputPath: "/feed.xml",
    collection: { name: "post", limit: 0 },
    metadata: {
      language: "en",
      title: "Nunjucks Fixture",
      subtitle: "Posts",
      base: "https://example.com/",
      author: { name: "A. Writer" },
    },
  });
  return {
    dir: { input: "src" },
    markdownTemplateEngine: "njk",
    htmlTemplateEngine: "njk",
    dataTemplateEngine: "njk",
  };
};
