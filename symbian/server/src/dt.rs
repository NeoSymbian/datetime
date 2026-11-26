// SPDX-License-Identifier: Apache-2.0
//! DTSRV's state: the alarms and the timer, the calendar's events and to-dos, what rings, the shade cards,
//! `dtsrv.next`, and dt-ipc's requests.

use crate::cal::{self, Local};
use crate::store;
use crate::ui::{OPEN_WAKE, Ui};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use calendar_core::wire;
use calendar_core::{Birthday, DAY, EventDraft, Instance, ReminderId, Store, Todo};
use core::cell::RefCell;
use core::ops::ControlFlow;
use datetime_core::alarm::{Alarms, format_hm};
use datetime_core::ipc::{self, CalendarInfo, RingInfo, action, mask, op, ring};
use datetime_core::next::Next;
use datetime_core::pages;
use datetime_core::plan::{self, At, Rung, boot_since, minute_of_week};
use datetime_core::timer::{Phase, Timer, format_left};
use e32::dprintln;
use e32::server::{Message, Service};
use e32::sys::{TVersion, err};
use neoui_proto::clock::wire::Corrupt;
use neoui_proto::clock::{tz, world};
use neoui_proto::ipc::AppEvent;
use neoui_proto::page::{
    NOTIFY_ONGOING, NOTIFY_SILENT_UPDATE, Notification, action as kind, style,
};
use neoui_proto::settings::Value;

/// Days from Symbian's `TTime` zero to 1970-01-01.
const TTIME_UNIX_DAYS: i64 = 719_540;
const TIMER_RING: u32 = ring::id(ring::TIMER, 0);
/// How far ahead the reminders are planned.
const REMINDERS_AHEAD: i64 = 2 * DAY;
/// The widest EVENT_QUERY (days).
const QUERY_DAYS: i32 = 1000;
/// The most calendars: EVENT_QUERY's hidden mask has 32 bits.
const CALENDARS_MAX: usize = 32;
const KERR_TOO_BIG: i32 = -40;
const KERR_NOT_READY: i32 = -18;
/// How often a store that couldn't be read at start is tried again (ms).
const RETRY_MS: i64 = 10_000;

/// UTC now in Unix ms, and the kernel's UTC offset in seconds (the shell sets it for the zone and summer time).
fn clock() -> (i64, i32) {
    let (us, offset) = e32::sys::time_now();
    (us.div_euclid(1000) - TTIME_UNIX_DAYS * 86_400_000, offset)
}

/// One client session: the changes it hasn't heard of, and its outstanding WATCH (mask, message).
#[derive(Default)]
pub struct Watcher {
    pending: u32,
    waiting: Option<(u32, Message)>,
}

struct Settings {
    snooze_min: u16,
    snooze_count: u8,
    h24: bool,
    /// `calendar.ring`: a reminder opens Calendar's Ringing page (else its card only)
    ring_reminders: bool,
    /// `calendar.birthdays`
    birthdays: bool,
    /// the `time.zone` city's rule while `time.auto_zone` is off
    city: Option<tz::Rule>,
}

pub struct Dt {
    ui: Ui,
    alarms: Alarms,
    timer: Timer,
    /// what rang, the rings going on and the alarms Snooze made (`rings.dat`)
    rung: Rung,
    /// the stores not read yet (mask bits; the calendar is EVENTS | TODOS, `rings.dat` RINGS): their writes wait
    pending: u32,
    /// when to try them again (UTC ms)
    retry_at: i64,
    settings: Settings,
    /// the last plan's now: things due after it ring
    since: At,
    /// the `dtsrv.next` last written
    published: Vec<u8>,
    /// the running timer's card text as last posted ("" = no card)
    chrono: String,
    watchers: Vec<Rc<RefCell<Watcher>>>,
    events: Store,
    birthdays: Vec<Birthday>,
    /// `contacts.rev` when the birthdays were read (`None`: not yet)
    book_rev: Option<Option<i32>>,
    /// the zone the calendar was last read in
    zone: Local,
    /// the reminders due from the last refresh to `REMINDERS_AHEAD` on: (utc, ring id)
    reminders: Vec<(i64, u32)>,
    /// snoozed reminders: (utc, ring id)
    snoozed_reminders: Vec<(i64, u32)>,
    /// the next event: (its start utc, it)
    next_event: Option<(i64, Instance)>,
    /// `reminders` and `next_event` hold until then (utc; `i64::MIN`: a change made them stale)
    cal_until: i64,
}

impl Dt {
    pub fn start(ui: Ui) -> Self {
        // a restart's cards: their rings are gone
        let _ = ui.clear_cards();
        for prefix in ["time.", "clock.", "calendar.", "contacts.rev"] {
            if let Err(e) = ui.watch(prefix) {
                dprintln!("dtsrv: settings watch {} refused ({})", prefix, e);
            }
        }
        let (now_ms, offset) = clock();
        let mut pending = 0;
        let alarms = ready(load_alarms(&ui), mask::ALARMS, &mut pending);
        let timer = ready(load_timer(&ui), mask::TIMERS, &mut pending);
        let rung = ready(load_rung(), mask::RINGS, &mut pending);
        // the seed only keeps new uids apart across restarts
        let events = cal::load(now_ms as u64).unwrap_or_else(|e| {
            dprintln!("dtsrv: the calendar not read ({}): tried again later", e);
            pending |= mask::EVENTS | mask::TODOS;
            Store::load("", "")
        });
        dprintln!(
            "dtsrv: start: {} alarms, timer {:?}, {} rings, unread {:#x}",
            alarms.as_slice().len(),
            timer.phase,
            rung.ringing.len(),
            pending
        );
        let mut d = Self {
            ui,
            alarms,
            timer,
            rung,
            pending,
            retry_at: now_ms + RETRY_MS,
            settings: Settings {
                snooze_min: 5,
                snooze_count: 3,
                h24: true,
                ring_reminders: true,
                birthdays: true,
                city: None,
            },
            since: At {
                ms: boot_since(now_ms),
                offset,
            },
            published: Vec::new(),
            chrono: String::new(),
            watchers: Vec::new(),
            events,
            birthdays: Vec::new(),
            book_rev: None,
            zone: Local::Kernel(offset),
            reminders: Vec::new(),
            snoozed_reminders: Vec::new(),
            next_event: None,
            cal_until: i64::MIN,
        };
        d.read_settings();
        d.repost(now_ms);
        d
    }

