// A plain object inside a plain object: 11ty copies its enumerable keys,
// so its hidden key, symbol and getter's later reads don't reach templates.
const child = { shown: 1, get g() { return "g"; } };
Object.defineProperty(child, "hidden", { value: 2, enumerable: false });
child[Symbol.toPrimitive] = () => "PRIM";
module.exports = { child };
