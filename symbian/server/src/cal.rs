// SPDX-License-Identifier: Apache-2.0
//! DTSRV's calendar (docs datetime.md DT3, DT5, DT6): the event and to-do stores (`calendar.ics`, `todo.ics`), the
//! zone they are read in, and the birthdays from NEOCONT's book.

use crate::store;
use alloc::string::String;
use alloc::vec::Vec;
use calendar_core::{Birthday, Store, Zone};
use e32::dprintln;
use neoui_proto::clock::tz;

/// The first run's calendar, "Personal".
const PERSONAL: u32 = 0x22B46A;

/// The phone's zone for the calendar: the kernel's offset while the network sets the zone (`time.auto_zone`), else
/// the rule of the `time.zone` city, so a summer-time change inside a query's range comes out right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Local {
    Kernel(i32),
    City(tz::Rule),
}

impl Zone for Local {
    fn offset_at(&self, utc: i64) -> i32 {
        match self {
            Local::Kernel(offset) => *offset,
            Local::City(rule) => rule.offset_at(utc),
        }
    }
}

/// The stores from their files. A first run (no `calendar.ics`) gets "Personal" and writes both; a file that isn't
/// an iCalendar is kept as `x.bad` and starts empty. `Err` when a file can't be read now (tried again later).
pub fn load(seed: u64) -> Result<Store, i32> {
    let (events, first) = text(store::EVENTS)?;
    let (todos, _) = text(store::TODOS)?;
    let mut s = Store::load(&events, &todos);
    s.seed = seed;
    if events.trim().is_empty() {
        s.set_calendar(0, "Personal", PERSONAL);
    }
    if first {
        let _ = save(&s, true, true);
    }
    dprintln!(
        "dtsrv: calendar: {} calendars, {} to-dos",
        s.calendars().len(),
        s.todos(&0).len()
    );
    Ok(s)
}

/// A store emptied as `which` says (Settings → Clear data), keeping the other.
pub fn cleared(s: &Store, events: bool, todos: bool) -> Store {
    let ev = if events {
        String::new()
    } else {
        s.calendars_text()
    };
    let td = if todos { String::new() } else { s.todos_text() };
    let mut new = Store::load(&ev, &td);
    new.seed = s.seed;
    if events {
        new.set_calendar(0, "Personal", PERSONAL);
    }
    new
}

pub fn save(s: &Store, events: bool, todos: bool) -> Result<(), i32> {
    if events {
        store::write(store::EVENTS, s.calendars_text().as_bytes())
            .inspect_err(|e| dprintln!("dtsrv: calendar.ics not saved ({})", e))?;
    }
    if todos {
        store::write(store::TODOS, s.todos_text().as_bytes())
            .inspect_err(|e| dprintln!("dtsrv: todo.ics not saved ({})", e))?;
    }
    Ok(())
}

/// A store file's text, and whether there was none (a first run). Text that isn't an iCalendar (not UTF-8, or no
/// VCALENDAR in it) is kept aside and reads as empty.
fn text(path: &str) -> Result<(String, bool), i32> {
    Ok(match store::read(path)? {
        Some(b) => match String::from_utf8(b) {
            Ok(t) if t.trim().is_empty() || has_calendar(&t) => (t, false),
            _ => {
                dprintln!("dtsrv: {} is corrupt: starting empty", path);
                store::keep_bad(path);
                (String::new(), false)
            }
        },
        None => (String::new(), true),
    })
}

fn has_calendar(t: &str) -> bool {
    t.as_bytes()
        .windows(15)
        .any(|w| w.eq_ignore_ascii_case(b"BEGIN:VCALENDAR"))
}

/// The birthdays in NEOCONT's book (none when there is no book or it can't be read).
pub fn birthdays() -> Vec<Birthday> {
    let book = match store::read(store::BOOK) {
        Ok(Some(b)) => contacts::Book::decode(&b),
        Ok(None) => None,
        Err(e) => {
            dprintln!("dtsrv: the contacts book not read ({})", e);
            None
        }
    };
    let v: Vec<Birthday> = book
        .as_ref()
        .map(|book| {
            contacts::birthdays(book)
                .map(|(contact_id, name, b)| Birthday {
                    contact_id,
                    name,
                    month: b.month,
                    day: b.day,
                    year: b.year.map(i32::from),
                })
                .collect()
        })
        .unwrap_or_default();
    dprintln!("dtsrv: birthdays {}", v.len());
    v
}
