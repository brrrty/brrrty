//! Time zones as JavaScript sees them: broken-down time, the offset, the
//! zone's id and its long name.
//!
//! `Tz::Utc` is exact. `Tz::Local` is the zone `TZ` (or `/etc/localtime`)
//! names, with offsets from the tz database bundled into Brrrty: release
//! [`TZDATA_VERSION`], the one Node 26.2.0 ships (`process.versions.tz`).
//! The operating system's tz data is never read, so a machine with older
//! or newer data gives the same pages. A `TZ` that names no zone in that
//! data is refused ([`check_local`]).
//!
//! The zone's id (LiquidJS's `%Z` prints it) and its English long name
//! (`Date.prototype.toString` prints it in parentheses) come from a table
//! generated from Node (`src/zones.rs`, made by `tools/gen-zones.mjs`).
//! See [`long_name`] for what is printed when the zone or the offset is
//! not in it.

use std::sync::OnceLock;

use crate::zones::{PROFILES, ZONE_IDS};

/// The time zone dates are printed and formatted in. JavaScript uses the
/// process's local zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tz {
    /// The machine's local zone, which honors `TZ`. The zone's id and
    /// name are looked up once per process, so set `TZ` before the first
    /// date is printed.
    Local,
    Utc,
}

pub const MS_PER_DAY: f64 = 86_400_000.0;
/// The most JavaScript will hold: 8.64e15 ms either side of the epoch.
pub const MAX_TIME: f64 = 8.64e15;

/// Broken-down time in one zone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fields {
    pub year: i64,
    pub month0: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
    /// 0 is Sunday.
    pub weekday: u32,
    /// `getTimezoneOffset()`: minutes west of UTC.
    pub offset_west: f64,
}

/// Days from 1970-01-01 to the civil date. `month` is 1 to 12.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The tz database release bundled into Brrrty (`jiff-tzdb` 0.1.7): the
/// one Node 26.2.0 ships.
pub const TZDATA_VERSION: &str = "2026b";

/// A zone from the bundled tz database.
pub struct Zone {
    /// The name as the tz database spells it.
    pub name: &'static str,
    rules: ::tz::TimeZone,
}

impl Zone {
    /// The zone called `name` (any case), if the bundled data has it.
    pub fn named(name: &str) -> Option<&'static Zone> {
        use std::collections::HashMap;
        use std::sync::Mutex;
        static CACHE: OnceLock<Mutex<HashMap<&'static str, &'static Zone>>> = OnceLock::new();
        let (canonical, bytes) = jiff_tzdb::get(name)?;
        let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
        if let Some(z) = cache.get(canonical) {
            return Some(z);
        }
        let rules = ::tz::TimeZone::from_tz_data(bytes).ok()?;
        let zone: &'static Zone = Box::leak(Box::new(Zone {
            name: canonical,
            rules,
        }));
        cache.insert(canonical, zone);
        Some(zone)
    }

    /// The UTC offset at the instant `ms`, in seconds east.
    pub fn offset_seconds(&self, ms: f64) -> i64 {
        let secs = (ms / 1000.0).floor() as i64;
        self.rules
            .find_local_time_type(in_rule_range(secs))
            .map_or(0, |t| i64::from(t.ut_offset()))
    }
}

/// `secs` moved by whole 400-year cycles to within about 250,000 years of
/// 1970, where the rules can be evaluated. The calendar repeats every 400
/// years, and so do the zone's rules that far out (the last rule, or the
/// local mean time before the first), so the offset is the same;
/// JavaScript's range reaches 275,760.
fn in_rule_range(secs: i64) -> i64 {
    const CYCLE: i64 = 146_097 * 86_400;
    const LIMIT: i64 = 7_800_000_000_000;
    if secs.abs() <= LIMIT {
        return secs;
    }
    let cycles = (secs.abs() - LIMIT) / CYCLE + 1;
    secs - secs.signum() * cycles * CYCLE
}

/// The local zone as ICU names it (the key `%Z` and the long names are
/// looked up by; `None` for an empty `TZ`), and its rules.
struct Local {
    key: Option<String>,
    zone: &'static Zone,
}

