// SPDX-License-Identifier: Apache-2.0
//! Calendar's model (docs datetime.md DT3): DTSRV's event and to-do stores (`calendar.ics`, one VCALENDAR per
//! calendar, and `todo.ics`), the recurrence subset, range queries with birthdays, reminders, import and export.
//!
//! Time: "local" times are seconds since 1970 of the caller's wall clock (a local date and time read as if UTC), so
//! `local / 86400` is the local day. The caller's zone comes in as a [`Zone`] (DTSRV's, from the kernel offset or a
//! `tz::Rule`); "utc" times are Unix seconds.
#![no_std]

extern crate alloc;

mod expand;
pub mod wire;

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use expand::{Key, Series};
use pim_ics::{Comp, Date, When, to_utc, to_wall, when_at};
pub use pim_ics::{Freq, Ics, Rule, Tz, Zone};

pub const DAY: i64 = 86_400;

/// The colours new calendars get in turn (`0xRRGGBB`).
pub const PALETTE: [u32; 6] = [0x3D7EFF, 0xE5484D, 0x30A46C, 0xF5A524, 0x8E4EC6, 0x12A594];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Event = 0,
    /// a yearly all-day event (`X-NEO-KIND:anniversary`)
    Anniversary = 1,
    /// from Contacts, never stored
    Birthday = 2,
}

pub struct Calendar {
    pub name: String,
    pub colour: u32,
    pub ics: Ics,
}

/// A contact's birthday, from NEOCONT's book.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Birthday {
    pub contact_id: u32,
    pub name: String,
    pub month: u8,
    pub day: u8,
    pub year: Option<i32>,
}

/// One occurrence, in local time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Instance {
    /// empty for a birthday
    pub uid: String,
    pub start: i64,
    /// exclusive; an all-day event ends at the midnight after its last day
    pub end: i64,
    /// the occurrence's original start (RECURRENCE-ID): what "this instance" edits name
    pub recur: i64,
    pub all_day: bool,
    pub kind: Kind,
    pub title: String,
    pub place: String,
    /// calendar index; `u8::MAX` for a birthday
    pub cal: u8,
    pub colour: u32,
    pub contact: u32,
    /// an anniversary's or birthday's count this time ("Turns 32", "7th"); 0 unknown
    pub years: u16,
    pub reminder: bool,
    pub repeats: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Never,
    Every {
        freq: Freq,
        interval: u16,
        /// WEEKLY's days, bit 0 = Monday; 0 = the start's weekday
        days: u8,
        /// 0 = no limit
        count: u16,
        /// the last local day
        until: Option<i64>,
    },
    /// an RRULE outside the subset: kept as written
    Other,
}

/// An event as NEOCAL edits it (EVENT_PUT) and reads it back for the event view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventDraft {
    /// empty: a new event
    pub uid: String,
    /// `None`: the whole series; `Some(recur)`: only that occurrence
    pub only: Option<i64>,
    pub cal: u8,
    pub kind: Kind,
    pub all_day: bool,
    /// local; all-day: midnights, the end exclusive
    pub start: i64,
    pub end: i64,
    pub title: String,
    pub place: String,
    pub notes: String,
    pub repeat: Repeat,
    /// minutes before the start
    pub reminder: Option<u16>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Todo {
    /// empty: a new to-do
    pub uid: String,
    pub title: String,
    pub notes: String,
    /// local
    pub due: Option<i64>,
    pub due_all_day: bool,
    /// 1 (high) … 9 (low); 0 none
    pub priority: u8,
    pub done: bool,
    /// utc
    pub completed: Option<i64>,
    /// minutes before the due time
    pub reminder: Option<u16>,
}

/// A reminder DTSRV arms: a small number standing for (uid, occurrence start) in the store's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReminderId(pub u32);

/// Ids fit the 24 bits the ring ids leave them.
const RING_IDS: u32 = 1 << 24;

/// A VEVENT as the queries need it.
struct Item {
    cal: usize,
    comp: usize,
    uid: String,
    /// RECURRENCE-ID: a changed occurrence of `uid`'s series
    rid: Option<Key>,
    kind: Kind,
    all_day: bool,
    start: When,
    /// wall time of DTSTART
    wall: i64,
    tz: Tz,
    /// seconds (an all-day event: whole days)
    dur: i64,
    rule: Option<Rule>,
    until: Option<Key>,
    repeats: bool,
    exdates: Vec<Key>,
    title: String,
    place: String,
    notes: String,
    reminder: Option<i64>,
}

/// A time read from `ics` in the form it compares in.
fn key(ics: &Ics, w: &When) -> Key {
    if w.time.is_none() {
        return Key::Day(w.date.days());
    }
    match ics.tz(w) {
        Tz::Utc => Key::Utc(w.seconds()),
        Tz::Floating => Key::Wall(w.seconds()),
        tz @ Tz::Rule(_) => Key::Utc(tz.utc(w.seconds(), &0)),
    }
}

