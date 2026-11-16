//! Engine behavior that the conformance cases and probes do not cover:
//! limits, deep nesting, the filter interface and strict-filter checks.

use std::sync::Arc;

use brrrty_js::Value;
use brrrty_liquid::{Engine, EngineOptions, ErrorKind, FilterFn, LiquidError, Loader, Tz};

struct NoPartials;

impl Loader for NoPartials {
    fn load(&self, _: &str) -> Result<(String, String), brrrty_liquid::LoadError> {
        Err(Default::default())
    }
}

fn engine() -> Engine {
    Engine::new(EngineOptions {
        strict_filters: true,
        timezone: Tz::Utc,
    })
}

fn render_with(engine: &Engine, src: &str, data: &Value) -> Result<String, LiquidError> {
    let t = engine.parse("t.liquid", src)?;
    engine.render(&t, data, &NoPartials)
}

fn render(src: &str) -> Result<String, LiquidError> {
    render_with(&engine(), src, &Value::Object(Default::default()))
}

fn data(json: &str) -> Value {
    Value::from_json(&serde_json::from_str(json).unwrap())
}

#[test]
fn engine_is_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Engine>();
}

#[test]
fn default_options_are_strict_and_local() {
    let o = EngineOptions::default();
    assert!(o.strict_filters);
    assert_eq!(o.timezone, Tz::Local);
}

#[test]
fn large_ranges_render_and_huge_ones_error() {
    let out = render("{% for i in (1..100000) %}{% endfor %}ok").unwrap();
    assert_eq!(out, "ok");
    let out = render("{% for i in (1..100000) %}{{ i }}{% endfor %}").unwrap();
    assert!(out.ends_with("99999100000"));

    let e = render("ab\n{% for i in (1..100000000) %}{{ i }}{% endfor %}").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Limit);
    assert_eq!((e.line, e.column), (2, 1));
    assert!(e.message.contains("100000000"), "{e}");
    assert!(e.help.is_some());

    // The guard is on the range itself, wherever it is used.
    let e = render("{% assign r = (1..100000000) %}").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Limit);
    let e = render("{{ (0..1000000) }}").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Limit);
    assert!(render("{% assign r = (1..1000000) %}{{ r.size }}").unwrap() == "1000000");
}

#[test]
fn huge_arrays_cannot_be_looped() {
    let big = Value::Array(Arc::new(vec![Value::Number(1.0); 1_000_001]));
    let mut o = indexmap::IndexMap::new();
    o.insert("xs".to_string(), big);
    let data = Value::Object(Arc::new(o));
    let e = render_with(&engine(), "{% for x in xs %}{% endfor %}", &data).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Limit);
    let e = render_with(&engine(), "{% tablerow x in xs %}{% endtablerow %}", &data).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Limit);
    // `size` does not loop.
    assert_eq!(
        render_with(&engine(), "{{ xs.size }}", &data).unwrap(),
        "1000001"
    );
}

