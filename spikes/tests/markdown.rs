use brrrty_markdown::MarkdownRenderer;
use brrrty_spike::engines::markdown::{Comrak, ComrakFormatted, MarkdownItRs, Pulldown};

fn all() -> Vec<(&'static str, Box<dyn MarkdownRenderer>)> {
    vec![
        ("comrak", Box::new(Comrak::new())),
        ("comrak-formatted", Box::new(ComrakFormatted::new())),
        ("pulldown", Box::new(Pulldown)),
        ("markdown-it", Box::new(MarkdownItRs::new())),
    ]
}

#[test]
fn heading_and_paragraph() {
    for (n, r) in all() {
        assert_eq!(
            r.render("# Hello\n\nWorld\n"),
            "<h1>Hello</h1>\n<p>World</p>\n",
            "{n}"
        );
    }
}

#[test]
fn raw_html_block_passes_through() {
    for (n, r) in all() {
        let out = r.render("<div class=\"x\">hi</div>\n");
        assert!(out.contains("<div class=\"x\">hi</div>"), "{n}: {out}");
    }
}

#[test]
fn fenced_code_with_language() {
    for (n, r) in all() {
        let out = r.render("```js\nlet a = 1;\n```\n");
        assert!(
            out.contains("<pre><code class=\"language-js\">"),
            "{n}: {out}"
        );
    }
}

#[test]
fn tables() {
    for (n, r) in all() {
        let out = r.render("a | b\n--|--\n1 | 2\n");
        assert!(out.contains("<table>"), "{n}: {out}");
    }
}

#[test]
fn no_panic_on_edge_inputs() {
    let nested = ">".repeat(10_000);
    for (n, _) in all() {
        for input in ["", "\u{feff}x", nested.as_str()] {
            let input = input.to_string();
            let len = input.len();
            // Deep nesting can recurse; give the renderer a large stack like the corpus run does.
            let handle = std::thread::Builder::new()
                .stack_size(64 * 1024 * 1024)
                .spawn(move || {
                    let (_, r) = all().into_iter().find(|(name, _)| *name == n).unwrap();
                    let _ = r.render(&input);
                })
                .unwrap();
            assert!(
                handle.join().is_ok(),
                "{n} panicked on input of {len} bytes"
            );
        }
    }
}

/// 11ty disables markdown-it's `code` rule, so an indented line is paragraph text. Only the
/// markdown-it adapter (which omits `block::code`) can match; comrak and pulldown-cmark
/// have no public switch for it and render a code block.
#[test]
fn indented_code_is_a_paragraph_only_where_the_rule_can_be_disabled() {
    let src = "para\n\n    indented\n";
    for (n, r) in all() {
        let out = r.render(src);
        if n == "markdown-it" {
            assert_eq!(out, "<p>para</p>\n<p>indented</p>\n", "{n}");
        } else {
            assert_eq!(
                out, "<p>para</p>\n<pre><code>indented\n</code></pre>\n",
                "{n}"
            );
        }
    }
}

#[test]
fn comrak_formatted_uses_markdown_it_forms() {
    let r = ComrakFormatted::new();
    assert_eq!(r.render("a\n\n---\n\nb\n"), "<p>a</p>\n<hr>\n<p>b</p>\n");
    assert_eq!(r.render("~~x~~\n"), "<p><s>x</s></p>\n");
    assert_eq!(r.render("a  \nb\n"), "<p>a<br>\nb</p>\n");
    assert_eq!(
        r.render("![a b](/i.png \"t\")\n"),
        "<p><img src=\"/i.png\" alt=\"a b\" title=\"t\"></p>\n"
    );
    let table = r.render("a | b\n:-:|--:\n1 | 2\n");
    assert!(
        table.contains("<th style=\"text-align:center\">a</th>"),
        "{table}"
    );
    assert!(
        table.contains("<td style=\"text-align:right\">2</td>"),
        "{table}"
    );
}
