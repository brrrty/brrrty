// An object inside an array: 11ty keeps the array, and so this object, as
// it is, hidden key and symbol included.
const el = { shown: 1 };
Object.defineProperty(el, "hidden", { value: 5, enumerable: false });
el[Symbol.toPrimitive] = () => "PRIM";
module.exports = { list: [el] };
