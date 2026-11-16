//! `new Date(string)`: V8's date parser, ported.
//!
//! It tries the ES5 ISO format first. Anything it does not accept goes to
//! V8's legacy parser, which takes month names, `AM`/`PM`, `GMT+0100`,
//! comments in parentheses and many other loose forms. A time with no
//! zone is local time, except an ISO date with no time, which is UTC.

use crate::engine::Tz;
use crate::tz::{self, MAX_TIME};

const NONE: i32 = i32::MAX;

#[derive(Clone, Copy, PartialEq)]
enum Kw {
    Month,
    AmPm,
    ZoneName,
    TimeSep,
    Invalid,
}

#[derive(Clone, Copy)]
enum Tok {
    Invalid,
    Unknown,
    Num { n: i32, len: usize },
    Sym(char),
    White,
    Word { kw: Kw, value: i32, len: usize },
    End,
}

impl Tok {
    fn is_number(self) -> bool {
        matches!(self, Tok::Num { .. })
    }
    fn is_sym(self, c: char) -> bool {
        matches!(self, Tok::Sym(s) if s == c)
    }
    fn is_sign(self) -> bool {
        self.is_sym('+') || self.is_sym('-')
    }
    fn is_fixed(self, len: usize) -> bool {
        matches!(self, Tok::Num { len: l, .. } if l == len)
    }
    fn number(self) -> i32 {
        match self {
            Tok::Num { n, .. } => n,
            _ => 0,
        }
    }
    fn is_z(self) -> bool {
        matches!(
            self,
            Tok::Word {
                kw: Kw::ZoneName,
                len: 1,
                value: 0
            }
        )
    }
    fn is_end(self) -> bool {
        matches!(self, Tok::End)
    }
    fn is_invalid(self) -> bool {
        matches!(self, Tok::Invalid)
    }
}

/// `{prefix, type, value}` rows of V8's keyword table.
const KEYWORDS: &[(&str, Kw, i32)] = &[
    ("jan", Kw::Month, 1),
    ("feb", Kw::Month, 2),
    ("mar", Kw::Month, 3),
    ("apr", Kw::Month, 4),
    ("may", Kw::Month, 5),
    ("jun", Kw::Month, 6),
    ("jul", Kw::Month, 7),
    ("aug", Kw::Month, 8),
    ("sep", Kw::Month, 9),
    ("oct", Kw::Month, 10),
    ("nov", Kw::Month, 11),
    ("dec", Kw::Month, 12),
    ("am", Kw::AmPm, 0),
    ("pm", Kw::AmPm, 12),
    ("ut", Kw::ZoneName, 0),
    ("utc", Kw::ZoneName, 0),
    ("z", Kw::ZoneName, 0),
    ("gmt", Kw::ZoneName, 0),
    ("cdt", Kw::ZoneName, -5),
    ("cst", Kw::ZoneName, -6),
    ("edt", Kw::ZoneName, -4),
    ("est", Kw::ZoneName, -5),
    ("mdt", Kw::ZoneName, -6),
    ("mst", Kw::ZoneName, -7),
    ("pdt", Kw::ZoneName, -7),
    ("pst", Kw::ZoneName, -8),
    ("t", Kw::TimeSep, 0),
];

fn is_space(c: char) -> bool {
    brrrty_js::is_js_whitespace(c)
}

struct Scanner {
    input: Vec<char>,
    pos: usize,
    next: Tok,
}

impl Scanner {
    fn new(s: &str) -> Scanner {
        let mut sc = Scanner {
            input: s.chars().collect(),
            pos: 0,
            next: Tok::End,
        };
        sc.next = sc.scan();
        sc
    }