/// The local zone, or why it can't be used. Read once per process.
fn local() -> Result<&'static Local, &'static str> {
    static LOCAL: OnceLock<Result<Local, String>> = OnceLock::new();
    LOCAL
        .get_or_init(|| {
            resolve_local(
                std::env::var("TZ").ok().as_deref(),
                std::path::Path::new(LOCALTIME),
                std::path::Path::new(ZONEINFO),
            )
        })
        .as_ref()
        .map_err(String::as_str)
}

fn local_zone() -> Result<&'static Zone, &'static str> {
    local().map(|l| l.zone)
}

/// Whether local time can be used: `Err` says why not, naming `TZ` or
/// `/etc/localtime`. Builds check this before they start, so a page is
/// never written with a zone guessed.
pub fn check_local() -> Result<(), String> {
    local_zone().map(|_| ()).map_err(str::to_string)
}

/// The name of the local zone as Node's ICU finds it, or why it can't be
/// told (for a probe or an error message).
pub fn local_zone_name() -> Result<&'static str, &'static str> {
    local_zone().map(|z| z.name)
}

/// ICU's `TZDEFAULT` and `TZZONEINFO` (`putil.cpp`, ICU 78.3), the same on
/// Linux and macOS.
const LOCALTIME: &str = "/etc/localtime";
const ZONEINFO: &str = "/usr/share/zoneinfo";

const SET_TZ: &str = "Set TZ to a zone name such as America/Chicago";

/// ICU's `skipZoneIDPrefix`: `posix/` and `right/` copy the top level.
fn skip_zone_id_prefix(id: &str) -> &str {
    id.strip_prefix("posix/")
        .or_else(|| id.strip_prefix("right/"))
        .unwrap_or(id)
}

/// The zone `TZ` names or, when it is unset, the one `/etc/localtime` is,
/// found as ICU 78.3's `uprv_tzname` finds it, with its rules from the
/// bundled data:
///
/// - `TZ` set: its value without a leading `:` and a `posix/` or `right/`
///   prefix. An empty `TZ` is UTC, as in Node. A name the bundled data
///   lacks is refused (Node would fall back to UTC or the system zone).
/// - `localtime` a symlink: the part of its real path after `/zoneinfo/`,
///   or of the link's own target when the real path has none or is
///   `posixrules` (macOS's real path is `/usr/share/zoneinfo.default/...`).
/// - `localtime` a regular file (a copied zone file, common in Docker
///   images): the first file under the zoneinfo folder with the same bytes,
///   searched in directory order as ICU does, skipping `posixrules` and
///   `localtime`.
/// - `localtime` missing: UTC.
fn resolve_local(
    tz: Option<&str>,
    localtime: &std::path::Path,
    zoneinfo: &std::path::Path,
) -> Result<Local, String> {
    let utc = |key: Option<String>| {
        Zone::named("UTC")
            .map(|zone| Local { key, zone })
            .ok_or_else(|| "the bundled tz data has no UTC".to_string())
    };
    if let Some(raw) = tz {
        let name = raw.strip_prefix(':').unwrap_or(raw);
        if name.is_empty() {
            return utc(None);
        }
        let name = match name.strip_prefix('/') {
            Some(_) => name.split_once("zoneinfo/").map_or("", |(_, z)| z),
            None => name,
        };
        let name = skip_zone_id_prefix(name);
        return Zone::named(name)
            .map(|zone| Local {
                key: Some(name.to_string()),
                zone,
            })
            .ok_or_else(|| {
                format!(
                    "TZ is `{raw}`, which isn't a time zone in Brrrty's tz data ({TZDATA_VERSION}, Node 26's). \
                     Node would quietly use UTC or the system zone instead. Set TZ to a zone name such as \
                     America/Chicago, or unset it"
                )
            });
    }
    let named = |name: &str, how: String| {
        Zone::named(name)
            .map(|zone| Local {
                key: Some(name.to_string()),
                zone,
            })
            .ok_or_else(|| {
                format!(
                    "{how} `{name}`, which isn't a time zone in Brrrty's tz data ({TZDATA_VERSION}). {}",
                    SET_TZ
                )
            })
    };
    let meta = match std::fs::symlink_metadata(localtime) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return utc(Some("UTC".into())),
        Err(e) => {
            return Err(format!(
                "can't read {}: {e}. {}",
                localtime.display(),
                SET_TZ
            ));
        }
    };
    if meta.file_type().is_symlink() {
        // ICU: `realpath`; if that fails (a broken link), the search below,
        // which can't read the file either, so ICU guesses from the C
        // library's abbreviation. Brrrty refuses.
        let Ok(real) = std::fs::canonicalize(localtime) else {
            return Err(format!(
                "{} is a broken link, so Brrrty can't tell its time zone (Node would guess from \
                 the C library's). {SET_TZ}",
                localtime.display()
            ));
        };
        let tail = |p: &str| p.split_once("/zoneinfo/").map(|(_, t)| t.to_string());
        let name = tail(&real.to_string_lossy())
            .filter(|t| t != "posixrules")
            .or_else(|| {
                std::fs::read_link(localtime)
                    .ok()
                    .and_then(|p| tail(&p.to_string_lossy()))
            });
        return match name {
            Some(name) => named(
                skip_zone_id_prefix(&name),
                format!("{} links to", localtime.display()),
            ),
            None => Err(format!(
                "{} is a link, but not into a zoneinfo folder, so Brrrty can't tell its time zone. {SET_TZ}",
                localtime.display()
            )),
        };
    }
    let wanted = std::fs::read(localtime)
        .map_err(|e| format!("can't read {}: {e}. {}", localtime.display(), SET_TZ))?;
    let mut dir = zoneinfo.to_string_lossy().into_owned();
    if !dir.ends_with('/') {
        dir.push('/');
    }
    match search_for_tz_file(&dir, dir.len(), &wanted) {
        Some(name) => named(&name, format!("{} is a copy of", localtime.display())),
        None => Err(format!(
            "{} is a file, and no zone in {} has the same bytes, so Brrrty can't tell its time zone \
             (Node would guess from its abbreviation). {}",
            localtime.display(),
            zoneinfo.display(),
            SET_TZ
        )),
    }
}

