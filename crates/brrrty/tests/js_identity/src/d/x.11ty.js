exports.data = { fm: { f: 1 }, eleventyComputed: { c: (d) => [d.site === d.site, d.site.inner === d.site.inner, d.dd === d.dd, d.page === d.page].join(), n: () => ({ fresh: 1 }) } };
exports.render = function (data) {
  const all = data.collections.all; const me = all.find((p) => p.url === data.page.url); const y = all.find((p) => p.url === "/d/y/");
  const r = {
    a1: data.site === data.site, a2: data.site.inner === data.site.inner, a3: data.site.inner.deep === data.site.inner.deep, a4: data.fm === data.fm,
    b1: me.data === data, b2: me.data.site === data.site, b3: me.data.site.inner.deep === data.site.inner.deep, b4: me.data === me.data,
    c1: me.page === data.page, c2: me.page === me.data.page, c3: y.page === y.data.page, c4: data.page === data.page, c5: y.page === y.page,
    d1: y.data.site === data.site, d2: y.data.site === y.data.site, d3: y.data.site.list === data.site.list, d4: y.data.site.list[0] === data.site.list[0], d5: y.data.site.list[0].sub === data.site.list[0].sub, d6: y.data === y.data, d7: y.data.site.inner.deep === data.site.inner.deep,
    e1: data.eleventy === y.data.eleventy, e2: data.pkg === y.data.pkg, e3: data.eleventy.env === y.data.eleventy.env, e4: Object.isFrozen(data.eleventy), e5: Object.isFrozen(data.pkg), e6: Object.isFrozen(data.eleventy.env),
    f3: data.collections.all === data.collections.all, f4: y === data.collections.t.find((p) => p.url === "/d/y/"),
    g1: data.tags === y.data.tags, g2: data.dd === y.data.dd, g3: data.dl === y.data.dl, g4: data.dl[0] === y.data.dl[0],
    h1: data.c, h3: data.n === data.n, h4: me.data.n === data.n,
    this1: this.page === data.page, this2: this.eleventy === data.eleventy,
  };
  return JSON.stringify(r);
};
