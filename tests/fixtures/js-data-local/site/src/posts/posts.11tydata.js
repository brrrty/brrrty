let counter = 0;
module.exports = function () {
  counter += 1;
  return { layout: "base.njk", tags: ["post"], n: counter };
};
