module.exports = class {
  data() {
    return { layout: "outer.njk", chainKey: "chained" };
  }
  render({ content, chainKey }) {
    return `<chained ${chainKey}>${content}</chained>`;
  }
};
