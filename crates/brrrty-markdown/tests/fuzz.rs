//! Replays a corpus from `tools/gen-markdown-fuzz.cjs` and lists every difference.
//! `MARKDOWN_FUZZ=fuzz.json cargo test -p brrrty-markdown --test fuzz -- --ignored --nocapture`

use std::cell::RefCell;

use brrrty_markdown::{
    ConfiguredMarkdownIt, DefaultMarkdownIt, MarkdownIt, MarkdownRenderer, NoRules, Options,
    RenderError, RenderOptions, RenderRules, Token,
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
    let (tokens_bad, ruled_bad) =
        tokens_and_no_rules(&v, eleventy, configured.as_ref(), &md, &eleventy_md);
    println!("{tokens_bad} token lists differ, {ruled_bad} renders through tokens differ");
    assert_eq!(tokens_bad + ruled_bad, 0);
    assert_eq!(bad, 0);
}

/// Records each inline token list and renders it with markdown-it's default rules.
#[derive(Default)]
struct Recorder(RefCell<Vec<Vec<Token>>>);

impl RenderRules for Recorder {
    fn render_inline(&self, tokens: Vec<Token>, options: &RenderOptions) -> Result<String, String> {
        self.0.borrow_mut().push(tokens.clone());
        NoRules.render_inline(tokens, options)
    }
}

/// A token as `tools/gen-markdown-fuzz.cjs` dumps it.
fn token_json(t: &Token) -> Value {
    serde_json::json!({
        "type": t.kind, "tag": t.tag,
        "attrs": t.attrs.as_ref().map(|a| a.iter().map(|(n, v)| serde_json::json!([n, v])).collect::<Vec<_>>()),
        "nesting": t.nesting, "level": t.level, "content": t.content, "markup": t.markup,
        "info": t.info, "block": t.block, "hidden": t.hidden,
    })
}

/// With `MARKDOWN_TOKENS=1` corpora: each inline token list must be markdown-it's, and the
/// render through tokens (no rules) must be the tree's. Returns how many of each differ.
fn tokens_and_no_rules(
    v: &Value,
    eleventy: bool,
    configured: Option<&ConfiguredMarkdownIt>,
    md: &DefaultMarkdownIt,
    eleventy_md: &MarkdownIt,
) -> (usize, usize) {
    let (mut tokens_bad, mut ruled_bad) = (0, 0);
    for c in v.as_array().unwrap() {
        let s = c["s"].as_str().unwrap();
        let rec = Recorder::default();
        let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match (configured, eleventy) {
                (Some(c), _) => c.render_with_rules(s, &rec),
                (None, true) => eleventy_md.try_render_with_rules(s, None, &rec),
                (None, false) => md.render_with_rules(s, &rec),
            }
        }))
        .unwrap_or_else(|_| Err(RenderError::Rule("<<PANIC>>".into())));
        let got = match got {
            Ok(html) => html,
            Err(RenderError::Refused(_)) => continue,
            Err(e) => format!("<<{e}>>"),
        };
        if got != c["render"].as_str().unwrap() {
            ruled_bad += 1;
            println!(
                "tokens render {s:?}\n  want {:?}\n  got  {got:?}",
                c["render"]
            );
        }
        let Some(want) = c.get("tokens") else {
            continue;
        };
        let lists: Vec<Value> = rec
            .0
            .borrow()
            .iter()
            .map(|l| Value::Array(l.iter().map(token_json).collect()))
            .collect();
        let got = Value::Array(lists);
        if &got != want {
            tokens_bad += 1;
            println!("tokens {s:?}\n  want {want}\n  got  {got}");
        }
    }
    (tokens_bad, ruled_bad)
}
