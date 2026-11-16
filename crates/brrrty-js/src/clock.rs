//! The current time, which `SOURCE_DATE_EPOCH` can freeze.
//!
//! Reproducible builds set `SOURCE_DATE_EPOCH` to a number of whole seconds
//! since 1970. When it is set, everything Brrrty would read the clock for
//! (Liquid's `"now"`, Rhai's `now()`) uses that instant instead.

use crate::tz::MAX_TIME;

/// The variable's name.
pub const SOURCE_DATE_EPOCH: &str = "SOURCE_DATE_EPOCH";

/// Read `SOURCE_DATE_EPOCH` as milliseconds. `Ok(None)` when it is unset;
/// an error message when it is not whole seconds in JavaScript's date range.
pub fn source_date_epoch() -> Result<Option<i64>, String> {
    match std::env::var(SOURCE_DATE_EPOCH) {
        Ok(v) => parse_epoch(&v).map(Some),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!(
            "{SOURCE_DATE_EPOCH} must be whole seconds, like 1700000000"
        )),
    }
}

/// `text` as whole seconds since 1970, in milliseconds.
pub fn parse_epoch(text: &str) -> Result<i64, String> {
    let bad = || {
        format!(
            "{SOURCE_DATE_EPOCH} must be whole seconds since 1970, like 1700000000, not `{text}`"
        )
    };
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let secs: i64 = text.parse().map_err(|_| bad())?;
    match secs.checked_mul(1000) {
        Some(ms) if (ms as f64) <= MAX_TIME => Ok(ms),
        _ => Err(bad()),
    }
}

/// Milliseconds since 1970: `SOURCE_DATE_EPOCH` when it is valid, else the
/// system clock. Callers that must not ignore a bad value check
/// [`source_date_epoch`] first; the build does.
pub fn now_ms() -> f64 {
    if let Ok(Some(ms)) = source_date_epoch() {
        return ms as f64;
    }
    system_now_ms()
}

/// The system clock, in milliseconds since 1970.
pub fn system_now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_millis() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whole_seconds_only() {
        assert_eq!(parse_epoch("0"), Ok(0));
        assert_eq!(parse_epoch("1700000000"), Ok(1_700_000_000_000));
        assert_eq!(parse_epoch("8640000000000"), Ok(8_640_000_000_000_000));
        for bad in [
            "",
            " 1",
            "1.5",
            "-1",
            "+1",
            "1e9",
            "8640000000001",
            "99999999999999999999",
        ] {
            assert!(parse_epoch(bad).is_err(), "{bad}");
        }
    }
}
