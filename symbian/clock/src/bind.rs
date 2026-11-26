// SPDX-License-Identifier: GPL-3.0-only
//! The screens' callbacks: what each tap does. Alarms and the timer go to DTSRV (dt-client) and come back through
//! [`Ctx::reload`]; the settings, the cities and the stopwatch are the app's own (`clock.*`).

use crate::ctx::{Ctx, Draft, Now, Tab, View};
use crate::{ring, show, sound};
use alloc::rc::Rc;
use alloc::string::String;
use clock_ui::{ClockApp, ClockState};
use datetime_core::alarm::{Alarm, LABEL_MAX, Weekdays};
use datetime_core::timer::{MAX_MS, MIN_MS};
use dt_client::timer as op;
use e32::{dprintln, time};
use neoui_proto::clock::Local;
use neoui_proto::clock::world::{AddError, CATALOGUE};
use neoui_proto::settings::keys;
use slint::ComponentHandle;

/// After a callback: show the model and draw.
fn after(ctx: &Ctx) {
    show::all(ctx, &Now::read());
    let _ = ctx.sync();
}

/// A callback without arguments that runs `f` with the context, then [`after`].
fn on0(ctx: &Rc<Ctx>, f: impl Fn(&Ctx) + 'static) -> impl Fn() + 'static {
    let weak = Rc::downgrade(ctx);
    move || {
        if let Some(ctx) = weak.upgrade() {
            f(&ctx);
            after(&ctx);
        }
    }
}

/// [`on0`] with one argument.
fn on<A>(ctx: &Rc<Ctx>, f: impl Fn(&Ctx, A) + 'static) -> impl Fn(A) + 'static {
    let weak = Rc::downgrade(ctx);
    move |a| {
        if let Some(ctx) = weak.upgrade() {
            f(&ctx, a);
            after(&ctx);
        }
    }
}

/// [`on0`] with two arguments.
fn on2<A, B>(ctx: &Rc<Ctx>, f: impl Fn(&Ctx, A, B) + 'static) -> impl Fn(A, B) + 'static {
    let weak = Rc::downgrade(ctx);
    move |a, b| {
        if let Some(ctx) = weak.upgrade() {
            f(&ctx, a, b);
            after(&ctx);
        }
    }
}

/// [`on0`] with three arguments.
fn on3<A, B, C>(ctx: &Rc<Ctx>, f: impl Fn(&Ctx, A, B, C) + 'static) -> impl Fn(A, B, C) + 'static {
    let weak = Rc::downgrade(ctx);
    move |a, b, c| {
        if let Some(ctx) = weak.upgrade() {
            f(&ctx, a, b, c);
            after(&ctx);
        }
    }
}

/// Leaves the view: Alarm sound back to where it came from, Ringing as its Back does, the rest to the tabs.
pub fn close(ctx: &Ctx) {
    let (view, from) = {
        let m = ctx.m.borrow();
        (m.view, m.sound_from)
    };
    match view {
        View::Sound => {
            sound::stop();
            ctx.open(from);
        }
        View::Ringing => ring::back(ctx),
        View::Alarm => {
            ctx.m.borrow_mut().draft = None;
            ctx.open(View::None);
        }
        _ => ctx.open(View::None),
    }
}

/// The tab `t`, saved in `clock.tab`.
pub fn tab(ctx: &Ctx, t: Tab, stopwatch: Option<bool>) {
    let n = {
        let mut m = ctx.m.borrow_mut();
        m.tab = t;
        if let Some(w) = stopwatch {
            m.stopwatch_shown = w;
        }
        m.tab_number()
    };
    dprintln!("clock: tab {}", n);
    ctx.save(keys::CLOCK_TAB, n);
}

/// The editor on alarm `id`, or on a new one (0) at the next whole hour.
pub fn edit(ctx: &Ctx, id: u16) {
    let draft = if id == 0 {
        let now = Now::read();
        Some(Alarm {
            id: 0,
            // under 24
            hour: ((now.local.hour + 1) % 24) as u8,
            minute: 0,
            days: Weekdays::ONCE,
            label: String::new(),
            on: true,
        })
    } else {
        ctx.m.borrow().alarms.get(id).cloned()
    };
    let Some(a) = draft else { return };
    ctx.ui()
        .global::<ClockState>()
        .set_edit_label(a.label.as_str().into());
    ctx.m.borrow_mut().draft = Some(Draft(a));
    ctx.open(View::Alarm);
}

/// Writes alarm `a` to DTSRV (id 0 adds it) and reads the list back.
fn put(ctx: &Ctx, a: &Alarm) -> bool {
    let r = ctx.dt("ALARM_PUT", |c| c.put_alarm(a));
    if let Ok(id) = r {
        dprintln!(
            "clock: alarm {} saved {:02}:{:02} on {}",
            id,
            a.hour,
            a.minute,
            a.on
        );
    }
    ctx.reload();
    r.is_ok()
}

/// `label` cut to [`LABEL_MAX`] bytes at a character boundary.
fn fit(label: &str) -> String {
    let t = label.trim();
    let mut end = t.len().min(LABEL_MAX);
    while !t.is_char_boundary(end) {
        end -= 1;
    }
    t[..end].into()
}

/// The idle timer's duration moved by `delta_ms`, kept in DTSRV's range.
fn step(ctx: &Ctx, delta_ms: i64) {
    let mut m = ctx.m.borrow_mut();
    // MIN_MS..=MAX_MS fits u32
    m.timer_ms =
        (i64::from(m.timer_ms) + delta_ms).clamp(i64::from(MIN_MS), i64::from(MAX_MS)) as u32;
}

/// A TIMER op on DTSRV; the timer comes back with it.
fn timer(ctx: &Ctx, which: i32, value: i32) {
    if let Ok(t) = ctx.dt("TIMER", |c| c.timer(which, value)) {
        dprintln!("clock: timer op {} {} -> {:?}", which, value, t.phase);
        ctx.m.borrow_mut().timer = t;
        ctx.ring_drawn.set((-1, false));
    }
}

pub fn bind(ctx: &Rc<Ctx>, ui: &ClockApp) {
    let s = ui.global::<ClockState>();

    // navigation
    s.on_tab_chosen(on(ctx, |ctx, i| {
        let t = match i {
            1 => Tab::Alarms,
            2 => Tab::Timers,
            _ => Tab::Clock,
        };
        tab(ctx, t, None);
    }));
    s.on_timers_chosen(on(ctx, |ctx, i| tab(ctx, Tab::Timers, Some(i == 1))));
    s.on_close(on0(ctx, close));
    s.on_add(on0(ctx, |ctx| {
        let t = ctx.m.borrow().tab;
        match t {
            Tab::Alarms => edit(ctx, 0),
            _ => {
                ctx.m.borrow_mut().query.clear();
                ctx.ui().global::<ClockState>().set_city_query("".into());
                ctx.open(View::Cities);
            }
        }
    }));
    s.on_menu(on(ctx, |ctx, i| match i {
        0 => crate::bind::set_time(ctx),
        _ => ctx.open(View::Settings),
    }));

    // the Clock tab: a long-press on a city asks to remove it (the answer comes as AppEvent::Answer)
    s.on_city_held(on(ctx, |ctx, i| {
        let i = usize::try_from(i).unwrap_or(usize::MAX);
        let Some(name) = ctx.m.borrow().cities.as_slice().get(i).map(|c| c.name) else {
            return;
        };
        ctx.m.borrow_mut().removing = Some(i);
        let q = neoapp::Query {
            text: alloc::format!("Remove {name} from World?"),
            buttons: alloc::vec!["Remove".into(), "Cancel".into()],
            destructive: true,
        };
        if let Err(e) = ctx.app.query(&q) {
            dprintln!("clock: dialog not shown ({})", e);
        }
    }));

    // Alarms
    s.on_edit_alarm(on(ctx, |ctx, id| edit(ctx, u16::try_from(id).unwrap_or(0))));
    s.on_alarm_switched(on2(ctx, |ctx, id, on| {
        let a = u16::try_from(id)
            .ok()
            .and_then(|id| ctx.m.borrow().alarms.get(id).cloned());
        if let Some(a) = a {
            put(ctx, &Alarm { on, ..a });
        }
    }));
    s.on_edit_time(on2(ctx, |ctx, h: i32, m: i32| {
        if let Some(d) = ctx.m.borrow_mut().draft.as_mut() {
            // the wheels give 0..24 and 0..60
            (d.0.hour, d.0.minute) = (h.clamp(0, 23) as u8, m.clamp(0, 59) as u8);
        }
    }));
    s.on_edit_day(on(ctx, |ctx, day| {
        if let Some(d) = ctx.m.borrow_mut().draft.as_mut() {
            d.0.days = d.0.days.toggled(u8::try_from(day).unwrap_or(7));
        }
    }));
    s.on_edit_preset_picked(on(ctx, |ctx, i| {
        if let Some(d) = ctx.m.borrow_mut().draft.as_mut() {
            d.0.days = match i {
                1 => Weekdays::WORKDAYS,
                2 => Weekdays::EVERY_DAY,
                _ => Weekdays::ONCE,
            };
        }
    }));
    s.on_save_alarm(on0(ctx, |ctx| {
        let label = fit(&ctx.ui().global::<ClockState>().get_edit_label());
        let Some(Draft(a)) = ctx.m.borrow_mut().draft.take() else {
            return;
        };
        // a saved alarm rings: saving turns it on
        put(
            ctx,
            &Alarm {
                label,
                on: true,
                ..a
            },
        );
        ctx.open(View::None);
    }));
    s.on_delete_alarm(on0(ctx, |ctx| {
        let draft = ctx.m.borrow_mut().draft.take();
        if let Some(Draft(a)) = draft {
            if ctx.dt("ALARM_DEL", |c| c.delete_alarm(a.id)).is_ok() {
                dprintln!("clock: alarm {} deleted", a.id);
            }
            ctx.reload();
        }
        ctx.open(View::None);
    }));
    s.on_open_sound(on0(ctx, |ctx| {
        {
            let mut m = ctx.m.borrow_mut();
            m.sound_from = m.view;
        }
        ctx.open(View::Sound);
    }));
    s.on_snooze_picked(on(ctx, |ctx, i| {
        let Some(&(min, n)) = usize::try_from(i).ok().and_then(|i| show::SNOOZES.get(i)) else {
            return;
        };
        (
            ctx.m.borrow_mut().snooze_min,
            ctx.m.borrow_mut().snooze_count,
        ) = (min, n);
        ctx.save(keys::CLOCK_SNOOZE, min);
        ctx.save(keys::CLOCK_SNOOZE_COUNT, n);
    }));
    s.on_sound_picked(on(ctx, |ctx, i| {
        let Some(&(name, tone, _)) = usize::try_from(i).ok().and_then(|i| show::SOUNDS.get(i))
        else {
            return;
        };
        ctx.m.borrow_mut().sound = name.into();
        ctx.save(keys::CLOCK_SOUND, name.into());
        // a preview: one period of it
        sound::play(tone, false);
    }));

    // Settings
    s.on_set_face(on(ctx, |ctx, i| {
        ctx.m.borrow_mut().analog = i == 1;
        ctx.save(keys::CLOCK_FACE, i32::from(i == 1));
    }));
    s.on_set_seconds(on(ctx, |ctx, v| {
        ctx.m.borrow_mut().seconds = v;
        ctx.save(keys::CLOCK_SECONDS, v);
    }));
    s.on_set_show_date(on(ctx, |ctx, v| {
        ctx.m.borrow_mut().date = v;
        ctx.save(keys::CLOCK_DATE, v);
    }));
    s.on_set_h24(on(ctx, |ctx, v| {
        ctx.m.borrow_mut().h24 = v;
        ctx.save(keys::CLOCK_24H, v);
    }));
    s.on_set_shade_next(on(ctx, |ctx, v| {
        ctx.m.borrow_mut().shade_next = v;
        ctx.save(keys::CLOCK_SHADE_NEXT, v);
    }));

    // Add city
    s.on_city_searched(on(ctx, |ctx, q: slint::SharedString| {
        ctx.m.borrow_mut().query = q.as_str().into();
    }));
    s.on_pick_city(on(ctx, |ctx, index| {
        let r = usize::try_from(index)
            .map_err(|_| AddError::Full)
            .and_then(|i| ctx.m.borrow_mut().cities.add(i));
        match r {
            Ok(()) => {
                let name = CATALOGUE.get(index as usize).map_or("", |c| c.name);
                dprintln!("clock: city {} added", name);
                ctx.save_cities();
                ctx.open(View::None);
            }
            Err(e) => dprintln!("clock: city {} not added ({:?})", index, e),
        }
    }));

    // Set time
    s.on_set_date(on3(ctx, |ctx, y: i32, mo: i32, d: i32| {
        let l = &mut ctx.m.borrow_mut().set_time;
        // the picker gives a valid date
        (l.year, l.month, l.day) = (y, mo.clamp(1, 12) as u32, d.clamp(1, 31) as u32);
    }));
    s.on_set_clock(on2(ctx, |ctx, h: i32, mi: i32| {
        let l = &mut ctx.m.borrow_mut().set_time;
        (l.hour, l.minute) = (h.clamp(0, 23) as u32, mi.clamp(0, 59) as u32);
    }));
    s.on_next_zone(on0(ctx, |ctx| {
        let utc = Now::read().utc;
        let mut m = ctx.m.borrow_mut();
        m.zone = (m.zone + 1) % CATALOGUE.len();
        m.set_time.tz = CATALOGUE[m.zone].offset_at(utc);
    }));
    s.on_save_time(on0(ctx, |ctx| {
        let l = ctx.m.borrow().set_time;
        match time::set_utc_and_offset(l.to_unix(), l.tz) {
            Ok(()) => dprintln!("clock: set {} UTC, offset {} s", l.to_unix(), l.tz),
            Err(e) => dprintln!("clock: set failed ({})", e),
        }
        ctx.open(View::None);
    }));

    // the timer (DTSRV's)
    s.on_timer_step(on(ctx, |ctx, seconds| step(ctx, i64::from(seconds) * 1000)));
    s.on_timer_preset(on(ctx, |ctx, i| {
        if let Some(&ms) = usize::try_from(i)
            .ok()
            .and_then(|i| show::PRESETS_MS.get(i))
        {
            ctx.m.borrow_mut().timer_ms = ms;
        }
    }));
    s.on_timer_start(on0(ctx, |ctx| {
        let (paused, ms) = {
            let m = ctx.m.borrow();
            (
                matches!(m.timer.phase, datetime_core::timer::Phase::Paused { .. }),
                m.timer_ms,
            )
        };
        if paused {
            timer(ctx, op::RESUME, 0);
        } else {
            // under a day in ms fits i32
            timer(ctx, op::START, ms as i32);
        }
    }));
    s.on_timer_pause(on0(ctx, |ctx| timer(ctx, op::PAUSE, 0)));
    s.on_timer_reset(on0(ctx, |ctx| timer(ctx, op::RESET, 0)));
    s.on_timer_add(on(ctx, |ctx, seconds| timer(ctx, op::ADD, seconds)));
    s.on_timer_stop(on0(ctx, |ctx| timer(ctx, op::RESET, 0)));

    // the stopwatch (the app's own)
    s.on_sw_toggle(on0(ctx, |ctx| {
        let now = Now::read();
        {
            let w = &mut ctx.m.borrow_mut().stopwatch;
            if w.is_running() {
                w.stop(now.wall_ms);
                dprintln!("clock: stopwatch stop {} ms", w.elapsed_ms(now.wall_ms));
            } else {
                w.start(now.wall_ms);
                dprintln!("clock: stopwatch start");
            }
        }
        ctx.save_stopwatch();
    }));
    s.on_sw_lap(on0(ctx, |ctx| {
        let now = Now::read();
        if ctx.m.borrow_mut().stopwatch.lap(now.wall_ms).is_ok() {
            ctx.save_stopwatch();
        }
    }));
    s.on_sw_reset(on0(ctx, |ctx| {
        ctx.m.borrow_mut().stopwatch.reset();
        dprintln!("clock: stopwatch reset");
        ctx.save_stopwatch();
    }));

    // Ringing
    s.on_ring_first_clicked(on0(ctx, ring::first));
    s.on_ring_second_clicked(on0(ctx, ring::second));
}

/// Set time, from the clock's local time and zone (the zone's city: `time.zone`'s, else the first on that offset).
pub fn set_time(ctx: &Ctx) {
    let now = Now::read();
    {
        let mut m = ctx.m.borrow_mut();
        m.set_time = Local {
            year: now.local.year,
            month: now.local.month,
            day: now.local.day,
            hour: now.local.hour,
            minute: now.local.minute,
            tz: now.tz,
        };
        let named = CATALOGUE.iter().position(|c| c.name == m.zone_name);
        m.zone = named
            .or_else(|| {
                CATALOGUE
                    .iter()
                    .position(|c| c.offset_at(now.utc) == now.tz)
            })
            .unwrap_or(0);
    }
    let auto = !matches!(
        ctx.app.setting_raw("time.auto"),
        Ok(neoui_proto::settings::Value::Bool(false))
    );
    let ui = ctx.ui();
    let s = ui.global::<ClockState>();
    s.set_auto_on(auto);
    s.set_auto_text(
        if auto {
            "On · from the network"
        } else {
            "Off · set by hand"
        }
        .into(),
    );
    ctx.open(View::SetTime);
}
