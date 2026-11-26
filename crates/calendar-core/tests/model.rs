// SPDX-License-Identifier: Apache-2.0
use calendar_core::wire::{self, Corrupt};
use calendar_core::{Birthday, DAY, EventDraft, Freq, Instance, Kind, Repeat, Store, Todo, Zone};
use neoui_proto::clock::tz;
use pim_ics::Date;

const PERSONAL: &str = include_str!("data/personal.ics");
const BERLIN: &str = include_str!("data/berlin.ics");
const CONCERT: &str = include_str!("data/concert.ics");
const TODO: &str = include_str!("data/todo.ics");

fn berlin() -> tz::Rule {
    tz::Rule::parse("CET-1CEST,M3.5.0,M10.5.0/3").expect("berlin")
}

fn day(y: i32, m: u8, d: u8) -> i64 {
    Date {
        year: y,
        month: m,
        day: d,
    }
    .days()
}

/// Local (or UTC) seconds of a date and time.
fn at(y: i32, m: u8, d: u8, h: i64, min: i64) -> i64 {
    day(y, m, d) * DAY + h * 3600 + min * 60
}

fn store() -> Store {
    Store::load(&[PERSONAL, BERLIN].concat(), TODO)
}

fn of<'a>(v: &'a [Instance], uid: &str) -> Vec<&'a Instance> {
    v.iter().filter(|i| i.uid == uid).collect()
}

fn hours(v: &[&Instance]) -> Vec<i64> {
    v.iter().map(|i| i.start.rem_euclid(DAY) / 3600).collect()
}

fn days(v: &[&Instance]) -> Vec<i64> {
    v.iter().map(|i| i.start.div_euclid(DAY)).collect()
}

// --- zones and daylight saving

#[test]
fn weekly_event_keeps_09_00_across_spring_change() {
    let s = store();
    let q = s.query(day(2026, 3, 1), day(2026, 4, 30), 0, &berlin(), &[]);
    let v = of(&q, "spring-standup@example.com");
    assert_eq!(
        days(&v),
        [
            day(2026, 3, 16),
            day(2026, 3, 23),
            day(2026, 3, 30),
            day(2026, 4, 6)
        ]
    );
    assert_eq!(hours(&v), [9, 9, 9, 9]);
    assert_eq!(v[0].end - v[0].start, 15 * 60);
    // in UTC it moves an hour on 29 March
    let q = s.query(day(2026, 3, 1), day(2026, 4, 30), 0, &0, &[]);
    assert_eq!(hours(&of(&q, "spring-standup@example.com")), [8, 8, 7, 7]);
}

#[test]
fn weekly_event_keeps_09_00_across_autumn_change_until_utc() {
    let s = store();
    let q = s.query(day(2026, 10, 1), day(2026, 11, 30), 0, &berlin(), &[]);
    let v = of(&q, "autumn-standup@example.com");
    // UNTIL 08:00Z on 2 November = 09:00 CET: that Monday is the last
    assert_eq!(
        days(&v),
        [
            day(2026, 10, 12),
            day(2026, 10, 19),
            day(2026, 10, 26),
            day(2026, 11, 2)
        ]
    );
    assert_eq!(hours(&v), [9, 9, 9, 9]);
}

#[test]
fn new_york_event_shown_in_berlin() {
    let s = store();
    let q = s.query(day(2026, 3, 1), day(2026, 4, 5), 0, &berlin(), &[]);
    let v = of(&q, "ny-call@example.com");
    assert_eq!(v.len(), 5);
    // 10:00 New York: 5 h behind, 4 h between the US change (8 March) and Europe's (29 March)
    assert_eq!(hours(&v), [16, 15, 15, 15, 16]);
    // without a VTIMEZONE the zone name finds the city
    let bare = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:ny\r\nDTSTART;TZID=America/New_York:20260302T100000\r\n\
                RRULE:FREQ=WEEKLY;COUNT=5\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let s = Store::load(bare, "");
    let q = s.query(day(2026, 3, 1), day(2026, 4, 5), 0, &berlin(), &[]);
    assert_eq!(hours(&of(&q, "ny")), [16, 15, 15, 15, 16]);
}

