// A frozen object: 11ty keeps it as it is.
const inner = { shown: 1 };
Object.defineProperty(inner, "hidden", { value: 6, enumerable: false });
module.exports = { frozen: Object.freeze({ inner }) };
