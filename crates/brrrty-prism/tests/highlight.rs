//! Replays tests/probes/highlight/*.json: what Prism 1.30.0 gave, called as
//! @11ty/eleventy-plugin-syntaxhighlight 5.0.2 calls it, for the code of
//! Prism's own test fixtures and tests/fuzz/corpus.txt, in every fence
//! language (tools/gen-highlight-probes.cjs wrote the files). The HTML must
//! be the same to the byte.

use brrrty_prism::grammar::grammars;
use brrrty_prism::{AllLoaded, highlight};
use serde_json::Value;

#[test]
fn generic_corpus_matches_node() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/probes/highlight");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut languages = Vec::new();
    for f in &files {
        let file: Value = serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        let lang = file["language"].as_str().unwrap();
        languages.push(lang.to_string());
        for case in file["cases"].as_array().unwrap() {
            let code = case["code"].as_str().unwrap();
            let from = case["from"].as_str().unwrap();
            let key = case["grammar"].as_str().unwrap();
            let grammar = grammars()
                .language(key)
                .unwrap_or_else(|| panic!("no grammar {key}"));
            compared += 1;
            match highlight(code, grammar, lang, &mut AllLoaded::default()) {
                Ok(html) if html == case["html"].as_str().unwrap() => {}
                Ok(html) => failures.push(format!(
                    "{from} as {lang}:\n  got  {:?}\n  node {:?}",
                    first_difference(&html, case["html"].as_str().unwrap()).0,
                    first_difference(&html, case["html"].as_str().unwrap()).1,
                )),
                Err(e) => failures.push(format!("{from} as {lang}: refused: {}", e.0)),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {compared} differ:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
    assert!(compared > 300, "only {compared} cases");
    assert!(languages.len() >= 30, "only {languages:?}");
}

/// The two strings from a little before where they first differ.
fn first_difference(a: &str, b: &str) -> (String, String) {
    let i = a
        .char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((i, _), _)| i);
    let from = a[..i].char_indices().rev().nth(60).map_or(0, |(j, _)| j);
    let take = |s: &str| s.get(from..).unwrap_or("").chars().take(160).collect();
    (take(a), take(b))
}

/// markup-templating recurses once per placeholder in a run of markup;
/// Node runs out of stack at about 4,400, so Brrrty refuses past 1,000.
#[test]
fn a_long_run_of_template_parts_is_refused() {
    // On a thread with the stack Brrrty's render threads have (16 MiB): the
    // recursion this limits needs more than the 2 MiB of a test thread in a
    // debug build.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let liquid = grammars().language("liquid").unwrap();
            let run = |n: usize| {
                highlight(
                    &"{{ a }}".repeat(n),
                    liquid,
                    "liquid",
                    &mut AllLoaded::default(),
                )
            };
            assert!(run(1000).is_ok());
            let e = run(1100).unwrap_err();
            assert!(e.0.contains("markup-templating"), "{}", e.0);
        })
        .unwrap()
        .join()
        .unwrap();
}

/// `Prism.languages[name]` as hooks see it: a grammar Prism has, a name a hook set (which wins
/// over the grammar of that name), nothing for a name Prism doesn't know, and a grammar with no
/// tokens for what every object has (`Prism.languages.toString` is a function).
#[test]
fn all_loaded_looks_names_up_as_prism_languages_does() {
    use brrrty_prism::Languages;
    let g = grammars();
    let js = g.language("javascript").unwrap();
    let css = g.language("css").unwrap();
    let plain = g.language("plain").unwrap();
    let mut langs = AllLoaded::default();
    assert_eq!(langs.get("javascript").unwrap(), Some(js));
    assert_eq!(langs.get("no-such-language").unwrap(), None);
    for key in ["toString", "constructor", "extend", "__proto__"] {
        assert_eq!(langs.get(key).unwrap(), Some(plain), "{key}");
    }
    langs.set("javascript", css);
    langs.set("custom", js);
    langs.set("custom", css);
    assert_eq!(langs.get("javascript").unwrap(), Some(css));
    assert_eq!(langs.get("custom").unwrap(), Some(css));
}
