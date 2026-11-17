//! JavaScript methods a template calls that depend on a locale: Brrrty
//! runs them as Node does in `en-US`, and refuses other locales and
//! options.

use brrrty_js::Value;
use brrrty_nunjucks::{Engine, EngineOptions, ErrorKind, LoadError, Loader, Tz};

struct NoIncludes;

impl Loader for NoIncludes {
    fn load(&self, _: &str) -> Result<(String, String), LoadError> {
        Err(Default::default())
    }
}

fn render(tpl: &str) -> Result<String, brrrty_nunjucks::NunjucksError> {
    let e = Engine::new(EngineOptions {
        timezone: Tz::Utc,
        autoescape: true,
    });
    let data = serde_json::json!({ "n": 1234.5678, "z": -0.0004, "s": "résumé" });
    e.compile("t", tpl)
        .and_then(|t| e.render(&t, &Value::from_json(&data), &NoIncludes))
}

#[test]
fn number_to_locale_string_is_en_us() {
    // Node: (1234.5678).toLocaleString(), ('en-US'), (-0.0004).toLocaleString('en'),
    // (1234.5678).toFixed(2)
    assert_eq!(
        render("{{ n.toLocaleString() }}|{{ n.toLocaleString(\"en-US\") }}|{{ z.toLocaleString(\"en\") }}|{{ n.toFixed(2) }}")
            .unwrap(),
        "1,234.568|1,234.568|-0|1234.57"
    );
    for tpl in [
        "{{ n.toLocaleString(\"de-DE\") }}",
        "{{ n.toLocaleString(\"en-US\", { maximumFractionDigits: 1 }) }}",
    ] {
        let e = render(tpl).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Unsupported, "{tpl}: {e:?}");
    }
}

#[test]
fn string_locale_compare_is_icus_root_order() {
    // Node (en-US): "résumé".localeCompare("resume"), "a".localeCompare("B"),
    // "a".localeCompare("a"), "10".localeCompare(9)
    assert_eq!(
        render("{{ s.localeCompare(\"resume\") }}|{{ \"a\".localeCompare(\"B\") }}|{{ \"a\".localeCompare(\"a\", \"en-US\") }}|{{ \"10\".localeCompare(9) }}")
            .unwrap(),
        "1|-1|0|-1"
    );
    let e = render("{{ s.localeCompare(\"x\", \"sv\") }}").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unsupported, "{e:?}");
    // Node sorts Han by radical and stroke; refused.
    let e = render("{{ \"缵\".localeCompare(\"纓\") }}").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unsupported, "{e:?}");
}
