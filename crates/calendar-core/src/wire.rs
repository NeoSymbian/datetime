// SPDX-License-Identifier: Apache-2.0
//! The dt-ipc records DTSRV and NEOCAL exchange, on `neoui_proto::clock::wire` (little-endian). Each blob starts with
//! [`VERSION`]; texts are `u16` length + UTF-8 (titles and notes can be longer than the codec's 255-byte strings).
//!
//! | Record | Fields after the version byte |
//! |---|---|
//! | instance list | u16 n, then n × Instance |
//! | Instance | text uid · i64 start · i64 end · i64 recur · u8 flags (1 all day, 2 reminder, 4 repeats) · u8 kind · u8 cal · u32 colour · u32 contact · u16 years · text title · text place |
//! | EventDraft | text uid · u8 has_only · i64 only · u8 cal · u8 kind · u8 all_day · i64 start · i64 end · text title · text place · text notes · repeat · u16 reminder (0xFFFF none) |
//! | repeat | u8 0 never, 2 other; 1 every: u8 freq (0 daily … 3 yearly) · u16 interval · u8 days · u16 count · u8 has_until · i64 until day |
//! | Todo / todo list | text uid · text title · text notes · u8 flags (1 done, 2 has due, 4 due all day, 8 has completed) · i64 due · u8 priority · i64 completed · u16 reminder (0xFFFF none) |

use crate::{EventDraft, Freq, Instance, Kind, Repeat, Todo};
use alloc::string::String;
use alloc::vec::Vec;
pub use neoui_proto::clock::wire::Corrupt;
use neoui_proto::clock::wire::{Reader, Writer};

pub const VERSION: u8 = 1;
const NONE16: u16 = u16::MAX;

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
    let n = r.u16()?;
    let mut b = Vec::with_capacity(usize::from(n));
    for _ in 0..n {
        b.push(r.u8()?);
    }
    String::from_utf8(b).map_err(|_| Corrupt)
}

fn kind(k: u8) -> Result<Kind, Corrupt> {
    match k {
        0 => Ok(Kind::Event),
        1 => Ok(Kind::Anniversary),
        2 => Ok(Kind::Birthday),
        _ => Err(Corrupt),
    }
}

fn opt16(v: Option<u16>) -> u16 {
    v.map_or(NONE16, |m| m.min(NONE16 - 1))
}

fn read_opt16(r: &mut Reader) -> Result<Option<u16>, Corrupt> {
    Ok(Some(r.u16()?).filter(|v| *v != NONE16))
}

impl Instance {
    pub fn put(&self, w: &mut Writer) {
        text(w, &self.uid);
        w.i64(self.start).i64(self.end).i64(self.recur);
        w.u8(u8::from(self.all_day) | u8::from(self.reminder) << 1 | u8::from(self.repeats) << 2);
        w.u8(self.kind as u8)
            .u8(self.cal)
            .u32(self.colour)
            .u32(self.contact)
            .u16(self.years);
        text(w, &self.title);
        text(w, &self.place);
    }

    pub fn get(r: &mut Reader) -> Result<Instance, Corrupt> {
        let uid = read_text(r)?;
        let (start, end, recur) = (r.i64()?, r.i64()?, r.i64()?);
        let flags = r.u8()?;
        Ok(Instance {
            uid,
            start,
            end,
            recur,
            all_day: flags & 1 != 0,
            reminder: flags & 2 != 0,
            repeats: flags & 4 != 0,
            kind: kind(r.u8()?)?,
            cal: r.u8()?,
            colour: r.u32()?,
            contact: r.u32()?,
            years: r.u16()?,
            title: read_text(r)?,
            place: read_text(r)?,
        })
    }
}

/// An EVENT_QUERY reply (at most 65 535 instances).
pub fn encode_instances(v: &[Instance]) -> Vec<u8> {
    let mut w = Writer::new(VERSION);
    let n = v.len().min(usize::from(u16::MAX));
    w.u16(n as u16);
    for i in &v[..n] {
        i.put(&mut w);
    }
    w.finish()
}

pub fn decode_instances(b: &[u8]) -> Result<Vec<Instance>, Corrupt> {
    let mut r = Reader::new(b, VERSION)?;
    let n = r.u16()?;
    let v = (0..n)
        .map(|_| Instance::get(&mut r))
        .collect::<Result<_, _>>()?;
    r.end()?;
    Ok(v)
}

