//! Luxon's `DateTime.fromJSDate(d, { zone }).toFormat(fmt)`, ported from
//! Luxon 3.7.2 (`src/impl/formatter.js`) for the `en-US` locale.
//!
//! Every token Luxon knows is supported: the field tokens (`yyyy`, `LLL`,
//! `EEEE`, `HH`, `a`, `SSS`, `ZZ`, `z`, `W`, `o`, `q`, `X`, `x`, ...) and the
//! macro tokens (`D`, `DD`, `t`, `T`, `f`, `F` and their longer forms, which
//! Luxon hands to `Intl.DateTimeFormat`; their `en-US` patterns are written
//! out here and pinned against Node).
//!
//! Zones: `"utc"` and `"gmt"`, fixed offsets (`"UTC+5"`, `"utc-3:30"`), and
//! IANA names (any case, as ICU accepts them). A zone Luxon can't use gives
//! `"Invalid DateTime"`, as it does. `"local"`, `"system"` and `"default"`
//! depend on the machine and are refused.
//!
//! Limits, each refused with an error rather than guessed:
//! - Any locale but `en-US` (or `en`).
//! - A zone *name* (`ZZZZ`, `ZZZZZ`, and the `ttt`/`tttt`/`TTT`/`TTTT`/`fff`/
//!   `ffff`/`FFF`/`FFFF` macros) in an IANA zone before 1970, or for an offset
//!   that had two different names from 1970 to 2038. The names come from a
//!   table made from Node's ICU (`luxon_zones.rs`).
//! - An instant outside chrono's calendar (about year +-262143) in an IANA zone.
//! - Any instant from 2100 in an IANA zone with daylight saving: chrono-tz has no rules past
//!   2099, so it would give the standard offset all year. Zones without DST, fixed offsets
//!   and UTC are fine.

use brrrty_js::Tz;
use brrrty_js::tz::{self, Fields};
use chrono::{Offset, TimeZone};

use super::luxon_zones::{ALIASES, NAMES};

const MONTHS_LONG: [&str; 12] = [
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
const MONTHS_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MONTHS_NARROW: [&str; 12] = ["J", "F", "M", "A", "M", "J", "J", "A", "S", "O", "N", "D"];
/// Monday first, as Luxon's English table.
const DAYS_LONG: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const DAYS_SHORT: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const DAYS_NARROW: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];

/// The text Luxon gives for a date it can't use.
pub const INVALID: &str = "Invalid DateTime";

/// `DateTime.fromJSDate(new Date(ms), { zone }).toFormat(fmt, { locale })`.
/// `ms` is `NaN` for an Invalid Date (or for anything that isn't a Date).
pub fn luxon_format(ms: f64, fmt: &str, zone: &str, locale: &str) -> Result<String, String> {
    if !(locale.eq_ignore_ascii_case("en-US") || locale.eq_ignore_ascii_case("en")) {
        return Err(format!(
            "luxon_format: the locale `{locale}` is not supported, only en-US"
        ));
    }
    let zone = match resolve_zone(zone)? {
        Some(z) => z,
        None => return Ok(INVALID.into()),
    };
    if ms.is_nan() || ms.abs() > tz::MAX_TIME {
        return Ok(INVALID.into());
    }
    let Some(dt) = Dt::new(ms, zone)? else {
        return Ok(INVALID.into());
    };
    let mut out = String::new();
    for (literal, val) in parse_format(fmt) {
        if literal {
            out.push_str(&val);
        } else {
            out.push_str(&dt.token(&val)?);
        }
    }
    Ok(out)
}

/// JavaScript's `\s`.
fn js_space(c: char) -> bool {
    brrrty_js::is_js_whitespace(c)
}

fn all_space(s: &str) -> bool {
    !s.is_empty() && s.chars().all(js_space)
}

