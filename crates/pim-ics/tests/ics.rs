// SPDX-License-Identifier: Apache-2.0
use neoui_proto::clock::tz;
use pim_ics::{Comp, Date, Freq, Ics, Rule, Tz, Zone, parse_when, when_value};

const BERLIN_ZONE: &str = "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\nBEGIN:STANDARD\r\n\
DTSTART:19961027T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nRRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\n\
END:STANDARD\r\nBEGIN:DAYLIGHT\r\nDTSTART:19810329T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\n\
RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\r\nEND:DAYLIGHT\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:a@x\r\nDTSTART;TZID=Europe/Berlin:20260329T090000\r\nX-ODD:1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

fn utc(y: i32, m: u8, d: u8, h: i64) -> i64 {
    Date {
        year: y,
        month: m,
        day: d,
    }
    .days()
        * 86_400
        + h * 3600
}

#[test]
fn vtimezone_with_daylight_saving_is_the_posix_rule() {
    let ics = Ics::parse(BERLIN_ZONE);
    let start = ics.when(1, "DTSTART").expect("dtstart");
    let Tz::Rule(r) = ics.tz(&start) else {
        panic!("a rule");
    };
    let posix = tz::Rule::parse("CET-1CEST,M3.5.0,M10.5.0/3").expect("berlin");
    for t in [
        utc(2026, 3, 29, 0) + 3599,
        utc(2026, 3, 29, 1),
        utc(2026, 10, 25, 0) + 3599,
        utc(2026, 10, 25, 1),
        utc(2027, 7, 1, 12),
    ] {
        assert_eq!(r.offset_at(t), posix.offset_at(t), "{t}");
    }
    // 09:00 on the change day is CEST
    assert_eq!(Tz::Rule(r).utc(start.seconds(), &0), utc(2026, 3, 29, 7));
}

#[test]
fn zones_fall_back_to_names_then_floating() {
    let ics = Ics::parse(
        "BEGIN:VEVENT\r\nDTSTART;TZID=America/New_York:20260115T100000\r\nEND:VEVENT\r\n",
    );
    let w = ics.when(0, "DTSTART").expect("dtstart");
    assert_eq!(ics.tz(&w).utc(w.seconds(), &0), utc(2026, 1, 15, 15));
    let w = parse_when("20260115T100000", Some("Mars/Olympus")).expect("when");
    assert_eq!(ics.tz(&w), Tz::Floating);
    let w = parse_when("20260115T100000Z", None).expect("when");
    assert_eq!(ics.tz(&w), Tz::Utc);
    // a fixed zone: STANDARD only
    let y = Ics::parse(
        "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:Asia/Yerevan\r\nBEGIN:STANDARD\r\nDTSTART:19700101T000000\r\n\
         TZOFFSETFROM:+0400\r\nTZOFFSETTO:+0400\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n",
    );
    let w = parse_when("20261128T190000", Some("Asia/Yerevan")).expect("when");
    assert_eq!(y.tz(&w).utc(w.seconds(), &0), utc(2026, 11, 28, 15));
}

#[test]
fn local_times_in_gaps_move_forward() {
    let berlin = tz::Rule::parse("CET-1CEST,M3.5.0,M10.5.0/3").expect("berlin");
    // 02:30 doesn't exist on 29 March: it is 03:30 CEST = 01:30 UTC
    assert_eq!(
        pim_ics::to_utc(utc(2026, 3, 29, 2) + 1800, &berlin),
        utc(2026, 3, 29, 1) + 1800
    );
    assert_eq!(berlin.offset_at(utc(2026, 7, 1, 0)), 7200);
}

#[test]
fn ambiguous_local_times_are_the_first_instant() {
    let berlin = tz::Rule::parse("CET-1CEST,M3.5.0,M10.5.0/3").expect("berlin");
    // 02:30 on 25 October is 02:30 CEST (00:30 UTC) first, then 02:30 CET
    assert_eq!(
        pim_ics::to_utc(utc(2026, 10, 25, 2) + 1800, &berlin),
        utc(2026, 10, 25, 0) + 1800
    );
    assert_eq!(
        pim_ics::to_utc(utc(2026, 10, 25, 3), &berlin),
        utc(2026, 10, 25, 2)
    );
}

#[test]
fn uids_add_remove_and_round_trip() {
    let mut ics = Ics::parse(BERLIN_ZONE);
    assert_eq!(ics.write(), BERLIN_ZONE);
    assert_eq!(ics.comp(0), Comp::Zone);
    assert_eq!(ics.uid(1).as_deref(), Some("a@x"));
    let uid = pim_ics::new_uid(42, 1_780_000_000);
    let i = ics.add(Comp::Todo, &uid, 1_780_000_000).expect("added");
    assert_eq!(ics.comp(i), Comp::Todo);
    assert_eq!(ics.find_uid(&uid), [i]);
    ics.set_text(i, "SUMMARY", Some("Buy milk; bread, eggs"));
    assert_eq!(
        ics.text(i, "SUMMARY").as_deref(),
        Some("Buy milk; bread, eggs")
    );
    assert!(
        ics.write()
            .contains("SUMMARY:Buy milk\\; bread\\, eggs\r\n")
    );
    ics.remove(i);
    assert_eq!(ics.write(), BERLIN_ZONE);
}

