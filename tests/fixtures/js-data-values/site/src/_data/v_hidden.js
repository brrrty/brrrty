const o = { shown: 1 };
Object.defineProperty(o, "hidden", { value: 2, enumerable: false });
module.exports = o;
