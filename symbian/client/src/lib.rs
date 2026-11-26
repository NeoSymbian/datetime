// SPDX-License-Identifier: Apache-2.0
//! DTSRV's client (dt-ipc 1.0, `datetime_core::ipc`): NEOCLOCK and NEOCAL read and change the alarms, the timer, the
//! events, the to-dos and the calendars here, act on a ring, and wait for changes with a [`Watch`].
#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use calendar_core::wire;
use datetime_core::alarm::{Alarm, Alarms};
use datetime_core::ipc::{self, op};
use datetime_core::next::Next;
use datetime_core::timer::Timer;
use e32::ipc::{Session, TIpcArgs, TVersion};
use e32::sys::{TPtr8, TPtrC8};

pub use calendar_core::{EventDraft, Instance, Todo};
pub use datetime_core::ipc::{CalendarInfo, RingInfo, action, mask, ring, timer};

const KERR_CORRUPT: i32 = -20;

/// A request argument before the reply's descriptor.
#[derive(Clone, Copy)]
enum In<'a> {
    Int(i32),
    Des(&'a [u8]),
}

fn connect() -> Result<Session, i32> {
    let (major, minor, build) = ipc::VERSION;
    Session::connect(
        ipc::NAME,
        TVersion {
            major,
            minor,
            build,
        },
    )
}

/// A session with DTSRV.
pub struct Client(Session);

impl Client {
    /// # Errors
    /// `KErrNotFound` while DTSRV isn't running.
    pub fn connect() -> Result<Self, i32> {
        connect().map(Self)
    }

    /// The next alarm and event.
    ///
    /// # Errors
    /// The server's, or `KErrCorrupt` for a reply that doesn't decode.
    pub fn next(&self) -> Result<Next, i32> {
        let b = self.read(op::NEXT, &[], ipc::NEXT_MAX)?;
        Next::decode(&b).map_err(|_| KERR_CORRUPT)
    }

    /// # Errors
    /// The server's, or `KErrCorrupt`.
    pub fn alarms(&self) -> Result<Alarms, i32> {
        let b = self.read(op::ALARM_LIST, &[], ipc::ALARMS_BLOB_MAX)?;
        Alarms::decode(&b).map_err(|_| KERR_CORRUPT)
    }

    /// Adds (`id` 0) or replaces an alarm; returns its id.
    ///
    /// # Errors
    /// `KErrNotFound` for an unknown id, `KErrOverflow` when the list is full, or a failed save.
    pub fn put_alarm(&self, alarm: &Alarm) -> Result<u16, i32> {
        let rec = ipc::alarm_record(alarm);
        let d = TPtrC8::new(&rec);
        let mut args = TIpcArgs::new();
        args.desc8(0, &d);
        // ids are u16
        self.0
            .send_receive(op::ALARM_PUT, &args)
            .map(|id| id as u16)
    }

    /// # Errors
    /// `KErrNotFound`, or a failed save.
    pub fn delete_alarm(&self, id: u16) -> Result<(), i32> {
        self.int(op::ALARM_DEL, i32::from(id), 0)
    }

    /// Runs timer op `which` (`timer::START`, `PAUSE`...; `timer::GET` only reads) with `value`; returns the timer.
    ///
    /// # Errors
    /// The server's, or `KErrCorrupt`.
    pub fn timer(&self, which: i32, value: i32) -> Result<Timer, i32> {
        let b = self.read(
            op::TIMER,
            &[In::Int(which), In::Int(value)],
            ipc::TIMER_BLOB_MAX,
        )?;
        Timer::decode(&b).map_err(|_| KERR_CORRUPT)
    }

    /// Acts on ring `ring` (`action::SNOOZE`, ...).
    ///
    /// # Errors
    /// `KErrNotFound` when it isn't ringing.
    pub fn ring_action(&self, ring: u32, action: i32) -> Result<(), i32> {
        // ring ids fit 32 bits; the slot is the same word
        self.int(op::RING_ACTION, ring as i32, action)
    }

    /// Empties the stores in `which` (`mask::ALARMS | mask::TIMERS | mask::EVENTS | mask::TODOS`).
    ///
    /// # Errors
    /// A failed save.
    pub fn clear(&self, which: u32) -> Result<(), i32> {
        self.int(op::CLEAR, which as i32, 0)
    }

    /// The occurrences on local days `from_day..=to_day` (days since 1970) of the calendars not in `hidden` (bit i
    /// hides calendar i), birthdays included while `calendar.birthdays` is on; sorted by start.
    ///
    /// # Errors
    /// `KErrArgument` for a range over 1000 days, or `KErrCorrupt`.
    pub fn event_query(
        &self,
        from_day: i32,
        to_day: i32,
        hidden: u32,
    ) -> Result<Vec<Instance>, i32> {
        // the mask travels as the word it is
        let ins = [In::Int(from_day), In::Int(to_day), In::Int(hidden as i32)];
        let b = self.read(op::EVENT_QUERY, &ins, ipc::REPLY_FIRST)?;
        wire::decode_instances(&b).map_err(|_| KERR_CORRUPT)
    }

