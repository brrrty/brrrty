//! Answers from LiquidJS 10.30.1, pinned.
//!
//! `probes/parse.json` holds templates run through `new Liquid().parse()`:
//! whether each parses, and if not the message, line and column.
//! `probes/text.json` holds templates of text, quoted-string outputs, raw
//! and comments, with what `parseAndRenderSync` printed. Those pin
//! whitespace control and how `raw` finds `endraw`.

use brrrty_liquid::ast::{Expr, Literal, Node, TagKind};
use serde::Deserialize;

#[derive(Deserialize)]
struct ParseProbe {
    src: String,
    ok: bool,
    msg: Option<String>,
    line: Option<usize>,
    col: Option<usize>,
}

#[derive(Deserialize)]
struct TextProbe {
    src: String,
    out: String,
}

/// Where Brrrty reports a better position than LiquidJS. LiquidJS parses a
/// quoted partial name as its own template and reports errors at positions
/// inside the name; Brrrty points at the name in the template.
const IMPROVED: [(&str, usize, usize); 1] = [("{% include 'a{{ b' %}", 1, 12)];

#[test]
fn parse_probes_match_liquidjs() {
    let probes: Vec<ParseProbe> = serde_json::from_str(include_str!("probes/parse.json")).unwrap();
    let mut failures = Vec::new();
    for p in &probes {
        let got = brrrty_liquid::parse("probe", &p.src);
        match (&got, p.ok) {
            (Ok(_), true) => {}
            (Err(e), false) => {
                let mut want = (p.msg.as_deref().unwrap(), p.line.unwrap(), p.col.unwrap());
                if let Some((_, line, col)) = IMPROVED.iter().find(|(src, ..)| *src == p.src) {
                    want = (want.0, *line, *col);
                }
                if (e.message.as_str(), e.line, e.column) != want {
                    failures.push(format!(
                        "{:?}: want {want:?}, got {:?}",
                        p.src,
                        (&e.message, e.line, e.column)
                    ));
                }
            }
            (Ok(_), false) => failures.push(format!(
                "{:?}: LiquidJS fails ({:?}), we parse",
                p.src, p.msg
            )),
            (Err(e), true) => failures.push(format!("{:?}: LiquidJS parses, we fail: {e}", p.src)),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {}:\n{}",
        failures.len(),
        probes.len(),
        failures.join("\n")
    );
}

/// What a template of text, string outputs, raw and comments prints.
fn flatten(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node {
            Node::Text(t) => out.push_str(&t.text),
            Node::Output(o) => match &o.value.expr {
                Expr::Literal {
                    value: Literal::String(s),
                    ..
                } if o.value.filters.is_empty() => out.push_str(s),
                other => panic!("unexpected output {other:?}"),
            },
            Node::Tag(t) => match &t.kind {
                TagKind::Raw { text } => out.push_str(text),
                TagKind::Comment | TagKind::InlineComment => {}
                TagKind::Liquid { body } => out.push_str(&flatten(body)),
                other => panic!("unexpected tag {other:?}"),
            },
        }
    }
    out
}

#[test]
fn text_probes_match_liquidjs() {
    let probes: Vec<TextProbe> = serde_json::from_str(include_str!("probes/text.json")).unwrap();
    for p in &probes {
        let t =
            brrrty_liquid::parse("probe", &p.src).unwrap_or_else(|e| panic!("{:?}: {e}", p.src));
        assert_eq!(flatten(&t.nodes), p.out, "{:?}", p.src);
    }
}
