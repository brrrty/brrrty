//! The shape of the AST, spans and errors.

use brrrty_liquid::ast::*;
use brrrty_liquid::{ErrorKind, parse};

fn nodes(src: &str) -> Vec<Node> {
    parse("t.liquid", src)
        .unwrap_or_else(|e| panic!("{src:?}: {e}"))
        .nodes
}

fn tag(node: &Node) -> &Tag {
    match node {
        Node::Tag(t) => t,
        other => panic!("not a tag: {other:?}"),
    }
}

fn text(node: &Node) -> &Text {
    match node {
        Node::Text(t) => t,
        other => panic!("not text: {other:?}"),
    }
}

fn output(node: &Node) -> &Output {
    match node {
        Node::Output(o) => o,
        other => panic!("not an output: {other:?}"),
    }
}

/// An expression as a short string, for comparing tree shapes.
fn show(e: &Expr) -> String {
    match e {
        Expr::Literal { value, .. } => match value {
            Literal::True => "true".into(),
            Literal::False => "false".into(),
            Literal::Nil => "nil".into(),
            Literal::Empty => "empty".into(),
            Literal::Blank => "blank".into(),
            Literal::Number(n) => format!("{n}"),
            Literal::String(s) => format!("{s:?}"),
        },
        Expr::Path { root, segments, .. } => {
            let mut s = root
                .as_ref()
                .map(|r| format!("({})", show(r)))
                .unwrap_or_default();
            for (i, seg) in segments.iter().enumerate() {
                match seg {
                    Segment::Key { name, .. } if i == 0 && root.is_none() => s.push_str(name),
                    Segment::Key { name, .. } => s.push_str(&format!(".{name}")),
                    Segment::Index(e) => s.push_str(&format!("[{}]", show(e))),
                }
            }
            s
        }
        Expr::Range { start, end, .. } => format!("({}..{})", show(start), show(end)),
        Expr::Binary { op, lhs, rhs, .. } => format!("({} {op:?} {})", show(lhs), show(rhs)),
        Expr::Not { operand, .. } => format!("(not {})", show(operand)),
        Expr::Missing { .. } => "_".into(),
    }
}

fn if_condition(src: &str) -> String {
    let n = nodes(src);
    match &tag(&n[0]).kind {
        TagKind::If { branches, .. } => show(&branches[0].condition.expr),
        other => panic!("{other:?}"),
    }
}

#[test]
fn whitespace_control_trims_text_and_keeps_spans() {
    let n = nodes("a  {{- x -}}  b");
    assert_eq!(n.len(), 3);
    let a = text(&n[0]);
    assert_eq!(a.text, "a");
    assert_eq!(
        (a.span.start, a.span.end, a.span.line, a.span.column),
        (0, 3, 1, 1)
    );
    let o = output(&n[1]);
    assert_eq!((o.span.start, o.span.end, o.span.column), (3, 12, 4));
    assert_eq!(
        o.trim,
        Trim {
            left: true,
            right: true
        }
    );
    let b = text(&n[2]);
    assert_eq!(b.text, "b");
    assert_eq!((b.span.start, b.span.end, b.span.column), (12, 15, 13));

    let n = nodes("x\n  {%- if a -%}\n y{% endif %}\n");
    assert_eq!(text(&n[0]).text, "x");
    let t = tag(&n[1]);
    assert_eq!((t.span.line, t.span.column), (2, 3));
    assert_eq!(
        t.trim,
        Trim {
            left: true,
            right: true
        }
    );
    let TagKind::If { branches, .. } = &t.kind else {
        panic!()
    };
    assert_eq!(text(&branches[0].body[0]).text, "y");
    assert_eq!(text(&n[2]).text, "\n");
}

#[test]
fn trim_reaches_across_newlines_but_not_past_tokens() {
    let n = nodes("a\n\n{%- # c -%}\n\nb");
    assert_eq!(text(&n[0]).text, "a");
    assert_eq!(text(&n[2]).text, "b");
    // All-blank text between two trimming outputs becomes empty.
    let n = nodes("{{ 'a' -}}  \n  {{- 'b' }}");
    assert_eq!(text(&n[1]).text, "");
}

#[test]
fn raw_keeps_endraw_lookalikes() {
    let n = nodes("{% raw %}{% endrawx %}{%- endraw %}{{ x {% endraw %}after");
    let TagKind::Raw { text: body } = &tag(&n[0]).kind else {
        panic!()
    };
    assert_eq!(body, "{% endrawx %}{%- endraw %}{{ x ");
    assert_eq!(text(&n[1]).text, "after");
    // `endraw` must come right after `{%` and blanks.
    let e = parse("t", "{% raw %}x{%- endraw %}").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unclosed);
    assert_eq!(e.message, r#"raw "{% raw %}x{%- endraw %}" not closed"#);
    assert!(e.help.unwrap().contains("{% endraw %}"));
}