#[test]
fn text_without_a_calendar_gets_one() {
    let mut ics = Ics::parse("");
    assert!(ics.is_empty());
    let i = ics.add(Comp::Event, "u1", 0).expect("added");
    assert_eq!(ics.uid(i).as_deref(), Some("u1"));
    assert!(ics.write().starts_with("BEGIN:VCALENDAR\r\n"));
    // a BOM before the first line
    let b = Ics::parse("\u{feff}BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:b\nEND:VEVENT\nEND:VCALENDAR\n");
    assert_eq!(b.uid(0).as_deref(), Some("b"));
}

#[test]
fn calendar_name_and_colour() {
    let mut ics = Ics::new("Work");
    assert_eq!(ics.name().as_deref(), Some("Work"));
    assert_eq!(ics.colour(), None);
    ics.set_colour(0x30A46C);
    assert_eq!(ics.colour(), Some(0x30A46C));
    assert!(ics.write().contains("X-NEO-COLOR:#30A46C\r\n"));
    ics.set_cal_text("X-WR-CALNAME", Some("Projects"));
    assert_eq!(ics.name().as_deref(), Some("Projects"));
}

#[test]
fn several_calendars_split_and_join() {
    let text = "junk\r\nBEGIN:VCALENDAR\r\nX-WR-CALNAME:A\r\nEND:VCALENDAR\r\nBEGIN:VCALENDAR\r\nX-WR-CALNAME:B\r\nEND:VCALENDAR\r\n";
    let all = Ics::parse_all(text);
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].name().as_deref(), Some("B"));
    let joined: String = all.iter().map(Ics::write).collect();
    assert_eq!(joined, text);
}

#[test]
fn exdates_recurrence_ids_and_reminders() {
    let mut ics = Ics::parse(BERLIN_ZONE);
    let w = parse_when("20260405T090000", Some("Europe/Berlin")).expect("when");
    ics.add_when(1, "EXDATE", &w);
    ics.add_when(1, "EXDATE", &parse_when("20260412", None).expect("date"));
    assert_eq!(ics.whens(1, "EXDATE").len(), 2);
    assert!(
        ics.write().contains(
            "EXDATE;TZID=Europe/Berlin:20260405T090000\r\nEXDATE;VALUE=DATE:20260412\r\n"
        )
    );
    ics.set_when(1, "RECURRENCE-ID", Some(&w));
    assert_eq!(ics.when(1, "RECURRENCE-ID"), Some(w));
    assert_eq!(ics.reminder(1), None);
    ics.set_reminder(1, Some(90));
    assert_eq!(ics.reminder(1), Some(90));
    assert!(ics.write().contains("TRIGGER:-PT90M\r\n"));
    ics.set_reminder(1, Some(1440));
    assert!(ics.write().contains("TRIGGER:-P1D\r\n"));
    ics.set_reminder(1, None);
    assert!(!ics.write().contains("VALARM"));
    assert!(ics.write().contains("X-ODD:1\r\n"));
}

#[test]
fn rules_read_and_write() {
    let start = Date {
        year: 2026,
        month: 11,
        day: 16,
    };
    let r = Rule::parse(
        "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;UNTIL=20261231T235959Z",
        start,
    )
    .expect("weekly");
    assert_eq!((r.freq, r.interval, r.days), (Freq::Weekly, 2, 0b101));
    assert_eq!(
        r.write(),
        "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;UNTIL=20261231T235959Z"
    );
    assert!(Rule::parse("FREQ=YEARLY;BYMONTH=11;BYMONTHDAY=16", start).is_some());
    for outside in [
        "FREQ=MONTHLY;BYDAY=2TU",
        "FREQ=MONTHLY;BYSETPOS=1;BYDAY=TU",
        "FREQ=YEARLY;BYMONTH=3",
        "FREQ=HOURLY",
        "FREQ=DAILY;INTERVAL=0",
        "COUNT=3",
    ] {
        assert_eq!(Rule::parse(outside, start), None, "{outside}");
    }
    let u = parse_when("20261231", None).expect("date");
    assert_eq!(when_value(&u), "20261231");
}

#[test]
fn fixed_zones_are_zones() {
    let z: &dyn Zone = &14_400;
    assert_eq!(z.offset_at(0), 14_400);
}
