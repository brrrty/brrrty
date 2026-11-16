//! `date` and friends, with LiquidJS's `strftime`.

use std::sync::OnceLock;

use brrrty_js::Value;
use regex::Regex;

use super::{Args, Out, is_nil, jsdate, string, stringify};
use crate::context::Val;
use crate::engine::Tz;
use crate::error::ErrorKind;
use crate::render::{R, RenderErr, Renderer};
use brrrty_js::tz::{self, Fields, MAX_TIME};

const DEFAULT_FORMAT: &str = "%A, %B %-e, %Y at %-l:%M %P %z";

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// LiquidJS's `LiquidDate`: an instant, shown in the process's zone or at a
/// fixed offset.
struct LiquidDate {
    /// The instant, in ms since the epoch.
    ms: f64,
    tz: Tz,
    /// `Some` when the `date` filter was given a zone.
    fixed: Option<Fixed>,
    /// The wall-clock fields to show.
    shown: Fields,
}

enum Fixed {
    /// Minutes west of UTC.
    Offset(f64),
    /// A zone name; only UTC's names are known.
    Name(f64, String),
}

impl LiquidDate {
    fn new(ms: f64, tz: Tz, fixed: Option<Fixed>) -> Option<LiquidDate> {
        let own = tz::fields(tz, ms)?;
        let shown = match &fixed {
            None => own,
            Some(Fixed::Offset(west) | Fixed::Name(west, _)) => {
                // `displayDate` is the instant moved by the difference
                // between the local offset and the requested one, read in
                // the local zone.
                let moved = ms + (own.offset_west - west) * 60_000.0;
                tz::fields(tz, moved)?
            }
        };
        Some(LiquidDate {
            ms,
            tz,
            fixed,
            shown,
        })
    }

    /// `getTimezoneOffset()`: minutes west of UTC.
    fn offset_west(&self) -> f64 {
        match &self.fixed {
            None => self.shown.offset_west,
            Some(Fixed::Offset(w) | Fixed::Name(w, _)) => *w,
        }
    }

    fn zone_name(&self) -> Option<String> {
        match &self.fixed {
            Some(Fixed::Offset(_)) => None,
            Some(Fixed::Name(_, name)) => Some(name.clone()).filter(|n| !n.is_empty()),
            None => tz::zone_id(self.tz).map(str::to_string),
        }
    }
}

/// `parseDate`: `None` when there is no date or it is invalid.
fn parse_date(r: &Renderer<'_>, v: &Value, timezone: &Value) -> R<Option<LiquidDate>> {
    let tz = r.tz();
    let fixed = match timezone {
        // `timezoneOffset ?? opts.timezoneOffset`.
        Value::Undefined | Value::Null => None,
        Value::Number(n) => Some(Fixed::Offset(*n)),
        Value::String(name) => Some(zone_by_name(name)?),
        // Anything else is used as a number of minutes: `true` is one.
        other => Some(Fixed::Offset(other.to_js_number())),
    };
    let ms = match v {
        Value::Null | Value::Undefined => return Ok(None),
        Value::String(s) if &**s == "now" || &**s == "today" => now_ms(),
        Value::Number(n) => time_clip(n * 1000.0),
        Value::String(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
            time_clip(s.parse::<f64>().unwrap_or(f64::NAN) * 1000.0)
        }
        Value::String(s) => jsdate::parse(s, tz).unwrap_or(f64::NAN),
        Value::Date(d) => d.timestamp_millis() as f64,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Array(_) | Value::Object(_) => {
            jsdate::parse(&crate::ops::to_js_string(v, tz), tz).unwrap_or(f64::NAN)
        }
    };
    if ms.is_nan() {
        return Ok(None);
    }
    Ok(LiquidDate::new(ms, tz, fixed))
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_millis() as f64)
}

/// `TimeClip`: out of range is `NaN`, anything else loses its fraction.
fn time_clip(t: f64) -> f64 {
    if !t.is_finite() || t.abs() > MAX_TIME {
        f64::NAN
    } else {
        t.trunc() + 0.0
    }
}

/// A named zone, as `date: format, 'UTC'` gives. LiquidJS asks `Intl` for
/// the zone's offset. Only UTC's names are known here, because the offsets
/// of other zones need the time zone database; a name `Intl` would not
/// know is an error with its message.
fn zone_by_name(name: &str) -> R<Fixed> {
    let upper = name.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "UTC" | "GMT" | "ETC/UTC" | "ETC/GMT" | "UCT" | "ETC/UCT" | "ZULU" | "ETC/ZULU"
    ) {
        return Ok(Fixed::Name(0.0, name.to_string()));
    }
    if !tz::is_known_zone(name) {
        return Err(RenderErr::new(
            ErrorKind::Render,
            format!("Invalid time zone specified: {name}"),
        ));
    }
    Err(RenderErr::new(
        ErrorKind::Render,
        format!("the time zone `{name}` is not supported by the date filter yet"),
    )
    .with_help("use UTC or a number of minutes west of UTC, such as 300 for New York in winter"))
}