#[test]
fn unclosed_if_reports_the_opening_tag() {
    let e = parse(
        "page.liquid",
        "a\nb\n  {% if x %}\n{% for i in y %}{% endfor %}\nz",
    )
    .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unclosed);
    assert_eq!(e.message, "tag {% if x %} not closed");
    assert_eq!((e.line, e.column), (3, 3));
    assert_eq!(e.template, "page.liquid");
    assert_eq!(e.help.as_deref(), Some("add `{% endif %}` to close it"));
    assert_eq!(
        e.to_string(),
        "page.liquid:3:3: tag {% if x %} not closed\nhelp: add `{% endif %}` to close it"
    );
}

fn nested_ifs(depth: usize) -> String {
    format!(
        "{}x{}",
        "{% if true %}".repeat(depth),
        "{% endif %}".repeat(depth)
    )
}

#[test]
fn deep_nesting_is_an_error_not_a_stack_overflow() {
    assert!(parse("t", &nested_ifs(256)).is_ok());
    for depth in [257, 500, 100_000] {
        let e = parse("t", &nested_ifs(depth)).unwrap_err();
        assert_eq!(e.kind, ErrorKind::TooDeep);
        assert_eq!(
            e.message,
            "templates nest too deeply (more than 256 levels)"
        );
        assert_eq!((e.line, e.column), (1, 256 * 13 + 1));
    }
    for src in [
        format!("{{{{ {}a{} }}}}", "a[".repeat(500), "]".repeat(500)),
        format!("{{{{ {}1..2{} }}}}", "(".repeat(500), ")".repeat(500)),
    ] {
        let e = parse("t", &src).unwrap_err();
        assert_eq!(e.kind, ErrorKind::TooDeep);
        assert_eq!(
            e.message,
            "expressions nest too deeply (more than 256 levels)"
        );
    }
    assert!(
        parse(
            "t",
            &format!("{{{{ {}a{} }}}}", "a[".repeat(200), "]".repeat(200))
        )
        .is_ok()
    );
}

#[test]
fn operators_group_like_liquidjs() {
    // Equal precedence groups to the right; pinned by rendering in LiquidJS:
    // `false and true or true` is false.
    assert_eq!(
        if_condition("{% if false and true or true %}{% endif %}"),
        "(false And (true Or true))"
    );
    assert_eq!(
        if_condition("{% if a == 1 or b contains 'x' %}{% endif %}"),
        "((a Eq 1) Or (b Contains \"x\"))"
    );
    assert_eq!(
        if_condition("{% if not a == b %}{% endif %}"),
        "(not (a Eq b))"
    );
    // `1 == not 2` renders false in LiquidJS: `==` runs with one operand.
    assert_eq!(if_condition("{% if 1 == not 2 %}{% endif %}"), "(_ Eq 1)");
    assert_eq!(if_condition("{% if == %}{% endif %}"), "(_ Eq _)");
    // Extra operands are dropped: `false true` is false.
    assert_eq!(if_condition("{% if false true %}{% endif %}"), "false");
    assert_eq!(if_condition("{% if a<>b %}{% endif %}"), "(_ Lt (a Gt b))");
    // Word operators need a boundary.
    assert_eq!(if_condition("{% if order %}{% endif %}"), "order");
    assert_eq!(if_condition("{% if a containsb %}{% endif %}"), "a");
}

#[test]
fn values_and_paths() {
    let n = nodes(
        "{{ a.b[\"c\"][d.e][] }}{{ 'x'.size }}{{ (1..n).last }}{{ [k] }}{{ 1.foo }}{{ a-b? }}\
         {{ 1. }}{{ +5 }}{{ -0 }}{{ 007 }}{{ 1.50 }}{{ \"a\\u0041\\x\\101\\n\" }}{{ nil }}{{ null }}",
    );
    let shown: Vec<String> = n.iter().map(|n| show(&output(n).value.expr)).collect();
    assert_eq!(
        shown,
        [
            "a.b[\"c\"][d.e].",
            "(\"x\").size",
            "((1..n)).last",
            "[k]",
            "1.foo",
            "a-b?",
            "1",
            "5",
            "-0",
            "7",
            "1.5",
            "\"aAxA\\n\"",
            "nil",
            "nil",
        ]
    );
    let Expr::Literal {
        value: Literal::Number(z),
        ..
    } = &output(&n[8]).value.expr
    else {
        panic!()
    };
    assert!(z.is_sign_negative());
}

