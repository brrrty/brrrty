//! Replays a corpus from `tools/gen-markdown-fuzz.cjs` and lists every difference.
//! `MARKDOWN_FUZZ=fuzz.json cargo test -p brrrty-markdown --test fuzz -- --ignored --nocapture`

use brrrty_markdown::{DefaultMarkdownIt, MarkdownRenderer};
use serde_json::Value;

#[test]
#[ignore = "needs MARKDOWN_FUZZ=<corpus.json>"]
fn fuzz_corpus() {
    let path = std::env::var("MARKDOWN_FUZZ").expect("MARKDOWN_FUZZ");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let md = DefaultMarkdownIt::new();
    let mut bad = 0;
    for c in v.as_array().unwrap() {
        let s = c["s"].as_str().unwrap();
        // A panic in the port is a finding too.
        let run = |inline: bool| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if inline {
                    md.render_inline(s)
                } else {
                    md.render(s)
                }
            }))
            .unwrap_or_else(|_| "<<PANIC>>".into())
        };
        for (kind, got, want) in [
            ("render", run(false), c["render"].as_str().unwrap()),
            ("inline", run(true), c["render_inline"].as_str().unwrap()),
        ] {
            if got != want {
                bad += 1;
                println!("{kind} {s:?}\n  want {want:?}\n  got  {got:?}");
            }
        }
    }
    println!("{bad} differences");
    assert_eq!(bad, 0);
}
