// SPDX-License-Identifier: Apache-2.0
//! DTSRV's planner: what rings now and what rings next, from the alarms (local minutes of the week), the timer
//! (wall-clock ms) and other dues (reminders, later), given UTC now and the local offset. Pure, so the zone changes,
//! the week wrap and clock jumps are tested here.
//!
//! Something is due when its time lies in `(since, now]`, `since` being the last plan's `now`: a jump forward rings
//! what it skipped once, a jump back rings nothing twice, and a zone change moves the alarms with local time.

use crate::alarm::{Alarm, Alarms, Minute};
use crate::ipc::ring;
use crate::timer::{Phase, Timer};
use alloc::vec::Vec;

/// What the planner found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// The earliest ring after now: (Unix UTC seconds, ring id).
    pub next: Option<(i64, u32)>,
    /// The rings due in `(since, now]`, alarms first, in time order of the list.
    pub due: Vec<u32>,
}

/// Plans at `now` (Unix UTC seconds) with the local offset `offset` (seconds east of UTC); `since` is the previous
/// plan's `now` (equal to `now` on the first). `extra` are other dues: `(utc, ring id)`.
#[must_use]
pub fn plan(
    alarms: &Alarms,
    timer: &Timer,
    since: i64,
    now: i64,
    offset: i32,
    extra: &[(i64, u32)],
) -> Plan {
    let (since_m, now_m) = (local_minute(since, offset), local_minute(now, offset));
    let mut p = Plan::default();
    for a in alarms.as_slice() {
        let id = ring::id(ring::ALARM, u32::from(a.id));
        if let Some(at) = after(a, since_m)
            && at <= now_m
        {
            p.due.push(id);
        }
        if let Some(at) = after(a, now_m) {
            earliest(&mut p.next, at * 60 - i64::from(offset), id);
        }
    }
    if let Phase::Running { ends_ms } = timer.phase {
        let id = ring::id(ring::TIMER, 0);
        if ends_ms <= now * 1000 {
            p.due.push(id);
        } else {
            earliest(&mut p.next, (ends_ms + 999).div_euclid(1000), id);
        }
    }
    for &(at, id) in extra {
        if since < at && at <= now {
            p.due.push(id);
        } else if at > now {
            earliest(&mut p.next, at, id);
        }
    }
    p
}

/// The next alarm after `now`: (Unix UTC seconds, the alarm).
#[must_use]
pub fn next_alarm(alarms: &Alarms, now: i64, offset: i32) -> Option<(i64, &Alarm)> {
    let now_m = local_minute(now, offset);
    alarms
        .as_slice()
        .iter()
        .filter_map(|a| after(a, now_m).map(|at| (at * 60 - i64::from(offset), a)))
        .min_by_key(|&(at, _)| at)
}

/// The local minute of the week at `utc`, for `Alarms::snooze_for` and the like.
#[must_use]
pub fn minute_of_week(utc: i64, offset: i32) -> Minute {
    week_minute(local_minute(utc, offset))
}

/// Local minutes since 1970-01-01 00:00.
fn local_minute(utc: i64, offset: i32) -> i64 {
    (utc + i64::from(offset)).div_euclid(60)
}

fn week_minute(m: i64) -> Minute {
    let day = m.div_euclid(1440);
    let of_day = m.rem_euclid(1440);
    // all three fit: 1970-01-01 was a Thursday (weekday 3, Monday = 0)
    Minute {
        weekday: (day + 3).rem_euclid(7) as u8,
        hour: (of_day / 60) as u8,
        minute: (of_day % 60) as u8,
    }
}

/// The local minute alarm `a` next rings after minute `m`; `None` when it is off.
fn after(a: &Alarm, m: i64) -> Option<i64> {
    a.minutes_until(week_minute(m)).map(|d| m + i64::from(d))
}

