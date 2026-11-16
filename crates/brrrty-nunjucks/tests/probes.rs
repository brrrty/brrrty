//! Answers from Nunjucks 3.2.4, pinned.
//!
//! `probes/parse.json` holds templates run through Nunjucks's parser as
//! 11ty configures it (made by `tools/gen-parse-probes.cjs`). For each
//! template that parses it has the tree, which [`dump`] prints Brrrty's
//! tree in the same form to compare. For each that fails it has the
//! message and, where Nunjucks gives one, the line and column.

use brrrty_nunjucks::ast::*;
use brrrty_nunjucks::{ErrorKind, parse};
use serde::Deserialize;

#[derive(Deserialize)]
struct ParseProbe {
    src: String,
    ok: bool,
    ast: Option<String>,
    msg: Option<String>,
    line: Option<usize>,
    col: Option<usize>,
}

/// Where Brrrty knowingly differs. 11ty adds `setAsync` to Nunjucks as a
/// tag; Brrrty does not run it and says so instead of calling it unknown.
const UNSUPPORTED: [&str; 1] = ["{% setAsync \"x\" %}a{% endsetAsync %}"];

fn at(span: Span) -> String {
    format!("@{}:{}", span.line - 1, span.column - 1)
}

fn list(nodes: &[Node]) -> String {
    let items: Vec<String> = nodes.iter().map(node).collect();
    format!("[{}]", items.join(" "))
}

fn exprs(items: &[Expr]) -> String {
    items.iter().map(expr).collect::<Vec<_>>().join(" ")
}

fn opt(e: Option<&Expr>) -> String {
    e.map(expr).unwrap_or_else(|| "~".into())
}

fn args(a: &Args) -> String {
    let mut items: Vec<String> = a.positional.iter().map(expr).collect();
    if !a.keyword.is_empty() {
        let kw: Vec<String> = a.keyword.iter().map(pair).collect();
        items.push(format!("KW({})", kw.join(" ")));
    }
    format!("Args({})", items.join(" "))
}

fn pair(p: &Pair) -> String {
    format!("Pair{}({}, {})", at(p.span), expr(&p.key), expr(&p.value))
}

fn flag(v: Option<bool>) -> String {
    v.map(|b| b.to_string()).unwrap_or_else(|| "~".into())
}

