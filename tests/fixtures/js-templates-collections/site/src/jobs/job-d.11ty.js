module.exports = {
  data: { title: "Carpenter", tags: ["job", "featured"], date: "2024-03-04" },
  render: ({ title, page }) => `<p>${title} at ${page.url}</p>`,
};