/// `Formatter.parseFormat`: `(literal, text)` pieces.
fn parse_format(fmt: &str) -> Vec<(bool, String)> {
    let mut splits = Vec::new();
    let mut current: Option<char> = None;
    let mut full = String::new();
    let mut bracketed = false;
    for c in fmt.chars() {
        if c == '\'' {
            if !full.is_empty() || bracketed {
                let val = if full.is_empty() {
                    "'".to_string()
                } else {
                    full.clone()
                };
                splits.push((bracketed || all_space(&full), val));
            }
            current = None;
            full.clear();
            bracketed = !bracketed;
        } else if bracketed || Some(c) == current {
            full.push(c);
        } else {
            if !full.is_empty() {
                splits.push((all_space(&full), full.clone()));
            }
            full.clear();
            full.push(c);
            current = Some(c);
        }
    }
    if !full.is_empty() {
        splits.push((bracketed || all_space(&full), full));
    }
    splits
}

enum Zone {
    /// Offset in minutes. Luxon's `FixedOffsetZone`.
    Fixed(i64),
    Iana {
        /// As the caller wrote it (`z` prints it).
        name: String,
        tz: chrono_tz::Tz,
        /// The id ICU resolves it to.
        id: &'static str,
    },
}

/// `normalizeZone`. `None` is an invalid zone.
fn resolve_zone(input: &str) -> Result<Option<Zone>, String> {
    let lowered = input.to_lowercase();
    match lowered.as_str() {
        "default" | "local" | "system" => {
            return Err(format!(
                "luxon_format: the zone `{input}` depends on the machine and is not supported; name a zone"
            ));
        }
        "utc" | "gmt" => return Ok(Some(Zone::Fixed(0))),
        _ => {}
    }
    if let Some(m) = parse_fixed(&lowered) {
        return Ok(Some(Zone::Fixed(m)));
    }
    let Ok(i) = ALIASES.binary_search_by_key(&lowered.as_str(), |(l, _, _)| *l) else {
        return Ok(None);
    };
    let (_, spelled, id) = ALIASES[i];
    let Some(tz) = chrono_tz::TZ_VARIANTS.iter().find(|t| t.name() == spelled) else {
        return Err(format!(
            "luxon_format: the zone `{input}` is not in Brrrty's time zone data"
        ));
    };
    Ok(Some(Zone::Iana {
        name: input.to_string(),
        tz: *tz,
        id,
    }))
}

/// `FixedOffsetZone.parseSpecifier`: `^utc(?:([+-]\d{1,2})(?::(\d{2}))?)?$`
/// on the lower-cased text. A bare `utc` was handled before.
fn parse_fixed(s: &str) -> Option<i64> {
    let rest = s.strip_prefix("utc")?;
    let b = rest.as_bytes();
    if b.is_empty() {
        return Some(0);
    }
    let neg = match b[0] {
        b'+' => false,
        b'-' => true,
        _ => return None,
    };
    let digits = b[1..].iter().take_while(|c| c.is_ascii_digit()).count();
    if !(1..=2).contains(&digits) {
        return None;
    }
    let hour: i64 = rest[1..1 + digits].parse().ok()?;
    let tail = &rest[1 + digits..];
    let minute: i64 = if tail.is_empty() {
        0
    } else {
        let m = tail.strip_prefix(':')?;
        if m.len() != 2 || !m.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        m.parse().ok()?
    };
    // `signedOffset`: the minutes take the hour's sign.
    Some(if neg {
        -(hour * 60) - minute
    } else {
        hour * 60 + minute
    })
}

/// Luxon's `formatOffset(offset, format)`. `offset` is in minutes.
fn format_offset(offset: f64, format: &str) -> String {
    let hours = (offset / 60.0).abs().trunc() as i64;
    let minutes = (offset % 60.0).abs().trunc() as i64;
    let sign = if offset >= 0.0 { '+' } else { '-' };
    match format {
        "short" => format!("{sign}{hours:02}:{minutes:02}"),
        "narrow" if minutes > 0 => format!("{sign}{hours}:{minutes}"),
        "narrow" => format!("{sign}{hours}"),
        _ => format!("{sign}{hours:02}{minutes:02}"),
    }
}

