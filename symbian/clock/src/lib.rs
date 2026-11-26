// SPDX-License-Identifier: GPL-3.0-only
//! NEOCLOCK.EXE (NeoSymbian/datetime, Clock v2 DT2): three tabs, Clock · Alarms · Timers, drawn by `clock-ui` into the
//! surface the UI server hands over ([`neoapp`]). The alarms and the timer are DTSRV's (DT1): the app reads them with
//! dt-client and redraws when DTSRV's WATCH says they changed; DTSRV rings them and opens the RING page here. The
//! stopwatch, the cities and the settings are the app's own `clock.*` keys.
//!
//! - [`ctx`]: the model and the shared moves; [`show`]: the model onto the screens; [`bind`]: the taps
//! - [`ring`]: Ringing; [`sound`]: its tone
//! - this file: connecting, and the event loop (the UI server's events, DTSRV's WATCH, the ticks)
#![no_std]

extern crate alloc;

mod bind;
mod ctx;
mod ring;
mod show;
mod sound;

use alloc::rc::Rc;
use clock_ui::{ClockApp, Kit, Theme};
use core::pin::pin;
use ctx::{Ctx, Model, Now, Tab, View};
use datetime_core::pages::clock as page;
use dt_client::{Watch, mask};
use e32::ipc::TIpcArgs;
use e32::rt::{self, Either};
use e32::sys::TPtr8;
use e32::{dprintln, time};
use neoui_proto::ipc::{self as proto, AppEvent};
use neoui_proto::{InputEvent, TouchPhase};
use slint::ComponentHandle;

e32::main!(main);

/// How often the face is checked for a new second.
const TICK_US: i32 = 250_000;
/// While the stopwatch runs on screen: hundredths move.
const FAST_TICK_US: i32 = 50_000;
/// Hidden, or DTSRV not there yet: how long before trying again.
const RETRY_US: i32 = 5_000_000;

fn main() -> i32 {
    match run() {
        Ok(()) => 0,
        Err(e) => {
            dprintln!("clock: failed {}", e);
            e
        }
    }
}

/// The next WATCH from DTSRV; without one, a new watch after [`RETRY_US`] (DTSRV starts at boot, and again when it
/// exits). `Ok(bits)`: what changed (all of it after a new watch).
async fn watch(w: Option<Watch>) -> (Option<Watch>, Result<u32, i32>) {
    match w {
        Some(w) => {
            let (w, r) = w.next().await;
            // a dead server's watch is dropped; the next round makes a new one
            (r.is_ok().then_some(w), r)
        }
        None => {
            time::delay(RETRY_US).await;
            match Watch::new(mask::ALL) {
                Ok(w) => (Some(w), Ok(mask::ALL)),
                Err(e) => (None, Err(e)),
            }
        }
    }
}

