module.exports = {
  o: { k: 1 },
  // Which of a collection's items hold the very objects the template handed over.
  across: function (items, site, inner, dd, page) {
    return items
      .map(function (p) {
        return [p.url, p.data.site === site, p.data.site.inner === inner, p.data.site.inner === site.inner, p.data.dd === dd, p.page === page, p.data.page === page, p.data.site.list === site.list].join(":");
      })
      .sort()
      .join(" ");
  },
  own: function () {
    return this.o === this.o;
  },
};
