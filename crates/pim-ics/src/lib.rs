// SPDX-License-Identifier: Apache-2.0
//! iCalendar for Calendar and DTSRV (docs datetime.md DT3), on files' `neo-format-pim`: its content lines (folding,
//! escapes, the exact text of every line) and its [`When`]/[`Date`]/[`duration`] types, plus what the stores need:
//! UIDs, adding and removing components, RRULE/EXDATE/RECURRENCE-ID, to-do fields, reminders, calendar names and
//! colours, and zones with daylight saving.
//!
//! `neo_format_pim::ical::Ics` keeps its lines private and reads one component at a time, so [`Ics`] here keeps its
//! own lines (made by `neo_format_pim::lines`). As there, an edit replaces only the lines it changes, so unknown
//! properties and components come back byte for byte.
#![no_std]

extern crate alloc;

pub mod rrule;
pub mod zone;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
pub use neo_format_pim::date::{self, Date};
pub use neo_format_pim::ical::{When, duration};
pub use neo_format_pim::{Line, escape, unescape};
pub use rrule::{Freq, Rule};
pub use zone::{Tz, Zone, to_utc, to_wall, when_at};

/// A component directly inside the VCALENDAR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comp {
    Event,
    Todo,
    Zone,
    Other,
}

/// One VCALENDAR (or any iCalendar text: a bare VEVENT, or nothing).
#[derive(Clone, Debug)]
pub struct Ics {
    lines: Vec<Line>,
    eol: &'static str,
    /// BEGIN:VCALENDAR and its END
    cal: Option<(usize, usize)>,
    /// kind, BEGIN, END
    comps: Vec<(Comp, usize, usize)>,
    zones: Vec<(String, Tz)>,
}

fn eol_of(text: &str) -> &'static str {
    match text.find('\n') {
        Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => "\r\n",
        Some(_) => "\n",
        None => "\r\n",
    }
}

/// The line's name, a UTF-8 BOM before the first one ignored.
fn name(l: &Line) -> &str {
    l.name.trim_start_matches('\u{feff}')
}

fn is(l: &Line, n: &str, v: &str) -> bool {
    name(l) == n && l.value.trim().eq_ignore_ascii_case(v)
}

/// One line of text (`head:value`, folded).
fn line(head: &str, value: &str, eol: &str) -> Line {
    let text = neo_format_pim::folded(head, value, eol);
    neo_format_pim::lines(&text).pop().unwrap_or(Line {
        raw: text,
        group: String::new(),
        name: String::new(),
        params: Vec::new(),
        value: String::new(),
    })
}

/// `YYYYMMDD[THHMMSS[Z]]` with the TZID of its line.
pub fn parse_when(v: &str, tzid: Option<&str>) -> Option<When> {
    let v = v.trim();
    let date = Date::parse(v)?;
    let time = match v.get(8..) {
        Some(t) if t.starts_with('T') => {
            let n = |r: core::ops::Range<usize>| t.get(r).and_then(|s| s.parse::<u8>().ok());
            let (h, m, s) = (n(1..3)?, n(3..5)?, n(5..7)?);
            (h < 24 && m < 60 && s < 61).then_some((h, m, s))
        }
        Some("") | None => None,
        Some(_) => return None,
    };
    Some(When {
        date,
        time,
        utc: time.is_some() && v.ends_with('Z'),
        tzid: tzid.filter(|_| time.is_some()).map(str::to_string),
    })
}

pub fn when_value(w: &When) -> String {
    match w.time {
        None => w.date.ymd(),
        Some((h, m, s)) => format!(
            "{}T{h:02}{m:02}{s:02}{}",
            w.date.ymd(),
            if w.utc { "Z" } else { "" }
        ),
    }
}

/// `NAME;VALUE=DATE` or `NAME;TZID=…` for a time.
fn when_head(name: &str, w: &When) -> String {
    match (&w.time, &w.tzid) {
        (None, _) => format!("{name};VALUE=DATE"),
        (Some(_), Some(z)) if !w.utc => format!("{name};TZID={z}"),
        _ => String::from(name),
    }
}

/// A UTC time as `YYYYMMDDTHHMMSSZ`.
pub fn stamp(utc: i64) -> String {
    when_value(&when_at(utc, false, true, None))
}

