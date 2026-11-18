const f = (n) => n; const g = () => 1;
module.exports = { f, g, list: [f, g], one: [f], obj: { a: f }, mixed: ["b", f, "a"] };