    /// The rings a restart cut off (and a timer that ended unacknowledged) ring again; a reminder's is forgotten.
    fn repost(&mut self, now_ms: i64) {
        let old = core::mem::take(&mut self.rung.ringing);
        let gone = old.len();
        for r in old {
            if matches!(ring::kind(r), ring::ALARM | ring::TIMER) {
                self.fire(r, now_ms);
            }
        }
        if self.timer.phase == Phase::Done && !self.rung.ringing.contains(&TIMER_RING) {
            self.fire(TIMER_RING, now_ms);
        }
        if gone != self.rung.ringing.len() {
            let _ = self.save_rung();
        }
    }

    /// Reads the stores the start couldn't again.
    fn retry(&mut self, now_ms: i64) {
        self.retry_at = now_ms + RETRY_MS;
        let mut got = 0;
        if self.pending & mask::ALARMS != 0
            && let Ok(a) = load_alarms(&self.ui)
        {
            self.alarms = a;
            got |= mask::ALARMS;
        }
        if self.pending & mask::TIMERS != 0
            && let Ok(t) = load_timer(&self.ui)
        {
            self.timer = t;
            got |= mask::TIMERS;
        }
        if self.pending & mask::RINGS != 0
            && let Ok(r) = load_rung()
        {
            // what rang since the start stays
            for (id, m) in r.last {
                self.rung.mark(id, m);
            }
            for s in r.snoozed {
                if !self.rung.snoozed.contains(&s) {
                    self.rung.snoozed.push(s);
                }
            }
            got |= mask::RINGS;
        }
        if self.pending & mask::EVENTS != 0
            && let Ok(s) = cal::load(self.events.seed)
        {
            self.events = s;
            self.cal_until = i64::MIN;
            got |= mask::EVENTS | mask::TODOS;
        }
        if got != 0 {
            self.pending &= !got;
            dprintln!("dtsrv: read late {:#x}, unread {:#x}", got, self.pending);
            self.changed(got & !mask::RINGS);
        }
    }

    pub fn ui_session(&self) -> Rc<e32::ipc::Session> {
        self.ui.session()
    }

    /// Plans, rings what is due, refreshes the timer's card and `dtsrv.next`; returns how long to sleep (µs).
    pub fn tick(&mut self) -> i32 {
        let (now_ms, offset) = clock();
        if self.pending != 0 && now_ms >= self.retry_at {
            self.retry(now_ms);
        }
        let now = now_ms.div_euclid(1000);
        self.refresh_calendar(now, offset);
        let mut extra = self.reminders.clone();
        extra.extend_from_slice(&self.snoozed_reminders);
        let at = At { ms: now_ms, offset };
        let p = plan::plan(
            &self.alarms,
            &self.timer,
            &self.rung,
            self.since,
            at,
            &extra,
        );
        self.since = at;
        // marked first: an occurrence rings once even while its alarm still rings from the last one
        for &(id, m) in &p.occurrences {
            self.rung.mark(id, m);
        }
        for r in p.due {
            if !self.rung.ringing.contains(&r) {
                self.fire(r, now_ms);
            }
        }
        if !p.occurrences.is_empty() {
            let _ = self.save_rung();
        }
        self.refresh_chrono(now_ms);
        self.publish(now, offset);
        // shortcut: sleeps at most 60 s, so a clock change other than a time.* setting is seen up to a minute late;
        // upgrade to an e32 TIMER_AT (0x36) wrapper, which the kernel completes when the clock is set.
        let most = if self.pending != 0 { RETRY_MS } else { 60_000 };
        let ms = p
            .next
            .map_or(most, |(at, _)| at.saturating_sub(now_ms).clamp(1, most));
        // at most 60 s in µs
        (ms * 1000) as i32
    }

    /// An event from the UI server; false for Quit.
    pub fn event(&mut self, e: AppEvent) -> bool {
        match e {
            AppEvent::Notified { id, action } => self.card(id, action),
            AppEvent::Settings { .. } => self.read_settings(),
            AppEvent::Quit => return false,
            _ => {}
        }
        true
    }

    fn read_settings(&mut self) {
        let int = |name, default| match self.ui.get(name) {
            Ok(Value::Int(v)) => v,
            _ => default,
        };
        let on = |name| !matches!(self.ui.get(name), Ok(Value::Bool(false)));
        let city = match self.ui.get("time.zone") {
            Ok(Value::Str(name)) if !on("time.auto_zone") => {
                world::find(&name).and_then(|c| tz::Rule::parse(c.tz))
            }
            _ => None,
        };
        let settings = Settings {
            // both clamped into their types
            snooze_min: int("clock.snooze", 5).clamp(1, 60) as u16,
            snooze_count: int("clock.snooze_count", 3).clamp(0, 10) as u8,
            h24: on("clock.24h"),
            ring_reminders: on("calendar.ring"),
            birthdays: on("calendar.birthdays"),
            city,
        };
        let rev = match self.ui.get("contacts.rev") {
            Ok(Value::Int(n)) => Some(n),
            _ => None,
        };
        if settings.birthdays != self.settings.birthdays {
            self.changed(mask::EVENTS);
        }
        self.settings = settings;
        // NEOCONT saved the book
        if self.book_rev != Some(rev) {
            self.book_rev = Some(rev);
            self.birthdays = cal::birthdays();
            self.changed(mask::EVENTS);
        }
    }

