//! Time zones as JavaScript sees them: broken-down time, the offset, the
//! zone's id and its long name.
//!
//! `Tz::Utc` is exact. `Tz::Local` takes the offsets and wall-clock time
//! from the operating system (through `chrono::Local`, which honors `TZ`).
//! The two things the system cannot give are the zone's id (`%Z` prints
//! it) and its English long name (`Date.prototype.toString` prints it in
//! parentheses). Node gets those from ICU, so they come from a table
//! generated from Node: [`crate::zones`]. See [`long_name`] for what is
//! printed when the zone or the offset is not in it.

use chrono::{Local, Offset, TimeZone};

use crate::engine::Tz;
use crate::zones::{PROFILES, ZONE_IDS};

pub(crate) const MS_PER_DAY: f64 = 86_400_000.0;
/// The most JavaScript will hold: 8.64e15 ms either side of the epoch.
pub(crate) const MAX_TIME: f64 = 8.64e15;

/// Broken-down time in one zone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Fields {
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
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
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

/// The UTC offset of `tz` at the instant `ms`, in seconds east.
fn offset_seconds(tz: Tz, ms: f64) -> i64 {
    match tz {
        Tz::Utc => 0,
        Tz::Local => Local
            .timestamp_millis_opt(ms as i64)
            .single()
            .map_or(0, |d| i64::from(d.offset().fix().local_minus_utc())),
    }
}

/// Broken-down time of the instant `ms` in `tz`.
pub(crate) fn fields(tz: Tz, ms: f64) -> Option<Fields> {
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
pub(crate) fn local_to_utc(tz: Tz, local_ms: f64) -> f64 {
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

/// The zone the process runs in, as `TZ` or `/etc/localtime` names it.
fn local_zone_key() -> Option<&'static str> {
    use std::sync::OnceLock;
    static KEY: OnceLock<Option<String>> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Ok(tz) = std::env::var("TZ") {
            let tz = tz.strip_prefix(':').unwrap_or(&tz).to_string();
            return (!tz.is_empty()).then_some(tz);
        }
        let link = std::fs::read_link("/etc/localtime").ok()?;
        let link = link.to_string_lossy();
        link.split_once("zoneinfo/").map(|(_, z)| z.to_string())
    })
    .as_deref()
}

/// The id Node reports for the local zone (`resolvedOptions().timeZone`),
/// which is what `%Z` prints. `None` when the zone is not one it knows.
pub(crate) fn zone_id(tz: Tz) -> Option<&'static str> {
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
pub(crate) fn long_name(tz: Tz, offset_east: i64) -> String {
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
pub(crate) fn date_to_string(tz: Tz, ms: f64) -> String {
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
}
