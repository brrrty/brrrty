//! LiquidJS 10.30.1's standard filters.
//!
//! Each filter is a port of the function in `liquid.node.js`, with the
//! JavaScript coercions it relies on. The set is exactly the one LiquidJS
//! registers, so a name that is not here (`md5`, `sha1`, `shuffle`) is an
//! unknown filter.
//!
//! | module | filters |
//! |---|---|
//! | `html` | `escape`, `xml_escape`, `escape_once`, `newline_to_br`, `strip_html` |
//! | `math` | `abs`, `at_least`, `at_most`, `ceil`, `divided_by`, `floor`, `minus`, `plus`, `modulo`, `times`, `round` |
//! | `url` | `url_decode`, `url_encode`, `cgi_escape`, `uri_escape`, `slugify` |
//! | `array` | `join`, `last`, `first`, `reverse`, `sort`, `sort_natural`, `size`, `map`, `sum`, `compact`, `concat`, `push`, `unshift`, `pop`, `shift`, `slice`, `where`, `reject`, `where_exp`, `reject_exp`, `group_by`, `group_by_exp`, `has`, `has_exp`, `find_index`, `find_index_exp`, `find`, `find_exp`, `uniq`, `sample` |
//! | `date` | `date`, `date_to_xmlschema`, `date_to_rfc822`, `date_to_string`, `date_to_long_string` |
//! | `string` | `append`, `prepend`, `lstrip`, `downcase`, `upcase`, `remove`, `remove_first`, `remove_last`, `rstrip`, `split`, `strip`, `strip_newlines`, `squish`, `capitalize`, `replace`, `replace_first`, `replace_last`, `truncate`, `truncatewords`, `normalize_whitespace`, `number_of_words`, `array_to_sentence_string` |
//! | `encode` | `base64_encode`, `base64_decode`, `sha256`, `hmac_sha256` |
//! | `misc` | `default`, `raw`, `jsonify`, `to_integer`, `json`, `inspect` |
//!
//! Strings are Rust strings, so JavaScript's lone surrogates cannot exist.
//! Where a filter would split a surrogate pair (`split: ''`, `replace: ''`,
//! `first` and `last` of an emoji), the halves become U+FFFD, which is what
//! writing such a string to a file produces.

mod array;
mod date;
mod encode;
mod html;
pub(crate) mod jsdate;
mod json;
mod math;
mod misc;
mod string;
mod url;

use std::collections::HashMap;
use std::sync::Arc;

use brrrty_js::Value;

use crate::context::Val;
use crate::engine::{Builtin, Filter};
use crate::ops;
use crate::render::{R, Renderer};

macro_rules! register {
    ($map:expr, $( $name:literal => $f:path ),+ $(,)?) => {
        $( $map.insert($name.to_string(), Filter::Builtin($f as Builtin)); )+
    };
}

