// brrrty.com, built with Brrrty.
config.add_passthrough_copy("assets");
config.add_passthrough_copy("CNAME");

// The docs sidebar, in the order each page's front matter gives.
config.add_collection("docs", |api|
    api.get_filtered_by_tag("docs").sort(|a, b| a.data.order - b.data.order));

// Markdown pages are plain Markdown: the docs show template syntax in code.
#{ markdownTemplateEngine: false, dir: #{ output: "_site" } }