    fn cur(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    fn scan(&mut self) -> Tok {
        let Some(c) = self.cur() else {
            return Tok::End;
        };
        if c.is_ascii_digit() {
            let start = self.pos;
            while self.cur() == Some('0') {
                self.pos += 1;
            }
            let (mut n, mut digits) = (0i32, 0);
            while let Some(d) = self.cur().and_then(|c| c.to_digit(10)) {
                if digits < 9 {
                    n = n * 10 + d as i32;
                }
                digits += 1;
                self.pos += 1;
            }
            return Tok::Num {
                n,
                len: self.pos - start,
            };
        }
        for s in [':', '-', '+', '.', ')'] {
            if c == s {
                self.pos += 1;
                return Tok::Sym(s);
            }
        }
        if c >= 'A' && !is_space(c) {
            let mut prefix = ['\0'; 3];
            let mut len = 0;
            while let Some(c) = self.cur() {
                if c < 'A' || is_space(c) {
                    break;
                }
                if len < 3 {
                    // `ch | 0x20` lowercases ASCII letters.
                    prefix[len] = if c.is_ascii() {
                        char::from((c as u8) | 0x20)
                    } else {
                        c
                    };
                }
                len += 1;
                self.pos += 1;
            }
            let word: String = prefix.iter().take_while(|c| **c != '\0').collect();
            let found = KEYWORDS
                .iter()
                .find(|(k, kw, _)| *k == word && (len <= 3 || *kw == Kw::Month));
            return match found {
                Some((_, kw, value)) => Tok::Word {
                    kw: *kw,
                    value: *value,
                    len,
                },
                None => Tok::Word {
                    kw: Kw::Invalid,
                    value: 0,
                    len,
                },
            };
        }
        if is_space(c) {
            while self.cur().is_some_and(is_space) {
                self.pos += 1;
            }
            return Tok::White;
        }
        if c == '(' {
            let mut balance = 0;
            loop {
                match self.cur() {
                    Some(')') => balance -= 1,
                    Some('(') => balance += 1,
                    _ => {}
                }
                self.pos += 1;
                if balance <= 0 || self.cur().is_none() {
                    break;
                }
            }
            return Tok::Unknown;
        }
        self.pos += 1;
        Tok::Unknown
    }

    fn next(&mut self) -> Tok {
        let t = self.next;
        self.next = self.scan();
        t
    }

    fn peek(&self) -> Tok {
        self.next
    }

    fn skip_symbol(&mut self, c: char) -> bool {
        if self.next.is_sym(c) {
            self.next();
            return true;
        }
        false
    }
}

fn between(x: i32, lo: i32, hi: i32) -> bool {
    lo <= x && x <= hi
}

#[derive(Default)]
struct Day {
    comp: [i32; 3],
    index: usize,
    named_month: Option<i32>,
    iso: bool,
}

impl Day {
    fn add(&mut self, n: i32) -> bool {
        if self.index < 3 {
            self.comp[self.index] = n;
            self.index += 1;
            return true;
        }
        false
    }

    fn is_day(x: i32) -> bool {
        between(x, 1, 31)
    }

    fn is_month(x: i32) -> bool {
        between(x, 1, 12)
    }

    fn is_empty(&self) -> bool {
        self.index == 0
    }

    /// `(year, month 0-based, day)`.
    fn write(&mut self) -> Option<(i32, i32, i32)> {
        if self.index < 1 {
            return None;
        }
        // Day and month default to 1.
        while self.index < 3 {
            self.comp[self.index] = 1;
            self.index += 1;
        }
        let c = self.comp;
        let (mut year, month, day);
        year = 0;
        match self.named_month {
            None => {
                if self.iso || (self.index == 3 && !Day::is_day(c[0])) {
                    year = c[0];
                    month = c[1];
                    day = c[2];
                } else {
                    month = c[0];
                    day = c[1];
                    if self.index == 3 {
                        year = c[2];
                    }
                }
            }
            Some(named) => {
                month = named;
                if self.index == 1 {
                    day = c[0];
                } else if !Day::is_day(c[0]) {
                    year = c[0];
                    day = c[1];
                } else {
                    day = c[0];
                    year = c[1];
                }
            }
        }
        if !self.iso {
            if between(year, 0, 49) {
                year += 2000;
            } else if between(year, 50, 99) {
                year += 1900;
            }
        }
        if !Day::is_month(month) || !Day::is_day(day) {
            return None;
        }
        Some((year, month - 1, day))
    }
}

#[derive(Default)]
struct Time {
    comp: [i32; 4],
    index: usize,
    hour_offset: Option<i32>,
}

impl Time {
    fn add(&mut self, n: i32) -> bool {
        if self.index < 4 {
            self.comp[self.index] = n;
            self.index += 1;
            return true;
        }
        false
    }

