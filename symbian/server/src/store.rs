// SPDX-License-Identifier: Apache-2.0
//! DTSRV's files in its private directory: `alarms.dat` (`Alarms::encode`) and `timers.dat` (`Timer::encode`). Each
//! is written as `x.new`, then swapped in with one `RFs::Replace`, so a cut leaves the old file or the new one.

use alloc::vec;
use alloc::vec::Vec;
use e32::fs::{File, Fs, mode};
use e32::sys::err;

const DIRS: [&str; 2] = ["C:\\private\\", "C:\\private\\e0008003\\"];
pub const ALARMS: &str = "C:\\private\\e0008003\\alarms.dat";
pub const TIMERS: &str = "C:\\private\\e0008003\\timers.dat";
/// Larger than any store DTSRV writes (16 alarms are under 600 bytes).
const FILE_MAX: usize = 64 << 10;

/// The file's bytes; `Ok(None)` when there is none yet.
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
