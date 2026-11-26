// SPDX-License-Identifier: Apache-2.0
//! The `dtsrv.next` setting (and NEXT's reply): the next alarm and the next event, which the shell's home widget, the
//! status bar and the agenda card show. DTSRV writes it whenever it changes; neoui-proto decodes the same layout.
//!
//! Layout (wire format 1): `i64 alarm_at, str alarm_label, i64 event_at, str event_title, u32 event_colour, u8
//! event_all_day`; times are Unix UTC seconds, 0 = none.

use alloc::string::String;
use alloc::vec::Vec;
use neoui_proto::clock::wire::{Corrupt, Reader, Writer};

const VERSION: u8 = 1;

/// What rings next.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Next {
    /// Unix UTC seconds; 0 = no alarm on.
    pub alarm_at: i64,
    /// "" = no label.
    pub alarm_label: String,
    /// Unix UTC seconds; 0 = no event ahead.
    pub event_at: i64,
    pub event_title: String,
    /// 0xRRGGBB, the event's calendar colour.
    pub event_colour: u32,
    pub event_all_day: bool,
}

impl Next {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new(VERSION);
        w.i64(self.alarm_at)
            .str(&self.alarm_label)
            .i64(self.event_at)
            .str(&self.event_title)
            .u32(self.event_colour)
            .u8(u8::from(self.event_all_day));
        w.finish()
    }

    /// # Errors
    /// [`Corrupt`] for a damaged blob or another version.
    pub fn decode(b: &[u8]) -> Result<Self, Corrupt> {
        let mut r = Reader::new(b, VERSION)?;
        let next = Self {
            alarm_at: r.i64()?,
            alarm_label: r.str()?,
            event_at: r.i64()?,
            event_title: r.str()?,
            event_colour: r.u32()?,
            event_all_day: match r.u8()? {
                0 => false,
                1 => true,
                _ => return Err(Corrupt),
            },
        };
        r.end()?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn the_blob_is_pinned_and_round_trips() {
        let n = Next {
            alarm_at: 1_794_814_200, // 2026-11-16 07:30 UTC
            alarm_label: "Gym".into(),
            event_at: 0,
            event_title: String::new(),
            event_colour: 0x3D7EFF,
            event_all_day: false,
        };
        let b = n.encode();
        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
        // neoui-proto's decoder is tested against these bytes
        assert_eq!(
            hex,
            "01f8b0fa6a000000000347796d000000000000000000ff7e3d0000"
        );
        assert_eq!(Next::decode(&b), Ok(n));
        assert_eq!(Next::decode(&b[..b.len() - 1]), Err(Corrupt));
        assert_eq!(Next::decode(&Next::default().encode()), Ok(Next::default()));
    }
}
