// SPDX-License-Identifier: Apache-2.0
use calendar_core::{DAY, EventDraft, Store, Todo};
use neoui_proto::clock::tz;
use std::fmt::Write as _;
use std::time::Instant;

const FIXTURES: [&str; 4] = [
    include_str!("data/personal.ics"),
    include_str!("data/berlin.ics"),
    include_str!("data/concert.ics"),
    include_str!("data/todo.ics"),
];

fn berlin() -> tz::Rule {
    tz::Rule::parse("CET-1CEST,M3.5.0,M10.5.0/3").expect("berlin")
}

/// Everything DTSRV does with a store, on damaged text.
fn exercise(text: &str) {
    let z = berlin();
    let mut s = Store::load(text, text);
    let d0 = 20_758; // 2026-11-01, where the fixtures' events are
    let q = s.query(d0, d0 + 45, 0, &z, &[]);
    s.next_event(d0 * DAY, &z);
    s.reminders(d0 * DAY, (d0 + 60) * DAY, &z);
    s.search("e", d0, d0 + 30, &z);
    s.todos(&z);
    s.export(None);
    if let Some(i) = q.first() {
        if let Some(mut d) = s.get(&i.uid, None, &z) {
            d.title.push('!');
            d.start += 3600;
            s.put(&d, &z, 0);
        }
        s.del(&i.uid, Some(i.recur), &z);
    }
    if let Some(t) = s.todos(&z).first() {
        let t = Todo {
            done: !t.done,
            ..t.clone()
        };
        s.put_todo(&t, &z, 0);
    }
    s.import(text, 0, 0);
    let again = Store::load(&s.calendars_text(), &s.todos_text());
    assert_eq!(again.calendars_text(), s.calendars_text());
}

#[test]
fn never_panics_on_truncated_or_mutated_text() {
    const NOISE: &[u8] = b"\r\n :;=,-+TZ019";
    for text in FIXTURES {
        let b = text.as_bytes();
        // each position gets one damage in turn (all three at every byte take 15 s in debug)
        for (i, &c) in b.iter().enumerate() {
            let mut m = b.to_vec();
            match i % 3 {
                0 => m.truncate(i),
                1 if c.is_ascii() => m[i] = NOISE[i / 3 % NOISE.len()],
                _ => {
                    m.remove(i);
                }
            }
            if let Ok(t) = std::str::from_utf8(&m) {
                exercise(t);
            }
        }
    }
    for odd in [
        "BEGIN:VEVENT\r\nDTSTART:00010101T000000\r\nRRULE:FREQ=DAILY;COUNT=4000000000\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nDTSTART:99991231\r\nRRULE:FREQ=YEARLY;INTERVAL=4294967295\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nDTSTART:20260101T000000\r\nDTEND:19000101T000000\r\nRRULE:FREQ=MONTHLY\r\nEND:VEVENT\r\n",
        "END:VCALENDAR\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nBEGIN:VALARM\r\nTRIGGER:-P99999999999999D\r\n",
        "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:x\r\nBEGIN:DAYLIGHT\r\nRRULE:BYDAY=-1;BYMONTH=13\r\nTZOFFSETTO:+9999\r\nEND:DAYLIGHT\r\nEND:VTIMEZONE\r\n",
    ] {
        exercise(odd);
    }
}

#[test]
fn a_thousand_events_query_fast() {
    let mut text = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
    text.push_str(
        &FIXTURES[1][FIXTURES[1].find("BEGIN:VTIMEZONE").expect("zones")
            ..FIXTURES[1].find("BEGIN:VEVENT").expect("events")],
    );
    for n in 0..1000 {
        let (m, d, h) = (1 + n % 12, 1 + n % 28, 8 + n % 10);
        let _ = write!(text, "BEGIN:VEVENT\r\nUID:e{n}@x\r\nSUMMARY:Event {n}\r\n");
        let _ = match n % 5 {
            0 => write!(
                text,
                "DTSTART;TZID=Europe/Berlin:2025{m:02}{d:02}T{h:02}0000\r\nDURATION:PT1H\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,TH\r\n"
            ),
            1 => write!(
                text,
                "DTSTART:2026{m:02}{d:02}T{h:02}3000\r\nDTEND:2026{m:02}{d:02}T{h:02}4500\r\n"
            ),
            2 => write!(
                text,
                "DTSTART;VALUE=DATE:2020{m:02}{d:02}\r\nRRULE:FREQ=YEARLY\r\n"
            ),
            3 => write!(
                text,
                "DTSTART:2024{m:02}{d:02}T{h:02}0000Z\r\nRRULE:FREQ=DAILY;INTERVAL=3\r\nEXDATE:2026{m:02}{d:02}T{h:02}0000Z\r\n"
            ),
            _ => write!(
                text,
                "DTSTART:2026{m:02}{d:02}T{h:02}0000\r\nRRULE:FREQ=MONTHLY;COUNT=24\r\nBEGIN:VALARM\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n"
            ),
        };
        text.push_str("END:VEVENT\r\n");
    }
    text.push_str("END:VCALENDAR\r\n");
    let t = Instant::now();
    let mut s = Store::load(&text, "");
    let loaded = t.elapsed();
    let z = berlin();
    let nov = 20_758; // 2026-11-01
    let t = Instant::now();
    let month = s.query(nov, nov + 41, 0, &z, &[]);
    let week = s.query(nov + 14, nov + 20, 0, &z, &[]);
    let took = t.elapsed();
    assert!(month.len() > 1000, "{}", month.len());
    assert!(!week.is_empty());
    assert!(
        took.as_millis() < 50,
        "a 6-week and a week query took {took:?}"
    );
    let t = Instant::now();
    s.next_event(nov * DAY, &z);
    s.reminders(nov * DAY, (nov + 1) * DAY, &z);
    assert!(
        t.elapsed().as_millis() < 50,
        "next + reminders took {:?}",
        t.elapsed()
    );
    // an edit re-reads the store: keep it interactive too
    let t = Instant::now();
    let d = s.get("e1@x", None, &z).expect("get");
    s.put(
        &EventDraft {
            title: "Renamed".into(),
            ..d
        },
        &z,
        0,
    );
    assert!(
        t.elapsed().as_millis() < 200,
        "an edit took {:?} (load {loaded:?})",
        t.elapsed()
    );
}
