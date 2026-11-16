//! YAML front matter parsed the way js-yaml 4 does it, the loader behind 11ty's
//! gray-matter.
//!
//! Scalar resolution is not what YAML crates that follow the YAML 1.2 core schema
//! do: js-yaml reads `0123` as 123, `.inf` as `Infinity`, and an unquoted timestamp
//! as a `Date`. Brrrty needs the raw scalar text and its quoting style to resolve
//! scalars itself, so this builds the value tree from `saphyr-parser` events instead
//! of deserializing through a typed API that has already decided each scalar's type.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use brrrty_js::Value;
use chrono::DateTime;
use indexmap::IndexMap;
use regex::Regex;
use saphyr_parser::{Event, Parser, ScalarStyle, Span, Tag};

/// A YAML error with its 1-based position inside the YAML text.
#[derive(Debug)]
pub(crate) struct YamlError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

type Events<'a> = std::iter::Peekable<Parser<'a, saphyr_parser::StrInput<'a>>>;

/// Parses a single YAML document the way `js-yaml`'s `load` does.
pub(crate) fn load(text: &str) -> Result<Value, YamlError> {
    let mut events: Events<'_> = Parser::new_from_str(text).peekable();
    let mut builder = Builder::default();
    let mut result = Value::Null;
    let mut documents = 0;
    while let Some(event) = events.next() {
        let (event, span) = event.map_err(|e| YamlError {
            message: e.info().to_string(),
            line: e.marker().line(),
            column: e.marker().col() + 1,
        })?;
        if matches!(event, Event::DocumentStart(_)) {
            documents += 1;
            if documents > 1 {
                return Err(error(
                    span,
                    "expected a single document in the stream, but found more",
                ));
            }
            builder.anchors.clear();
            builder.merged = 0;
            if let Some(Ok((next, _))) = events.peek()
                && matches!(next, Event::DocumentEnd)
            {
                continue;
            }
            result = builder.node(&mut events, 0)?;
        }
    }
    Ok(result)
}

fn error(span: Span, message: impl Into<String>) -> YamlError {
    YamlError {
        message: message.into(),
        line: span.start.line(),
        column: span.start.col() + 1,
    }
}

#[derive(Default)]
struct Builder {
    /// Anchored values with their nesting height.
    anchors: HashMap<usize, (Value, usize)>,
    /// Nesting height of the value `node` returned last (0 for a scalar).
    height: usize,
    /// Entries copied by `<<` merges so far.
    merged: usize,
}

/// The most entries `<<` merges may copy in one document. Chained merges copy
/// quadratically, and real front matter needs a handful.
const MAX_MERGED: usize = 100_000;

impl Builder {
    fn next<'a>(&self, events: &mut Events<'a>) -> Result<(Event<'a>, Span), YamlError> {
        match events.next() {
            Some(Ok(pair)) => Ok(pair),
            Some(Err(e)) => Err(YamlError {
                message: e.info().to_string(),
                line: e.marker().line(),
                column: e.marker().col() + 1,
            }),
            None => Err(YamlError {
                message: "unexpected end of the YAML".into(),
                line: 1,
                column: 1,
            }),
        }
    }

    fn node(&mut self, events: &mut Events<'_>, depth: usize) -> Result<Value, YamlError> {
        let (event, span) = self.next(events)?;
        let (value, anchor) = match event {
            Event::Alias(id) => {
                let (value, height) = self
                    .anchors
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| error(span, "unidentified alias"))?;
                // An alias is the anchored value at this depth, so it counts in full.
                check_depth(depth + height, span)?;
                self.height = height;
                return Ok(value);
            }
            Event::Scalar(text, style, anchor, tag) => {
                self.height = 0;
                (scalar(&text, style, tag.as_deref(), span)?, anchor)
            }
            Event::SequenceStart(anchor, tag) => {
                check_collection_tag(tag.as_deref(), "seq", span)?;
                check_depth(depth + 1, span)?;
                let mut items = Vec::new();
                let mut deepest = 0;
                while !matches!(events.peek(), Some(Ok((Event::SequenceEnd, _)))) {
                    items.push(self.node(events, depth + 1)?);
                    deepest = deepest.max(self.height);
                }
                self.next(events)?;
                self.height = deepest + 1;
                (Value::Array(Arc::new(items)), anchor)
            }
            Event::MappingStart(anchor, tag) => {
                check_collection_tag(tag.as_deref(), "map", span)?;
                check_depth(depth + 1, span)?;
                (self.mapping(events, depth + 1)?, anchor)
            }
            _ => return Err(error(span, "unexpected YAML structure")),
        };
        if anchor != 0 {
            self.anchors.insert(anchor, (value.clone(), self.height));
        }
        Ok(value)
    }

    fn mapping(&mut self, events: &mut Events<'_>, depth: usize) -> Result<Value, YamlError> {
        let mut map: IndexMap<String, Value> = IndexMap::new();
        // Keys that came from a `<<` merge; an explicit key may replace those.
        let mut overridable: HashSet<String> = HashSet::new();
        let mut deepest = 0;
        while !matches!(events.peek(), Some(Ok((Event::MappingEnd, _)))) {
            let is_merge = matches!(
                events.peek(),
                Some(Ok((Event::Scalar(t, ScalarStyle::Plain, _, None), _))) if t == "<<"
            );
            let key_span = match events.peek() {
                Some(Ok((_, span))) => *span,
                _ => Span::empty(saphyr_parser::Marker::new(0, 1, 0)),
            };
            if matches!(events.peek(), Some(Ok((Event::Alias(_), _)))) {
                // js-yaml hangs on these, so 11ty never finishes.
                return Err(error(key_span, "an alias can't be used as a mapping key"));
            }
            let key = self.node(events, depth)?;
            deepest = deepest.max(self.height);
            let value = self.node(events, depth)?;
            deepest = deepest.max(self.height);
            if is_merge {
                let sources: Vec<Value> = match &value {
                    Value::Array(items) => items.iter().cloned().collect(),
                    other => vec![other.clone()],
                };
                for source in sources {
                    let Value::Object(source) = source else {
                        return Err(error(
                            key_span,
                            "cannot merge mappings; the provided source object is unacceptable",
                        ));
                    };
                    self.merged += source.len();
                    if self.merged > MAX_MERGED {
                        return Err(error(key_span, "too many keys merged with `<<`"));
                    }
                    for (k, v) in source.iter() {
                        if !map.contains_key(k) {
                            map.insert(k.clone(), v.clone());
                            overridable.insert(k.clone());
                        }
                    }
                }
                continue;
            }
            let key = key_text(&key, key_span)?;
            if map.contains_key(&key) && !overridable.contains(&key) {
                return Err(error(key_span, "duplicated mapping key"));
            }
            overridable.remove(&key);
            map.insert(key, value);
        }
        self.next(events)?;
        self.height = deepest + 1;
        Ok(Value::Object(Arc::new(map)))
    }
}