/// Runs `f` on a thread with the 2 MiB stack Rust gives test threads, so a
/// pass does not depend on the main thread's larger stack.
fn on_small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn deep_templates_render_without_overflow() {
    on_small_stack(|| {
        let n = 256;
        let src = format!("{}x{}", "{% if true %}".repeat(n), "{% endif %}".repeat(n));
        assert_eq!(render(&src).unwrap(), "x");

        let src = format!(
            "{}x{}",
            "{% for i in (1..1) %}".repeat(n),
            "{% endfor %}".repeat(n)
        );
        assert_eq!(render(&src).unwrap(), "x");

        let src = format!(
            "{}x{}",
            "{% capture c %}".repeat(n),
            "{% endcapture %}".repeat(n)
        );
        assert_eq!(render(&src).unwrap(), "");

        let chain = vec!["true"; 257].join(" and ");
        assert_eq!(
            render(&format!("{{% if {chain} %}}y{{% endif %}}")).unwrap(),
            "y"
        );

        let chain = vec!["1 == 1"; 128].join(" or ");
        assert_eq!(render(&format!("{{{{ {chain} }}}}")).unwrap(), "true");

        // 255 brackets is as deep as the parser allows.
        let nested = format!("{}0{}", "a[".repeat(255), "]".repeat(255));
        assert_eq!(
            render_with(
                &engine(),
                &format!("{{{{ {nested} }}}}"),
                &data(r#"{"a":[0]}"#)
            )
            .unwrap(),
            "0"
        );
    });
}

#[test]
fn filters_get_input_positional_and_keyword_arguments() {
    let mut e = engine();
    let f: FilterFn = Arc::new(|input, args, kwargs, _ctx| {
        let args: Vec<String> = args.iter().map(|a| format!("{a:?}")).collect();
        let kwargs: Vec<String> = kwargs.iter().map(|(k, v)| format!("{k}={v:?}")).collect();
        Ok(Value::String(
            format!("{input:?} [{}] [{}]", args.join(" "), kwargs.join(" ")).into(),
        ))
    });
    e.register_filter("probe", f);
    let out = render_with(
        &e,
        "{{ 'x' | probe: 1, n, k: 2, 'q': 'v', j: }}",
        &data(r#"{"n": null}"#),
    )
    .unwrap();
    assert_eq!(
        out,
        r#"String("x") [Number(1.0) Null] [k=Number(2.0) 'q'=String("v") j=Undefined]"#
    );
    // Drops reach filters as their values: nil is null, empty is "".
    let out = render_with(&e, "{{ nil | probe: empty, blank }}", &data("{}")).unwrap();
    assert_eq!(out, r#"Null [String("") String("")] []"#);
    // Filters chain left to right.
    let out = render_with(&e, "{{ 1 | probe | probe }}", &data("{}")).unwrap();
    assert!(out.starts_with(r#"String("Number(1.0) [] []") "#), "{out}");
}

#[test]
fn filters_see_the_engine_time_zone() {
    let mut e = Engine::new(EngineOptions {
        strict_filters: true,
        timezone: Tz::Utc,
    });
    e.register_filter(
        "tz",
        Arc::new(|_, _, _, ctx| Ok(Value::String(format!("{:?}", ctx.timezone).into()))),
    );
    assert_eq!(render_with(&e, "{{ 1 | tz }}", &data("{}")).unwrap(), "Utc");
}

#[test]
fn a_filter_error_is_a_render_error_at_its_tag() {
    let mut e = engine();
    e.register_filter(
        "boom",
        Arc::new(|_, _, _, _| Err("boom went the filter".into())),
    );
    let err = render_with(
        &e,
        "ab\n  {% if x %}{{ 1 | boom }}{% endif %}",
        &data(r#"{"x":1}"#),
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Render);
    assert_eq!(err.message, "boom went the filter");
    assert_eq!((err.line, err.column), (2, 13));
    assert_eq!(err.template, "t.liquid");
}

#[test]
fn strict_filters_are_checked_at_parse_time() {
    let mut e = engine();
    e.register_filter("upcase", Arc::new(|v, _, _, _| Ok(v.clone())));
    let err = e.parse("t", "a\n {{ x | upcas }}").unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownFilter);
    assert_eq!(err.message, "undefined filter: upcas");
    assert_eq!((err.line, err.column), (2, 2));
    assert!(err.help.as_deref().unwrap().contains("`upcase`"), "{err}");

    // An `elsif` error is reported at its `if`, as LiquidJS does.
    let err = e
        .parse("t", "{% if a %}{% elsif b | nope %}{% endif %}")
        .unwrap_err();
    assert_eq!(
        (err.message.as_str(), err.column),
        ("undefined filter: nope", 1)
    );
    // Inside `liquid`, at the line's tag.
    let err = e.parse("t", "{% liquid\necho 'a' | nope\n%}").unwrap_err();
    assert_eq!((err.line, err.column), (2, 1));
    // `assign`, `case`, `unless` and `echo` values are checked.
    for src in [
        "{% assign a = 1 | nope %}",
        "{% case 1 | nope %}{% endcase %}",
        "{% unless 1 | nope %}{% endunless %}",
        "{% echo 1 | nope %}",
    ] {
        assert_eq!(
            e.parse("t", src).unwrap_err().kind,
            ErrorKind::UnknownFilter,
            "{src}"
        );
    }
    // `cycle` values are only checked when they render, and an `elsif`
    // after `else` in `unless` never is: LiquidJS discards it.
    assert!(e.parse("t", "{% cycle 'a' | nope %}").is_ok());
    assert!(
        e.parse(
            "t",
            "{% unless a %}{% else %}{% elsif b | nope %}{% endunless %}"
        )
        .is_ok()
    );
    let err = render_with(&e, "{% cycle 'a' | nope %}", &data("{}")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownFilter);
    assert_eq!(err.message, "undefined filter: nope");
}

#[test]
fn strict_filter_errors_keep_source_order() {
    let e = engine();
    let err = e.parse("t", "{% nosuch %}{{ x | bad }}").unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownTag);
    let err = e.parse("t", "{{ x | bad }}{% nosuch %}").unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownFilter);
    // A tokenizer error anywhere still wins, as in LiquidJS.
    let err = e.parse("t", "{{ x | bad }}{{ y").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unclosed);
}

#[test]
fn free_parse_skips_the_filter_check() {
    let t = brrrty_liquid::parse("t", "{{ 'x' | nosuch }}").unwrap();
    let err = engine().render(&t, &data("{}"), &NoPartials).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownFilter);
    assert_eq!((err.line, err.column), (1, 1));

    // Without strict filters an unknown filter passes its input through,
    // as LiquidJS's `identify` does.
    let lax = Engine::new(EngineOptions {
        strict_filters: false,
        timezone: Tz::Utc,
    });
    assert_eq!(
        render_with(&lax, "{{ 'x' | nosuch: 1 }}", &data("{}")).unwrap(),
        "x"
    );
    assert_eq!(
        render_with(&lax, "{{ nil | nosuch }}", &data("{}")).unwrap(),
        ""
    );
}

#[test]
fn dates_print_in_the_engine_time_zone() {
    let d = Value::Date(chrono::DateTime::parse_from_rfc3339("2020-05-04T08:02:01+05:00").unwrap());
    let mut o = indexmap::IndexMap::new();
    o.insert("d".to_string(), d);
    let data = Value::Object(Arc::new(o));
    assert_eq!(
        render_with(&engine(), "{{ d }}", &data).unwrap(),
        "Mon May 04 2020 03:02:01 GMT+0000 (Coordinated Universal Time)"
    );
}

#[test]
fn rendering_does_not_change_the_data() {
    let d = data(r#"{"x": 5}"#);
    let e = engine();
    assert_eq!(
        render_with(&e, "{% increment x %}{{ x }}", &d).unwrap(),
        "56"
    );
    assert_eq!(
        render_with(&e, "{% increment x %}{{ x }}", &d).unwrap(),
        "56"
    );
}

#[test]
fn render_errors_name_the_template() {
    let t = brrrty_liquid::parse("pages/a.liquid", "{{ 'x' | nosuch }}").unwrap();
    let err = engine().render(&t, &data("{}"), &NoPartials).unwrap_err();
    assert!(
        err.to_string()
            .starts_with("pages/a.liquid:1:1: undefined filter: nosuch")
    );
}
