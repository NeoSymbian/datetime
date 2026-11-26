// SPDX-License-Identifier: Apache-2.0
//! The Clock and Calendar model shared by NEOCLOCK, NEOCAL and DTSRV (docs datetime.md): pure, no clock or file
//! system, host-tested. Moved from neoui-proto's `clock` (DT0); time zones, world cities, the face text and the
//! small byte codec stay there, because the shell uses them too.
//!
//! | Module | Owns |
//! |---|---|
//! | [`alarm`] | alarms: weekdays, next due, snooze, and their blob |
//! | [`analog`] | the analog face drawn into pixels (dial once, hands each second) |
//! | [`timer`] | the countdown timer's state machine |
//! | [`stopwatch`] | the stopwatch and its laps |
#![no_std]

extern crate alloc;

pub mod alarm;
pub mod analog;
pub mod stopwatch;
pub mod timer;
