import pluginRss from "@11ty/eleventy-plugin-rss";
import folders from "./config/folders.js";
import * as aliases from "./config/aliases.js";

export default function (eleventyConfig) {
  folders(eleventyConfig);
  for (const [name, file] of Object.entries(aliases)) {
    eleventyConfig.addLayoutAlias(name, file);
  }
  eleventyConfig.addPlugin(pluginRss);
}

export const config = {
  dir: {
    input: "src",
    includes: "parts",
    layouts: "layouts",
  },
};