fn put_repeat(w: &mut Writer, rep: &Repeat) {
    match rep {
        Repeat::Never => {
            w.u8(0);
        }
        Repeat::Other => {
            w.u8(2);
        }
        &Repeat::Every {
            freq,
            interval,
            days,
            count,
            until,
        } => {
            let f = match freq {
                Freq::Daily => 0,
                Freq::Weekly => 1,
                Freq::Monthly => 2,
                Freq::Yearly => 3,
            };
            w.u8(1).u8(f).u16(interval).u8(days).u16(count);
            w.u8(u8::from(until.is_some())).i64(until.unwrap_or(0));
        }
    }
}

fn get_repeat(r: &mut Reader) -> Result<Repeat, Corrupt> {
    Ok(match r.u8()? {
        0 => Repeat::Never,
        2 => Repeat::Other,
        1 => {
            let freq = match r.u8()? {
                0 => Freq::Daily,
                1 => Freq::Weekly,
                2 => Freq::Monthly,
                3 => Freq::Yearly,
                _ => return Err(Corrupt),
            };
            let (interval, days, count) = (r.u16()?, r.u8()?, r.u16()?);
            let has = r.u8()? != 0;
            let until = r.i64()?;
            Repeat::Every {
                freq,
                interval,
                days,
                count,
                until: has.then_some(until),
            }
        }
        _ => return Err(Corrupt),
    })
}

impl EventDraft {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new(VERSION);
        text(&mut w, &self.uid);
        w.u8(u8::from(self.only.is_some()))
            .i64(self.only.unwrap_or(0));
        w.u8(self.cal)
            .u8(self.kind as u8)
            .u8(u8::from(self.all_day));
        w.i64(self.start).i64(self.end);
        text(&mut w, &self.title);
        text(&mut w, &self.place);
        text(&mut w, &self.notes);
        put_repeat(&mut w, &self.repeat);
        w.u16(opt16(self.reminder));
        w.finish()
    }

    pub fn decode(b: &[u8]) -> Result<EventDraft, Corrupt> {
        let mut r = Reader::new(b, VERSION)?;
        let uid = read_text(&mut r)?;
        let has = r.u8()? != 0;
        let only = r.i64()?;
        let d = EventDraft {
            uid,
            only: has.then_some(only),
            cal: r.u8()?,
            kind: kind(r.u8()?)?,
            all_day: r.u8()? != 0,
            start: r.i64()?,
            end: r.i64()?,
            title: read_text(&mut r)?,
            place: read_text(&mut r)?,
            notes: read_text(&mut r)?,
            repeat: get_repeat(&mut r)?,
            reminder: read_opt16(&mut r)?,
        };
        r.end()?;
        Ok(d)
    }
}

impl Todo {
    pub fn put(&self, w: &mut Writer) {
        text(w, &self.uid);
        text(w, &self.title);
        text(w, &self.notes);
        w.u8(u8::from(self.done)
            | u8::from(self.due.is_some()) << 1
            | u8::from(self.due_all_day) << 2
            | u8::from(self.completed.is_some()) << 3);
        w.i64(self.due.unwrap_or(0)).u8(self.priority);
        w.i64(self.completed.unwrap_or(0)).u16(opt16(self.reminder));
    }

    pub fn get(r: &mut Reader) -> Result<Todo, Corrupt> {
        let (uid, title, notes) = (read_text(r)?, read_text(r)?, read_text(r)?);
        let flags = r.u8()?;
        let due = r.i64()?;
        let priority = r.u8()?;
        let completed = r.i64()?;
        Ok(Todo {
            uid,
            title,
            notes,
            done: flags & 1 != 0,
            due: (flags & 2 != 0).then_some(due),
            due_all_day: flags & 4 != 0,
            priority,
            completed: (flags & 8 != 0).then_some(completed),
            reminder: read_opt16(r)?,
        })
    }

    /// TODO_PUT's request.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new(VERSION);
        self.put(&mut w);
        w.finish()
    }

    pub fn decode(b: &[u8]) -> Result<Todo, Corrupt> {
        let mut r = Reader::new(b, VERSION)?;
        let t = Todo::get(&mut r)?;
        r.end()?;
        Ok(t)
    }
}

/// TODO_LIST's reply (at most 65 535 to-dos).
pub fn encode_todos(v: &[Todo]) -> Vec<u8> {
    let mut w = Writer::new(VERSION);
    let n = v.len().min(usize::from(u16::MAX));
    w.u16(n as u16);
    for t in &v[..n] {
        t.put(&mut w);
    }
    w.finish()
}

pub fn decode_todos(b: &[u8]) -> Result<Vec<Todo>, Corrupt> {
    let mut r = Reader::new(b, VERSION)?;
    let n = r.u16()?;
    let v = (0..n)
        .map(|_| Todo::get(&mut r))
        .collect::<Result<_, _>>()?;
    r.end()?;
    Ok(v)
}