    /// The zone the calendar is read in now.
    fn local(&self, offset: i32) -> Local {
        self.settings
            .city
            .map_or(Local::Kernel(offset), Local::City)
    }

    /// Plans the reminders and finds the next event again when the stores, the zone or the day changed.
    fn refresh_calendar(&mut self, now: i64, offset: i32) {
        let zone = self.local(offset);
        if zone != self.zone {
            self.zone = zone;
            self.cal_until = i64::MIN;
            // local times moved
            self.changed(mask::EVENTS | mask::TODOS);
        }
        if now < self.cal_until {
            return;
        }
        // from the last plan on, so one due since then still rings; at most a day back (a jump forward)
        let from = (self.since.ms.div_euclid(1000) + 1).min(now).max(now - DAY);
        self.reminders = self
            .events
            .reminders(from, now + REMINDERS_AHEAD, &zone)
            .into_iter()
            .map(|(at, id)| (at, ring::id(ring::REMINDER, id.0)))
            .collect();
        self.next_event = self
            .events
            .next_event(now, &zone)
            .map(|i| (pim_ics::to_utc(i.start, &zone), i));
        self.cal_until = self
            .next_event
            .as_ref()
            .map_or(now + DAY, |(at, _)| (*at + 1).clamp(now + 1, now + DAY));
    }

    /// The events changed: re-plan, tell the watchers, save.
    fn events_changed(&mut self, bits: u32) -> Result<(), i32> {
        self.cal_until = i64::MIN;
        self.changed(bits);
        if self.pending & bits != 0 {
            return Err(KERR_NOT_READY);
        }
        cal::save(
            &self.events,
            bits & mask::EVENTS != 0,
            bits & mask::TODOS != 0,
        )
    }

    fn fire(&mut self, r: u32, now_ms: i64) {
        if ring::kind(r) == ring::REMINDER {
            return self.fire_reminder(r);
        }
        let (title, text, actions) = match ring::kind(r) {
            ring::ALARM => {
                // the planner's ids come from the alarms
                let Some(a) = self.alarms.get(ring::item(r) as u16) else {
                    return;
                };
                let title = if a.label.is_empty() {
                    "Alarm".to_string()
                } else {
                    a.label.clone()
                };
                (title, a.time_text(self.settings.h24), ["Snooze", "Dismiss"])
            }
            ring::TIMER => {
                if self.timer.tick(now_ms) {
                    let _ = self.save_timer();
                }
                // the done card replaces the running timer's (same id)
                self.chrono.clear();
                let text = format!("Timer · {}", format_left(self.timer.set_ms));
                ("Time's up".to_string(), text, ["+1 min", "Stop"])
            }
            _ => return,
        };
        dprintln!("dtsrv: fire {:#x} {} {}", r, title, text);
        // saved before it shows, so a restart in between rings it again
        self.rung.ringing.push(r);
        let _ = self.save_rung();
        self.open("clock", pages::clock::RING, &r.to_string());
        let n = Notification {
            id: r,
            title,
            text,
            page: pages::clock::RING,
            actions: actions.map(String::from).to_vec(),
            action_kinds: vec![kind::IN_PLACE; 2],
            flags: NOTIFY_ONGOING,
            ..Notification::default()
        };
        self.post(&n);
        self.changed(mask::RINGS | if r == TIMER_RING { mask::TIMERS } else { 0 });
    }

    /// Snooze, Dismiss, Stop or +1 min on a ring (RING_ACTION, or its card's buttons). The store's change is saved
    /// first: when that fails, or a snooze finds the alarm list full, the ring goes on and the error is returned.
    fn act(&mut self, r: u32, what: i32) -> Result<(), i32> {
        if !self.rung.ringing.contains(&r) {
            return Err(err::KERR_NOT_FOUND);
        }
        let (now_ms, offset) = clock();
        let again = matches!(what, action::SNOOZE | action::PLUS_MINUTE);
        match ring::kind(r) {
            ring::ALARM => {
                // the planner's ids come from the alarms
                let id = ring::item(r) as u16;
                let copy = self.rung.snoozed.iter().position(|s| s.0 == id);
                let left = copy.map_or(self.settings.snooze_count, |i| self.rung.snoozed[i].1);
                let now = minute_of_week(now_ms.div_euclid(1000), offset);
                let mut staged = self.alarms.clone();
                let new = if again && left > 0 {
                    let new = staged
                        .snooze_for(id, now, self.settings.snooze_min)
                        .map_err(|_| {
                            dprintln!("dtsrv: action {:#x} snooze: the list is full", r);
                            err::KERR_OVERFLOW
                        })?;
                    Some(new)
                } else {
                    staged.dismissed(id);
                    None
                };
                if copy.is_some() {
                    staged.remove(id);
                }
                self.write(mask::ALARMS, store::ALARMS, &staged.encode())?;
                self.alarms = staged;
                if copy.is_some() {
                    self.rung.forget(id);
                }
                match new {
                    Some(new) => {
                        dprintln!(
                            "dtsrv: action {:#x} snooze -> alarm {} ({} left)",
                            r,
                            new,
                            left - 1
                        );
                        self.rung.snoozed.push((new, left - 1));
                    }
                    None => dprintln!("dtsrv: action {:#x} dismiss ({} snoozes left)", r, left),
                }
                self.changed(mask::ALARMS);
            }
            ring::TIMER => {
                let mut t = self.timer;
                if again {
                    t.add(60_000, now_ms);
                } else {
                    t.reset();
                }
                self.write(mask::TIMERS, store::TIMERS, &t.encode())?;
                self.timer = t;
                let done = if again { "+1 min" } else { "stop" };
                dprintln!("dtsrv: action {:#x} {}", r, done);
                self.changed(mask::TIMERS);
            }
            ring::REMINDER => {
                if again {
                    let at = now_ms.div_euclid(1000) + i64::from(self.settings.snooze_min) * 60;
                    self.snoozed_reminders.push((at, r));
                    dprintln!("dtsrv: action {:#x} snooze -> {}", r, at);
                } else {
                    dprintln!("dtsrv: action {:#x} dismiss", r);
                }
            }
            _ => {}
        }
        self.rung.ringing.retain(|&x| x != r);
        self.remove_card(r);
        self.changed(mask::RINGS);
        self.save_rung()
    }