impl Item {
    fn read(ics: &Ics, cal: usize, comp: usize) -> Option<Item> {
        let start = ics.when(comp, "DTSTART")?;
        let all_day = start.time.is_none();
        let tz = ics.tz(&start);
        let wall = start.seconds();
        let dur = match ics.end(comp) {
            Some(e) if all_day => (e.date.days() - start.date.days()) * DAY,
            Some(e) if e.utc == start.utc && e.tzid == start.tzid => e.seconds() - wall,
            Some(e) => ics.tz(&e).utc(e.seconds(), &0) - tz.utc(wall, &0),
            None => 0,
        };
        let dur = if all_day { dur.max(DAY) } else { dur.max(0) };
        let text = ics.rrule(comp);
        let rule = text.as_deref().and_then(|r| Rule::parse(r, start.date));
        Some(Item {
            cal,
            comp,
            uid: ics.uid(comp).unwrap_or_default(),
            rid: ics.when(comp, "RECURRENCE-ID").map(|w| key(ics, &w)),
            kind: if ics
                .text(comp, "X-NEO-KIND")
                .is_some_and(|k| k.trim().eq_ignore_ascii_case("anniversary"))
            {
                Kind::Anniversary
            } else {
                Kind::Event
            },
            all_day,
            until: rule
                .as_ref()
                .and_then(|r| r.until.as_ref())
                .map(|u| key(ics, u)),
            repeats: text.is_some(),
            rule,
            exdates: ics
                .whens(comp, "EXDATE")
                .iter()
                .map(|w| key(ics, w))
                .collect(),
            title: ics.text(comp, "SUMMARY").unwrap_or_default(),
            place: ics.text(comp, "LOCATION").unwrap_or_default(),
            notes: ics.text(comp, "DESCRIPTION").unwrap_or_default(),
            reminder: ics.reminder(comp),
            start,
            wall,
            tz,
            dur,
        })
    }

    fn series(&self) -> Series<'_> {
        Series {
            start: self.wall,
            tz: self.tz,
            rule: self.rule.as_ref(),
            until: self.until,
        }
    }

    /// A local time as this event's DTSTART would write it (an EXDATE's or RECURRENCE-ID's value).
    fn like_start(&self, local: i64, zone: &dyn Zone) -> When {
        if self.all_day {
            return when_at(local, true, false, None);
        }
        let utc = to_utc(local, zone);
        match self.tz {
            Tz::Utc => when_at(utc, false, true, None),
            Tz::Floating => when_at(local, false, false, None),
            tz @ Tz::Rule(_) => {
                when_at(tz.wall(utc, zone), false, false, self.start.tzid.as_deref())
            }
        }
    }

    fn matches(&self, needle: &str) -> bool {
        [&self.title, &self.place, &self.notes]
            .iter()
            .any(|t| t.to_lowercase().contains(needle))
    }
}

pub struct Store {
    calendars: Vec<Calendar>,
    todos: Ics,
    items: Vec<Item>,
    /// ReminderId → (uid, occurrence start utc)
    rings: BTreeMap<u32, (String, i64)>,
    next_ring: u32,
    /// mixed into new UIDs (the caller's: boot time, a counter…)
    pub seed: u64,
    made: u64,
}

fn local_of(w: &When, ics: &Ics, zone: &dyn Zone) -> i64 {
    if w.time.is_none() {
        return w.date.days() * DAY;
    }
    to_wall(ics.tz(w).utc(w.seconds(), zone), zone)
}

impl Store {
    /// The stores from the text of `calendar.ics` and `todo.ics` (either may be empty: a first boot gets one
    /// calendar, "Personal").
    pub fn load(calendars: &str, todos: &str) -> Store {
        let mut s = Store {
            calendars: Vec::new(),
            todos: Ics::parse(todos),
            items: Vec::new(),
            rings: BTreeMap::new(),
            next_ring: 1,
            seed: 0,
            made: 0,
        };
        for ics in Ics::parse_all(calendars) {
            s.push_calendar(ics);
        }
        if s.calendars.is_empty() {
            s.add_calendar("Personal", PALETTE[0]);
        }
        s.reindex();
        s
    }

    fn push_calendar(&mut self, ics: Ics) {
        let n = self.calendars.len();
        self.calendars.push(Calendar {
            name: ics.name().unwrap_or_else(|| String::from("Calendar")),
            colour: ics.colour().unwrap_or(PALETTE[n % PALETTE.len()]),
            ics,
        });
    }

    /// `calendar.ics`'s text.
    pub fn calendars_text(&self) -> String {
        self.calendars.iter().map(|c| c.ics.write()).collect()
    }

