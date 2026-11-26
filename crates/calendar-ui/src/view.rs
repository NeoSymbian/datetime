// SPDX-License-Identifier: GPL-3.0-only
//! What the screens show, from calendar-core's instances and to-dos: NEOCAL calls these with DTSRV's replies, the
//! host snapshots with a `Store` of the boards' sample events. Each sets the `Cal` global's properties for one view;
//! rows carry the index of their item in the slice given, which the taps (`Cal.act`) send back.
//!
//! Times are local seconds (calendar-core's "local"), days are `local / DAY`.

use crate::{
    AllDay, Block, Cal, CalLine, DayCell, Detail, EventLine, Hit, MonthBar, Pin, StripDay,
    TodoGroup, TodoLine,
};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use calendar_core::{DAY, EventDraft, Freq, Instance, Kind, Repeat, Store, Todo, Zone};
use pim_ics::date::Date;
use slint::{Color, ModelRc, SharedString, VecModel};

/// The Birthdays calendar's colour (they come from Contacts, not from a stored calendar).
pub const BIRTHDAYS: u32 = 0xE04E9B;
/// The colours a calendar can take (cal2-calendar-color).
pub const SWATCHES: [u32; 8] = [
    0x2F80ED, 0x22B46A, 0xF2994A, 0x8656E8, 0xE04E9B, 0x12A594, 0xF2C94C, 0xE5484D,
];
/// What the Reminder sheet offers, in minutes before the start (None: no reminder).
pub const REMINDERS: [Option<u16>; 6] = [None, Some(0), Some(5), Some(15), Some(60), Some(1440)];
/// An all-day event's "1 day before, 09:00": 15 hours before its midnight.
const ALL_DAY_EVE: u16 = 900;
const LETTERS: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];

/// The settings NEOCAL keeps (`calendar.*`, docs datetime.md; the `dt2-spec-keys` board).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefs {
    /// the tab Calendar opens on: 0 Month, 1 Week, 2 Day, 3 To-do
    pub view: u8,
    /// 0 Monday … 6 Sunday
    pub week_start: u8,
    pub week_numbers: bool,
    /// minutes before a new event (`calendar.reminder`)
    pub reminder: u16,
    /// a new event's minutes (`calendar.length`)
    pub length: u16,
    /// reminders ring full screen like alarms
    pub ring: bool,
    pub birthdays: bool,
    /// bit i hides calendar i
    pub hidden: u32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            view: 0,
            week_start: 0,
            week_numbers: false,
            reminder: 15,
            length: 60,
            ring: true,
            birthdays: true,
            hidden: 0,
        }
    }
}

/// A calendar as Calendars lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalInfo {
    pub name: String,
    pub colour: u32,
    /// its events (series count once)
    pub count: u32,
}

/// The wall clock the views are drawn at.
#[derive(Clone, Copy, Debug)]
pub struct Now {
    /// local seconds
    pub local: i64,
    /// seconds east of UTC (to-dos' completion times are UTC)
    pub tz: i32,
}

