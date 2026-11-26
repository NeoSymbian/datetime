// SPDX-License-Identifier: GPL-3.0-only
//! The model onto the screens: [`all`] after a change, [`tick`] as time passes (the parts that move by themselves).

use crate::ctx::{Ctx, Model, Now, Tab, View};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use clock_ui::{
    AlarmItem, CatalogueItem, CityCard, ClockApp, ClockState, InfoChip, LapItem, ring, text,
};
use datetime_core::alarm::{Alarm, Alarms, Weekdays, format_hm};
use datetime_core::analog;
use datetime_core::ipc::ring as ring_id;
use datetime_core::plan;
use datetime_core::stopwatch::format_elapsed;
use datetime_core::timer::{Phase, format_left};
use neoui_proto::clock::face::{DAYS, Face, MONTHS};
use neoui_proto::clock::next::Next as ShellNext;
use neoui_proto::clock::world::{CATALOGUE, City};
use slint::{
    ComponentHandle, Image, Model as _, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString,
    VecModel,
};

/// The timer's presets (clk2-timer-set).
pub const PRESETS_MS: [u32; 6] = [60_000, 180_000, 300_000, 600_000, 900_000, 1_800_000];
/// The snooze choices: (minutes, times); 0 times = off.
pub const SNOOZES: [(i32, i32); 6] = [(5, 3), (10, 3), (15, 3), (5, 5), (10, 1), (5, 0)];
/// The alarm sounds: name, NeoAudio tone, note.
// shortcut: NeoAudio has three built-in tones, so Chimes and Bells (clk2-alarm-sound) wait for more tones or files.
pub const SOUNDS: [(&str, u8, &str); 4] = [
    ("Dawn", neoaudio::tones::BUILT_IN, "Built in"),
    ("Pulse", neoaudio::tones::TWO_BEEPS, "Built in"),
    ("Classic beep", neoaudio::tones::BEEP, "Built in"),
    ("Vibrate only", neoaudio::tones::NONE, "No sound"),
];

/// The tone of the sound called `name` (Dawn when unknown).
pub fn tone(name: &str) -> u8 {
    SOUNDS
        .iter()
        .find(|s| s.0 == name)
        .map_or(neoaudio::tones::BUILT_IN, |s| s.1)
}

fn rows<T: Clone + 'static>(items: impl IntoIterator<Item = T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items.into_iter().collect::<Vec<_>>()))
}

fn strings<S: Into<SharedString>>(items: impl IntoIterator<Item = S>) -> ModelRc<SharedString> {
    rows(items.into_iter().map(Into::into))
}

/// An hour as shown: (hours, "AM"/"PM" or "").
fn hour(h: u8, h24: bool) -> (i32, &'static str) {
    if h24 {
        (i32::from(h), "")
    } else {
        (
            i32::from((h + 11) % 12 + 1),
            if h < 12 { "AM" } else { "PM" },
        )
    }
}

/// "Today 14:30" without "Today " (the hero is today's).
fn chip_time(t: String) -> String {
    t.strip_prefix("Today ").map(String::from).unwrap_or(t)
}

/// The next alarm after now: (Unix UTC seconds, the alarm).
fn next_alarm<'a>(m: &'a Model, now: &Now) -> Option<(i64, &'a Alarm)> {
    // the app has no ring history (DTSRV keeps it): the display ignores what already rang
    plan::next_alarm(&m.alarms, &plan::Rung::default(), now.utc, now.tz)
}

