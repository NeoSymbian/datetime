// SPDX-License-Identifier: Apache-2.0
//! dt-ipc 1.0: DTSRV's client-server contract (the `DtSrv` server; docs datetime.md, the `dt2-spec-dtsrv` board).
//! Integers travel in the message's slots, records as `neoui_proto::clock::wire` blobs in 8-bit descriptors. Every
//! reply is the request's completion value: 0 or a length/id (>= 0), else a Symbian error.

use crate::alarm::{Alarm, LABEL_MAX, Weekdays};
use alloc::vec::Vec;
use neoui_proto::clock::wire::{Corrupt, Reader, Writer};

/// The server's name.
pub const NAME: &str = "DtSrv";
/// (major, minor, build); a connect with another major is refused.
pub const VERSION: (u8, u8, u16) = (1, 0, 0);

/// The opcodes, with their slots.
pub mod op {
    /// `[0 Des8 out: crate::next blob, at most NEXT_MAX]` → its length.
    pub const NEXT: i32 = 1;
    /// `[0 mask (super::mask)]` → the bits of `mask` that changed since this session's last WATCH (> 0). Async:
    /// it completes when one of them changes; changes in between are kept per session, so none is lost.
    pub const WATCH: i32 = 2;
    /// `[0 Des8 out: Alarms::encode, at most ALARMS_BLOB_MAX]` → its length.
    pub const ALARM_LIST: i32 = 3;
    /// `[0 Des8: alarm_record]` → the alarm's id. Id 0 creates one; `KErrNotFound` for an unknown id,
    /// `KErrOverflow` when the list is full.
    pub const ALARM_PUT: i32 = 4;
    /// `[0 id]` → 0, or `KErrNotFound`.
    pub const ALARM_DEL: i32 = 5;
    /// `[0 super::timer op, 1 value, 2 Des8 out: Timer::encode, at most TIMER_BLOB_MAX]` → its length.
    pub const TIMER: i32 = 6;
    /// `[0 ring id, 1 super::action]` → 0, or `KErrNotFound` when that ring isn't ringing.
    pub const RING_ACTION: i32 = 7;
    /// Calendar (DT3): `KErrNotSupported` until the event and to-do stores land.
    pub const EVENT_QUERY: i32 = 8;
    pub const EVENT_PUT: i32 = 9;
    pub const EVENT_DEL: i32 = 10;
    pub const TODO_LIST: i32 = 11;
    pub const TODO_PUT: i32 = 12;
    pub const TODO_DEL: i32 = 13;
    pub const IMPORT_ICS: i32 = 14;
    pub const EXPORT_ICS: i32 = 15;
    /// `[0 mask]` → 0: empties those stores (Settings → Clear data); their rings stop.
    pub const CLEAR: i32 = 16;
    /// `[]` → 0: completes this session's outstanding WATCH with `KErrCancel` (a client dropping its watch).
    pub const WATCH_CANCEL: i32 = 17;
}

/// WATCH and CLEAR mask bits.
pub mod mask {
    pub const ALARMS: u32 = 1;
    pub const TIMERS: u32 = 2;
    pub const EVENTS: u32 = 4;
    pub const TODOS: u32 = 8;
    /// a ring started or ended
    pub const RINGS: u32 = 16;
    pub const ALL: u32 = 31;
}

/// A ring id: `(kind << 24) | item`, the alarm's id, 0 for the timer, or a reminder's number. It is the ring card's
/// id too, and Clock's RING page arg (in decimal).
pub mod ring {
    pub const ALARM: u8 = 1;
    pub const TIMER: u8 = 2;
    pub const REMINDER: u8 = 3;

    #[must_use]
    pub const fn id(kind: u8, item: u32) -> u32 {
        (kind as u32) << 24 | (item & 0x00FF_FFFF)
    }

    #[must_use]
    pub const fn kind(ring: u32) -> u8 {
        (ring >> 24) as u8
    }

    #[must_use]
    pub const fn item(ring: u32) -> u32 {
        ring & 0x00FF_FFFF
    }
}

/// RING_ACTION's actions.
pub mod action {
    pub const SNOOZE: i32 = 1;
    pub const DISMISS: i32 = 2;
    /// a timer's Stop
    pub const STOP: i32 = 3;
    /// a timer's +1 min
    pub const PLUS_MINUTE: i32 = 4;
}

/// TIMER's ops (slot 0); slot 1 is their value.
pub mod timer {
    /// only reads the state
    pub const GET: i32 = 0;
    /// value: the duration in ms (0 = the one it has); a paused timer resumes
    pub const START: i32 = 1;
    pub const PAUSE: i32 = 2;
    pub const RESUME: i32 = 3;
    pub const RESET: i32 = 4;
    /// value: seconds to add (negative takes away); a done timer starts again with them
    pub const ADD: i32 = 5;
}

/// The longest replies.
pub const NEXT_MAX: usize = 320;
pub const ALARMS_BLOB_MAX: usize = 1024;
pub const TIMER_BLOB_MAX: usize = 32;
const RECORD_VERSION: u8 = 1;

/// One alarm for ALARM_PUT: the fields of an `Alarms` blob entry.
#[must_use]
pub fn alarm_record(a: &Alarm) -> Vec<u8> {
    let mut w = Writer::new(RECORD_VERSION);
    w.u16(a.id)
        .u8(a.hour)
        .u8(a.minute)
        .u8(a.days.bits())
        .u8(u8::from(a.on))
        .str(&a.label);
    w.finish()
}

/// Reads an ALARM_PUT record.
///
/// # Errors
/// [`Corrupt`] for a damaged record or out-of-range fields.
pub fn read_alarm_record(b: &[u8]) -> Result<Alarm, Corrupt> {
    let mut r = Reader::new(b, RECORD_VERSION)?;
    let (id, hour, minute, days, on) = (r.u16()?, r.u8()?, r.u8()?, r.u8()?, r.u8()?);
    let label = r.str()?;
    r.end()?;
    if hour > 23 || minute > 59 || on > 1 || label.len() > LABEL_MAX {
        return Err(Corrupt);
    }
    Ok(Alarm {
        id,
        hour,
        minute,
        days: Weekdays::from_bits(days),
        label,
        on: on == 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_ids_split_back() {
        let r = ring::id(ring::TIMER, 0);
        assert_eq!(r, 0x0200_0000);
        let a = ring::id(ring::ALARM, 7);
        assert_eq!((ring::kind(a), ring::item(a)), (ring::ALARM, 7));
    }

    #[test]
    fn alarm_records_round_trip_and_refuse_damage() {
        let a = Alarm {
            id: 3,
            hour: 7,
            minute: 30,
            days: Weekdays::WORKDAYS,
            label: "Gym".into(),
            on: true,
        };
        let b = alarm_record(&a);
        assert_eq!(read_alarm_record(&b), Ok(a));
        assert_eq!(read_alarm_record(&b[..b.len() - 1]), Err(Corrupt));
        let mut bad = b.clone();
        bad[3] = 24; // the hour: version, id (2), hour
        assert_eq!(read_alarm_record(&bad), Err(Corrupt));
    }
}
