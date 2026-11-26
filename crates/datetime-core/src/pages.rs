// SPDX-License-Identifier: Apache-2.0
//! The page numbers of Clock and Calendar: what `OPEN(key, page, arg)` and a card's `page` name (protocol 1.9). DTSRV
//! opens [`clock::RING`] with the ring id as the arg.

/// NEOCLOCK's pages.
pub mod clock {
    pub const ROOT: u16 = 0;
    pub const ALARMS: u16 = 1;
    /// arg: the alarm id
    pub const ALARM: u16 = 2;
    pub const TIMER: u16 = 3;
    pub const STOPWATCH: u16 = 4;
    /// arg: the ring id in decimal ([`crate::ipc::ring`])
    pub const RING: u16 = 5;
    pub const SETTINGS: u16 = 6;
    pub const CITIES: u16 = 7;
    pub const SET_TIME: u16 = 8;
}

/// NEOCAL's pages.
pub mod calendar {
    /// arg: the month, "2026-11"
    pub const MONTH: u16 = 0;
    pub const WEEK: u16 = 1;
    /// arg: the day, "2026-11-16"
    pub const DAY: u16 = 2;
    pub const TODO: u16 = 3;
    /// arg: the event's UID
    pub const EVENT: u16 = 4;
    /// arg: the day the new event starts on
    pub const NEW: u16 = 5;
    /// the .ics file comes with `OPEN_FILE` (protocol 1.12)
    pub const IMPORT: u16 = 6;
    /// arg: the ring id in decimal
    pub const RING: u16 = 7;
    pub const SETTINGS: u16 = 8;
}
