// SPDX-License-Identifier: Apache-2.0
//! Time zones: the caller's [`Zone`] (the phone's), and an iCalendar time's [`Tz`] (UTC, floating, or a TZID's rule).
//!
//! "Wall" seconds are seconds since 1970 of a wall-clock date and time read as if it were UTC; "local" is the wall
//! time of the caller's zone.

use super::Line;
use alloc::format;
use alloc::string::{String, ToString};
use neo_format_pim::date::Date;
use neo_format_pim::ical::When;
use neoui_proto::clock::{tz, world};

/// The caller's zone: seconds east of UTC at `utc`.
pub trait Zone {
    fn offset_at(&self, utc: i64) -> i32;
}

impl Zone for tz::Rule {
    fn offset_at(&self, utc: i64) -> i32 {
        tz::Rule::offset_at(self, utc)
    }
}

/// A fixed offset (seconds east of UTC).
impl Zone for i32 {
    fn offset_at(&self, _: i64) -> i32 {
        *self
    }
}

/// The UTC of a wall time in `z`. A time in the spring gap moves forward by the gap; in the autumn overlap it is the
/// first occurrence (RFC 5545 3.3.5).
pub fn to_utc(wall: i64, z: &dyn Zone) -> i64 {
    let guess = wall - i64::from(z.offset_at(wall));
    let u = wall - i64::from(z.offset_at(guess));
    // with the offset of a day before, an overlap's earlier instant reads the same wall time
    let first = wall - i64::from(z.offset_at(u - 86_400));
    if first < u && to_wall(first, z) == wall {
        first
    } else {
        u
    }
}

pub fn to_wall(utc: i64, z: &dyn Zone) -> i64 {
    utc + i64::from(z.offset_at(utc))
}

/// The zone of an iCalendar time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tz {
    Utc,
    /// No zone (a date, or a time without `Z` or TZID): the caller's local time.
    Floating,
    Rule(tz::Rule),
}

impl Tz {
    pub fn utc(&self, wall: i64, local: &dyn Zone) -> i64 {
        match self {
            Tz::Utc => wall,
            Tz::Floating => to_utc(wall, local),
            Tz::Rule(r) => to_utc(wall, r),
        }
    }

    pub fn wall(&self, utc: i64, local: &dyn Zone) -> i64 {
        match self {
            Tz::Utc => utc,
            Tz::Floating => to_wall(utc, local),
            Tz::Rule(r) => to_wall(utc, r),
        }
    }

    /// A zone named without a VTIMEZONE: UTC's names, or a world city ("Europe/Berlin" → Berlin's rule).
    pub fn named(tzid: &str) -> Tz {
        let t = tzid.trim().trim_start_matches('/');
        if ["UTC", "GMT", "Z", "Etc/UTC", "Etc/GMT"]
            .iter()
            .any(|n| t.eq_ignore_ascii_case(n))
        {
            return Tz::Utc;
        }
        world::find(&neo_format_pim::date::city(t))
            .and_then(|c| tz::Rule::parse(c.tz))
            .map_or(Tz::Floating, Tz::Rule)
    }
}

/// "+0100", "-0430", "+013000" → seconds east.
pub(crate) fn offset(s: &str) -> Option<i32> {
    let s = s.trim();
    let sign = match s.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let n = |r: core::ops::Range<usize>| s.get(r).and_then(|d| d.parse::<i32>().ok());
    let secs = n(1..3)? * 3600 + n(3..5)? * 60 + n(5..7).unwrap_or(0);
    Some(sign * secs)
}

/// A POSIX offset (west of UTC) for `east` seconds.
fn posix(east: i32) -> String {
    let w = -east;
    let a = w.abs();
    format!(
        "{}{}:{:02}:{:02}",
        if w < 0 { "-" } else { "" },
        a / 3600,
        a / 60 % 60,
        a % 60
    )
}

/// One STANDARD or DAYLIGHT part.
#[derive(Default)]
struct Part {
    start: Option<When>,
    to: Option<i32>,
    rule: String,
}