/// `padStart(n, p)` / `Intl.NumberFormat` without grouping and with `p`
/// minimum integer digits: a sign, then the digits padded with zeros.
fn num(n: i64, p: usize) -> String {
    let digits = n.unsigned_abs().to_string();
    format!("{}{digits:0>p$}", if n < 0 { "-" } else { "" })
}

/// The last two characters of the year's text, as `String(year).slice(-2)`,
/// then formatted as a number with two digits.
fn two_digit_year(year: i64) -> String {
    let s = year.to_string();
    let tail: String = s
        .chars()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    // A lone "-" can't happen: the sign is only ever in front of a digit.
    num(tail.parse::<i64>().unwrap_or(0), 2)
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_year(y: i64) -> i64 {
    if is_leap(y) { 366 } else { 365 }
}

/// Luxon's `dayOfWeek`: 1 is Monday. For years 0 to 99 it builds the date
/// in 1900 + year and then moves it back, which puts February 29 of year 0
/// on March 1 (1900 has no leap day).
fn day_of_week(year: i64, month: i64, day: i64) -> i64 {
    let days = if (0..100).contains(&year) {
        // Date.UTC(year + 1900, ...), then setUTCFullYear(year) keeping the
        // month and day the first step produced.
        let d = tz::days_from_civil(year + 1900, month, day);
        let (_, m, dd) = civil(d);
        tz::days_from_civil(year, m as i64, dd as i64)
    } else {
        tz::days_from_civil(year, month, day)
    };
    let js = (days + 4).rem_euclid(7);
    if js == 0 { 7 } else { js }
}

/// Month and day of a day number, by way of the zone-free `fields`.
fn civil(days: i64) -> (i64, u32, u32) {
    let f = tz::fields(Tz::Utc, days as f64 * tz::MS_PER_DAY).expect("in range");
    (f.year, f.month0 + 1, f.day)
}

const NON_LEAP: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
const LEAP: [i64; 12] = [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335];

fn ordinal(year: i64, month: i64, day: i64) -> i64 {
    day + (if is_leap(year) { LEAP } else { NON_LEAP })[(month - 1) as usize]
}

fn iso_weekday_to_local(iso: i64, start: i64) -> i64 {
    (iso - start + 7) % 7 + 1
}

fn first_week_offset(year: i64, min_days: i64, start: i64) -> i64 {
    let fwdlw = iso_weekday_to_local(day_of_week(year, 1, min_days), start);
    -fwdlw + min_days - 1
}

fn weeks_in_week_year(year: i64, min_days: i64, start: i64) -> i64 {
    let o = first_week_offset(year, min_days, start);
    let next = first_week_offset(year + 1, min_days, start);
    (days_in_year(year) - o + next) / 7
}

/// `gregorianToWeek`: `(week year, week number, weekday)`.
fn gregorian_to_week(y: i64, m: i64, d: i64, min_days: i64, start: i64) -> (i64, i64, i64) {
    let ord = ordinal(y, m, d);
    let weekday = iso_weekday_to_local(day_of_week(y, m, d), start);
    let mut week = (ord - weekday + 14 - min_days).div_euclid(7);
    let week_year;
    if week < 1 {
        week_year = y - 1;
        week = weeks_in_week_year(week_year, min_days, start);
    } else if week > weeks_in_week_year(y, min_days, start) {
        week_year = y + 1;
        week = 1;
    } else {
        week_year = y;
    }
    (week_year, week, weekday)
}

/// The DateTime: an instant, a zone, and the wall-clock fields in it.
struct Dt {
    ts: f64,
    zone: Zone,
    f: Fields,
    /// Minutes east of UTC. Fractional for the historical offsets with seconds.
    offset: f64,
}

/// 2100-01-01T00:00:00Z. chrono-tz holds no rules past 2099, so after this a zone that
/// changes its offset through the year would be given its standard offset all year.
const RULES_END_MS: f64 = 4_102_444_800_000.0;

/// Whether the zone's offset changes through 2099 (so it would be wrong in 2100 and later).
fn has_rules_after_2099(tz: &chrono_tz::Tz) -> bool {
    let offset = |month: u32| {
        let t = chrono::NaiveDate::from_ymd_opt(2099, month, 15)
            .and_then(|d| d.and_hms_opt(12, 0, 0))
            .map(|d| d.and_utc().timestamp_millis());
        t.and_then(|t| tz.timestamp_millis_opt(t).single())
            .map(|d| d.offset().fix().local_minus_utc())
    };
    let first = offset(1);
    (2..=12).any(|m| offset(m) != first)
}

fn zone_name_of(z: &Zone) -> String {
    match z {
        Zone::Iana { name, .. } => name.clone(),
        Zone::Fixed(_) => "UTC".into(),
    }
}

impl Dt {
    fn new(ts: f64, zone: Zone) -> Result<Option<Dt>, String> {
        let offset = match &zone {
            Zone::Fixed(m) => *m as f64,
            Zone::Iana { tz, .. } => {
                let Some(at) = tz.timestamp_millis_opt(ts as i64).single() else {
                    return Err(
                        "luxon_format: that date is outside the range Brrrty can look time zone offsets up for"
                            .into(),
                    );
                };
                if ts >= RULES_END_MS && has_rules_after_2099(tz) {
                    return Err(format!(
                        "luxon_format: `{}` has daylight saving rules, and Brrrty's time zone data has none from 2100, so it can't give that zone's time then",
                        zone_name_of(&zone)
                    ));
                }
                f64::from(at.offset().fix().local_minus_utc()) / 60.0
            }
        };
        match tz::fields(Tz::Utc, ts + offset * 60_000.0) {
            None => Ok(None),
            Some(f) => Ok(Some(Dt {
                ts,
                zone,
                f,
                offset,
            })),
        }
    }

    fn year(&self) -> i64 {
        self.f.year
    }
    fn month(&self) -> i64 {
        i64::from(self.f.month0) + 1
    }
    fn day(&self) -> i64 {
        i64::from(self.f.day)
    }
    fn hour(&self) -> i64 {
        i64::from(self.f.hour)
    }
    fn hour12(&self) -> i64 {
        if self.hour() % 12 == 0 {
            12
        } else {
            self.hour() % 12
        }
    }
    /// ISO weekday, 1 is Monday, by Luxon's `dayOfWeek`.
    fn weekday(&self) -> i64 {
        day_of_week(self.year(), self.month(), self.day())
    }

    /// `(week year, week number, weekday)`, ISO.
    fn iso_week(&self) -> (i64, i64, i64) {
        gregorian_to_week(self.year(), self.month(), self.day(), 4, 1)
    }

    /// The same for en-US as Luxon has it: weeks start on Sunday, and week 1 is the first with four days.
    fn local_week(&self) -> (i64, i64, i64) {
        gregorian_to_week(self.year(), self.month(), self.day(), 4, 7)
    }

    fn zone_name(&self) -> String {
        match &self.zone {
            Zone::Fixed(0) => "UTC".into(),
            Zone::Fixed(m) => format!("UTC{}", format_offset(*m as f64, "narrow")),
            Zone::Iana { name, .. } => name.clone(),
        }
    }

    /// `zone.offsetName(ts, { format })`: `short` or `long`.
    fn offset_name(&self, long: bool) -> Result<String, String> {
        match &self.zone {
            Zone::Fixed(_) => Ok(self.zone_name()),
            Zone::Iana { id, .. } => iana_name(id, self.ts, self.offset, long),
        }
    }

    /// The zone name `Intl` adds to a macro token's output.
    fn intl_zone_name(&self, long: bool) -> Result<String, String> {
        match &self.zone {
            // Whole-hour offsets from -12 to +14 are `Etc/GMT` zones, which
            // ICU names `GMT+5` and `GMT+05:00`. Anything else is printed
            // with Luxon's own name.
            Zone::Fixed(m) if *m != 0 && m % 60 == 0 && (-12 * 60..=14 * 60).contains(m) => {
                Ok(gmt_name(*m as f64, long))
            }
            _ => self.offset_name(long),
        }
    }

    fn token(&self, token: &str) -> Result<String, String> {
        let f = &self.f;
        let ms = i64::from(f.millis);
        Ok(match token {
            "S" => num(ms, 0),
            "u" | "SSS" => num(ms, 3),
            "s" => num(i64::from(f.second), 0),
            "ss" => num(i64::from(f.second), 2),
            "uu" => num(ms / 10, 2),
            "uuu" => num(ms / 100, 0),
            "m" => num(i64::from(f.minute), 0),
            "mm" => num(i64::from(f.minute), 2),
            "h" => num(self.hour12(), 0),
            "hh" => num(self.hour12(), 2),
            "H" => num(self.hour(), 0),
            "HH" => num(self.hour(), 2),
            "Z" => format_offset(self.offset, "narrow"),
            "ZZ" => format_offset(self.offset, "short"),
            "ZZZ" => format_offset(self.offset, "techie"),
            "ZZZZ" => self.offset_name(false)?,
            "ZZZZZ" => self.offset_name(true)?,
            "z" => self.zone_name(),
            "a" => (if self.hour() < 12 { "AM" } else { "PM" }).into(),
            "d" => num(self.day(), 0),
            "dd" => num(self.day(), 2),
            "c" | "E" => num(self.weekday(), 0),
            "ccc" | "EEE" => DAYS_SHORT[(self.weekday() - 1) as usize].into(),
            "cccc" | "EEEE" => DAYS_LONG[(self.weekday() - 1) as usize].into(),
            "ccccc" | "EEEEE" => DAYS_NARROW[(self.weekday() - 1) as usize].into(),
            "L" | "M" => num(self.month(), 0),
            "LL" | "MM" => num(self.month(), 2),
            "LLL" | "MMM" => MONTHS_SHORT[f.month0 as usize].into(),
            "LLLL" | "MMMM" => MONTHS_LONG[f.month0 as usize].into(),
            "LLLLL" | "MMMMM" => MONTHS_NARROW[f.month0 as usize].into(),
            "y" => num(self.year(), 0),
            "yy" => two_digit_year(self.year()),
            "yyyy" => num(self.year(), 4),
            "yyyyyy" => num(self.year(), 6),
            "G" => (if self.year() < 0 { "BC" } else { "AD" }).into(),
            "GG" => (if self.year() < 0 {
                "Before Christ"
            } else {
                "Anno Domini"
            })
            .into(),
            "GGGGG" => (if self.year() < 0 { "B" } else { "A" }).into(),
            "kk" => two_digit_year(self.iso_week().0),
            "kkkk" => num(self.iso_week().0, 4),
            "W" => num(self.iso_week().1, 0),
            "WW" => num(self.iso_week().1, 2),
            "n" => num(self.local_week().1, 0),
            "nn" => num(self.local_week().1, 2),
            "ii" => two_digit_year(self.local_week().0),
            "iiii" => num(self.local_week().0, 4),
            "o" => num(ordinal(self.year(), self.month(), self.day()), 0),
            "ooo" => num(ordinal(self.year(), self.month(), self.day()), 3),
            "q" => num((self.month() + 2) / 3, 0),
            "qq" => num((self.month() + 2) / 3, 2),
            "X" => num((self.ts / 1000.0).floor() as i64, 0),
            "x" => num(self.ts as i64, 0),
            _ => match self.macro_token(token)? {
                Some(text) => text,
                None => token.to_string(),
            },
        })
    }

    /// The macro tokens, as `Intl.DateTimeFormat('en-US')` writes them.
    fn macro_token(&self, token: &str) -> Result<Option<String>, String> {
        // (date, time) parts: date 0 none, 1 numeric, 2 abbreviated month,
        // 3 long month, 4 long month with weekday. Time 0 none, 1 no
        // seconds, 2 seconds. `h23` for the upper-case T forms; zone 0 none,
        // 1 short, 2 long.
        let (date, time, h23, zone) = match token {
            "D" => (1, 0, false, 0),
            "DD" => (2, 0, false, 0),
            "DDD" => (3, 0, false, 0),
            "DDDD" => (4, 0, false, 0),
            "t" => (0, 1, false, 0),
            "tt" => (0, 2, false, 0),
            "ttt" => (0, 2, false, 1),
            "tttt" => (0, 2, false, 2),
            "T" => (0, 1, true, 0),
            "TT" => (0, 2, true, 0),
            "TTT" => (0, 2, true, 1),
            "TTTT" => (0, 2, true, 2),
            "f" => (1, 1, false, 0),
            "ff" => (2, 1, false, 0),
            "fff" => (3, 1, false, 1),
            "ffff" => (4, 1, false, 2),
            "F" => (1, 2, false, 0),
            "FF" => (2, 2, false, 0),
            "FFF" => (3, 2, false, 1),
            "FFFF" => (4, 2, false, 2),
            _ => return Ok(None),
        };
        let mut out = String::new();
        let f = &self.f;
        if date > 0 {
            // ICU writes a year before 1 as its absolute value plus one, with
            // no era, because none was asked for.
            let year = if f.year >= 1 { f.year } else { 1 - f.year };
            let (month, day) = (f.month0 as usize, f.day);
            out = match date {
                1 => format!("{}/{day}/{year}", month + 1),
                2 => format!("{} {day}, {year}", MONTHS_SHORT[month]),
                3 => format!("{} {day}, {year}", MONTHS_LONG[month]),
                _ => {
                    let wd = (f.weekday as usize + 6) % 7;
                    format!("{}, {} {day}, {year}", DAYS_LONG[wd], MONTHS_LONG[month])
                }
            };
        }
        if time > 0 {
            if date > 0 {
                out.push_str(if date >= 3 { " at " } else { ", " });
            }
            let seconds = if time == 2 {
                format!(":{:02}", f.second)
            } else {
                String::new()
            };
            if h23 {
                out.push_str(&format!("{:02}:{:02}{seconds}", f.hour, f.minute));
            } else {
                let ampm = if f.hour < 12 { "AM" } else { "PM" };
                // ICU puts a narrow no-break space before AM/PM, and V8 turns it into a space.
                out.push_str(&format!(
                    "{}:{:02}{seconds} {ampm}",
                    self.hour12(),
                    f.minute
                ));
            }
            if zone > 0 {
                out.push(' ');
                out.push_str(&self.intl_zone_name(zone == 2)?);
            }
        }
        Ok(Some(out))
    }
}

/// ICU's `GMT+5:30` (short) and `GMT+05:30` (long) forms.
fn gmt_name(offset: f64, long: bool) -> String {
    if offset == 0.0 {
        return "GMT".into();
    }
    let sign = if offset < 0.0 { '-' } else { '+' };
    let a = offset.abs().trunc() as i64;
    let (h, m) = (a / 60, a % 60);
    if long {
        format!("GMT{sign}{h:02}:{m:02}")
    } else if m > 0 {
        format!("GMT{sign}{h}:{m:02}")
    } else {
        format!("GMT{sign}{h}")
    }
}

/// The name ICU gives an IANA zone at `ts`: the short (`EST`) or long
/// (`Eastern Standard Time`) form, from the table made from Node.
fn iana_name(id: &str, ts: f64, offset: f64, long: bool) -> Result<String, String> {
    const FROM_1970_MS: f64 = 0.0;
    if ts < FROM_1970_MS {
        return Err(format!(
            "luxon_format: the zone name of `{id}` before 1970 is not supported (it depends on the zone's history)"
        ));
    }
    let offset_min = offset.trunc() as i16;
    let names = NAMES
        .binary_search_by_key(&id, |(i, _)| *i)
        .ok()
        .and_then(|i| NAMES[i].1.iter().find(|(o, ..)| *o == offset_min));
    match names {
        Some((_, _, _, since)) if ts < *since as f64 => Err(format!(
            "luxon_format: `{id}` had a different name for the offset {} before this date; Brrrty can't tell which one it had",
            format_offset(offset, "short")
        )),
        Some((_, short, longer, _)) => Ok((if long { *longer } else { *short }).to_string()),
        None => Ok(gmt_name(offset, long)),
    }
}
