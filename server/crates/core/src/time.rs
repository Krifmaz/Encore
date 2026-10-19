//! Time without a date crate: the unix clock, and civil dates in UTC from and to day counts since
//! 1970-01-01 (Howard Hinnant's algorithms).
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since 1970-01-01 UTC (0 when the clock is set before it).
pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// (year, month 1–12, day 1–31) of a day count since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

/// Days since 1970-01-01 of a civil date (month 1–12).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// (year, month 1–12, day) of unix seconds, in UTC.
pub fn civil(secs: f64) -> (i64, i64, i64) {
    civil_from_days((secs / 86_400.0).floor() as i64)
}

/// `YYYY-MM-DD` of unix seconds, in UTC (a time before 1970 counts as 1970-01-01).
pub fn ymd(secs: f64) -> String {
    let (y, m, d) = civil(secs.max(0.0));
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(ymd(0.0), "1970-01-01");
        assert_eq!(ymd(-5.0), "1970-01-01");
        assert_eq!(ymd(1_700_000_000.0), "2023-11-14");
        assert_eq!(civil(951_782_400.0), (2000, 2, 29));
        // there and back, across leap days, the turn of a century and dates before 1970
        for days in [-800_000, -1, 0, 59, 10_956, 11_016, 20_000, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert!((1..=12).contains(&m) && (1..=31).contains(&d), "{days}");
            assert_eq!(days_from_civil(y, m, d), days, "{days}");
        }
        assert_eq!(days_from_civil(2026, 10, 19), 20_745);
        assert!(now() > 1_700_000_000.0);
    }
}