impl Now {
    #[must_use]
    pub fn today(&self) -> i64 {
        self.local.div_euclid(DAY)
    }
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

#[must_use]
pub fn rgb(c: u32) -> Color {
    Color::from_argb_encoded(0xff00_0000 | c)
}

/// An instance's colour: its calendar's, or Birthdays'.
#[must_use]
pub fn colour(i: &Instance) -> u32 {
    if i.kind == Kind::Birthday {
        BIRTHDAYS
    } else {
        i.colour
    }
}

/// "16:00"
#[must_use]
pub fn hm(local: i64) -> String {
    let s = local.rem_euclid(DAY);
    format!("{:02}:{:02}", s / 3600, s / 60 % 60)
}

fn hours(local: i64) -> f32 {
    local.rem_euclid(DAY) as f32 / 3600.0
}

/// "Mon 16 Nov"
#[must_use]
pub fn short_day(day: i64) -> String {
    let d = Date::from_days(day);
    format!(
        "{} {} {}",
        &d.weekday_name()[..3],
        d.day,
        &d.month_name()[..3]
    )
}

/// "Monday, 16 November"
#[must_use]
pub fn long_day(day: i64) -> String {
    let d = Date::from_days(day);
    format!("{}, {} {}", d.weekday_name(), d.day, d.month_name())
}

/// "16th"
fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// The ISO week number of the week holding `day`.
#[must_use]
pub fn iso_week(day: i64) -> u32 {
    let d = Date::from_days(day);
    let thursday = day - d.weekday() as i64 + 3;
    let t = Date::from_days(thursday);
    let jan1 = Date {
        year: t.year,
        month: 1,
        day: 1,
    }
    .days();
    ((thursday - jan1) / 7 + 1) as u32
}

/// The first day of the week holding `day`.
#[must_use]
pub fn week_of(day: i64, week_start: u8) -> i64 {
    let wd = Date::from_days(day).weekday() as i64;
    day - (wd - i64::from(week_start)).rem_euclid(7)
}

/// The first day of the 6-week grid of `year-month`.
#[must_use]
pub fn grid_start(year: i32, month: u8, week_start: u8) -> i64 {
    week_of(
        Date {
            year,
            month,
            day: 1,
        }
        .days(),
        week_start,
    )
}

fn starts_day(i: &Instance) -> i64 {
    i.start.div_euclid(DAY)
}

/// The last day an instance covers (its end is exclusive).
fn last_day(i: &Instance) -> i64 {
    (i.end - 1).max(i.start).div_euclid(DAY)
}

fn covers(i: &Instance, day: i64) -> bool {
    starts_day(i) <= day && last_day(i) >= day
}

fn multi_day(i: &Instance) -> bool {
    last_day(i) > starts_day(i)
}

fn weekday_letters(week_start: u8) -> Vec<SharedString> {
    (0..7)
        .map(|k| LETTERS[(k + usize::from(week_start)) % 7].into())
        .collect()
}

/// The line under an instance's title: the place, a birthday's age, a long event's day.
fn sub_line(i: &Instance, day: i64) -> String {
    if i.kind == Kind::Birthday {
        return if i.years > 0 {
            format!("Turns {} · from Contacts", i.years)
        } else {
            "From Contacts".into()
        };
    }
    let mut s = String::new();
    if multi_day(i) && i.all_day {
        s = format!(
            "Day {} of {}",
            day - starts_day(i) + 1,
            last_day(i) - starts_day(i) + 1
        );
    } else if i.kind == Kind::Anniversary && i.years > 0 {
        s = format!("{} anniversary", ordinal(u32::from(i.years)));
    }
    if !i.place.is_empty() {
        if !s.is_empty() {
            s.push_str(" · ");
        }
        s.push_str(&i.place);
    }
    s
}

fn title_of(i: &Instance) -> String {
    if i.kind == Kind::Birthday {
        format!("{}’s birthday", i.title)
    } else {
        i.title.clone()
    }
}

/// One agenda row (`k`: the item's index).
#[must_use]
pub fn line(i: &Instance, k: usize, day: i64, now: &Now) -> EventLine {
    let timed = !i.all_day && !multi_day(i);
    EventLine {
        i: k as i32,
        time: if timed { hm(i.start).into() } else { "".into() },
        end: if timed { hm(i.end).into() } else { "".into() },
        title: title_of(i).into(),
        sub: sub_line(i, day).into(),
        color: rgb(colour(i)),
        cake: i.kind == Kind::Birthday,
        past: i.end <= now.local && !i.all_day,
    }
}

/// Month (cal2-month, -busy): the grid of `year-month` with `selected`'s agenda (at most 3 rows; `pin`: today's next
/// alarm, "14:30 Medicine · Clock"). `items` covers the grid's 42 days.
#[allow(clippy::too_many_arguments)]
pub fn month(
    cal: &Cal,
    now: &Now,
    prefs: &Prefs,
    year: i32,
    month: u8,
    selected: i64,
    items: &[Instance],
    busy: bool,
    pin: &str,
) {
    let start = grid_start(year, month, prefs.week_start);
    let today = now.today();
    // the long events become bars, and their days lose the dots
    let mut bars = Vec::new();
    for i in items.iter().filter(|i| multi_day(i)) {
        for row in 0..6 {
            let (r0, r1) = (start + row * 7, start + row * 7 + 6);
            let (a, b) = (starts_day(i).max(r0), last_day(i).min(r1));
            if a <= b {
                bars.push(MonthBar {
                    row: row as i32,
                    from: (a - r0) as i32,
                    to: (b - r0) as i32,
                    color: rgb(colour(i)),
                });
            }
        }
    }
    let cells: Vec<DayCell> = (0..42)
        .map(|k| {
            let day = start + k;
            let d = Date::from_days(day);
            let barred = items.iter().any(|i| multi_day(i) && covers(i, day));
            let mut dots: Vec<Color> = Vec::new();
            if !barred && d.month == month {
                for i in items.iter().filter(|i| covers(i, day)) {
                    let c = rgb(colour(i));
                    if dots.len() < 3 && !dots.contains(&c) {
                        dots.push(c);
                    }
                }
            }
            DayCell {
                label: d.day.to_string().into(),
                outside: d.month != month,
                weekend: d.weekday() >= 5,
                today: day == today,
                selected: day == selected,
                dots: model(dots),
            }
        })
        .collect();
    let first = Date::from_days(start + 7);
    cal.set_month_title(format!("{} {}", first.month_name(), first.year).into());
    cal.set_weekdays(model(weekday_letters(prefs.week_start)));
    cal.set_cells(model(cells));
    cal.set_bars(model(bars));
    cal.set_week_numbers(model(if prefs.week_numbers {
        (0..6)
            .map(|r| iso_week(start + r * 7 + 3).to_string().into())
            .collect()
    } else {
        Vec::new()
    }));
    cal.set_busy(busy);
    agenda(cal, now, selected, items, pin);
}

/// The agenda card under the month grid.
fn agenda(cal: &Cal, now: &Now, day: i64, items: &[Instance], pin: &str) {
    let d = Date::from_days(day);
    let title = if day == now.today() {
        format!("Today · {} {}", d.weekday_name(), d.day)
    } else {
        format!("{} {} {}", d.weekday_name(), d.day, d.month_name())
    };
    let mut on: Vec<EventLine> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| covers(i, day))
        .map(|(k, i)| line(i, k, day, now))
        .collect();
    // what is still ahead first; the past ones go to "+N more"
    on.sort_by_key(|l| l.past);
    let more = on.len().saturating_sub(3);
    cal.set_agenda_title(title.to_uppercase().into());
    cal.set_agenda(model(on.into_iter().take(3).collect()));
    cal.set_agenda_pin(pin.into());
    cal.set_agenda_more(if more > 0 {
        format!("+{more} more").into()
    } else {
        "".into()
    });
}

fn strip(now: &Now, first: i64, selected: i64, items: &[Instance]) -> Vec<StripDay> {
    (0..7)
        .map(|k| {
            let day = first + k;
            let d = Date::from_days(day);
            let mut dots: Vec<Color> = Vec::new();
            for i in items.iter().filter(|i| covers(i, day)) {
                let c = rgb(colour(i));
                if dots.len() < 3 && !dots.contains(&c) {
                    dots.push(c);
                }
            }
            StripDay {
                letter: LETTERS[d.weekday()].into(),
                label: d.day.to_string().into(),
                weekend: d.weekday() >= 5,
                today: day == now.today(),
                selected: day == selected,
                dots: model(dots),
            }
        })
        .collect()
}

