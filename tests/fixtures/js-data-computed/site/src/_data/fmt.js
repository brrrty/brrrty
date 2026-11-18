// Helpers templates call, as methods of a data object.
module.exports = {
  label: "Price",
  money: (n) => "$" + n.toFixed(2),
  upper(s) {
    return String(s).toUpperCase();
  },
  // Liquid reads a function property by calling it, with the object as `this`.
  who() {
    return "who:" + this.label;
  },
  year: () => 2024,
  list: (...items) => items.join("+"),
};
