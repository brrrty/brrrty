// brrrty.com, built with Brrrty.
config.add_passthrough_copy("assets");
config.add_passthrough_copy("CNAME");

// The docs sidebar, in the order each page's front matter gives.
config.add_collection("docs", |api|
    api.get_filtered_by_tag("docs").sort(|a, b| a.data.order - b.data.order));

// TODO: add config.add_plugin("syntaxhighlight") once Brrrty 0.0.11 is out. In 0.0.10 its
// language scan counts a highlight tag quoted inside inline code in CHANGELOG.md (built
// here through changelog.md) as a language that could change any grammar, and refuses
// every page with a highlighted fence. The fix is in core on another branch.

// Markdown pages are plain Markdown: the docs show template syntax in code.
#{ markdownTemplateEngine: false, dir: #{ output: "_site" } }