#[test]
fn floating_events_follow_the_phone() {
    let mut s = Store::load("", "");
    let uid = s
        .put(
            &EventDraft {
                title: "Swim".into(),
                start: at(2026, 3, 23, 9, 0),
                end: at(2026, 3, 23, 10, 0),
                repeat: Repeat::Every {
                    freq: Freq::Weekly,
                    interval: 1,
                    days: 0,
                    count: 3,
                    until: None,
                },
                reminder: Some(10),
                ..EventDraft::default()
            },
            &berlin(),
            at(2026, 3, 1, 0, 0),
        )
        .expect("put");
    let q = s.query(day(2026, 3, 1), day(2026, 4, 30), 0, &berlin(), &[]);
    assert_eq!(hours(&of(&q, &uid)), [9, 9, 9]);
    let r = s.reminders(at(2026, 3, 1, 0, 0), at(2026, 5, 1, 0, 0), &berlin());
    let times: Vec<i64> = r.iter().map(|(t, _)| *t).collect();
    // 08:50 CET = 07:50Z, then 08:50 CEST = 06:50Z
    assert_eq!(
        times,
        [
            at(2026, 3, 23, 7, 50),
            at(2026, 3, 30, 6, 50),
            at(2026, 4, 6, 6, 50)
        ]
    );
}

// --- recurrence

#[test]
fn count_interval_and_exdate() {
    let q = store().query(day(2026, 11, 1), day(2026, 11, 30), 0, &berlin(), &[]);
    let v = of(&q, "gym@example.com");
    assert_eq!(
        days(&v),
        [
            day(2026, 11, 2),
            day(2026, 11, 4),
            day(2026, 11, 8),
            day(2026, 11, 10)
        ]
    );
}

#[test]
fn february_29_yearly_only_in_leap_years() {
    let q = store().query(day(2024, 1, 1), day(2032, 12, 31), 0, &berlin(), &[]);
    let v = of(&q, "leap@example.com");
    assert_eq!(
        days(&v),
        [day(2024, 2, 29), day(2028, 2, 29), day(2032, 2, 29)]
    );
    assert!(v.iter().all(|i| i.all_day && i.end - i.start == DAY));
}

#[test]
fn monthly_on_the_31st_skips_short_months() {
    let q = store().query(day(2026, 1, 1), day(2027, 12, 31), 0, &berlin(), &[]);
    let months: Vec<u8> = of(&q, "rent@example.com")
        .iter()
        .map(|i| Date::from_days(i.start.div_euclid(DAY)).month)
        .collect();
    assert_eq!(months, [1, 3, 5, 7, 8, 10, 12]);
}

#[test]
fn anniversaries_count_years() {
    let q = store().query(day(2026, 6, 1), day(2026, 6, 30), 0, &berlin(), &[]);
    let v = of(&q, "wedding@example.com");
    assert_eq!(days(&v), [day(2026, 6, 3)]);
    assert_eq!(
        (v[0].kind, v[0].years, v[0].all_day),
        (Kind::Anniversary, 7, true)
    );
}

#[test]
fn rules_outside_the_subset_give_their_first_instance() {
    let mut s = store();
    let q = s.query(day(2026, 11, 1), day(2027, 3, 31), 0, &berlin(), &[]);
    let v = of(&q, "setpos@example.com");
    assert_eq!(days(&v), [day(2026, 11, 3)]);
    assert!(v[0].repeats);
    let mut d = s.get("setpos@example.com", None, &berlin()).expect("get");
    assert_eq!(d.repeat, Repeat::Other);
    d.title = "Lunch".into();
    s.put(&d, &berlin(), 0).expect("put");
    assert!(
        s.calendars_text()
            .contains("RRULE:FREQ=MONTHLY;BYDAY=TU;BYSETPOS=1\n")
    );
}

