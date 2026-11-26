// SPDX-License-Identifier: Apache-2.0
//! DTSRV's state: the alarms and the timer, what rings, the shade cards, `dtsrv.next`, and dt-ipc's requests.

use crate::store;
use crate::ui::{OPEN_WAKE, Ui};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::ops::ControlFlow;
use datetime_core::alarm::Alarms;
use datetime_core::ipc::{self, action, mask, op, ring};
use datetime_core::next::Next;
use datetime_core::pages;
use datetime_core::plan::{self, minute_of_week};
use datetime_core::timer::{Phase, Timer, format_left};
use e32::dprintln;
use e32::server::{Message, Service};
use e32::sys::{TVersion, err};
use neoui_proto::clock::wire::Corrupt;
use neoui_proto::ipc::AppEvent;
use neoui_proto::page::{
    NOTIFY_ONGOING, NOTIFY_SILENT_UPDATE, Notification, action as kind, style,
};
use neoui_proto::settings::Value;

/// Days from Symbian's `TTime` zero to 1970-01-01.
const TTIME_UNIX_DAYS: i64 = 719_540;
const TIMER_RING: u32 = ring::id(ring::TIMER, 0);

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
}

pub struct Dt {
    ui: Ui,
    alarms: Alarms,
    timer: Timer,
    /// the rings going on (ring ids)
    ringing: Vec<u32>,
    /// alarms Snooze made: (id, snoozes left after it); they go when dismissed
    snoozed: Vec<(u16, u8)>,
    settings: Settings,
    /// the last plan's now (Unix seconds): things due after it ring
    since: i64,
    /// the `dtsrv.next` last written
    published: Vec<u8>,
    /// the running timer's card text as last posted ("" = no card)
    chrono: String,
    watchers: Vec<Rc<RefCell<Watcher>>>,
}

impl Dt {
    pub fn start(ui: Ui) -> Self {
        // a restart's cards: their rings are gone
        let _ = ui.clear_cards();
        for prefix in ["time.", "clock."] {
            if let Err(e) = ui.watch(prefix) {
                dprintln!("dtsrv: settings watch {} refused ({})", prefix, e);
            }
        }
        let alarms = load(
            &ui,
            store::ALARMS,
            "clock.alarms",
            Alarms::decode,
            Alarms::encode,
        );
        let timer = load(
            &ui,
            store::TIMERS,
            "clock.timer",
            Timer::decode,
            Timer::encode,
        );
        dprintln!(
            "dtsrv: start: {} alarms, timer {:?}",
            alarms.as_slice().len(),
            timer.phase
        );
        let mut d = Self {
            ui,
            alarms,
            timer,
            ringing: Vec::new(),
            snoozed: Vec::new(),
            settings: Settings {
                snooze_min: 5,
                snooze_count: 3,
                h24: true,
            },
            since: clock().0.div_euclid(1000),
            published: Vec::new(),
            chrono: String::new(),
            watchers: Vec::new(),
        };
        d.read_settings();
        d
    }

    pub fn ui_session(&self) -> Rc<e32::ipc::Session> {
        self.ui.session()
    }

    /// Plans, rings what is due, refreshes the timer's card and `dtsrv.next`; returns how long to sleep (µs).
    pub fn tick(&mut self) -> i32 {
        let (now_ms, offset) = clock();
        let now = now_ms.div_euclid(1000);
        let p = plan::plan(&self.alarms, &self.timer, self.since, now, offset, &[]);
        self.since = now;
        for r in p.due {
            if !self.ringing.contains(&r) {
                self.fire(r, now_ms);
            }
        }
        self.refresh_chrono(now_ms);
        self.publish(now, offset);
        // shortcut: sleeps at most 60 s, so a clock change other than a time.* setting is seen up to a minute late;
        // upgrade to an e32 TIMER_AT (0x36) wrapper, which the kernel completes when the clock is set.
        let ms = p
            .next
            .map_or(60_000, |(at, _)| (at * 1000 - now_ms).clamp(1, 60_000));
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
        self.settings = Settings {
            // both clamped into their types
            snooze_min: int("clock.snooze", 5).clamp(1, 60) as u16,
            snooze_count: int("clock.snooze_count", 3).clamp(0, 10) as u8,
            h24: !matches!(self.ui.get("clock.24h"), Ok(Value::Bool(false))),
        };
    }