/// Adds the standard filters.
pub(crate) fn register(map: &mut HashMap<String, Filter>) {
    register!(map,
        "escape" => html::escape,
        "xml_escape" => html::xml_escape,
        "escape_once" => html::escape_once,
        "newline_to_br" => html::newline_to_br,
        "strip_html" => html::strip_html,
        "abs" => math::abs,
        "at_least" => math::at_least,
        "at_most" => math::at_most,
        "ceil" => math::ceil,
        "divided_by" => math::divided_by,
        "floor" => math::floor,
        "minus" => math::minus,
        "plus" => math::plus,
        "modulo" => math::modulo,
        "times" => math::times,
        "round" => math::round,
        "url_decode" => url::url_decode,
        "url_encode" => url::url_encode,
        "cgi_escape" => url::cgi_escape,
        "uri_escape" => url::uri_escape,
        "slugify" => url::slugify,
        "join" => array::join,
        "last" => array::last,
        "first" => array::first,
        "reverse" => array::reverse,
        "sort" => array::sort,
        "sort_natural" => array::sort_natural,
        "size" => array::size,
        "map" => array::map,
        "sum" => array::sum,
        "compact" => array::compact,
        "concat" => array::concat,
        "push" => array::push,
        "unshift" => array::unshift,
        "pop" => array::pop,
        "shift" => array::shift,
        "slice" => array::slice,
        "where" => array::where_,
        "reject" => array::reject,
        "where_exp" => array::where_exp,
        "reject_exp" => array::reject_exp,
        "group_by" => array::group_by,
        "group_by_exp" => array::group_by_exp,
        "has" => array::has,
        "has_exp" => array::has_exp,
        "find_index" => array::find_index,
        "find_index_exp" => array::find_index_exp,
        "find" => array::find,
        "find_exp" => array::find_exp,
        "uniq" => array::uniq,
        "sample" => array::sample,
        "date" => date::date,
        "date_to_xmlschema" => date::date_to_xmlschema,
        "date_to_rfc822" => date::date_to_rfc822,
        "date_to_string" => date::date_to_string,
        "date_to_long_string" => date::date_to_long_string,
        "append" => string::append,
        "prepend" => string::prepend,
        "lstrip" => string::lstrip,
        "downcase" => string::downcase,
        "upcase" => string::upcase,
        "remove" => string::remove,
        "remove_first" => string::remove_first,
        "remove_last" => string::remove_last,
        "rstrip" => string::rstrip,
        "split" => string::split,
        "strip" => string::strip,
        "strip_newlines" => string::strip_newlines,
        "squish" => string::squish,
        "capitalize" => string::capitalize,
        "replace" => string::replace,
        "replace_first" => string::replace_first,
        "replace_last" => string::replace_last,
        "truncate" => string::truncate,
        "truncatewords" => string::truncatewords,
        "normalize_whitespace" => string::normalize_whitespace,
        "number_of_words" => string::number_of_words,
        "array_to_sentence_string" => string::array_to_sentence_string,
        "base64_encode" => encode::base64_encode,
        "base64_decode" => encode::base64_decode,
        "sha256" => encode::sha256,
        "hmac_sha256" => encode::hmac_sha256,
        "default" => misc::default,
        "raw" => misc::raw,
        "jsonify" => misc::json,
        "to_integer" => misc::to_integer,
        "json" => misc::json,
        "inspect" => misc::inspect,
    );
}

// ---- helpers shared by the filters ----------------------------------------

/// A filter's arguments, as LiquidJS's `arguments` after the input.
pub(crate) struct Args<'a>(pub &'a [Val]);

impl Args<'_> {
    /// The argument as a plain value, or `undefined` when it is missing.
    pub fn value(&self, i: usize) -> Value {
        self.0.get(i).map_or(Value::Undefined, Val::to_value)
    }

    /// The argument, or `default` when it is missing or `undefined`, as a
    /// JavaScript default parameter does.
    pub fn or(&self, i: usize, default: Value) -> Value {
        match self.value(i) {
            Value::Undefined => default,
            v => v,
        }
    }
}

pub(crate) fn is_nil(v: &Value) -> bool {
    matches!(v, Value::Null | Value::Undefined)
}

pub(crate) fn string(s: impl Into<Arc<str>>) -> Value {
    Value::String(s.into())
}

pub(crate) fn array(items: Vec<Value>) -> Value {
    Value::Array(Arc::new(items))
}

/// LiquidJS's `stringify`: strings as they are, null and undefined as
/// nothing, arrays as their items run together, anything else as `String(v)`.
pub(crate) fn stringify(r: &Renderer<'_>, v: &Value) -> String {
    ops::stringify(v, r.tz())
}

/// `stringify(v)` without copying a string that is already one.
pub(crate) fn text<'a>(r: &Renderer<'_>, v: &'a Value) -> std::borrow::Cow<'a, str> {
    match v {
        Value::String(s) => std::borrow::Cow::Borrowed(s),
        other => std::borrow::Cow::Owned(stringify(r, other)),
    }
}

/// JavaScript's `String(v)`.
pub(crate) fn js_string(r: &Renderer<'_>, v: &Value) -> String {
    ops::to_js_string(v, r.tz())
}

/// LiquidJS's `toArray`: null and undefined are empty, an array is itself,
/// anything else is a one-item array.
pub(crate) fn to_array(v: &Value) -> Arc<Vec<Value>> {
    match v {
        Value::Null | Value::Undefined => Arc::new(Vec::new()),
        Value::Array(a) => a.clone(),
        other => Arc::new(vec![other.clone()]),
    }
}

/// LiquidJS's `toNumber`: `+value || 0`.
pub(crate) fn to_number(v: &Value) -> f64 {
    let n = v.to_js_number();
    if n.is_nan() || n == 0.0 { 0.0 } else { n }
}

/// A UTF-16 length, as JavaScript's `.length`.
pub(crate) fn utf16_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.encode_utf16().count()
    }
}

