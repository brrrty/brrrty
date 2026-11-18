// The port of eleventy.config.js.

config.add_layout_alias("post", "post.njk");
config.add_layout_alias("base", "nested/base.liquid");

#{
    dir: #{
        input: "src",
        includes: "parts",
        layouts: "layouts",
    },
}
