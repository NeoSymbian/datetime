// SPDX-License-Identifier: GPL-3.0-only
//! Calendar's screens (Slint, no_std): the generated component [`CalendarApp`] and its global `Cal`, and [`view`],
//! which turns calendar-core's instances and to-dos into what the screens show. NEOCAL.EXE (symbian/calendar) and the
//! host snapshot tests drive the same UI through the same code.
#![no_std]

extern crate alloc;

pub mod view;

slint::include_modules!();