/// The deepest nesting of mappings and sequences accepted.
const MAX_DEPTH: usize = 128;

fn check_depth(depth: usize, span: Span) -> Result<(), YamlError> {
    if depth > MAX_DEPTH {
        return Err(error(
            span,
            format!("nesting is deeper than {MAX_DEPTH} levels"),
        ));
    }
    Ok(())
}

/// A mapping key as JavaScript turns it into a property name.
fn key_text(key: &Value, span: Span) -> Result<String, YamlError> {
    if matches!(key, Value::Date(_)) {
        // `String(date)` depends on the machine's time zone.
        return Err(error(span, "a timestamp can't be used as a mapping key"));
    }
    Ok(key.to_js_string())
}

fn check_collection_tag(tag: Option<&Tag>, kind: &str, span: Span) -> Result<(), YamlError> {
    match tag {
        None => Ok(()),
        Some(t) if t.is_yaml_core_schema() && t.suffix == kind => Ok(()),
        Some(t) => Err(error(span, format!("unsupported YAML tag {t}"))),
    }
}

fn scalar(
    text: &str,
    style: ScalarStyle,
    tag: Option<&Tag>,
    span: Span,
) -> Result<Value, YamlError> {
    if let Some(tag) = tag {
        if !tag.is_yaml_core_schema() {
            return Err(error(span, format!("unknown tag {tag}")));
        }
        let expect = |ok: bool| {
            if ok {
                Ok(())
            } else {
                Err(error(span, format!("cannot resolve `{text}` as {tag}")))
            }
        };
        let resolved = resolve_plain(text);
        return match tag.suffix.as_str() {
            "str" => Ok(Value::String(text.into())),
            "null" => expect(matches!(resolved, Value::Null)).map(|()| Value::Null),
            "bool" => expect(matches!(resolved, Value::Bool(_))).map(|()| resolved),
            "int" | "float" => expect(matches!(resolved, Value::Number(_))).map(|()| resolved),
            "timestamp" => expect(matches!(resolved, Value::Date(_))).map(|()| resolved),
            _ => Err(error(span, format!("unsupported YAML tag {tag}"))),
        };
    }
    match style {
        ScalarStyle::Plain => Ok(resolve_plain(text)),
        _ => Ok(Value::String(text.into())),
    }
}

/// Resolves an unquoted scalar with js-yaml's default schema.
pub(crate) fn resolve_plain(text: &str) -> Value {
    if matches!(text, "" | "~" | "null" | "Null" | "NULL") {
        return Value::Null;
    }
    match text {
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        _ => {}
    }
    if let Some(n) = resolve_int(text) {
        return Value::Number(n);
    }
    if let Some(n) = resolve_float(text) {
        return Value::Number(n);
    }
    if let Some(d) = resolve_timestamp(text) {
        return d;
    }
    Value::String(text.into())
}