/// Timed pieces of `items` on `day` as blocks in column `col`, side by side where they overlap.
fn blocks(items: &[Instance], day: i64, col: i32, now: &Now, out: &mut Vec<Block>) {
    let (lo, hi) = (day * DAY, (day + 1) * DAY);
    let mut on: Vec<(usize, i64, i64)> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| !i.all_day && i.start < hi && i.end > lo && !(multi_day(i) && i.all_day))
        .map(|(k, i)| {
            (
                k,
                i.start.max(lo),
                i.end.min(hi).max(i.start.max(lo) + 15 * 60),
            )
        })
        .collect();
    on.sort_by_key(|&(_, s, _)| s);
    // clusters of overlapping events share the column's width
    let mut k = 0;
    while k < on.len() {
        let mut end = on[k].2;
        let mut j = k + 1;
        while j < on.len() && on[j].1 < end {
            end = end.max(on[j].2);
            j += 1;
        }
        let lanes = (j - k) as i32;
        for (lane, &(n, s, e)) in on[k..j].iter().enumerate() {
            let i = &items[n];
            let place = if i.place.is_empty() {
                String::new()
            } else {
                format!(" · {}", i.place)
            };
            out.push(Block {
                i: n as i32,
                col,
                start: hours(s) + if s == hi { 24.0 } else { 0.0 },
                hours: (e - s) as f32 / 3600.0,
                lane: lane as i32,
                lanes,
                title: i.title.clone().into(),
                sub: format!("{} – {}{}", hm(i.start), hm(i.end), place).into(),
                color: rgb(colour(i)),
                past: i.end <= now.local,
            });
        }
        k = j;
    }
}

/// `x` rounded down (no `f32::floor` in core).
fn floor(x: f32) -> f32 {
    let t = x as i32 as f32;
    if t > x { t - 1.0 } else { t }
}

/// The first hour the grid shows: that of the first block or of now (`now` < 0: none), at most `latest` (Week
/// always shows its 08); `latest` when there is nothing.
fn top_hour(b: &[Block], now: f32, latest: f32) -> f32 {
    let mut top = b.iter().map(|b| floor(b.start)).fold(f32::MAX, f32::min);
    if now >= 0.0 {
        top = top.min(floor(now));
    }
    if top == f32::MAX { latest } else { top }.clamp(0.0, 16.0)
}

/// Week (cal2-week): the 7 days from `first`, the all-day lane and the hour grid.
pub fn week(cal: &Cal, now: &Now, first: i64, selected: i64, items: &[Instance]) {
    let last = first + 6;
    let (a, b) = (Date::from_days(first), Date::from_days(last));
    let title = if a.month == b.month {
        format!("{} – {} {}", a.day, b.day, a.month_name())
    } else {
        format!(
            "{} {} – {} {}",
            a.day,
            &a.month_name()[..3],
            b.day,
            &b.month_name()[..3]
        )
    };
    let mut lane = Vec::new();
    for (k, i) in items.iter().enumerate() {
        if i.all_day || multi_day(i) {
            let (s, e) = (starts_day(i).max(first), last_day(i).min(last));
            if s <= e {
                lane.push(AllDay {
                    i: k as i32,
                    col: (s - first) as i32,
                    span: (e - s + 1) as i32,
                    title: i.title.clone().into(),
                    color: rgb(colour(i)),
                    cake: i.kind == Kind::Birthday,
                });
            }
        }
    }
    let mut out = Vec::new();
    for c in 0..7 {
        blocks(items, first + c, c as i32, now, &mut out);
    }
    let today = now.today();
    let on = (first..=last).contains(&today);
    let now_h = if on { hours(now.local) } else { -1.0 };
    cal.set_week_title(title.into());
    cal.set_week_sub(format!("Week {}", iso_week(first)).into());
    cal.set_week_days(model(strip(now, first, selected, &[])));
    cal.set_week_all_day(model(lane));
    cal.set_week_top(top_hour(&out, -1.0, 8.0).min(8.0));
    cal.set_week_blocks(model(out));
    cal.set_week_now(now_h);
    cal.set_week_now_col((today - first) as i32);
}

/// Day (cal2-day, -day-empty): `day` with its week strip, all-day events, the hour grid, and the alarms on it
/// (`pins`: local time, "14:30 Medicine").
pub fn day(
    cal: &Cal,
    now: &Now,
    prefs: &Prefs,
    day: i64,
    week_items: &[Instance],
    pins: &[(i64, String)],
) {
    let first = week_of(day, prefs.week_start);
    let on: Vec<(usize, &Instance)> = week_items
        .iter()
        .enumerate()
        .filter(|(_, i)| covers(i, day))
        .collect();
    let all_day: Vec<AllDay> = on
        .iter()
        .filter(|(_, i)| i.all_day || multi_day(i))
        .map(|&(k, i)| AllDay {
            i: k as i32,
            col: 0,
            span: 1,
            title: title_of(i).into(),
            color: rgb(colour(i)),
            cake: i.kind == Kind::Birthday,
        })
        .collect();
    let mut out = Vec::new();
    blocks(week_items, day, 0, now, &mut out);
    let pins: Vec<Pin> = pins
        .iter()
        .filter(|(t, _)| t.div_euclid(DAY) == day)
        .map(|(t, text)| Pin {
            at: hours(*t),
            text: text.into(),
        })
        .collect();
    let today = now.today();
    let now_h = if day == today { hours(now.local) } else { -1.0 };
    let mut line = long_day(day);
    if day == today {
        line.push_str(" · Today");
    }
    cal.set_day_strip(model(strip(now, first, day, week_items)));
    cal.set_day_line(line.into());
    cal.set_day_empty(if on.is_empty() && pins.is_empty() {
        format!(
            "No events, to-dos or alarms on {}.",
            Date::from_days(day).weekday_name()
        )
        .into()
    } else {
        "".into()
    });
    cal.set_day_all_day(model(all_day));
    // half an hour before the first thing (or now), as on the board
    cal.set_day_top(top_hour(&out, now_h, 8.0) - 0.5);
    cal.set_day_blocks(model(out));
    cal.set_day_pins(model(pins));
    cal.set_day_now(now_h);
    cal.set_day_now_text(hm(now.local).into());
}

fn flag(priority: u8) -> i32 {
    match priority {
        1..=4 => 2,
        5 => 1,
        _ => 0,
    }
}

fn due_text(t: &Todo, now: &Now) -> String {
    let Some(due) = t.due else {
        return String::new();
    };
    let day = due.div_euclid(DAY);
    let mut s = if day == now.today() {
        "Today".to_string()
    } else {
        short_day(day)
    };
    if !t.due_all_day {
        s = format!("{s}, {}", hm(due));
    }
    s
}

