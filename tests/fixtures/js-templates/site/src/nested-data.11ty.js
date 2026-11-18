module.exports = {
  data: { title: "Nested", list: [3, 1, 2], meta: { a: 1, b: [true, null] } },
  render({ title, list, meta }) {
    return `${title}|${[...list].sort().join(",")}|${JSON.stringify(meta)}`;
  },
};
