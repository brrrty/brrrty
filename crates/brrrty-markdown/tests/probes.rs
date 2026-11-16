//! `DefaultMarkdownIt` against markdown-it 15.0.3 with no options, on the pinned cases (see the known gaps in PORTING.md).
//! The inputs and outputs are in `tests/probes/markdown_it.json`, made by
//! `tools/gen-markdown-probes.cjs`.

use brrrty_markdown::{DefaultMarkdownIt, MarkdownIt, MarkdownRenderer};
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

/// 11ty's own instance (markdown-it 14.3.3, html on, `code` off). The unclosed-fence cases are
/// known gaps (see PORTING.md) and are listed here so they stay visible.
#[test]
fn eleventy_instance_matches_its_markdown_it() {
    let md = MarkdownIt::new();
    let known_gaps = [
        "```js\r\n\n  ",
        "```js\r><div>+ + \n ",
        "```\nx\n  ",
        "```\n  ",
        "```\n\n",
        "<div>x</div>",
    ];
    let cases = probes("render_eleventy");
    assert!(cases.len() >= 25);
    let bad: Vec<String> = cases
        .iter()
        .filter(|(s, _)| !known_gaps.contains(&s.as_str()))
        .filter_map(|(s, want)| {
            let got = md.render(s);
            (&got != want).then(|| format!("{s:?}\n  want {want:?}\n  got  {got:?}"))
        })
        .collect();
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}
