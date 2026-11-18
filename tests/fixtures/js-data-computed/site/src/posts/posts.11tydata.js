// Computed values as functions, listed out of their dependency order.
module.exports = {
  layout: "post.njk",
  eleventyComputed: {
    c: (data) => data.b + "!",
    b: (data) => data.a + "-b",
    a: (data) => data.title.toUpperCase(),
    heading: async (data) => `${data.c} (${data.page.fileSlug})`,
    nested: {
      len: (data) => data.c.length,
      text: "{{ c }} and {{ a }}",
    },
    canonical: (data) => "https://example.com" + data.page.url,
    label: (data) => data.i18n.en.homeAriaLabel(data.title),
    permalink: (data) => `/posts/${data.a.toLowerCase()}/`,
  },
};
