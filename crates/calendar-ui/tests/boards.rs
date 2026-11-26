// SPDX-License-Identifier: GPL-3.0-only
// Calendar v2, one snapshot per design board (cal2-*, dt2-ring-event), from a calendar-core Store holding the boards'
// sample week (Monday 16 November 2026, 10:24, Yerevan) through the same `view` code NEOCAL runs.
mod common;
use calendar_core::{Birthday, DAY, EventDraft, Freq, Instance, Kind, Repeat, Store, Todo};
use calendar_ui::Cal;
use calendar_ui::view::{self, CalInfo, Now, Prefs};
use pim_ics::Date;
use slint::{ComponentHandle, Model};

const DP: f32 = 3.0;
/// Yerevan, +04:00
const ZONE: i32 = 4 * 3600;
const CONCERT: &str = include_str!("../../calendar-core/tests/data/concert.ics");

fn day(m: u8, d: u8) -> i64 {
    Date {
        year: 2026,
        month: m,
        day: d,
    }
    .days()
}

fn at(m: u8, d: u8, h: i64, min: i64) -> i64 {
    day(m, d) * DAY + h * 3600 + min * 60
}

fn now() -> Now {
    Now {
        local: at(11, 16, 10, 24),
        tz: ZONE,
    }
}

struct Ev {
    cal: u8,
    title: &'static str,
    place: &'static str,
    start: i64,
    end: i64,
    all_day: bool,
    repeat: Repeat,
    reminder: Option<u16>,
    notes: &'static str,
    kind: Kind,
}

const EV: Ev = Ev {
    cal: 0,
    title: "",
    place: "",
    start: 0,
    end: 0,
    all_day: false,
    repeat: Repeat::Never,
    reminder: None,
    notes: "",
    kind: Kind::Event,
};

fn weekly(days: u8) -> Repeat {
    Repeat::Every {
        freq: Freq::Weekly,
        interval: 1,
        days,
        count: 0,
        until: None,
    }
}

