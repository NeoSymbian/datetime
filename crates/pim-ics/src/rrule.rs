// SPDX-License-Identifier: Apache-2.0
//! The RRULE subset Calendar edits and expands: FREQ=DAILY, WEEKLY (BYDAY), MONTHLY (on the start's day), YEARLY;
//! INTERVAL, COUNT, UNTIL. A rule outside it reads as `None` (and stays in the file as it was).

use alloc::format;
use alloc::string::String;
use neo_format_pim::date::Date;
use neo_format_pim::ical::When;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub freq: Freq,
    /// 1 or more
    pub interval: u32,
    pub count: Option<u32>,
    pub until: Option<When>,
    /// WEEKLY's days, bit 0 = Monday; 0 = the start's weekday
    pub days: u8,
}

const DAYS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

/// "MO" → 0 … "SU" → 6.
pub(crate) fn weekday(s: &str) -> Option<usize> {
    DAYS.iter().position(|d| s.eq_ignore_ascii_case(d))
}

impl Rule {
    /// The rule `s` for an event starting on `start`, or `None` outside the subset. BYMONTH and BYMONTHDAY are taken
    /// when they only repeat the start's own month and day (as many writers add them).
    pub fn parse(s: &str, start: Date) -> Option<Rule> {
        let mut r = Rule {
            freq: Freq::Daily,
            interval: 1,
            count: None,
            until: None,
            days: 0,
        };
        let mut freq = None;
        for part in s.trim().split(';').filter(|p| !p.is_empty()) {
            let (k, v) = part.split_once('=')?;
            match k.to_ascii_uppercase().as_str() {
                "FREQ" => {
                    freq = Some(match v.to_ascii_uppercase().as_str() {
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        _ => return None,
                    })
                }
                "INTERVAL" => r.interval = v.parse().ok().filter(|n| *n > 0)?,
                "COUNT" => r.count = Some(v.parse().ok()?),
                "UNTIL" => r.until = Some(super::parse_when(v, None)?),
                "WKST" => {} // shortcut: weeks start on Monday; WKST matters only for WEEKLY with INTERVAL>1 and BYDAY spanning a Sunday
                "BYDAY" => {
                    for d in v.split(',') {
                        r.days |= 1 << weekday(d)?;
                    }
                }
                "BYMONTHDAY" if v.parse() == Ok(start.day) => {}
                "BYMONTH" if v.parse() == Ok(start.month) => {}
                _ => return None,
            }
        }
        r.freq = freq?;
        (r.days == 0 || r.freq == Freq::Weekly).then_some(r)
    }

    /// The RRULE value.
    pub fn write(&self) -> String {
        let mut s = String::from(match self.freq {
            Freq::Daily => "FREQ=DAILY",
            Freq::Weekly => "FREQ=WEEKLY",
            Freq::Monthly => "FREQ=MONTHLY",
            Freq::Yearly => "FREQ=YEARLY",
        });
        if self.interval > 1 {
            s.push_str(&format!(";INTERVAL={}", self.interval));
        }
        if self.days != 0 {
            let days: alloc::vec::Vec<&str> = (0..7)
                .filter(|d| self.days & (1 << d) != 0)
                .map(|d| DAYS[d])
                .collect();
            s.push_str(";BYDAY=");
            s.push_str(&days.join(","));
        }
        if let Some(c) = self.count {
            s.push_str(&format!(";COUNT={c}"));
        }
        if let Some(u) = &self.until {
            s.push_str(";UNTIL=");
            s.push_str(&super::when_value(u));
        }
        s
    }
}
