//! The filters `@11ty/eleventy-plugin-rss` 3.1.0 adds to Nunjucks only
//! (`addNunjucksFilter`, `addNunjucksAsyncFilter`): a Liquid template can't
//! use them in 11ty either.

use std::sync::Arc;

use brrrty_js::{FilterEnv, FilterFn, Value};
use chrono::DateTime;

use super::feed::{date_to_rfc822_in, date_to_rfc3339, member, newest_date, now_ms};
use super::{absolute_url, absolute_url_no_base};

const REFUSE: &str = brrrty_js::REFUSAL_PREFIX;

/// `new Date(ms)` as data. Brrrty's dates are always valid.
fn date_value(ms: f64) -> Result<Value, String> {
    DateTime::from_timestamp_millis(ms as i64)
        .map(|d| Value::Date(d.fixed_offset()))
        .ok_or_else(|| format!("{REFUSE}can't make a date from {ms} milliseconds"))
}

fn type_error_not_a_function(input: &Value, what: &str) -> String {
    match input {
        Value::Undefined | Value::Null => format!(
            "TypeError: Cannot read properties of {} (reading '{what}')",
            input.to_js_string()
        ),
        _ => format!("TypeError: {}.{what} is not a function", "dateObj"),
    }
}

/// `dateToRfc3339(dateObj)`: `dateObj.toISOString()` without the
/// milliseconds.
fn date_to_rfc3339_filter(
    input: &Value,
    _args: &[Value],
    _env: &FilterEnv,
) -> Result<Value, String> {
    let Value::Date(d) = input else {
        return Err(type_error_not_a_function(input, "toISOString"));
    };
    date_to_rfc3339(d.timestamp_millis() as f64).map(|s| Value::String(s.into()))
}

/// `dateToRfc822(value, timeZone = undefined)`: `new Date(value)` in the
/// local zone.
fn date_to_rfc822_filter(input: &Value, args: &[Value], env: &FilterEnv) -> Result<Value, String> {
    if !matches!(args.first(), None | Some(Value::Undefined)) {
        return Err(format!(
            "{REFUSE}doesn't take a time zone argument for `dateToRfc822` yet"
        ));
    }
    let ms = match input {
        Value::Date(d) => d.timestamp_millis() as f64,
        Value::Number(n) => *n,
        // `new Date(null)` is the epoch and `new Date(true)` is 1 ms.
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Undefined => return Err("RangeError: Invalid time value".into()),
        _ => {
            return Err(format!(
                "{REFUSE}takes only a date or a number in `dateToRfc822`: JavaScript would read other values as a date string"
            ));
        }
    };
    date_to_rfc822_in(env.timezone, ms).map(|s| Value::String(s.into()))
}

/// `getNewestCollectionItemDate(collection, emptyFallbackDate)`.
fn newest_filter(input: &Value, args: &[Value], _env: &FilterEnv) -> Result<Value, String> {
    let fallback = || -> Result<Value, String> {
        match args.first() {
            Some(v) if v.js_truthy() => Ok(v.clone()),
            _ => date_value(now_ms()),
        }
    };
    let items = match input {
        Value::Array(a) if a.is_empty() => return fallback(),
        Value::Array(a) => a.as_ref().clone(),
        v if !v.js_truthy() => return fallback(),
        Value::String(_) => return Err("TypeError: collection.map is not a function".into()),
        Value::Object(o) if member(input, "length").js_truthy() => {
            let _ = o;
            return Err("TypeError: collection.map is not a function".into());
        }
        _ => return fallback(),
    };
    match newest_date(&items) {
        Ok(ms) => date_value(ms),
        Err(_) => Err(format!(
            "{REFUSE}can't hold an invalid date: an item's `date` isn't a date or a number"
        )),
    }
}

/// `absoluteUrl(url, base)`: `new URL(url, base).toString()`, or `url`
/// itself when that throws.
fn absolute_url_filter(input: &Value, args: &[Value], env: &FilterEnv) -> Result<Value, String> {
    let url = input.to_js_string_in(env.timezone);
    let made = match args.first() {
        None | Some(Value::Undefined) => absolute_url_no_base(&url),
        Some(base) => {
            let base = base.to_js_string_in(env.timezone);
            // `absolute_url` hands back its input when the URL is invalid.
            let joined = absolute_url(&url, &base);
            (joined != url).then_some(joined)
        }
    };
    Ok(match made {
        Some(s) => Value::String(s.into()),
        None => input.clone(),
    })
}

fn html_to_absolute_urls_filter(
    _input: &Value,
    _args: &[Value],
    _env: &FilterEnv,
) -> Result<Value, String> {
    Err(format!(
        "{REFUSE}can't run `htmlToAbsoluteUrls` yet: Eleventy adds it to Nunjucks as an async filter, and Brrrty doesn't run those"
    ))
}

fn removed(message: &'static str) -> FilterFn {
    Arc::new(move |_, _, _| Err(message.to_string()))
}

/// The plugin's Nunjucks-only filters, by name.
pub(crate) fn filters() -> Vec<(&'static str, FilterFn)> {
    vec![
        (
            "getNewestCollectionItemDate",
            Arc::new(newest_filter) as FilterFn,
        ),
        ("dateToRfc3339", Arc::new(date_to_rfc3339_filter)),
        ("dateToRfc822", Arc::new(date_to_rfc822_filter)),
        ("absoluteUrl", Arc::new(absolute_url_filter)),
        ("htmlToAbsoluteUrls", Arc::new(html_to_absolute_urls_filter)),
        (
            "rssLastUpdatedDate",
            removed(
                "The `rssLastUpdatedDate` filter was removed. Use `getNewestCollectionItemDate | dateToRfc3339` (for Atom) or `getNewestCollectionItemDate | dateToRfc822` (for RSS) instead.",
            ),
        ),
        (
            "rssDate",
            removed(
                "The `rssDate` filter was removed. Use `dateToRfc3339` (for Atom) or `dateToRfc822` (for RSS) instead.",
            ),
        ),
    ]
}
