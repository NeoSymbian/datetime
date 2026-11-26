// SPDX-License-Identifier: Apache-2.0
//! DTSRV's planner: what rings now and what rings next, from the alarms (local minutes of the week), the timer
//! (wall-clock ms) and other dues (reminders), given UTC now and the local offset. Pure, so the zone changes, the week
//! wrap and clock jumps are tested here.
//!
//! Something is due when its time lies in `(since, now]`, `since` being the last plan's `now`: a jump forward rings
//! what it skipped once, and a zone change moves the alarms with local time. The interval since the last plan is
//! read under the offset in force then, so a deadline it crossed still rings when the offset changes before the next
//! plan. [`Rung`] keeps the last occurrence each alarm rang, so a clock set back (or a restart) rings none twice; at a
//! start, [`boot_since`] makes the current minute count.

use crate::alarm::{Alarm, Alarms, Minute};
use crate::ipc::ring;
use crate::timer::{Phase, Timer};
use alloc::vec::Vec;
use neoui_proto::clock::wire::{Corrupt, Reader, Writer};

/// A moment: Unix UTC ms and the local offset then (seconds east of UTC).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct At {
    pub ms: i64,
    pub offset: i32,
}

/// What the planner found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// The earliest ring after now: (Unix UTC ms, ring id).
    pub next: Option<(i64, u32)>,
    /// The rings due in `(since, now]`, alarms first, in time order of the list.
    pub due: Vec<u32>,
    /// The due alarms' occurrences: (alarm id, local minute since 1970), for [`Rung::mark`].
    pub occurrences: Vec<(u16, i64)>,
}

/// What has rung (DTSRV's `rings.dat`): per alarm the local minute its last occurrence rang, the rings not acted on
/// yet (reposted after a restart), and the alarms Snooze made with the snoozes left after them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rung {
    pub last: Vec<(u16, i64)>,
    pub ringing: Vec<u32>,
    pub snoozed: Vec<(u16, u8)>,
}

const RUNG_VERSION: u8 = 1;
const WEEK_MINUTES: i64 = 7 * 1440;

impl Rung {
    /// The local minute alarm `id` last rang.
    #[must_use]
    pub fn last(&self, id: u16) -> Option<i64> {
        self.last.iter().find(|l| l.0 == id).map(|l| l.1)
    }

    /// Alarm `id` rang its occurrence at local minute `m`.
    pub fn mark(&mut self, id: u16, m: i64) {
        match self.last.iter_mut().find(|l| l.0 == id) {
            Some(l) => l.1 = l.1.max(m),
            None => self.last.push((id, m)),
        }
    }

    /// Forgets alarm `id` (it was deleted).
    pub fn forget(&mut self, id: u16) {
        self.last.retain(|l| l.0 != id);
        self.snoozed.retain(|s| s.0 != id);
        self.ringing
            .retain(|&r| r != ring::id(ring::ALARM, u32::from(id)));
    }

    /// `u8 n, n × (u16 id, i64 minute)`, `u8 n, n × u32 ring`, `u8 n, n × (u16 id, u8 left)` (each list cut at 255).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new(RUNG_VERSION);
        let n = |len: usize| len.min(255);
        // n() <= 255
        w.u8(n(self.last.len()) as u8);
        for &(id, m) in &self.last[..n(self.last.len())] {
            w.u16(id).i64(m);
        }
        w.u8(n(self.ringing.len()) as u8);
        for &r in &self.ringing[..n(self.ringing.len())] {
            w.u32(r);
        }
        w.u8(n(self.snoozed.len()) as u8);
        for &(id, left) in &self.snoozed[..n(self.snoozed.len())] {
            w.u16(id).u8(left);
        }
        w.finish()
    }

    /// # Errors
    /// [`Corrupt`] for a damaged blob.
    pub fn decode(b: &[u8]) -> Result<Self, Corrupt> {
        let mut r = Reader::new(b, RUNG_VERSION)?;
        let n = r.u8()?;
        let last = (0..n)
            .map(|_| Ok((r.u16()?, r.i64()?)))
            .collect::<Result<_, Corrupt>>()?;
        let n = r.u8()?;
        let ringing = (0..n).map(|_| r.u32()).collect::<Result<_, _>>()?;
        let n = r.u8()?;
        let snoozed = (0..n)
            .map(|_| Ok((r.u16()?, r.u8()?)))
            .collect::<Result<_, Corrupt>>()?;
        r.end()?;
        Ok(Self {
            last,
            ringing,
            snoozed,
        })
    }
}

/// The first plan's `since` at a start: the end of the minute before `now_ms`, so an alarm in the current minute
/// still rings (once: [`Rung`] knows when it did).
#[must_use]
pub fn boot_since(now_ms: i64) -> i64 {
    now_ms - now_ms.rem_euclid(60_000) - 1
}