/// Personal, Work, Family, Projects and the boards' events and to-dos.
fn store() -> Store {
    let mut s = Store::load("", "");
    s.set_calendar(0, "Personal", 0x22B46A);
    s.add_calendar("Work", 0x2F80ED);
    s.add_calendar("Family", 0xF2994A);
    s.add_calendar("Projects", 0x8656E8);
    let utc = at(11, 1, 9, 0) - i64::from(ZONE);
    let events = [
        Ev {
            cal: 1,
            title: "Stand-up",
            place: "Video call",
            start: at(11, 2, 9, 30),
            end: at(11, 2, 10, 0),
            // Monday, Tuesday, Thursday
            repeat: weekly(0b1011),
            ..EV
        },
        Ev {
            cal: 3,
            title: "Kernel review",
            place: "Room 4",
            start: at(10, 26, 16, 0),
            end: at(10, 26, 17, 0),
            repeat: weekly(0),
            reminder: Some(15),
            notes: "Walk through the AArch64 ILP32 exec table and the WB1 changes. Bring the Pixel.",
            ..EV
        },
        Ev {
            title: "Lunch with Ben",
            place: "Harbour Café",
            start: at(11, 16, 13, 0),
            end: at(11, 16, 14, 0),
            ..EV
        },
        Ev {
            cal: 2,
            title: "Dinner",
            place: "Home",
            start: at(11, 16, 19, 0),
            end: at(11, 16, 21, 0),
            ..EV
        },
        Ev {
            title: "Dentist",
            place: "Dr. Petrosyan",
            start: at(11, 17, 12, 0),
            end: at(11, 17, 13, 0),
            ..EV
        },
        Ev {
            cal: 3,
            title: "NeoSymbian meetup · Berlin",
            place: "c-base",
            start: day(11, 18) * DAY,
            end: day(11, 21) * DAY,
            all_day: true,
            ..EV
        },
        Ev {
            cal: 3,
            title: "Talk: Rust on EKA2",
            place: "Main hall",
            start: at(11, 18, 10, 0),
            end: at(11, 18, 10, 45),
            ..EV
        },
        Ev {
            cal: 1,
            title: "Speaker dinner",
            place: "Prenzlauer Berg",
            start: at(11, 18, 19, 30),
            end: at(11, 18, 22, 0),
            ..EV
        },
        Ev {
            cal: 3,
            title: "Workshop",
            place: "c-base",
            start: at(11, 20, 14, 0),
            end: at(11, 20, 16, 0),
            ..EV
        },
        Ev {
            cal: 2,
            title: "Market",
            place: "Vernissage",
            start: at(11, 21, 11, 0),
            end: at(11, 21, 13, 0),
            ..EV
        },
        Ev {
            cal: 1,
            title: "Design review: Clock v2",
            place: "Video call",
            start: at(11, 26, 11, 0),
            end: at(11, 26, 12, 0),
            ..EV
        },
        Ev {
            cal: 1,
            title: "Sprint planning",
            start: at(11, 6, 10, 0),
            end: at(11, 6, 11, 0),
            ..EV
        },
        Ev {
            title: "Yoga",
            start: at(11, 6, 18, 0),
            end: at(11, 6, 19, 0),
            ..EV
        },
        Ev {
            cal: 2,
            title: "Grandma's visit",
            start: at(11, 13, 17, 0),
            end: at(11, 13, 19, 0),
            ..EV
        },
        Ev {
            title: "Haircut",
            start: at(11, 25, 18, 0),
            end: at(11, 25, 18, 30),
            ..EV
        },
        Ev {
            cal: 2,
            title: "Wedding anniversary",
            start: day(6, 3) * DAY,
            end: day(6, 4) * DAY,
            all_day: true,
            kind: Kind::Anniversary,
            repeat: Repeat::Every {
                freq: Freq::Yearly,
                interval: 1,
                days: 0,
                count: 0,
                until: None,
            },
            ..EV
        },
    ];
    for e in events {
        let d = EventDraft {
            cal: e.cal,
            kind: e.kind,
            all_day: e.all_day,
            start: e.start,
            end: e.end,
            title: e.title.into(),
            place: e.place.into(),
            notes: e.notes.into(),
            repeat: e.repeat,
            reminder: e.reminder,
            ..EventDraft::default()
        };
        s.put(&d, &ZONE, utc).expect("put");
    }
    let todo = |title: &str, due: Option<i64>, priority: u8, done: Option<i64>, notes: &str| Todo {
        title: title.into(),
        notes: notes.into(),
        due,
        due_all_day: true,
        priority,
        done: done.is_some(),
        completed: done.map(|d| d - i64::from(ZONE)),
        ..Todo::default()
    };
    for t in [
        todo("Renew passport", Some(day(11, 12) * DAY), 1, None, ""),
        todo("Pay internet bill", Some(day(11, 16) * DAY), 5, None, ""),
        todo(
            "Buy a gift for Ava",
            Some(day(11, 16) * DAY),
            0,
            None,
            "linked to her birthday",
        ),
        todo(
            "Book flights to Berlin",
            Some(day(11, 17) * DAY),
            1,
            None,
            "",
        ),
        todo(
            "Prepare meetup slides",
            Some(day(11, 18) * DAY),
            0,
            None,
            "",
        ),
        todo(
            "Send meetup slides",
            Some(day(11, 15) * DAY),
            0,
            Some(at(11, 15, 20, 0)),
            "",
        ),
        todo(
            "Pay rent",
            Some(day(11, 13) * DAY),
            0,
            Some(at(11, 13, 9, 0)),
            "",
        ),
        todo(
            "Call the bank",
            Some(day(11, 12) * DAY),
            0,
            Some(at(11, 12, 11, 0)),
            "",
        ),
        todo(
            "Write a review of the N8 port",
            Some(day(10, 9) * DAY),
            0,
            Some(at(10, 9, 12, 0)),
            "",
        ),
    ] {
        s.put_todo(&t, &ZONE, utc).expect("todo");
    }
    s
}

fn birthdays() -> Vec<Birthday> {
    vec![Birthday {
        contact_id: 7,
        name: "Ava Lindqvist".into(),
        month: 11,
        day: 16,
        year: Some(1994),
    }]
}