    /// `todo.ics`'s text.
    pub fn todos_text(&self) -> String {
        self.todos.write()
    }

    pub fn calendars(&self) -> &[Calendar] {
        &self.calendars
    }

    pub fn add_calendar(&mut self, name: &str, colour: u32) -> usize {
        let mut ics = Ics::new(name);
        ics.set_colour(colour);
        self.push_calendar(ics);
        self.calendars.len() - 1
    }

    pub fn set_calendar(&mut self, i: usize, name: &str, colour: u32) {
        let Some(c) = self.calendars.get_mut(i) else {
            return;
        };
        if c.name != name {
            c.ics.set_cal_text("X-WR-CALNAME", Some(name));
            c.name = name.to_string();
        }
        if c.colour != colour {
            c.ics.set_colour(colour);
            c.colour = colour;
        }
    }

    /// Remove calendar `i` and its events (the last one stays).
    pub fn remove_calendar(&mut self, i: usize) -> bool {
        if i >= self.calendars.len() || self.calendars.len() == 1 {
            return false;
        }
        self.calendars.remove(i);
        self.reindex();
        true
    }

    /// The number of events (series count once) in calendar `i`.
    pub fn count(&self, i: usize) -> usize {
        self.items
            .iter()
            .filter(|it| it.cal == i && it.rid.is_none())
            .count()
    }

    fn reindex(&mut self) {
        self.items.clear();
        for (c, cal) in self.calendars.iter().enumerate() {
            for i in 0..cal.ics.len() {
                if cal.ics.comp(i) == Comp::Event
                    && let Some(it) = Item::read(&cal.ics, c, i)
                {
                    self.items.push(it);
                }
            }
        }
    }

    fn exists(&self, uid: &str) -> bool {
        self.items.iter().any(|it| it.uid == uid) || !self.todos.find_uid(uid).is_empty()
    }

    fn new_uid(&mut self, now_utc: i64) -> String {
        loop {
            self.made += 1;
            let u = pim_ics::new_uid(self.seed.wrapping_add(self.made), now_utc);
            if !self.exists(&u) {
                return u;
            }
        }
    }

    // --- queries

    /// Every changed occurrence's RECURRENCE-ID, by uid.
    fn overrides(&self) -> BTreeMap<&str, Vec<Key>> {
        let mut m: BTreeMap<&str, Vec<Key>> = BTreeMap::new();
        for it in &self.items {
            if let Some(r) = it.rid {
                m.entry(it.uid.as_str()).or_default().push(r);
            }
        }
        m
    }

    /// Calls `f(instance, start utc)` for `it`'s occurrences that overlap local `lo..hi`, in order, until it returns
    /// false.
    fn each(
        &self,
        it: &Item,
        lo: i64,
        hi: i64,
        zone: &dyn Zone,
        changed: &[Key],
        f: &mut dyn FnMut(Instance, i64) -> bool,
    ) {
        let colour = self.calendars.get(it.cal).map_or(0, |c| c.colour);
        // zones differ by at most 26 h: two days of margin on wall days
        let lo_day = (lo - it.dur).div_euclid(DAY) - 2;
        let hi_day = hi.div_euclid(DAY) + 2;
        expand::walls(&it.series(), zone, lo_day, hi_day, &mut |wall, utc| {
            if it.rid.is_none()
                && (it
                    .exdates
                    .iter()
                    .chain(changed)
                    .any(|k| k.hits(wall, utc, it.all_day)))
            {
                return true;
            }
            let (start, end) = if it.all_day {
                (wall, wall + it.dur)
            } else {
                (to_wall(utc, zone), to_wall(utc + it.dur, zone))
            };
            let inside = if start == end {
                lo <= start && start < hi
            } else {
                start < hi && end > lo
            };
            if !inside {
                return true;
            }
            let years = if it.kind == Kind::Anniversary {
                let y = Date::from_days(wall.div_euclid(DAY)).year - it.start.date.year;
                u16::try_from(y).unwrap_or(0)
            } else {
                0
            };
            f(
                Instance {
                    uid: it.uid.clone(),
                    start,
                    end,
                    recur: it.rid.map_or(start, |r| r.local(zone)),
                    all_day: it.all_day,
                    kind: it.kind,
                    title: it.title.clone(),
                    place: it.place.clone(),
                    cal: u8::try_from(it.cal).unwrap_or(u8::MAX - 1),
                    colour,
                    contact: 0,
                    years,
                    reminder: it.reminder.is_some(),
                    repeats: it.repeats,
                },
                utc,
            )
        });
    }