/// A new UID from the caller's `seed` (no RNG on the device: the caller mixes in what it has, e.g. a counter and the
/// boot time) and the time.
pub fn new_uid(seed: u64, now_utc: i64) -> String {
    format!("{now_utc:x}-{seed:016x}@neosymbian")
}

/// A TRIGGER for `minutes` before the start.
fn trigger(minutes: i64) -> String {
    let (sign, m) = if minutes > 0 {
        ("-", minutes)
    } else {
        ("", -minutes)
    };
    match m {
        0 => String::from("PT0S"),
        m if m % 1440 == 0 => format!("{sign}P{}D", m / 1440),
        m if m % 60 == 0 => format!("{sign}PT{}H", m / 60),
        m => format!("{sign}PT{m}M"),
    }
}

impl Ics {
    /// Never fails: text without a VCALENDAR or components is kept as it is and gets one on the first [`Ics::add`].
    pub fn parse(text: &str) -> Ics {
        let mut ics = Ics {
            lines: neo_format_pim::lines(text),
            eol: eol_of(text),
            cal: None,
            comps: Vec::new(),
            zones: Vec::new(),
        };
        ics.scan();
        ics
    }

    /// Each VCALENDAR of a file of several (text before the first stays with the first, so writing them all in order
    /// gives the file back).
    pub fn parse_all(text: &str) -> Vec<Ics> {
        let eol = eol_of(text);
        let mut parts: Vec<Vec<Line>> = Vec::new();
        let mut depth = 0usize;
        for l in neo_format_pim::lines(text) {
            if depth == 0
                && is(&l, "BEGIN", "VCALENDAR")
                && parts
                    .last()
                    .is_none_or(|p| p.iter().any(|l| is(l, "BEGIN", "VCALENDAR")))
            {
                parts.push(Vec::new());
            }
            match name(&l) {
                "BEGIN" => depth += 1,
                "END" => depth = depth.saturating_sub(1),
                _ => {}
            }
            match parts.last_mut() {
                Some(p) => p.push(l),
                None => parts.push(alloc::vec![l]),
            }
        }
        parts
            .into_iter()
            .map(|lines| {
                let mut ics = Ics {
                    lines,
                    eol,
                    cal: None,
                    comps: Vec::new(),
                    zones: Vec::new(),
                };
                ics.scan();
                ics
            })
            .collect()
    }

    /// An empty VCALENDAR named `name` (none when empty).
    pub fn new(name: &str) -> Ics {
        let mut ics = Ics::parse(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//NeoSymbian//Calendar 1.0//EN\r\nEND:VCALENDAR\r\n",
        );
        if !name.is_empty() {
            ics.set_cal_text("X-WR-CALNAME", Some(name));
        }
        ics
    }

    fn scan(&mut self) {
        self.cal = None;
        self.comps.clear();
        self.zones.clear();
        // the open components: whether it is the VCALENDAR, and its BEGIN
        let mut open: Vec<(bool, usize)> = Vec::new();
        for (k, l) in self.lines.iter().enumerate() {
            match name(l) {
                "BEGIN" => open.push((open.is_empty() && is(l, "BEGIN", "VCALENDAR"), k)),
                "END" => {
                    let Some((vcal, b)) = open.pop() else {
                        continue;
                    };
                    if vcal {
                        self.cal.get_or_insert((b, k));
                    } else if open.is_empty() || (open.len() == 1 && open[0].0) {
                        let kind = match self.lines[b].value.trim().to_ascii_uppercase().as_str() {
                            "VEVENT" => Comp::Event,
                            "VTODO" => Comp::Todo,
                            "VTIMEZONE" => Comp::Zone,
                            _ => Comp::Other,
                        };
                        self.comps.push((kind, b, k));
                    }
                }
                _ => {}
            }
        }
        for &(kind, b, e) in &self.comps {
            if kind == Comp::Zone
                && let Some(z) = zone::vtimezone(&self.lines[b..=e])
            {
                self.zones.push(z);
            }
        }
    }

    pub fn write(&self) -> String {
        self.lines.iter().map(|l| l.raw.as_str()).collect()
    }

    /// The number of components.
    pub fn len(&self) -> usize {
        self.comps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.comps.is_empty()
    }

    pub fn comp(&self, i: usize) -> Comp {
        self.comps[i].0
    }

