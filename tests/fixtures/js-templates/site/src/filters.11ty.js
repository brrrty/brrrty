module.exports = class {
  data() {
    return { title: "Hello World & Friends", pagination: { data: "names", size: 1 }, names: ["Ünïcode Name", "plain"], permalink: (d) => `/filters/${d.pagination.pageNumber}/` };
  }
  render({ title, pagination }) {
    return [
      this.slugify(title),
      this.slug(pagination.items[0]),
      this.url("/a/b/"),
      this.url("https://example.com/x"),
      this.log(`logged ${pagination.pageNumber}`),
    ].join("|");
  }
};