/// Everything, from the model.
pub fn all(ctx: &Ctx, now: &Now) {
    let ui = ctx.ui();
    let s = ui.global::<ClockState>();
    {
        let m = ctx.m.borrow();
        s.set_tab(m.tab as i32);
        s.set_timers(i32::from(m.stopwatch_shown));
        s.set_view(m.view.name().into());
        s.set_h24(m.h24);
        s.set_analog(m.analog);
        s.set_show_seconds(m.seconds);
        s.set_show_date(m.date);
        s.set_shade_next(m.shade_next);
        s.set_sound_name(m.sound.as_str().into());
        s.set_snooze_text(text::snooze(m.snooze_min, m.snooze_count).as_str().into());
        s.set_snooze_choices(strings(
            SNOOZES.iter().map(|&(min, n)| text::snooze(min, n)),
        ));
        s.set_snooze_current(
            SNOOZES
                .iter()
                .position(|&(min, n)| (min, n) == (m.snooze_min, m.snooze_count))
                .map_or(-1, |i| i as i32),
        );
        s.set_sounds(strings(SOUNDS.iter().map(|s| s.0)));
        s.set_sound_notes(strings(SOUNDS.iter().map(|s| s.2)));
        s.set_sound_current(
            SOUNDS
                .iter()
                .position(|x| x.0 == m.sound)
                .map_or(-1, |i| i as i32),
        );
        s.set_presets(strings(
            PRESETS_MS.iter().map(|ms| format!("{} min", ms / 60_000)),
        ));
        world(&s, &m, now);
        alarms(&s, &m, now);
        editor(&s, &m, now);
        if m.view == View::Cities {
            catalogue(&s, &m, now);
        }
        if m.view == View::SetTime {
            set_time(&s, &m);
        }
        if m.view == View::Ringing {
            ringing(&s, &m, now);
        }
    }
    tick(ctx, now);
}

/// The parts that move with time; true while something moves faster than once a second (the stopwatch on screen).
pub fn tick(ctx: &Ctx, now: &Now) -> bool {
    let ui = ctx.ui();
    let s = ui.global::<ClockState>();
    let m = ctx.m.borrow();
    hero(&s, &m, now);
    if m.analog && m.tab == Tab::Clock && m.view == View::None {
        s.set_face(face(ctx, &ui, now));
    }
    if now.second == 0 {
        world(&s, &m, now);
        alarms(&s, &m, now);
    }
    timer(ctx, &s, &m, now);
    stopwatch(&s, &m, now);
    m.view == View::None && m.tab == Tab::Timers && m.stopwatch_shown && m.stopwatch.is_running()
}

/// The TimeHero: the time, the date, the next alarm and event chips.
fn hero(s: &ClockState<'_>, m: &Model, now: &Now) {
    let face = Face::new(&now.local, m.h24, "");
    // at most 23 / 59 / 59
    s.set_hours(face.hours as i32);
    s.set_minutes(face.minutes as i32);
    s.set_seconds(
        if m.seconds {
            format!(":{:02}", now.second)
        } else {
            String::new()
        }
        .as_str()
        .into(),
    );
    s.set_am_pm(face.am_pm.into());
    s.set_date(if m.date { face.date.as_str() } else { "" }.into());
    let mut chips = Vec::new();
    if let Some((at, a)) = next_alarm(m, now) {
        let when = ShellNext {
            alarm_at: at,
            ..ShellNext::default()
        }
        .alarm_text(now.utc, now.tz, m.h24);
        chips.push(InfoChip {
            text: format!("{} {}", chip_time(when), label(a)).as_str().into(),
            alarm: true,
            dot: slint::Color::default(),
        });
    }
    if m.next.event_at != 0 {
        let when = ShellNext {
            event_at: m.next.event_at,
            event_all_day: m.next.event_all_day,
            ..ShellNext::default()
        }
        .event_text(now.utc, now.tz, m.h24);
        if !when.is_empty() {
            chips.push(InfoChip {
                text: format!("{} {}", chip_time(when), m.next.event_title)
                    .as_str()
                    .into(),
                alarm: false,
                dot: slint::Color::from_argb_encoded(0xff00_0000 | m.next.event_colour),
            });
        }
    }
    let old = s.get_chips();
    let same = old.row_count() == chips.len()
        && chips
            .iter()
            .enumerate()
            .all(|(i, c)| old.row_data(i).as_ref() == Some(c));
    if !same {
        s.set_chips(rows(chips));
    }
}

fn label(a: &Alarm) -> &str {
    if a.label.is_empty() {
        "Alarm"
    } else {
        &a.label
    }
}