fn json(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

/// `{% if %}` prints as Nunjucks nests it: each `elif` is an `If` in the
/// `else_` of the one before.
fn if_chain(n: &If, i: usize) -> String {
    let b = &n.branches[i];
    let name = if i == 0 && n.is_async {
        "IfAsync"
    } else {
        "If"
    };
    let else_ = if i + 1 < n.branches.len() {
        if_chain(n, i + 1)
    } else {
        n.else_body
            .as_deref()
            .map(list)
            .unwrap_or_else(|| "~".into())
    };
    format!(
        "{name}{}({}, {}, {else_})",
        at(b.span),
        expr(&b.cond),
        list(&b.body)
    )
}

fn node(n: &Node) -> String {
    match n {
        Node::Text(t) => format!("Text{}({})", at(t.span), json(&t.text)),
        Node::Output(o) => format!("Output{}({})", at(o.span), expr(&o.expr)),
        Node::If(i) => if_chain(i, 0),
        Node::For(f) => {
            let name = match f.kind {
                ForKind::For => "For",
                ForKind::AsyncEach => "AsyncEach",
                ForKind::AsyncAll => "AsyncAll",
            };
            let target = if f.targets.len() == 1 {
                expr(&f.targets[0])
            } else {
                format!("Array{}({})", at(f.targets[0].span()), exprs(&f.targets))
            };
            let else_ = f.else_body.as_deref().map(list).unwrap_or("~".into());
            format!(
                "{name}{}({}, {target}, {}, {else_})",
                at(f.span),
                expr(&f.iter),
                list(&f.body)
            )
        }
        Node::Set(s) => {
            let (value, body) = match &s.value {
                SetValue::Expr(e) => (expr(e), "~".to_string()),
                SetValue::Capture(c) => ("~".to_string(), capture(c)),
            };
            format!(
                "Set{}([{}], {value}, {body})",
                at(s.span),
                exprs(&s.targets)
            )
        }
        Node::Block(b) => format!(
            "Block{}(Symbol{}({}), {})",
            at(b.span),
            at(b.name_span),
            json(&b.name),
            list(&b.body)
        ),
        Node::Extends(e) => format!("Extends{}({})", at(e.span), expr(&e.template)),
        Node::Include(i) => format!(
            "Include{}({}, {})",
            at(i.span),
            expr(&i.template),
            i.ignore_missing
        ),
        Node::Macro(m) => format!(
            "Macro{}({}, {}, {})",
            at(m.span),
            expr(&m.name),
            args(&m.args),
            list(&m.body)
        ),
        Node::Import(i) => format!(
            "Import{}({}, {}, {})",
            at(i.span),
            expr(&i.template),
            expr(&i.target),
            flag(i.with_context)
        ),
        Node::FromImport(f) => {
            let names: Vec<String> = f
                .names
                .iter()
                .map(|n| match &n.alias {
                    None => expr(&n.name),
                    Some(a) => format!("Pair{}({}, {})", at(n.span), expr(&n.name), expr(a)),
                })
                .collect();
            format!(
                "FromImport{}({}, [{}], {})",
                at(f.span),
                expr(&f.template),
                names.join(" "),
                flag(f.with_context)
            )
        }
        Node::Switch(s) => {
            let cases: Vec<String> = s
                .cases
                .iter()
                .map(|c| format!("Case({}, {})", expr(&c.cond), list(&c.body)))
                .collect();
            let default = s.default.as_deref().map(list).unwrap_or("~".into());
            format!(
                "Switch{}({}, [{}], {default})",
                at(s.span),
                expr(&s.expr),
                cases.join(" ")
            )
        }
    }
}

fn capture(c: &Capture) -> String {
    format!("Capture{}({})", at(c.span), list(&c.body))
}

fn literal(v: &Literal) -> String {
    match v {
        Literal::String(s) => json(s),
        Literal::Number(n) => brrrty_js::js_number_to_string(*n),
        Literal::Bool(b) => b.to_string(),
        Literal::Null => "null".into(),
        Literal::Regex { flags, .. } => {
            // `RegExp.prototype.flags` lists flags in this order.
            let sorted: String = "dgimsuvy".chars().filter(|c| flags.contains(*c)).collect();
            format!("Regex({})", json(&sorted))
        }
    }
}

fn binary(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Or => "Or",
        BinaryOp::And => "And",
        BinaryOp::In => "In",
        BinaryOp::Is => "Is",
        BinaryOp::Add => "Add",
        BinaryOp::Concat => "Concat",
        BinaryOp::Sub => "Sub",
        BinaryOp::Mul => "Mul",
        BinaryOp::Div => "Div",
        BinaryOp::FloorDiv => "FloorDiv",
        BinaryOp::Mod => "Mod",
        BinaryOp::Pow => "Pow",
    }
}

