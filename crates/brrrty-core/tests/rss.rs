//! The RSS plugin's HTML handling, against what Node printed.
//!
//! `tests/probes/posthtml.json` holds HTML fragments (real post shapes:
//! links, images, `srcset`, `poster`, `ping`, a refresh `<meta>`, URLs in
//! `<code>`, `<pre>` and `<script>`, and broken markup) with what 11ty
//! 3.1.8 gives for each: `transforms` is `renderTransforms` in a feed (the
//! HTML `<base>` plugin's URL transform through `@11ty/posthtml-urls`,
//! `posthtml` 0.16.7), and `absolute` is the plugin's `htmlToAbsoluteUrls`.
//! An `err` is where 11ty throws.
//!
//! `BRRRTY_POSTHTML_CASES=file cargo test -p brrrty-core --test rss -- --ignored`
//! checks another file of the same shape, such as a fuzz run.

use brrrty_core::plugins::rss::{html_to_absolute_urls, render_transforms};
use brrrty_js::Value;

/// Brrrty refuses an element or attribute named like a property every
/// JavaScript object has (`toString`, `constructor`): the plugin's lookups
/// find those on its own tables and do things that make no sense.
fn refused(e: &str) -> bool {
    e.contains("which the URL plugin can't read the way 11ty does")
}

fn check(path: &std::path::Path) -> (usize, Vec<String>) {
    let text = std::fs::read_to_string(path).expect("probe file");
    let cases: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    let mut bad = Vec::new();
    let cases = cases.as_array().expect("a list");
    for c in cases {
        let html = c["html"].as_str().unwrap();
        let base = c["base"].as_str().unwrap();
        let page = c["page"].as_str().unwrap();
        let out = Value::String("./_site/x/index.html".into());
        let got = render_transforms(html, &out, Some(page), base);
        let want = &c["transforms"];
        match (&got, want.get("ok").and_then(|v| v.as_str())) {
            (Ok(g), Some(w)) if g == w => {}
            (Err(_), None) => {}
            (Err(e), _) if refused(e) => {}
            _ => bad.push(format!(
                "transforms {html:?} base {base} page {page}\n  got  {got:?}\n  want {want}"
            )),
        }
        let got = html_to_absolute_urls(html, base);
        let want = &c["absolute"];
        match (&got, want.get("ok").and_then(|v| v.as_str())) {
            (Ok(g), Some(w)) if g == w => {}
            (Err(_), None) => {}
            (Err(e), _) if refused(e) => {}
            _ => bad.push(format!(
                "absolute {html:?} base {base}\n  got  {got:?}\n  want {want}"
            )),
        }
    }
    (cases.len(), bad)
}

#[test]
fn posthtml_matches_node() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/probes/posthtml.json");
    let (n, bad) = check(&path);
    assert!(n > 100, "{n} cases");
    assert!(
        bad.is_empty(),
        "{} of {n} differ:\n{}",
        bad.len(),
        bad[..bad.len().min(20)].join("\n")
    );
}

#[test]
#[ignore = "needs BRRRTY_POSTHTML_CASES"]
fn posthtml_matches_node_on_a_file() {
    let path = std::env::var("BRRRTY_POSTHTML_CASES").expect("BRRRTY_POSTHTML_CASES");
    let (n, bad) = check(std::path::Path::new(&path));
    println!("{n} cases, {} differ", bad.len());
    assert!(
        bad.is_empty(),
        "{} of {n} differ:\n{}",
        bad.len(),
        bad[..bad.len().min(30)].join("\n")
    );
}

/// A page whose output isn't `.html` is left as it is.
#[test]
fn only_html_pages_are_transformed() {
    let html = "<a href=\"x\">x</a>";
    for out in ["./_site/notes/a.txt", "./_site/feed", ""] {
        let got = render_transforms(
            html,
            &Value::String(out.into()),
            Some("/a/"),
            "https://e.com/",
        )
        .unwrap();
        assert_eq!(got, html, "{out}");
    }
    assert_eq!(
        render_transforms(html, &Value::Bool(false), Some("/a/"), "https://e.com/").unwrap(),
        html
    );
}