#[test]
fn filters_with_positional_and_keyword_arguments() {
    let n = nodes("{{ a | f: 1, k: 'v', 'q': b | g | h: }}");
    let v = &output(&n[0]).value;
    assert_eq!(v.text, "a | f: 1, k: 'v', 'q': b | g | h:");
    let names: Vec<&str> = v.filters.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["f", "g", "h"]);
    let args = &v.filters[0].args;
    assert!(matches!(&args[0], FilterArg::Positional(e) if show(e) == "1"));
    assert!(
        matches!(&args[1], FilterArg::Keyword { name, value: Some(e) } if name == "k" && show(e) == "\"v\"")
    );
    assert!(
        matches!(&args[2], FilterArg::Keyword { name, value: Some(e) } if name == "'q'" && show(e) == "b")
    );
    assert!(v.filters[1].args.is_empty());
    assert!(v.filters[2].args.is_empty());
}

#[test]
fn for_and_tablerow() {
    let n = nodes("{% for i in (1..3) reversed limit: 2 offset:x %}a{% else %}b{% endfor %}");
    let TagKind::For(f) = &tag(&n[0]).kind else {
        panic!()
    };
    assert_eq!(f.variable, "i");
    assert_eq!(show(&f.collection), "(1..3)");
    assert_eq!(f.collection_text, "(1..3)");
    let args: Vec<(String, Option<String>)> = f
        .args
        .iter()
        .map(|a| (a.name.clone(), a.value.as_ref().map(show)))
        .collect();
    assert_eq!(
        args,
        [
            ("reversed".to_string(), None),
            ("limit".to_string(), Some("2".to_string())),
            ("offset".to_string(), Some("x".to_string())),
        ]
    );
    assert_eq!(text(&f.body[0]).text, "a");
    assert_eq!(text(&f.else_body[0]).text, "b");

    let n = nodes("{% tablerow p in site.pages cols:2 %}{{ p }}{% endtablerow %}");
    let TagKind::Tablerow(t) = &tag(&n[0]).kind else {
        panic!()
    };
    assert_eq!(
        (t.variable.as_str(), t.collection_text.as_str()),
        ("p", "site.pages")
    );
    assert_eq!(t.args[0].name, "cols");
}

#[test]
fn branches_of_if_unless_and_case() {
    let n = nodes("{% if a %}1{% elsif b %}2{% else %}3{% endif %}");
    let TagKind::If {
        branches,
        else_body,
    } = &tag(&n[0]).kind
    else {
        panic!()
    };
    assert_eq!(branches.len(), 2);
    assert_eq!(show(&branches[1].condition.expr), "b");
    assert_eq!(text(&else_body.as_ref().unwrap()[0]).text, "3");

    // After `else`, LiquidJS drops an `elsif` and everything after it.
    let n = nodes("{% unless a %}1{% elsif b %}2{% else %}3{% elsif c %}4{% endunless %}");
    let TagKind::Unless {
        branches,
        else_body,
    } = &tag(&n[0]).kind
    else {
        panic!()
    };
    assert_eq!(branches.len(), 2);
    assert_eq!(else_body.len(), 1);

    // Text before the first `when` and after a second `else` is dropped;
    // `when` after `else` is ignored.
    let n = nodes(
        "{% case x %}junk{% when 1, 2 or 3 %}a{% when 'z' 'skipped' %}b{% else %}c{% else %}d{% when 4 %}e{% endcase %}",
    );
    let TagKind::Case {
        value,
        whens,
        else_body,
    } = &tag(&n[0]).kind
    else {
        panic!()
    };
    assert_eq!(show(&value.expr), "x");
    assert_eq!(whens.len(), 2);
    let v: Vec<String> = whens[0].values.iter().map(show).collect();
    assert_eq!(v, ["1", "2", "3"]);
    let v: Vec<String> = whens[1].values.iter().map(show).collect();
    assert_eq!(v, ["\"z\""]);
    assert_eq!(else_body.len(), 1);
    assert_eq!(text(&else_body[0]).text, "c");
}