fn earliest(next: &mut Option<(i64, u32)>, at: i64, id: u32) {
    if next.is_none_or(|(t, _)| at < t) {
        *next = Some((at, id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alarm::Weekdays;

    /// Monday 2026-11-16 06:00 UTC.
    const MON_0600: i64 = 1_794_808_800;
    const BERLIN: i32 = 3600;

    fn alarm_ring(id: u16) -> u32 {
        ring::id(ring::ALARM, u32::from(id))
    }

    #[test]
    fn the_next_alarm_follows_the_zone() {
        let mut a = Alarms::new();
        let id = a.add(7, 30, Weekdays::EVERY_DAY, "").expect("room");
        // 07:00 in Berlin: 07:30 local is 06:30 UTC
        let p = plan(&a, &Timer::default(), MON_0600, MON_0600, BERLIN, &[]);
        assert_eq!(p.next, Some((MON_0600 + 30 * 60, alarm_ring(id))));
        assert!(p.due.is_empty());
        // moved to Moscow (UTC+3): 07:30 local was 04:30 UTC, gone; tomorrow's is next, nothing rings for the jump
        let p = plan(&a, &Timer::default(), MON_0600, MON_0600, 3 * 3600, &[]);
        assert_eq!(
            p.next,
            Some((MON_0600 + 22 * 3600 + 30 * 60, alarm_ring(id)))
        );
        assert!(p.due.is_empty());
        // to London (UTC): 07:30 local is 07:30 UTC
        let at = next_alarm(&a, MON_0600, 0).map(|(t, _)| t);
        assert_eq!(at, Some(MON_0600 + 90 * 60));
    }

    #[test]
    fn rings_in_its_minute_once() {
        let mut a = Alarms::new();
        let id = a.add(7, 0, Weekdays::ONCE, "").expect("room");
        let at = MON_0600; // 07:00 in Berlin
        let p = plan(&a, &Timer::default(), at - 30, at, BERLIN, &[]);
        assert_eq!(p.due, [alarm_ring(id)]);
        // a second plan in the same minute rings nothing again
        let p = plan(&a, &Timer::default(), at, at + 20, BERLIN, &[]);
        assert!(p.due.is_empty());
    }

    #[test]
    fn the_week_wraps() {
        let mut a = Alarms::new();
        // Monday 00:10 local; now is Sunday 23:50 local (Berlin)
        let id = a.add(0, 10, Weekdays::ONCE.toggled(0), "").expect("room");
        let sun_2350 = MON_0600 - 7 * 3600 - 10 * 60;
        let p = plan(&a, &Timer::default(), sun_2350, sun_2350, BERLIN, &[]);
        assert_eq!(p.next, Some((sun_2350 + 20 * 60, alarm_ring(id))));
        let p = plan(
            &a,
            &Timer::default(),
            sun_2350,
            sun_2350 + 20 * 60,
            BERLIN,
            &[],
        );
        assert_eq!(p.due, [alarm_ring(id)]);
    }

    #[test]
    fn a_snooze_rings_after_its_minutes() {
        let mut a = Alarms::new();
        let id = a.add(7, 0, Weekdays::EVERY_DAY, "Up").expect("room");
        let new = a
            .snooze_for(id, minute_of_week(MON_0600, BERLIN), 10)
            .expect("room");
        let p = plan(&a, &Timer::default(), MON_0600, MON_0600, BERLIN, &[]);
        assert_eq!(p.next, Some((MON_0600 + 10 * 60, alarm_ring(new))));
        assert_eq!(a.get(new).map(|x| x.label.as_str()), Some("Up"));
    }

    #[test]
    fn a_timer_ends_and_extras_count() {
        let mut t = Timer::default();
        t.set(90_000);
        t.start(MON_0600 * 1000);
        let timer = ring::id(ring::TIMER, 0);
        let reminder = ring::id(ring::REMINDER, 1);
        let extra = [(MON_0600 + 3600, reminder)];
        let p = plan(&Alarms::new(), &t, MON_0600, MON_0600 + 10, 0, &extra);
        assert_eq!(p.next, Some((MON_0600 + 90, timer)));
        let p = plan(&Alarms::new(), &t, MON_0600 + 60, MON_0600 + 90, 0, &extra);
        assert_eq!(p.due, [timer]);
        assert!(t.tick((MON_0600 + 90) * 1000));
        let p = plan(
            &Alarms::new(),
            &t,
            MON_0600 + 90,
            MON_0600 + 3600,
            0,
            &extra,
        );
        assert_eq!((p.due, p.next), (alloc::vec![reminder], None));
    }

    #[test]
    fn a_jump_forward_rings_a_missed_alarm_once() {
        let mut a = Alarms::new();
        let id = a.add(7, 30, Weekdays::EVERY_DAY, "").expect("room");
        // 07:00 to 10:00 Berlin in one jump (three days on, too): 07:30 rings once, not three times
        let later = MON_0600 + 3 * 86_400 + 3 * 3600;
        let p = plan(&a, &Timer::default(), MON_0600, later, BERLIN, &[]);
        assert_eq!(p.due, [alarm_ring(id)]);
        let p = plan(&a, &Timer::default(), later, later + 60, BERLIN, &[]);
        assert!(p.due.is_empty());
        // back an hour: nothing rings again
        let p = plan(&a, &Timer::default(), later, later - 3600, BERLIN, &[]);
        assert!(p.due.is_empty());
    }
}
