module.exports = {
  data: { title: "Obj", eleventyComputed: { shout: "{{ title }}!", whisper: (d) => d.title.toLowerCase() } },
  render(data) {
    return `obj|${data.shout}|${data.whisper}|${this === module.exports}`;
  },
};
