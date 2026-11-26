// SPDX-License-Identifier: Apache-2.0
//! DTSRV's client (dt-ipc 1.0, `datetime_core::ipc`): NEOCLOCK and NEOCAL read and change the alarms and the timer
//! here, act on a ring, and wait for changes with a [`Watch`].
#![no_std]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use datetime_core::alarm::{Alarm, Alarms};
use datetime_core::ipc::{self, op};
use datetime_core::next::Next;
use datetime_core::timer::Timer;
use e32::ipc::{Session, TIpcArgs, TVersion};
use e32::sys::{TPtr8, TPtrC8};

pub use datetime_core::ipc::{action, mask, ring, timer};

const KERR_CORRUPT: i32 = -20;

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
        let b = self.read(op::NEXT, ipc::NEXT_MAX, 0, 0)?;
        Next::decode(&b).map_err(|_| KERR_CORRUPT)
    }

    /// # Errors
    /// The server's, or `KErrCorrupt`.
    pub fn alarms(&self) -> Result<Alarms, i32> {
        let b = self.read(op::ALARM_LIST, ipc::ALARMS_BLOB_MAX, 0, 0)?;
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
        let b = self.read(op::TIMER, ipc::TIMER_BLOB_MAX, which, value)?;
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

    /// Empties the stores in `which` (`mask::ALARMS | mask::TIMERS`).
    ///
    /// # Errors
    /// A failed save.
    pub fn clear(&self, which: u32) -> Result<(), i32> {
        self.int(op::CLEAR, which as i32, 0)
    }

    fn int(&self, function: i32, a: i32, b: i32) -> Result<(), i32> {
        let mut args = TIpcArgs::new();
        args.int(0, a).int(1, b);
        self.0.send_receive(function, &args).map(drop)
    }

    /// A reply into a buffer of `max` bytes: slot 0 for NEXT and ALARM_LIST, slot 2 for TIMER (its slots 0 and 1 are
    /// `a` and `b`).
    fn read(&self, function: i32, max: usize, a: i32, b: i32) -> Result<Vec<u8>, i32> {
        let mut buf = vec![0u8; max];
        let n = {
            let mut des = TPtr8::new(&mut buf);
            let mut args = TIpcArgs::new();
            if function == op::TIMER {
                args.int(0, a).int(1, b).des8(2, &mut des);
            } else {
                args.des8(0, &mut des);
            }
            self.0.send_receive(function, &args)?;
            des.len()
        };
        buf.truncate(n);
        Ok(buf)
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