/// To-do (cal2-todo, -todo-done): the open ones in Overdue, Today, Upcoming and No date, or (`done`) the completed
/// ones (latest first); the tally of the week around today (done in the last 7 days, of those and the open ones due
/// by 6 days on) and the footer's badge.
pub fn todos(cal: &Cal, now: &Now, list: &[Todo], done: bool) {
    let today = now.today();
    let day_of = |t: &Todo| t.due.map(|d| d.div_euclid(DAY));
    let mut groups: Vec<(&str, bool, Vec<TodoLine>)> = vec![
        ("OVERDUE", true, vec![]),
        ("TODAY", false, vec![]),
        ("UPCOMING", false, vec![]),
        ("NO DATE", false, vec![]),
    ];
    let mut finished = Vec::new();
    let (mut overdue, mut due_today, mut total, mut closed) = (0, 0, 0, 0);
    for (k, t) in list.iter().enumerate() {
        let day = day_of(t);
        let done_day = t.completed.map(|c| (c + i64::from(now.tz)).div_euclid(DAY));
        if t.done && done_day.is_some_and(|d| d > today - 7) {
            total += 1;
            closed += 1;
        } else if !t.done && day.is_some_and(|d| d < today + 7) {
            total += 1;
        }
        if t.done {
            finished.push((
                t.completed.unwrap_or(0),
                TodoLine {
                    i: k as i32,
                    title: t.title.clone().into(),
                    sub: done_day
                        .map(|d| format!("Done {}", short_day(d)))
                        .unwrap_or_else(|| "Done".into())
                        .into(),
                    done: true,
                    overdue: false,
                    flag: 0,
                },
            ));
            continue;
        }
        let g = match day {
            Some(d) if d < today => 0,
            Some(d) if d == today => 1,
            Some(_) => 2,
            None => 3,
        };
        let mut sub = due_text(t, now);
        if g == 0 {
            overdue += 1;
            sub = format!("Due {sub}");
        }
        if g == 1 {
            due_today += 1;
        }
        let note = t.notes.lines().next().unwrap_or("");
        if !note.is_empty() {
            sub = if sub.is_empty() {
                note.into()
            } else {
                format!("{sub} · {note}")
            };
        }
        groups[g].2.push(TodoLine {
            i: k as i32,
            title: t.title.clone().into(),
            sub: sub.into(),
            done: false,
            overdue: g == 0,
            flag: flag(t.priority),
        });
    }
    finished.sort_by_key(|l: &(i64, TodoLine)| core::cmp::Reverse(l.0));
    let finished: Vec<TodoLine> = finished.into_iter().map(|(_, l)| l).collect();
    let shown: Vec<TodoGroup> = if done {
        vec![TodoGroup {
            title: format!("DONE · {}", finished.len()).into(),
            danger: false,
            rows: model(finished.clone()),
        }]
    } else {
        groups
            .into_iter()
            .filter(|g| !g.2.is_empty())
            .map(|(title, danger, rows)| TodoGroup {
                title: title.into(),
                danger,
                rows: model(rows),
            })
            .collect()
    };
    cal.set_todo_done(closed);
    cal.set_todo_total(total);
    cal.set_todo_overdue(if overdue > 0 {
        format!("{overdue} overdue").into()
    } else {
        "".into()
    });
    cal.set_todo_due(match (overdue > 0, due_today) {
        (_, 0) => "".into(),
        (true, n) => format!("· {n} due today").into(),
        (false, n) => format!("{n} due today").into(),
    });
    cal.set_todo_groups(model(shown));
    cal.set_todo_clear(done && !finished.is_empty());
    cal.set_todo_done_link(if !done && !finished.is_empty() {
        format!("Show done · {}", finished.len()).into()
    } else {
        "".into()
    });
    cal.set_todo_badge(overdue);
}

/// "Every Monday", "Every 2 weeks"...
#[must_use]
pub fn repeat_text(r: &Repeat, start: i64) -> String {
    let d = Date::from_days(start.div_euclid(DAY));
    match r {
        Repeat::Never => "Never".into(),
        Repeat::Other => "Custom".into(),
        Repeat::Every {
            freq,
            interval,
            days,
            ..
        } => {
            let n = (*interval).max(1);
            match freq {
                Freq::Daily if n == 1 => "Every day".into(),
                Freq::Daily => format!("Every {n} days"),
                Freq::Weekly if *days == 0b1_1111 => "Every weekday (Mon–Fri)".into(),
                Freq::Weekly if n == 1 => format!("Every {}", d.weekday_name()),
                Freq::Weekly => format!("Every {n} weeks"),
                Freq::Monthly if n == 1 => {
                    format!("Every month on the {}", ordinal(u32::from(d.day)))
                }
                Freq::Monthly => format!("Every {n} months"),
                Freq::Yearly => format!("Every year on {} {}", d.day, d.month_name()),
            }
        }
    }
}

/// The Repeat sheet's choices for an event starting at `start`, and which one `r` is (cal2-repeat-sheet).
#[must_use]
pub fn repeat_choices(start: i64) -> Vec<(String, Repeat)> {
    let d = Date::from_days(start.div_euclid(DAY));
    let every = |freq, interval, days| Repeat::Every {
        freq,
        interval,
        days,
        count: 0,
        until: None,
    };
    vec![
        ("Never".into(), Repeat::Never),
        ("Every day".into(), every(Freq::Daily, 1, 0)),
        (
            "Every weekday (Mon–Fri)".into(),
            every(Freq::Weekly, 1, 0b1_1111),
        ),
        (
            format!("Every week on {}", d.weekday_name()),
            every(Freq::Weekly, 1, 0),
        ),
        ("Every 2 weeks".into(), every(Freq::Weekly, 2, 0)),
        (
            format!("Every month on the {}", ordinal(u32::from(d.day))),
            every(Freq::Monthly, 1, 0),
        ),
        (
            format!("Every year on {} {}", d.day, d.month_name()),
            every(Freq::Yearly, 1, 0),
        ),
    ]
}

