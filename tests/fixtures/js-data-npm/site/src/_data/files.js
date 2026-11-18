const fs = require("fs");
const path = require("path");
const { split } = require("../../lib/csv.js");

const text = fs.readFileSync(path.join(__dirname, "..", "..", "lib", "rows.csv"), "utf8");
module.exports = split(text);