#[test]
fn simple_tags() {
    let n = nodes(
        "{% assign x = a | upcase %}{% capture 'q r' %}c{% endcapture %}{% increment n %}{% decrement %}\
         {% cycle 'g': 'a', 'b' %}{% cycle 'a', 'b' %}{% echo %}{% echo a | f %}{% break %}{% continue %}{%# note %}\
         {% comment %}{% nosuch %}{% endcomment %}{% block b1 %}x{% endblock %}",
    );
    let kinds: Vec<&TagKind> = n.iter().map(|n| &tag(n).kind).collect();
    assert!(
        matches!(kinds[0], TagKind::Assign { name, value } if name == "x" && value.filters[0].name == "upcase")
    );
    assert!(
        matches!(kinds[1], TagKind::Capture { name, body } if name == "q r" && body.len() == 1)
    );
    assert!(matches!(kinds[2], TagKind::Increment { name } if name == "n"));
    assert!(matches!(kinds[3], TagKind::Decrement { name } if name.is_empty()));
    let TagKind::Cycle { group, candidates } = kinds[4] else {
        panic!()
    };
    assert_eq!(group.as_ref().map(show).as_deref(), Some("\"g\""));
    let texts: Vec<&str> = candidates.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, [" 'a'", " 'b'"]);
    let TagKind::Cycle { group, candidates } = kinds[5] else {
        panic!()
    };
    assert!(group.is_none());
    assert_eq!(candidates.len(), 2);
    assert!(matches!(kinds[6], TagKind::Echo { value: None }));
    assert!(matches!(kinds[7], TagKind::Echo { value: Some(_) }));
    assert!(matches!(kinds[8], TagKind::Break));
    assert!(matches!(kinds[9], TagKind::Continue));
    assert!(matches!(kinds[10], TagKind::InlineComment));
    assert_eq!(tag(&n[10]).name, "#");
    assert!(matches!(kinds[11], TagKind::Comment));
    assert!(matches!(kinds[12], TagKind::Block { name, .. } if name == "b1"));
}

#[test]
fn partials() {
    let n = nodes(
        "{% include 'a.liquid', x: 1 %}{% include name with v %}{% include none %}\
         {% include 'p-{{ n }}.liquid' %}{% render 'r' with item as p, k: 2 %}{% render 'r' for xs as x %}",
    );
    let TagKind::Include(i) = &tag(&n[0]).kind else {
        panic!()
    };
    assert_eq!(i.file, FilePath::Name("a.liquid".into()));
    assert_eq!(i.args[0].name, "x");
    let TagKind::Include(i) = &tag(&n[1]).kind else {
        panic!()
    };
    assert!(matches!(&i.file, FilePath::Expr(e) if show(e) == "name"));
    assert_eq!(i.with.as_ref().map(show).as_deref(), Some("v"));
    let TagKind::Include(i) = &tag(&n[2]).kind else {
        panic!()
    };
    assert_eq!(i.file, FilePath::None);
    let TagKind::Include(i) = &tag(&n[3]).kind else {
        panic!()
    };
    let FilePath::Template(parts) = &i.file else {
        panic!("{:?}", i.file)
    };
    assert_eq!(parts.len(), 3);
    // Nodes inside a quoted name point at the name in the template.
    assert_eq!(parts[1].span(), parts[0].span());
    assert_eq!(parts[1].span().column, tag(&n[3]).span.column + 11);
    let TagKind::Render(r) = &tag(&n[4]).kind else {
        panic!()
    };
    let w = r.with.as_ref().unwrap();
    assert_eq!(
        (show(&w.value), w.alias.as_deref()),
        ("item".into(), Some("p"))
    );
    assert_eq!(r.args[0].name, "k");
    let TagKind::Render(r) = &tag(&n[5]).kind else {
        panic!()
    };
    let f = r.for_binding.as_ref().unwrap();
    assert_eq!((f.text.as_str(), f.alias.as_deref()), ("xs", Some("x")));

    // An error inside a quoted name points at the name.
    let e = parse("t", "ab {% include 'a{{ b' %}").unwrap_err();
    assert_eq!(e.message, "output \"{{ b\" not closed");
    assert_eq!((e.line, e.column), (1, 15));
}

#[test]
fn liquid_tag_lines() {
    let n = nodes("{% liquid\n  assign x = 1\n  if x\n    echo 'a'\n  endif\n  # note\n%}");
    let TagKind::Liquid { body } = &tag(&n[0]).kind else {
        panic!()
    };
    let names: Vec<&str> = body.iter().map(|n| tag(n).name.as_str()).collect();
    assert_eq!(names, ["assign", "if", "#"]);
    assert_eq!((body[1].span().line, body[1].span().column), (3, 3));
    let e = parse("t", "{% liquid\nif x\n%}").unwrap_err();
    assert_eq!(e.message, "tag if x not closed");
    assert_eq!((e.line, e.column), (2, 1));
}

