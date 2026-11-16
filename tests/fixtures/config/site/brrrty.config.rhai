config.add_passthrough_copy("src/notes/");
config.add_passthrough_copy(#{ "src/assets/logo.svg": "logo.svg" });
config.add_passthrough_copy(#{ "static": "/" });
config.ignores.add("src/notes/**/*.md");

let d = now();
config.add_global_data("build", #{
    date: date_to_utc_string(d),
    year: year(d),
    stamp: get_time(d),
});
config.add_global_data("site", #{ extra: "from config", title: "Config title" });
config.add_global_data("nested.key", "dotted");

#{
    dir: #{
        input: "src",
    },
}