#[test]
fn changed_occurrences_replace_theirs() {
    let q = store().query(day(2026, 11, 16), day(2026, 11, 30), 0, &berlin(), &[]);
    let v = of(&q, "review@example.com");
    assert_eq!(
        days(&v),
        [day(2026, 11, 16), day(2026, 11, 23), day(2026, 11, 30)]
    );
    assert_eq!(hours(&v), [16, 17, 16]);
    assert_eq!(v[1].title, "Kernel review (moved)");
    assert_eq!(v[1].place, "Room 5");
    assert_eq!(v[1].recur, at(2026, 11, 23, 16, 0));
    assert!(v[0].reminder && v[0].repeats);
}

#[test]
fn multi_day_events_and_order() {
    let b = [Birthday {
        contact_id: 7,
        name: "Ava Lindqvist".into(),
        month: 11,
        day: 20,
        year: Some(1994),
    }];
    let q = store().query(day(2026, 11, 20), day(2026, 11, 20), 0, &berlin(), &b);
    assert_eq!(q.len(), 2);
    // the trip began on the 19th: it sorts before the day's own all-day items
    assert!(q[0].title.starts_with("Trip to Berlin with a very long title that goes past seventy-five octets so that it is folded"));
    assert_eq!(
        (q[0].start, q[0].end - q[0].start),
        (day(2026, 11, 19) * DAY, 3 * DAY)
    );
    assert_eq!(q[1].kind, Kind::Birthday);
    assert_eq!((q[1].contact, q[1].years, q[1].uid.as_str()), (7, 32, ""));
}

#[test]
fn birthdays_on_29_february() {
    let b = [Birthday {
        contact_id: 1,
        name: "Leap".into(),
        month: 2,
        day: 29,
        year: None,
    }];
    let s = Store::load("", "");
    let q = s.query(day(2027, 1, 1), day(2028, 12, 31), 0, &berlin(), &b);
    assert_eq!(
        days(&q.iter().collect::<Vec<_>>()),
        [day(2027, 2, 28), day(2028, 2, 29)]
    );
    assert_eq!(q[0].years, 0);
}

#[test]
fn hidden_calendars() {
    let s = store();
    assert_eq!(s.calendars().len(), 2);
    assert_eq!(s.calendars()[1].name, "Work");
    assert_eq!(s.calendars()[0].colour, 0x3D7EFF);
    assert_eq!(s.count(1), 3);
    let q = s.query(day(2026, 3, 1), day(2026, 3, 31), 0b10, &berlin(), &[]);
    assert!(q.iter().all(|i| i.cal == 0));
    assert!(!q.is_empty());
}

// --- the store

#[test]
fn round_trip_is_byte_exact() {
    let s = store();
    assert_eq!(s.calendars_text(), [PERSONAL, BERLIN].concat());
    assert_eq!(s.todos_text(), TODO);
}

#[test]
fn edits_keep_unknown_lines() {
    let mut s = store();
    let mut d = s.get("gym@example.com", None, &berlin()).expect("gym");
    d.title = "Gym, legs".into();
    d.reminder = Some(30);
    s.put(&d, &berlin(), 0).expect("put");
    let text = s.calendars_text();
    for kept in [
        "X-UNKNOWN-PROP:still here\n",
        "X-SOMETHING-ELSE;FOO=bar:kept as it is\n",
        "BEGIN:VJOURNAL\nUID:journal@example.com\n",
        "EXDATE:20261106T180000\n",
        "RRULE:FREQ=DAILY;INTERVAL=2;COUNT=5\n",
        "DTSTART:20261102T180000\n",
        BERLIN,
    ] {
        assert!(text.contains(kept), "{kept}");
    }
    assert!(text.contains("SUMMARY:Gym\\, legs\n"));
    assert_eq!(s.get("gym@example.com", None, &berlin()).expect("gym"), d);
    // and through a reload
    let s2 = Store::load(&text, &s.todos_text());
    assert_eq!(s2.calendars_text(), text);
}