#[test]
fn unknown_tags_name_the_fix() {
    let e = parse("t", "{% if n == 1 %}one{% elseif n == 2 %}two{% endif %}").unwrap_err();
    assert_eq!(e.kind, ErrorKind::UnknownTag);
    assert_eq!(e.message, "tag \"elseif\" not found");
    assert_eq!((e.line, e.column), (1, 19));
    assert_eq!(e.help.as_deref(), Some("did you mean `elsif`?"));
    let e = parse("t", "{% endif %}").unwrap_err();
    assert_eq!(
        e.help.as_deref(),
        Some("there is no open `{% if %}` for this `{% endif %}` to close")
    );
    let e = parse("t", "{% else %}").unwrap_err();
    assert_eq!(
        e.help.as_deref(),
        Some("`{% else %}` only works inside if, unless, case or for")
    );
    let e = parse("t", "{% frobnicate %}").unwrap_err();
    assert_eq!(e.help, None);
}

#[test]
fn errors_come_from_the_tokenizer_first() {
    // LiquidJS reads every token before parsing any, so an unclosed output
    // later on wins over an unknown tag earlier.
    let e = parse("t", "{% nosuch %}{{ x").unwrap_err();
    assert_eq!(e.message, "output \"{{ x\" not closed");
    assert_eq!((e.line, e.column), (1, 13));
}

#[test]
fn columns_count_utf16_units() {
    let e = parse("t", "é😀\n😀{% nosuch %}").unwrap_err();
    assert_eq!((e.line, e.column), (2, 3));
}

#[test]
fn never_panics_on_fragment_soup() {
    let parts = [
        "{%",
        "%}",
        "{{",
        "}}",
        "-",
        " ",
        "\n",
        "if",
        "endif",
        "raw",
        "endraw",
        "for i in",
        "(",
        ")",
        "..",
        "[",
        "]",
        "'",
        "\"",
        "\\",
        "|",
        ":",
        ",",
        "a",
        "1",
        ".",
        "é",
        "😀",
        "\u{a0}",
        "liquid",
        "#",
        "comment",
        "endcomment",
        "case",
        "when",
        "include",
        "render",
        "cycle",
        "==",
        "not",
        "or",
    ];
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    for _ in 0..20_000 {
        let mut src = String::new();
        for _ in 0..(seed % 24) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            src.push_str(parts[(seed % parts.len() as u64) as usize]);
        }
        let _ = parse("t", &src);
        seed = seed.wrapping_add(1);
    }
}

/// Runs `f` on a thread with a 2 MiB stack, the size Rust gives spawned
/// threads by default.
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn long_operator_chains_are_capped_not_overflowed() {
    for src in [
        format!("{{% if a{} %}}{{% endif %}}", " and a".repeat(20_000)),
        format!("{{% if a{} %}}{{% endif %}}", " == a".repeat(20_000)),
        format!("{{% if {}a %}}{{% endif %}}", "not ".repeat(20_000)),
    ] {
        let e = on_small_stack(move || parse("t", &src).unwrap_err());
        assert_eq!(e.kind, ErrorKind::TooDeep);
        assert_eq!(
            e.message,
            "expressions nest too deeply (more than 256 levels)"
        );
    }
    // 256 levels of operators still parse.
    let ok = format!("{{% if a{} %}}{{% endif %}}", " and a".repeat(256));
    assert!(on_small_stack(move || parse("t", &ok).is_ok()));
    let deep = format!("{{% if a{} %}}{{% endif %}}", " and a".repeat(257));
    assert!(on_small_stack(move || parse("t", &deep).is_err()));
}

#[test]
fn spans_on_one_long_line_are_linear() {
    let src = format!(
        "é{{% if a %}}{}{{% endif %}}{{{{ x{} }}}}",
        "{% elsif a %}x".repeat(20_000),
        " | f".repeat(20_000)
    );
    let start = std::time::Instant::now();
    let t = parse("t", &src).unwrap();
    assert!(
        start.elapsed().as_secs_f64() < 1.0,
        "took {:?}",
        start.elapsed()
    );
    let TagKind::If { branches, .. } = &tag(&t.nodes[1]).kind else {
        panic!()
    };
    assert_eq!(branches.len(), 20_001);
    // "é" is one UTF-16 unit, `{% if a %}` is 10 and each `{% elsif a %}x` is 14.
    assert_eq!(branches[20_000].span.column, 2 + 10 + 14 * 19_999);

    let src = format!(
        "{{% case a %}}{}{{% endcase %}}",
        "{% when 1, 2 or 3 %}x".repeat(20_000)
    );
    let start = std::time::Instant::now();
    parse("t", &src).unwrap();
    assert!(
        start.elapsed().as_secs_f64() < 1.0,
        "took {:?}",
        start.elapsed()
    );
}
