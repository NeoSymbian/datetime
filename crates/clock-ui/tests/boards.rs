// SPDX-License-Identifier: GPL-3.0-only
// Clock v2, one snapshot per design board (clk2-*, dt2-ring-*), with the boards' sample data: Monday 16 November,
// 10:24:07 in Yerevan (UTC+4). Gestures (679 dp) unless the board is a Buttons one (clk2-b-*, 639 dp); Ringing is full
// screen (720 dp). One test: Slint's platform is per process.
mod common;
use clock_ui::{AlarmItem, CatalogueItem, CityCard, ClockState, InfoChip, LapItem, ring};
use datetime_core::analog;
use slint::{
    Color, ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};
use std::rc::Rc;

const DP: usize = 3;
const GESTURES: u32 = 2037;
const BUTTONS: u32 = 1917;
const FULL: u32 = 2160;

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

fn strings(v: &[&str]) -> ModelRc<SharedString> {
    model(v.iter().map(|s| SharedString::from(*s)).collect())
}

fn rgb(c: u32) -> Color {
    Color::from_argb_encoded(0xff00_0000 | c)
}

fn days(bits: u8) -> ModelRc<bool> {
    model((0..7).map(|d| bits & 1 << d != 0).collect())
}

fn alarm(id: i32, hm: (i32, i32), label: &str, bits: u8, on: bool) -> AlarmItem {
    AlarmItem {
        id,
        hours: hm.0,
        minutes: hm.1,
        am_pm: "".into(),
        label: label.into(),
        days: days(bits),
        once: "".into(),
        on,
    }
}

fn city(name: &str, when: &str, time: &str, night: bool) -> CityCard {
    CityCard {
        name: name.into(),
        when: when.into(),
        time: time.into(),
        night,
    }
}

fn place(index: i32, name: &str, detail: &str, time: &str, section: &str) -> CatalogueItem {
    CatalogueItem {
        index,
        name: name.into(),
        detail: detail.into(),
        time: time.into(),
        added: time.is_empty(),
        section: section.into(),
    }
}

fn lap(n: i32, lap: &str, total: &str, mark: i32) -> LapItem {
    LapItem {
        name: format!("Lap {n}").into(),
        lap: lap.into(),
        total: total.into(),
        mark,
    }
}

/// The analog face at 10:24:07, as NEOCLOCK draws it.
fn face() -> Image {
    let size = 190 * DP;
    let mut px = vec![[0u8; 4]; size * size];
    analog::dial(&mut px, size, [255, 255, 255]);
    analog::hands(
        &mut px,
        size,
        (10, 24, 7),
        true,
        [255, 255, 255],
        [0x3d, 0x8b, 0xfd],
    );
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(size as u32, size as u32);
    buf.make_mut_bytes()
        .as_chunks_mut::<4>()
        .0
        .copy_from_slice(&px);
    Image::from_rgba8(buf)
}

/// Everything the boards share, 24-hour, the Clock tab, nothing open.
fn base(s: &ClockState<'_>) {
    s.set_tab(0);
    s.set_timers(0);
    s.set_view("".into());
    s.set_h24(true);
    s.set_hours(10);
    s.set_minutes(24);
    s.set_seconds(":07".into());
    s.set_am_pm("".into());
    s.set_date("Monday, 16 November".into());
    s.set_chips(model(vec![
        InfoChip {
            text: "14:30 Medicine".into(),
            alarm: true,
            dot: Color::default(),
        },
        InfoChip {
            text: "13:00 Lunch with Ben".into(),
            alarm: false,
            dot: rgb(0x22b46a),
        },
    ]));
    s.set_analog(false);
    s.set_world_title("World · 4 cities".into());
    s.set_here("Yerevan · UTC+4".into());
    s.set_cities(model(vec![
        city("London", "Today · −4 h", "06:24", false),
        city("New York", "Today · −9 h", "01:24", true),
        city("Tokyo", "Today · +5 h", "15:24", false),
        city("Los Angeles", "Yesterday · −12 h", "22:24", true),
    ]));
    s.set_next_caption("Next alarm · today".into());
    s.set_next_hours(14);
    s.set_next_minutes(30);
    s.set_next_line("Medicine · rings in 4 h 06 min".into());
    s.set_alarms(model(vec![
        alarm(1, (6, 45), "Wake up", 0b001_1111, true),
        alarm(2, (7, 30), "Gym", 0b001_0101, false),
        alarm(3, (9, 0), "Work", 0b001_1111, true),
        alarm(4, (14, 30), "Medicine", 0b111_1111, true),
    ]));
    s.set_sound_name("Dawn".into());
    s.set_snooze_text("5 min · up to 3 times".into());
    s.set_snooze_choices(strings(&[
        "5 min · up to 3 times",
        "10 min · up to 3 times",
        "15 min · up to 3 times",
        "5 min · up to 5 times",
        "10 min · once",
        "Off",
    ]));
    s.set_sounds(strings(&["Dawn", "Pulse", "Classic beep", "Vibrate only"]));
    s.set_sound_notes(strings(&["Built in", "Built in", "Built in", "No sound"]));
    s.set_presets(strings(&[
        "1 min", "3 min", "5 min", "10 min", "15 min", "30 min",
    ]));
    s.set_timer_phase(0);
    s.set_timer_a(5);
    s.set_timer_b(0);
    s.set_preset_current(2);
    s.set_sw_a(0);
    s.set_sw_b(0);
    s.set_sw_small(".00".into());
    s.set_sw_line("".into());
    s.set_sw_running(false);
    s.set_sw_started(false);
    s.set_laps(model(vec![]));
}

