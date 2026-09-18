//! Centralized date/time display formatting (plan §7: "centralize display
//! formatting"). Rules:
//! - today → `HH:MM`
//! - same year → `Sep 1`
//! - otherwise → `2025`
//!
//! Reader meta line (absolute): `Mon, Sep 1 · 18:32` (mockup `.mm-date`).

use chrono::{DateTime, Datelike, FixedOffset};

pub fn format_clock(now: DateTime<FixedOffset>) -> String {
    now.format("%a %b %-d · %H:%M").to_string()
}

/// Absolute header date for the reader meta line.
pub fn format_absolute(timestamp: DateTime<FixedOffset>) -> String {
    timestamp.format("%a, %b %-d · %H:%M").to_string()
}

pub fn format_relative(now: DateTime<FixedOffset>, timestamp: DateTime<FixedOffset>) -> String {
    if timestamp > now {
        // Clock skew or future-dated mail: show the time of day.
        return timestamp.format("%H:%M").to_string();
    }
    let today = now.date_naive();
    let day = timestamp.date_naive();
    if day == today {
        timestamp.format("%H:%M").to_string()
    } else if day.year() == today.year() {
        timestamp.format("%b %-d").to_string()
    } else {
        timestamp.format("%Y").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<FixedOffset> {
        crate::domain::time::TZ_PLUS_3
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .unwrap()
    }

    fn now() -> DateTime<FixedOffset> {
        at(2026, 9, 2, 10, 47)
    }

    #[test]
    fn clock_format() {
        assert_eq!(format_clock(now()), "Wed Sep 2 · 10:47");
    }

    #[test]
    fn relative_buckets() {
        let n = now();
        assert_eq!(format_relative(n, at(2026, 9, 2, 10, 42)), "10:42");
        assert_eq!(format_relative(n, at(2026, 9, 1, 18, 3)), "Sep 1");
        assert_eq!(format_relative(n, at(2026, 8, 28, 9, 0)), "Aug 28");
        assert_eq!(format_relative(n, at(2025, 12, 30, 9, 0)), "2025");
    }

    #[test]
    fn future_timestamp_is_time_only() {
        let n = now();
        assert_eq!(format_relative(n, at(2026, 9, 2, 23, 0)), "23:00");
    }
}