fn run() -> Result<(), i32> {
    let app = neoapp::App::connect()?;
    dprintln!("clock: surface {}x{} mapped", app.width, app.height());
    let ui = ClockApp::new().map_err(|_| -2)?;
    ui.global::<Theme>().set_dp(app.dp);
    ui.show().map_err(|_| -2)?;
    for prefix in ["clock.", "time."] {
        if let Err(e) = app.watch_settings(prefix) {
            dprintln!("clock: cannot watch {} ({})", prefix, e);
        }
    }
    let model = Model::load(&app);
    let ctx = Rc::new(Ctx::new(app, &ui, model));
    bind::bind(&ctx, &ui);
    ctx.reload();
    show::all(&ctx, &Now::read());
    ctx.sync()?;
    let first = Watch::new(mask::ALL)
        .inspect_err(|e| dprintln!("clock: DTSRV not watched yet ({})", e))
        .ok();

    let mut shown_second = u32::MAX;
    let mut fast = false;
    rt::block_on(async {
        let mut changes = pin!(watch(first));
        loop {
            let mut ev = [0u8; proto::EVENT_LEN];
            let got = {
                let mut des = TPtr8::new(&mut ev);
                let mut args = TIpcArgs::new();
                args.des8(0, &mut des);
                // one NEXT_EVENT stays outstanding while DTSRV's changes and the ticks come in
                let mut next = pin!(ctx.app.server.send(proto::NEXT_EVENT, &args));
                loop {
                    let own = if !ctx.m.borrow().visible {
                        RETRY_US
                    } else if fast {
                        FAST_TICK_US
                    } else {
                        TICK_US
                    };
                    let slint = ctx
                        .app
                        .wake_us()
                        .map_or(i32::MAX, |us| i32::try_from(us).unwrap_or(i32::MAX));
                    let wait = own.min(slint).max(1000);
                    match rt::select(
                        next.as_mut(),
                        rt::select(changes.as_mut(), time::delay(wait)),
                    )
                    .await
                    {
                        Either::Left(r) => break r,
                        Either::Right(Either::Left((w, bits))) => {
                            changes.set(watch(w));
                            if let Ok(bits) = bits {
                                ctx.reload();
                                ring::watched(&ctx, bits);
                                show::all(&ctx, &Now::read());
                                ctx.sync()?;
                            }
                        }
                        Either::Right(Either::Right(())) => {
                            if ctx.m.borrow().visible {
                                let now = Now::read();
                                if fast || now.second != shown_second {
                                    if now.second != shown_second {
                                        dprintln!(
                                            "clock: tick {:02}:{:02}:{:02}",
                                            now.local.hour,
                                            now.local.minute,
                                            now.second
                                        );
                                    }
                                    shown_second = now.second;
                                    fast = show::tick(&ctx, &now);
                                }
                            }
                            ctx.app.redraw()?;
                        }
                    }
                }
            };
            got?;
            let Some(e) = AppEvent::decode(&ev) else {
                continue;
            };
            ctx.app.handle(&e);
            match e {
                AppEvent::Quit => {
                    dprintln!("clock: quit");
                    sound::stop();
                    return Ok(());
                }
                AppEvent::Visible(v) => {
                    dprintln!("clock: visible {}", v);
                    ctx.m.borrow_mut().visible = v;
                    // redraw at once when shown again
                    shown_second = u32::MAX;
                    ctx.ring_drawn.set((-1, false));
                    if v {
                        ctx.reload();
                        show::all(&ctx, &Now::read());
                    }
                }
                AppEvent::Settings { seq } => {
                    ctx.m.borrow_mut().reload_settings(&ctx.app);
                    dprintln!("clock: settings changed ({})", seq);
                    show::all(&ctx, &Now::read());
                }
                // DTSRV (a ring, a card's tap), the shell's page links, another app
                AppEvent::Open { page: p } | AppEvent::Page { page: p } => {
                    let arg = ctx.app.arg().unwrap_or_default();
                    opened(&ctx, p, &arg);
                }
                // Settings → Apps → Clock → Clear data: DTSRV's alarms and timer, then every key of Clock's
                AppEvent::ClearData => {
                    clear(&ctx);
                    return Ok(());
                }
                // the shell's dialog: Remove the city?
                AppEvent::Answer { i } => {
                    let pos = ctx.m.borrow_mut().removing.take();
                    if let (0, Some(pos)) = (i, pos) {
                        let removed = ctx.m.borrow_mut().cities.remove(pos);
                        if removed {
                            dprintln!("clock: city {} removed", pos);
                            ctx.save_cities();
                        }
                    }
                    show::all(&ctx, &Now::read());
                }
                // the kit's layers first (⋮, the sheets)
                AppEvent::Back if neoapp::kit_back!(ctx.ui()) => {}
                AppEvent::Back => back(&ctx),
                AppEvent::Keyboard { h } => {
                    let ui = ctx.ui();
                    let kf = ui.global::<clock_ui::KeyboardFocus>();
                    kf.set_dismissed(h == 0 && kf.get_open());
                    kf.set_text_h(h as f32);
                }
                AppEvent::Area { .. } => show::all(&ctx, &Now::read()),
                AppEvent::Input(InputEvent::Touch {
                    x,
                    y,
                    phase: TouchPhase::Up,
                    ..
                }) => dprintln!("clock: touch {},{}", x, y),
                _ => {}
            }
            ctx.sync()?;
        }
    })
}

/// Back: Ringing as its rule says, a view closes, the Alarms and Timers tabs go to the Clock tab (tab history).
fn back(ctx: &Ctx) {
    let (view, tab) = {
        let m = ctx.m.borrow();
        (m.view, m.tab)
    };
    if view != View::None {
        bind::close(ctx);
    } else if tab != Tab::Clock {
        bind::tab(ctx, Tab::Clock, None);
        show::all(ctx, &Now::read());
    }
}

/// `OPEN clock` on `p` (`datetime_core::pages::clock`) with `arg`.
fn opened(ctx: &Ctx, p: u16, arg: &str) {
    dprintln!("clock: opened on page {} ({})", p, arg);
    match p {
        page::RING => match arg.parse::<u32>() {
            Ok(id) => ring::start(ctx, id),
            Err(_) => dprintln!("clock: ring page without a ring id"),
        },
        page::ALARMS => bind::tab(ctx, Tab::Alarms, None),
        page::ALARM => {
            bind::tab(ctx, Tab::Alarms, None);
            if let Ok(id) = arg.parse::<u16>() {
                ctx.reload();
                bind::edit(ctx, id);
            }
        }
        page::TIMER => bind::tab(ctx, Tab::Timers, Some(false)),
        page::STOPWATCH => bind::tab(ctx, Tab::Timers, Some(true)),
        page::SETTINGS => ctx.open(View::Settings),
        page::CITIES => ctx.open(View::Cities),
        page::SET_TIME => bind::set_time(ctx),
        _ => bind::tab(ctx, Tab::Clock, None),
    }
    show::all(ctx, &Now::read());
}

/// Clear data: DTSRV empties the alarms and the timer, the app's keys go back to their defaults.
fn clear(ctx: &Ctx) {
    sound::stop();
    if let Err(e) = ctx.dt("CLEAR", |c| c.clear(mask::ALARMS | mask::TIMERS)) {
        dprintln!("clock: DTSRV's alarms and timer not cleared ({})", e);
    }
    let (mut n, mut refused) = (0, 0);
    for spec in neoui_proto::settings::registry::cleared_with("clock") {
        n += 1;
        if let Err(e) = ctx.app.set_setting_raw(spec.name, &spec.default.to_value()) {
            dprintln!("clock: clearing {} refused ({})", spec.name, e);
            refused += 1;
        }
    }
    dprintln!("clock: data cleared ({} keys, {} refused)", n, refused);
}
