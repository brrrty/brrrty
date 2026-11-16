const syntaxHighlight = require("@11ty/eleventy-plugin-syntaxhighlight");

module.exports = function (eleventyConfig) {
  eleventyConfig.addPlugin(syntaxHighlight, {
    preAttributes: { tabindex: 0, "data-note": "a & b", 2: "two" },
    codeAttributes: { class: "custom" },
    alwaysWrapLineHighlights: true,
    lineSeparator: "<br>",
    trim: false,
    errorOnInvalidLanguage: false,
    languages: ["yaml", "nope"],
    templateFormats: ["md", "liquid"],
  });

  return {
    dir: {
      input: "src",
    },
  };
};
