module.exports = {
  async data() {
    await new Promise((r) => setTimeout(r, 5));
    return { title: "Async", permalink: (d) => `/async-${d.title.toLowerCase()}/` };
  },
  async render(data) {
    await null;
    return `async|${data.title}|${data.page.url}`;
  },
};
