// The benchmark blog's 11ty config. brrrty.config.rhai is its port; keep the
// two in step.
import { feedPlugin } from "@11ty/eleventy-plugin-rss";
import syntaxHighlight from "@11ty/eleventy-plugin-syntaxhighlight";

const MONTHS = [
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
];

const plain = (html) => String(html ?? "").replace(/<[^>]*>/g, " ").replace(/[ \n\t]+/g, " ").trim();

// Pages of `size` posts for each name `keyOf` gives, newest first.
function paged(posts, names, has, size, key) {
  const pages = [];
  for (const name of names) {
    const list = posts.filter((post) => has(post, name));
    const totalPages = Math.ceil(list.length / size);
    for (let i = 0; i < totalPages; i++) {
      pages.push({
        [key]: name,
        pageNumber: i + 1,
        totalPages,
        total: list.length,
        posts: list.slice(i * size, (i + 1) * size),
      });
    }
  }
  return pages;
}

function counted(posts, has, names) {
  return names.map((name) => ({ name, count: posts.filter((post) => has(post, name)).length }));
}

const hasTag = (post, tag) => post.data.tags.includes(tag);
const hasCategory = (post, category) => post.data.category === category;

function tagNames(posts) {
  const names = [];
  for (const post of posts) for (const tag of post.data.tags) if (!names.includes(tag)) names.push(tag);
  return names.sort();
}

function categoryNames(posts) {
  const names = [];
  for (const post of posts) if (!names.includes(post.data.category)) names.push(post.data.category);
  return names.sort();
}

export default function (eleventyConfig) {
  eleventyConfig.addPassthroughCopy("src/images");
  eleventyConfig.addPassthroughCopy("src/css");

  eleventyConfig.addFilter("isoDate", (date) => date.toISOString().slice(0, 10));
  eleventyConfig.addFilter(
    "readableDate",
    (date) => `${MONTHS[date.getUTCMonth()]} ${date.getUTCDate()}, ${date.getUTCFullYear()}`,
  );
  eleventyConfig.addFilter("readingTime", (html) => {
    const words = plain(html).split(" ").filter((w) => w !== "").length;
    return Math.max(1, Math.ceil(words / 200));
  });
  eleventyConfig.addFilter("excerpt", (html, length = 160) => {
    const text = plain(html);
    if (text.length <= length) return text;
    return text.slice(0, length).replace(/ [^ ]*$/, "") + "...";
  });

  eleventyConfig.addCollection("posts", (api) => api.getFilteredByGlob("./src/posts/*.md"));
  eleventyConfig.addCollection("tagPages", (api) => {
    const posts = api.getFilteredByGlob("./src/posts/*.md").reverse();
    return paged(posts, tagNames(posts), hasTag, 10, "tag");
  });
  eleventyConfig.addCollection("categoryPages", (api) => {
    const posts = api.getFilteredByGlob("./src/posts/*.md").reverse();
    return paged(posts, categoryNames(posts), hasCategory, 20, "category");
  });
  eleventyConfig.addCollection("tagList", (api) => {
    const posts = api.getFilteredByGlob("./src/posts/*.md");
    return counted(posts, hasTag, tagNames(posts));
  });
  eleventyConfig.addCollection("categoryList", (api) => {
    const posts = api.getFilteredByGlob("./src/posts/*.md");
    return counted(posts, hasCategory, categoryNames(posts));
  });

  eleventyConfig.addPlugin(syntaxHighlight);
  eleventyConfig.addPlugin(feedPlugin, {
    type: "atom",
    outputPath: "/feed.xml",
    collection: { name: "posts", limit: 20 },
    metadata: {
      language: "en",
      title: "Example Field Notes",
      subtitle: "A synthetic blog for benchmarking static site generators.",
      base: "https://example.com/",
      author: { name: "A. Writer", email: "writer@example.com" },
    },
  });

  return {
    dir: { input: "src", output: "_site" },
  };
}