/// js-yaml's `int`: decimal (leading zeros allowed and read as decimal), and
/// `0b`, `0o` and `0x` with an optional sign. No `_` separators.
fn resolve_int(text: &str) -> Option<f64> {
    let (sign, rest) = match text.as_bytes().first()? {
        b'-' => (-1.0, &text[1..]),
        b'+' => (1.0, &text[1..]),
        _ => (1.0, text),
    };
    if rest == "0" {
        return Some(0.0);
    }
    let radix = match rest.as_bytes() {
        [b'0', b'b', ..] => Some(2),
        [b'0', b'o', ..] => Some(8),
        [b'0', b'x', ..] => Some(16),
        _ => None,
    };
    let value = match radix {
        Some(radix) => {
            let digits = &rest[2..];
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return None;
            }
            digits.chars().fold(0.0_f64, |acc, c| {
                acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0))
            })
        }
        None => {
            if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            rest.parse::<f64>().ok()?
        }
    };
    (value.is_finite()).then_some(sign * value)
}

static FLOAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:[-+]?[0-9]+(?:\.[0-9]*)?(?:[eE][-+]?[0-9]+)?|\.[0-9]+(?:[eE][-+]?[0-9]+)?|[-+]?\.(?:inf|Inf|INF)|\.(?:nan|NaN|NAN))$",
    )
    .expect("float pattern is valid")
});

/// js-yaml's `float`, including `.inf`, `-.inf` and `.nan`. Finite-looking
/// text that overflows (`1e400`) stays a string.
fn resolve_float(text: &str) -> Option<f64> {
    if !FLOAT.is_match(text) {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    let (sign, body) = match lower.as_bytes()[0] {
        b'-' => (-1.0, &lower[1..]),
        b'+' => (1.0, &lower[1..]),
        _ => (1.0, lower.as_str()),
    };
    match body {
        ".inf" => Some(sign * f64::INFINITY),
        ".nan" => Some(f64::NAN),
        _ => {
            let n: f64 = body.parse().ok()?;
            n.is_finite().then_some(sign * n)
        }
    }
}

static DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9]{4})-([0-9]{2})-([0-9]{2})$").expect("valid"));
static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^([0-9]{4})-([0-9][0-9]?)-([0-9][0-9]?)(?:[Tt]|[ \t]+)([0-9][0-9]?):([0-9]{2}):([0-9]{2})(?:\.([0-9]*))?(?:[ \t]*(Z|([-+])([0-9][0-9]?)(?::([0-9]{2}))?))?$",
    )
    .expect("valid")
});

/// js-yaml's `timestamp`: a date, or a date and time with an optional zone. No
/// zone means UTC. `Date.UTC` rolls out-of-range fields over (month 13 is next
/// January), and the instant has millisecond precision.
fn resolve_timestamp(text: &str) -> Option<Value> {
    let num = |c: &regex::Captures<'_>, i: usize| -> f64 {
        c.get(i).map_or(0.0, |m| m.as_str().parse().unwrap_or(0.0))
    };
    let (year, month, day, hour, minute, second, millis, delta) =
        if let Some(c) = DATE.captures(text) {
            (num(&c, 1), num(&c, 2), num(&c, 3), 0.0, 0.0, 0.0, 0.0, 0.0)
        } else {
            let c = TIMESTAMP.captures(text)?;
            let mut fraction = c.get(7).map_or("", |m| m.as_str()).to_string();
            fraction.truncate(3);
            while !fraction.is_empty() && fraction.len() < 3 {
                fraction.push('0');
            }
            let delta = c.get(9).map_or(0.0, |sign| {
                let minutes = num(&c, 10) * 60.0 + num(&c, 11);
                let ms = minutes * 60_000.0;
                if sign.as_str() == "-" { -ms } else { ms }
            });
            (
                num(&c, 1),
                num(&c, 2),
                num(&c, 3),
                num(&c, 4),
                num(&c, 5),
                num(&c, 6),
                fraction.parse().unwrap_or(0.0),
                delta,
            )
        };
    let ms = date_utc(year, month - 1.0, day, hour, minute, second, millis) - delta;
    DateTime::from_timestamp_millis(ms as i64).map(|d| Value::Date(d.fixed_offset()))
}

/// JavaScript's `Date.UTC`, in milliseconds. Years 0 to 99 mean 1900 to 1999.
fn date_utc(year: f64, month: f64, day: f64, h: f64, m: f64, s: f64, ms: f64) -> f64 {
    let year = if (0.0..=99.0).contains(&year) {
        1900.0 + year
    } else {
        year
    };
    let y = year + (month / 12.0).floor();
    let mo = month.rem_euclid(12.0);
    let days = days_from_civil(y as i64, mo as i64 + 1, 1) as f64 + (day - 1.0);
    ((days * 24.0 + h) * 60.0 + m) * 60_000.0 + s * 1000.0 + ms
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
