// The port of eleventy.config.js.

config.add_plugin("rss", #{
    type: "atom",
    output_path: "/feed.xml",
    collection: #{ name: "post", limit: 0 },
    metadata: #{
        language: "en",
        title: "Nunjucks Fixture",
        subtitle: "Posts",
        base: "https://example.com/",
        author: #{ name: "A. Writer" },
    },
});

#{
    dir: #{ input: "src" },
    markdownTemplateEngine: "njk",
    htmlTemplateEngine: "njk",
}