    fn collect(
        &self,
        lo: i64,
        hi: i64,
        zone: &dyn Zone,
        keep: &dyn Fn(&Item) -> bool,
    ) -> Vec<Instance> {
        let changed = self.overrides();
        let mut out = Vec::new();
        for it in self.items.iter().filter(|it| keep(it)) {
            let ch = changed.get(it.uid.as_str()).map_or(&[][..], Vec::as_slice);
            self.each(it, lo, hi, zone, ch, &mut |inst, _| {
                out.push(inst);
                true
            });
        }
        out
    }

    /// The occurrences on local days `from_day..=to_day` of the calendars not in `hidden` (bit i hides calendar i),
    /// with `birthdays`, sorted by start (all-day ones first on their day).
    pub fn query(
        &self,
        from_day: i64,
        to_day: i64,
        hidden: u32,
        zone: &dyn Zone,
        birthdays: &[Birthday],
    ) -> Vec<Instance> {
        let (lo, hi) = (from_day * DAY, (to_day + 1) * DAY);
        let mut out = self.collect(lo, hi, zone, &|it| {
            it.cal >= 32 || hidden & (1 << it.cal) == 0
        });
        for b in birthdays {
            let (y0, y1) = (Date::from_days(from_day).year, Date::from_days(to_day).year);
            for year in y0..=y1 {
                // 29 February shows on the 28th in other years
                let day = b
                    .day
                    .min(pim_ics::date::days_in(year, b.month.clamp(1, 12)));
                let d = Date {
                    year,
                    month: b.month.clamp(1, 12),
                    day: day.max(1),
                }
                .days();
                if d < from_day || d > to_day {
                    continue;
                }
                out.push(Instance {
                    start: d * DAY,
                    end: (d + 1) * DAY,
                    recur: d * DAY,
                    all_day: true,
                    kind: Kind::Birthday,
                    title: b.name.clone(),
                    cal: u8::MAX,
                    contact: b.contact_id,
                    years: b
                        .year
                        .and_then(|by| u16::try_from(year - by).ok())
                        .unwrap_or(0),
                    repeats: true,
                    ..Instance::default()
                });
            }
        }
        sort(&mut out);
        out
    }

    /// The first occurrence starting at or after `now_utc` (birthdays aside).
    pub fn next_event(&self, now_utc: i64, zone: &dyn Zone) -> Option<Instance> {
        let now = to_wall(now_utc, zone);
        let changed = self.overrides();
        let mut best: Option<Instance> = None;
        for it in &self.items {
            let ch = changed.get(it.uid.as_str()).map_or(&[][..], Vec::as_slice);
            let hi = best.as_ref().map_or(now + 400 * DAY, |b| b.start + 1);
            self.each(it, now, hi, zone, ch, &mut |inst, _| {
                if inst.start < now {
                    return true;
                }
                if best.as_ref().is_none_or(|b| inst.start < b.start) {
                    best = Some(inst);
                }
                false
            });
        }
        best
    }

    /// The reminders due in `from_utc..until_utc` (events and to-dos), as (when, id), in time order. Ids name the same
    /// occurrence on every call; ones for occurrences over a week before `from_utc` are dropped.
    pub fn reminders(
        &mut self,
        from_utc: i64,
        until_utc: i64,
        zone: &dyn Zone,
    ) -> Vec<(i64, ReminderId)> {
        let mut due: Vec<(i64, String, i64)> = Vec::new();
        let changed = self.overrides();
        for it in &self.items {
            let Some(m) = it.reminder else { continue };
            let lead = m * 60;
            let ch = changed.get(it.uid.as_str()).map_or(&[][..], Vec::as_slice);
            let (lo, hi) = (
                to_wall(from_utc, zone) + lead - DAY,
                to_wall(until_utc, zone) + lead + DAY,
            );
            self.each(it, lo, hi, zone, ch, &mut |inst, utc| {
                let utc = if inst.all_day {
                    to_utc(inst.start, zone)
                } else {
                    utc
                };
                let at = utc - lead;
                if from_utc <= at && at < until_utc {
                    due.push((at, inst.uid, utc));
                }
                true
            });
        }
        for t in self.todos(zone) {
            if let (Some(d), Some(m), false) = (t.due, t.reminder, t.done) {
                let utc = to_utc(d, zone);
                let at = utc - i64::from(m) * 60;
                if from_utc <= at && at < until_utc {
                    due.push((at, t.uid, utc));
                }
            }
        }
        due.sort();
        self.rings.retain(|_, (_, s)| *s >= from_utc - 7 * DAY);
        due.into_iter()
            .map(|(at, uid, start)| (at, self.ring_id(uid, start)))
            .collect()
    }

