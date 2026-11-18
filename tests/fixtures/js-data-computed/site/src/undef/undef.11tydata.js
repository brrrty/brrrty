// A computed permalink function that gives undefined: 11ty falls back to
// `eleventyComputed.permalink` (the function) and then the default.
module.exports = {
  eleventyComputed: {
    permalink: (data) => (data.page.fileSlug === "keep" ? "/kept/" : undefined),
  },
};