/// The analog face for `now`: the dial kept, the hands drawn over a copy, once a second.
fn face(ctx: &Ctx, ui: &ClockApp, now: &Now) -> Image {
    let size = ((190.0 * ctx.app.dp) as usize).max(16);
    let theme = ui.global::<clock_ui::Theme>();
    let rgb = |c: slint::Color| [c.red(), c.green(), c.blue()];
    let (ink, accent) = (rgb(theme.get_text()), rgb(theme.get_accent()));
    let mut a = ctx.analog.borrow_mut();
    if a.size != size || a.ink != ink {
        a.dial = alloc::vec![[0; 4]; size * size];
        analog::dial(&mut a.dial, size, ink);
        (a.size, a.ink) = (size, ink);
    }
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(size as u32, size as u32);
    let (px, _) = buf.make_mut_bytes().as_chunks_mut::<4>();
    px.copy_from_slice(&a.dial);
    let seconds = ctx.m.borrow().seconds;
    analog::hands(
        px,
        size,
        (now.local.hour, now.local.minute, now.second),
        seconds,
        ink,
        accent,
    );
    Image::from_rgba8(buf)
}

/// The hour at a city now, for its day/night dot.
fn night(city: &City, utc: i64) -> bool {
    let h = (utc + i64::from(city.offset_at(utc))).rem_euclid(86_400) / 3600;
    !(6..18).contains(&h)
}

/// "−4 h" → "Today · −4 h"; "Same time" reads "same time" after a dot.
fn difference(d: &str) -> String {
    if d == "Same time" {
        "same time".into()
    } else {
        d.into()
    }
}

fn world(s: &ClockState<'_>, m: &Model, now: &Now) {
    let cities = m.cities.as_slice();
    s.set_cities(rows(cities.iter().map(|c| {
        let shown = c.show(now.utc, now.tz, m.h24);
        CityCard {
            name: c.name.into(),
            when: format!("{} · {}", shown.day, difference(&shown.difference))
                .as_str()
                .into(),
            time: shown.time.as_str().into(),
            night: night(c, now.utc),
        }
    })));
    s.set_cities_full(cities.len() >= neoui_proto::clock::world::CITIES_MAX);
    s.set_here(here(&m.zone_name, now.tz).as_str().into());
    let n = cities.len();
    s.set_world_title(
        format!("World · {n} {}", if n == 1 { "city" } else { "cities" })
            .as_str()
            .into(),
    );
}

/// "Yerevan · UTC+4": the zone in force (`time.zone` names its city).
pub fn here(zone: &str, tz: i32) -> String {
    if zone.is_empty() {
        text::utc(tz)
    } else {
        format!("{zone} · {}", text::utc(tz))
    }
}

fn alarms(s: &ClockState<'_>, m: &Model, now: &Now) {
    match next_alarm(m, now) {
        Some((at, a)) => {
            let local = (at + i64::from(now.tz)).rem_euclid(86_400);
            // under 24 / 60
            let (h, am_pm) = hour((local / 3600) as u8, m.h24);
            s.set_next_caption(
                format!("Next alarm · {}", text::day(at, now.utc, now.tz))
                    .as_str()
                    .into(),
            );
            s.set_next_hours(h);
            s.set_next_minutes((local / 60 % 60) as i32);
            s.set_next_am_pm(am_pm.into());
            s.set_next_line(
                format!(
                    "{} · rings in {}",
                    label(a),
                    text::span(text::minutes_until(at, now.utc))
                )
                .as_str()
                .into(),
            );
        }
        None => s.set_next_caption("".into()),
    }
    s.set_alarms(rows(m.alarms.as_slice().iter().map(|a| {
        let (h, am_pm) = hour(a.hour, m.h24);
        AlarmItem {
            id: i32::from(a.id),
            hours: h,
            minutes: i32::from(a.minute),
            am_pm: am_pm.into(),
            label: label(a).into(),
            days: rows((0..7u8).map(|d| a.days.contains(d))),
            once: if a.days.is_once() { "Once" } else { "" }.into(),
            on: a.on,
        }
    })));
    s.set_alarms_full(m.alarms.as_slice().len() >= datetime_core::alarm::ALARMS_MAX);
}