/// "15 min before", "None"
#[must_use]
pub fn reminder_text(m: Option<u16>, all_day: bool) -> String {
    match m {
        None => "None".into(),
        Some(0) => "At the start".into(),
        Some(ALL_DAY_EVE) if all_day => "1 day before, 09:00".into(),
        Some(1440) => "1 day before".into(),
        Some(m) if m % 60 == 0 => {
            let h = m / 60;
            format!("{h} hour{} before", if h == 1 { "" } else { "s" })
        }
        Some(m) => format!("{m} min before"),
    }
}

/// The Reminder sheet's minutes for choice `k` (an all-day event's last choice is 09:00 the day before).
#[must_use]
pub fn reminder_at(k: usize, all_day: bool) -> Option<u16> {
    match REMINDERS.get(k).copied().flatten() {
        Some(1440) if all_day => Some(ALL_DAY_EVE),
        m => m,
    }
}

/// Sets the Reminder sheet for `m` (cal2-reminder-sheet).
pub fn reminder_sheet(cal: &Cal, m: Option<u16>, all_day: bool, ring: bool) {
    let labels: Vec<SharedString> = (0..REMINDERS.len())
        .map(|k| reminder_text(reminder_at(k, all_day), all_day).into())
        .collect();
    let current = (0..REMINDERS.len())
        .position(|k| reminder_at(k, all_day) == m)
        .unwrap_or(0);
    cal.set_reminder_choices(model(labels));
    cal.set_reminder_current(current as i32);
    cal.set_reminder_ring(ring);
}

/// Sets the Repeat sheet for an event at `start` repeating as `r`.
pub fn repeat_sheet(cal: &Cal, r: &Repeat, start: i64) {
    let choices = repeat_choices(start);
    let current = choices.iter().position(|(_, c)| c == r).unwrap_or(0);
    cal.set_repeat_choices(model(choices.into_iter().map(|(s, _)| s.into()).collect()));
    cal.set_repeat_current(current as i32);
    cal.set_repeat_ends(
        match r {
            Repeat::Every { count, .. } if *count > 0 => format!("Ends: after {count} times"),
            Repeat::Every {
                until: Some(day), ..
            } => format!("Ends: {}", short_day(*day)),
            _ => "Ends: never".into(),
        }
        .into(),
    );
}

/// "in 5 h 36 min", "now", "ended"
fn until(start: i64, end: i64, now: i64) -> String {
    if now >= end {
        return "ended".into();
    }
    if now >= start {
        return "now".into();
    }
    let m = (start - now) / 60;
    match m {
        0..=59 => format!("in {m} min"),
        60..=1439 => format!("in {} h {} min", m / 60, m % 60),
        _ => {
            let d = (m + 1439) / 1440;
            format!("in {d} day{}", if d == 1 { "" } else { "s" })
        }
    }
}

/// The event (cal2-event-view): `d` is the series or occurrence as stored (repeat, reminder, notes).
pub fn event(cal: &Cal, now: &Now, i: &Instance, d: &EventDraft, cal_name: &str, ring: bool) {
    let day = starts_day(i);
    let mut when = long_day(day);
    if multi_day(i) {
        when = format!("{} – {}", short_day(day), short_day(last_day(i)));
    }
    when = format!("{when} · {}", until(i.start, i.end, now.local));
    let mut rows = Vec::new();
    if !d.place.is_empty() {
        rows.push(Detail {
            glyph: 0,
            title: d.place.clone().into(),
            label: "Location".into(),
        });
    }
    if d.repeat != Repeat::Never {
        rows.push(Detail {
            glyph: 1,
            title: repeat_text(&d.repeat, d.start).into(),
            label: "Repeat".into(),
        });
    }
    if d.reminder.is_some() {
        let mut t = reminder_text(d.reminder, d.all_day);
        if ring {
            t.push_str(" · rings like an alarm");
        }
        rows.push(Detail {
            glyph: 2,
            title: t.into(),
            label: "Reminder".into(),
        });
    }
    if !d.notes.is_empty() {
        rows.push(Detail {
            glyph: 3,
            title: d.notes.clone().into(),
            label: "".into(),
        });
    }
    let timed = !i.all_day;
    cal.set_ev_cal(cal_name.into());
    cal.set_ev_color(rgb(colour(i)));
    cal.set_ev_title(i.title.clone().into());
    cal.set_ev_timed(timed);
    let (s, e) = (i.start.rem_euclid(DAY), i.end.rem_euclid(DAY));
    cal.set_ev_h0((s / 3600) as i32);
    cal.set_ev_m0((s / 60 % 60) as i32);
    cal.set_ev_h1((e / 3600) as i32);
    cal.set_ev_m1((e / 60 % 60) as i32);
    cal.set_ev_when(when.into());
    cal.set_ev_rows(model(rows));
}

/// "Mon 16 Nov 2026 · 16:00"
fn stamp(t: i64, all_day: bool) -> String {
    let d = Date::from_days(t.div_euclid(DAY));
    let date = format!(
        "{} {} {} {}",
        &d.weekday_name()[..3],
        d.day,
        &d.month_name()[..3],
        d.year
    );
    if all_day {
        date
    } else {
        format!("{date} · {}", hm(t))
    }
}

/// "17:00 · 1 h", or the end's day when it is another day
fn ends(d: &EventDraft) -> String {
    if d.all_day {
        return stamp(d.end - DAY, true);
    }
    let len = (d.end - d.start) / 60;
    let dur = match (len / 60, len % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    };
    if d.end.div_euclid(DAY) == d.start.div_euclid(DAY) {
        format!("{} · {dur}", hm(d.end))
    } else {
        format!("{} · {dur}", stamp(d.end, false))
    }
}

/// The event form (cal2-event-edit): `title` "New event" or "Edit event".
pub fn event_form(cal: &Cal, title: &str, d: &EventDraft, cals: &[CalInfo]) {
    let c = cals.get(usize::from(d.cal));
    cal.set_form_title(title.into());
    cal.set_f_title(d.title.clone().into());
    cal.set_f_place(d.place.clone().into());
    cal.set_f_notes(d.notes.clone().into());
    cal.set_f_all_day(d.all_day);
    event_fields(cal, d, c);
}