    fn add_final(&mut self, n: i32) -> bool {
        if !self.add(n) {
            return false;
        }
        while self.index < 4 {
            self.comp[self.index] = 0;
            self.index += 1;
        }
        true
    }

    fn is_empty(&self) -> bool {
        self.index == 0
    }

    fn is_minute(x: i32) -> bool {
        between(x, 0, 59)
    }

    fn is_hour(x: i32) -> bool {
        between(x, 0, 23)
    }

    fn is_expecting(&self, n: i32) -> bool {
        (self.index == 1 && Time::is_minute(n))
            || (self.index == 2 && Time::is_minute(n))
            || (self.index == 3 && between(n, 0, 999))
    }

    fn write(&mut self) -> Option<[i32; 4]> {
        while self.index < 4 {
            self.comp[self.index] = 0;
            self.index += 1;
        }
        let [mut hour, minute, second, ms] = self.comp;
        if let Some(offset) = self.hour_offset {
            if !between(hour, 0, 12) {
                return None;
            }
            hour %= 12;
            hour += offset;
        }
        let ok = Time::is_hour(hour)
            && Time::is_minute(minute)
            && Time::is_minute(second)
            && between(ms, 0, 999);
        if !ok && (hour != 24 || minute != 0 || second != 0 || ms != 0) {
            return None;
        }
        Some([hour, minute, second, ms])
    }
}

struct Zone {
    sign: i32,
    hour: i32,
    minute: i32,
}

impl Zone {
    fn new() -> Zone {
        Zone {
            sign: NONE,
            hour: NONE,
            minute: NONE,
        }
    }

    fn set(&mut self, offset_in_hours: i32) {
        self.sign = if offset_in_hours < 0 { -1 } else { 1 };
        self.hour = offset_in_hours * self.sign;
        self.minute = 0;
    }

    fn is_expecting(&self, n: i32) -> bool {
        self.hour != NONE && self.minute == NONE && Time::is_minute(n)
    }

    fn is_utc(&self) -> bool {
        self.hour == 0 && self.minute == 0
    }

    fn is_empty(&self) -> bool {
        self.hour == NONE
    }

