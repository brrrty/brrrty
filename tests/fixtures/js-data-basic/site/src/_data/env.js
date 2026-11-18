const root = process.env.ELEVENTY_ROOT;
module.exports = {
  root: root === process.cwd() ? "<root>" : "other: " + root,
  source: process.env.ELEVENTY_SOURCE,
  runMode: process.env.ELEVENTY_RUN_MODE,
  version: process.env.ELEVENTY_VERSION,
};
