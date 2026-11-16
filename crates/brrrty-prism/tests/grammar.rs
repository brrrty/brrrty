//! The grammars in grammars/prism.json, as loaded.

use brrrty_prism::grammar::{Grammars, grammars};

fn id(g: &Grammars, lang: &str) -> usize {
    g.language(lang)
        .unwrap_or_else(|| panic!("{lang} is missing"))
        .0
}

fn names(g: &Grammars, lang: &str) -> Vec<String> {
    g.grammar(g.language(lang).unwrap())
        .tokens
        .iter()
        .map(|t| t.name.clone())
        .collect()
}

#[test]
fn versions_and_fence_languages() {
    let g = grammars();
    assert_eq!(g.prism, "1.30.0");
    assert_eq!(g.plugin, "5.0.2");
    let unknown: Vec<&str> = g
        .fences
        .iter()
        .filter(|f| !f.known)
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(
        unknown,
        [
            "conf",
            "jql",
            "console",
            "config",
            "zsh",
            "terraform",
            "fstab"
        ]
    );
    let shell = g.fences.iter().find(|f| f.name == "shell").unwrap();
    assert_eq!(shell.normalized, "bash");
    // Aliases are the same grammar, as in Prism.languages.
    assert_eq!(id(g, "js"), id(g, "javascript"));
    assert_eq!(id(g, "sh"), id(g, "bash"));
    assert_eq!(id(g, "html"), id(g, "markup"));
    assert_eq!(id(g, "text"), id(g, "plain"));
    assert!(g.grammar(g.language("text").unwrap()).tokens.is_empty());
    // Every language the fences load, dependencies included.
    for lang in [
        "markup",
        "css",
        "clike",
        "javascript",
        "diff",
        "bash",
        "markup-templating",
        "php",
        "sql",
        "yaml",
        "typescript",
        "json",
        "ruby",
        "python",
        "markdown",
        "ini",
        "vim",
        "liquid",
        "c",
        "nginx",
        "perl",
        "less",
        "jsx",
        "go",
        "toml",
        "rust",
        "r",
        "lua",
    ] {
        id(g, lang);
    }
    assert!(g.language("haskell").is_none());
}

#[test]
fn tokens_keep_prisms_order_after_insert_before() {
    let g = grammars();
    assert_eq!(
        names(g, "javascript"),
        [
            "comment",
            "hashbang",
            "template-string",
            "string-property",
            "string",
            "class-name",
            "regex",
            "function-variable",
            "parameter",
            "constant",
            "keyword",
            "boolean",
            "function",
            "number",
            "literal-property",
            "operator",
            "punctuation",
        ]
    );
    assert_eq!(
        names(g, "css"),
        [
            "comment",
            "atrule",
            "url",
            "selector",
            "string",
            "property",
            "important",
            "function",
            "punctuation",
        ]
    );
}

#[test]
fn inside_and_rest_are_shared_references() {
    let g = grammars();
    let css = g.language("css").unwrap();
    let atrule = &g.grammar(css).tokens[1];
    assert_eq!(atrule.name, "atrule");
    let inside = atrule.patterns[0].inside.unwrap();
    // `atrule.inside.rest` is the css grammar itself: a cycle.
    assert_eq!(g.grammar(inside).rest, Some(css));
    let markup = g.language("markup").unwrap();
    let style = &g.grammar(markup).tokens[3];
    assert_eq!(style.name, "style");
    assert!(style.patterns[0].lookbehind && style.patterns[0].greedy);
    // Every reference is in range, and every pattern has its regex.
    for (i, gr) in g.grammars.iter().enumerate() {
        for t in &gr.tokens {
            for p in &t.patterns {
                assert!(
                    p.inside.is_none_or(|x| x.0 < g.grammars.len()),
                    "{i} {}",
                    t.name
                );
                let r = g.regex_entry(p.regex);
                assert!(!r.source.is_empty() || r.flags.is_empty());
            }
        }
    }
}

#[test]
fn markdown_front_matter_depends_on_load_order() {
    let g = grammars();
    assert_eq!(
        g.order_dependent("markdown"),
        ["front-matter-block/0 > front-matter/0: inside"]
    );
    assert!(g.order_dependent("javascript").is_empty());
}

#[test]
fn every_regex_is_translated_or_has_a_reason() {
    let g = grammars();
    for id in 0..g.regexes.len() {
        let id = brrrty_prism::grammar::RegexId(id);
        if let Err(why) = g.regex(id) {
            assert!(!why.0.is_empty());
        }
    }
    // The languages the fences use, and which of them are fully translated.
    let unsupported: Vec<String> = g
        .fences
        .iter()
        .filter(|f| f.known)
        .filter(|f| g.support(g.language(&f.normalized).unwrap()).is_err())
        .map(|f| f.normalized.clone())
        .collect();
    assert_eq!(
        unsupported,
        Vec::<String>::new(),
        "unsupported fence languages"
    );
}