    /// Seconds east of UTC, or `None` for local time.
    fn write(&mut self) -> Result<Option<i64>, ()> {
        if self.sign == NONE {
            return Ok(None);
        }
        if self.hour == NONE {
            self.hour = 0;
        }
        if self.minute == NONE {
            self.minute = 0;
        }
        let total = i64::from(self.hour) * 3600 + i64::from(self.minute) * 60;
        if total > i64::from(i32::MAX >> 1) {
            return Err(());
        }
        Ok(Some(if self.sign < 0 { -total } else { total }))
    }
}

/// The first three significant digits of a fraction, as milliseconds.
fn read_milliseconds(n: i32, len: usize) -> i32 {
    let mut len = len;
    let mut number = n;
    if len < 3 {
        number *= if len == 1 { 100 } else { 10 };
    } else if len > 3 {
        len = len.min(9);
        let mut factor = 1;
        while len > 3 {
            factor *= 10;
            len -= 1;
        }
        number /= factor;
    }
    number
}

/// ES5's `YYYY-MM-DDTHH:mm:ss.sssZ`. Returns the token to carry on with
/// in the legacy parser, or `Invalid`.
fn parse_es5(sc: &mut Scanner, day: &mut Day, time: &mut Time, zone: &mut Zone) -> Tok {
    if sc.peek().is_sign() {
        let sign_token = sc.next();
        if !sc.peek().is_fixed(6) {
            return sign_token;
        }
        let sign = if sign_token.is_sym('-') { -1 } else { 1 };
        let year = sc.next().number();
        if sign < 0 && year == 0 {
            return sign_token;
        }
        day.add(sign * year);
    } else if sc.peek().is_fixed(4) {
        day.add(sc.next().number());
    } else {
        return sc.next();
    }
    if sc.skip_symbol('-') {
        if !sc.peek().is_fixed(2) || !Day::is_month(sc.peek().number()) {
            return sc.next();
        }
        day.add(sc.next().number());
        if sc.skip_symbol('-') {
            if !sc.peek().is_fixed(2) || !Day::is_day(sc.peek().number()) {
                return sc.next();
            }
            day.add(sc.next().number());
        }
    }
    if !matches!(
        sc.peek(),
        Tok::Word {
            kw: Kw::TimeSep,
            ..
        }
    ) {
        if !sc.peek().is_end() {
            return sc.next();
        }
    } else {
        sc.next();
        if !sc.peek().is_fixed(2) || !between(sc.peek().number(), 0, 24) {
            return Tok::Invalid;
        }
        let hour_is_24 = sc.peek().number() == 24;
        time.add(sc.next().number());
        if !sc.skip_symbol(':') {
            return Tok::Invalid;
        }
        if !sc.peek().is_fixed(2)
            || !Time::is_minute(sc.peek().number())
            || (hour_is_24 && sc.peek().number() > 0)
        {
            return Tok::Invalid;
        }
        time.add(sc.next().number());
        if sc.skip_symbol(':') {
            if !sc.peek().is_fixed(2)
                || !Time::is_minute(sc.peek().number())
                || (hour_is_24 && sc.peek().number() > 0)
            {
                return Tok::Invalid;
            }
            time.add(sc.next().number());
            if sc.skip_symbol('.') {
                if !sc.peek().is_number() || (hour_is_24 && sc.peek().number() > 0) {
                    return Tok::Invalid;
                }
                let t = sc.next();
                let Tok::Num { n, len } = t else {
                    return Tok::Invalid;
                };
                time.add(read_milliseconds(n, len));
            }
        }
        if sc.peek().is_z() {
            sc.next();
            zone.set(0);
        } else if sc.peek().is_sym('+') || sc.peek().is_sym('-') {
            zone.sign = if sc.next().is_sym('+') { 1 } else { -1 };
            if sc.peek().is_fixed(4) {
                let hourmin = sc.next().number();
                let (hour, min) = (hourmin / 100, hourmin % 100);
                if !Time::is_hour(hour) || !Time::is_minute(min) {
                    return Tok::Invalid;
                }
                zone.hour = hour;
                zone.minute = min;
            } else {
                if !sc.peek().is_fixed(2) || !Time::is_hour(sc.peek().number()) {
                    return Tok::Invalid;
                }
                zone.hour = sc.next().number();
                if !sc.skip_symbol(':') {
                    return Tok::Invalid;
                }
                if !sc.peek().is_fixed(2) || !Time::is_minute(sc.peek().number()) {
                    return Tok::Invalid;
                }
                zone.minute = sc.next().number();
            }
        }
        if !sc.peek().is_end() {
            return Tok::Invalid;
        }
    }
    // A date with no time is UTC; a date with a time and no zone is local.
    if zone.is_empty() && time.is_empty() {
        zone.set(0);
    }
    day.iso = true;
    Tok::End
}

/// `MakeDay(year, month, day)` in days since the epoch.
fn make_day(year: i32, month: i32, day: i32) -> f64 {
    let mut y = i64::from(year);
    let mut m = i64::from(month);
    y += m.div_euclid(12);
    m = m.rem_euclid(12);
    (tz::days_from_civil(y, m + 1, 1) + i64::from(day) - 1) as f64
}

/// `new Date(text).getTime()`, in `tz` for a time with no zone. `None` is
/// an invalid date.
pub(crate) fn parse(text: &str, tz: Tz) -> Option<f64> {
    let mut sc = Scanner::new(text);
    let (mut day, mut time, mut zone) = (Day::default(), Time::default(), Zone::new());
    let first = parse_es5(&mut sc, &mut day, &mut time, &mut zone);
    if first.is_invalid() {
        return None;
    }
    let mut has_read_number = !day.is_empty();
    let mut token = first;
    while !token.is_end() {
        match token {
            Tok::Num { n, .. } => {
                has_read_number = true;
                if sc.skip_symbol(':') {
                    if sc.skip_symbol(':') {
                        if !time.is_empty() {
                            return None;
                        }
                        time.add(n);
                        time.add(0);
                    } else {
                        if !time.add(n) {
                            return None;
                        }
                        if sc.peek().is_sym('.') {
                            sc.next();
                        }
                    }
                } else if sc.skip_symbol('.') && time.is_expecting(n) {
                    time.add(n);
                    let Tok::Num { n: frac, len } = sc.peek() else {
                        return None;
                    };
                    sc.next();
                    let ms = read_milliseconds(frac, len);
                    time.add_final(ms);
                } else if zone.is_expecting(n) {
                    zone.minute = n;
                } else if time.is_expecting(n) {
                    time.add_final(n);
                    // Only the end, white space, `Z` or a sign may follow.
                    let peek = sc.peek();
                    if !peek.is_end()
                        && !matches!(peek, Tok::White)
                        && !peek.is_z()
                        && !peek.is_sign()
                    {
                        return None;
                    }
                } else {
                    if !day.add(n) {
                        return None;
                    }
                    sc.skip_symbol('-');
                }
            }
            Tok::Word { kw, value, .. } => {
                if kw == Kw::AmPm && !time.is_empty() {
                    time.hour_offset = Some(value);
                } else if kw == Kw::Month {
                    day.named_month = Some(value);
                    sc.skip_symbol('-');
                } else if kw == Kw::ZoneName && has_read_number {
                    zone.set(value);
                } else {
                    // Garbage words are illegal once a number has been read,
                    // and the first number has to be set apart from them.
                    if has_read_number {
                        return None;
                    }
                    if sc.peek().is_number() {
                        return None;
                    }
                }
            }
            Tok::Sym(sign @ ('+' | '-')) if zone.is_utc() || !time.is_empty() => {
                zone.sign = if sign == '-' { -1 } else { 1 };
                let (mut n, mut len) = (0, 0);
                if let Tok::Num { n: v, len: l } = sc.peek() {
                    sc.next();
                    n = v;
                    len = l;
                }
                has_read_number = true;
                if sc.peek().is_sym(':') {
                    zone.hour = n;
                    zone.minute = NONE;
                } else if len == 2 || len == 1 {
                    zone.hour = n;
                    zone.minute = 0;
                } else if len == 4 || len == 3 {
                    zone.hour = n / 100;
                    zone.minute = n % 100;
                } else {
                    return None;
                }
            }
            Tok::Sym('+' | '-' | ')') if has_read_number => return None,
            _ => {}
        }
        token = sc.next();
    }
    let (year, month, d) = day.write()?;
    let [hour, minute, second, ms] = time.write()?;
    let offset = zone.write().ok()?;
    let days = make_day(year, month, d);
    let in_day = f64::from(hour) * 3_600_000.0
        + f64::from(minute) * 60_000.0
        + f64::from(second) * 1000.0
        + f64::from(ms);
    let date = days * tz::MS_PER_DAY + in_day;
    let utc = match offset {
        None => {
            if date.abs() > MAX_TIME + tz::MS_PER_DAY {
                return None;
            }
            tz::local_to_utc(tz, date)
        }
        Some(seconds) => date - seconds as f64 * 1000.0,
    };
    (utc.is_finite() && utc.abs() <= MAX_TIME).then_some(utc)
}