#[test]
fn new_events_round_trip_through_the_text() {
    let mut s = Store::load("", "");
    s.seed = 99;
    let d = EventDraft {
        title: "Kernel review".into(),
        place: "Room 4".into(),
        notes: "Bring the Pixel.".into(),
        start: at(2026, 11, 16, 16, 0),
        end: at(2026, 11, 16, 17, 0),
        repeat: Repeat::Every {
            freq: Freq::Weekly,
            interval: 2,
            days: 0b101,
            count: 0,
            until: Some(day(2026, 12, 31)),
        },
        reminder: Some(15),
        ..EventDraft::default()
    };
    let uid = s.put(&d, &berlin(), at(2026, 11, 1, 0, 0)).expect("put");
    assert!(uid.ends_with("@neosymbian"));
    let got = s.get(&uid, None, &berlin()).expect("get");
    assert_eq!(
        got,
        EventDraft {
            uid: uid.clone(),
            ..d
        }
    );
    let s = Store::load(&s.calendars_text(), "");
    let q = s.query(day(2026, 11, 1), day(2027, 1, 31), 0, &berlin(), &[]);
    // Mondays and Wednesdays every other week, to the end of the year
    assert_eq!(
        days(&of(&q, &uid)),
        [
            day(2026, 11, 16),
            day(2026, 11, 18),
            day(2026, 11, 30),
            day(2026, 12, 2),
            day(2026, 12, 14),
            day(2026, 12, 16),
            day(2026, 12, 28),
            day(2026, 12, 30)
        ]
    );
}

#[test]
fn delete_one_occurrence_or_all() {
    let mut s = store();
    let z = berlin();
    assert!(s.del("review@example.com", Some(at(2026, 11, 30, 16, 0)), &z));
    // the changed one goes too
    assert!(s.del("review@example.com", Some(at(2026, 11, 23, 16, 0)), &z));
    let q = s.query(day(2026, 11, 16), day(2026, 12, 7), 0, &z, &[]);
    assert_eq!(
        days(&of(&q, "review@example.com")),
        [day(2026, 11, 16), day(2026, 12, 7)]
    );
    assert!(s.calendars_text().contains("EXDATE:20261130T160000\n"));
    assert!(!s.calendars_text().contains("Kernel review (moved)"));
    assert!(s.del("review@example.com", None, &z));
    assert!(
        s.query(day(2026, 11, 1), day(2027, 12, 31), 0, &z, &[])
            .iter()
            .all(|i| i.uid != "review@example.com")
    );
    assert!(!s.del("review@example.com", None, &z));
}

#[test]
fn edit_one_occurrence() {
    let mut s = store();
    let z = berlin();
    let mut d = s
        .get("review@example.com", Some(at(2026, 11, 30, 16, 0)), &z)
        .expect("get");
    assert_eq!(
        (d.start, d.repeat.clone()),
        (at(2026, 11, 30, 16, 0), Repeat::Never)
    );
    d.title = "Kernel review: SMP".into();
    d.start += 1800;
    d.end += 1800;
    s.put(&d, &z, 0).expect("put");
    let q = s.query(day(2026, 11, 16), day(2026, 12, 7), 0, &z, &[]);
    let v = of(&q, "review@example.com");
    assert_eq!(v.len(), 4);
    assert_eq!(v[2].title, "Kernel review: SMP");
    assert_eq!(v[2].start, at(2026, 11, 30, 16, 30));
    assert_eq!(v[3].title, "Kernel review");
    // the changed copy again: edits it in place
    let mut d = s
        .get("review@example.com", Some(at(2026, 11, 30, 16, 0)), &z)
        .expect("get");
    assert_eq!(d.title, "Kernel review: SMP");
    d.place = "Room 6".into();
    s.put(&d, &z, 0).expect("put");
    assert_eq!(
        s.calendars_text().matches("UID:review@example.com").count(),
        3
    );
}

