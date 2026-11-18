const rules = [
  ["/old/", "/new/", 301],
  ["/blog/*", "/posts/:splat", 302],
];
module.exports = class {
  data() {
    return { permalink: "/_redirects", eleventyExcludeFromCollections: true };
  }
  render() {
    return rules.map(([from, to, code]) => `${from} ${to} ${code}`).join("\n") + "\n";
  }
};