    fn ring_id(&mut self, uid: String, start: i64) -> ReminderId {
        if let Some((&id, _)) = self
            .rings
            .iter()
            .find(|(_, (u, s))| *u == uid && *s == start)
        {
            return ReminderId(id);
        }
        while self.rings.contains_key(&self.next_ring) {
            self.next_ring = self.next_ring % (RING_IDS - 1) + 1;
        }
        let id = self.next_ring;
        self.next_ring = self.next_ring % (RING_IDS - 1) + 1;
        self.rings.insert(id, (uid, start));
        ReminderId(id)
    }

    /// The (uid, occurrence start utc) a reminder id stands for.
    pub fn reminder(&self, id: ReminderId) -> Option<(&str, i64)> {
        self.rings.get(&id.0).map(|(u, s)| (u.as_str(), *s))
    }

    /// The occurrences on `from_day..=to_day` and the to-dos whose text has `text` (any case).
    pub fn search(
        &self,
        text: &str,
        from_day: i64,
        to_day: i64,
        zone: &dyn Zone,
    ) -> (Vec<Instance>, Vec<Todo>) {
        let needle = text.trim().to_lowercase();
        let mut found = self.collect(from_day * DAY, (to_day + 1) * DAY, zone, &|it| {
            it.matches(&needle)
        });
        sort(&mut found);
        let todos = self
            .todos(zone)
            .into_iter()
            .filter(|t| {
                t.title.to_lowercase().contains(&needle) || t.notes.to_lowercase().contains(&needle)
            })
            .collect();
        (found, todos)
    }

    // --- events

    fn master(&self, uid: &str) -> Option<&Item> {
        self.items
            .iter()
            .find(|it| it.uid == uid && it.rid.is_none())
    }

    fn draft(&self, it: &Item, zone: &dyn Zone) -> EventDraft {
        let ics = &self.calendars[it.cal].ics;
        let start = local_of(&it.start, ics, zone);
        let end = if it.all_day {
            start + it.dur
        } else {
            to_wall(it.tz.utc(it.wall, zone) + it.dur, zone)
        };
        let repeat = match (&it.rule, it.repeats) {
            (Some(r), _) => Repeat::Every {
                freq: r.freq,
                interval: u16::try_from(r.interval).unwrap_or(u16::MAX),
                days: r.days,
                count: r.count.map_or(0, |c| u16::try_from(c).unwrap_or(u16::MAX)),
                until: it.until.map(|u| u.local(zone).div_euclid(DAY)),
            },
            (None, true) => Repeat::Other,
            (None, false) => Repeat::Never,
        };
        EventDraft {
            uid: it.uid.clone(),
            only: it.rid.map(|r| r.local(zone)),
            cal: u8::try_from(it.cal).unwrap_or(0),
            kind: it.kind,
            all_day: it.all_day,
            start,
            end,
            title: it.title.clone(),
            place: it.place.clone(),
            notes: it.notes.clone(),
            repeat,
            reminder: it.reminder.and_then(|m| u16::try_from(m).ok()),
        }
    }

    fn changed_one(&self, uid: &str, recur: i64, zone: &dyn Zone) -> Option<&Item> {
        let utc = to_utc(recur, zone);
        self.items
            .iter()
            .find(|it| it.uid == uid && it.rid.is_some_and(|r| r.hits(recur, utc, it.all_day)))
    }

    /// The event `uid` as the form edits it: the series, or with `only` that occurrence (as changed, or as the series
    /// has it).
    pub fn get(&self, uid: &str, only: Option<i64>, zone: &dyn Zone) -> Option<EventDraft> {
        if let Some(r) = only {
            if let Some(it) = self.changed_one(uid, r, zone) {
                return Some(self.draft(it, zone));
            }
            let mut d = self.draft(self.master(uid)?, zone);
            d.end = r + (d.end - d.start);
            d.start = r;
            d.only = Some(r);
            d.repeat = Repeat::Never;
            return Some(d);
        }
        Some(self.draft(self.master(uid)?, zone))
    }

