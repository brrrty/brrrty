//! The local zone comes from the operating system, not from a bundled table, so rules past 2099
//! keep working. Node (TZ=America/New_York): `new Date(4120000000000).toString()` is
//! `Fri Jul 23 2100 00:26:40 GMT-0400 (Eastern Daylight Time)`.

use brrrty_js::{Tz, tz};

#[test]
fn local_dst_after_2099_matches_node() {
    if std::env::var("BRRRTY_FAR_FUTURE_CHILD").is_ok() {
        assert_eq!(
            tz::date_to_string(Tz::Local, 4_120_000_000_000.0),
            "Fri Jul 23 2100 00:26:40 GMT-0400 (Eastern Daylight Time)"
        );
        return;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "local_dst_after_2099_matches_node",
            "--nocapture",
        ])
        .env("TZ", "America/New_York")
        .env("BRRRTY_FAR_FUTURE_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}