fn expr(e: &Expr) -> String {
    match e {
        Expr::Literal { span, value } => format!("Literal{}({})", at(*span), literal(value)),
        Expr::Symbol { span, name } => format!("Symbol{}({})", at(*span), json(name)),
        Expr::Group { span, items } => format!("Group{}({})", at(*span), exprs(items)),
        Expr::Array { span, items } => format!("Array{}({})", at(*span), exprs(items)),
        Expr::Dict { span, pairs } => {
            let p: Vec<String> = pairs.iter().map(pair).collect();
            format!("Dict{}({})", at(*span), p.join(" "))
        }
        Expr::Lookup { span, target, key } => {
            format!("LookupVal{}({}, {})", at(*span), expr(target), expr(key))
        }
        Expr::Call {
            span,
            callee,
            args: a,
        } => {
            format!("FunCall{}({}, {})", at(*span), expr(callee), args(a))
        }
        Expr::Filter {
            span,
            name,
            args: a,
        } => format!(
            "Filter{}(Symbol{}({}), {})",
            at(*span),
            at(*span),
            json(name),
            args(a)
        ),
        Expr::InlineIf {
            span,
            cond,
            body,
            else_,
        } => format!(
            "InlineIf{}({}, {}, {})",
            at(*span),
            expr(cond),
            expr(body),
            opt(else_.as_deref())
        ),
        Expr::Binary { span, op, lhs, rhs } => {
            format!("{}{}({}, {})", binary(*op), at(*span), expr(lhs), expr(rhs))
        }
        Expr::Unary { span, op, operand } => {
            let name = match op {
                UnaryOp::Not => "Not",
                UnaryOp::Neg => "Neg",
                UnaryOp::Pos => "Pos",
            };
            format!("{name}{}({})", at(*span), expr(operand))
        }
        Expr::Compare { span, expr: e, ops } => {
            let o: Vec<String> = ops
                .iter()
                .map(|o| {
                    format!(
                        "CompareOperand{}({}, {})",
                        at(o.span),
                        expr(&o.expr),
                        json(o.op.as_str())
                    )
                })
                .collect();
            format!("Compare{}({}, [{}])", at(*span), expr(e), o.join(" "))
        }
        Expr::Caller {
            span,
            args: a,
            body,
        } => format!(
            "Caller{}(Symbol{}(\"caller\"), {}, {})",
            at(*span),
            at(*span),
            args(a),
            list(body)
        ),
        Expr::Capture(c) => capture(c),
        Expr::Missing { .. } => "~".into(),
    }
}

#[test]
fn parse_probes_match_nunjucks() {
    let probes: Vec<ParseProbe> = serde_json::from_str(include_str!("probes/parse.json")).unwrap();
    assert!(probes.len() >= 150, "{} probes", probes.len());
    check(&probes);
}

/// Checks a probe file made with `gen-parse-probes.cjs` from other
/// sources, such as random templates:
///
///   NUNJUCKS_PROBES=/path/to/probes.json cargo test -p brrrty-nunjucks \
///     --test probes -- --ignored
#[test]
#[ignore = "needs NUNJUCKS_PROBES"]
fn probes_from_a_file_match_nunjucks() {
    let path = std::env::var("NUNJUCKS_PROBES").expect("set NUNJUCKS_PROBES");
    let text = std::fs::read_to_string(path).unwrap();
    let probes: Vec<ParseProbe> = serde_json::from_str(&text).unwrap();
    check(&probes);
    eprintln!("{} probes match", probes.len());
}

fn check(probes: &[ParseProbe]) {
    let mut failures = Vec::new();
    for p in probes {
        let got = parse("probe", &p.src);
        if UNSUPPORTED.contains(&p.src.as_str()) {
            match got {
                Err(e) if e.kind == ErrorKind::Unsupported => {}
                other => failures.push(format!("{:?}: want Unsupported, got {other:?}", p.src)),
            }
            continue;
        }
        match (&got, p.ok) {
            (Ok(t), true) => {
                let mine = list(&t.nodes);
                let want = p.ast.as_deref().unwrap();
                if mine != want {
                    failures.push(format!("{:?}:\n  want {want}\n  got  {mine}", p.src));
                }
            }
            (Err(e), false) => {
                let want = (p.msg.as_deref().unwrap(), p.line, p.col);
                if (e.message.as_str(), e.line, e.column) != want {
                    failures.push(format!(
                        "{:?}: want {want:?}, got {:?}",
                        p.src,
                        (&e.message, e.line, e.column)
                    ));
                }
            }
            (Ok(_), false) => failures.push(format!(
                "{:?}: Nunjucks fails ({:?}), we parse",
                p.src, p.msg
            )),
            (Err(e), true) => failures.push(format!("{:?}: Nunjucks parses, we fail: {e}", p.src)),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {}:\n{}",
        failures.len(),
        probes.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
