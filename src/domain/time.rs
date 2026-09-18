//! Shared timestamp construction (ticket 7ayf): one writer per offset and
//! per fixture epoch, so the "this offset is valid" expects and the mock
//! clock base cannot drift apart between the backend mapper, the domain,
//! and the test fixtures.
//!
//! The app carries `DateTime<FixedOffset>` end to end (backend date
//! mapping, epoch fallbacks, fixtures), so UTC is represented as the
//! offset-zero `FixedOffset` — not `chrono::Utc`, which would not unify
//! with the rest of the pipeline.

use chrono::{DateTime, FixedOffset};

/// The UTC fixed offset (offset-zero [`FixedOffset`], not `chrono::Utc`).
pub const UTC: FixedOffset = match FixedOffset::east_opt(0) {
    Some(tz) => tz,
    None => unreachable!(),
};

/// The mock-fixture zone (+03:00): `app::mock` timestamps are authored
/// against it and the date-format tests assert in it, so fixture times and
/// rendering expectations can never disagree.
pub const TZ_PLUS_3: FixedOffset = match FixedOffset::east_opt(3 * 3600) {
    Some(tz) => tz,
    None => unreachable!(),
};

/// Unix seconds of the shared fixture "now" (`2026-09-02 10:47:00 +03:00`,
/// see `app::mock::now`); the draft tests offset from the same base.
pub const MOCK_NOW_SECS: i64 = 1_788_335_220;

/// Unix `secs` as a UTC-anchored timestamp (`None` when out of range).
pub fn from_unix(secs: i64) -> Option<DateTime<FixedOffset>> {
    chrono::DateTime::from_timestamp(secs, 0).map(|dt| dt.with_timezone(&UTC))
}

/// The Unix epoch as a UTC-anchored timestamp: the stable fallback for a
/// missing or unparseable Date header (ADR 0001 finding 2) — it renders as
/// an explicit unknown date, never as "now".
pub fn epoch() -> DateTime<FixedOffset> {
    from_unix(0).expect("epoch is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offsets are compile-time-validated consts, so the unreachable
    /// branches can never fire; the values are pinned so a typo in one
    /// cannot silently shift every fixture.
    #[test]
    fn offsets_and_epoch_are_pinned() {
        assert_eq!(UTC.local_minus_utc(), 0);
        assert_eq!(TZ_PLUS_3.local_minus_utc(), 3 * 3600);
        assert_eq!(epoch().to_string(), "1970-01-01 00:00:00 +00:00");
        assert_eq!(
            from_unix(MOCK_NOW_SECS).expect("fixture epoch").to_string(),
            "2026-09-02 07:47:00 +00:00"
        );
    }

    /// Out-of-range Unix seconds stay `None` instead of panicking.
    #[test]
    fn from_unix_rejects_out_of_range() {
        assert!(from_unix(i64::MAX).is_none());
        assert!(from_unix(i64::MIN).is_none());
    }
}