/// Plans at `now`; `since` is the previous plan's `now` (with its offset). `extra` are other dues: `(utc seconds, ring
/// id)`.
#[must_use]
pub fn plan(
    alarms: &Alarms,
    timer: &Timer,
    rung: &Rung,
    since: At,
    now: At,
    extra: &[(i64, u32)],
) -> Plan {
    // the elapsed interval as the clock read then; what comes next under the offset now
    let (since_m, upto_m) = (
        local_minute(since.ms, since.offset),
        local_minute(now.ms, since.offset),
    );
    let now_m = local_minute(now.ms, now.offset);
    let mut p = Plan::default();
    for a in alarms.as_slice() {
        let id = ring::id(ring::ALARM, u32::from(a.id));
        let last = rung.last(a.id);
        if let Some(at) = after(a, since_m, last)
            && at <= upto_m
        {
            p.due.push(id);
            p.occurrences.push((a.id, at));
        }
        if let Some(at) = after(a, now_m, last) {
            earliest(&mut p.next, minute_ms(at, now.offset), id);
        }
    }
    if let Phase::Running { ends_ms } = timer.phase {
        let id = ring::id(ring::TIMER, 0);
        if ends_ms <= now.ms {
            p.due.push(id);
        } else {
            earliest(&mut p.next, ends_ms, id);
        }
    }
    for &(at, id) in extra {
        let at = at.saturating_mul(1000);
        if since.ms < at && at <= now.ms {
            p.due.push(id);
        } else if at > now.ms {
            earliest(&mut p.next, at, id);
        }
    }
    p
}

/// The next alarm after `now`: (Unix UTC seconds, the alarm).
#[must_use]
pub fn next_alarm<'a>(
    alarms: &'a Alarms,
    rung: &Rung,
    now: i64,
    offset: i32,
) -> Option<(i64, &'a Alarm)> {
    let now_m = local_minute(now.saturating_mul(1000), offset);
    alarms
        .as_slice()
        .iter()
        .filter_map(|a| after(a, now_m, rung.last(a.id)).map(|at| (at * 60 - i64::from(offset), a)))
        .min_by_key(|&(at, _)| at)
}

/// The local minute of the week at `utc`, for `Alarms::snooze_for` and the like.
#[must_use]
pub fn minute_of_week(utc: i64, offset: i32) -> Minute {
    week_minute(local_minute(utc.saturating_mul(1000), offset))
}

/// The local minute (since 1970-01-01 00:00) at `ms`.
#[must_use]
pub fn local_minute(ms: i64, offset: i32) -> i64 {
    (ms.div_euclid(1000) + i64::from(offset)).div_euclid(60)
}

