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

#[test]
fn no_rest_grammar_has_a_rest_of_its_own() {
    // So merging `rest` doesn't depend on which grammar Prism tokenizes
    // first (see `Grammars::tokens`).
    let g = grammars();
    for gr in &g.grammars {
        if let Some(rest) = gr.rest {
            assert!(g.grammar(rest).rest.is_none());
        }
    }
    // A grammar's `rest` tokens come after its own.
    let css = g.language("css").unwrap();
    let atrule = g.tokens(css).iter().find(|t| t.name == "atrule").unwrap();
    let inside = atrule.patterns[0].inside.unwrap();
    let names: Vec<_> = g.tokens(inside).iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names.first(), Some(&"rule"));
    assert!(names.contains(&"url") && names.contains(&"string"));
}

#[test]
fn hook_patterns_are_prisms_and_translate() {
    use brrrty_prism::highlight::{PREFIXES, compile_hook_regex, hook_regex};
    let json: serde_json::Value =
        serde_json::from_str(include_str!("../grammars/prism.json")).unwrap();
    let sources: String = json["hooks"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|v| v.as_array().unwrap())
        .map(|s| s.as_str().unwrap())
        .collect();
    for r in [
        hook_regex::PHP_OPEN,
        hook_regex::PHP,
        hook_regex::LIQUID,
        hook_regex::LIQUID_TAG,
        hook_regex::SHARP,
        hook_regex::PLUS_PLUS,
        hook_regex::FIRST_WORD,
        hook_regex::LANGUAGE_CLASS,
    ] {
        let literal = format!("/{}/{}", r.1, r.2);
        assert!(sources.contains(&literal), "{literal} isn't in a hook");
    }
    for r in hook_regex::ALL {
        compile_hook_regex(r).unwrap_or_else(|e| panic!("/{}/{}: {}", r.1, r.2, e.0));
    }
    // diff-highlight's prefixes are the diff grammar's tokens, aliased by
    // their first word.
    let g = grammars();
    let diff = g.language("diff").unwrap();
    for (kind, _, word) in PREFIXES {
        let t = g.tokens(diff).iter().find(|t| t.name == *kind).unwrap();
        if kind.contains('-') {
            assert_eq!(t.patterns[0].alias.first().map(String::as_str), Some(*word));
        }
    }
}

#[test]
fn what_loading_a_language_surely_brings() {
    let g = grammars();
    for k in [
        "markup",
        "html",
        "css",
        "clike",
        "javascript",
        "js",
        "diff",
        "text",
    ] {
        assert!(g.base_keys.iter().any(|b| b == k), "{k}");
    }
    assert!(!g.base_keys.iter().any(|b| b == "yaml"));
    let ts = g.keys_after_loading("typescript").unwrap();
    assert!(ts.iter().any(|k| k == "ts") && ts.iter().any(|k| k == "javascript"));
    let php = g.keys_after_loading("php").unwrap();
    assert!(php.iter().any(|k| k == "markup-templating"));
    // Every name is a grammar Brrrty has.
    for (_, keys) in &g.keys_by_language {
        for k in keys {
            assert!(g.language(k).is_some(), "{k}");
        }
    }
    assert!(g.keys_after_loading("haskell").is_none());
}

#[test]
fn languages_brrrty_lacks_that_change_its_grammars() {
    let g = grammars();
    let has = |by: &str, key: &str| {
        g.mutated_by(by)
            .is_some_and(|ks| ks.iter().any(|k| k == key))
    };
    // `javascript` lists `regex` as optional: loading it reloads
    // javascript, and markup, css and everything built on them.
    for key in [
        "javascript",
        "js",
        "markup",
        "html",
        "css",
        "php",
        "markdown",
        "jsx",
    ] {
        assert!(has("regex", key), "regex changes {key}");
    }
    assert!(!has("regex", "bash"));
    assert!(has("css-extras", "css"));
    assert!(has("jsdoc", "javascript"));
    assert!(has("php-extras", "php"));
    assert!(has("opencl", "c"));
    // cilkc's insertBefore replaces Prism.languages.c, which components.json
    // doesn't say.
    assert!(has("cilkc", "c"));
    // Aliases count as their component.
    assert_eq!(g.mutated_by("adoc"), g.mutated_by("asciidoc"));
    // Languages that change nothing Brrrty has.
    for name in [
        "kotlin",
        "swift",
        "cpp",
        "csharp",
        "javascript",
        "bash",
        "yaml",
    ] {
        assert!(g.mutated_by(name).is_none(), "{name}");
    }
    // Every key named is one Brrrty has.
    for (_, keys) in &g.mutators {
        for k in keys {
            assert!(g.language(k).is_some(), "{k}");
        }
    }
}
