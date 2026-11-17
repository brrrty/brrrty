// The Rhai side of the renderer-rules differential: the rules of
// crates/brrrty-markdown/tools/markdown-rules.cjs, written as a port would write them.

fn is_unset(f) { type_of(f) == "()" }

fn tagged(href) {
    regex_test(href, "(?:^|\\/\\/)(?:go\\.to|(?:www\\.)?shop\\.)", "i")
}

fn link_open(base, tokens, idx, options, env, renderer) {
    let token = tokens[idx];
    let href = token.attr_get("href") ?? "";
    if tagged(href) {
        token.attr_set("rel", "sponsored nofollow");
        token.attr_set("target", "_blank");
    }
    if href.starts_with("/") {
        token.attr_set("href", "https://example.org" + href);
    }
    let next = tokens[idx + 1];
    if token.attr_index("title") < 0 && !is_unset(next) && next.type == "text" {
        token.attr_push(["title", next.content]);
    }
    token.attr_join("class", "l" + token.level);
    token.attr_join("class", if token.markup == "" { "plain" } else { token.markup });
    if is_unset(base) {
        renderer.render_token(tokens, idx, options)
    } else {
        base.call(tokens, idx, options, env, renderer)
    }
}

fn link_close(tokens, idx, options, env, renderer) {
    renderer.render_token(tokens, idx, options)
        + if tokens[idx].info == "auto" { "<span class=\"auto\"></span>" } else { "" }
}

fn image(base, tokens, idx, options, env, renderer) {
    tokens[idx].attr_set("loading", "lazy");
    "<span class=\"img\" data-label=\"" + escape_html(tokens[idx].content) + "\">"
        + base.call(tokens, idx, options, env, renderer) + "</span>"
}

fn code_inline(base, tokens, idx, options, env, renderer) {
    tokens[idx].attr_join("class", "a");
    tokens[idx].attr_join("class", "b\"&");
    base.call(tokens, idx, options, env, renderer)
}

fn text(base, tokens, idx, options, env, renderer) {
    let t = tokens[idx];
    let prefix = if t.info != "" { "[" + t.info + ":" + escape_html(t.markup) + "]" } else { "" };
    prefix + base.call(tokens, idx, options, env, renderer)
}

fn strong_open(tokens, idx, options, env, renderer) {
    tokens[idx].attr_set("data-level", `${tokens[idx].level}`);
    renderer.render_token(tokens, idx, options)
}

fn s_close(tokens, idx, options, env, renderer) {
    tokens[idx].tag = "del";
    renderer.render_token(tokens, idx, options)
}

fn install(md) {
    let rules = md.renderer.rules;
    rules.link_open = Fn("link_open").curry(rules.link_open);
    rules.link_close = Fn("link_close");
    rules.image = Fn("image").curry(rules.image);
    rules.code_inline = Fn("code_inline").curry(rules.code_inline);
    rules.text = Fn("text").curry(rules.text);
    rules.em_open = |tokens, idx, options, env, renderer| {
        "<i" + renderer.render_attrs(tokens[idx]) + " data-m=\"" + tokens[idx].markup + "\">"
    };
    rules.em_close = |tokens, idx, options, env, renderer| "</i>";
    rules.strong_open = Fn("strong_open");
    rules.softbreak = |tokens, idx, options, env, renderer| "<wbr>\n";
    rules.html_inline = |tokens, idx, options, env, renderer| escape_html(tokens[idx].content);
    rules.s_close = Fn("s_close");
    md
}