fn infos(s: &Store) -> Vec<CalInfo> {
    s.calendars()
        .iter()
        .enumerate()
        .map(|(k, c)| CalInfo {
            name: c.name.clone(),
            colour: c.colour,
            count: s.count(k) as u32,
        })
        .collect()
}

fn query(s: &Store, from: i64, to: i64) -> Vec<Instance> {
    s.query(from, to, 0, &ZONE, &birthdays())
}

#[test]
fn boards() {
    let shot = common::app();
    let cal = shot.ui.global::<Cal>();
    let s = store();
    let cals = infos(&s);
    let now = now();
    let mut prefs = Prefs::default();
    let today = now.today();
    let pin = "14:30 Medicine · Clock";

    // Month: the grid with dots and the meetup's bar, today's agenda
    let start = view::grid_start(2026, 11, 0);
    let month = query(&s, start, start + 41);
    view::month(&cal, &now, &prefs, 2026, 11, today, &month, false, pin);
    view::todos(&cal, &now, &s.todos(&ZONE), false);
    cal.set_view(0);
    let px = shot.shoot("cal2-month");
    assert!(px.iter().any(|&p| p != 0), "something was drawn");
    assert_eq!(cal.get_agenda().row_count(), 3);
    assert_eq!(cal.get_agenda_more(), "+2 more");
    shot.resize(1917);
    shot.shoot("cal2-b-month");
    shot.resize(common::H);

    // Go to month
    view::month_sheet(&cal, 2026, 11);
    shot.shoot("cal2-month-picker");
    cal.set_s_month(false);
    shot.tick();

    // ⋮ on a tab
    shot.press((360.0 - 4.0 - 24.0) * DP, 24.0 * DP, 0);
    shot.shoot("cal2-menu");
    shot.press(20.0 * DP, 600.0 * DP, 0);
    shot.tick();

    // + New on Monday 16
    view::new_sheet(&cal, today);
    shot.shoot("cal2-new");
    cal.set_s_new(false);
    shot.tick();

    // busy: week numbers and bars, Wednesday 18 selected
    prefs.week_numbers = true;
    view::month(&cal, &now, &prefs, 2026, 11, day(11, 18), &month, true, "");
    shot.shoot("cal2-month-busy");
    prefs.week_numbers = false;

    // Week 47
    let week0 = view::week_of(today, 0);
    let week = query(&s, week0, week0 + 6);
    view::week(&cal, &now, week0, today, &week);
    cal.set_view(1);
    shot.shoot("cal2-week");

    // Day: Monday 16 with Clock's 14:30 alarm
    let pins = vec![(at(11, 16, 14, 30), String::from("14:30 Medicine"))];
    view::day(&cal, &now, &prefs, today, &week, &pins);
    cal.set_view(2);
    shot.shoot("cal2-day");
    assert_eq!(cal.get_day_empty(), "");
    shot.resize(1917);
    shot.shoot("cal2-b-day");
    shot.resize(common::H);
    view::day(&cal, &now, &prefs, day(11, 22), &week, &pins);
    shot.shoot("cal2-day-empty");
    assert!(cal.get_day_empty().contains("Sunday"));

    // To-do, then its done list with the toast of a to-do just ticked
    let todos = s.todos(&ZONE);
    view::todos(&cal, &now, &todos, false);
    cal.set_view(3);
    shot.shoot("cal2-todo");
    assert_eq!(cal.get_todo_badge(), 1);
    shot.resize(1917);
    shot.shoot("cal2-b-todo");
    shot.resize(common::H);
    view::todos(&cal, &now, &todos, true);
    shot.ui.set_toast("“Pay internet bill” done".into());
    shot.ui.set_toast_action("Undo".into());
    shot.shoot("cal2-todo-done");
    shot.ui.set_toast("".into());

    // New to-do
    let flights = todos
        .iter()
        .find(|t| t.title == "Book flights to Berlin")
        .expect("flights")
        .clone();
    let mut t = flights.clone();
    t.reminder = Some(0);
    view::todo_form(&cal, "New to-do", &t, &now, "Personal");
    cal.set_view(12);
    shot.shoot("cal2-todo-edit");

    // the event: Kernel review on Monday 16
    let review = week
        .iter()
        .find(|i| i.title == "Kernel review" && i.start / DAY == today)
        .expect("review")
        .clone();
    let draft = s.get(&review.uid, None, &ZONE).expect("draft");
    view::event(&cal, &now, &review, &draft, "Projects", true);
    cal.set_view(10);
    shot.shoot("cal2-event-view");
    assert!(cal.get_ev_when().contains("in 5 h 36 min"));

    // Share
    cal.set_share_text(view::share_text(&review).into());
    cal.set_s_share(true);
    shot.shoot("cal2-share");
    cal.set_s_share(false);
    shot.tick();

    // the form, its Repeat and Reminder sheets
    let mut occurrence = draft.clone();
    occurrence.start = review.start;
    occurrence.end = review.end;
    view::event_form(&cal, "New event", &occurrence, &cals);
    cal.set_view(11);
    shot.shoot("cal2-event-edit");
    shot.resize(1917);
    shot.shoot("cal2-b-event-edit");
    shot.resize(common::H);
    view::repeat_sheet(&cal, &occurrence.repeat, occurrence.start);
    cal.set_s_repeat(true);
    shot.shoot("cal2-repeat-sheet");
    assert_eq!(cal.get_repeat_current(), 3);
    cal.set_s_repeat(false);
    shot.tick();
    view::reminder_sheet(&cal, occurrence.reminder, false, true);
    cal.set_s_reminder(true);
    shot.shoot("cal2-reminder-sheet");
    assert_eq!(cal.get_reminder_current(), 3);
    cal.set_s_reminder(false);
    shot.tick();

    // Calendars, Projects' colour open
    view::calendars(&cal, &cals, 0, true, Some(3));
    cal.set_view(17);
    shot.shoot("cal2-calendar-color");

    // New anniversary
    let wedding = s
        .query(day(6, 3), day(6, 3), 0, &ZONE, &[])
        .into_iter()
        .next()
        .expect("anniversary");
    let mut w = s.get(&wedding.uid, None, &ZONE).expect("wedding");
    w.reminder = Some(900);
    view::anniversary_form(&cal, "New anniversary", &w, &cals, 2019, 7);
    cal.set_view(13);
    shot.shoot("cal2-anniversary-edit");

    // Ava's birthday
    let ava = week
        .iter()
        .find(|i| i.kind == Kind::Birthday)
        .expect("birthday")
        .clone();
    let gift: Vec<Todo> = todos
        .iter()
        .filter(|t| t.title.contains("Ava"))
        .cloned()
        .collect();
    view::birthday(&cal, &now, &ava, Some(1994), &gift, "9:00 the day before");
    cal.set_view(14);
    shot.shoot("cal2-birthday-view");
    assert_eq!(cal.get_bd_turns(), "Turns 32 today");

    // Search "review" over two months
    let around = query(&s, day(10, 1), day(11, 30));
    let found = view::search(&cal, &now, "review", 0, &around, &todos, &cals);
    cal.set_view(15);
    shot.shoot("cal2-search");
    assert!(found.len() >= 4, "{found:?}");

    // Settings
    view::settings(&cal, &prefs, "Yerevan", 123);
    cal.set_view(16);
    shot.shoot("cal2-settings");

    // Add to Calendar: concert.ics from Files
    let (concert, n) = view::preview(CONCERT, &ZONE).expect("concert");
    let clash = query(&s, concert.start / DAY, concert.start / DAY);
    view::import(
        &cal,
        "Files",
        "E:\\Documents\\concert.ics",
        &concert,
        n,
        &cals[0],
        &clash,
    );
    cal.set_view(18);
    shot.shoot("cal2-ics-import");
    assert_eq!(cal.get_im_h0(), 19);
    assert!(cal.get_im_note().starts_with("No clash"));

    // the reminder ringing at 15:45
    let ringing = Now {
        local: at(11, 16, 15, 45),
        ..now
    };
    view::ring(&cal, &ringing, &review, "Projects", 5);
    cal.set_view(19);
    shot.shoot("dt2-ring-event");
    assert_eq!(cal.get_ring_head(), "REMINDER · IN 15 MIN");
}
