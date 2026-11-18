module.exports = class {
  data() {
    return {
      pagination: { data: "items", size: 1 },
      items: ["x", "y", "z"],
      permalink: (d) => `/paged/${d.pagination.pageNumber}/`,
    };
  }
  render(data) {
    return `paged|${data.pagination.items[0]}|${data.page.url}|this.page.url=${this.page.url}`;
  }
};
