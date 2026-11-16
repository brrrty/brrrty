//! The shape of the tree, spans, safety bounds and plain text.

use brrrty_nunjucks::ast::*;
use brrrty_nunjucks::{ErrorKind, parse};

fn nodes(src: &str) -> Vec<Node> {
    parse("t.njk", src)
        .unwrap_or_else(|e| panic!("{src:?}: {e}"))
        .nodes
}

fn output_expr(src: &str) -> Expr {
    match nodes(src).remove(0) {
        Node::Output(o) => o.expr,
        other => panic!("not an output: {other:?}"),
    }
}

/// A large script in the style of a JavaScript library: braces, `}}`,
/// `%}`, `#` and `{ {`, but no `{{`, `{%`, `{#` or `#}`.
fn script(lines: usize) -> String {
    let body = [
        "lunr.Index.prototype.query = function (fn) { var q = new lunr.Query(this.fields); return {a: {b: q}} }",
        "  var s = '}}', t = \"%}\", u = '{ {', h = '#', re = /[{}]+/g, m = 7 % 3",
        "  if (x) { y({ z: [1, 2, { w: 3 }] }) } else { /* } { */ }",
        "  obj[\"k\" + i] = { \"v\": i, 'é': '😀' } // # { }",
        "",
    ];
    (0..lines)
        .map(|i| body[i % body.len()])
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_large_script_is_one_text_node() {
    let src = script(20_000);
    let n = nodes(&src);
    assert_eq!(n.len(), 1);
    match &n[0] {
        Node::Text(t) => {
            assert_eq!(t.text, src);
            assert_eq!((t.span.start, t.span.end), (0, src.len()));
            assert_eq!((t.span.line, t.span.column), (1, 1));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn delimiters_inside_script_strings_are_still_nunjucks() {
    // Nunjucks reads `{{` and `{#` wherever they are, strings included.
    let e = parse("t.njk", &(script(100) + "\nvar s = \"{{\";\n")).unwrap_err();
    assert_eq!(e.message, "expected variable end");
    let e = parse("t.njk", &(script(100) + "\nvar s = '{#';\n")).unwrap_err();
    assert_eq!(e.message, "Error: expected end of comment, got end of file");
    assert_eq!((e.line, e.column), (None, None));
    let e = parse("t.njk", "a = '#}'").unwrap_err();
    assert_eq!(e.message, "Error: unexpected end of comment");
    // A complete comment in a string is a comment.
    let n = nodes("var s = '{# c #}';");
    assert_eq!(n.len(), 2);
}

#[test]
fn text_spans_count_utf16_columns_and_lines() {
    let n = nodes("😀é\n\tx {{ y }}");
    let Node::Output(o) = &n[1] else { panic!() };
    assert_eq!((o.span.line, o.span.column), (2, 4));
    let Expr::Symbol { span, .. } = &o.expr else {
        panic!()
    };
    assert_eq!((span.line, span.column), (2, 7));
    assert_eq!(&"😀é\n\tx {{ y }}"[span.start..span.end], "y");
}

#[test]
fn crlf_counts_the_carriage_return_as_a_column() {
    let e = parse("t.njk", "a\r\nb\r{% nope %}").unwrap_err();
    assert_eq!((e.line, e.column), (Some(2), Some(6)));
}

#[test]
fn elif_chains_are_flat() {
    let n = nodes("{% if a %}1{% elif b %}2{% elseif c %}3{% else %}4{% endif %}");
    let Node::If(i) = &n[0] else { panic!() };
    assert_eq!(i.branches.len(), 3);
    assert_eq!(i.else_body.as_ref().unwrap().len(), 1);
    assert_eq!(i.branches[1].span.column, 15);
}

#[test]
fn many_elifs_do_not_nest() {
    let src = format!("{{% if a %}}{}{{% endif %}}", "{% elif b %}x".repeat(5_000));
    let n = nodes(&src);
    let Node::If(i) = &n[0] else { panic!() };
    assert_eq!(i.branches.len(), 5_001);
}

#[test]
fn regex_literals_keep_pattern_and_flags_as_written() {
    let e = output_expr(r"{{ r/a\/b[}]/mg }}");
    let Expr::Literal {
        value: Literal::Regex { pattern, flags },
        ..
    } = e
    else {
        panic!("{e:?}")
    };
    assert_eq!(pattern, r"a\/b[}]");
    assert_eq!(flags, "mg");
}

#[test]
fn eleventy_tags_are_refused_as_unsupported() {
    for src in [
        "{% setAsync \"x\" %}a{% endsetAsync %}",
        "{% getBundle \"css\" %}",
        "{% getBundleFileUrl \"css\" %}",
    ] {
        let e = parse("t.njk", src).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Unsupported, "{src}");
        assert_eq!((e.line, e.column), (Some(1), Some(4)), "{src}");
        assert!(e.help.is_some());
    }
    // A stray end tag is unknown, as in 11ty.
    let e = parse("t.njk", "{% endsetAsync %}").unwrap_err();
    assert_eq!(e.message, "unknown block tag: endsetAsync");
}

#[test]
fn missing_parts_are_kept_for_the_compiler_to_reject() {
    let e = output_expr("{{ ,(1) }}");
    let Expr::Call { callee, .. } = e else {
        panic!()
    };
    assert!(matches!(*callee, Expr::Missing { .. }));
    let e = output_expr("{{ a[] }}");
    let Expr::Lookup { key, .. } = e else {
        panic!()
    };
    assert!(matches!(*key, Expr::Missing { .. }));
}

#[test]
fn error_display_has_template_line_and_column() {
    let e = parse("page.njk", "\n{% nope %}").unwrap_err();
    assert_eq!(e.to_string(), "page.njk:2:4: unknown block tag: nope");
    let e = parse("page.njk", "{% if x %}").unwrap_err();
    assert_eq!(
        e.to_string(),
        "page.njk: parseIf: expected elif, else, or endif, got end of file"
    );
}

fn too_deep(src: &str) {
    let e = parse("t.njk", src).unwrap_err();
    assert_eq!(e.kind, ErrorKind::TooDeep, "{e}");
}

/// Runs `f` on a thread with a small stack, as a build's worker threads
/// might have.
fn on_small_stack(f: fn()) {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn deep_nesting_is_refused_not_overflowed() {
    on_small_stack(deep_nesting);
}

#[test]
fn long_chains_are_refused_not_overflowed() {
    on_small_stack(long_chains);
}

fn deep_nesting() {
    let n = 100_000;
    too_deep(&format!("{{{{ {}x }}}}", "(".repeat(n)));
    too_deep(&format!("{{{{ {}x }}}}", "[".repeat(n)));
    too_deep(&format!("{{{{ {}x }}}}", "{a: ".repeat(n)));
    too_deep(&format!("{{{{ {}x }}}}", "not ".repeat(n)));
    too_deep(&format!("{{{{ {}x }}}}", "- ".repeat(n)));
    too_deep(&format!("{{{{ f{} }}}}", "(".repeat(n)));
    too_deep(&"{% if a %}".repeat(n));
    too_deep(&"{% for a in b %}".repeat(n));
    too_deep(&"{% call m() %}".repeat(n));
    too_deep(&"{% filter f %}".repeat(n));
    // 256 levels of tags are allowed, and 200 of parentheses.
    let src = format!("{}{}", "{% if a %}".repeat(255), "{% endif %}".repeat(255));
    assert!(parse("t.njk", &src).is_ok());
    let src = format!("{{{{ {}x{} }}}}", "(".repeat(200), ")".repeat(200));
    assert!(parse("t.njk", &src).is_ok());
}

fn long_chains() {
    let n = 100_000;
    let chain = |op: &str| format!("{{{{ a{} }}}}", format!(" {op} a").repeat(n));
    for op in [
        "+", "-", "*", "/", "//", "%", "**", "~", "and", "or", "in", "|",
    ] {
        let src = if op == "|" {
            format!("{{{{ a{} }}}}", " | f".repeat(n))
        } else {
            chain(op)
        };
        too_deep(&src);
    }
    too_deep(&format!("{{{{ a{} }}}}", ".b".repeat(n)));
    too_deep(&format!("{{{{ a{} }}}}", "[0]".repeat(n)));
    too_deep(&format!("{{{{ a{} }}}}", "()".repeat(n)));
    // Flat lists are not deep.
    let items = vec!["1"; n].join(", ");
    assert!(parse("t.njk", &format!("{{{{ [{items}] }}}}")).is_ok());
    assert!(parse("t.njk", &format!("{{{{ a {} }}}}", "== a ".repeat(n))).is_ok());
    // 256 levels are allowed.
    assert!(parse("t.njk", &format!("{{{{ a{} }}}}", " + a".repeat(200))).is_ok());
    assert!(
        parse(
            "t.njk",
            &format!("{{{{ {}x{} }}}}", "(".repeat(200), ")".repeat(200))
        )
        .is_ok()
    );
}

#[test]
fn hostile_inputs_do_not_panic() {
    let pieces = [
        "{{", "}}", "{%", "%}", "{#", "#}", "-", "(", ")", "[", "]", "{", "}", ",", ":", "|", "~",
        ".", "'", "\"", "\\", "r/", "/", "if", "endif", "for", "in", "raw", "endraw", "set", "=",
        "call", "macro", "from", "import", "switch", "case", "not", "is", " ", "\n", "\r", "😀",
        "é", "1", "1.", "x",
    ];
    // A fixed pseudo-random walk over the pieces.
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    for _ in 0..20_000 {
        let mut src = String::new();
        for _ in 0..(seed % 24) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            src.push_str(pieces[(seed % pieces.len() as u64) as usize]);
        }
        let _ = parse("t.njk", &src);
        seed = seed.wrapping_add(0x2545_f491_4f6c_dd1d);
    }
}

/// Runs `f` on a thread with a 2 MiB stack, the default for spawned
/// threads.
fn on_default_stack(f: fn()) {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn deep_dict_keys_are_refused_not_overflowed() {
    on_default_stack(|| {
        let keys = "{".repeat(10_000);
        for src in [
            format!("{{{{ {keys} }}}}"),
            format!("{{{{ {{{keys} }}}}"),
            format!("{{% set {keys} = 1 %}}"),
            format!("{{% for {keys} in x %}}{{% endfor %}}"),
            format!("{{% for a, {keys} in x %}}{{% endfor %}}"),
            format!("{{% macro {keys}() %}}{{% endmacro %}}"),
            format!("{{% call {keys} %}}{{% endcall %}}"),
            format!("{{% from \"m\" import {keys} %}}"),
            format!("{{% block {keys} %}}{{% endblock %}}"),
            format!("{{{{ a | f({keys}) }}}}"),
            format!("{{{{ a[{keys}] }}}}"),
        ] {
            let e = parse("t.njk", &src).unwrap_err();
            assert_eq!(e.kind, ErrorKind::TooDeep, "{}: {e}", &src[..12]);
        }
    });
}
