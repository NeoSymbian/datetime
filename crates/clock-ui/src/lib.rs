// SPDX-License-Identifier: GPL-3.0-only
//! Clock v2's screens (Slint, no_std): the generated component [`ClockApp`] and its globals (`ClockState`, `Theme`,
//! `Kit`, `Scroll`, `KeyboardFocus`). NEOCLOCK.EXE (symbian/clock) and the host snapshot tests drive the same UI.
//! [`ring`] draws the timer's ProgressRing, [`text`] words the times the screens show.
#![no_std]

extern crate alloc;

pub mod ring;
pub mod text;

slint::include_modules!();