    /// Write `d`: a new event (empty uid), the whole series, or one occurrence (`d.only`). Only the fields that
    /// changed are written. The uid, or `None` when `d.uid` names no event.
    pub fn put(&mut self, d: &EventDraft, zone: &dyn Zone, now_utc: i64) -> Option<String> {
        let mut d = d.clone();
        if d.kind == Kind::Anniversary {
            d.all_day = true;
            // the series repeats yearly; a changed occurrence (RECURRENCE-ID) stays a single one
            if d.only.is_none()
                && !matches!(
                    d.repeat,
                    Repeat::Every {
                        freq: Freq::Yearly,
                        ..
                    }
                )
            {
                d.repeat = Repeat::Every {
                    freq: Freq::Yearly,
                    interval: 1,
                    days: 0,
                    count: 0,
                    until: None,
                };
            }
        }
        if d.all_day {
            d.start = d.start.div_euclid(DAY) * DAY;
            d.end = (d.end.div_euclid(DAY) * DAY).max(d.start + DAY);
        }
        d.end = d.end.max(d.start);
        let cal = usize::from(d.cal).min(self.calendars.len() - 1);
        if d.uid.is_empty() {
            let uid = self.new_uid(now_utc);
            let ics = &mut self.calendars[cal].ics;
            let i = ics.add(Comp::Event, &uid, now_utc)?;
            d.only = None;
            write_event(ics, i, None, &d, zone);
            self.reindex();
            return Some(uid);
        }
        let master = self.master(&d.uid)?;
        let (mcal, mcomp) = (master.cal, master.comp);
        if let Some(r) = d.only {
            let (c, i, old) = match self.changed_one(&d.uid, r, zone) {
                Some(it) => (it.cal, it.comp, self.draft(it, zone)),
                None => {
                    // a copy of the series with RECURRENCE-ID and the occurrence's times
                    let m = self.master(&d.uid)?;
                    let rid = m.like_start(r, zone);
                    let old = self.get(&d.uid, Some(r), zone)?;
                    let ics = &mut self.calendars[mcal].ics;
                    let i = ics.insert(&ics.comp_text(mcomp))?;
                    for p in ["RRULE", "RDATE", "EXDATE"] {
                        ics.set_raw(i, p, None);
                    }
                    ics.set_when(i, "RECURRENCE-ID", Some(&rid));
                    let mut moved = old.clone();
                    // a start that can't match: the copy gets the occurrence's own times written
                    moved.start = !old.start;
                    write_event(ics, i, Some(&moved), &old, zone);
                    (mcal, i, old)
                }
            };
            write_event(&mut self.calendars[c].ics, i, Some(&old), &d, zone);
        } else {
            let old = self.draft(self.master(&d.uid)?, zone);
            write_event(&mut self.calendars[mcal].ics, mcomp, Some(&old), &d, zone);
            if cal != mcal {
                self.move_uid(&d.uid, mcal, cal);
            }
        }
        self.reindex();
        Some(d.uid)
    }

    /// Move `uid`'s components (and the zones they need) from calendar `from` to `to`.
    fn move_uid(&mut self, uid: &str, from: usize, to: usize) {
        let src = &mut self.calendars[from].ics;
        let comps = src.find_uid(uid);
        let texts: Vec<String> = comps.iter().map(|&i| src.comp_text(i)).collect();
        let zones = zone_texts(src);
        for &i in comps.iter().rev() {
            src.remove(i);
        }
        let dst = &mut self.calendars[to].ics;
        copy_zones(dst, &zones);
        for t in &texts {
            dst.insert(t);
        }
    }

    /// Delete `uid`: the series, or with `only` that occurrence (an EXDATE on the series, and its changed copy goes).
    pub fn del(&mut self, uid: &str, only: Option<i64>, zone: &dyn Zone) -> bool {
        match only {
            None => {
                let mut any = false;
                for c in &mut self.calendars {
                    for i in c.ics.find_uid(uid).into_iter().rev() {
                        c.ics.remove(i);
                        any = true;
                    }
                }
                self.reindex();
                any
            }
            Some(r) => {
                let changed = self.changed_one(uid, r, zone).map(|it| (it.cal, it.comp));
                let master = self
                    .master(uid)
                    .map(|m| (m.cal, m.comp, m.like_start(r, zone)));
                // the EXDATE first: adding a line keeps component indices, removing a component doesn't
                if let Some((c, i, ex)) = &master {
                    self.calendars[*c].ics.add_when(*i, "EXDATE", ex);
                }
                if let Some((c, i)) = changed {
                    self.calendars[c].ics.remove(i);
                }
                self.reindex();
                changed.is_some() || master.is_some()
            }
        }
    }

    // --- import and export

    /// Add the events (and to-dos) of an .ics file to calendar `cal`. Those whose UID is already in the store are
    /// left out: (added, their uids).
    pub fn import(&mut self, text: &str, cal: usize, now_utc: i64) -> (usize, Vec<String>) {
        let cal = cal.min(self.calendars.len() - 1);
        let mut added = 0;
        let mut clashes: Vec<String> = Vec::new();
        let files = Ics::parse_all(text);
        let mut known = Vec::new();
        for f in &files {
            for i in 0..f.len() {
                known.push(f.uid(i).is_some_and(|u| self.exists(&u)));
            }
        }
        let mut n = 0;
        for f in &files {
            copy_zones(&mut self.calendars[cal].ics, &zone_texts(f));
            for i in 0..f.len() {
                let clash = known[n];
                n += 1;
                let kind = f.comp(i);
                if !matches!(kind, Comp::Event | Comp::Todo) {
                    continue;
                }
                if clash {
                    let u = f.uid(i).unwrap_or_default();
                    if !clashes.contains(&u) {
                        clashes.push(u);
                    }
                    continue;
                }
                let no_uid = f.uid(i).is_none_or(|u| u.is_empty());
                let uid = if no_uid {
                    self.new_uid(now_utc)
                } else {
                    String::new()
                };
                let dst = if kind == Comp::Todo {
                    &mut self.todos
                } else {
                    &mut self.calendars[cal].ics
                };
                if let Some(j) = dst.insert(&f.comp_text(i)) {
                    if no_uid {
                        dst.set_text(j, "UID", Some(&uid));
                    }
                    added += 1;
                }
            }
        }
        self.reindex();
        (added, clashes)
    }