    fn fire(&mut self, r: u32, now_ms: i64) {
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
                self.timer.tick(now_ms);
                let _ = self.save_timer();
                // the done card replaces the running timer's (same id)
                self.chrono.clear();
                let text = format!("Timer · {}", format_left(self.timer.set_ms));
                ("Time's up".to_string(), text, ["+1 min", "Stop"])
            }
            _ => return,
        };
        dprintln!("dtsrv: fire {:#x} {} {}", r, title, text);
        self.open(pages::clock::RING, &r.to_string());
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
        self.ringing.push(r);
        self.changed(mask::RINGS | if r == TIMER_RING { mask::TIMERS } else { 0 });
    }

    /// Snooze, Dismiss, Stop or +1 min on a ring (RING_ACTION, or its card's buttons).
    fn act(&mut self, r: u32, what: i32) -> Result<(), i32> {
        let Some(i) = self.ringing.iter().position(|&x| x == r) else {
            return Err(err::KERR_NOT_FOUND);
        };
        self.ringing.remove(i);
        let (now_ms, offset) = clock();
        let saved = match ring::kind(r) {
            ring::ALARM => {
                // the planner's ids come from the alarms
                let id = ring::item(r) as u16;
                let copy = self.snoozed.iter().position(|s| s.0 == id);
                let left = copy.map_or(self.settings.snooze_count, |i| self.snoozed.remove(i).1);
                let snooze = matches!(what, action::SNOOZE | action::PLUS_MINUTE) && left > 0;
                let now = minute_of_week(now_ms.div_euclid(1000), offset);
                match snooze.then(|| self.alarms.snooze_for(id, now, self.settings.snooze_min)) {
                    Some(Ok(new)) => {
                        dprintln!(
                            "dtsrv: action {:#x} snooze -> alarm {} ({} left)",
                            r,
                            new,
                            left - 1
                        );
                        self.snoozed.push((new, left - 1));
                    }
                    Some(Err(_)) => {
                        dprintln!("dtsrv: action {:#x} snooze: the list is full, dismissed", r);
                    }
                    None => {
                        self.alarms.dismissed(id);
                        dprintln!("dtsrv: action {:#x} dismiss ({} snoozes left)", r, left);
                    }
                }
                if copy.is_some() {
                    self.alarms.remove(id);
                }
                self.changed(mask::ALARMS);
                self.save_alarms()
            }
            ring::TIMER => {
                if matches!(what, action::PLUS_MINUTE | action::SNOOZE) {
                    self.timer.add(60_000, now_ms);
                    dprintln!("dtsrv: action {:#x} +1 min", r);
                } else {
                    self.timer.reset();
                    dprintln!("dtsrv: action {:#x} stop", r);
                }
                self.changed(mask::TIMERS);
                self.save_timer()
            }
            _ => Ok(()),
        };
        self.remove_card(r);
        self.changed(mask::RINGS);
        saved
    }

    /// A tap on one of DTSRV's cards (action 0) or on its button `action - 1`.
    fn card(&mut self, id: u32, action: u16) {
        let timer = ring::kind(id) == ring::TIMER;
        if self.ringing.contains(&id) {
            let what = match (timer, action) {
                (_, 0) => return self.open(pages::clock::RING, &id.to_string()),
                (true, 1) => action::PLUS_MINUTE,
                (true, _) => action::STOP,
                (false, 1) => action::SNOOZE,
                (false, _) => action::DISMISS,
            };
            let _ = self.act(id, what);
        } else if timer && action == 0 {
            self.open(pages::clock::TIMER, "");
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
                format!("{} min left", ((ends_ms - now_ms).max(1) + 59_999) / 60_000)
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
        } else if !self.ringing.contains(&TIMER_RING) {
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

    /// The `dtsrv.next` blob (the events come with DT3).
    fn next(&self, now: i64, offset: i32) -> Vec<u8> {
        let mut next = Next::default();
        if let Some((at, a)) = plan::next_alarm(&self.alarms, now, offset) {
            next.alarm_at = at;
            next.alarm_label = a.label.clone();
        }
        next.encode()
    }

    fn open(&self, page: u16, arg: &str) {
        if let Err(e) = self.ui.open("clock", page, arg, OPEN_WAKE) {
            dprintln!("dtsrv: OPEN clock {} {} refused ({})", page, arg, e);
        }
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
            .ringing
            .iter()
            .copied()
            .filter(|&r| ring::kind(r) == kind)
            .collect();
        self.ringing.retain(|&r| ring::kind(r) != kind);
        for &r in &gone {
            self.remove_card(r);
        }
        if !gone.is_empty() {
            self.changed(mask::RINGS);
        }
    }

    fn save_alarms(&self) -> Result<(), i32> {
        store::write(store::ALARMS, &self.alarms.encode())
            .inspect_err(|e| dprintln!("dtsrv: alarms not saved ({})", e))
    }

    fn save_timer(&self) -> Result<(), i32> {
        store::write(store::TIMERS, &self.timer.encode())
            .inspect_err(|e| dprintln!("dtsrv: timer not saved ({})", e))
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
        let id = if a.id == 0 {
            let id = self
                .alarms
                .add(a.hour, a.minute, a.days, &a.label)
                .map_err(|_| err::KERR_OVERFLOW)?;
            self.alarms.set_on(id, a.on);
            id
        } else if self.alarms.update(a.clone()) {
            a.id
        } else {
            return Err(err::KERR_NOT_FOUND);
        };
        self.changed(mask::ALARMS);
        self.save_alarms().map(|()| i32::from(id))
    }

    fn delete_alarm(&mut self, id: i32) -> Result<i32, i32> {
        let id = u16::try_from(id).map_err(|_| err::KERR_NOT_FOUND)?;
        if !self.alarms.remove(id) {
            return Err(err::KERR_NOT_FOUND);
        }
        let r = ring::id(ring::ALARM, u32::from(id));
        if let Some(i) = self.ringing.iter().position(|&x| x == r) {
            self.ringing.remove(i);
            self.remove_card(r);
            self.changed(mask::RINGS);
        }
        self.snoozed.retain(|s| s.0 != id);
        self.changed(mask::ALARMS);
        self.save_alarms().map(|()| 0)
    }

    fn timer_op(&mut self, msg: &Message) -> Result<i32, i32> {
        let (which, value) = (msg.int(0), msg.int(1));
        let now_ms = clock().0;
        let t = &mut self.timer;
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
            self.save_timer()?;
        }
        reply(msg, 2, &self.timer.encode())
    }

    fn clear(&mut self, which: u32) -> Result<i32, i32> {
        dprintln!("dtsrv: clear {:#x}", which);
        let mut saved = Ok(0);
        if which & mask::ALARMS != 0 {
            self.alarms = Alarms::new();
            self.snoozed.clear();
            self.stop_rings(ring::ALARM);
            saved = saved.and(self.save_alarms().map(|()| 0));
        }
        if which & mask::TIMERS != 0 {
            self.timer = Timer::default();
            self.stop_rings(ring::TIMER);
            saved = saved.and(self.save_timer().map(|()| 0));
        }
        self.changed(which & (mask::ALARMS | mask::TIMERS));
        saved
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
            // the calendar's (DT3)
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

/// Writes `data` into the client's descriptor `slot`; the reply is its length.
fn reply(msg: &Message, slot: usize, data: &[u8]) -> Result<i32, i32> {
    msg.write8(slot, data, 0)?;
    // the stores are far below 2 GB
    Ok(data.len() as i32)
}

/// A store from its file; on the first run (no file) from the setting `key` the Clock app kept it in, which is then
/// emptied. A corrupt file or setting starts empty.
fn load<T: Default>(
    ui: &Ui,
    path: &str,
    key: &str,
    decode: impl Fn(&[u8]) -> Result<T, Corrupt>,
    encode: impl Fn(&T) -> Vec<u8>,
) -> T {
    match store::read(path) {
        Ok(Some(b)) => decode(&b).unwrap_or_else(|_| {
            dprintln!("dtsrv: {} is corrupt: starting empty", path);
            T::default()
        }),
        Ok(None) => {
            let blob = match ui.get(key) {
                Ok(Value::Bytes(b)) => b,
                _ => Vec::new(),
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
            v
        }
        Err(e) => {
            dprintln!("dtsrv: {} not read ({}): starting empty", path, e);
            T::default()
        }
    }
}
