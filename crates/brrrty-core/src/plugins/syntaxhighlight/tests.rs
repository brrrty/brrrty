use super::*;

fn plugin() -> SyntaxHighlight {
    SyntaxHighlight::new(Options::default()).unwrap()
}

fn s(v: &str) -> Value {
    Value::String(v.into())
}

#[test]
fn aliases_normalize_as_the_plugin_does() {
    assert_eq!(normalize("js"), "javascript");
    assert_eq!(normalize("sh"), "bash");
    assert_eq!(normalize("shell"), "bash");
    assert_eq!(normalize("html"), "markup");
    assert_eq!(normalize("njk"), "django");
    assert_eq!(normalize("nunjucks"), "django");
    assert_eq!(normalize("ts"), "typescript");
    assert_eq!(normalize("conf"), "conf");
    assert_eq!(normalize("JS"), "JS");
}

#[test]
fn attributes_follow_javascript_key_order() {
    let none: Attributes = None;
    assert_eq!(attributes(&none, "js").unwrap(), r#" class="language-js""#);
    assert_eq!(attributes(&none, "").unwrap(), "");
    assert_eq!(attributes(&Some(vec![]), "").unwrap(), "");
    let attrs = Some(vec![
        ("tabindex".into(), Value::Number(0.0)),
        ("data-x".into(), s("a & b")),
        ("10".into(), s("ten")),
        ("2".into(), Value::Number(1.5)),
    ]);
    assert_eq!(
        attributes(&attrs, "js").unwrap(),
        r#" 2="1.5" 10="ten" class="language-js" tabindex="0" data-x="a & b""#
    );
    let class = Some(vec![("id".into(), s("x")), ("class".into(), s("c"))]);
    assert_eq!(attributes(&class, "js").unwrap(), r#" id="x" class="c""#);
    let bad = Some(vec![("x".into(), Value::Bool(true))]);
    assert_eq!(
        attributes(&bad, "js"),
        Err(Error::Thrown(
            "Attribute \"x\" must have, or evaluate to, a value of type string or number, not \"boolean\".".into()
        ))
    );
}

#[test]
fn unknown_languages_and_text_are_written_as_they_are() {
    let p = plugin();
    let mut page = p.page();
    assert_eq!(
        page.markdown("<b>&</b>\n", "nope").unwrap(),
        r#"<pre class="language-nope"><code class="language-nope"><b>&</b></code></pre>"#
    );
    assert_eq!(
        page.markdown("<b>\n", "text").unwrap(),
        r#"<pre class="language-text"><code class="language-text"><b></code></pre>"#
    );
    assert_eq!(
        page.markdown("", "js").unwrap(),
        r#"<pre class="language-js"><code class="language-js"></code></pre>"#
    );
}

#[test]
fn languages_brrrty_lacks_and_object_keys_are_refused() {
    let p = plugin();
    let mut page = p.page();
    for lang in ["haskell", "njk", "meta", "constructor", "diff-haskell"] {
        assert!(
            matches!(page.markdown("x", lang), Err(Error::Unsupported(_))),
            "{lang}"
        );
    }
}

#[test]
fn lookups_that_depend_on_other_pages_are_refused() {
    let p = plugin();
    let mut page = p.page();
    // `ts` is there once diff-ts loads TypeScript.
    assert!(page.markdown("+ let a: number;\n", "diff-ts").is_ok());
    // diff-<x> highlights the changed lines as <x> when Prism has it, which
    // for `yml` depends on whether another page loaded YAML.
    let mut fresh = p.page();
    assert!(fresh.markdown("+ a: 1\n", "diff-yml").is_ok());
    let mut fresh = p.page();
    fresh.state.keys.retain(|k| k != "yml");
    let mut st = fresh.state.clone();
    assert!(st.get("yml").is_err());
    assert_eq!(st.get("nope").unwrap(), None);
    st.set("diff-x", grammars().language("diff").unwrap());
    assert!(st.get("diff-x").unwrap().is_some());
    assert!(st.get("diff-y").is_err());
}

#[test]
fn markdown_front_matter_needs_yaml_loaded() {
    let p = plugin();
    let code = "---\na: 1\n---\n# T\n";
    assert!(matches!(
        p.page().markdown(code, "md"),
        Err(Error::Unsupported(_))
    ));
    let mut page = p.page();
    page.markdown("a: 1\n", "yaml").unwrap();
    assert!(page.markdown(code, "md").is_ok());
    let preloaded = SyntaxHighlight::new(Options {
        languages: vec!["yml".into()],
        ..Options::default()
    })
    .unwrap();
    assert!(preloaded.page().markdown(code, "md").is_ok());
}

#[test]
fn the_liquid_tag_splits_on_single_spaces() {
    let p = plugin();
    assert_eq!(
        p.liquid_tag("text 0", "\n a \r\nb\n").unwrap(),
        r#"<pre class="language-text"><code class="language-text"><mark class="highlight-line highlight-line-active">a </mark>
<span class="highlight-line">b</span></code></pre>"#
    );
    assert_eq!(p.liquid_tag("", "x").unwrap(), "<pre><code>x</code></pre>");
}

#[test]
fn template_formats_pick_what_is_highlighted() {
    let mut o = Options::default();
    assert!(o.has_format("md") && o.has_format("liquid"));
    o.template_formats = Some(vec!["md".into()]);
    assert!(o.has_format("md") && !o.has_format("liquid"));
    o.template_formats = Some(vec!["*".into()]);
    assert!(o.has_format("liquid"));
}
