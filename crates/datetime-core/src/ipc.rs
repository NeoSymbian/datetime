// SPDX-License-Identifier: Apache-2.0
//! dt-ipc 1.0: DTSRV's client-server contract (the `DtSrv` server; docs datetime.md, the `dt2-spec-dtsrv` board).
//! Integers travel in the message's slots, records as `neoui_proto::clock::wire` blobs in 8-bit descriptors (the
//! calendar's as `calendar_core::wire`). Every reply is the request's completion value: 0 or a length/id (>= 0), else
//! a Symbian error.
//!
//! **Replies into a descriptor** ("→ its length"): the completion is always the reply's full length. When that is
//! more than the out descriptor's capacity, nothing is written and the client asks again with that much room (dt-client
//! does). The reading ops are safe to repeat; a PUT is not, so its uid reply is never retried (a uid longer than the
//! client's [`UID_MAX`] buffer is the one it sent: DTSRV's own uids are short).

use crate::alarm::{Alarm, LABEL_MAX, Weekdays};
use alloc::string::String;
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
    /// `[0 from_day, 1 to_day (local days since 1970, inclusive), 2 hidden calendars (bit i hides calendar i), 3 Des8
    /// out: calendar_core::wire instance list]` → its length. Birthdays come along when `calendar.birthdays` is on.
    pub const EVENT_QUERY: i32 = 8;
    /// `[0 Des8: EventDraft::encode, 1 Des8 out: the uid]` → its length. An empty uid creates the event; `only` writes
    /// one occurrence. `KErrNotFound` for an unknown uid, `KErrArgument` for a damaged draft.
    pub const EVENT_PUT: i32 = 9;
    /// `[0 Des8: super::event_key]` → 0: the series, or with `only` that occurrence; `KErrNotFound`.
    pub const EVENT_DEL: i32 = 10;
    /// `[0 Des8 out: calendar_core::wire todo list]` → its length.
    pub const TODO_LIST: i32 = 11;
    /// `[0 Des8: Todo::encode, 1 Des8 out: the uid]` → its length; an empty uid creates one. `KErrNotFound`.
    pub const TODO_PUT: i32 = 12;
    /// `[0 Des8: the uid]` → 0, or `KErrNotFound`.
    pub const TODO_DEL: i32 = 13;
    /// `[0 Des8: the .ics text (UTF-8, at most IMPORT_MAX bytes), 1 calendar index]` → `super::import_reply`: the
    /// events and to-dos added, and those left out because their UID is in the store already. `KErrTooBig`.
    pub const IMPORT_ICS: i32 = 14;
    /// `[0 Des8: a uid, empty = everything, 1 Des8 out: one VCALENDAR]` → its length; `KErrNotFound` for a uid with
    /// nothing.
    pub const EXPORT_ICS: i32 = 15;
    /// `[0 mask]` → 0: empties those stores (Settings → Clear data); their rings stop.
    pub const CLEAR: i32 = 16;
    /// `[]` → 0: completes this session's outstanding WATCH with `KErrCancel` (a client dropping its watch).
    pub const WATCH_CANCEL: i32 = 17;
    /// `[0 Des8 out: super::encode_calendars]` → its length. Hidden = listed in `calendar.hidden`.
    pub const CALENDARS: i32 = 18;
    /// `[0 index (-1: a new calendar), 1 colour 0xRRGGBB, 2 Des8: the name]` → its index; `KErrNotFound`.
    pub const CALENDAR_PUT: i32 = 19;
    /// `[0 ring id, 1 Des8 out: super::RingInfo::encode]` → its length: a reminder's ring (kind 3), for Calendar's
    /// Ringing page. `KErrNotFound` once the reminder is forgotten, `KErrNotSupported` for other kinds.
    pub const RING_INFO: i32 = 20;
    /// `[0 Des8: super::event_key, 1 Des8 out: EventDraft::encode]` → its length: the series, or with `only` that
    /// occurrence, as the form edits it. `KErrNotFound`.
    pub const EVENT_GET: i32 = 21;
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
/// The uid buffer a PUT's reply goes into.
pub const UID_MAX: usize = 256;
/// The first buffer for the calendar's growing replies (a month of events fits); larger ones are asked again.
pub const REPLY_FIRST: usize = 16 << 10;
/// The largest .ics text IMPORT_ICS takes.
pub const IMPORT_MAX: usize = 256 << 10;
const RECORD_VERSION: u8 = 1;

