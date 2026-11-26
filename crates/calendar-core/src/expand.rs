// SPDX-License-Identifier: Apache-2.0
//! Recurrence expansion in the event's own wall time (RFC 5545: a 09:00 weekly event in Berlin stays at 09:00 across
//! daylight saving changes; its UTC moves).

use crate::DAY;
use pim_ics::{Date, Freq, Rule, Tz, Zone};

/// A time an EXDATE, RECURRENCE-ID or UNTIL names, in the form it can be compared in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    /// a floating wall time (the event's own)
    Wall(i64),
    Utc(i64),
    /// a date (day number)
    Day(i64),
}

impl Key {
    /// Whether this names the occurrence at `wall` (`utc`); an all-day event compares days only.
    pub(crate) fn hits(self, wall: i64, utc: i64, all_day: bool) -> bool {
        if all_day {
            return self.day() == wall.div_euclid(DAY);
        }
        match self {
            Key::Wall(w) => w == wall,
            Key::Utc(u) => u == utc,
            Key::Day(d) => d == wall.div_euclid(DAY),
        }
    }

    pub(crate) fn day(self) -> i64 {
        match self {
            Key::Wall(w) | Key::Utc(w) => w.div_euclid(DAY),
            Key::Day(d) => d,
        }
    }

    /// The caller's local time it names.
    pub(crate) fn local(self, zone: &dyn Zone) -> i64 {
        match self {
            Key::Wall(w) => w,
            Key::Utc(u) => pim_ics::to_wall(u, zone),
            Key::Day(d) => d * DAY,
        }
    }

    /// The last moment UNTIL allows (a date: its whole day).
    fn until_passed(self, wall: i64, utc: i64) -> bool {
        match self {
            Key::Wall(w) => wall > w,
            Key::Utc(u) => utc > u,
            Key::Day(d) => wall.div_euclid(DAY) > d,
        }
    }
}

/// What expansion needs of an event.
pub(crate) struct Series<'a> {
    /// wall time of DTSTART
    pub start: i64,
    pub tz: Tz,
    pub rule: Option<&'a Rule>,
    pub until: Option<Key>,
}

/// Periods are counted from DTSTART; this many is far beyond any real calendar and stops a hostile rule.
const MAX_PERIODS: i64 = 400_000;

/// Calls `f(wall, utc)` for each occurrence (before EXDATEs) whose wall day is in `lo_day..=hi_day` or before it, in
/// order, until `f` returns false. Without COUNT the periods before `lo_day` are skipped.
pub(crate) fn walls(
    s: &Series,
    zone: &dyn Zone,
    lo_day: i64,
    hi_day: i64,
    f: &mut dyn FnMut(i64, i64) -> bool,
) {
    let utc = |w: i64| s.tz.utc(w, zone);
    let Some(r) = s.rule else {
        if s.start.div_euclid(DAY) <= hi_day {
            f(s.start, utc(s.start));
        }
        return;
    };
    let day0 = s.start.div_euclid(DAY);
    let sod = s.start.rem_euclid(DAY);
    let d0 = Date::from_days(day0);
    let iv = i64::from(r.interval.max(1));
    let month0 = i64::from(d0.year) * 12 + i64::from(d0.month) - 1;
    let week0 = day0 - d0.weekday() as i64;
    let days = if r.days == 0 {
        1u8 << d0.weekday()
    } else {
        r.days
    };
    let mut n = 0i64;
    if r.count.is_none() && lo_day > day0 {
        let to = Date::from_days(lo_day);
        let periods = match r.freq {
            Freq::Daily => (lo_day - day0) / iv,
            Freq::Weekly => (lo_day - week0) / (7 * iv),
            Freq::Monthly => (i64::from(to.year) * 12 + i64::from(to.month) - 1 - month0) / iv,
            Freq::Yearly => (i64::from(to.year) - i64::from(d0.year)) / iv,
        };
        n = (periods - 1).max(0);
    }
    let mut emitted = 0u32;
    while n < MAX_PERIODS {
        // the period's first day, and its occurrences' days (a short month or a non-leap year has none)
        let mut cand = [0i64; 7];
        let mut k = 0;
        let first = match r.freq {
            Freq::Daily => {
                let d = day0 + n * iv;
                cand[0] = d;
                k = 1;
                d
            }
            Freq::Weekly => {
                let w = week0 + n * 7 * iv;
                for wd in 0..7 {
                    if days & (1 << wd) != 0 {
                        cand[k] = w + wd;
                        k += 1;
                    }
                }
                w
            }
            Freq::Monthly | Freq::Yearly => {
                let m = if r.freq == Freq::Monthly {
                    month0 + n * iv
                } else {
                    month0 + n * iv * 12
                };
                let Ok(year) = i32::try_from(m.div_euclid(12)) else {
                    return;
                };
                if year > 9999 {
                    return;
                }
                let month = (m.rem_euclid(12) + 1) as u8;
                let first = Date {
                    year,
                    month,
                    day: 1,
                };
                if d0.day <= pim_ics::date::days_in(year, month) {
                    cand[0] = first.days() + i64::from(d0.day) - 1;
                    k = 1;
                }
                first.days()
            }
        };
        if first > hi_day {
            return;
        }
        for &day in &cand[..k] {
            if day < day0 {
                continue;
            }
            if day > hi_day {
                return;
            }
            let wall = day * DAY + sod;
            let u = utc(wall);
            if s.until.is_some_and(|until| until.until_passed(wall, u)) {
                return;
            }
            if r.count.is_some_and(|c| emitted >= c) {
                return;
            }
            emitted += 1;
            if !f(wall, u) {
                return;
            }
        }
        n += 1;
    }
}
