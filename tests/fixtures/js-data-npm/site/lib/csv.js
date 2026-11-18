exports.split = function (text) {
  const [head, ...rows] = text.trim().split("\n").map((l) => l.split(","));
  return rows.map((r) => Object.fromEntries(head.map((h, i) => [h, r[i]])));
};