/// The form's rows that follow from the draft (after a sheet or picker changed it); the typed fields stay.
pub fn event_fields(cal: &Cal, d: &EventDraft, c: Option<&CalInfo>) {
    cal.set_f_starts(stamp(d.start, d.all_day).into());
    cal.set_f_ends(ends(d).into());
    cal.set_f_repeat(repeat_text(&d.repeat, d.start).into());
    cal.set_f_reminder(reminder_text(d.reminder, d.all_day).into());
    cal.set_f_cal(c.map(|c| c.name.clone()).unwrap_or_default().into());
    cal.set_f_cal_color(rgb(c.map_or(SWATCHES[0], |c| c.colour)));
    cal.set_f_date(
        {
            let day = Date::from_days(d.start.div_euclid(DAY));
            format!("{} {} · every year", day.day, day.month_name())
        }
        .into(),
    );
}

/// The anniversary form (cal2-anniversary-edit): `since` the first year, `next` the count it shows next time.
pub fn anniversary_form(
    cal: &Cal,
    title: &str,
    d: &EventDraft,
    cals: &[CalInfo],
    since: i32,
    next: u16,
) {
    event_form(cal, title, d, cals);
    cal.set_f_since(format!("{since} · shows “{}” next time", ordinal(u32::from(next))).into());
}

/// The to-do form (cal2-todo-edit); `list` is the calendar it shows under.
pub fn todo_form(cal: &Cal, title: &str, t: &Todo, now: &Now, list: &str) {
    cal.set_form_title(title.into());
    cal.set_f_title(t.title.clone().into());
    cal.set_f_notes(t.notes.clone().into());
    cal.set_f_priority(match t.priority {
        1..=4 => 2,
        6..=9 => 0,
        _ => 1,
    });
    todo_fields(cal, t, now, list);
}

/// The to-do form's rows that follow from the draft.
pub fn todo_fields(cal: &Cal, t: &Todo, now: &Now, list: &str) {
    cal.set_f_due(
        t.due
            .map(|d| {
                let mut s = long_day(d.div_euclid(DAY));
                if !t.due_all_day {
                    s = format!("{s}, {}", hm(d));
                }
                let _ = now;
                s
            })
            .unwrap_or_else(|| "No date".into())
            .into(),
    );
    cal.set_f_reminder(
        match t.reminder {
            None => "None".into(),
            // a due day's reminder at 09:00
            Some(m) if t.due_all_day && m == 0 => "On the day, 09:00".into(),
            m => reminder_text(m, t.due_all_day),
        }
        .into(),
    );
    cal.set_f_cal(list.into());
}

/// The priority a form's segment (0 Low, 1 Normal, 2 High) stores.
#[must_use]
pub fn priority(segment: i32) -> u8 {
    match segment {
        0 => 9,
        2 => 1,
        _ => 0,
    }
}

/// A birthday (cal2-birthday-view); `todos`: to-dos that name the person.
pub fn birthday(
    cal: &Cal,
    now: &Now,
    i: &Instance,
    born: Option<i32>,
    todos: &[Todo],
    remind: &str,
) {
    let d = Date::from_days(starts_day(i));
    let initials: String = i
        .title
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect();
    let today = starts_day(i) == now.today();
    let turns = match (i.years, today) {
        (0, true) => "Birthday today".into(),
        (0, false) => format!("Birthday {}", short_day(starts_day(i))),
        (n, true) => format!("Turns {n} today"),
        (n, false) => format!("Turns {n} on {}", short_day(starts_day(i))),
    };
    let mut rows: Vec<Detail> = todos
        .iter()
        .map(|t| Detail {
            glyph: 4,
            title: t.title.clone().into(),
            label: format!("To-do · due {}", due_text(t, now).to_lowercase()).into(),
        })
        .collect();
    if !remind.is_empty() {
        rows.push(Detail {
            glyph: 2,
            title: remind.into(),
            label: "Reminder for birthdays".into(),
        });
    }
    cal.set_bd_initials(initials.to_uppercase().into());
    cal.set_bd_name(i.title.clone().into());
    cal.set_bd_turns(turns.into());
    cal.set_bd_date(
        match born {
            Some(y) => format!("{} {} {y} · from Contacts", d.day, d.month_name()),
            None => format!("{} {} · from Contacts", d.day, d.month_name()),
        }
        .into(),
    );
    cal.set_bd_rows(model(rows));
    cal.set_bd_note(
        "Read only. Call, Message and Contact open Phone, Messages and Contacts.".into(),
    );
}

/// What a search result opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    Event(usize),
    Todo(usize),
}

fn split_hit(title: &str, q: &str) -> Option<(String, String, String)> {
    let lower = title.to_lowercase();
    let at = lower.find(&q.to_lowercase())?;
    // the match's length in the title (case changes keep ASCII lengths; others fall back to the query's)
    let end = (at + q.len()).min(title.len());
    if !title.is_char_boundary(at) || !title.is_char_boundary(end) {
        return Some((title.into(), String::new(), String::new()));
    }
    Some((
        title[..at].into(),
        title[at..end].into(),
        title[end..].into(),
    ))
}

