//! RFC 3339 UTC rendering without a calendar dependency.
//!
//! The contract says timestamps are RFC 3339 UTC. `SystemTime`'s serde form
//! is a seconds/nanos object, which would leak a non-contract shape to the
//! UI, so responses carry a string produced here.
//!
//! The conversion is pure (no clock read): the timestamp is always an
//! input fact (`ScanRecord::completed_at`). A time before 1970 or beyond
//! year 9999 is not representable as a four-digit RFC 3339 year and yields
//! `None` — an honest unknown, never a clamped fake date.

use std::time::{SystemTime, UNIX_EPOCH};

/// Render `time` as `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Returns `None` for pre-epoch values and for years outside `0000..=9999`;
/// the caller publishes the unknown rather than inventing a timestamp.
pub(crate) fn rfc3339_utc(time: SystemTime) -> Option<String> {
    // 9999-12-31T23:59:59Z; beyond it a four-digit RFC 3339 year is not
    // representable, so the timestamp is honestly unknown.
    const MAX_SECONDS: u64 = 253_402_300_799;
    let seconds = time.duration_since(UNIX_EPOCH).ok()?.as_secs();
    if seconds > MAX_SECONDS {
        return None;
    }
    let days_since_epoch = (seconds / 86_400) as i64;
    let second_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days_since_epoch);
    if !(0..=9999).contains(&year) {
        return None;
    }
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let second = second_of_day % 60;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

/// Convert days since 1970-01-01 to a proleptic-Gregorian civil date.
///
/// Howard Hinnant's `civil_from_days` algorithm; all values are `i64` and
/// every division is exact. Kept private so the wire format has exactly one
/// implementation.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (year + i64::from(month <= 2), month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn epoch_is_1970() {
        assert_eq!(
            rfc3339_utc(UNIX_EPOCH).as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
    }

    #[test]
    fn known_instant_is_rendered_in_utc() {
        assert_eq!(
            rfc3339_utc(UNIX_EPOCH + Duration::from_secs(1_700_000_000)).as_deref(),
            Some("2023-11-14T22:13:20Z")
        );
    }

    #[test]
    fn leap_day_is_rendered() {
        assert_eq!(
            rfc3339_utc(UNIX_EPOCH + Duration::from_secs(1_709_164_800)).as_deref(),
            Some("2024-02-29T00:00:00Z")
        );
    }

    #[test]
    fn pre_epoch_is_unknown_not_a_fake_date() {
        let before = UNIX_EPOCH
            .checked_sub(Duration::from_secs(1))
            .unwrap_or(UNIX_EPOCH);
        assert_eq!(rfc3339_utc(before), None);
    }
}