    /// One VCALENDAR with `uid`'s components (an event's series and changed occurrences, or a to-do), or with
    /// everything; with the zones they need.
    pub fn export(&self, uid: Option<&str>) -> String {
        let mut out = Ics::new("");
        let sources = self.calendars.iter().map(|c| &c.ics).chain([&self.todos]);
        for ics in sources {
            let comps: Vec<usize> = match uid {
                Some(u) => ics.find_uid(u),
                None => (0..ics.len())
                    .filter(|&i| matches!(ics.comp(i), Comp::Event | Comp::Todo))
                    .collect(),
            };
            if comps.is_empty() {
                continue;
            }
            copy_zones(&mut out, &zone_texts(ics));
            for i in comps {
                out.insert(&ics.comp_text(i));
            }
        }
        out.write()
    }

    // --- to-dos

    /// The to-dos: open ones first, by due time (none last), then priority.
    pub fn todos(&self, zone: &dyn Zone) -> Vec<Todo> {
        let ics = &self.todos;
        let mut out: Vec<Todo> = (0..ics.len())
            .filter(|&i| ics.comp(i) == Comp::Todo)
            .map(|i| {
                let due = ics.when(i, "DUE");
                Todo {
                    uid: ics.uid(i).unwrap_or_default(),
                    title: ics.text(i, "SUMMARY").unwrap_or_default(),
                    notes: ics.text(i, "DESCRIPTION").unwrap_or_default(),
                    due: due.as_ref().map(|w| local_of(w, ics, zone)),
                    due_all_day: due.as_ref().is_some_and(|w| w.time.is_none()),
                    priority: ics.priority(i),
                    done: ics
                        .text(i, "STATUS")
                        .is_some_and(|s| s.trim().eq_ignore_ascii_case("COMPLETED"))
                        || ics.prop(i, "COMPLETED").is_some(),
                    completed: ics
                        .when(i, "COMPLETED")
                        .map(|w| ics.tz(&w).utc(w.seconds(), zone)),
                    reminder: ics.reminder(i).and_then(|m| u16::try_from(m).ok()),
                }
            })
            .collect();
        out.sort_by_key(|t| {
            (
                t.done,
                t.due.is_none(),
                t.due,
                if t.priority == 0 { 10 } else { t.priority },
            )
        });
        out
    }

    /// Write a to-do (a new one when `t.uid` is empty); only changed fields are written. Its uid.
    pub fn put_todo(&mut self, t: &Todo, zone: &dyn Zone, now_utc: i64) -> Option<String> {
        let old = (!t.uid.is_empty())
            .then(|| self.todos(zone).into_iter().find(|o| o.uid == t.uid))
            .flatten();
        let uid = if t.uid.is_empty() {
            self.new_uid(now_utc)
        } else if old.is_some() {
            t.uid.clone()
        } else {
            return None;
        };
        let i = match old {
            Some(_) => *self.todos.find_uid(&uid).first()?,
            None => self.todos.add(Comp::Todo, &uid, now_utc)?,
        };
        let o = old.unwrap_or_default();
        let ics = &mut self.todos;
        if o.title != t.title {
            ics.set_text(i, "SUMMARY", Some(&t.title));
        }
        if o.notes != t.notes {
            ics.set_text(i, "DESCRIPTION", Some(&t.notes));
        }
        if o.due != t.due || o.due_all_day != t.due_all_day {
            let w = t.due.map(|d| when_at(d, t.due_all_day, false, None));
            ics.set_when(i, "DUE", w.as_ref());
        }
        if o.priority != t.priority {
            let p = alloc::format!("{}", t.priority.min(9));
            ics.set_raw(
                i,
                "PRIORITY",
                (t.priority != 0).then_some(("PRIORITY", p.as_str())),
            );
        }
        if o.done != t.done {
            if t.done {
                ics.set_raw(i, "STATUS", Some(("STATUS", "COMPLETED")));
                let at = pim_ics::stamp(t.completed.unwrap_or(now_utc));
                ics.set_raw(i, "COMPLETED", Some(("COMPLETED", at.as_str())));
            } else {
                ics.set_raw(i, "STATUS", Some(("STATUS", "NEEDS-ACTION")));
                ics.set_raw(i, "COMPLETED", None);
            }
        }
        if o.reminder != t.reminder {
            ics.set_reminder(i, t.reminder.map(i64::from));
        }
        Some(uid)
    }