    /// A tap on one of DTSRV's cards (action 0) or on its button `action - 1`.
    fn card(&mut self, id: u32, action: u16) {
        if ring::kind(id) == ring::REMINDER {
            let what = match action {
                0 => return self.open_event(id),
                1 => action::SNOOZE,
                _ => action::DISMISS,
            };
            let _ = self.act(id, what);
            return;
        }
        let timer = ring::kind(id) == ring::TIMER;
        if self.rung.ringing.contains(&id) {
            let what = match (timer, action) {
                (_, 0) => return self.open("clock", pages::clock::RING, &id.to_string()),
                (true, 1) => action::PLUS_MINUTE,
                (true, _) => action::STOP,
                (false, 1) => action::SNOOZE,
                (false, _) => action::DISMISS,
            };
            let _ = self.act(id, what);
        } else if timer && action == 0 {
            self.open("clock", pages::clock::TIMER, "");
        } else if timer && action == 1 {
            // the running timer's Pause
            self.timer.pause(clock().0);
            dprintln!("dtsrv: action {:#x} pause", id);
            let _ = self.save_timer();
            self.changed(mask::TIMERS);
        }
    }

    /// The running timer's card: "8 min left", Pause; the shade counts down to its end itself (CHRONO).
    fn refresh_chrono(&mut self, now_ms: i64) {
        let text = match self.timer.phase {
            Phase::Running { ends_ms } => {
                let left = ends_ms.saturating_sub(now_ms).max(1);
                format!("{} min left", left.saturating_add(59_999) / 60_000)
            }
            _ => String::new(),
        };
        if text == self.chrono {
            return;
        }
        if let Phase::Running { ends_ms } = self.timer.phase {
            let n = Notification {
                id: TIMER_RING,
                title: "Timer".into(),
                text: text.clone(),
                page: pages::clock::TIMER,
                actions: vec!["Pause".into()],
                action_kinds: vec![kind::IN_PLACE],
                flags: NOTIFY_ONGOING | NOTIFY_SILENT_UPDATE,
                style: style::CHRONO,
                // after the clamp it is positive
                when: ends_ms.max(0) as u64,
                ..Notification::default()
            };
            self.post(&n);
        } else if !self.rung.ringing.contains(&TIMER_RING) {
            self.remove_card(TIMER_RING);
        }
        self.chrono = text;
    }

    /// Writes `dtsrv.next` when it changed.
    fn publish(&mut self, now: i64, offset: i32) {
        let b = self.next(now, offset);
        if b == self.published {
            return;
        }
        if let Err(e) = self.ui.set("dtsrv.next", &Value::Bytes(b.clone())) {
            dprintln!("dtsrv: dtsrv.next not written ({})", e);
        }
        // a refused write (an older shell) isn't retried on every pass
        self.published = b;
    }

    /// The `dtsrv.next` blob: the next alarm and the next event.
    fn next(&self, now: i64, offset: i32) -> Vec<u8> {
        let mut next = Next::default();
        if let Some((at, a)) = plan::next_alarm(&self.alarms, &self.rung, now, offset) {
            next.alarm_at = at;
            next.alarm_label = a.label.clone();
        }
        if let Some((at, e)) = &self.next_event {
            next.event_at = *at;
            next.event_title.clone_from(&e.title);
            next.event_colour = e.colour;
            next.event_all_day = e.all_day;
        }
        next.encode()
    }

    /// Opens app `key` on `page`, waking the screen and closing the shade.
    fn open(&self, key: &str, page: u16, arg: &str) {
        if let Err(e) = self.ui.open(key, page, arg, OPEN_WAKE) {
            dprintln!("dtsrv: OPEN {} {} {} refused ({})", key, page, arg, e);
        }
    }

    /// A reminder's card was tapped: its event (or the to-do list).
    fn open_event(&self, r: u32) {
        match self.ring_info(r) {
            Some(i) if i.todo => self.open("calendar", pages::calendar::TODO, ""),
            Some(i) => self.open("calendar", pages::calendar::EVENT, &i.uid),
            None => self.open("calendar", pages::calendar::DAY, ""),
        }
    }