fn editor(s: &ClockState<'_>, m: &Model, now: &Now) {
    let Some(d) = &m.draft else { return };
    let a = &d.0;
    let (h, am_pm) = hour(a.hour, m.h24);
    s.set_edit_new(a.id == 0);
    s.set_edit_hours(h);
    s.set_edit_minutes(i32::from(a.minute));
    s.set_edit_am_pm(am_pm.into());
    s.set_sheet_hour(i32::from(a.hour));
    s.set_sheet_minute(i32::from(a.minute));
    let mut one = Alarms::new();
    let when = one
        .add(a.hour, a.minute, a.days, "")
        .ok()
        .and_then(|_| plan::next_alarm(&one, &plan::Rung::default(), now.utc, now.tz))
        .map(|(at, _)| text::rings(at, now.utc, now.tz))
        .unwrap_or_default();
    s.set_edit_when(when.as_str().into());
    s.set_edit_days(rows((0..7u8).map(|d| a.days.contains(d))));
    s.set_edit_preset(match a.days {
        Weekdays::ONCE => 0,
        Weekdays::WORKDAYS => 1,
        Weekdays::EVERY_DAY => 2,
        _ => -1,
    });
}

/// Add city: the catalogue by name, filtered by the search, in two groups.
fn catalogue(s: &ClockState<'_>, m: &Model, now: &Now) {
    let q = m.query.to_lowercase();
    let mut list: Vec<(usize, &City)> = CATALOGUE
        .iter()
        .enumerate()
        .filter(|(_, c)| q.is_empty() || c.name.to_lowercase().contains(&q))
        .collect();
    list.sort_by(|a, b| a.1.name.cmp(b.1.name));
    let mut group = "";
    s.set_catalogue(rows(list.into_iter().map(|(i, c)| {
        let shown = c.show(now.utc, now.tz, m.h24);
        let g = if c.name < "N" { "A – M" } else { "N – Z" };
        let section = if g == group { "" } else { g };
        group = g;
        CatalogueItem {
            // under 100 cities
            index: i as i32,
            name: c.name.into(),
            detail: format!(
                "{} · {}",
                text::utc(c.offset_at(now.utc)),
                difference(&shown.difference)
            )
            .as_str()
            .into(),
            time: shown.time.as_str().into(),
            added: m.cities.as_slice().iter().any(|x| x.name == c.name),
            section: section.into(),
        }
    })));
}

/// "Monday, 16 November 2026" for a date (its weekday from the days since 1970, a Thursday).
fn long_date(l: &neoui_proto::clock::Local) -> String {
    let days = neoui_proto::clock::Local {
        hour: 0,
        minute: 0,
        tz: 0,
        ..*l
    }
    .to_unix()
    .div_euclid(86_400);
    let weekday = (days + 3).rem_euclid(7) as usize;
    format!(
        "{}, {} {} {}",
        DAYS[weekday],
        l.day,
        MONTHS[(l.month.clamp(1, 12) - 1) as usize],
        l.year
    )
}

fn set_time(s: &ClockState<'_>, m: &Model) {
    let l = &m.set_time;
    s.set_date_text(long_date(l).as_str().into());
    // at most 23 / 59
    s.set_time_text(
        format_hm(l.hour as u8, l.minute as u8, m.h24)
            .as_str()
            .into(),
    );
    let city = CATALOGUE.get(m.zone).map_or("", |c| c.name);
    s.set_zone_text(here(city, l.tz).as_str().into());
    s.set_st_year(l.year);
    s.set_st_month(l.month as i32);
    s.set_st_day(l.day as i32);
    s.set_st_hour(l.hour as i32);
    s.set_st_minute(l.minute as i32);
}

fn ringing(s: &ClockState<'_>, m: &Model, now: &Now) {
    let Some(r) = m.ring else { return };
    let face = Face::new(&now.local, m.h24, "");
    s.set_ring_line(face.date.as_str().into());
    s.set_ring_lead_zero(m.h24);
    if ring_id::kind(r) == ring_id::TIMER {
        s.set_ring_timer(true);
        s.set_ring_caption(
            format!("Timer · {}", format_left(m.timer.set_ms))
                .as_str()
                .into(),
        );
        s.set_ring_hours(0);
        s.set_ring_minutes(0);
        s.set_ring_lead_zero(true);
        s.set_ring_title("Time’s up".into());
        s.set_ring_first("+1 min".into());
    } else {
        // alarm ids are u16
        let a = m.alarms.get(ring_id::item(r) as u16);
        let (h, mi) = a.map_or((now.local.hour as u8, now.local.minute as u8), |a| {
            (a.hour, a.minute)
        });
        s.set_ring_timer(false);
        s.set_ring_caption("Alarm".into());
        s.set_ring_hours(hour(h, m.h24).0);
        s.set_ring_minutes(i32::from(mi));
        s.set_ring_title(a.map_or("Alarm", label).into());
        s.set_ring_first(
            if m.snooze_count > 0 {
                format!("Snooze {} min", m.snooze_min)
            } else {
                "Snooze off".to_string()
            }
            .as_str()
            .into(),
        );
    }
}