    pub fn del_todo(&mut self, uid: &str) -> bool {
        let found = self.todos.find_uid(uid);
        for &i in found.iter().rev() {
            self.todos.remove(i);
        }
        !found.is_empty()
    }

    /// The open to-dos due before local day `day`.
    pub fn overdue(&self, day: i64, zone: &dyn Zone) -> usize {
        self.todos(zone)
            .iter()
            .filter(|t| !t.done && t.due.is_some_and(|d| d.div_euclid(DAY) < day))
            .count()
    }
}

/// Sorted by start, all-day ones first on their day.
fn sort(v: &mut [Instance]) {
    v.sort_by(|a, b| {
        let k = |i: &Instance| {
            (
                if i.all_day {
                    i.start
                } else {
                    i.start.div_euclid(DAY) * DAY
                },
                !i.all_day,
                i.start,
            )
        };
        k(a).cmp(&k(b)).then_with(|| a.title.cmp(&b.title))
    });
}

/// Every VTIMEZONE of `ics`: (TZID, text).
fn zone_texts(ics: &Ics) -> Vec<(String, String)> {
    (0..ics.len())
        .filter(|&i| ics.comp(i) == Comp::Zone)
        .filter_map(|i| Some((ics.text(i, "TZID")?.trim().to_string(), ics.comp_text(i))))
        .collect()
}

/// Add the zones `dst` doesn't define yet (before its events: insert puts them at the end, which iCalendar allows).
fn copy_zones(dst: &mut Ics, zones: &[(String, String)]) {
    for (id, text) in zones {
        if dst.zone_comp(id).is_none() {
            dst.insert(text);
        }
    }
}

/// Write the fields of `d` that differ from `old` (all of them for a new event).
fn write_event(ics: &mut Ics, i: usize, old: Option<&EventDraft>, d: &EventDraft, zone: &dyn Zone) {
    let blank = EventDraft {
        // a start that can't match: a new event gets every field written
        start: !d.start,
        ..EventDraft::default()
    };
    let o = old.unwrap_or(&blank);
    if o.title != d.title {
        ics.set_text(i, "SUMMARY", Some(&d.title));
    }
    if o.place != d.place {
        ics.set_text(i, "LOCATION", Some(&d.place));
    }
    if o.notes != d.notes {
        ics.set_text(i, "DESCRIPTION", Some(&d.notes));
    }
    if o.kind != d.kind {
        let k = (d.kind == Kind::Anniversary).then_some(("X-NEO-KIND", "anniversary"));
        ics.set_raw(i, "X-NEO-KIND", k);
    }
    let times = o.start != d.start || o.end != d.end || o.all_day != d.all_day;
    if times {
        // shortcut: new times are floating (the phone's wall clock); write a TZID + VTIMEZONE from the phone's
        // tz::Rule when shared events must keep their instant across zones
        let s = when_at(d.start, d.all_day, false, None);
        ics.set_when(i, "DTSTART", Some(&s));
        ics.set_raw(i, "DURATION", None);
        let e = when_at(d.end, d.all_day, false, None);
        ics.set_when(i, "DTEND", Some(&e));
    }
    if o.repeat != d.repeat || (times && matches!(d.repeat, Repeat::Every { until: Some(_), .. })) {
        match &d.repeat {
            Repeat::Never => ics.set_raw(i, "RRULE", None),
            Repeat::Other => {}
            &Repeat::Every {
                freq,
                interval,
                days,
                count,
                until,
            } => {
                let start = ics.when(i, "DTSTART");
                let until = until.map(|day| {
                    let last = day * DAY + DAY - 1;
                    match &start {
                        Some(s) if s.time.is_none() => when_at(day * DAY, true, false, None),
                        // RFC 5545: UNTIL is UTC when DTSTART has a zone
                        Some(s) if s.utc || s.tzid.is_some() => {
                            when_at(to_utc(last, zone), false, true, None)
                        }
                        _ => when_at(last, false, false, None),
                    }
                });
                let r = Rule {
                    freq,
                    interval: u32::from(interval.max(1)),
                    count: (count > 0).then_some(u32::from(count)),
                    until,
                    days: if freq == Freq::Weekly { days & 0x7F } else { 0 },
                };
                ics.set_raw(i, "RRULE", Some(("RRULE", &r.write())));
            }
        }
    }
    if o.reminder != d.reminder {
        ics.set_reminder(i, d.reminder.map(i64::from));
    }
}
