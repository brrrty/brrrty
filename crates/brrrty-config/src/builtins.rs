//! Built-in helpers that stand in for npm packages a config uses: Luxon's
//! `toFormat` and a plain `markdown-it` instance.
//!
//! - `luxon_format(date, format, zone)` is
//!   `DateTime.fromJSDate(date, { zone }).toFormat(format)` (Luxon 3.7.2);
//!   `luxon_format(date, format, zone, locale)` takes a locale, and only
//!   `en-US` is supported. Anything that isn't a `date` gives `"Invalid
//!   DateTime"`, as it does in Luxon.
//! - `markdown(s)` is `markdownIt().render(s || '')` and `markdown_inline(s)`
//!   is `markdownIt().renderInline(s || '')`, with markdown-it's default
//!   options: HTML **off** (unlike 11ty's own instance), no linkify and no
//!   typographer, and indented code blocks on. A falsy `s` (`()`, `null`,
//!   `""`, `0`, `false`) gives `""`, as `s || ''` does; any other value that
//!   is not a string is an error, as markdown-it throws.
//! - `markdown(s, options)` is `markdownIt({ html, linkify, typographer })`
//!   with `md.linkify.set({ fuzzyLink })`: `options` is a map of booleans with
//!   any of `html`, `linkify`, `typographer` and `fuzzy_link`.

use brrrty_core::helpers::{luxon, markdown};
use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString};

use crate::api::{JsDate, invalid};
use crate::convert::to_value;
use crate::object::JsNull;

type Res<T> = Result<T, Box<EvalAltResult>>;

fn format(d: &Dynamic, fmt: &str, zone: &str, locale: &str) -> Res<String> {
    let ms = d.read_lock::<JsDate>().map_or(f64::NAN, |d| d.0);
    luxon::luxon_format(ms, fmt, zone, locale).map_err(|e| invalid(e, rhai::Position::NONE))
}

/// `s || ''`, then markdown-it's check that the input is a string.
fn source(s: &Dynamic, what: &str) -> Res<Option<String>> {
    let v = to_value(s).map_err(|why| invalid(format!("{what}: {why}"), rhai::Position::NONE))?;
    if !v.js_truthy() {
        return Ok(None);
    }
    match v {
        brrrty_js::Value::String(s) => Ok(Some(s.to_string())),
        _ => Err(invalid(
            format!("{what}: Input data should be a String"),
            rhai::Position::NONE,
        )),
    }
}

/// The options map of `markdown(s, options)`.
fn markdown_options(options: &Dynamic) -> Res<markdown::Options> {
    let err = |m: String| invalid(format!("markdown: {m}"), rhai::Position::NONE);
    let v = to_value(options).map_err(|why| err(format!("options: {why}")))?;
    let brrrty_js::Value::Object(map) = v else {
        return Err(err(
            "options must be a map, like #{ linkify: true, typographer: true }".into(),
        ));
    };
    let mut o = markdown::Options::default();
    for (key, value) in map.iter() {
        let slot = match key.as_str() {
            "html" => &mut o.html,
            "linkify" => &mut o.linkify,
            "typographer" => &mut o.typographer,
            "fuzzy_link" => &mut o.fuzzy_link,
            _ => {
                return Err(err(format!(
                    "unknown option `{key}`: Brrrty's markdown-it takes `html`, `linkify`, `typographer` and `fuzzy_link` (linkify-it's `fuzzyLink`)"
                )));
            }
        };
        let brrrty_js::Value::Bool(b) = value else {
            return Err(err(format!("option `{key}` must be true or false")));
        };
        *slot = *b;
    }
    Ok(o)
}

pub fn register(e: &mut Engine) {
    e.register_fn(
        "luxon_format",
        |d: Dynamic, fmt: ImmutableString, zone: ImmutableString| format(&d, &fmt, &zone, "en-US"),
    );
    e.register_fn(
        "luxon_format",
        |d: Dynamic, fmt: ImmutableString, zone: ImmutableString, locale: ImmutableString| {
            format(&d, &fmt, &zone, &locale)
        },
    );
    // `toFormat(null)` reads `null.length`.
    e.register_fn(
        "luxon_format",
        |_: Dynamic, _: JsNull, _: ImmutableString| -> Res<String> {
            Err(invalid(
                "luxon_format: Cannot read properties of null (reading 'length')".into(),
                rhai::Position::NONE,
            ))
        },
    );
    e.register_fn("markdown", |s: Dynamic| -> Res<String> {
        Ok(source(&s, "markdown")?.map_or_else(String::new, |s| markdown::markdown(&s)))
    });
    e.register_fn("markdown", |s: Dynamic, options: Dynamic| -> Res<String> {
        let options = markdown_options(&options)?;
        let Some(s) = source(&s, "markdown")? else {
            return Ok(String::new());
        };
        markdown::markdown_with(&s, options)
            .map_err(|e| invalid(format!("markdown: {e}"), rhai::Position::NONE))
    });
    e.register_fn("markdown_inline", |s: Dynamic| -> Res<String> {
        Ok(source(&s, "markdown_inline")?
            .map_or_else(String::new, |s| markdown::markdown_inline(&s)))
    });
}
