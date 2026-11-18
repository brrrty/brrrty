module.exports = async function () {
  const a = await Promise.resolve(1);
  const b = await new Promise((resolve) => setTimeout(() => resolve(2), 5));
  return { a, b, sum: a + b };
};
