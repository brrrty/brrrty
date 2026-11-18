module.exports = {
  data: { layoutKey: "fromJsLayout" },
  render(data) {
    return `<jsl ${data.title} ${data.layoutKey}>${data.content}</jsl>`;
  },
};
