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
    assert!(cases.len() >= 15, "{kind}: {} probes", cases.len());
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

/// 11ty's own instance (markdown-it 14.3.3, html on, `code` off), unclosed fences included.
#[test]
fn eleventy_instance_matches_its_markdown_it() {
    let md = MarkdownIt::new();
    let cases = probes("render_eleventy");
    assert!(cases.len() >= 25);
    let bad: Vec<String> = cases
        .iter()
        .filter_map(|(s, want)| {
            let got = md.render(s);
            (&got != want).then(|| format!("{s:?}\n  want {want:?}\n  got  {got:?}"))
        })
        .collect();
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}

/// Inputs the fuzz runs found and Brrrty now renders as markdown-it 15.0.3 does.
#[test]
fn fixed_cases_match_markdown_it() {
    check("render_fixed", |md, s| md.render(s));
}

/// `markdownIt({ linkify: true, typographer: true })` with `fuzzyLink` on (`out`) and off
/// (`strict`), and with `html: true` too (`html`).
#[test]
fn linkify_and_typographer_match_markdown_it() {
    use brrrty_markdown::{ConfiguredMarkdownIt, Options};
    let v: Value =
        serde_json::from_str(include_str!("probes/markdown_it.json")).expect("probe file");
    let base = Options {
        linkify: true,
        typographer: true,
        ..Options::default()
    };
    let instances = [
        (
            "out",
            ConfiguredMarkdownIt::new(Options {
                fuzzy_link: true,
                ..base
            }),
        ),
        ("strict", ConfiguredMarkdownIt::new(base)),
        (
            "html",
            ConfiguredMarkdownIt::new(Options {
                fuzzy_link: true,
                html: true,
                ..base
            }),
        ),
    ];
    let cases = v["render_linkify_typographer"].as_array().expect("list");
    assert!(cases.len() >= 20);
    let mut bad = Vec::new();
    for c in cases {
        let s = c["s"].as_str().unwrap();
        for (key, md) in &instances {
            let want = c[*key].as_str().unwrap();
            let got = md.render(s).unwrap();
            if got != want {
                bad.push(format!("{key} {s:?}\n  want {want:?}\n  got  {got:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}

/// The review's cases, in every instance: 15's `render` and `renderInline`, 11ty's 14.3.3, and
/// 15 with `html`, `linkify` and the typographer on (`fuzzyLink` on).
#[test]
fn round1_cases_match_markdown_it_in_every_instance() {
    use brrrty_markdown::{ConfiguredMarkdownIt, Options};
    let v: Value =
        serde_json::from_str(include_str!("probes/markdown_it.json")).expect("probe file");
    let md = DefaultMarkdownIt::new();
    let eleventy = MarkdownIt::new();
    let html = ConfiguredMarkdownIt::new(Options {
        html: true,
        linkify: true,
        typographer: true,
        fuzzy_link: true,
    });
    let cases = v["render_round1"].as_array().expect("list");
    assert!(cases.len() >= 30);
    let mut bad = Vec::new();
    for c in cases {
        let s = c["s"].as_str().unwrap();
        let got = [
            ("out", md.try_render(s).unwrap()),
            ("inline", md.render_inline(s)),
            ("eleventy", eleventy.try_render(s).unwrap()),
            ("html", html.render(s).unwrap()),
        ];
        for (key, got) in got {
            let want = c[key].as_str().unwrap();
            if got != want {
                bad.push(format!("{key} {s:?}\n  want {want:?}\n  got  {got:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}

/// Shapes the port's block rules still parse differently are refused in every instance, and
/// their near neighbours that render the same are not.
#[test]
fn block_shapes_the_port_gets_wrong_are_refused() {
    use brrrty_markdown::{ConfiguredMarkdownIt, Options};
    let md = DefaultMarkdownIt::new();
    let eleventy = MarkdownIt::new();
    let configured = ConfiguredMarkdownIt::new(Options {
        linkify: true,
        typographer: true,
        ..Options::default()
    });
    let refused = [
        "[docs]: C:\\path\\\n\n[docs]",
        "> quote\n-",
        "> a\n> b\n2)",
        "> a\n| b | c |\n|---|---|",
        "> a `b\n    c` d",
        ">\t<pre>\n>\tx</pre>\n\n>>\t\tcode",
        ">\t>\t* x",
        ">```go\n>\tb c",
        "[docs]: \n+",
        "> [x]: /u\"\\\n\tx[x]",
        "[docs]: \nC:\\path\\\nx [docs]",
    ];
    for s in refused {
        assert!(md.try_render(s).is_err(), "{s:?}");
        assert!(configured.render(s).is_err(), "{s:?}");
    }
    for s in refused {
        assert!(eleventy.try_render(s).is_err(), "{s:?}");
    }
    // A tab among the markers after a second `>` (the port counts nested columns differently),
    // and an HTML block after `>\t` (11ty; HTML is off in the default instance).
    assert!(md.try_render(">>\t\tcode").is_err());
    assert!(eleventy.try_render(">>\t\tcode").is_err());
    assert!(eleventy.try_render(">\t<pre>").is_err());
    for s in [
        "> quote\n- item",
        "| a |\n| --- |\n| b |",
        "[dev server](http://[::1]:8080/)",
        "> a\n\n| b | c |\n|---|---|",
        "a `b\n    c` d",
        "Title\n---",
    ] {
        assert!(md.try_render(s).is_ok(), "{s:?}");
        assert!(eleventy.try_render(s).is_ok(), "{s:?}");
    }
}
