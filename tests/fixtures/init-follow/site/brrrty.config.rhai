// Written by `brrrty init` from eleventy.config.js, which Brrrty doesn't run.
// Brrrty reads this file instead; 11ty still reads eleventy.config.js.
// Reference: https://brrrty.com/docs/configuration/
// Ported from: eleventy.config.js sha256:d734bb2ca948c842b512781ea9b1d384d2e1e27c2e7286bb3464f8cc615fec18
// Also ported from: config/folders.js sha256:f8650b3b4855cd2a0a7393f70e8b2a40fb094d6792a0100d00f954e255186727
// Also ported from: config/aliases.js sha256:99151da3df9d5c7dbb1f637b89f07160b58079bbfa80f77780b2a615b4240bfd

config.add_passthrough_copy("src/img");
config.add_passthrough_copy("src/css");
config.add_passthrough_copy(#{ "src/static": "/" });
config.add_layout_alias("base", "base.liquid");
config.add_layout_alias("post", "post.njk");
config.add_plugin("rss", #{ feed: false });

#{
    dir: #{
        input: "src",
        includes: "parts",
        layouts: "layouts",
    },
}