/// The UTC ms local minute `m` begins at.
fn minute_ms(m: i64, offset: i32) -> i64 {
    (m * 60 - i64::from(offset)).saturating_mul(1000)
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

/// The local minute alarm `a` next rings after minute `m`, past the occurrence it last rang (`last`); `None` when it
/// is off.
fn after(a: &Alarm, m: i64, last: Option<i64>) -> Option<i64> {
    let next = |m: i64| a.minutes_until(week_minute(m)).map(|d| m + i64::from(d));
    let at = next(m)?;
    match last {
        // a mark over a week ahead is a clock that was set forward and back: forgotten
        Some(l) if at <= l && l - m < WEEK_MINUTES => next(l),
        _ => Some(at),
    }
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

    /// Seconds and one offset for both ends, as most tests need.
    fn plan_s(
        a: &Alarms,
        t: &Timer,
        since: i64,
        now: i64,
        offset: i32,
        extra: &[(i64, u32)],
    ) -> Plan {
        let at = |s: i64| At {
            ms: s * 1000,
            offset,
        };
        plan(a, t, &Rung::default(), at(since), at(now), extra)
    }

    fn ms(s: i64) -> i64 {
        s * 1000
    }

    #[test]
    fn the_next_alarm_follows_the_zone() {
        let mut a = Alarms::new();
        let id = a.add(7, 30, Weekdays::EVERY_DAY, "").expect("room");
        // 07:00 in Berlin: 07:30 local is 06:30 UTC
        let p = plan_s(&a, &Timer::default(), MON_0600, MON_0600, BERLIN, &[]);
        assert_eq!(p.next, Some((ms(MON_0600 + 30 * 60), alarm_ring(id))));
        assert!(p.due.is_empty());
        // moved to Moscow (UTC+3): 07:30 local was 04:30 UTC, gone; tomorrow's is next, nothing rings for the jump
        let p = plan_s(&a, &Timer::default(), MON_0600, MON_0600, 3 * 3600, &[]);
        assert_eq!(
            p.next,
            Some((ms(MON_0600 + 22 * 3600 + 30 * 60), alarm_ring(id)))
        );
        assert!(p.due.is_empty());
        // to London (UTC): 07:30 local is 07:30 UTC
        let at = next_alarm(&a, &Rung::default(), MON_0600, 0).map(|(t, _)| t);
        assert_eq!(at, Some(MON_0600 + 90 * 60));
    }

    #[test]
    fn rings_in_its_minute_once() {
        let mut a = Alarms::new();
        let id = a.add(7, 0, Weekdays::ONCE, "").expect("room");
        let at = MON_0600; // 07:00 in Berlin
        let p = plan_s(&a, &Timer::default(), at - 30, at, BERLIN, &[]);
        assert_eq!(p.due, [alarm_ring(id)]);
        // its occurrence: 07:00 local that day
        assert_eq!(p.occurrences, [(id, local_minute(ms(at), BERLIN))]);
        // a second plan in the same minute rings nothing again
        let p = plan_s(&a, &Timer::default(), at, at + 20, BERLIN, &[]);
        assert!(p.due.is_empty());
    }

    #[test]
    fn the_week_wraps() {
        let mut a = Alarms::new();
        // Monday 00:10 local; now is Sunday 23:50 local (Berlin)
        let id = a.add(0, 10, Weekdays::ONCE.toggled(0), "").expect("room");
        let sun_2350 = MON_0600 - 7 * 3600 - 10 * 60;
        let p = plan_s(&a, &Timer::default(), sun_2350, sun_2350, BERLIN, &[]);
        assert_eq!(p.next, Some((ms(sun_2350 + 20 * 60), alarm_ring(id))));
        let p = plan_s(
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
        let p = plan_s(&a, &Timer::default(), MON_0600, MON_0600, BERLIN, &[]);
        assert_eq!(p.next, Some((ms(MON_0600 + 10 * 60), alarm_ring(new))));
        assert_eq!(a.get(new).map(|x| x.label.as_str()), Some("Up"));
    }

    #[test]
    fn a_timer_ends_and_extras_count() {
        let mut t = Timer::default();
        t.set(90_000);
        t.start(ms(MON_0600));
        let timer = ring::id(ring::TIMER, 0);
        let reminder = ring::id(ring::REMINDER, 1);
        let extra = [(MON_0600 + 3600, reminder)];
        let p = plan_s(&Alarms::new(), &t, MON_0600, MON_0600 + 10, 0, &extra);
        assert_eq!(p.next, Some((ms(MON_0600 + 90), timer)));
        let p = plan_s(&Alarms::new(), &t, MON_0600 + 60, MON_0600 + 90, 0, &extra);
        assert_eq!(p.due, [timer]);
        assert!(t.tick(ms(MON_0600 + 90)));
        let p = plan_s(
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
        let p = plan_s(&a, &Timer::default(), MON_0600, later, BERLIN, &[]);
        assert_eq!(p.due, [alarm_ring(id)]);
        let p = plan_s(&a, &Timer::default(), later, later + 60, BERLIN, &[]);
        assert!(p.due.is_empty());
        // back an hour: nothing rings again
        let p = plan_s(&a, &Timer::default(), later, later - 3600, BERLIN, &[]);
        assert!(p.due.is_empty());
    }

    #[test]
    fn a_clock_set_back_doesnt_ring_an_occurrence_twice() {
        let mut a = Alarms::new();
        let id = a.add(7, 30, Weekdays::EVERY_DAY, "").expect("room");
        let at = |s: i64| At {
            ms: ms(s),
            offset: BERLIN,
        };
        let t0730 = MON_0600 + 30 * 60; // 07:30 Berlin
        let mut rung = Rung::default();
        let p = plan(
            &a,
            &Timer::default(),
            &rung,
            at(t0730 - 10),
            at(t0730 + 5),
            &[],
        );
        assert_eq!(p.due, [alarm_ring(id)]);
        for &(id, m) in &p.occurrences {
            rung.mark(id, m);
        }
        // dismissed, then the clock set back to 07:20: 07:30 comes again and rings nothing
        let back = t0730 - 10 * 60;
        let p = plan(&a, &Timer::default(), &rung, at(back), at(t0730 + 5), &[]);
        assert!(p.due.is_empty());
        // tomorrow's rings, and is what comes next
        assert_eq!(p.next, Some((ms(t0730 + 86_400), alarm_ring(id))));
        let p = plan(
            &a,
            &Timer::default(),
            &rung,
            at(t0730 + 86_400 - 10),
            at(t0730 + 86_400),
            &[],
        );
        assert_eq!(p.due, [alarm_ring(id)]);
        let at_s = next_alarm(&a, &rung, back, BERLIN).map(|(t, _)| t);
        assert_eq!(at_s, Some(t0730 + 86_400));
        // a mark over a week ahead (the clock was set a year forward and back) is forgotten
        let mut far = Rung::default();
        far.mark(id, local_minute(ms(t0730 + 365 * 86_400), BERLIN));
        let p = plan(&a, &Timer::default(), &far, at(t0730 - 10), at(t0730), &[]);
        assert_eq!(p.due, [alarm_ring(id)]);
    }

    #[test]
    fn a_start_in_the_alarms_minute_rings_it_once() {
        let mut a = Alarms::new();
        let id = a.add(7, 30, Weekdays::EVERY_DAY, "").expect("room");
        let t0730 = MON_0600 + 30 * 60;
        let now = At {
            ms: ms(t0730),
            offset: BERLIN,
        };
        let since = At {
            ms: boot_since(now.ms),
            offset: BERLIN,
        };
        assert_eq!(since.ms, ms(t0730) - 1);
        let mut rung = Rung::default();
        let p = plan(&a, &Timer::default(), &rung, since, now, &[]);
        assert_eq!(p.due, [alarm_ring(id)]);
        // a restart later in that minute, after it rang: not again
        rung.mark(id, p.occurrences[0].1);
        let now = At {
            ms: ms(t0730 + 40),
            offset: BERLIN,
        };
        let since = At {
            ms: boot_since(now.ms),
            ..now
        };
        let p = plan(&a, &Timer::default(), &rung, since, now, &[]);
        assert!(p.due.is_empty());
    }

    #[test]
    fn a_deadline_crossed_before_an_offset_change_still_rings() {
        let mut a = Alarms::new();
        let id = a.add(7, 30, Weekdays::EVERY_DAY, "").expect("room");
        let t0730 = MON_0600 + 30 * 60; // 07:30 Berlin = 06:30 London
        // planned at 07:29:50 Berlin; by the next plan 07:30 Berlin passed and the zone became London
        let since = At {
            ms: ms(t0730 - 10),
            offset: BERLIN,
        };
        let now = At {
            ms: ms(t0730 + 10),
            offset: 0,
        };
        let p = plan(&a, &Timer::default(), &Rung::default(), since, now, &[]);
        assert_eq!(p.due, [alarm_ring(id)]);
        // what comes next follows London: 07:30 London today
        assert_eq!(p.next, Some((ms(t0730 + 3600), alarm_ring(id))));
    }

    #[test]
    fn timers_compare_in_ms_and_survive_a_corrupt_end() {
        let mut t = Timer::default();
        t.set(1000);
        let start = ms(MON_0600) + 1;
        t.start(start);
        let timer = ring::id(ring::TIMER, 0);
        let at = |ms: i64| At { ms, offset: 0 };
        let p = plan(
            &Alarms::new(),
            &t,
            &Rung::default(),
            at(start),
            at(start + 999),
            &[],
        );
        assert_eq!(p.next, Some((start + 1000, timer)));
        assert!(p.due.is_empty());
        // due at 1.001 s, not at 2 s
        let p = plan(
            &Alarms::new(),
            &t,
            &Rung::default(),
            at(start + 999),
            at(start + 1000),
            &[],
        );
        assert_eq!(p.due, [timer]);
        let corrupt = Timer {
            phase: Phase::Running { ends_ms: i64::MAX },
            ..t
        };
        let p = plan(
            &Alarms::new(),
            &corrupt,
            &Rung::default(),
            at(start),
            at(start),
            &[],
        );
        assert_eq!(p.next, Some((i64::MAX, timer)));
        // an extra at the far end doesn't overflow either
        let p = plan(
            &Alarms::new(),
            &Timer::default(),
            &Rung::default(),
            at(start),
            at(start),
            &[(i64::MAX, 7)],
        );
        assert_eq!(p.next, Some((i64::MAX, 7)));
    }

    #[test]
    fn rung_round_trips_and_forgets() {
        let mut r = Rung::default();
        r.mark(3, 100);
        r.mark(3, 90);
        r.mark(4, 7);
        r.ringing.push(ring::id(ring::ALARM, 3));
        r.ringing.push(ring::id(ring::TIMER, 0));
        r.snoozed.push((3, 2));
        assert_eq!(r.last(3), Some(100));
        assert_eq!(Rung::decode(&r.encode()), Ok(r.clone()));
        let b = r.encode();
        assert_eq!(Rung::decode(&b[..b.len() - 1]), Err(Corrupt));
        r.forget(3);
        assert_eq!(r.last(3), None);
        assert_eq!(r.ringing, [ring::id(ring::TIMER, 0)]);
        assert!(r.snoozed.is_empty());
    }
}
