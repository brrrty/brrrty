const copies = ["src/img", "src/css"];

export default function (eleventyConfig) {
  copies.forEach((dir) => eleventyConfig.addPassthroughCopy(dir));
  eleventyConfig.addPassthroughCopy({ "src/static": "/" });
}