impl Part {
    /// `Mm.w.d/time` from `FREQ=YEARLY;BYMONTH=m;BYDAY=2SU` (or -1SU) and the part's start time.
    fn posix_change(&self) -> Option<String> {
        let (mut month, mut day) = (None, None);
        for p in self.rule.split(';') {
            match p.split_once('=') {
                Some(("FREQ", "YEARLY")) => {}
                Some(("BYMONTH", m)) => {
                    month = m.parse::<u8>().ok().filter(|m| (1..=12).contains(m))
                }
                Some(("BYDAY", d)) => day = Some(d),
                Some(("INTERVAL", "1")) | Some(("WKST", _)) | Some(("UNTIL", _)) => {}
                _ => return None,
            }
        }
        let d = day?;
        let wd = super::rrule::weekday(d.get(d.len().checked_sub(2)?..)?)?;
        let week = match d.get(..d.len() - 2)? {
            "-1" | "5" => 5,
            n => n.parse::<u8>().ok().filter(|n| (1..=4).contains(n))?,
        };
        let (h, m, s) = self.start.as_ref()?.time.unwrap_or((2, 0, 0));
        // POSIX weekdays count from Sunday
        Some(format!(
            "M{}.{week}.{}/{h}:{m:02}:{s:02}",
            month?,
            (wd + 1) % 7
        ))
    }
}

/// A VTIMEZONE's lines (BEGIN … END) as its TZID and rule: daylight saving when both parts repeat by a weekday rule
/// POSIX can say, else the standard offset alone.
pub(crate) fn vtimezone(lines: &[Line]) -> Option<(String, Tz)> {
    let mut tzid = None;
    let (mut std, mut dst): (Option<Part>, Option<Part>) = (None, None);
    let mut cur: Option<(bool, Part)> = None;
    for l in lines {
        let v = l.value.trim();
        match l.name.as_str() {
            "TZID" if cur.is_none() => tzid = Some(l.text().trim().to_string()),
            "BEGIN" if v.eq_ignore_ascii_case("DAYLIGHT") => cur = Some((true, Part::default())),
            "BEGIN" if v.eq_ignore_ascii_case("STANDARD") => cur = Some((false, Part::default())),
            "END" => {
                if let Some((daylight, p)) = cur.take() {
                    let slot = if daylight { &mut dst } else { &mut std };
                    // the newest part is today's rule (older ones are history)
                    let newer = slot.as_ref().is_none_or(|o| {
                        p.start.as_ref().map(When::seconds) >= o.start.as_ref().map(When::seconds)
                    });
                    if newer {
                        *slot = Some(p);
                    }
                }
            }
            "DTSTART" => {
                if let Some((_, p)) = &mut cur {
                    p.start = super::parse_when(v, None);
                }
            }
            "TZOFFSETTO" => {
                if let Some((_, p)) = &mut cur {
                    p.to = offset(v);
                }
            }
            "RRULE" => {
                if let Some((_, p)) = &mut cur {
                    p.rule = v.to_ascii_uppercase();
                }
            }
            _ => {}
        }
    }
    let tzid = tzid?;
    let fixed = |east: i32| tz::Rule::parse(&format!("<S>{}", posix(east)));
    let rule = match (&std, &dst) {
        (Some(s), Some(d)) => {
            let full = (|| {
                Some(format!(
                    "<S>{}<D>{},{},{}",
                    posix(s.to?),
                    posix(d.to?),
                    d.posix_change()?,
                    s.posix_change()?
                ))
            })();
            full.and_then(|f| tz::Rule::parse(&f))
                .or_else(|| fixed(s.to?))
        }
        (Some(p), None) | (None, Some(p)) => fixed(p.to?),
        (None, None) => None,
    };
    Some((tzid, rule.map_or(Tz::Floating, Tz::Rule)))
}

/// A wall time as a [`When`]: a date when `all_day`, else UTC, in `tzid`, or floating.
pub fn when_at(wall: i64, all_day: bool, utc: bool, tzid: Option<&str>) -> When {
    let day = wall.div_euclid(86_400);
    let sod = wall.rem_euclid(86_400);
    When {
        date: Date::from_days(day),
        // sod < 86400, so each part fits
        time: (!all_day).then_some(((sod / 3600) as u8, (sod / 60 % 60) as u8, (sod % 60) as u8)),
        utc: utc && !all_day,
        tzid: tzid.filter(|_| !all_day && !utc).map(str::to_string),
    }
}