#[test]
fn move_to_another_calendar() {
    let mut s = store();
    let z = berlin();
    let mut d = s.get("review@example.com", None, &z).expect("get");
    d.cal = 1;
    s.put(&d, &z, 0).expect("put");
    let q = s.query(day(2026, 11, 16), day(2026, 11, 30), 0, &z, &[]);
    assert!(of(&q, "review@example.com").iter().all(|i| i.cal == 1));
    assert_eq!(of(&q, "review@example.com").len(), 3);
}

#[test]
fn anniversaries_are_yearly_and_all_day() {
    let mut s = Store::load("", "");
    let uid = s
        .put(
            &EventDraft {
                kind: Kind::Anniversary,
                title: "Met Ani".into(),
                start: at(2019, 6, 3, 14, 0),
                end: at(2019, 6, 3, 15, 0),
                reminder: Some(15 * 60),
                ..EventDraft::default()
            },
            &berlin(),
            0,
        )
        .expect("put");
    let text = s.calendars_text();
    assert!(text.contains("X-NEO-KIND:anniversary\r\n"));
    assert!(text.contains("DTSTART;VALUE=DATE:20190603\r\n"));
    assert!(text.contains("RRULE:FREQ=YEARLY\r\n"));
    let q = s.query(day(2026, 6, 3), day(2026, 6, 3), 0, &berlin(), &[]);
    assert_eq!((q[0].uid.as_str(), q[0].years), (uid.as_str(), 7));
    // 1 day before at 09:00: 15 hours before midnight
    let r = s.reminders(at(2026, 6, 1, 0, 0), at(2026, 6, 4, 0, 0), &berlin());
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].0, at(2026, 6, 2, 7, 0));
}

#[test]
fn next_event() {
    let s = store();
    let z = berlin();
    let n = s.next_event(at(2026, 11, 16, 14, 0), &z).expect("next");
    assert_eq!(
        (n.uid.as_str(), n.start),
        ("review@example.com", at(2026, 11, 16, 16, 0))
    );
    let n = s.next_event(at(2026, 11, 16, 15, 1), &z).expect("next");
    assert_eq!(n.uid, "trip@example.com");
    assert!(Store::load("", "").next_event(0, &z).is_none());
}

#[test]
fn reminders_have_stable_small_ids() {
    let mut s = store();
    let z = berlin();
    let (from, to) = (at(2026, 11, 16, 0, 0), at(2026, 11, 17, 0, 0));
    let r = s.reminders(from, to, &z);
    // the bill (18:00 local, 9 h before = 08:00Z) and the review (16:00 local, 15 min before = 14:45Z)
    assert_eq!(
        r.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
        [at(2026, 11, 16, 8, 0), at(2026, 11, 16, 14, 45)]
    );
    assert_eq!(
        s.reminder(r[1].1),
        Some(("review@example.com", at(2026, 11, 16, 15, 0)))
    );
    assert_eq!(s.reminder(r[0].1).map(|(u, _)| u), Some("bill@example.com"));
    assert!(r.iter().all(|(_, id)| id.0 > 0 && id.0 < 1 << 24));
    assert_eq!(s.reminders(from, to, &z), r);
    let later = s.reminders(at(2026, 11, 23, 0, 0), at(2026, 11, 24, 0, 0), &z);
    // the moved review has no VALARM of its own
    assert!(later.is_empty());
}

#[test]
fn search_events_and_todos() {
    let s = store();
    let (ev, todos) = s.search("REVIEW", day(2026, 11, 1), day(2026, 11, 30), &berlin());
    assert_eq!(
        days(&ev.iter().collect::<Vec<_>>()),
        [day(2026, 11, 16), day(2026, 11, 23), day(2026, 11, 30)]
    );
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].title, "Write a review of the N8 port");
    let (ev, _) = s.search("aarch64", day(2026, 11, 1), day(2026, 11, 20), &berlin());
    assert_eq!(ev.len(), 1);
}