/// ICU's `searchForTZFile`: depth first in directory order, the first file
/// with the same bytes as `wanted`, named from `root_len` on, with a
/// `posix/` or `right/` prefix skipped. Paths are cut off at `PATH_MAX`,
/// where `opendir` fails.
fn search_for_tz_file(dir: &str, root_len: usize, wanted: &[u8]) -> Option<String> {
    const PATH_MAX: usize = 4096;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "posixrules" || name == "localtime" {
            continue;
        }
        let path = format!("{dir}{name}");
        if path.len() + 1 >= PATH_MAX {
            continue;
        }
        if std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
            if let Some(found) = search_for_tz_file(&format!("{path}/"), root_len, wanted) {
                return Some(found);
            }
        } else if std::fs::metadata(&path).is_ok_and(|m| m.len() == wanted.len() as u64)
            && std::fs::read(&path).is_ok_and(|b| b == wanted)
        {
            return Some(skip_zone_id_prefix(&path[root_len.min(path.len())..]).to_string());
        }
    }
    None
}

/// The UTC offset of `tz` at the instant `ms`, in seconds east.
fn offset_seconds(tz: Tz, ms: f64) -> i64 {
    match tz {
        Tz::Utc => 0,
        Tz::Local => local_zone().map_or(0, |z| z.offset_seconds(ms)),
    }
}

/// Broken-down time of the instant `ms` in `tz`.
pub fn fields(tz: Tz, ms: f64) -> Option<Fields> {
    if !ms.is_finite() || ms.abs() > MAX_TIME {
        return None;
    }
    let offset = offset_seconds(tz, ms);
    let local = ms as i64 + offset * 1000;
    let days = local.div_euclid(86_400_000);
    let in_day = local.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    Some(Fields {
        year,
        month0: month - 1,
        day,
        hour: (in_day / 3_600_000) as u32,
        minute: (in_day / 60_000 % 60) as u32,
        second: (in_day / 1000 % 60) as u32,
        millis: (in_day % 1000) as u32,
        weekday: (days + 4).rem_euclid(7) as u32,
        // V8 drops the seconds of a historical offset.
        offset_west: (-(offset as f64) / 60.0).trunc() + 0.0,
    })
}