// ---- strftime --------------------------------------------------------------------------

fn format_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"%([-_0^#:]+)?([0-9]+)?([EO])?([^\n\r\u{2028}\u{2029}])").unwrap())
}

fn is_leap_year(year: i64) -> bool {
    (year & 3) == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn day_of_year(f: &Fields) -> u32 {
    let feb = if is_leap_year(f.year) { 29 } else { 28 };
    let days = [31, feb, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    days[..f.month0 as usize].iter().sum::<u32>() + f.day
}

fn week_of_year(f: &Fields, start_day: u32) -> u32 {
    let since = (f.weekday + 7 - start_day) % 7;
    (day_of_year(f) - 1 + 7 - since) / 7
}

fn ordinal(day: u32) -> &'static str {
    if matches!(day, 11..=13) {
        return "th";
    }
    match day % 10 {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    }
}

fn hour12(f: &Fields) -> u32 {
    if f.hour.is_multiple_of(12) {
        12
    } else {
        f.hour % 12
    }
}

fn pad_start(s: &str, width: usize, ch: char) -> String {
    let len = super::utf16_len(s);
    if len >= width {
        return s.to_string();
    }
    let mut out: String = std::iter::repeat_n(ch, width - len).collect();
    out.push_str(s);
    out
}

fn pad_end(s: &str, width: usize, ch: char) -> String {
    let len = super::utf16_len(s);
    let mut out = s.to_string();
    out.extend(std::iter::repeat_n(ch, width.saturating_sub(len)));
    out
}

/// `+hhmm`, with a colon when the `:` flag is set.
fn offset_text(d: &LiquidDate, colon: bool) -> String {
    let west = d.offset_west();
    let abs = west.abs();
    let (h, m) = ((abs / 60.0).floor(), abs % 60.0);
    format!(
        "{}{}{}{}",
        if west > 0.0 { '-' } else { '+' },
        pad_start(&js_int(h), 2, '0'),
        if colon { ":" } else { "" },
        pad_start(&js_int(m), 2, '0'),
    )
}

fn js_int(n: f64) -> String {
    brrrty_js::js_number_to_string(n)
}

/// `toLocaleDateString` and friends in `en-US`.
fn locale_date(f: &Fields) -> String {
    format!("{}/{}/{}", f.month0 + 1, f.day, f.year)
}

fn locale_time(f: &Fields) -> String {
    format!(
        "{}:{:02}:{:02} {}",
        hour12(f),
        f.minute,
        f.second,
        if f.hour < 12 { "AM" } else { "PM" }
    )
}

/// What one conversion gives, or `None` for a code LiquidJS does not have.
fn convert(d: &LiquidDate, code: char, colon: bool, width: Option<&str>) -> Option<String> {
    let f = &d.shown;
    Some(match code {
        'a' => WEEKDAYS[f.weekday as usize][..3].to_string(),
        'A' => WEEKDAYS[f.weekday as usize].to_string(),
        'b' | 'h' => MONTHS[f.month0 as usize][..3].to_string(),
        'B' => MONTHS[f.month0 as usize].to_string(),
        'c' => format!("{}, {}", locale_date(f), locale_time(f)),
        'C' => f.year.div_euclid(100).to_string(),
        'd' | 'e' => f.day.to_string(),
        'H' | 'k' => f.hour.to_string(),
        'I' | 'l' => hour12(f).to_string(),
        'j' => day_of_year(f).to_string(),
        'L' => f.millis.to_string(),
        'm' => (f.month0 + 1).to_string(),
        'M' => f.minute.to_string(),
        'N' => {
            let w = width
                .and_then(|w| w.parse::<f64>().ok())
                .filter(|w| *w != 0.0 && !w.is_nan())
                .unwrap_or(9.0) as usize;
            let digits = pad_start(&f.millis.to_string(), 3, '0');
            let kept: String = digits.chars().take(w).collect();
            pad_end(&kept, w, '0')
        }
        'p' => if f.hour < 12 { "AM" } else { "PM" }.to_string(),
        'P' => if f.hour < 12 { "am" } else { "pm" }.to_string(),
        'q' => ordinal(f.day).to_string(),
        's' => js_int((d.ms / 1000.0).floor()),
        'S' => f.second.to_string(),
        'u' => if f.weekday == 0 { 7 } else { f.weekday }.to_string(),
        'U' => week_of_year(f, 0).to_string(),
        'w' => f.weekday.to_string(),
        'W' => week_of_year(f, 1).to_string(),
        'x' => locale_date(f),
        'X' => locale_time(f),
        'y' => (f.year % 100 + 100).rem_euclid(100).to_string(),
        'Y' => f.year.to_string(),
        'z' => offset_text(d, colon),
        'Z' => d.zone_name().unwrap_or_else(|| offset_text(d, colon)),
        't' => "\t".into(),
        'n' => "\n".into(),
        '%' => "%".into(),
        _ => return None,
    })
}

/// `changeCase`: text with any lower-case letter is upper-cased, and text
/// without one is lower-cased.
fn change_case(s: &str) -> String {
    if s.chars().any(|c| c.is_ascii_lowercase()) {
        s.to_uppercase()
    } else {
        s.to_lowercase()
    }
}

/// LiquidJS's `strftime`.
fn strftime(r: &Renderer<'_>, d: &LiquidDate, format: &str) -> R<String> {
    let mut out = String::new();
    let mut last = 0;
    for m in format_regex().captures_iter(format) {
        let whole = m.get(0).unwrap();
        out.push_str(&format[last..whole.start()]);
        last = whole.end();
        let flags = m.get(1).map_or("", |g| g.as_str());
        let width = m.get(2).map(|g| g.as_str());
        let code = m.get(4).unwrap().as_str().chars().next().unwrap();
        let has = |c: char| flags.contains(c);
        // `%<width>N` builds its digits before any padding, so a width
        // beyond the limit is refused first, as JavaScript refuses a
        // string that long.
        if code == 'N'
            && let Some(w) = width.and_then(|w| w.parse::<f64>().ok())
            && w > r.engine.output_limit as f64
        {
            return Err(RenderErr::new(ErrorKind::Limit, "Invalid string length")
                .with_help("the width of %N is larger than Brrrty allows in one output"));
        }
        let Some(text) = convert(d, code, has(':'), width) else {
            out.push_str(whole.as_str());
            continue;
        };
        let mut text = text;
        let mut pad_char = if "aAbBceklpP".contains(code) {
            ' '
        } else {
            '0'
        };
        let mut pad_width = width
            .and_then(|w| w.parse::<f64>().ok())
            .filter(|w| *w != 0.0)
            .unwrap_or(match code {
                'C' | 'd' | 'e' | 'H' | 'I' | 'k' | 'l' | 'm' | 'M' | 'S' | 'U' | 'W' | 'y' => 2.0,
                'j' | 'L' => 3.0,
                'Y' => 4.0,
                _ => 0.0,
            });
        if has('^') {
            text = text.to_uppercase();
        } else if has('#') {
            text = change_case(&text);
        }
        if has('_') {
            pad_char = ' ';
        } else if has('0') {
            pad_char = '0';
        }
        if has('-') {
            pad_width = 0.0;
        }
        // A width this large is a `RangeError` in JavaScript.
        r.check_bytes(pad_width.min(f64::from(u32::MAX)) as usize)?;
        let width = pad_width as usize;
        let padded = if pad_char == '0' && text.starts_with('-') {
            format!("-{}", pad_start(&text[1..], width.saturating_sub(1), '0'))
        } else {
            pad_start(&text, width, pad_char)
        };
        r.check_bytes(out.len() + padded.len())?;
        out.push_str(&padded);
    }
    out.push_str(&format[last..]);
    Ok(out)
}

// ---- the filters -------------------------------------------------------------------------

pub(super) fn date(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    let a = Args(a);
    let input = v.to_value();
    let Some(d) = parse_date(r, &input, &a.value(1))? else {
        return Ok(input);
    };
    let format = match a.value(0) {
        f if is_nil(&f) => DEFAULT_FORMAT.to_string(),
        f => stringify(r, &f),
    };
    Ok(string(strftime(r, &d, &format)?))
}

pub(super) fn date_to_xmlschema(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let input = v.to_value();
    let Some(d) = parse_date(r, &input, &Value::Undefined)? else {
        return Ok(input);
    };
    Ok(string(strftime(r, &d, "%Y-%m-%dT%H:%M:%S%:z")?))
}

pub(super) fn date_to_rfc822(r: &mut Renderer<'_>, v: &Val, _: &[Val]) -> Out {
    let input = v.to_value();
    let Some(d) = parse_date(r, &input, &Value::Undefined)? else {
        return Ok(input);
    };
    Ok(string(strftime(r, &d, "%a, %d %b %Y %H:%M:%S %z")?))
}

fn stringify_date(r: &mut Renderer<'_>, v: &Val, a: &[Val], month: &str) -> Out {
    let a = Args(a);
    let input = v.to_value();
    let Some(d) = parse_date(r, &input, &Value::Undefined)? else {
        return Ok(input);
    };
    let ordinal = matches!(a.value(0), Value::String(t) if &*t == "ordinal");
    let us = matches!(a.value(1), Value::String(s) if &*s == "US");
    let day = d.shown.day;
    let format = match (ordinal, us) {
        (true, true) => format!("{month} {day}%q, %Y"),
        (true, false) => format!("{day}%q {month} %Y"),
        _ => format!("%d {month} %Y"),
    };
    Ok(string(strftime(r, &d, &format)?))
}

pub(super) fn date_to_string(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    stringify_date(r, v, a, "%b")
}

pub(super) fn date_to_long_string(r: &mut Renderer<'_>, v: &Val, a: &[Val]) -> Out {
    stringify_date(r, v, a, "%B")
}