/// The Timers tab's timer: the draft while idle, DTSRV's timer otherwise; the ring redrawn only when it moved.
fn timer(ctx: &Ctx, s: &ClockState<'_>, m: &Model, now: &Now) {
    let t = &m.timer;
    let phase = match t.phase {
        Phase::Idle => 0,
        Phase::Running { .. } => 1,
        Phase::Paused { .. } => 2,
        Phase::Done => 3,
    };
    s.set_timer_phase(phase);
    let left = if phase == 0 {
        m.timer_ms
    } else {
        t.left_ms(now.wall_ms)
    };
    // a countdown shows a second until it is over: round up
    let (a, b) = text::digits(u64::from(left.div_ceil(1000)) * 1000);
    s.set_timer_a(a);
    s.set_timer_b(b);
    s.set_preset_current(
        PRESETS_MS
            .iter()
            .position(|&p| p == m.timer_ms)
            .map_or(-1, |i| i as i32),
    );
    let set = format_left(t.set_ms);
    let line = match t.phase {
        Phase::Idle => String::new(),
        Phase::Running { ends_ms } => {
            let end = (ends_ms.div_euclid(1000) + i64::from(now.tz)).rem_euclid(86_400);
            // under 24 / 60
            text::timer_line(
                &set,
                Some(((end / 3600) as u8, (end / 60 % 60) as u8)),
                m.h24,
            )
        }
        Phase::Paused { .. } => text::timer_line(&set, None, m.h24),
        Phase::Done => format!("Time’s up · {set}"),
    };
    s.set_timer_line(line.as_str().into());
    // the ring: only while it can be seen
    if phase == 0 || !m.visible || m.tab != Tab::Timers || m.stopwatch_shown {
        return;
    }
    let done = phase == 3;
    let fraction = if done || t.set_ms == 0 {
        1.0
    } else {
        (left as f32 / t.set_ms as f32).min(1.0)
    };
    let key = ((fraction * 1000.0) as i32, done);
    if ctx.ring_drawn.get() == key {
        return;
    }
    ctx.ring_drawn.set(key);
    let size = (250.0 * ctx.app.dp) as usize;
    let colour = if done {
        [0xe5, 0x48, 0x4d]
    } else {
        [0x3d, 0x8b, 0xfd]
    };
    s.set_timer_ring(ring::image(
        size,
        10.0 * ctx.app.dp,
        fraction,
        colour,
        [0x1d, 0x20, 0x25],
    ));
}

fn stopwatch(s: &ClockState<'_>, m: &Model, now: &Now) {
    let w = &m.stopwatch;
    let elapsed = w.elapsed_ms(now.wall_ms);
    let (a, b) = text::digits(elapsed);
    s.set_sw_a(a);
    s.set_sw_b(b);
    s.set_sw_small(text::small(elapsed).as_str().into());
    s.set_sw_running(w.is_running());
    s.set_sw_started(elapsed > 0);
    let laps: Vec<(usize, u64, u64)> = w.laps().collect();
    // the lap that runs now, after the last one taken
    let line = match laps.first() {
        Some(&(n, _, total)) if w.is_running() => {
            format!(
                "Lap {} · {}",
                n + 1,
                format_elapsed(elapsed.saturating_sub(total))
            )
        }
        _ => String::new(),
    };
    s.set_sw_line(line.as_str().into());
    // the laps change only on a lap or a reset; rebuilding them each tick would redraw the list
    if s.get_laps().row_count() != laps.len() {
        let fastest = laps.iter().map(|l| l.1).min();
        let slowest = laps.iter().map(|l| l.1).max();
        s.set_laps(rows(laps.iter().map(|&(n, lap, total)| LapItem {
            name: format!("Lap {n}").as_str().into(),
            lap: format_elapsed(lap).as_str().into(),
            total: format_elapsed(total).as_str().into(),
            mark: if laps.len() < 2 {
                0
            } else if Some(lap) == fastest {
                1
            } else if Some(lap) == slowest {
                2
            } else {
                0
            },
        })));
    }
}