    /// Component `i`'s text, BEGIN to END.
    pub fn comp_text(&self, i: usize) -> String {
        let (_, b, e) = self.comps[i];
        self.lines[b..=e].iter().map(|l| l.raw.as_str()).collect()
    }

    /// The indices of the events and to-dos with `uid` (a series and its changed instances).
    pub fn find_uid(&self, uid: &str) -> Vec<usize> {
        (0..self.len())
            .filter(|&i| {
                matches!(self.comp(i), Comp::Event | Comp::Todo)
                    && self.uid(i).as_deref() == Some(uid)
            })
            .collect()
    }

    // --- the calendar's own properties

    /// The VCALENDAR's own lines (not inside a component).
    fn cal_own(&self) -> Vec<usize> {
        let Some((b, e)) = self.cal else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut depth = 0usize;
        for k in b + 1..e {
            match name(&self.lines[k]) {
                "BEGIN" => depth += 1,
                "END" => depth = depth.saturating_sub(1),
                _ if depth == 0 => out.push(k),
                _ => {}
            }
        }
        out
    }

    /// A calendar property as text (X-WR-CALNAME, X-NEO-COLOR…).
    pub fn cal_text(&self, prop: &str) -> Option<String> {
        self.cal_own()
            .into_iter()
            .find(|&k| self.lines[k].name == prop)
            .map(|k| self.lines[k].text())
    }

    pub fn set_cal_text(&mut self, prop: &str, text: Option<&str>) {
        if self.cal.is_none() {
            self.wrap();
        }
        let at = self
            .cal_own()
            .into_iter()
            .find(|&k| self.lines[k].name == prop);
        match (at, text) {
            (Some(k), None) => {
                self.lines.remove(k);
            }
            (Some(k), Some(t)) => self.lines[k] = line(prop, &escape(t, true), self.eol),
            (None, Some(t)) => {
                let Some((b, e)) = self.cal else { return };
                let pos = (b + 1..e)
                    .find(|&k| name(&self.lines[k]) == "BEGIN")
                    .unwrap_or(e);
                let l = line(prop, &escape(t, true), self.eol);
                self.lines.insert(pos, l);
            }
            (None, None) => return,
        }
        self.scan();
    }

    /// Calendar's name (X-WR-CALNAME).
    pub fn name(&self) -> Option<String> {
        self.cal_text("X-WR-CALNAME")
    }

    /// Calendar's colour (X-NEO-COLOR, `#RRGGBB`).
    pub fn colour(&self) -> Option<u32> {
        let c = self.cal_text("X-NEO-COLOR")?;
        u32::from_str_radix(c.trim().trim_start_matches('#'), 16)
            .ok()
            .filter(|c| *c <= 0xFF_FFFF)
    }

    pub fn set_colour(&mut self, rgb: u32) {
        self.set_cal_text("X-NEO-COLOR", Some(&format!("#{:06X}", rgb & 0xFF_FFFF)));
    }

    /// Add an empty VCALENDAR at the end (for text that had none).
    fn wrap(&mut self) {
        let eol = self.eol;
        let text = format!(
            "BEGIN:VCALENDAR{eol}VERSION:2.0{eol}PRODID:-//NeoSymbian//Calendar 1.0//EN{eol}END:VCALENDAR{eol}"
        );
        if self.lines.last().is_some_and(|l| !l.raw.ends_with('\n')) {
            let l = self.lines.len() - 1;
            self.lines[l].raw.push_str(eol);
        }
        self.lines.extend(neo_format_pim::lines(&text));
        self.scan();
    }

    // --- components

    /// Add component text (BEGIN … END, as from [`Ics::comp_text`] of another file) before the VCALENDAR's END; its
    /// index, or `None` when `text` holds no component.
    pub fn insert(&mut self, text: &str) -> Option<usize> {
        let mut new = neo_format_pim::lines(text);
        if new.is_empty() {
            return None;
        }
        // line ends in this file's own form
        for l in &mut new {
            let body = l.raw.trim_end_matches(['\r', '\n']).len();
            if body < l.raw.len() {
                l.raw.truncate(body);
                l.raw.push_str(self.eol);
            }
        }
        if let Some(l) = new.last_mut()
            && !l.raw.ends_with('\n')
        {
            l.raw.push_str(self.eol);
        }
        if self.cal.is_none() {
            self.wrap();
        }
        let (_, e) = self.cal?;
        let before = self.comps.len();
        let first = new.first().map(|l| name(l) == "BEGIN").unwrap_or(false);
        self.lines.splice(e..e, new);
        self.scan();
        (first && self.comps.len() > before)
            .then(|| self.comps.iter().position(|c| c.1 == e))
            .flatten()
    }