/// The instant whose wall-clock time in `tz` is `local_ms` (read as if it
/// were UTC). For a time the clocks skip, the offset from before the
/// change is used, and for a time they repeat, the first one, as
/// JavaScript does.
pub fn local_to_utc(tz: Tz, local_ms: f64) -> f64 {
    if tz == Tz::Utc || !local_ms.is_finite() {
        return local_ms;
    }
    // The offsets either side of the day: a change of offset lies between
    // them, so one of them is the right one, or two are for a time that
    // repeats.
    let before = offset_seconds(tz, local_ms - MS_PER_DAY);
    let after = offset_seconds(tz, local_ms + MS_PER_DAY);
    let valid = |off: i64| offset_seconds(tz, local_ms - off as f64 * 1000.0) == off;
    let offset = match (valid(before), valid(after)) {
        (true, true) => before.max(after),
        (true, false) => before,
        (false, true) => after,
        // The clocks skip this time: the offset from before the change.
        (false, false) => before,
    };
    local_ms - offset as f64 * 1000.0
}

/// `new Date(year, month, day, hours, minutes, seconds, ms)` with two to
/// seven arguments, already numbers: a wall-clock time in `tz`, as
/// milliseconds since 1970, or `NaN`. Missing arguments are day 1 and 0.
/// Fields roll over as in JavaScript (month 12 is January of the next
/// year, day 0 the last day of the month before), fractions are dropped,
/// and a whole year from 0 to 99 means 1900 to 1999.
pub fn make_date(tz: Tz, args: &[f64]) -> f64 {
    let get = |i: usize, default: f64| args.get(i).copied().unwrap_or(default);
    let mut year = get(0, f64::NAN);
    if year.is_finite() {
        let y = year.trunc();
        if (0.0..=99.0).contains(&y) {
            year = 1900.0 + y;
        }
    }
    let day = make_day(year, get(1, 0.0), get(2, 1.0));
    let time = make_time(get(3, 0.0), get(4, 0.0), get(5, 0.0), get(6, 0.0));
    let local = day * MS_PER_DAY + time;
    // V8 gives up on a local time well outside the range before looking
    // up the zone.
    if !local.is_finite() || local.abs() > MAX_TIME + 10.0 * MS_PER_DAY {
        return f64::NAN;
    }
    time_clip(local_to_utc(tz, local))
}

/// The spec's `TimeClip`: `NaN` outside 8.64e15 ms, else whole ms.
pub fn time_clip(t: f64) -> f64 {
    if !t.is_finite() || t.abs() > MAX_TIME {
        f64::NAN
    } else {
        t.trunc() + 0.0
    }
}

/// The spec's `MakeDay`, with V8's limits on the year and month.
fn make_day(year: f64, month: f64, date: f64) -> f64 {
    if !year.is_finite() || !month.is_finite() || !date.is_finite() {
        return f64::NAN;
    }
    let (y, m, dt) = (year.trunc(), month.trunc(), date.trunc());
    if !(-1_000_000.0..=1_000_000.0).contains(&y) || !(-10_000_000.0..=10_000_000.0).contains(&m) {
        return f64::NAN;
    }
    let ym = y as i64 + (m as i64).div_euclid(12);
    let mn = (m as i64).rem_euclid(12);
    days_from_civil(ym, mn + 1, 1) as f64 + dt - 1.0
}

/// The spec's `MakeTime`.
fn make_time(h: f64, min: f64, s: f64, ms: f64) -> f64 {
    if !h.is_finite() || !min.is_finite() || !s.is_finite() || !ms.is_finite() {
        return f64::NAN;
    }
    h.trunc() * 3_600_000.0 + min.trunc() * 60_000.0 + s.trunc() * 1000.0 + ms.trunc()
}

/// `Date.prototype.toISOString()`, or `None` for an invalid date (where
/// JavaScript throws a `RangeError`, "Invalid time value"). A year outside
/// 0 to 9999 has a sign and six digits.
pub fn iso_string(ms: f64) -> Option<String> {
    let f = fields(Tz::Utc, ms)?;
    let year = if (0..=9999).contains(&f.year) {
        format!("{:04}", f.year)
    } else {
        format!(
            "{}{:06}",
            if f.year < 0 { '-' } else { '+' },
            f.year.unsigned_abs()
        )
    };
    Some(format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        f.month0 + 1,
        f.day,
        f.hour,
        f.minute,
        f.second,
        f.millis
    ))
}

