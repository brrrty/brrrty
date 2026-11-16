// Brrrty's port of eleventy.config.js. Keep the two in step.

let pre = object();
pre.tabindex = 0;
pre["data-note"] = "a & b";
pre["2"] = "two";

config.add_plugin("syntaxhighlight", #{
    pre_attributes: pre,
    code_attributes: #{ class: "custom" },
    always_wrap_line_highlights: true,
    line_separator: "<br>",
    trim: false,
    error_on_invalid_language: false,
    languages: ["yaml", "nope"],
    template_formats: ["md", "liquid"],
});

#{
    dir: #{
        input: "src",
    },
}
