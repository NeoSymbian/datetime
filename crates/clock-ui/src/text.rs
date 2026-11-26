// SPDX-License-Identifier: GPL-3.0-only
//! How Clock words its times (the boards' wording): "rings in 4 h 06 min", "of 5:00 · ends 10:27", the digits the
//! timer and the stopwatch show, the snooze choice.

use alloc::format;
use alloc::string::String;
use datetime_core::alarm::{DAY_NAMES, format_hm};

const DAY: i64 = 86_400;

/// "4 h 06 min", "45 min", "less than a minute".
#[must_use]
pub fn span(minutes: i64) -> String {
    match minutes {
        ..1 => "less than a minute".into(),
        1..60 => format!("{minutes} min"),
        _ => format!("{} h {:02} min", minutes / 60, minutes % 60),
    }
}

/// The local day of `at` against `now` (both Unix UTC seconds, `offset` east of UTC): "today", "tomorrow", a weekday
/// within the week, else "in N days".
#[must_use]
pub fn day(at: i64, now: i64, offset: i32) -> String {
    let local = |t: i64| (t + i64::from(offset)).div_euclid(DAY);
    let d = local(at);
    match d - local(now) {
        ..1 => "today".into(),
        1 => "tomorrow".into(),
        // 1970-01-01 was a Thursday; DAY_NAMES starts on Monday
        2..7 => DAY_NAMES[(d + 3).rem_euclid(7) as usize].into(),
        n => format!("in {n} days"),
    }
}

/// The minutes from `now` until `at` (rounded up: an alarm at 10:30 seen at 10:29:10 rings in 1 min).
#[must_use]
pub fn minutes_until(at: i64, now: i64) -> i64 {
    (at - now + 59).div_euclid(60)
}

/// The editor's line: "Rings today, in 45 min", "Rings tomorrow, in 20 h 21 min", "Rings Thu, in 3 days".
#[must_use]
pub fn rings(at: i64, now: i64, offset: i32) -> String {
    let m = minutes_until(at, now);
    if m >= 48 * 60 {
        return format!("Rings {}, in {} days", day(at, now, offset), m / (24 * 60));
    }
    format!("Rings {}, in {}", day(at, now, offset), span(m))
}

/// A countdown or a stopwatch as the two numbers its digits show: (min, sec) under an hour, (h, min) from one on.
#[must_use]
pub fn digits(ms: u64) -> (i32, i32) {
    let s = ms / 1000;
    // both under 100 for a day's time
    if s >= 3600 {
        ((s / 3600) as i32, (s / 60 % 60) as i32)
    } else {
        ((s / 60) as i32, (s % 60) as i32)
    }
}

/// The stopwatch's small part after the digits: ".37" hundredths under an hour, ":05" seconds from one on.
#[must_use]
pub fn small(ms: u64) -> String {
    if ms >= 3_600_000 {
        format!(":{:02}", ms / 1000 % 60)
    } else {
        format!(".{:02}", ms / 10 % 100)
    }
}

/// "of 5:00 · ends 10:27" while running, "of 5:00 · paused".
#[must_use]
pub fn timer_line(set: &str, ends: Option<(u8, u8)>, h24: bool) -> String {
    match ends {
        Some((h, m)) => format!("of {set} · ends {}", format_hm(h, m, h24)),
        None => format!("of {set} · paused"),
    }
}

/// The snooze choice: "5 min · up to 3 times", "10 min · once", "Off".
#[must_use]
pub fn snooze(minutes: i32, count: i32) -> String {
    match count {
        ..1 => "Off".into(),
        1 => format!("{minutes} min · once"),
        n => format!("{minutes} min · up to {n} times"),
    }
}

/// A zone's offset: "UTC+4", "UTC−3:30", "UTC+0".
#[must_use]
pub fn utc(offset: i32) -> String {
    let m = offset.unsigned_abs() / 60;
    let sign = if offset < 0 { '−' } else { '+' };
    match m % 60 {
        0 => format!("UTC{sign}{}", m / 60),
        r => format!("UTC{sign}{}:{r:02}", m / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monday 16 November 2026 10:24 UTC
    const NOW: i64 = 1_794_824_640;

    #[test]
    fn spans_read_like_the_boards() {
        assert_eq!(span(246), "4 h 06 min");
        assert_eq!(span(45), "45 min");
        assert_eq!(span(0), "less than a minute");
        assert_eq!(minutes_until(NOW + 70, NOW), 2);
        assert_eq!(minutes_until(NOW + 60, NOW), 1);
    }

    #[test]
    fn days_count_in_local_time() {
        assert_eq!(day(NOW + 3600, NOW, 0), "today");
        assert_eq!(day(NOW + 20 * 3600, NOW, 0), "tomorrow");
        // Monday + 3 = Thursday
        assert_eq!(day(NOW + 3 * DAY, NOW, 0), "Thu");
        assert_eq!(day(NOW + 9 * DAY, NOW, 0), "in 9 days");
        // 10:24 UTC is 23:24 in UTC+13: an hour later is tomorrow there
        assert_eq!(day(NOW + 3600, NOW, 13 * 3600), "tomorrow");
        assert_eq!(
            rings(NOW + 20 * 3600 + 21 * 60, NOW, 0),
            "Rings tomorrow, in 20 h 21 min"
        );
        assert_eq!(rings(NOW + 3 * DAY, NOW, 0), "Rings Thu, in 3 days");
    }

    #[test]
    fn digits_switch_to_hours_past_an_hour() {
        assert_eq!(digits(186_000), (3, 6));
        assert_eq!(digits(3_723_000), (1, 2));
        assert_eq!(small(161_370), ".37");
        assert_eq!(small(3_725_000), ":05");
    }

    #[test]
    fn lines_and_choices() {
        assert_eq!(
            timer_line("5:00", Some((10, 27)), true),
            "of 5:00 · ends 10:27"
        );
        assert_eq!(timer_line("5:00", None, true), "of 5:00 · paused");
        assert_eq!(snooze(5, 3), "5 min · up to 3 times");
        assert_eq!(snooze(10, 1), "10 min · once");
        assert_eq!(snooze(5, 0), "Off");
        assert_eq!(utc(4 * 3600), "UTC+4");
        assert_eq!(utc(-(3 * 3600 + 1800)), "UTC−3:30");
    }
}