    /// What reminder ring `r` is about: the occurrence (or to-do) its id stands for.
    fn ring_info(&self, r: u32) -> Option<RingInfo> {
        let (uid, start) = self.events.reminder(ReminderId(ring::item(r)))?;
        let uid = String::from(uid);
        let zone = &self.zone;
        let local = pim_ics::to_wall(start, zone);
        let day = local.div_euclid(DAY);
        let ringing = self.rung.ringing.contains(&r);
        if let Some(i) = self
            .events
            .query(day, day, 0, zone, &[])
            .into_iter()
            .find(|i| i.uid == uid && i.start == local)
        {
            return Some(RingInfo {
                kind: ring::REMINDER,
                todo: false,
                all_day: i.all_day,
                ringing,
                start,
                end: pim_ics::to_utc(i.end, zone),
                colour: i.colour,
                uid,
                title: i.title,
                place: i.place,
            });
        }
        let t = self.events.todos(zone).into_iter().find(|t| t.uid == uid)?;
        Some(RingInfo {
            kind: ring::REMINDER,
            todo: true,
            all_day: t.due_all_day,
            ringing,
            start,
            end: start,
            colour: 0,
            uid,
            title: t.title,
            place: String::new(),
        })
    }

    /// A reminder is due: Calendar's Ringing page (`calendar.ring`) and always its card, Snooze and Dismiss.
    fn fire_reminder(&mut self, r: u32) {
        self.snoozed_reminders.retain(|s| s.1 != r);
        let Some(info) = self.ring_info(r) else {
            dprintln!("dtsrv: reminder {} is gone", ring::item(r));
            return;
        };
        dprintln!("dtsrv: reminder {} {}", ring::item(r), info.uid);
        if self.settings.ring_reminders {
            self.open("calendar", pages::calendar::RING, &r.to_string());
        }
        let local = pim_ics::to_wall(info.start, &self.zone).rem_euclid(DAY);
        // the hour and minute of a day
        let (h, m) = ((local / 3600) as u8, (local / 60 % 60) as u8);
        let when = match (info.all_day, info.todo) {
            (true, false) => String::from("All day"),
            (true, true) => String::from("Due today"),
            (false, false) => format_hm(h, m, self.settings.h24),
            (false, true) => format!("Due {}", format_hm(h, m, self.settings.h24)),
        };
        let text = if info.place.is_empty() {
            when
        } else {
            format!("{} · {}", when, info.place)
        };
        let title = if info.title.is_empty() {
            String::from("Reminder")
        } else {
            info.title
        };
        let n = Notification {
            id: r,
            title,
            text,
            page: pages::calendar::EVENT,
            actions: ["Snooze", "Dismiss"].map(String::from).to_vec(),
            action_kinds: vec![kind::IN_PLACE; 2],
            flags: NOTIFY_ONGOING,
            ..Notification::default()
        };
        self.post(&n);
        self.rung.ringing.push(r);
        self.changed(mask::RINGS);
    }

    fn post(&self, n: &Notification) {
        if let Err(e) = self.ui.notify(n) {
            dprintln!("dtsrv: card {:#x} not posted ({})", n.id, e);
        }
    }

    fn remove_card(&self, id: u32) {
        if let Err(e) = self.ui.remove_card(id) {
            dprintln!("dtsrv: card {:#x} not removed ({})", id, e);
        }
    }

    /// Ends the rings of `kind` without acting on them (their alarm went, Clear data).
    fn stop_rings(&mut self, kind: u8) {
        let gone: Vec<u32> = self
            .rung
            .ringing
            .iter()
            .copied()
            .filter(|&r| ring::kind(r) == kind)
            .collect();
        self.rung.ringing.retain(|&r| ring::kind(r) != kind);
        for &r in &gone {
            self.remove_card(r);
        }
        if !gone.is_empty() {
            let _ = self.save_rung();
            self.changed(mask::RINGS);
        }
    }

    /// Writes store `bit`'s file; `KErrNotReady` while it hasn't been read (so the file on disk isn't lost).
    fn write(&self, bit: u32, path: &str, bytes: &[u8]) -> Result<(), i32> {
        if self.pending & bit != 0 {
            return Err(KERR_NOT_READY);
        }
        store::write(path, bytes).inspect_err(|e| dprintln!("dtsrv: {} not saved ({})", path, e))
    }

    fn save_alarms(&self) -> Result<(), i32> {
        self.write(mask::ALARMS, store::ALARMS, &self.alarms.encode())
    }

    fn save_timer(&self) -> Result<(), i32> {
        self.write(mask::TIMERS, store::TIMERS, &self.timer.encode())
    }

    fn save_rung(&self) -> Result<(), i32> {
        self.write(mask::RINGS, store::RINGS, &self.rung.encode())
    }

    /// Tells the sessions watching `bits`; the others keep them for their next WATCH.
    fn changed(&mut self, bits: u32) {
        for w in &self.watchers {
            let mut w = w.borrow_mut();
            w.pending |= bits;
            let hit = w.waiting.as_ref().map_or(0, |(m, _)| m & w.pending);
            if hit != 0 {
                w.pending &= !hit;
                if let Some((_, msg)) = w.waiting.take() {
                    // the mask bits are small
                    msg.complete(hit as i32);
                }
            }
        }
    }

    fn put_alarm(&mut self, msg: &Message) -> Result<i32, i32> {
        let rec = msg.read8_vec(0)?;
        let a = ipc::read_alarm_record(&rec).map_err(|_| err::KERR_ARGUMENT)?;
        let mut staged = self.alarms.clone();
        let id = if a.id == 0 {
            let id = staged
                .add(a.hour, a.minute, a.days, &a.label)
                .map_err(|_| err::KERR_OVERFLOW)?;
            staged.set_on(id, a.on);
            id
        } else if staged.update(a.clone()) {
            a.id
        } else {
            return Err(err::KERR_NOT_FOUND);
        };
        // saved before it's in force: a failed save changes nothing, and a retry doesn't make two
        self.write(mask::ALARMS, store::ALARMS, &staged.encode())?;
        self.alarms = staged;
        self.changed(mask::ALARMS);
        Ok(i32::from(id))
    }

