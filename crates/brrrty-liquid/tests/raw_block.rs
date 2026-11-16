//! Raw block tags an embedder adds, such as the syntax highlighting
//! plugin's `{% highlight %}`: LiquidJS hands such a tag the tokens up to
//! its end tag, and the plugin joins their text.

use std::sync::Arc;

use brrrty_js::Value;
use brrrty_liquid::{Engine, EngineOptions, ErrorKind, LoadError, Loader, TagError};

struct NoFiles;

impl Loader for NoFiles {
    fn load(&self, _: &str) -> Result<(String, String), LoadError> {
        Err(LoadError::default())
    }
}

fn engine() -> Engine {
    let mut e = Engine::new(EngineOptions::default());
    e.register_raw_block(
        "hl",
        Arc::new(|args: &str, body: &str| {
            if args == "fail" {
                return Err(TagError {
                    kind: ErrorKind::Unsupported,
                    message: "no".into(),
                    help: Some("why".into()),
                });
            }
            Ok(format!("[{args}|{body}]"))
        }),
    );
    e
}

fn run(src: &str) -> Result<String, brrrty_liquid::LiquidError> {
    let e = engine();
    let t = e.parse("t", src)?;
    e.render(&t, &Value::Undefined, &NoFiles)
}

#[test]
fn the_body_is_the_text_as_written() {
    assert_eq!(
        run("a{% hl  js 1 %}\nx {{ y | nope }}{% if %}{%- raw -%}{% endhl %}{% endraw %}\n{% endhl %}b")
            .unwrap(),
        "a[js 1|\nx {{ y | nope }}{% if %}{%- raw -%}{% endhl %}{% endraw %}\n]b"
    );
    assert_eq!(run("{%- hl -%} x {%- endhl -%}").unwrap(), "[| x ]");
    assert_eq!(
        run("{% hl a %}{% hl b %}{% endhl %}").unwrap(),
        "[a|{% hl b %}]"
    );
}

#[test]
fn an_unclosed_block_is_an_error() {
    let e = run("{% hl js %}x").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unclosed);
    assert_eq!(e.message, "tag {% hl js %} not closed");
}

#[test]
fn errors_are_at_the_tag() {
    let e = run("\n {% hl fail %}{% endhl %}").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert_eq!((e.message.as_str(), e.line, e.column), ("no", 2, 2));
    assert_eq!(e.help.as_deref(), Some("why"));
}

#[test]
fn without_the_tag_it_is_unknown() {
    let e = Engine::new(EngineOptions::default());
    let err = e.parse("t", "{% hl %}{% endhl %}").unwrap_err();
    assert!(
        err.message.starts_with("tag \"hl\" not found"),
        "{}",
        err.message
    );
}