/// The local zone's name as ICU found it, for the id and long names.
fn local_zone_key() -> Option<&'static str> {
    local().ok()?.key.as_deref()
}

/// Whether `name` is a zone in the tz database, ignoring case.
pub fn is_known_zone(name: &str) -> bool {
    ZONE_IDS
        .iter()
        .any(|(zone, _)| zone.eq_ignore_ascii_case(name))
}

/// The id Node reports for the local zone (`resolvedOptions().timeZone`),
/// which is what `%Z` prints. `None` when the zone is not one it knows.
pub fn zone_id(tz: Tz) -> Option<&'static str> {
    match tz {
        Tz::Utc => Some("UTC"),
        Tz::Local => {
            let key = local_zone_key()?;
            ZONE_IDS
                .binary_search_by_key(&key, |(zone, _)| *zone)
                .ok()
                .map(|i| ZONE_IDS[i].1)
        }
    }
}

/// The long name `Date.prototype.toString` prints in parentheses for a
/// time at `offset_east` minutes east of UTC.
///
/// Node's names come from ICU. For the local zone this looks the zone's id
/// up in a table generated from Node and finds the name for the offset,
/// which tells standard time from daylight time. An offset the zone has
/// used more than once under different names gets the most recent name
/// (Moscow's +4 was "Moscow Summer Time" before 2011 and "Moscow Standard
/// Time" after it). A zone or offset that is not in the table, such as a
/// custom `TZ` like `CET-1CEST`, gets ICU's fallback form: `GMT+05:30`.
pub fn long_name(tz: Tz, offset_east: i64) -> String {
    let offset_min = (offset_east / 60) as i16;
    let known = match tz {
        Tz::Utc => Some("Coordinated Universal Time"),
        Tz::Local => zone_id(tz).and_then(|id| {
            let profile = PROFILES
                .binary_search_by_key(&id, |(p, _)| *p)
                .ok()
                .map(|i| PROFILES[i].1)?;
            profile
                .iter()
                .find(|(o, _)| *o == offset_min)
                .map(|(_, name)| *name)
        }),
    };
    match known {
        Some(name) => name.to_string(),
        None => gmt_offset(offset_east),
    }
}