#[test]
fn import_concert_and_clashes() {
    let mut s = store();
    let z = berlin();
    assert_eq!(s.import(CONCERT, 0, 0), (1, vec![]));
    let q = s.query(day(2026, 11, 28), day(2026, 11, 28), 0, &z, &[]);
    let c = of(&q, "20261128-komitas@khachaturian.am");
    // 19:00 Yerevan (UTC+4) = 16:00 in Berlin
    assert_eq!(hours(&c), [16]);
    assert_eq!(c[0].title, "Concert · Komitas Quartet");
    assert!(c[0].reminder);
    assert_eq!(
        s.import(CONCERT, 0, 0),
        (0, vec![String::from("20261128-komitas@khachaturian.am")])
    );
    let out = s.export(Some("20261128-komitas@khachaturian.am"));
    assert!(out.contains("TZID:Asia/Yerevan"));
    assert!(out.contains("GEO:40.1872;44.5152"));
    assert!(out.contains("ATTENDEE;CN=\"Ani P\";ROLE=REQ-PARTICIPANT:mailto:ani@example.am"));
    assert_eq!(out.matches("BEGIN:VEVENT").count(), 1);
    let mut fresh = Store::load("", "");
    assert_eq!(fresh.import(&out, 0, 0).0, 1);
    let q2 = fresh.query(day(2026, 11, 28), day(2026, 11, 28), 0, &z, &[]);
    assert_eq!(q2.len(), 1);
    assert_eq!((q2[0].start, &q2[0].title), (c[0].start, &c[0].title));
}

#[test]
fn import_gives_missing_uids_and_takes_todos() {
    let mut s = Store::load("", "");
    let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART:20261201T100000\r\nSUMMARY:No uid\r\nEND:VEVENT\r\n\
                BEGIN:VTODO\r\nUID:t1\r\nSUMMARY:Imported task\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
    assert_eq!(s.import(text, 0, 5), (2, vec![]));
    let q = s.query(day(2026, 12, 1), day(2026, 12, 1), 0, &berlin(), &[]);
    assert!(!q[0].uid.is_empty());
    assert_eq!(s.todos(&berlin())[0].title, "Imported task");
}

#[test]
fn export_everything_as_one_calendar() {
    let s = store();
    let out = s.export(None);
    assert_eq!(out.matches("BEGIN:VCALENDAR").count(), 1);
    assert_eq!(out.matches("BEGIN:VEVENT").count(), 11);
    assert_eq!(out.matches("BEGIN:VTODO").count(), 3);
    assert_eq!(out.matches("TZID:Europe/Berlin\r\n").count(), 1);
    let again = Store::load(&out, "");
    assert_eq!(
        again
            .query(day(2026, 1, 1), day(2026, 12, 31), 0, &berlin(), &[])
            .len(),
        s.query(day(2026, 1, 1), day(2026, 12, 31), 0, &berlin(), &[])
            .len()
    );
}