/// Search (cal2-search): events whose title or place has `q`, then to-dos, by month; `filter` 0 all, 1 events, 2
/// to-dos, 3 anniversaries: what is ahead first, then the past, latest first. Returns what each row opens.
pub fn search(
    cal: &Cal,
    now: &Now,
    q: &str,
    filter: i32,
    items: &[Instance],
    todos: &[Todo],
    cal_names: &[CalInfo],
) -> Vec<Found> {
    let mut found = Vec::new();
    let mut hits = Vec::new();
    let q = q.trim();
    let mut month = String::new();
    let mut section = |day: i64| {
        let d = Date::from_days(day);
        let m = format!("{} {}", d.month_name(), d.year).to_uppercase();
        if m == month {
            String::new()
        } else {
            month = m.clone();
            m
        }
    };
    if !q.is_empty() {
        let mut seen: Vec<(&str, i64)> = Vec::new();
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by_key(|&k| {
            let i = &items[k];
            let past = i.end <= now.local;
            (past, if past { -i.start } else { i.start })
        });
        for k in order {
            let i = &items[k];
            let wanted = match filter {
                1 => i.kind == Kind::Event,
                2 => false,
                3 => i.kind != Kind::Event,
                _ => true,
            };
            if !wanted || seen.contains(&(i.uid.as_str(), i.start)) {
                continue;
            }
            let lq = q.to_lowercase();
            let Some((pre, hit, post)) = split_hit(&i.title, q).or_else(|| {
                i.place
                    .to_lowercase()
                    .contains(&lq)
                    .then(|| (i.title.clone(), String::new(), String::new()))
            }) else {
                continue;
            };
            seen.push((i.uid.as_str(), i.start));
            let day = starts_day(i);
            let cal_name = cal_names
                .get(usize::from(i.cal))
                .map_or("", |c| c.name.as_str());
            let mut sub = if i.all_day {
                "All day".to_string()
            } else {
                hm(i.start)
            };
            for part in [i.place.as_str(), cal_name] {
                if !part.is_empty() {
                    sub = format!("{sub} · {part}");
                }
            }
            let d = Date::from_days(day);
            hits.push(Hit {
                i: found.len() as i32,
                section: section(day).into(),
                date: format!("{} {}", &d.weekday_name()[..3], d.day).into(),
                pre: pre.into(),
                hit: hit.into(),
                post: post.into(),
                sub: sub.into(),
                color: rgb(colour(i)),
                past: i.end <= now.local,
            });
            found.push(Found::Event(k));
        }
        if filter == 0 || filter == 2 {
            for (k, t) in todos.iter().enumerate() {
                let Some((pre, hit, post)) = split_hit(&t.title, q) else {
                    continue;
                };
                let day = t.due.map_or(now.today(), |d| d.div_euclid(DAY));
                let d = Date::from_days(day);
                hits.push(Hit {
                    i: found.len() as i32,
                    section: section(day).into(),
                    date: format!("{} {}", &d.weekday_name()[..3], d.day).into(),
                    pre: pre.into(),
                    hit: hit.into(),
                    post: post.into(),
                    sub: if t.done { "To-do · done" } else { "To-do" }.into(),
                    color: rgb(0x22B46A),
                    past: t.done,
                });
                found.push(Found::Todo(k));
            }
        }
    }
    cal.set_query(q.into());
    cal.set_filter(filter);
    cal.set_hits(model(hits));
    found
}

/// Settings (cal2-settings): `entries` stored, `zone` the phone's ("Yerevan").
pub fn settings(cal: &Cal, p: &Prefs, zone: &str, entries: usize) {
    const DAYS: [&str; 7] = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ];
    cal.set_set_week_start(DAYS[usize::from(p.week_start % 7)].into());
    cal.set_set_week_numbers(p.week_numbers);
    cal.set_set_open_on(VIEWS[usize::from(p.view.min(3))].into());
    cal.set_set_reminder(reminder_text(Some(p.reminder), false).into());
    cal.set_set_length(length_text(p.length).into());
    cal.set_set_ring(p.ring);
    cal.set_set_birthdays(p.birthdays);
    cal.set_set_zone(format!("Follows the phone · {zone}").into());
    cal.set_set_stored(format!("C:\\private\\e0008003\\calendar.ics · {entries} entries").into());
}

/// The tabs, as `calendar.view` and the log name them.
pub const VIEWS: [&str; 4] = ["Month", "Week", "Day", "To-do"];
/// Default length's choices, minutes.
pub const LENGTHS: [u16; 5] = [15, 30, 60, 90, 120];

#[must_use]
pub fn length_text(m: u16) -> String {
    match m {
        60 => "1 hour".into(),
        m if m % 60 == 0 => format!("{} hours", m / 60),
        m if m > 60 => format!("{} h {} min", m / 60, m % 60),
        m => format!("{m} min"),
    }
}

/// Opens the shared choice sheet.
pub fn choose(cal: &Cal, title: &str, choices: &[String], current: usize) {
    cal.set_choice_title(title.into());
    cal.set_choices(model(choices.iter().map(|s| s.into()).collect()));
    cal.set_choice_current(current as i32);
    cal.set_s_choice(true);
}

/// Calendars (cal2-calendar-color): every calendar with its switch (off: in `hidden`), Birthdays last; `edit` is the
/// one whose colour card shows.
pub fn calendars(cal: &Cal, cals: &[CalInfo], hidden: u32, birthdays: bool, edit: Option<usize>) {
    let mut rows: Vec<CalLine> = cals
        .iter()
        .enumerate()
        .map(|(k, c)| CalLine {
            name: c.name.clone().into(),
            sub: format!("{} event{}", c.count, if c.count == 1 { "" } else { "s" }).into(),
            color: rgb(c.colour),
            on: hidden & (1 << k) == 0,
        })
        .collect();
    rows.push(CalLine {
        name: "Birthdays".into(),
        sub: "From Contacts · read only".into(),
        color: rgb(BIRTHDAYS),
        on: birthdays,
    });
    let swatch = edit
        .and_then(|e| cals.get(e))
        .and_then(|c| SWATCHES.iter().position(|&s| s == c.colour))
        .map_or(-1, |s| s as i32);
    cal.set_cals(model(rows));
    cal.set_cal_edit(edit.map_or(-1, |e| e as i32));
    cal.set_swatches(model(SWATCHES.iter().map(|&c| rgb(c)).collect()));
    cal.set_swatch(swatch);
}

