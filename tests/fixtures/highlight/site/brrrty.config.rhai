// Brrrty's port of eleventy.config.js. Keep the two in step.

config.add_plugin("syntaxhighlight");

config.add_collection("code", |api| api.get_filtered_by_glob("./src/posts/*.md"));

#{
    dir: #{
        input: "src",
    },
}
