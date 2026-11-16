// The port of eleventy.config.js.

config.add_collection("postsNewest", |api| api.get_filtered_by_glob("./src/posts/**/*.md"));

config.add_plugin("rss", #{
    type: "atom",
    output_path: "/atom.xml",
    collection: #{ name: "postsNewest", limit: 3 },
    metadata: #{
        language: "en",
        title: "Example Blog",
        subtitle: "Notes & \"quotes\" <here>",
        base: "https://example.com/blog/",
        icon: "https://example.com/icon.png",
        logo: "https://example.com/logo.png",
        author: #{ name: "A. Writer", email: "writer@example.com" },
    },
});
config.add_plugin("rss", #{
    type: "rss",
    output_path: "/rss.xml",
    stylesheet: "/feed.xsl",
    collection: #{ name: "postsNewest", sort: "descending" },
    metadata: #{
        title: "Example Blog",
        subtitle: "Every post",
        base: "https://example.com/",
        icon: "https://example.com/icon.png",
        author: #{ name: "A. Writer" },
    },
});
config.add_plugin("rss", #{
    type: "json",
    output_path: "/feed.json",
    collection: #{ name: "postsNewest", sort: "ascending", limit: -2 },
    metadata: #{
        title: "Example Blog",
        description: "The JSON feed",
        base: "https://example.com",
        author: #{ name: "A. Writer", email: "writer@example.com" },
    },
});
config.add_plugin("rss", #{
    type: "atom",
    input_path: "feeds/everything.njk",
    output_path: "/everything.xml",
    script: "/feed.js",
    collection: #{ name: "all", limit: 5 },
    metadata: #{ base: "https://example.com/" },
});

#{
    dir: #{
        input: "src",
    },
}
