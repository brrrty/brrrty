//! Answers from LiquidJS 10.30.1, pinned.
//!
//! `probes/parse.json` holds templates run through `new Liquid().parse()`:
//! whether each parses, and if not the message, line and column.
//! `probes/text.json` holds templates of text, quoted-string outputs, raw
//! and comments, with what `parseAndRenderSync` printed. Those pin
//! whitespace control and how `raw` finds `endraw`.
//! `probes/render.json` holds templates and data run through `parse()` and
//! `render()` with `strictFilters: true` and `TZ=UTC`, with what LiquidJS
//! printed or the error it raised. Data objects of the form
//! `{"$date": "..."}` are `Date`s.

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

#[derive(Deserialize)]
struct RenderProbe {
    src: String,
    data: serde_json::Value,
    out: Option<String>,
    err: Option<String>,
}

/// Where Brrrty knowingly differs from LiquidJS, with what Brrrty prints.
///
/// LiquidJS compares `Date`s with `===`, so two `Date` objects for the same
/// moment are unequal. Brrrty's dates are plain values with no identity, so
/// it compares them by time.
const RENDER_DEVIATIONS: [(&str, &str); 1] = [(
    "{% if d == d %}a{% endif %}{% if d == e %}b{% endif %}{% if d >= e %}c{% endif %}",
    "abc",
)];

fn revive(v: &serde_json::Value) -> brrrty_js::Value {
    use std::sync::Arc;
    match v {
        serde_json::Value::Object(o) if o.len() == 1 && o.contains_key("$date") => {
            let text = o["$date"].as_str().unwrap();
            brrrty_js::Value::Date(chrono::DateTime::parse_from_rfc3339(text).unwrap())
        }
        serde_json::Value::Object(o) => brrrty_js::Value::Object(Arc::new(
            o.iter().map(|(k, v)| (k.clone(), revive(v))).collect(),
        )),
        serde_json::Value::Array(a) => {
            brrrty_js::Value::Array(Arc::new(a.iter().map(revive).collect()))
        }
        other => brrrty_js::Value::from_json(other),
    }
}

/// Splits LiquidJS's `message, line:N, col:M`.
fn split_position(text: &str) -> (&str, Option<(usize, usize)>) {
    let Some(at) = text.rfind(", line:") else {
        return (text, None);
    };
    let (line, col) = text[at + 7..].split_once(", col:").unwrap();
    (
        &text[..at],
        Some((line.parse().unwrap(), col.parse().unwrap())),
    )
}

struct NoPartials;

impl brrrty_liquid::Loader for NoPartials {
    fn load(&self, _: &str) -> Result<(String, String), brrrty_liquid::LoadError> {
        Err(Default::default())
    }
}

#[test]
fn render_probes_match_liquidjs() {
    use brrrty_liquid::{Engine, EngineOptions, Tz};
    let probes: Vec<RenderProbe> =
        serde_json::from_str(include_str!("probes/render.json")).unwrap();
    let engine = Engine::new(EngineOptions {
        strict_filters: true,
        timezone: Tz::Utc,
    });
    let mut failures = Vec::new();
    for p in &probes {
        let got = engine
            .parse("probe", &p.src)
            .and_then(|t| engine.render(&t, &revive(&p.data), &NoPartials));
        let deviation = RENDER_DEVIATIONS.iter().find(|(src, _)| *src == p.src);
        match (&got, &p.out, &p.err) {
            (Ok(got), Some(want), None) => {
                let want = deviation.map_or(want.as_str(), |(_, ours)| ours);
                if got != want {
                    failures.push(format!("{:?}: want {want:?}, got {got:?}", p.src));
                }
            }
            (Err(e), None, Some(err)) => {
                let (msg, pos) = split_position(err);
                let pos = pos.unwrap();
                if (e.message.as_str(), e.line, e.column) != (msg, pos.0, pos.1) {
                    failures.push(format!(
                        "{:?}: want {msg:?} at {pos:?}, got {:?} at {}:{}",
                        p.src, e.message, e.line, e.column
                    ));
                }
            }
            (Ok(got), _, Some(err)) => failures.push(format!(
                "{:?}: LiquidJS fails ({err}), we print {got:?}",
                p.src
            )),
            (Err(e), Some(want), _) => failures.push(format!(
                "{:?}: LiquidJS prints {want:?}, we fail: {e}",
                p.src
            )),
            _ => unreachable!("a probe has an out or an err"),
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
