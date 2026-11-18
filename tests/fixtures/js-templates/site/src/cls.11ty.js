class Page {
  data() {
    return { title: "Cls", permalink: "/cls-{{ title }}/", tags: "js" };
  }
  helper(x) {
    return "<" + x + ">";
  }
  render(data) {
    return `cls|${this.helper(data.title)}|${data.page.url}|${typeof this.page}|${Object.keys(this).join(",")}`;
  }
}
module.exports = Page;
