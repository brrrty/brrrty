exports.data = { tags: "pg", fm: { f: 1 }, pagination: { data: "site.list", size: 1, alias: "it", addAllPagesToCollections: true }, permalink: (d) => `/pg/${d.pagination.pageNumber}/`,
  eleventyComputed: { c: (d) => [d.site === d.site, d.it === d.it, d.page === d.page, d.pagination === d.pagination].join(), fm2: () => ({ z: 1 }) } };
exports.render = function (data) {
  const n = data.pagination.pageNumber; const col = data.collections.pg; const me = col[n]; const other = col[1 - n];
  return JSON.stringify({
    a1: data.site === data.site, a2: data.site.inner === data.site.inner, a3: data.site.list === data.site.list, a5: data.fm2 === data.fm2,
    p1: data.page === data.page, p2: data.pagination === data.pagination, p3: data.pagination.items === data.pagination.items, p4: data.pagination.page === data.pagination.page, p5: data.pagination.href === data.pagination.href, p6: data.pagination.pages === data.pagination.pages,
    i1: data.it === data.it, i2: data.it === data.pagination.items[0], i3: data.it === data.site.list[n], i4: data.pagination.items[0] === data.site.list[n], i5: data.pagination.pages[n] === data.it,
    b1: me.data === data, b2: me.data === me.data, b3: me.data.site === me.data.site, b4: me.page === data.page, b5: me.page === me.page, b6: me.page === me.data.page,
    o1: other.data === other.data, o2: other.data.site === other.data.site, o3: other.data.it === other.data.it, o4: other.data.site === data.site, o5: other.data.pagination.pages === data.pagination.pages, o6: other.data.pagination === other.data.pagination,
    c: data.c, e1: data.eleventy === data.eleventy, e2: data.pkg === data.pkg, f3: data.collections === data.collections,
  });
};
