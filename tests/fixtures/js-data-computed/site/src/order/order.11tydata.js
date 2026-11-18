// `early` reads `late` only inside tags, which 11ty's Liquid symbol pass
// doesn't see, so on a Liquid page it runs first: it sees the "" 11ty's
// proxy pass sets on computed keys not set yet. Nunjucks sees every name.
module.exports = {
  topfn: (n) => `top ${n}`,
  eleventyComputed: {
    early: "{% if late == '' %}empty{% else %}{% if late %}set{% else %}undefined{% endif %}{% endif %}",
    late: () => "L",
    obj: () => ({ a: 1, inner: { greet: (n) => `hi ${n}` } }),
  },
};
