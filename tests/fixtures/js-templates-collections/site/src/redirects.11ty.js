module.exports = {
  data: { permalink: "/_redirects", eleventyExcludeFromCollections: true },
  render: ({ collections }) => collections.job.map((j) => `/j/${j.fileSlug} ${j.url} 301`).join("\n") + "\n",
};