/// `GMT+05:30`.
fn gmt_offset(offset_east: i64) -> String {
    let sign = if offset_east < 0 { '-' } else { '+' };
    let abs = offset_east.abs();
    format!("GMT{sign}{:02}:{:02}", abs / 3600, abs / 60 % 60)
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `Date.prototype.toString()` of the instant `ms`:
/// `Fri Nov 15 2026 00:00:00 GMT+0000 (Coordinated Universal Time)`.
pub fn date_to_string(tz: Tz, ms: f64) -> String {
    let Some(f) = fields(tz, ms) else {
        return "Invalid Date".into();
    };
    let offset_east = (-f.offset_west * 60.0) as i64;
    let sign = if offset_east < 0 { '-' } else { '+' };
    let abs = offset_east.abs();
    let year = if f.year < 0 {
        format!("-{:06}", -f.year)
    } else {
        format!("{:04}", f.year)
    };
    format!(
        "{} {} {:02} {year} {:02}:{:02}:{:02} GMT{sign}{:02}{:02} ({})",
        DAYS[f.weekday as usize],
        MONTHS[f.month0 as usize],
        f.day,
        f.hour,
        f.minute,
        f.second,
        abs / 3600,
        abs / 60 % 60,
        long_name(tz, offset_east),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_prints_like_node() {
        // node: new Date(Date.UTC(2026, 10, 15)).toString() with TZ=UTC
        let ms = 1_794_700_800_000.0;
        assert_eq!(
            date_to_string(Tz::Utc, ms),
            "Sun Nov 15 2026 00:00:00 GMT+0000 (Coordinated Universal Time)"
        );
        // Years outside 0000 to 9999: node prints -000001 and 275760.
        assert_eq!(
            date_to_string(Tz::Utc, -62_198_755_200_000.0),
            "Fri Jan 01 -000001 00:00:00 GMT+0000 (Coordinated Universal Time)"
        );
        assert_eq!(
            date_to_string(Tz::Utc, 8.64e15),
            "Sat Sep 13 275760 00:00:00 GMT+0000 (Coordinated Universal Time)"
        );
    }

    #[test]
    fn civil_dates_round_trip() {
        for days in [-1_000_000, -719_468, -1, 0, 1, 19_000, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, i64::from(m), i64::from(d)), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_787), (2024, 3, 5));
    }

    #[test]
    fn the_generated_tables_can_be_searched() {
        assert!(
            ZONE_IDS.windows(2).all(|w| w[0].0 < w[1].0),
            "ZONE_IDS sorted"
        );
        assert!(
            PROFILES.windows(2).all(|w| w[0].0 < w[1].0),
            "PROFILES sorted"
        );
        for (zone, id) in ZONE_IDS {
            assert!(
                PROFILES.binary_search_by_key(id, |(p, _)| *p).is_ok(),
                "{zone} has no profile for {id}"
            );
        }
        let profile =
            |id: &str| PROFILES[PROFILES.binary_search_by_key(&id, |(p, _)| *p).unwrap()].1;
        // node: TZ=America/New_York new Date(2026, 0, 15).toString() and July.
        assert!(profile("America/New_York").contains(&(-300, "Eastern Standard Time")));
        assert!(profile("America/New_York").contains(&(-240, "Eastern Daylight Time")));
        assert!(profile("Europe/London").contains(&(60, "British Summer Time")));
        assert!(profile("Asia/Calcutta").contains(&(330, "India Standard Time")));
        // Aliases resolve to the id Node reports.
        let id =
            |zone: &str| ZONE_IDS[ZONE_IDS.binary_search_by_key(&zone, |(z, _)| *z).unwrap()].1;
        assert_eq!(id("US/Eastern"), "America/New_York");
        assert_eq!(id("Asia/Kolkata"), "Asia/Calcutta");
        assert_eq!(id("Etc/UTC"), "UTC");
    }

    #[test]
    fn an_offset_missing_from_the_table_falls_back_to_gmt_form() {
        assert_eq!(gmt_offset(19_800), "GMT+05:30");
        assert_eq!(gmt_offset(-3_600), "GMT-01:00");
        assert_eq!(gmt_offset(0), "GMT+00:00");
    }

    fn name(
        tz: Option<&str>,
        lt: &std::path::Path,
        zi: &std::path::Path,
    ) -> Result<String, String> {
        resolve_local(tz, lt, zi).map(|l| format!("{} {:?}", l.zone.name, l.key))
    }

    /// ICU 78.3's `uprv_tzname`, case by case.
    #[test]
    fn the_local_zone_is_found_as_icu_finds_it() {
        let dir = tempfile::tempdir().unwrap();
        let zi = dir.path().join("zoneinfo");
        std::fs::create_dir_all(zi.join("Europe")).unwrap();
        std::fs::create_dir_all(zi.join("posix/Asia")).unwrap();
        let berlin = jiff_tzdb::get("Europe/Berlin").unwrap().1;
        let tokyo = jiff_tzdb::get("Asia/Tokyo").unwrap().1;
        std::fs::write(zi.join("Europe/Berlin"), berlin).unwrap();
        std::fs::write(zi.join("posix/Asia/Tokyo"), tokyo).unwrap();
        // Skipped by name, as ICU skips them.
        std::fs::write(zi.join("posixrules"), tokyo).unwrap();
        std::fs::write(zi.join("localtime"), berlin).unwrap();

        // Missing: UTC.
        let missing = dir.path().join("missing");
        assert_eq!(name(None, &missing, &zi).unwrap(), "UTC Some(\"UTC\")");
        // A copy: found by its bytes; a `posix/` prefix is skipped.
        let copy = dir.path().join("copy");
        std::fs::write(&copy, berlin).unwrap();
        assert_eq!(
            name(None, &copy, &zi).unwrap(),
            "Europe/Berlin Some(\"Europe/Berlin\")"
        );
        std::fs::write(&copy, tokyo).unwrap();
        assert_eq!(
            name(None, &copy, &zi).unwrap(),
            "Asia/Tokyo Some(\"Asia/Tokyo\")"
        );
        // A copy of nothing in the folder, or with no folder: refused.
        std::fs::write(&copy, b"TZif not a zone").unwrap();
        assert!(name(None, &copy, &zi).unwrap_err().contains("same bytes"));
        std::fs::write(&copy, berlin).unwrap();
        assert!(
            name(None, &copy, &dir.path().join("none"))
                .unwrap_err()
                .contains("Set TZ")
        );
        // A link: the part of its real path after /zoneinfo/, without posix/.
        std::fs::create_dir_all(zi.join("posix/Europe")).unwrap();
        std::fs::write(zi.join("posix/Europe/Paris"), b"x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(zi.join("posix/Europe/Paris"), &link).unwrap();
        assert_eq!(
            name(None, &link, &zi).unwrap(),
            "Europe/Paris Some(\"Europe/Paris\")"
        );
        // A broken link: ICU can't read it either, and guesses; refused.
        let broken = dir.path().join("broken");
        std::os::unix::fs::symlink("/nowhere/zoneinfo/posix/Asia/Tokyo", &broken).unwrap();
        assert!(
            name(None, &broken, &zi)
                .unwrap_err()
                .contains("broken link")
        );
        let real = dir.path().join("real");
        std::os::unix::fs::symlink(zi.join("Europe/Berlin"), &real).unwrap();
        assert_eq!(
            name(None, &real, &zi).unwrap(),
            "Europe/Berlin Some(\"Europe/Berlin\")"
        );
        let odd = dir.path().join("odd");
        std::os::unix::fs::symlink(&copy, &odd).unwrap();
        assert!(
            name(None, &odd, &zi)
                .unwrap_err()
                .contains("not into a zoneinfo folder")
        );

        // TZ wins, without its colon or a posix/ prefix; empty is UTC with no id.
        assert_eq!(
            name(Some(":Asia/Tokyo"), &link, &zi).unwrap(),
            "Asia/Tokyo Some(\"Asia/Tokyo\")"
        );
        assert_eq!(
            name(Some("posix/Asia/Tokyo"), &link, &zi).unwrap(),
            "Asia/Tokyo Some(\"Asia/Tokyo\")"
        );
        assert_eq!(
            name(Some("asia/tokyo"), &link, &zi).unwrap(),
            "Asia/Tokyo Some(\"asia/tokyo\")"
        );
        assert_eq!(
            name(Some("US/Pacific"), &link, &zi).unwrap(),
            "US/Pacific Some(\"US/Pacific\")"
        );
        assert_eq!(name(Some(""), &link, &zi).unwrap(), "UTC None");
        assert_eq!(
            name(Some("/usr/share/zoneinfo/Europe/Paris"), &missing, &zi).unwrap(),
            "Europe/Paris Some(\"Europe/Paris\")"
        );
        // Typos, stray spaces and POSIX rules: Node would quietly use UTC
        // or the system zone, so Brrrty refuses, naming TZ.
        for bad in [
            "Foo/Bar",
            "America/Chigaco",
            "America/Chicago ",
            "EST5EDT,M3.2.0,M11.1.0",
            "CET-1CEST,M3.5.0,M10.5.0/3",
            "<+0330>-3:30",
            "JST-9",
            "/etc/localtime",
        ] {
            let why = name(Some(bad), &missing, &zi).unwrap_err();
            assert!(why.contains(&format!("TZ is `{bad}`")), "{why}");
        }
    }

    #[test]
    fn the_bundled_tz_data_is_the_release_named() {
        assert_eq!(jiff_tzdb::VERSION, Some(TZDATA_VERSION));
    }

    #[test]
    fn offsets_are_node_26_s_not_the_system_s() {
        // tz 2026c makes Alberta UTC-6 all year from 2026-11-01; Node 26.2.0
        // ships 2026b, where 2026-11-17 is still UTC-7.
        let edmonton = Zone::named("America/Edmonton").unwrap();
        assert_eq!(edmonton.offset_seconds(1_794_916_800_000.0), -7 * 3600);
    }

    #[test]
    fn known_zones_ignore_case() {
        assert!(is_known_zone("america/new_york"));
        assert!(is_known_zone("Europe/Paris"));
        assert!(!is_known_zone("Mars/Olympus"));
    }
}
