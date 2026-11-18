module.exports = function ({ collections }) {
  return collections.featured.map((p) => `${p.data.title}:${p.url}`).join(",");
};
module.exports.data = { date: "2024-01-02", permalink: "/featured/" };