fn running_stopwatch(s: &ClockState<'_>) {
    s.set_tab(2);
    s.set_timers(1);
    s.set_sw_a(2);
    s.set_sw_b(41);
    s.set_sw_small(".37".into());
    s.set_sw_line("Lap 4 · 00:38.12".into());
    s.set_sw_running(true);
    s.set_sw_started(true);
    s.set_laps(model(vec![
        lap(4, "00:38.12", "02:41.37", 0),
        lap(3, "00:41.90", "02:03.25", 0),
        lap(2, "00:36.05", "01:21.35", 1),
        lap(1, "00:45.30", "00:45.30", 2),
    ]));
}

fn running_timer(s: &ClockState<'_>) {
    s.set_tab(2);
    s.set_timer_phase(1);
    s.set_timer_a(3);
    s.set_timer_b(6);
    s.set_timer_line("of 5:00 · ends 10:27".into());
    s.set_timer_ring(ring::image(
        250 * DP,
        10.0 * DP as f32,
        186.0 / 300.0,
        [0x3d, 0x8b, 0xfd],
        [0x1d, 0x20, 0x25],
    ));
}

#[test]
fn every_board() {
    let shot = common::app();
    let s = shot.ui.global::<ClockState>();

    // the Clock tab: digital 24-hour, analog, 12-hour, the ⋮ menu
    base(&s);
    shot.shoot("clk2-clock");
    s.set_analog(true);
    s.set_face(face());
    shot.shoot("clk2-clock-analog");
    s.set_analog(false);
    s.set_h24(false);
    s.set_am_pm("AM".into());
    s.set_chips(model(vec![
        InfoChip {
            text: "2:30 PM Medicine".into(),
            alarm: true,
            dot: Color::default(),
        },
        InfoChip {
            text: "1:00 PM Lunch with Ben".into(),
            alarm: false,
            dot: rgb(0x22b46a),
        },
    ]));
    s.set_cities(model(vec![
        city("London", "Today · −4 h", "6:24 AM", false),
        city("New York", "Today · −9 h", "1:24 AM", true),
        city("Tokyo", "Today · +5 h", "3:24 PM", false),
        city("Los Angeles", "Yesterday · −12 h", "10:24 PM", true),
    ]));
    shot.shoot("clk2-clock-12h");
    base(&s);
    shot.tick();
    // ⋮ at the header's right: 4 dp in, 48 dp wide, 48 high
    shot.press((1080 - (4 + 24) * DP) as f32, (24 * DP) as f32, 60);
    shot.shoot("clk2-menu");
    // the menu closes on a tap outside it
    shot.press(540.0, 1500.0, 60);
    shot.tick();

    // Clock's views
    s.set_view("cities".into());
    s.set_catalogue(model(vec![
        place(0, "Amsterdam", "UTC+1 · −3 h", "07:24", "A – M"),
        place(1, "Berlin", "UTC+1 · −3 h", "07:24", ""),
        place(2, "Buenos Aires", "UTC−3 · −7 h", "03:24", ""),
        place(3, "Cairo", "UTC+2 · −2 h", "08:24", ""),
        place(4, "Dubai", "UTC+4 · same time", "10:24", ""),
        place(5, "London", "UTC+0 · −4 h", "", ""),
        place(6, "Los Angeles", "UTC−8 · −12 h", "", ""),
        place(7, "Moscow", "UTC+3 · −1 h", "09:24", ""),
    ]));
    shot.shoot("clk2-world-add");
    s.set_view("set-time".into());
    s.set_auto_text("Off · set by hand".into());
    s.set_date_text("Monday, 16 November 2026".into());
    s.set_time_text("10:24".into());
    s.set_zone_text("Yerevan · UTC+4".into());
    s.set_st_year(2026);
    s.set_st_month(11);
    s.set_st_day(16);
    s.set_date_sheet(true);
    shot.shoot("clk2-set-time");
    s.set_date_sheet(false);
    shot.tick();
    s.set_view("settings".into());
    shot.shoot("clk2-settings");

    // the Alarms tab, its editor, the time sheet, the sound
    base(&s);
    s.set_tab(1);
    shot.shoot("clk2-alarms");
    s.set_alarms(model(vec![]));
    s.set_next_caption("".into());
    shot.shoot("clk2-alarms-empty");
    base(&s);
    s.set_view("alarm".into());
    s.set_edit_new(false);
    s.set_edit_hours(6);
    s.set_edit_minutes(45);
    s.set_edit_when("Rings tomorrow, in 20 h 21 min".into());
    s.set_edit_days(days(0b001_1111));
    s.set_edit_preset(1);
    s.set_edit_label("Wake up".into());
    shot.shoot("clk2-alarm-edit");
    s.set_sheet_hour(6);
    s.set_sheet_minute(45);
    s.set_time_sheet(true);
    shot.shoot("clk2-alarm-time-sheet");
    s.set_time_sheet(false);
    shot.tick();
    s.set_view("sound".into());
    s.set_sound_current(0);
    shot.shoot("clk2-alarm-sound");

    // the Timers tab
    base(&s);
    s.set_tab(2);
    shot.shoot("clk2-timer-set");
    running_timer(&s);
    shot.shoot("clk2-timer-running");
    s.set_timer_phase(3);
    s.set_timer_a(0);
    s.set_timer_b(0);
    s.set_timer_line("Time’s up · 5:00".into());
    s.set_timer_ring(ring::image(
        250 * DP,
        10.0 * DP as f32,
        1.0,
        [0xe5, 0x48, 0x4d],
        [0x1d, 0x20, 0x25],
    ));
    shot.shoot("clk2-timer-done");
    base(&s);
    s.set_tab(2);
    s.set_timers(1);
    shot.shoot("clk2-stopwatch-idle");
    running_stopwatch(&s);
    shot.shoot("clk2-stopwatch-running");

    // Buttons mode: the key row takes 56 dp instead of the 16 dp band
    shot.resize(BUTTONS);
    base(&s);
    shot.shoot("clk2-b-clock");
    s.set_tab(1);
    shot.shoot("clk2-b-alarms");
    base(&s);
    running_timer(&s);
    shot.shoot("clk2-b-timer");
    base(&s);
    running_stopwatch(&s);
    shot.shoot("clk2-b-stopwatch");

    // Ringing, full screen
    shot.resize(FULL);
    base(&s);
    s.set_view("ringing".into());
    s.set_ring_timer(false);
    s.set_ring_caption("Alarm".into());
    s.set_ring_hours(14);
    s.set_ring_minutes(30);
    s.set_ring_title("Medicine".into());
    s.set_ring_line("Monday, 16 November".into());
    s.set_ring_first("Snooze 5 min".into());
    shot.shoot("dt2-ring-alarm");
    s.set_ring_timer(true);
    s.set_ring_caption("Timer · 5:00".into());
    s.set_ring_hours(0);
    s.set_ring_minutes(0);
    s.set_ring_title("Time’s up".into());
    s.set_ring_line("Monday, 16 November".into());
    s.set_ring_first("+1 min".into());
    shot.shoot("dt2-ring-timer");
    shot.resize(GESTURES);
}
