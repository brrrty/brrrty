//! `DefaultMarkdownIt` against markdown-it 15.0.3 with no options, byte for byte.
//! The inputs and outputs are in `tests/probes/markdown_it.json`, made by
//! `tools/gen-markdown-probes.cjs`.

use brrrty_markdown::{DefaultMarkdownIt, MarkdownRenderer};
use serde_json::Value;

fn probes(kind: &str) -> Vec<(String, String)> {
    let v: Value =
        serde_json::from_str(include_str!("probes/markdown_it.json")).expect("probe file");
    v[kind]
        .as_array()
        .expect("list")
        .iter()
        .map(|c| {
            (
                c["s"].as_str().unwrap().into(),
                c["out"].as_str().unwrap().into(),
            )
        })
        .collect()
}

fn check(kind: &str, f: impl Fn(&DefaultMarkdownIt, &str) -> String) {
    let md = DefaultMarkdownIt::new();
    let cases = probes(kind);
    assert!(cases.len() >= 50, "{kind}: {} probes", cases.len());
    let bad: Vec<String> = cases
        .iter()
        .filter_map(|(s, want)| {
            let got = f(&md, s);
            (&got != want).then(|| format!("{s:?}\n  want {want:?}\n  got  {got:?}"))
        })
        .collect();
    assert!(
        bad.is_empty(),
        "{} of {} differ:\n{}",
        bad.len(),
        cases.len(),
        bad.join("\n")
    );
}

#[test]
fn render_matches_markdown_it() {
    check("render", |md, s| md.render(s));
}

#[test]
fn render_inline_matches_markdown_it() {
    check("render_inline", |md, s| md.render_inline(s));
}
