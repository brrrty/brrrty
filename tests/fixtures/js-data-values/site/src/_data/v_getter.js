let reads = 0;
module.exports = {
  get value() {
    reads += 1;
    return "read " + reads;
  },
};