    fn delete_alarm(&mut self, id: i32) -> Result<i32, i32> {
        let id = u16::try_from(id).map_err(|_| err::KERR_NOT_FOUND)?;
        let mut staged = self.alarms.clone();
        if !staged.remove(id) {
            return Err(err::KERR_NOT_FOUND);
        }
        self.write(mask::ALARMS, store::ALARMS, &staged.encode())?;
        self.alarms = staged;
        let r = ring::id(ring::ALARM, u32::from(id));
        if self.rung.ringing.contains(&r) {
            self.remove_card(r);
            self.changed(mask::RINGS);
        }
        self.rung.forget(id);
        let _ = self.save_rung();
        self.changed(mask::ALARMS);
        Ok(0)
    }

    fn timer_op(&mut self, msg: &Message) -> Result<i32, i32> {
        let (which, value) = (msg.int(0), msg.int(1));
        let now_ms = clock().0;
        let mut staged = self.timer;
        let t = &mut staged;
        match which {
            ipc::timer::GET => {}
            ipc::timer::START => {
                if value > 0 || t.phase == Phase::Done {
                    t.reset();
                }
                if value > 0 {
                    t.set(i64::from(value));
                }
                t.start(now_ms);
            }
            ipc::timer::PAUSE => t.pause(now_ms),
            ipc::timer::RESUME if matches!(t.phase, Phase::Paused { .. }) => t.start(now_ms),
            ipc::timer::RESUME => {}
            ipc::timer::RESET => t.reset(),
            ipc::timer::ADD => t.add(i64::from(value) * 1000, now_ms),
            _ => return Err(err::KERR_ARGUMENT),
        }
        if which != ipc::timer::GET {
            self.write(mask::TIMERS, store::TIMERS, &staged.encode())?;
            self.timer = staged;
            dprintln!(
                "dtsrv: timer op {} {} -> {:?}",
                which,
                value,
                self.timer.phase
            );
            if self.timer.phase != Phase::Done {
                self.stop_rings(ring::TIMER);
            }
            self.changed(mask::TIMERS);
        }
        reply(msg, 2, &self.timer.encode())
    }

    fn clear(&mut self, which: u32) -> Result<i32, i32> {
        dprintln!("dtsrv: clear {:#x}", which);
        let mut saved = Ok(0);
        if which & mask::ALARMS != 0 {
            self.alarms = Alarms::new();
            self.rung.snoozed.clear();
            self.rung.last.clear();
            self.stop_rings(ring::ALARM);
            saved = saved.and(self.save_alarms().map(|()| 0));
        }
        if which & mask::TIMERS != 0 {
            self.timer = Timer::default();
            self.stop_rings(ring::TIMER);
            saved = saved.and(self.save_timer().map(|()| 0));
        }
        let (events, todos) = (which & mask::EVENTS != 0, which & mask::TODOS != 0);
        if events || todos {
            self.events = cal::cleared(&self.events, events, todos);
            self.snoozed_reminders.clear();
            self.stop_rings(ring::REMINDER);
            saved = saved.and(
                self.events_changed(which & (mask::EVENTS | mask::TODOS))
                    .map(|()| 0),
            );
        }
        self.changed(which & (mask::ALARMS | mask::TIMERS));
        saved
    }

    // --- the calendar's requests

    fn birthdays(&self) -> &[Birthday] {
        if self.settings.birthdays {
            &self.birthdays
        } else {
            &[]
        }
    }

    fn event_query(&self, msg: &Message) -> Result<i32, i32> {
        let (from, to) = (msg.int(0), msg.int(1));
        if to < from || to - from >= QUERY_DAYS {
            return Err(err::KERR_ARGUMENT);
        }
        // the mask is bits
        let hidden = msg.int(2) as u32;
        let v = self.events.query(
            i64::from(from),
            i64::from(to),
            hidden,
            &self.zone,
            self.birthdays(),
        );
        reply(msg, 3, &wire::encode_instances(&v))
    }

    fn event_put(&mut self, msg: &Message) -> Result<i32, i32> {
        let d = EventDraft::decode(&msg.read8_vec(0)?).map_err(|_| err::KERR_ARGUMENT)?;
        let uid = self
            .events
            .put(&d, &self.zone, clock().0.div_euclid(1000))
            .ok_or(err::KERR_NOT_FOUND)?;
        dprintln!("dtsrv: event put {}", uid);
        self.events_changed(mask::EVENTS)?;
        reply(msg, 1, uid.as_bytes())
    }

    fn event_del(&mut self, msg: &Message) -> Result<i32, i32> {
        let (uid, only) =
            ipc::read_event_key(&msg.read8_vec(0)?).map_err(|_| err::KERR_ARGUMENT)?;
        if !self.events.del(&uid, only, &self.zone) {
            return Err(err::KERR_NOT_FOUND);
        }
        dprintln!("dtsrv: event del {} {:?}", uid, only);
        self.events_changed(mask::EVENTS).map(|()| 0)
    }

    fn event_get(&self, msg: &Message) -> Result<i32, i32> {
        let (uid, only) =
            ipc::read_event_key(&msg.read8_vec(0)?).map_err(|_| err::KERR_ARGUMENT)?;
        let d = self
            .events
            .get(&uid, only, &self.zone)
            .ok_or(err::KERR_NOT_FOUND)?;
        reply(msg, 1, &d.encode())
    }

