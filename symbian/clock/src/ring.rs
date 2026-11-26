// SPDX-License-Identifier: GPL-3.0-only
//! Ringing (dt2-ring-alarm, dt2-ring-timer): DTSRV opens Clock on its RING page with the ring's id (protocol 1.19
//! OPEN with WAKE). The screen shows the alarm or the timer and plays the tone; its buttons, and Back, go to DTSRV as
//! RING_ACTION (Back = Snooze for an alarm, Stop for the timer: dt2-spec-map). A ring that ends elsewhere (the shade
//! card's buttons) closes the screen when the WATCH says so.

use crate::ctx::{Ctx, View};
use crate::{show, sound};
use clock_ui::ClockState;
use dt_client::{action, mask, ring};
use e32::dprintln;
use slint::ComponentHandle;

fn is_timer(r: u32) -> bool {
    ring::kind(r) == ring::TIMER
}

/// Shows ring `id` (the RING page's arg).
pub fn start(ctx: &Ctx, id: u32) {
    ctx.reload();
    let tone = {
        let mut m = ctx.m.borrow_mut();
        if m.view != View::Ringing {
            m.before_ring = m.view;
        }
        m.ring = Some(id);
        show::tone(&m.sound)
    };
    dprintln!("clock: ring {}", id);
    // the sheets are drawn over every view: none may cover the ring
    let ui = ctx.ui();
    let s = ui.global::<ClockState>();
    s.set_time_sheet(false);
    s.set_snooze_sheet(false);
    s.set_date_sheet(false);
    s.set_clock_sheet(false);
    ctx.open(View::Ringing);
    sound::play(tone, true);
}

/// Leaves Ringing for the view it covered.
fn close(ctx: &Ctx) {
    sound::stop();
    let before = {
        let mut m = ctx.m.borrow_mut();
        m.ring = None;
        m.before_ring
    };
    ctx.open(before);
}

/// RING_ACTION `n` on the ring shown, then back to the app.
fn act(ctx: &Ctx, n: i32) {
    let Some(r) = ctx.m.borrow().ring else { return };
    dprintln!("clock: ring action {} {}", r, n);
    // KErrNotFound: it ended meanwhile (the shade); either way the screen goes
    let _ = ctx.dt("RING_ACTION", |c| c.ring_action(r, n));
    ctx.reload();
    close(ctx);
}

/// Snooze, or the timer's +1 min.
pub fn first(ctx: &Ctx) {
    let timer = ctx.m.borrow().ring.is_some_and(is_timer);
    act(
        ctx,
        if timer {
            action::PLUS_MINUTE
        } else {
            action::SNOOZE
        },
    );
}

/// Dismiss, or the timer's Stop.
pub fn second(ctx: &Ctx) {
    let timer = ctx.m.borrow().ring.is_some_and(is_timer);
    act(ctx, if timer { action::STOP } else { action::DISMISS });
}

/// Back while ringing: Snooze an alarm, Stop the timer.
pub fn back(ctx: &Ctx) {
    let timer = ctx.m.borrow().ring.is_some_and(is_timer);
    act(ctx, if timer { action::STOP } else { action::SNOOZE });
}

/// After a WATCH with `bits` (the stores already read again): closes the screen when its ring ended elsewhere.
// shortcut: dt-ipc has no "is this ringing" query, so an alarm's ring counts as ended when the alarms changed with
// the rings (a card's Snooze or Dismiss, a delete); a second alarm acted on from the shade closes the first one's
// screen too. Add a RINGS list op to DTSRV if that matters.
pub fn watched(ctx: &Ctx, bits: u32) {
    let ended = {
        let m = ctx.m.borrow();
        match m.ring {
            Some(r) if m.view == View::Ringing && bits & mask::RINGS != 0 => {
                if is_timer(r) {
                    m.timer.phase != datetime_core::timer::Phase::Done
                } else {
                    bits & mask::ALARMS != 0
                }
            }
            _ => false,
        }
    };
    if ended {
        dprintln!("clock: ring ended elsewhere");
        close(ctx);
    }
}
