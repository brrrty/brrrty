// Changes a.js's value as it loads, through the module cache.
const a = require("./a");
a.list.push(2);
a.list.sort();
a.meta.title = "B";
a.nested.deep.n = 2;
a.added = "new key";
module.exports = { seen: a.list.length };
