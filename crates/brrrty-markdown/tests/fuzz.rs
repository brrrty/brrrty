//! Replays a corpus from `tools/gen-markdown-fuzz.cjs` and lists every difference.
//! `MARKDOWN_FUZZ=fuzz.json cargo test -p brrrty-markdown --test fuzz -- --ignored --nocapture`

use brrrty_markdown::{
    ConfiguredMarkdownIt, DefaultMarkdownIt, MarkdownIt, MarkdownRenderer, Options,
};
use serde_json::Value;

/// What a refused render is counted as: refusals are loud, so they aren't differences.
const REFUSED: &str = "<<REFUSED>>";

#[test]
#[ignore = "needs MARKDOWN_FUZZ=<corpus.json>"]
fn fuzz_corpus() {
    // MARKDOWN_MODE=eleventy replays a corpus made in that mode against 11ty's instance, and
    // MARKDOWN_OPTIONS one made with those options.
    let eleventy = std::env::var("MARKDOWN_MODE").as_deref() == Ok("eleventy");
    let options = std::env::var("MARKDOWN_OPTIONS").ok().map(|o| {
        let o: Value = serde_json::from_str(&o).expect("MARKDOWN_OPTIONS is JSON");
        let flag = |k: &str| o[k].as_bool().unwrap_or(false);
        Options {
            html: flag("html"),
            linkify: flag("linkify"),
            typographer: flag("typographer"),
            fuzzy_link: flag("fuzzyLink"),
        }
    });
    let path = std::env::var("MARKDOWN_FUZZ").expect("MARKDOWN_FUZZ");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let md = DefaultMarkdownIt::new();
    let eleventy_md = MarkdownIt::new();
    let configured = options.map(ConfiguredMarkdownIt::new);
    let mut bad = 0;
    let mut refused = 0;
    for c in v.as_array().unwrap() {
        let s = c["s"].as_str().unwrap();
        // A panic in the port is a finding too.
        let run = |inline: bool| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match (&configured, eleventy, inline) {
                    (Some(_), _, true) | (None, true, true) => String::new(),
                    (Some(c), _, false) => c.render(s).unwrap_or_else(|_| REFUSED.into()),
                    (None, true, false) => {
                        eleventy_md.try_render(s).unwrap_or_else(|_| REFUSED.into())
                    }
                    (None, false, true) => md.render_inline(s),
                    (None, false, false) => md.try_render(s).unwrap_or_else(|_| REFUSED.into()),
                }
            }))
            .unwrap_or_else(|_| "<<PANIC>>".into())
        };
        for (kind, got) in [("render", run(false)), ("inline", run(true))] {
            let Some(want) = c[if kind == "render" {
                "render"
            } else {
                "render_inline"
            }]
            .as_str() else {
                continue;
            };
            if (eleventy || configured.is_some()) && kind == "inline" {
                continue;
            }
            if got == REFUSED {
                refused += 1;
                continue;
            }
            if got != want {
                bad += 1;
                println!("{kind} {s:?}\n  want {want:?}\n  got  {got:?}");
            }
        }
    }
    println!("{bad} differences, {refused} refused");
    assert_eq!(bad, 0);
}