    fn todo_put(&mut self, msg: &Message) -> Result<i32, i32> {
        let t = Todo::decode(&msg.read8_vec(0)?).map_err(|_| err::KERR_ARGUMENT)?;
        let uid = self
            .events
            .put_todo(&t, &self.zone, clock().0.div_euclid(1000))
            .ok_or(err::KERR_NOT_FOUND)?;
        self.events_changed(mask::TODOS)?;
        reply(msg, 1, uid.as_bytes())
    }

    fn todo_del(&mut self, msg: &Message) -> Result<i32, i32> {
        let uid = String::from_utf8(msg.read8_vec(0)?).map_err(|_| err::KERR_ARGUMENT)?;
        if !self.events.del_todo(&uid) {
            return Err(err::KERR_NOT_FOUND);
        }
        self.events_changed(mask::TODOS).map(|()| 0)
    }

    fn import(&mut self, msg: &Message) -> Result<i32, i32> {
        if msg.des_len(0)? > ipc::IMPORT_MAX {
            return Err(KERR_TOO_BIG);
        }
        let b = msg.read8_vec(0)?;
        let text = String::from_utf8_lossy(&b);
        let cal = usize::try_from(msg.int(1)).unwrap_or(0);
        let (added, clashes) = self.events.import(&text, cal, clock().0.div_euclid(1000));
        dprintln!("dtsrv: import {} {}", added, clashes.len());
        if added > 0 {
            self.events_changed(mask::EVENTS | mask::TODOS)?;
        }
        Ok(ipc::import_reply(added, clashes.len()))
    }

    fn export(&self, msg: &Message) -> Result<i32, i32> {
        let uid = String::from_utf8(msg.read8_vec(0)?).map_err(|_| err::KERR_ARGUMENT)?;
        let uid = (!uid.is_empty()).then_some(uid.as_str());
        if let Some(u) = uid {
            let known = self.events.get(u, None, &self.zone).is_some()
                || self.events.todos(&self.zone).iter().any(|t| t.uid == u);
            if !known {
                return Err(err::KERR_NOT_FOUND);
            }
        }
        reply(msg, 1, self.events.export(uid).as_bytes())
    }

    fn calendars(&self, msg: &Message) -> Result<i32, i32> {
        let hidden = match self.ui.get("calendar.hidden") {
            Ok(Value::Bytes(b)) => b,
            _ => Vec::new(),
        };
        let v: Vec<CalendarInfo> = self
            .events
            .calendars()
            .iter()
            .enumerate()
            .map(|(i, c)| CalendarInfo {
                name: c.name.clone(),
                colour: c.colour,
                // at most CALENDARS_MAX
                hidden: hidden.contains(&(i as u8)),
                events: u16::try_from(self.events.count(i)).unwrap_or(u16::MAX),
            })
            .collect();
        reply(msg, 0, &ipc::encode_calendars(&v))
    }

    fn calendar_put(&mut self, msg: &Message) -> Result<i32, i32> {
        // the colour is 0xRRGGBB
        let colour = msg.int(1) as u32 & 0x00FF_FFFF;
        let name = String::from_utf8(msg.read8_vec(2)?).map_err(|_| err::KERR_ARGUMENT)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(err::KERR_ARGUMENT);
        }
        let n = self.events.calendars().len();
        let i = match msg.int(0) {
            -1 if n >= CALENDARS_MAX => return Err(err::KERR_OVERFLOW),
            -1 => self.events.add_calendar(name, colour),
            i => {
                let i = usize::try_from(i)
                    .ok()
                    .filter(|&i| i < n)
                    .ok_or(err::KERR_NOT_FOUND)?;
                self.events.set_calendar(i, name, colour);
                i
            }
        };
        self.events_changed(mask::EVENTS)?;
        // at most CALENDARS_MAX
        Ok(i as i32)
    }

    fn ring_info_op(&self, msg: &Message) -> Result<i32, i32> {
        // the ring id travels as the word it is
        let r = msg.int(0) as u32;
        if ring::kind(r) != ring::REMINDER {
            return Err(err::KERR_NOT_SUPPORTED);
        }
        let info = self.ring_info(r).ok_or(err::KERR_NOT_FOUND)?;
        reply(msg, 1, &info.encode())
    }
}

impl Service for Dt {
    type Session = Rc<RefCell<Watcher>>;

    fn connect(&mut self, version: TVersion) -> Result<Self::Session, i32> {
        if version.major != ipc::VERSION.0 {
            return Err(err::KERR_NOT_SUPPORTED);
        }
        let w = Rc::new(RefCell::new(Watcher::default()));
        self.watchers.push(w.clone());
        Ok(w)
    }

