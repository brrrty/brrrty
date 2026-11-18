const label = await Promise.resolve("tla");
export default class {
  #count = 0;
  data() { return { title: "Private", label }; }
  render(d) { this.#count += 1; return `private|${d.title}|${d.label}|${this.#count}`; }
}