    /// Add an empty VEVENT or VTODO with `uid` and a DTSTAMP; its index.
    pub fn add(&mut self, kind: Comp, uid: &str, now_utc: i64) -> Option<usize> {
        let n = if kind == Comp::Todo {
            "VTODO"
        } else {
            "VEVENT"
        };
        let eol = self.eol;
        let text = format!(
            "BEGIN:{n}{eol}{}{}END:{n}{eol}",
            neo_format_pim::folded("UID", &escape(uid, true), eol),
            neo_format_pim::folded("DTSTAMP", &stamp(now_utc), eol)
        );
        self.insert(&text)
    }

    pub fn remove(&mut self, i: usize) {
        let (_, b, e) = self.comps[i];
        self.lines.drain(b..=e);
        self.scan();
    }

    // --- properties of component i

    /// Component `i`'s own lines (not those of its VALARMs).
    fn own(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let (_, b, e) = self.comps[i];
        let mut depth = 0usize;
        (b + 1..e).filter(move |&k| match name(&self.lines[k]) {
            "BEGIN" => {
                depth += 1;
                false
            }
            "END" => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0,
        })
    }

    pub fn prop(&self, i: usize, prop: &str) -> Option<&Line> {
        self.own(i).map(|k| &self.lines[k]).find(|l| l.name == prop)
    }

    pub fn text(&self, i: usize, prop: &str) -> Option<String> {
        self.prop(i, prop).map(Line::text)
    }

    pub fn uid(&self, i: usize) -> Option<String> {
        self.text(i, "UID").map(|u| u.trim().to_string())
    }

    pub fn when(&self, i: usize, prop: &str) -> Option<When> {
        let l = self.prop(i, prop)?;
        parse_when(&l.value, l.param("TZID"))
    }

    /// Every time of a list property (EXDATE: several lines, each with commas).
    pub fn whens(&self, i: usize, prop: &str) -> Vec<When> {
        self.own(i)
            .map(|k| &self.lines[k])
            .filter(|l| l.name == prop)
            .flat_map(|l| {
                l.value
                    .split(',')
                    .filter_map(|v| parse_when(v, l.param("TZID")))
            })
            .collect()
    }

    /// The end: DTEND (DUE for a to-do), or DTSTART + DURATION.
    pub fn end(&self, i: usize) -> Option<When> {
        self.when(i, "DTEND")
            .or_else(|| self.when(i, "DUE"))
            .or_else(|| {
                let d = duration(&self.prop(i, "DURATION")?.value)?;
                Some(self.when(i, "DTSTART")?.plus(d))
            })
    }

    /// The RRULE as read (a rule outside [`Rule`]'s subset too).
    pub fn rrule(&self, i: usize) -> Option<String> {
        self.prop(i, "RRULE").map(|l| l.value.trim().to_string())
    }

    /// PRIORITY 1 (high) … 9 (low); 0 = none.
    pub fn priority(&self, i: usize) -> u8 {
        self.text(i, "PRIORITY")
            .and_then(|p| p.trim().parse::<u8>().ok())
            .filter(|p| *p <= 9)
            .unwrap_or(0)
    }

    /// Replace `prop`'s line (or add it before the first nested component) as `head:value`; `None` removes every
    /// `prop` line.
    pub fn set_raw(&mut self, i: usize, prop: &str, head_value: Option<(&str, &str)>) {
        let at: Vec<usize> = self
            .own(i)
            .filter(|&k| self.lines[k].name == prop)
            .collect();
        match head_value {
            None => {
                for &k in at.iter().rev() {
                    self.lines.remove(k);
                }
            }
            Some((head, value)) => {
                let l = line(head, value, self.eol);
                match at.first() {
                    Some(&k) => self.lines[k] = l,
                    None => {
                        let (_, b, e) = self.comps[i];
                        let pos = (b + 1..e)
                            .find(|&k| name(&self.lines[k]) == "BEGIN")
                            .unwrap_or(e);
                        self.lines.insert(pos, l);
                    }
                }
            }
        }
        self.scan();
    }

