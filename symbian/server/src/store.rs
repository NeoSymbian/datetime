// SPDX-License-Identifier: Apache-2.0
//! DTSRV's files in its private directory: `alarms.dat` (`Alarms::encode`), `timers.dat` (`Timer::encode`),
//! `calendar.ics` (one VCALENDAR per calendar) and `todo.ics`. Each is written as `x.new`, then swapped in with one
//! `RFs::Replace`, so a cut leaves the old file or the new one. It also reads NEOCONT's book for the birthdays.

use alloc::vec;
use alloc::vec::Vec;
use e32::fs::{File, Fs, mode};
use e32::sys::err;

const DIRS: [&str; 2] = ["C:\\private\\", "C:\\private\\e0008003\\"];
pub const ALARMS: &str = "C:\\private\\e0008003\\alarms.dat";
pub const TIMERS: &str = "C:\\private\\e0008003\\timers.dat";
/// What rang and what still rings (`datetime_core::plan::Rung`).
pub const RINGS: &str = "C:\\private\\e0008003\\rings.dat";
pub const EVENTS: &str = "C:\\private\\e0008003\\calendar.ics";
pub const TODOS: &str = "C:\\private\\e0008003\\todo.ics";
/// NEOCONT's book (phone's `contacts` crate), only read here.
pub const BOOK: &str = "C:\\private\\e0005001\\contacts.dat";
/// Larger than any file DTSRV reads (16 alarms are under 600 bytes; years of events and a book of 1000 full contacts
/// are well under this).
const FILE_MAX: usize = 2 << 20;

/// The file's bytes; `Ok(None)` when there is none yet (`KErrNotFound`, or no private directory yet:
/// `KErrPathNotFound`).
pub fn read(path: &str) -> Result<Option<Vec<u8>>, i32> {
    let fs = Fs::connect()?;
    let f = match File::open(&fs, path, mode::READ | mode::SHARE_READERS_ONLY) {
        Ok(f) => f,
        Err(err::KERR_NOT_FOUND | err::KERR_PATH_NOT_FOUND) => return Ok(None),
        Err(e) => return Err(e),
    };
    let size = usize::try_from(f.size()?).unwrap_or(usize::MAX);
    if size > FILE_MAX {
        return Err(-40); // KErrTooBig
    }
    let mut b = vec![0u8; size];
    let n = f.read_at(0, &mut b)?;
    b.truncate(n);
    Ok(Some(b))
}

pub fn write(path: &str, bytes: &[u8]) -> Result<(), i32> {
    let fs = Fs::connect()?;
    for d in DIRS {
        match fs.mkdir(d) {
            Ok(()) | Err(err::KERR_ALREADY_EXISTS) => {}
            Err(e) => return Err(e),
        }
    }
    // "x.dat" -> "x.new"
    let mut new = alloc::string::String::from(&path[..path.len() - 3]);
    new.push_str("new");
    {
        let f = File::replace(&fs, &new, mode::WRITE | mode::SHARE_EXCLUSIVE)?;
        f.write(bytes)?;
        f.flush()?;
    }
    fs.replace_file(&new, path)
}

/// Moves an unreadable `x.ics` aside as `x.bad` (one kept), so a fresh store never overwrites it.
pub fn keep_bad(path: &str) {
    let Ok(fs) = Fs::connect() else { return };
    let mut bad = alloc::string::String::from(&path[..path.len() - 3]);
    bad.push_str("bad");
    let _ = fs.delete(&bad);
    let r = fs.rename(path, &bad);
    e32::dprintln!("dtsrv: {} kept as {} ({:?})", path, bad, r);
}