    /// Writes an event: a new one (empty uid), the series, or with `only` one occurrence. Returns its uid.
    ///
    /// # Errors
    /// `KErrNotFound` for an unknown uid, or a failed save.
    pub fn event_put(&self, draft: &EventDraft) -> Result<String, i32> {
        self.put(op::EVENT_PUT, &draft.encode(), &draft.uid)
    }

    /// The event `uid` as the form edits it: the series, or with `only` (an instance's `recur`) that occurrence.
    ///
    /// # Errors
    /// `KErrNotFound`, or `KErrCorrupt`.
    pub fn event_get(&self, uid: &str, only: Option<i64>) -> Result<EventDraft, i32> {
        let key = ipc::event_key(uid, only);
        let b = self.read(op::EVENT_GET, &[In::Des(&key)], 1024)?;
        EventDraft::decode(&b).map_err(|_| KERR_CORRUPT)
    }

    /// Deletes the series `uid`, or with `only` (an instance's `recur`) that occurrence.
    ///
    /// # Errors
    /// `KErrNotFound`, or a failed save.
    pub fn event_del(&self, uid: &str, only: Option<i64>) -> Result<(), i32> {
        let key = ipc::event_key(uid, only);
        self.call(op::EVENT_DEL, &[In::Des(&key)]).map(drop)
    }

    /// The to-dos: open ones first, by due time, then priority.
    ///
    /// # Errors
    /// The server's, or `KErrCorrupt`.
    pub fn todo_list(&self) -> Result<Vec<Todo>, i32> {
        let b = self.read(op::TODO_LIST, &[], ipc::REPLY_FIRST)?;
        wire::decode_todos(&b).map_err(|_| KERR_CORRUPT)
    }

    /// Writes a to-do (a new one for an empty uid); returns its uid.
    ///
    /// # Errors
    /// `KErrNotFound` for an unknown uid, or a failed save.
    pub fn todo_put(&self, todo: &Todo) -> Result<String, i32> {
        self.put(op::TODO_PUT, &todo.encode(), &todo.uid)
    }

    /// # Errors
    /// `KErrNotFound`, or a failed save.
    pub fn todo_del(&self, uid: &str) -> Result<(), i32> {
        self.call(op::TODO_DEL, &[In::Des(uid.as_bytes())])
            .map(drop)
    }

    /// Adds an .ics file's events and to-dos (UTF-8 text, at most `ipc::IMPORT_MAX` bytes) to calendar `cal`:
    /// (added, clashes), clashes being those whose UID is in the store already (left out).
    ///
    /// # Errors
    /// `KErrTooBig`, or a failed save.
    pub fn import_ics(&self, text: &[u8], cal: u8) -> Result<(u16, u16), i32> {
        let ins = [In::Des(text), In::Int(i32::from(cal))];
        self.call(op::IMPORT_ICS, &ins).map(ipc::import_counts)
    }

    /// One VCALENDAR with event or to-do `uid` (its series and changed occurrences), or with everything (`None`).
    ///
    /// # Errors
    /// `KErrNotFound` for an unknown uid.
    pub fn export_ics(&self, uid: Option<&str>) -> Result<String, i32> {
        let u = uid.unwrap_or("");
        let b = self.read(op::EXPORT_ICS, &[In::Des(u.as_bytes())], ipc::REPLY_FIRST)?;
        String::from_utf8(b).map_err(|_| KERR_CORRUPT)
    }

    /// The calendars, by index (an `Instance::cal`, `EventDraft::cal`); hidden = listed in `calendar.hidden`.
    ///
    /// # Errors
    /// The server's, or `KErrCorrupt`.
    pub fn calendars(&self) -> Result<Vec<CalendarInfo>, i32> {
        let b = self.read(op::CALENDARS, &[], 1024)?;
        ipc::decode_calendars(&b).map_err(|_| KERR_CORRUPT)
    }

    /// Renames and recolours calendar `index`, or adds one (`None`); returns its index.
    ///
    /// # Errors
    /// `KErrArgument` for an empty name, `KErrNotFound`, `KErrOverflow` past 32 calendars, or a failed save.
    pub fn calendar_put(&self, index: Option<u8>, name: &str, colour: u32) -> Result<u8, i32> {
        let i = index.map_or(-1, i32::from);
        // the colour is 0xRRGGBB
        let ins = [In::Int(i), In::Int(colour as i32), In::Des(name.as_bytes())];
        // indices are below 32
        self.call(op::CALENDAR_PUT, &ins).map(|i| i as u8)
    }

