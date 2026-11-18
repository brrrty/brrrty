module.exports = {
  pagination: { data: "items", size: 1, alias: "item" },
  items: ["red", "green"],
  permalink: (data) => `/paged/${data.item}/`,
};
