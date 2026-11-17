// Brrrty's port of eleventy.config.js. Keep the two in step.

fn link_open(base, tokens, idx, options, env, renderer) {
    let href = tokens[idx].attr_get("href") ?? "";
    if regex_test(href, "(?:^|\\/\\/)(?:go\\.to|(?:www\\.)?shop\\.)", "i") {
        tokens[idx].attr_set("rel", "sponsored nofollow");
        tokens[idx].attr_set("target", "_blank");
    }
    if regex_test(href, "^https?:", "") {
        tokens[idx].attr_join("class", "external");
    }
    if type_of(base) == "()" {
        renderer.render_token(tokens, idx, options)
    } else {
        base.call(tokens, idx, options, env, renderer)
    }
}

fn link_rules(md) {
    md.renderer.rules.link_open = Fn("link_open").curry(md.renderer.rules.link_open);
    md.renderer.rules.link_close = |tokens, idx, options, env, renderer| {
        (if tokens[idx].markup == "autolink" { "<span class=\"auto\"></span>" } else { "" })
            + renderer.render_token(tokens, idx, options)
    };
    md
}

fn lazy_image(image, tokens, idx, options, env, renderer) {
    tokens[idx].attr_set("loading", "lazy");
    image.call(tokens, idx, options, env, renderer)
}

fn kbd_code(code, tokens, idx, options, env, renderer) {
    "<kbd>" + code.call(tokens, idx, options, env, renderer) + "</kbd>"
}

config.amend_library("md", |md| link_rules(md));
config.amend_library("md", |md| {
    md.renderer.rules.image = Fn("lazy_image").curry(md.renderer.rules.image);
    md.renderer.rules.code_inline = Fn("kbd_code").curry(md.renderer.rules.code_inline);
});

#{
    dir: #{
        input: "src",
    },
}