/// IMPORT_ICS's completion: `added << 16 | clashes`, each capped (added at 0x7FFF, so it stays positive).
#[must_use]
pub fn import_reply(added: usize, clashes: usize) -> i32 {
    // both capped into their halves
    (added.min(0x7FFF) << 16 | clashes.min(0xFFFF)) as i32
}

/// (added, clashes) from [`import_reply`]'s value.
#[must_use]
pub fn import_counts(reply: i32) -> (u16, u16) {
    // a non-negative reply's halves
    ((reply >> 16) as u16, reply as u16)
}

/// EVENT_DEL's and EVENT_GET's request: `version, u8 has_only, i64 only (local), the uid's bytes` (any length).
#[must_use]
pub fn event_key(uid: &str, only: Option<i64>) -> Vec<u8> {
    let mut b = Vec::with_capacity(10 + uid.len());
    b.push(RECORD_VERSION);
    b.push(u8::from(only.is_some()));
    b.extend_from_slice(&only.unwrap_or(0).to_le_bytes());
    b.extend_from_slice(uid.as_bytes());
    b
}

/// Reads an [`event_key`]: (uid, only).
///
/// # Errors
/// [`Corrupt`] for a short or damaged key.
pub fn read_event_key(b: &[u8]) -> Result<(String, Option<i64>), Corrupt> {
    let [RECORD_VERSION, has @ (0 | 1), rest @ ..] = b else {
        return Err(Corrupt);
    };
    let (only, uid) = rest.split_at_checked(8).ok_or(Corrupt)?;
    let only = i64::from_le_bytes(only.try_into().map_err(|_| Corrupt)?);
    let uid = core::str::from_utf8(uid).map_err(|_| Corrupt)?;
    Ok((String::from(uid), (*has == 1).then_some(only)))
}

/// One calendar in CALENDARS' reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarInfo {
    pub name: String,
    /// 0xRRGGBB
    pub colour: u32,
    /// listed in `calendar.hidden`
    pub hidden: bool,
    /// its events (a series counts once)
    pub events: u16,
}

/// CALENDARS' reply: `u8 n, then n × (str name, u32 colour, u8 hidden, u16 events)`.
#[must_use]
pub fn encode_calendars(v: &[CalendarInfo]) -> Vec<u8> {
    let mut w = Writer::new(RECORD_VERSION);
    let n = v.len().min(usize::from(u8::MAX));
    // n <= 255
    w.u8(n as u8);
    for c in &v[..n] {
        w.str(&c.name)
            .u32(c.colour)
            .u8(u8::from(c.hidden))
            .u16(c.events);
    }
    w.finish()
}

/// # Errors
/// [`Corrupt`] for a damaged list.
pub fn decode_calendars(b: &[u8]) -> Result<Vec<CalendarInfo>, Corrupt> {
    let mut r = Reader::new(b, RECORD_VERSION)?;
    let n = r.u8()?;
    let v = (0..n)
        .map(|_| {
            Ok(CalendarInfo {
                name: r.str()?,
                colour: r.u32()?,
                hidden: r.u8()? != 0,
                events: r.u16()?,
            })
        })
        .collect::<Result<_, Corrupt>>()?;
    r.end()?;
    Ok(v)
}

/// What a reminder's ring is about (RING_INFO), for Calendar's Ringing page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RingInfo {
    /// the ring's kind ([`ring::REMINDER`])
    pub kind: u8,
    /// a to-do's reminder (else an event's)
    pub todo: bool,
    pub all_day: bool,
    /// it rings now (else it was snoozed or dismissed)
    pub ringing: bool,
    /// Unix UTC seconds: the occurrence's start (a to-do's due time) and end
    pub start: i64,
    pub end: i64,
    /// 0xRRGGBB, its calendar's
    pub colour: u32,
    /// for `pages::calendar::EVENT`
    pub uid: String,
    pub title: String,
    pub place: String,
}

/// Up to `u16::MAX` bytes, cut on a character boundary.
fn text(w: &mut Writer, s: &str) {
    let mut end = s.len().min(usize::from(u16::MAX));
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    // end <= u16::MAX
    w.u16(end as u16);
    for &b in &s.as_bytes()[..end] {
        w.u8(b);
    }
}

