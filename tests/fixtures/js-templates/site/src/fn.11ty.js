module.exports = function (data) {
  return [
    "fn",
    typeof this,
    this === module.exports,
    Object.keys(this).join(","),
    this.page && this.page.url,
    this.eleventy && this.eleventy.version,
    typeof this.slugify,
    data.page.url,
    data.page.inputPath,
    data.page.fileSlug,
    data.page.filePathStem,
    data.page.templateSyntax,
    data.page.outputFileExtension,
    Object.keys(data).join(","),
  ].join("|");
};
module.exports.data = { title: "FnTitle", tags: ["js"] };
