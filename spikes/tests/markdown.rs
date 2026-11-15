use brrrty_markdown::MarkdownRenderer;
use brrrty_spike::engines::markdown::{Comrak, MarkdownItRs, Pulldown};

fn all() -> Vec<(&'static str, Box<dyn MarkdownRenderer>)> {
    vec![
        ("comrak", Box::new(Comrak::new())),
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
