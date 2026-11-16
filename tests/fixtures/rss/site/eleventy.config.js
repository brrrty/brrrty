const { feedPlugin } = require("@11ty/eleventy-plugin-rss");

module.exports = function (eleventyConfig) {
  eleventyConfig.addCollection("postsNewest", (api) =>
    api.getFilteredByGlob("./src/posts/**/*.md"),
  );

  eleventyConfig.addPlugin(feedPlugin, {
    type: "atom",
    outputPath: "/atom.xml",
    collection: { name: "postsNewest", limit: 3 },
    metadata: {
      language: "en",
      title: "Example Blog",
      subtitle: "Notes & \"quotes\" <here>",
      base: "https://example.com/blog/",
      icon: "https://example.com/icon.png",
      logo: "https://example.com/logo.png",
      author: { name: "A. Writer", email: "writer@example.com" },
    },
  });
  eleventyConfig.addPlugin(feedPlugin, {
    type: "rss",
    outputPath: "/rss.xml",
    stylesheet: "/feed.xsl",
    collection: { name: "postsNewest", sort: "descending" },
    metadata: {
      title: "Example Blog",
      subtitle: "Every post",
      base: "https://example.com/",
      icon: "https://example.com/icon.png",
      author: { name: "A. Writer" },
    },
  });
  eleventyConfig.addPlugin(feedPlugin, {
    type: "json",
    outputPath: "/feed.json",
    collection: { name: "postsNewest", sort: "ascending", limit: -2 },
    metadata: {
      title: "Example Blog",
      description: "The JSON feed",
      base: "https://example.com",
      author: { name: "A. Writer", email: "writer@example.com" },
    },
  });
  eleventyConfig.addPlugin(feedPlugin, {
    type: "atom",
    inputPath: "feeds/everything.njk",
    outputPath: "/everything.xml",
    script: "/feed.js",
    collection: { name: "all", limit: 5 },
    metadata: { base: "https://example.com/" },
  });

  return {
    dir: {
      input: "src",
    },
  };
};