    pub fn set_text(&mut self, i: usize, prop: &str, text: Option<&str>) {
        let v = text.filter(|t| !t.is_empty()).map(|t| escape(t, true));
        self.set_raw(i, prop, v.as_deref().map(|v| (prop, v)));
    }

    pub fn set_when(&mut self, i: usize, prop: &str, w: Option<&When>) {
        let hv = w.map(|w| (when_head(prop, w), when_value(w)));
        self.set_raw(i, prop, hv.as_ref().map(|(h, v)| (h.as_str(), v.as_str())));
    }

    /// Add one more `prop` line (EXDATE) after the last one.
    pub fn add_when(&mut self, i: usize, prop: &str, w: &When) {
        let l = line(&when_head(prop, w), &when_value(w), self.eol);
        let pos = self
            .own(i)
            .filter(|&k| self.lines[k].name == prop || self.lines[k].name == "DTSTART")
            .last()
            .map_or(self.comps[i].1 + 1, |k| k + 1);
        self.lines.insert(pos, l);
        self.scan();
    }

    /// The first VALARM's TRIGGER line.
    fn trigger_line(&self, i: usize) -> Option<usize> {
        let (_, b, e) = self.comps[i];
        let mut in_alarm = false;
        for k in b + 1..e {
            let l = &self.lines[k];
            if is(l, "BEGIN", "VALARM") {
                in_alarm = true;
            } else if is(l, "END", "VALARM") {
                in_alarm = false;
            } else if in_alarm && l.name == "TRIGGER" {
                return Some(k);
            }
        }
        None
    }

    /// Minutes before the start of the first reminder (a TRIGGER relative to the end or at a fixed time reads as none).
    pub fn reminder(&self, i: usize) -> Option<i64> {
        let l = &self.lines[self.trigger_line(i)?];
        if l.has("VALUE", "DATE-TIME") || l.has("RELATED", "END") {
            return None;
        }
        duration(&l.value).map(|s| -s / 60)
    }

    /// Set the first reminder's trigger (adding a VALARM if there is none); `None` removes every VALARM.
    pub fn set_reminder(&mut self, i: usize, minutes: Option<i64>) {
        let eol = self.eol;
        match (minutes, self.trigger_line(i)) {
            (Some(m), Some(k)) => self.lines[k] = line("TRIGGER", &trigger(m), eol),
            (Some(m), None) => {
                let (_, _, e) = self.comps[i];
                let block = format!(
                    "BEGIN:VALARM{eol}ACTION:DISPLAY{eol}DESCRIPTION:Reminder{eol}TRIGGER:{}{eol}END:VALARM{eol}",
                    trigger(m)
                );
                self.lines.splice(e..e, neo_format_pim::lines(&block));
            }
            (None, _) => loop {
                let (_, b, e) = self.comps[i];
                let Some(s) = (b + 1..e).find(|&k| is(&self.lines[k], "BEGIN", "VALARM")) else {
                    break;
                };
                let end = (s..e)
                    .find(|&k| is(&self.lines[k], "END", "VALARM"))
                    .unwrap_or(s);
                self.lines.drain(s..=end);
                self.scan();
            },
        }
        self.scan();
    }

    // --- zones

    /// The zone of a time read from this file: UTC, its TZID's VTIMEZONE (or a known zone name), or floating.
    pub fn tz(&self, w: &When) -> Tz {
        if w.time.is_none() {
            return Tz::Floating;
        }
        if w.utc {
            return Tz::Utc;
        }
        match &w.tzid {
            None => Tz::Floating,
            Some(id) => self
                .zones
                .iter()
                .find(|(z, _)| z == id)
                .map_or_else(|| Tz::named(id), |(_, tz)| *tz),
        }
    }

    /// The VTIMEZONE defining `tzid`.
    pub fn zone_comp(&self, tzid: &str) -> Option<usize> {
        (0..self.len()).find(|&i| {
            self.comp(i) == Comp::Zone && self.text(i, "TZID").is_some_and(|t| t.trim() == tzid)
        })
    }
}
