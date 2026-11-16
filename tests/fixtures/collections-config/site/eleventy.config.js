module.exports = function (eleventyConfig) {
  eleventyConfig.addCollection("meta", (collectionApi) => {
    const posts = collectionApi.getFilteredByGlob("./src/posts/**/*.md").reverse();
    const collection = { posts: { categories: {}, tags: {} } };
    const categories = new Set();
    const tags = new Set();

    posts.forEach((post) => {
      if (!collection.posts.categories[post.data.category]) {
        collection.posts.categories[post.data.category] = [];
      }

      collection.posts.categories[post.data.category].push(post);
      categories.add(post.data.category);

      if (post.data.tags) {
        post.data.tags.forEach((tag) => {
          if (!collection.posts.tags[tag]) {
            collection.posts.tags[tag] = [];
          }

          tags.add(tag);
          collection.posts.tags[tag].push(post);
        });
      }
    });

    collection.categories = [...categories].sort();
    collection.tags = [...tags].sort();

    return collection;
  });

  eleventyConfig.addCollection("posts", (collectionApi) =>
    collectionApi.getFilteredByGlob("./src/posts/**/*.md").reverse()
  );

  eleventyConfig.addCollection("postsNewest", (collectionApi) =>
    collectionApi.getFilteredByGlob("./src/posts/**/*.md")
  );

  eleventyConfig.addCollection("apiProbe", (api) => {
    const urls = (items) => items.map((i) => i.url);
    return {
      all: urls(api.getAll()),
      sorted: urls(api.getAllSorted()),
      featured: urls(api.getFilteredByTag("featured")),
      both: urls(api.getFilteredByTags("beta", "Alpha")),
      none: urls(api.getFilteredByTags()),
      globs: urls(api.getFilteredByGlob(["src/extra.md", "./src/posts/2024/**/*.liquid"])),
      inputDir: urls(api.getFilteredByGlob("posts/**/*.md")),
      upper: urls(api.getFilteredByGlob("./SRC/Posts/*/*.MD")),
      beta: api.getFilteredByTag("beta").slice(0, 2),
      count: api.getAll().length,
    };
  });

  return {
    dir: {
      input: "src",
    },
  };
};