    fn message(&mut self, session: &mut Self::Session, msg: Message) {
        // a change to a store not read yet would overwrite its file
        let writes = match msg.function() {
            op::ALARM_PUT | op::ALARM_DEL => mask::ALARMS,
            op::TIMER if msg.int(0) != ipc::timer::GET => mask::TIMERS,
            op::EVENT_PUT
            | op::EVENT_DEL
            | op::TODO_PUT
            | op::TODO_DEL
            | op::IMPORT_ICS
            | op::CALENDAR_PUT => mask::EVENTS,
            // the mask is bits
            op::CLEAR => msg.int(0) as u32 & !mask::RINGS,
            _ => 0,
        };
        if self.pending & writes != 0 {
            msg.complete(KERR_NOT_READY);
            return;
        }
        let r = match msg.function() {
            op::NEXT => {
                let (now_ms, offset) = clock();
                reply(&msg, 0, &self.next(now_ms.div_euclid(1000), offset))
            }
            op::WATCH => {
                let mut w = session.borrow_mut();
                // the mask is bits
                let m = msg.int(0) as u32;
                let hit = w.pending & m;
                if hit != 0 {
                    w.pending &= !hit;
                    msg.complete(hit as i32);
                } else if let Some((_, old)) = w.waiting.replace((m, msg)) {
                    old.complete(err::KERR_CANCEL);
                }
                return;
            }
            op::WATCH_CANCEL => {
                if let Some((_, m)) = session.borrow_mut().waiting.take() {
                    m.complete(err::KERR_CANCEL);
                }
                Ok(0)
            }
            op::ALARM_LIST => reply(&msg, 0, &self.alarms.encode()),
            op::ALARM_PUT => self.put_alarm(&msg),
            op::ALARM_DEL => self.delete_alarm(msg.int(0)),
            op::TIMER => self.timer_op(&msg),
            // the ring id travels as the word it is
            op::RING_ACTION => self.act(msg.int(0) as u32, msg.int(1)).map(|()| 0),
            op::CLEAR => self.clear(msg.int(0) as u32),
            op::EVENT_QUERY => self.event_query(&msg),
            op::EVENT_PUT => self.event_put(&msg),
            op::EVENT_DEL => self.event_del(&msg),
            op::EVENT_GET => self.event_get(&msg),
            op::TODO_LIST => reply(&msg, 0, &wire::encode_todos(&self.events.todos(&self.zone))),
            op::TODO_PUT => self.todo_put(&msg),
            op::TODO_DEL => self.todo_del(&msg),
            op::IMPORT_ICS => self.import(&msg),
            op::EXPORT_ICS => self.export(&msg),
            op::CALENDARS => self.calendars(&msg),
            op::CALENDAR_PUT => self.calendar_put(&msg),
            op::RING_INFO => self.ring_info_op(&msg),
            _ => Err(err::KERR_NOT_SUPPORTED),
        };
        msg.complete(r.unwrap_or_else(|e| e));
    }

    fn disconnect(&mut self, session: Self::Session) -> ControlFlow<()> {
        self.watchers.retain(|w| !Rc::ptr_eq(w, &session));
        if let Some((_, m)) = session.borrow_mut().waiting.take() {
            m.complete(err::KERR_CANCEL);
        }
        ControlFlow::Continue(())
    }
}

/// Writes `data` into the client's descriptor `slot` when it fits; the reply is its length either way, so a client
/// with too small a buffer asks again with that much room (`datetime_core::ipc`).
fn reply(msg: &Message, slot: usize, data: &[u8]) -> Result<i32, i32> {
    if data.len() <= msg.des_max_len(slot)? {
        msg.write8(slot, data, 0)?;
    }
    // the stores are far below 2 GB
    Ok(data.len() as i32)
}

/// A store moved aside as `x.bad` because it doesn't decode starts empty.
fn corrupt<T: Default>(path: &str) -> T {
    dprintln!("dtsrv: {} is corrupt: starting empty", path);
    store::keep_bad(path);
    T::default()
}

/// The store, or the default with `bit` added to `pending` (read again later; its writes wait).
fn ready<T: Default>(r: Result<T, i32>, bit: u32, pending: &mut u32) -> T {
    r.unwrap_or_else(|e| {
        dprintln!(
            "dtsrv: store {:#x} not read ({}): tried again later",
            bit,
            e
        );
        *pending |= bit;
        T::default()
    })
}

fn load_alarms(ui: &Ui) -> Result<Alarms, i32> {
    load(
        ui,
        store::ALARMS,
        "clock.alarms",
        Alarms::decode,
        Alarms::encode,
    )
}

fn load_timer(ui: &Ui) -> Result<Timer, i32> {
    load(
        ui,
        store::TIMERS,
        "clock.timer",
        Timer::decode,
        Timer::encode,
    )
}

fn load_rung() -> Result<Rung, i32> {
    Ok(match store::read(store::RINGS)? {
        Some(b) => Rung::decode(&b).unwrap_or_else(|_| corrupt(store::RINGS)),
        None => Rung::default(),
    })
}

/// A store from its file; on the first run (no file) from the setting `key` the Clock app kept it in, which is then
/// emptied. A file or setting that doesn't decode starts empty. `Err` when the file or the setting can't be read now:
/// nothing is written, and the caller tries again.
fn load<T: Default>(
    ui: &Ui,
    path: &str,
    key: &str,
    decode: impl Fn(&[u8]) -> Result<T, Corrupt>,
    encode: impl Fn(&T) -> Vec<u8>,
) -> Result<T, i32> {
    if let Some(b) = store::read(path)? {
        return Ok(decode(&b).unwrap_or_else(|_| corrupt(path)));
    }
    let blob = match ui.get(key) {
        Ok(Value::Bytes(b)) => b,
        // an unknown key (a newer shell dropped it): nothing to import
        Ok(_) | Err(err::KERR_NOT_FOUND) => Vec::new(),
        Err(e) => return Err(e),
    };
    let v = if blob.is_empty() {
        T::default()
    } else {
        decode(&blob).unwrap_or_else(|_| {
            dprintln!("dtsrv: import {}: corrupt, starting empty", key);
            T::default()
        })
    };
    dprintln!("dtsrv: import {}: {} bytes", key, blob.len());
    match store::write(path, &encode(&v)) {
        Ok(()) if !blob.is_empty() => {
            if let Err(e) = ui.set(key, &Value::Bytes(Vec::new())) {
                dprintln!("dtsrv: import {}: the key stays ({})", key, e);
            }
        }
        Ok(()) => {}
        Err(e) => dprintln!("dtsrv: import {}: {} not written ({})", key, path, e),
    }
    Ok(v)
}
