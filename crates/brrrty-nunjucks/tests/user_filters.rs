//! The filters an embedder registers: what they get from the engine
//! (`FilterEnv`) and how their results are treated.

use std::sync::Arc;

use brrrty_js::Value;
use brrrty_nunjucks::{Engine, EngineOptions, ErrorKind, FilterEnv, LoadError, Loader, Tz};

struct NoIncludes;

impl Loader for NoIncludes {
    fn load(&self, _: &str) -> Result<(String, String), LoadError> {
        Err(Default::default())
    }
}

fn engine() -> Engine {
    Engine::new(EngineOptions {
        timezone: Tz::Utc,
        autoescape: true,
    })
}

fn render(
    e: &Engine,
    tpl: &str,
    data: serde_json::Value,
) -> Result<String, brrrty_nunjucks::NunjucksError> {
    e.compile("t", tpl)
        .and_then(|t| e.render(&t, &Value::from_json(&data), &NoIncludes))
}

#[test]
fn a_safe_string_arrives_as_its_text_with_a_flag() {
    let mut e = engine();
    e.register_filter(
        "probe",
        Arc::new(|input: &Value, args: &[Value], env: &FilterEnv| {
            Ok(Value::String(
                format!(
                    "{}|{}|{:?}|{:?}",
                    input.to_js_string(),
                    env.input_safe,
                    args.iter().map(Value::to_js_string).collect::<Vec<_>>(),
                    env.args_safe
                )
                .into(),
            ))
        }),
    );
    let data = serde_json::json!({"s": "a<b"});
    assert_eq!(
        render(&e, "{{ s | probe }}", data.clone()).unwrap(),
        "a&lt;b|false|[]|[]"
    );
    assert_eq!(
        render(&e, "{{ (s | safe) | probe(1, (s | e)) }}", data.clone()).unwrap(),
        "a&lt;b|true|[&quot;1&quot;, &quot;a&amp;lt;b&quot;]|[false, true]"
    );
}

#[test]
fn a_filter_that_returns_its_input_keeps_a_safe_string_safe() {
    let mut e = engine();
    e.register_filter(
        "same",
        Arc::new(|input: &Value, _: &[Value], env: &FilterEnv| {
            env.returns_input();
            Ok(input.clone())
        }),
    );
    e.register_filter(
        "copy",
        Arc::new(|input: &Value, _: &[Value], _: &FilterEnv| Ok(input.clone())),
    );
    let data = serde_json::json!({"s": "<b>"});
    assert_eq!(
        render(&e, "{{ (s | safe) | same }}", data.clone()).unwrap(),
        "<b>"
    );
    assert_eq!(
        render(&e, "{{ s | same }}", data.clone()).unwrap(),
        "&lt;b&gt;"
    );
    // An equal string that the filter built is a plain string.
    assert_eq!(
        render(&e, "{{ (s | safe) | copy }}", data).unwrap(),
        "&lt;b&gt;"
    );
}

#[test]
fn keyword_arguments_come_taken_apart_and_as_a_last_object() {
    let mut e = engine();
    e.register_filter(
        "kw",
        Arc::new(|_: &Value, args: &[Value], env: &FilterEnv| {
            let names: Vec<_> = env.kwargs.iter().map(|(k, _)| k.as_str()).collect();
            Ok(Value::String(
                format!(
                    "{}|{names:?}|{}",
                    args.len(),
                    args.last()
                        .map_or(String::new(), |a| a.get("__keywords").to_js_string())
                )
                .into(),
            ))
        }),
    );
    assert_eq!(
        render(&e, "{{ 1 | kw(2, a=3, b=4) }}", serde_json::json!({})).unwrap(),
        "2|[&quot;a&quot;, &quot;b&quot;]|true"
    );
}

#[test]
fn a_filter_sees_the_page() {
    let mut e = engine();
    e.register_filter(
        "where",
        Arc::new(|_: &Value, _: &[Value], env: &FilterEnv| {
            Ok(env.page.as_ref().map_or(Value::Null, |p| p.get("url")))
        }),
    );
    let data = serde_json::json!({"page": {"url": "/x/"}});
    assert_eq!(render(&e, "{{ 1 | where }}", data).unwrap(), "/x/");
    assert_eq!(
        render(&e, "{{ 1 | where }}", serde_json::json!({})).unwrap(),
        ""
    );
}

#[test]
fn a_safe_string_inside_a_list_is_refused() {
    let mut e = engine();
    e.register_filter(
        "id",
        Arc::new(|i: &Value, _: &[Value], _: &FilterEnv| Ok(i.clone())),
    );
    let err = render(&e, "{{ [('a' | safe)] | id }}", serde_json::json!({})).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported, "{err}");
}

#[test]
fn errors_are_wrapped_as_eleventy_wraps_them_and_refusals_are_unsupported() {
    let mut e = engine();
    e.register_filter(
        "boom",
        Arc::new(|_: &Value, _: &[Value], _: &FilterEnv| Err("it broke".into())),
    );
    e.register_filter(
        "refuse",
        Arc::new(|_: &Value, _: &[Value], _: &FilterEnv| {
            Err(format!("{}can't do that", brrrty_js::REFUSAL_PREFIX))
        }),
    );
    let err = render(&e, "{{ 1 | boom }}", serde_json::json!({})).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Render);
    assert!(
        err.message
            .contains("Error in Nunjucks Filter `boom`: it broke"),
        "{err}"
    );
    let err = render(&e, "{{ 1 | refuse }}", serde_json::json!({})).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported);
}

#[test]
fn eleventys_url_filter_keeps_a_safe_url_safe() {
    let mut e = engine();
    e.register_eleventy_filters();
    let data = serde_json::json!({"u": "https://a.b/<c>"});
    assert_eq!(
        render(&e, "{{ (u | safe) | url }}", data.clone()).unwrap(),
        "https://a.b/<c>"
    );
    assert_eq!(
        render(&e, "{{ u | url }}", data).unwrap(),
        "https://a.b/&lt;c&gt;"
    );
}