#[test]
fn todos_list_put_done_del() {
    let mut s = store();
    let z = berlin();
    let t = s.todos(&z);
    let titles: Vec<&str> = t.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Renew passport",
            "Pay internet bill",
            "Write a review of the N8 port"
        ]
    );
    assert_eq!(
        (t[0].priority, t[0].due_all_day, t[0].due),
        (1, true, Some(day(2026, 11, 12) * DAY))
    );
    assert_eq!(t[1].due, Some(at(2026, 11, 16, 18, 0)));
    assert_eq!(t[1].reminder, Some(540));
    assert!(t[2].done);
    assert_eq!(t[2].completed, Some(at(2026, 10, 9, 12, 0)));
    assert_eq!(s.overdue(day(2026, 11, 16), &z), 1);
    assert_eq!(s.overdue(day(2026, 11, 17), &z), 2);
    let uid = s
        .put_todo(
            &Todo {
                title: "Buy a gift for Ava".into(),
                due: Some(day(2026, 11, 16) * DAY),
                due_all_day: true,
                priority: 1,
                ..Todo::default()
            },
            &z,
            at(2026, 11, 10, 0, 0),
        )
        .expect("put");
    let mut gift = s
        .todos(&z)
        .into_iter()
        .find(|t| t.uid == uid)
        .expect("gift");
    assert_eq!(
        (gift.priority, gift.due),
        (1, Some(day(2026, 11, 16) * DAY))
    );
    gift.done = true;
    s.put_todo(&gift, &z, at(2026, 11, 16, 9, 0)).expect("done");
    let done = s
        .todos(&z)
        .into_iter()
        .find(|t| t.uid == uid)
        .expect("gift");
    assert!(done.done);
    assert_eq!(done.completed, Some(at(2026, 11, 16, 9, 0)));
    let mut undone = done.clone();
    undone.done = false;
    s.put_todo(&undone, &z, 0).expect("undone");
    assert!(
        !s.todos(&z)
            .into_iter()
            .find(|t| t.uid == uid)
            .expect("gift")
            .done
    );
    assert!(s.del_todo(&uid));
    assert!(!s.del_todo(&uid));
    assert!(s.todos_text().contains("X-KEEP-ME:yes\n"));
    assert!(
        s.put_todo(
            &Todo {
                uid: "nope".into(),
                ..Todo::default()
            },
            &z,
            0
        )
        .is_none()
    );
}

// --- wire

#[test]
fn wire_round_trips() {
    let s = store();
    let mut q = s.query(day(2026, 11, 1), day(2026, 11, 30), 0, &berlin(), &[]);
    q[0].title = "Ա".repeat(300); // past the 255-byte strings of clock::wire
    let b = wire::encode_instances(&q);
    assert_eq!(wire::decode_instances(&b), Ok(q));
    let d = s.get("review@example.com", None, &berlin()).expect("get");
    let d = EventDraft {
        only: Some(5),
        kind: Kind::Anniversary,
        ..d
    };
    assert_eq!(EventDraft::decode(&d.encode()), Ok(d.clone()));
    let t = s.todos(&berlin());
    assert_eq!(wire::decode_todos(&wire::encode_todos(&t)), Ok(t.clone()));
    assert_eq!(Todo::decode(&t[2].encode()), Ok(t[2].clone()));
    let e = d.encode();
    for n in 0..e.len() {
        assert_eq!(EventDraft::decode(&e[..n]), Err(Corrupt));
    }
    assert_eq!(wire::decode_instances(&[2, 0, 0]), Err(Corrupt));
}

#[test]
fn fixed_offsets_are_zones() {
    let s = store();
    let yerevan: &dyn Zone = &(4 * 3600);
    let q = s.query(day(2026, 3, 16), day(2026, 3, 16), 0, yerevan, &[]);
    // 09:00 CET = 12:00 in Yerevan
    assert_eq!(hours(&of(&q, "spring-standup@example.com")), [12]);
}

// --- review fixes

#[test]
fn editing_one_anniversary_keeps_the_copy_single() {
    let mut s = Store::load("", "");
    let z = berlin();
    let uid = s
        .put(
            &EventDraft {
                kind: Kind::Anniversary,
                title: "Met Ani".into(),
                start: day(2019, 6, 3) * DAY,
                end: day(2019, 6, 4) * DAY,
                ..EventDraft::default()
            },
            &z,
            0,
        )
        .expect("put");
    let mut d = s.get(&uid, Some(day(2026, 6, 3) * DAY), &z).expect("get");
    d.title = "Met Ani: 7 years".into();
    s.put(&d, &z, 0).expect("put one");
    let q = s.query(day(2026, 6, 1), day(2027, 6, 30), 0, &z, &[]);
    let titles: Vec<&str> = of(&q, &uid).iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["Met Ani: 7 years", "Met Ani"]);
}