/// Add to Calendar (cal2-ics-import): the file's first event `d` of `n`, into calendar `into`; `clash` the events
/// already on its day.
#[allow(clippy::too_many_arguments)]
pub fn import(
    cal: &Cal,
    from: &str,
    path: &str,
    d: &EventDraft,
    n: usize,
    into: &CalInfo,
    clash: &[Instance],
) {
    let day = d.start.div_euclid(DAY);
    let (s, e) = (d.start.rem_euclid(DAY), d.end.rem_euclid(DAY));
    cal.set_im_from(format!("Opened from {from}").into());
    cal.set_im_path(path.into());
    cal.set_im_title(d.title.clone().into());
    cal.set_im_timed(!d.all_day);
    cal.set_im_h0((s / 3600) as i32);
    cal.set_im_m0((s / 60 % 60) as i32);
    cal.set_im_h1((e / 3600) as i32);
    cal.set_im_m1((e / 60 % 60) as i32);
    let date = Date::from_days(day);
    cal.set_im_date(
        format!(
            "{}, {} {} {}",
            date.weekday_name(),
            date.day,
            date.month_name(),
            date.year
        )
        .into(),
    );
    cal.set_im_place(d.place.clone().into());
    cal.set_im_cal(into.name.clone().into());
    cal.set_im_cal_color(rgb(into.colour));
    cal.set_im_reminder(
        match d.reminder {
            None => "None".into(),
            m => format!("From the file: {}", reminder_text(m, d.all_day)),
        }
        .into(),
    );
    let weekday = date.weekday_name();
    cal.set_im_clash(!clash.is_empty());
    cal.set_im_note(
        match clash.first() {
            None => format!("No clash: {weekday} {} has nothing else.", date.day),
            Some(c) if c.all_day => format!("Also on {weekday}: {}", title_of(c)),
            Some(c) => format!("Clash: {} at {}", title_of(c), hm(c.start)),
        }
        .into(),
    );
    cal.set_im_add(if n > 1 {
        format!("Add {n} events").into()
    } else {
        "Add event".into()
    });
}

/// An .ics file's first event and how many events it has (for Add to Calendar), read the way DTSRV will import it.
#[must_use]
pub fn preview(text: &str, zone: &dyn Zone) -> Option<(EventDraft, usize)> {
    let mut s = Store::load("", "");
    s.import(text, 0, 0);
    let events: Vec<EventDraft> = calendar_core::Ics::parse_all(text)
        .iter()
        .flat_map(|f| (0..f.len()).filter_map(|i| f.uid(i)).collect::<Vec<_>>())
        .filter_map(|u| s.get(&u, None, zone))
        .collect();
    let n = events.len();
    events.into_iter().next().map(|d| (d, n))
}

/// A reminder ringing (dt2-ring-event): `snooze` minutes on the Snooze button.
pub fn ring(cal: &Cal, now: &Now, i: &Instance, cal_name: &str, snooze: u8) {
    let head = if now.local >= i.start {
        "Reminder · now".to_string()
    } else {
        format!("Reminder · {}", until(i.start, i.end, now.local))
    };
    let mut sub = if i.all_day {
        long_day(starts_day(i))
    } else {
        format!("{} – {}", hm(i.start), hm(i.end))
    };
    if !i.place.is_empty() {
        sub = format!("{sub} · {}", i.place);
    }
    let s = i.start.rem_euclid(DAY);
    cal.set_ring_head(head.to_uppercase().into());
    cal.set_ring_cal(cal_name.into());
    cal.set_ring_color(rgb(colour(i)));
    cal.set_ring_title(title_of(i).into());
    cal.set_ring_timed(!i.all_day);
    cal.set_ring_h((s / 3600) as i32);
    cal.set_ring_m((s / 60 % 60) as i32);
    cal.set_ring_sub(sub.into());
    cal.set_ring_snooze(format!("Snooze {snooze} min").into());
}

/// The text Share sends (Messages, Copy) and shows (cal2-share).
#[must_use]
pub fn share_text(i: &Instance) -> String {
    let day = starts_day(i);
    let d = Date::from_days(day);
    let date = format!(
        "{} {} {} {}",
        &d.weekday_name()[..3],
        d.day,
        &d.month_name()[..3],
        d.year
    );
    let mut s = format!("{}\n", title_of(i));
    if i.all_day {
        s.push_str(&date);
    } else {
        s.push_str(&format!("{date}, {}–{}", hm(i.start), hm(i.end)));
    }
    if !i.place.is_empty() {
        s.push('\n');
        s.push_str(&i.place);
    }
    s
}

/// Sets the New sheet's title for `day` ("New on Monday 16").
pub fn new_sheet(cal: &Cal, day: i64) {
    let d = Date::from_days(day);
    cal.set_new_title(format!("New on {} {}", d.weekday_name(), d.day).into());
    cal.set_s_new(true);
}

/// Sets Go to month on `year-month`.
pub fn month_sheet(cal: &Cal, year: i32, month: u8) {
    cal.set_pick_year(year);
    cal.set_pick_month(i32::from(month));
    cal.set_s_month(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weeks_and_numbers() {
        let d = Date {
            year: 2026,
            month: 11,
            day: 16,
        }
        .days();
        assert_eq!(iso_week(d), 47);
        assert_eq!(week_of(d + 3, 0), d);
        // Sunday-first: the Sunday before
        assert_eq!(week_of(d, 6), d - 1);
        assert_eq!(
            grid_start(2026, 11, 0),
            Date {
                year: 2026,
                month: 10,
                day: 26
            }
            .days()
        );
        assert_eq!(
            iso_week(
                Date {
                    year: 2027,
                    month: 1,
                    day: 1
                }
                .days()
            ),
            53
        );
    }

    #[test]
    fn texts() {
        assert_eq!(hm(16 * 3600 + 5 * 60), "16:05");
        assert_eq!(ordinal(16), "16th");
        assert_eq!(ordinal(22), "22nd");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(reminder_text(Some(15), false), "15 min before");
        assert_eq!(reminder_text(Some(60), false), "1 hour before");
        assert_eq!(
            reminder_text(reminder_at(5, true), true),
            "1 day before, 09:00"
        );
        assert_eq!(until(3600, 7200, 0), "in 1 h 0 min");
        assert_eq!(
            split_hit("Design review: Clock v2", "REVIEW"),
            Some(("Design ".into(), "review".into(), ": Clock v2".into()))
        );
        let monday = Date {
            year: 2026,
            month: 11,
            day: 16,
        }
        .days()
            * DAY
            + 16 * 3600;
        let weekly = repeat_choices(monday)[3].1.clone();
        assert_eq!(repeat_text(&weekly, monday), "Every Monday");
    }
}
