module.exports = function (eleventyConfig) {
  eleventyConfig.addPassthroughCopy("src/notes/");
  eleventyConfig.addPassthroughCopy({ "src/assets/logo.svg": "logo.svg" });
  eleventyConfig.addPassthroughCopy({ "static": "/" });
  eleventyConfig.ignores.add("src/notes/**/*.md");

  eleventyConfig.addGlobalData("build", {
    date: new Date().toUTCString(),
    year: new Date().getFullYear(),
    stamp: Date.now(),
  });
  eleventyConfig.addGlobalData("site", { extra: "from config", title: "Config title" });
  eleventyConfig.addGlobalData("nested.key", "dotted");

  return {
    dir: {
      input: "src",
    },
  };
};
