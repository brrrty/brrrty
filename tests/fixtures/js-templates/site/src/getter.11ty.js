let reads = 0;
module.exports = {
  get data() {
    reads += 1;
    return { title: "Getter", reads };
  },
  render: (data) => `getter|${data.title}|${data.reads}`,
};