fn read_text(r: &mut Reader) -> Result<String, Corrupt> {
    let b = (0..r.u16()?)
        .map(|_| r.u8())
        .collect::<Result<Vec<u8>, _>>()?;
    String::from_utf8(b).map_err(|_| Corrupt)
}

impl RingInfo {
    /// `u8 kind, u8 flags (1 to-do, 2 all day, 4 ringing), i64 start, i64 end, u32 colour, then uid, title and place as
    /// u16 length + UTF-8`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new(RECORD_VERSION);
        w.u8(self.kind)
            .u8(u8::from(self.todo) | u8::from(self.all_day) << 1 | u8::from(self.ringing) << 2)
            .i64(self.start)
            .i64(self.end)
            .u32(self.colour);
        for s in [&self.uid, &self.title, &self.place] {
            text(&mut w, s);
        }
        w.finish()
    }

    /// # Errors
    /// [`Corrupt`] for a damaged record.
    pub fn decode(b: &[u8]) -> Result<Self, Corrupt> {
        let mut r = Reader::new(b, RECORD_VERSION)?;
        let (kind, flags) = (r.u8()?, r.u8()?);
        let info = Self {
            kind,
            todo: flags & 1 != 0,
            all_day: flags & 2 != 0,
            ringing: flags & 4 != 0,
            start: r.i64()?,
            end: r.i64()?,
            colour: r.u32()?,
            uid: read_text(&mut r)?,
            title: read_text(&mut r)?,
            place: read_text(&mut r)?,
        };
        r.end()?;
        Ok(info)
    }
}

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

    #[test]
    fn import_replies_split_back() {
        assert_eq!(import_counts(import_reply(3, 2)), (3, 2));
        assert_eq!(import_counts(import_reply(0, 0)), (0, 0));
        let big = import_reply(100_000, 70_000);
        assert!(big > 0);
        assert_eq!(import_counts(big), (0x7FFF, 0xFFFF));
    }

    #[test]
    fn event_keys_round_trip_and_refuse_damage() {
        let long = "x".repeat(400);
        for (uid, only) in [
            ("", None),
            ("a@b", Some(-86_400)),
            (long.as_str(), Some(1 << 40)),
        ] {
            let k = event_key(uid, only);
            assert_eq!(read_event_key(&k), Ok((String::from(uid), only)));
        }
        let k = event_key("a", Some(5));
        assert_eq!(read_event_key(&k[..9]), Err(Corrupt));
        let mut bad = k.clone();
        bad[1] = 2;
        assert_eq!(read_event_key(&bad), Err(Corrupt));
        assert_eq!(read_event_key(&[]), Err(Corrupt));
    }

    #[test]
    fn calendar_lists_round_trip() {
        let v = alloc::vec![
            CalendarInfo {
                name: "Personal".into(),
                colour: 0x22B46A,
                hidden: false,
                events: 3,
            },
            CalendarInfo {
                name: "Work".into(),
                colour: 0x3D7EFF,
                hidden: true,
                events: 0,
            },
        ];
        let b = encode_calendars(&v);
        assert_eq!(decode_calendars(&b), Ok(v));
        assert_eq!(decode_calendars(&b[..b.len() - 1]), Err(Corrupt));
        assert_eq!(decode_calendars(&encode_calendars(&[])), Ok(Vec::new()));
    }

    #[test]
    fn ring_infos_round_trip_with_long_texts() {
        let i = RingInfo {
            kind: ring::REMINDER,
            todo: false,
            all_day: false,
            ringing: true,
            start: 1_794_844_800,
            end: 1_794_848_400,
            colour: 0x22B46A,
            uid: "u".repeat(300),
            title: "Kernel review".into(),
            place: "Room 4 · Ünter".into(),
        };
        let b = i.encode();
        assert_eq!(RingInfo::decode(&b), Ok(i.clone()));
        assert_eq!(RingInfo::decode(&b[..b.len() - 1]), Err(Corrupt));
        let todo = RingInfo {
            todo: true,
            all_day: true,
            ringing: false,
            ..i
        };
        assert_eq!(RingInfo::decode(&todo.encode()), Ok(todo));
    }
}