    /// What reminder ring `ring` (kind 3) is about, for the Ringing page.
    ///
    /// # Errors
    /// `KErrNotFound` once DTSRV forgot it, `KErrNotSupported` for another kind, or `KErrCorrupt`.
    pub fn ring_info(&self, ring: u32) -> Result<RingInfo, i32> {
        // ring ids fit 32 bits; the slot is the same word
        let b = self.read(op::RING_INFO, &[In::Int(ring as i32)], 1024)?;
        RingInfo::decode(&b).map_err(|_| KERR_CORRUPT)
    }

    fn int(&self, function: i32, a: i32, b: i32) -> Result<(), i32> {
        self.call(function, &[In::Int(a), In::Int(b)]).map(drop)
    }

    /// A request without a reply descriptor: its completion value.
    fn call(&self, function: i32, ins: &[In]) -> Result<i32, i32> {
        let ptrs = descs(ins);
        let mut args = TIpcArgs::new();
        set(&mut args, ins, &ptrs);
        self.0.send_receive(function, &args)
    }

    /// A reply into the descriptor after `ins`, first into `first` bytes; a longer reply is asked for again with its
    /// length (`datetime_core::ipc`).
    fn read(&self, function: i32, ins: &[In], first: usize) -> Result<Vec<u8>, i32> {
        let (mut buf, n) = self.once(function, ins, first)?;
        if n > buf.len() {
            (buf, _) = self.once(function, ins, n)?;
        }
        buf.truncate(n);
        Ok(buf)
    }

    /// A PUT's uid reply, never asked again: a uid longer than the buffer is the one sent.
    fn put(&self, function: i32, record: &[u8], uid: &str) -> Result<String, i32> {
        let (buf, n) = self.once(function, &[In::Des(record)], ipc::UID_MAX)?;
        if n > buf.len() {
            return Ok(String::from(uid));
        }
        String::from_utf8(buf[..n].to_vec()).map_err(|_| KERR_CORRUPT)
    }

    /// One request with a `max`-byte reply buffer: (the buffer, the reply's full length).
    fn once(&self, function: i32, ins: &[In], max: usize) -> Result<(Vec<u8>, usize), i32> {
        let ptrs = descs(ins);
        let mut buf = vec![0u8; max];
        let n = {
            let mut des = TPtr8::new(&mut buf);
            let mut args = TIpcArgs::new();
            set(&mut args, ins, &ptrs);
            args.des8(ins.len(), &mut des);
            self.0.send_receive(function, &args)?
        };
        Ok((buf, usize::try_from(n).unwrap_or(0)))
    }
}

/// The descriptors of `ins`' `Des` arguments, in place.
fn descs(ins: &[In]) -> Vec<Option<TPtrC8>> {
    ins.iter()
        .map(|a| match a {
            In::Des(b) => Some(TPtrC8::new(b)),
            In::Int(_) => None,
        })
        .collect()
}

/// Puts `ins` into the first slots (`ptrs` holds their descriptors).
fn set<'a>(args: &mut TIpcArgs<'a>, ins: &[In], ptrs: &'a [Option<TPtrC8>]) {
    for (i, (a, p)) in ins.iter().zip(ptrs).enumerate() {
        match (a, p) {
            (In::Int(v), _) => {
                args.int(i, *v);
            }
            (In::Des(_), Some(p)) => {
                args.desc8(i, p);
            }
            (In::Des(_), None) => {}
        }
    }
}

/// Waits for changes on a session of its own, so it can move into an app's select future while the [`Client`] is
/// used: `watch.next().await` gives the watch back with the bits that changed (re-read those stores).
pub struct Watch {
    session: Session,
    mask: u32,
}

/// Withdraws a WATCH that is still outstanding: dropping a pending send waits for its reply, which may never come.
struct Cancel<'a>(&'a Session, bool);

impl Drop for Cancel<'_> {
    fn drop(&mut self) {
        if !self.1 {
            let _ = self.0.send_receive(op::WATCH_CANCEL, &TIpcArgs::new());
        }
    }
}

impl Watch {
    /// Watches the stores in `which` (`mask::*`). Changes count from here on.
    ///
    /// # Errors
    /// `KErrNotFound` while DTSRV isn't running.
    pub fn new(which: u32) -> Result<Self, i32> {
        Ok(Self {
            session: connect()?,
            mask: which,
        })
    }

    /// The next change: the bits of the mask that changed, or the server's error (`KErrServerTerminated`: DTSRV
    /// went away; make a new watch).
    pub async fn next(self) -> (Self, Result<u32, i32>) {
        let r = {
            let mut args = TIpcArgs::new();
            args.int(0, self.mask as i32);
            let future = core::pin::pin!(self.session.send(op::WATCH, &args));
            // declared after the pinned future: if this future is dropped early, the cancel goes out first
            let mut cancel = Cancel(&self.session, false);
            let r = future.await;
            cancel.1 = true;
            r
        };
        (self, r.map(|bits| bits as u32))
    }
}