/// An error a JavaScript `throw` would raise.
pub(crate) fn throw(message: impl Into<String>) -> crate::render::RenderErr {
    crate::render::RenderErr::new(crate::error::ErrorKind::Render, message)
}

/// Whether `a` and `b` are the same for `Set` and `Map`: JavaScript's
/// SameValueZero. Objects are the same only when they are the same object.
pub(crate) fn same_value_zero(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || (x.is_nan() && y.is_nan()),
        (Value::Array(x), Value::Array(y)) => Arc::ptr_eq(x, y),
        (Value::Object(x), Value::Object(y)) => Arc::ptr_eq(x, y),
        (Value::Date(x), Value::Date(y)) => x == y,
        (Value::Number(_), _) | (_, Value::Number(_)) => false,
        _ => a == b,
    }
}

pub(crate) type Out = R<Value>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Engine, EngineOptions, Tz};
    use crate::{ErrorKind, Loader};

    struct NoPartials;

    impl Loader for NoPartials {
        fn load(&self, _: &str) -> Option<(String, String)> {
            None
        }
    }

    fn small(limit: usize) -> Engine {
        let mut e = Engine::new(EngineOptions {
            strict_filters: true,
            timezone: Tz::Utc,
        });
        e.output_limit = limit;
        e
    }

    fn run(e: &Engine, src: &str) -> Result<String, crate::LiquidError> {
        let t = e.parse("t", src)?;
        e.render(&t, &Value::Object(Default::default()), &NoPartials)
    }

    #[test]
    fn registers_88_filters() {
        let mut map = HashMap::new();
        register(&mut map);
        assert_eq!(map.len(), 88);
    }

    /// Filters count toward the limit even when nothing is printed.
    #[test]
    fn a_doubling_loop_stops_at_the_limit() {
        let e = small(1 << 16);
        for body in [
            "{% assign s = s | append: s %}",
            "{% assign s = s | prepend: s %}",
            "{% assign s = s | replace: 'x', 'xx' %}",
            "{% assign s = s | replace: '', 'x' %}",
            "{% assign s = s | split: '' | join: 'xx' %}",
            "{% assign s = s | replace_first: 'x', s %}",
            "{% assign s = s | replace_last: 'x', s %}",
        ] {
            let src =
                format!("{{% assign s = 'xx' %}}{{% for i in (1..64) %}}{body}{{% endfor %}}");
            let err = run(&e, &src).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Limit, "{body}: {err}");
            assert!(err.help.is_some(), "{body}");
            assert_eq!(err.line, 1, "{body}");
        }
        for body in [
            "{% assign a = a | concat: a %}",
            "{% assign a = a | push: 1 | concat: a %}",
        ] {
            let src =
                format!("{{% assign a = (1..3) %}}{{% for i in (1..64) %}}{body}{{% endfor %}}");
            let err = run(&e, &src).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Limit, "{body}: {err}");
        }
        // Just under the limit is fine.
        let ok = run(
            &e,
            "{% assign s = 'x' %}{% for i in (1..15) %}{% assign s = s | append: s %}{% endfor %}{{ s | size }}",
        );
        assert_eq!(ok.unwrap(), "32768");
    }

    #[test]
    fn a_huge_date_width_is_a_limit_error() {
        let e = small(1 << 16);
        let err = run(&e, "{{ 0 | date: '%99999999Y' }}").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Limit);
        // LiquidJS: "Invalid string length". These used to panic or
        // allocate gigabytes.
        for w in [
            "99999999999999999999",
            "1000000000",
            "70000",
            "999999999999",
        ] {
            let start = std::time::Instant::now();
            for code in ['N', 'Y', 'e', 'L'] {
                let src = format!("{{{{ 0 | date: '%{w}{code}' }}}}");
                let err = run(&e, &src).unwrap_err();
                assert_eq!(err.kind, ErrorKind::Limit, "{src}");
            }
            assert!(start.elapsed().as_secs() < 2);
        }
        // Real-limit engine: must fail fast, not build a gigabyte.
        let real = Engine::new(EngineOptions {
            strict_filters: true,
            timezone: Tz::Utc,
        });
        let start = std::time::Instant::now();
        for w in ["99999999999999999999", "1000000000"] {
            let err = run(&real, &format!("{{{{ 0 | date: '%{w}N' }}}}")).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Limit);
            assert_eq!(err.message, "Invalid string length");
        }
        assert!(start.elapsed().as_secs() < 2);
    }
}
