// SPDX-License-Identifier: GPL-3.0-only
//! NEOCAL.EXE: Calendar v2 (NeoSymbian/datetime, docs datetime.md DT4–DT6). Month, Week, Day and To-do under one
//! footer, the event and its forms, birthdays, search, settings, Calendars, Add to Calendar (.ics) and a reminder's
//! Ringing. DTSRV owns the events and to-dos and rings the reminders; this app reads and changes them through dt-ipc
//! and redraws when DTSRV's WATCH says they changed.
//!
//! - [`cal`]: the state and every tap (`Cal.act`)
//! - `dt` (dt-client): DTSRV's client
//! - this file: connecting, and the event loop (input, pages, DTSRV's changes, the minute)
#![no_std]

extern crate alloc;

mod cal;

use alloc::rc::Rc;
use cal::Ctx;
use calendar_ui::{CalendarApp, Kit, Theme};
use core::pin::pin;
use dt_client as dt;
use e32::ipc::TIpcArgs;
use e32::rt::{self, Either};
use e32::sys::TPtr8;
use e32::{dprintln, time};
use neoapp::App;
use neoui_proto::InputEvent;
use neoui_proto::ipc::{self as proto, AppEvent};
use slint::ComponentHandle;

e32::main!(main);

fn main() -> i32 {
    match run() {
        Ok(()) => 0,
        Err(e) => {
            dprintln!("calendar: failed {}", e);
            e
        }
    }
}

const WATCHED: u32 = dt::mask::EVENTS | dt::mask::TODOS | dt::mask::RINGS;

/// DTSRV's next change: (the watch to go on with, the bits that changed). Without a watch (DTSRV not up, or gone):
/// a new one 5 s later, reported as everything changed.
async fn changes(w: Option<dt::Watch>) -> (Option<dt::Watch>, Result<u32, i32>) {
    match w {
        Some(w) => {
            let (w, r) = w.next().await;
            (Some(w), r)
        }
        None => {
            time::delay(5_000_000).await;
            let w = dt::Watch::new(WATCHED).ok();
            let r = if w.is_some() { Ok(u32::MAX) } else { Err(-1) };
            (w, r)
        }
    }
}

fn run() -> Result<(), i32> {
    let app = App::connect()?;
    dprintln!("calendar: surface {}x{}", app.width, app.height());
    let ui = CalendarApp::new().map_err(|_| -2)?;
    ui.global::<Theme>().set_dp(app.dp);
    ui.show().map_err(|_| -2)?;
    if let Err(e) = app.watch_settings("calendar.") {
        dprintln!("calendar: cannot watch settings ({})", e);
    }
    let ctx = Rc::new(Ctx::new(app, &ui));
    cal::bind(&ctx, &ui);
    ctx.start();
    ctx.sync()?;

    rt::block_on(async {
        let mut changed = pin!(changes(dt::Watch::new(WATCHED).ok()));
        loop {
            let mut ev = [0u8; proto::EVENT_LEN];
            let got = {
                let mut des = TPtr8::new(&mut ev);
                let mut args = TIpcArgs::new();
                args.des8(0, &mut des);
                // one NEXT_EVENT stays outstanding while DTSRV's changes and the minute come in
                let mut next = pin!(ctx.app.server.send(proto::NEXT_EVENT, &args));
                loop {
                    let wake = ctx.wake_us();
                    match rt::select(
                        next.as_mut(),
                        rt::select(changed.as_mut(), time::delay(wake)),
                    )
                    .await
                    {
                        Either::Left(r) => break r,
                        Either::Right(Either::Left((w, r))) => {
                            // a watch that failed (DTSRV went away) is dropped: the next try makes a new one
                            let ok = r.is_ok();
                            changed.set(changes(if ok { w } else { None }));
                            if let Ok(bits) = r {
                                ctx.changed(bits);
                                ctx.sync()?;
                            }
                        }
                        // the minute turned (the now line, "in 5 h 36 min"), or Slint's timers
                        Either::Right(Either::Right(())) => {
                            ctx.tick();
                            ctx.sync()?;
                        }
                    }
                }
            };
            got?;
            match AppEvent::decode(&ev) {
                Some(AppEvent::Quit) => {
                    dprintln!("calendar: quit");
                    return Ok(());
                }
                Some(e @ AppEvent::Visible(v)) => {
                    ctx.app.handle(&e);
                    ctx.visible(v);
                }
                Some(AppEvent::Open { page }) => {
                    let arg = ctx.app.arg().unwrap_or_default();
                    ctx.opened(page, &arg);
                }
                Some(AppEvent::NoteAction) => ctx.undo(),
                Some(AppEvent::Answer { i }) => ctx.answer(i),
                Some(AppEvent::Settings { .. }) => ctx.settings_changed(),
                // Settings → Apps → Calendar → Clear data: DTSRV empties the events and to-dos, the keys go back to
                // their defaults, and the app leaves
                Some(AppEvent::ClearData) => {
                    ctx.clear_data();
                    return Ok(());
                }
                // the kit's layers first (a sheet, the ⋮ menu)
                Some(AppEvent::Back) if neoapp::kit_back!(ctx.ui()) => {}
                Some(AppEvent::Back) => ctx.back(),
                Some(AppEvent::Keyboard { h }) => {
                    let ui = ctx.ui();
                    let kf = ui.global::<calendar_ui::KeyboardFocus>();
                    kf.set_dismissed(h == 0 && kf.get_open());
                    kf.set_text_h(h as f32);
                }
                Some(e @ AppEvent::Area { .. }) => ctx.app.handle(&e),
                Some(AppEvent::Input(input)) => match input {
                    InputEvent::Key { code, down: true } => ctx.app.dispatch_key(code),
                    InputEvent::Key { .. } => {}
                    touch => ctx.app.dispatch_touch(touch),
                },
                _ => {}
            }
            ctx.sync()?;
        }
    })
}
