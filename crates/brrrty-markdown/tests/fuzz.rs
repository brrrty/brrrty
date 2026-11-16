//! Replays a corpus from `tools/gen-markdown-fuzz.cjs` and lists every difference.
//! `MARKDOWN_FUZZ=fuzz.json cargo test -p brrrty-markdown --test fuzz -- --ignored --nocapture`

use brrrty_markdown::{DefaultMarkdownIt, MarkdownIt, MarkdownRenderer};
use serde_json::Value;

#[test]
#[ignore = "needs MARKDOWN_FUZZ=<corpus.json>"]
fn fuzz_corpus() {
    // MARKDOWN_MODE=eleventy replays a corpus made in that mode against 11ty's instance.
    let eleventy = std::env::var("MARKDOWN_MODE").as_deref() == Ok("eleventy");
    let path = std::env::var("MARKDOWN_FUZZ").expect("MARKDOWN_FUZZ");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let md = DefaultMarkdownIt::new();
    let eleventy_md = MarkdownIt::new();
    let mut bad = 0;
    for c in v.as_array().unwrap() {
        let s = c["s"].as_str().unwrap();
        // A panic in the port is a finding too.
        let run = |inline: bool| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match (eleventy, inline) {
                    (true, true) => String::new(), // 11ty's instance has no inline render
                    (true, false) => eleventy_md.render(s),
                    (false, true) => md.render_inline(s),
                    (false, false) => md.render(s),
                }
            }))
            .unwrap_or_else(|_| "<<PANIC>>".into())
        };
        for (kind, got, want) in [
            ("render", run(false), c["render"].as_str().unwrap()),
            ("inline", run(true), c["render_inline"].as_str().unwrap()),
        ] {
            if eleventy && kind == "inline" {
                continue;
            }
            if got != want {
                bad += 1;
                println!("{kind} {s:?}\n  want {want:?}\n  got  {got:?}");
            }
        }
    }
    println!("{bad} differences");
    assert_eq!(bad, 0);
}
