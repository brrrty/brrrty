// A permalink function that isn't computed.
module.exports = {
  permalink: (data) => `/plain/${data.page.fileSlug}-${data.kind}/`,
};
