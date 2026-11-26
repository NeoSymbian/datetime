// SPDX-License-Identifier: GPL-3.0-only
//! Calendar's state and what every tap does: which view shows (and what Back goes to), the data read from DTSRV for
//! it, the forms and their sheets, the shell's page, dialogs, toasts and keyboard.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use calendar_core::{DAY, EventDraft, Instance, Kind, Repeat, Todo};
use calendar_ui::view::{self, CalInfo, Found, Now, Prefs};
use calendar_ui::{Cal, CalendarApp, KeyboardFocus, Kit};
use core::cell::{Cell, RefCell};
use datetime_core::pages::calendar as page;
use dt_client as dt;
use e32::dprintln;
use e32::fs::{File, Fs, mode};
use neoapp::{App, Page, Query};
use neoui_proto::ipc as proto;
use neoui_proto::open::files;
use neoui_proto::settings::Value;
use pim_ics::Date;
use slint::{ComponentHandle, SharedString};

// Cal.view
const MONTH: i32 = 0;
const WEEK: i32 = 1;
const DAY_VIEW: i32 = 2;
const TODO: i32 = 3;
const EVENT: i32 = 10;
const EVENT_FORM: i32 = 11;
const TODO_FORM: i32 = 12;
const ANNIV_FORM: i32 = 13;
const BIRTHDAY: i32 = 14;
const SEARCH: i32 = 15;
const SETTINGS: i32 = 16;
const CALENDARS: i32 = 17;
const IMPORT: i32 = 18;
const RING: i32 = 19;

/// The settings keys (docs datetime.md, `dt2-spec-keys`).
mod key {
    pub const VIEW: &str = "calendar.view";
    pub const WEEK_START: &str = "calendar.week_start";
    pub const WEEK_NUMBERS: &str = "calendar.week_numbers";
    pub const REMINDER: &str = "calendar.reminder";
    pub const LENGTH: &str = "calendar.length";
    pub const RING: &str = "calendar.ring";
    pub const BIRTHDAYS: &str = "calendar.birthdays";
    pub const HIDDEN: &str = "calendar.hidden";
}

/// The apps a birthday opens (neoui `apps`) and their pages (NeoSymbian/phone `phone_core::page::number`).
mod other {
    pub const PHONE: &str = "phone";
    pub const DIAL: u16 = 1;
    pub const MESSAGES: &str = "messages";
    pub const NEW_MESSAGE: u16 = 3;
    pub const CONTACTS: &str = "contacts";
    pub const CONTACT_LIST: u16 = 0;
    pub const CLOCK: &str = "clock";
}

/// What a choice sheet, a picker or a dialog answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    None,
    WeekStart,
    OpenOn,
    Reminder,
    Length,
    Calendar,
    ImportCalendar,
    Since,
    Starts,
    StartTime,
    Ends,
    EndTime,
    Due,
    AnniversaryDate,
    /// a repeating event's Edit: this one or all
    EditWhich,
    /// Delete: (this one, all,) Cancel
    DeleteWhich,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    Event,
    Todo,
    Anniversary,
}

/// An .ics file being added.
struct Import {
    text: String,
    first: EventDraft,
    n: usize,
    cal: u8,
    from: String,
    path: String,
}

pub struct Model {
    prefs: Prefs,
    now: Now,
    view: i32,
    /// the views Back goes to
    stack: Vec<i32>,
    year: i32,
    month: u8,
    /// the selected day
    sel: i64,
    /// the month grid's or the week's occurrences (the rows' indices point here)
    items: Vec<Instance>,
    todos: Vec<Todo>,
    show_done: bool,
    cals: Vec<CalInfo>,
    /// Clock's next alarm (local time, "14:30 Medicine")
    alarm: Option<(i64, String)>,
    /// the event view's occurrence and its series
    event: Instance,
    series: EventDraft,
    form: Form,
    draft: EventDraft,
    todo: Todo,
    ask: Ask,
    search: Vec<Instance>,
    found: Vec<Found>,
    ring: Option<(u32, dt::RingInfo)>,
    import: Option<Import>,
    cal_edit: Option<usize>,
    /// an export waiting for Files' Save
    saving: Option<String>,
    /// waiting for Files' picker (Import .ics)
    picking: bool,
    /// the to-do the toast's Undo takes back
    undo: Option<Todo>,
    /// another app opened the page showing (Ringing, Add to Calendar): leaving it goes back there (DONE)
    opened: bool,
}

pub struct Ctx {
    pub app: App,
    ui: slint::Weak<CalendarApp>,
    m: RefCell<Model>,
    dt: RefCell<Option<dt::Client>>,
    shown: RefCell<Option<Page>>,
    keyboard: Cell<(bool, i32, i32)>,
    badge: Cell<u32>,
    minute: Cell<i64>,
}

fn now() -> Now {
    let (utc, tz) = e32::time::utc_now();
    Now {
        local: utc + i64::from(tz),
        tz,
    }
}

fn ymd(day: i64) -> Date {
    Date::from_days(day)
}

/// "2026-11-16" or "2026-11" (the 1st)
fn parse_day(s: &str) -> Option<i64> {
    let mut it = s.trim().split('-');
    let year = it.next()?.parse().ok()?;
    let month: u8 = it.next()?.parse().ok()?;
    let day: u8 = it.next().map_or(Some(1), |d| d.parse().ok())?;
    (1..=12).contains(&month).then_some(())?;
    (1..=31).contains(&day).then_some(())?;
    Some(Date { year, month, day }.days())
}

impl Ctx {
    pub fn new(app: App, ui: &CalendarApp) -> Self {
        let now = now();
        let today = now.today();
        let d = ymd(today);
        Self {
            app,
            ui: ui.as_weak(),
            m: RefCell::new(Model {
                prefs: Prefs::default(),
                now,
                view: MONTH,
                stack: Vec::new(),
                year: d.year,
                month: d.month,
                sel: today,
                items: Vec::new(),
                todos: Vec::new(),
                show_done: false,
                cals: Vec::new(),
                alarm: None,
                event: Instance::default(),
                series: EventDraft::default(),
                form: Form::Event,
                draft: EventDraft::default(),
                todo: Todo::default(),
                ask: Ask::None,
                search: Vec::new(),
                found: Vec::new(),
                ring: None,
                import: None,
                cal_edit: None,
                saving: None,
                picking: false,
                undo: None,
                opened: false,
            }),
            dt: RefCell::new(None),
            shown: RefCell::new(None),
            keyboard: Cell::new((false, 0, 0)),
            badge: Cell::new(u32::MAX),
            minute: Cell::new(now.local / 60),
        }
    }

    pub fn ui(&self) -> CalendarApp {
        self.ui
            .upgrade()
            .expect("the window lives as long as the app")
    }

    /// DTSRV, connected on first use and again after a failure.
    fn dt<T>(&self, what: &str, f: impl FnOnce(&dt::Client) -> Result<T, i32>) -> Option<T> {
        if self.dt.borrow().is_none() {
            match dt::Client::connect() {
                Ok(c) => *self.dt.borrow_mut() = Some(c),
                Err(e) => {
                    dprintln!("calendar: DTSRV not reached ({})", e);
                    return None;
                }
            }
        }
        let r = f(self.dt.borrow().as_ref()?);
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                dprintln!("calendar: {} failed ({})", what, e);
                // -15 KErrServerTerminated: connect again next time
                if e == -15 {
                    *self.dt.borrow_mut() = None;
                }
                None
            }
        }
    }

    /// The first screen: the settings, then the tab `calendar.view` names.
    pub fn start(&self) {
        self.read_prefs();
        let v = i32::from(self.m.borrow().prefs.view.min(3));
        self.tab(v);
    }

    fn read_prefs(&self) {
        let int = |k: &str, d: i32| match self.app.setting_raw(k) {
            Ok(Value::Int(n)) => n,
            _ => d,
        };
        let flag = |k: &str, d: bool| match self.app.setting_raw(k) {
            Ok(Value::Bool(b)) => b,
            _ => d,
        };
        let d = Prefs::default();
        let hidden = match self.app.setting_raw(key::HIDDEN) {
            Ok(Value::Bytes(b)) if b.len() >= 4 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            _ => 0,
        };
        // the stored values are small; out-of-range ones fall back to the defaults
        let p = Prefs {
            view: u8::try_from(int(key::VIEW, 0)).unwrap_or(0).min(3),
            week_start: u8::try_from(int(key::WEEK_START, 0)).unwrap_or(0).min(6),
            week_numbers: flag(key::WEEK_NUMBERS, d.week_numbers),
            reminder: u16::try_from(int(key::REMINDER, i32::from(d.reminder)))
                .unwrap_or(d.reminder),
            length: u16::try_from(int(key::LENGTH, i32::from(d.length)))
                .unwrap_or(d.length)
                .max(5),
            ring: flag(key::RING, d.ring),
            birthdays: flag(key::BIRTHDAYS, d.birthdays),
            hidden,
        };
        self.m.borrow_mut().prefs = p;
    }

    fn set_pref(&self, k: &str, v: Value) {
        if let Err(e) = self.app.set_setting_raw(k, &v) {
            dprintln!("calendar: {} not saved ({})", k, e);
        }
    }

    pub fn settings_changed(&self) {
        self.read_prefs();
        self.refresh();
    }

    /// How long the loop may sleep: to the next minute, or sooner for Slint (a sheet sliding).
    pub fn wake_us(&self) -> i32 {
        let local = now().local;
        let to_minute = ((60 - local.rem_euclid(60)) * 1_000_000) as i32;
        let slint = self
            .app
            .wake_us()
            .map_or(i32::MAX, |us| i32::try_from(us).unwrap_or(i32::MAX));
        to_minute.min(slint).max(1000)
    }

    /// The loop woke: a new minute redraws the view (the now line, "in 5 h 36 min").
    pub fn tick(&self) {
        let n = now();
        if n.local / 60 != self.minute.get() {
            self.minute.set(n.local / 60);
            self.m.borrow_mut().now = n;
            // a new day moves "today"
            self.render();
        }
    }

    pub fn visible(&self, v: bool) {
        if !v {
            return;
        }
        // back from Files: a file saved or picked
        let (saving, picking) = {
            let mut m = self.m.borrow_mut();
            (m.saving.take(), core::mem::take(&mut m.picking))
        };
        if let Some(text) = saving {
            match self.app.result_file() {
                Some(f) => match f.file().write(text.as_bytes()) {
                    Ok(()) => self.note("Saved to Files"),
                    Err(e) => {
                        dprintln!("calendar: export not written ({})", e);
                        self.note("Couldn't save the file");
                    }
                },
                None => dprintln!("calendar: save cancelled"),
            }
        }
        if picking && let Some(f) = self.app.result_file() {
            let file = f.file();
            let path = file.full_name().unwrap_or_default();
            match read_all(&file) {
                Ok(b) => self.import_text(&b, "Files", &path),
                Err(e) => {
                    dprintln!("calendar: {} not read ({})", path, e);
                    self.note("Couldn't read the file");
                }
            }
        }
        self.refresh();
    }

    // --- views ---------------------------------------------------------------------------------------------------

    fn set_view(&self, v: i32) {
        self.m.borrow_mut().view = v;
        self.ui().global::<Cal>().set_view(v);
        dprintln!("calendar: view {}", view_name(v));
    }

    /// A tab: the stack goes; Back from Week, Day or To-do goes to Month.
    fn tab(&self, v: i32) {
        {
            let mut m = self.m.borrow_mut();
            m.stack.clear();
            m.show_done = false;
            if v == MONTH {
                let d = ymd(m.sel);
                m.year = d.year;
                m.month = d.month;
            }
        }
        self.set_view(v);
        self.refresh();
    }

    /// A page over the current view.
    fn push(&self, v: i32) {
        {
            let mut m = self.m.borrow_mut();
            let cur = m.view;
            if cur != v {
                m.stack.push(cur);
            }
        }
        self.set_view(v);
    }

    pub fn back(&self) {
        let (prev, view, opened) = {
            let mut m = self.m.borrow_mut();
            (m.stack.pop(), m.view, m.opened)
        };
        // Back on Ringing is Snooze (dt2-spec-map)
        if view == RING {
            self.ring_act(dt::action::SNOOZE);
            self.leave();
            return;
        }
        if view == TODO && self.m.borrow().show_done {
            self.m.borrow_mut().show_done = false;
            self.render();
            return;
        }
        match prev {
            // a page another app opened us on: back there
            None if opened => self.leave(),
            Some(v) => {
                self.set_view(v);
                self.refresh();
            }
            None if view != MONTH => self.tab(MONTH),
            None => {}
        }
    }

    /// Done with the page another app opened (Ringing, Add to Calendar): the shell shows that app again.
    fn leave(&self) {
        {
            let mut m = self.m.borrow_mut();
            m.opened = false;
            m.ring = None;
            m.import = None;
        }
        self.tab(MONTH);
        if let Err(e) = self.app.done() {
            dprintln!("calendar: done not sent ({})", e);
        }
    }

    /// DTSRV says these stores changed.
    pub fn changed(&self, bits: u32) {
        dprintln!("calendar: changed {:#x}", bits);
        self.refresh();
    }

    /// Read what the view needs from DTSRV and show it.
    fn refresh(&self) {
        self.m.borrow_mut().now = now();
        let (view, hidden, sel, year, month, ws) = {
            let m = self.m.borrow();
            (
                m.view,
                m.prefs.hidden,
                m.sel,
                m.year,
                m.month,
                m.prefs.week_start,
            )
        };
        if let Some(cals) = self.dt("calendars", dt::Client::calendars) {
            self.m.borrow_mut().cals = cals
                .into_iter()
                .map(|c| CalInfo {
                    name: c.name,
                    colour: c.colour,
                    count: u32::from(c.events),
                })
                .collect();
        }
        let todos = self.dt("to-dos", dt::Client::todo_list);
        self.m.borrow_mut().todos = todos.unwrap_or_default();
        let range = match view {
            MONTH => {
                let s = view::grid_start(year, month, ws);
                Some((s, s + 41))
            }
            WEEK | DAY_VIEW => {
                let s = view::week_of(sel, ws);
                Some((s, s + 6))
            }
            _ => None,
        };
        if let Some((a, b)) = range {
            // local days since 1970 fit an i32 for millennia
            let items = self.dt("query", |c| c.event_query(a as i32, b as i32, hidden));
            self.m.borrow_mut().items = items.unwrap_or_default();
        }
        if matches!(view, MONTH | DAY_VIEW) {
            let next = self.dt("next", dt::Client::next);
            let tz = i64::from(self.m.borrow().now.tz);
            self.m.borrow_mut().alarm = next.filter(|n| n.alarm_at != 0).map(|n| {
                let local = n.alarm_at + tz;
                (
                    local,
                    format!("{} {}", view::hm(local), n.alarm_label)
                        .trim_end()
                        .into(),
                )
            });
        }
        self.render();
    }

    /// Show the current view from the model (no IPC).
    fn render(&self) {
        let ui = self.ui();
        let cal = ui.global::<Cal>();
        let m = self.m.borrow();
        // the badge and the To-do tab's count come from every view
        view::todos(&cal, &m.now, &m.todos, m.show_done);
        match m.view {
            MONTH => {
                let pin = m
                    .alarm
                    .as_ref()
                    .filter(|(t, _)| t.div_euclid(DAY) == m.sel)
                    .map(|(_, s)| format!("{s} · Clock"))
                    .unwrap_or_default();
                let busy = m.prefs.week_numbers;
                view::month(
                    &cal, &m.now, &m.prefs, m.year, m.month, m.sel, &m.items, busy, &pin,
                );
            }
            WEEK => view::week(
                &cal,
                &m.now,
                view::week_of(m.sel, m.prefs.week_start),
                m.sel,
                &m.items,
            ),
            DAY_VIEW => {
                let pins: Vec<(i64, String)> = m.alarm.iter().cloned().collect();
                view::day(&cal, &m.now, &m.prefs, m.sel, &m.items, &pins);
            }
            EVENT => {
                let name = self.cal_name(&m, m.event.cal);
                view::event(&cal, &m.now, &m.event, &m.series, &name, m.prefs.ring);
            }
            SETTINGS => {
                let n = m.cals.iter().map(|c| c.count as usize).sum();
                let zone = match self.app.setting_raw("time.zone") {
                    Ok(Value::Str(z)) => pim_ics::date::city(&z),
                    _ => String::new(),
                };
                view::settings(&cal, &m.prefs, &zone, n);
            }
            CALENDARS => {
                view::calendars(&cal, &m.cals, m.prefs.hidden, m.prefs.birthdays, m.cal_edit)
            }
            _ => {}
        }
    }

    fn cal_name(&self, m: &Model, i: u8) -> String {
        if i == u8::MAX {
            return "Birthdays".into();
        }
        m.cals
            .get(usize::from(i))
            .map(|c| c.name.clone())
            .unwrap_or_default()
    }

    // --- another app opened us -----------------------------------------------------------------------------------

    pub fn opened(&self, n: u16, arg: &str) {
        dprintln!("calendar: opened on page {} ({})", n, arg);
        self.m.borrow_mut().opened = false;
        match n {
            page::MONTH | page::WEEK | page::DAY => {
                if let Some(d) = parse_day(arg) {
                    self.m.borrow_mut().sel = d;
                }
                self.tab(i32::from(n));
            }
            page::TODO => self.tab(TODO),
            page::EVENT => {
                self.tab(MONTH);
                self.open_uid(arg);
            }
            page::NEW => {
                let day = parse_day(arg).unwrap_or(self.m.borrow().now.today());
                self.m.borrow_mut().sel = day;
                self.tab(MONTH);
                self.new_event(day, false);
            }
            page::IMPORT => {
                self.m.borrow_mut().opened = true;
                // Files' Open with: the file itself; Add to Calendar: its path
                if let Some(f) = self.app.arg_file() {
                    let file = f.file();
                    let path = file.full_name().unwrap_or_default();
                    match read_all(&file) {
                        Ok(b) => self.import_text(&b, "Files", &path),
                        Err(e) => self.unreadable(&path, e),
                    }
                } else if !arg.is_empty() {
                    match read_path(arg) {
                        Ok(b) => self.import_text(&b, "Files", arg),
                        Err(e) => self.unreadable(arg, e),
                    }
                } else {
                    self.note("No file to add");
                    self.leave();
                }
            }
            page::RING => match arg.trim().parse::<u32>() {
                Ok(id) => self.ring(id),
                Err(_) => self.tab(MONTH),
            },
            page::SETTINGS => {
                self.tab(MONTH);
                self.push(SETTINGS);
                self.render();
            }
            _ => self.tab(MONTH),
        }
    }

    fn unreadable(&self, path: &str, e: i32) {
        dprintln!("calendar: {} not read ({})", path, e);
        self.note("Couldn't read the file");
        self.leave();
    }

    /// The event page for `uid` (its series' first look: the next occurrence isn't known here).
    fn open_uid(&self, uid: &str) {
        let Some(d) = self.dt("event", |c| c.event_get(uid, None)) else {
            self.note("Event not found");
            return;
        };
        let colour = {
            let m = self.m.borrow();
            m.cals.get(usize::from(d.cal)).map_or(0, |c| c.colour)
        };
        let i = Instance {
            uid: d.uid.clone(),
            start: d.start,
            end: d.end,
            recur: d.start,
            all_day: d.all_day,
            kind: d.kind,
            title: d.title.clone(),
            place: d.place.clone(),
            cal: d.cal,
            colour,
            reminder: d.reminder.is_some(),
            repeats: d.repeat != Repeat::Never,
            ..Instance::default()
        };
        self.show_event(i, d);
    }

    fn show_event(&self, i: Instance, series: EventDraft) {
        {
            let mut m = self.m.borrow_mut();
            m.event = i;
            m.series = series;
        }
        self.push(EVENT);
        self.render();
    }

    /// Ringing (dt2-ring-event): DTSRV's ring `id`.
    fn ring(&self, id: u32) {
        dprintln!("calendar: ring {}", id);
        let Some(info) = self.dt("ring", |c| c.ring_info(id)) else {
            self.tab(MONTH);
            return;
        };
        let (now, name) = {
            let m = self.m.borrow();
            let name = m
                .cals
                .iter()
                .find(|c| c.colour == info.colour)
                .map(|c| c.name.clone())
                .unwrap_or_default();
            (m.now, name)
        };
        let tz = i64::from(now.tz);
        let i = Instance {
            uid: info.uid.clone(),
            start: info.start + tz,
            end: info.end + tz,
            recur: info.start + tz,
            all_day: info.all_day,
            title: info.title.clone(),
            place: info.place.clone(),
            colour: info.colour,
            ..Instance::default()
        };
        let ui = self.ui();
        let cal = ui.global::<Cal>();
        // Clock's snooze length (DTSRV snoozes reminders by it too)
        let snooze = match self.app.setting_raw("clock.snooze") {
            Ok(Value::Int(n)) => u8::try_from(n).unwrap_or(5),
            _ => 5,
        };
        view::ring(&cal, &now, &i, &name, snooze);
        {
            let mut m = self.m.borrow_mut();
            m.ring = Some((id, info));
            m.event = i;
            m.opened = true;
            m.stack.clear();
        }
        self.set_view(RING);
    }

    fn ring_act(&self, action: i32) {
        let Some((id, _)) = self.m.borrow().ring.clone() else {
            return;
        };
        let _ = self.dt("ring action", |c| c.ring_action(id, action));
    }

    // --- import, export ------------------------------------------------------------------------------------------

    fn import_text(&self, bytes: &[u8], from: &str, path: &str) {
        let text = String::from_utf8_lossy(bytes).into_owned();
        let tz = self.m.borrow().now.tz;
        let Some((first, n)) = view::preview(&text, &tz) else {
            self.note("No events in this file");
            if self.m.borrow().opened {
                self.leave();
            }
            return;
        };
        let cal = {
            let m = self.m.borrow();
            m.cals.iter().position(|c| !c.name.is_empty()).unwrap_or(0) as u8
        };
        self.m.borrow_mut().import = Some(Import {
            text,
            first,
            n,
            cal,
            from: from.into(),
            path: path.into(),
        });
        self.push(IMPORT);
        self.show_import();
    }

    fn show_import(&self) {
        let m = self.m.borrow();
        let Some(im) = m.import.as_ref() else {
            return;
        };
        let day = im.first.start.div_euclid(DAY);
        let clash: Vec<Instance> = self
            .dt("query", |c| c.event_query(day as i32, day as i32, 0))
            .unwrap_or_default();
        let into = m.cals.get(usize::from(im.cal)).cloned().unwrap_or(CalInfo {
            name: "Personal".into(),
            colour: view::SWATCHES[1],
            count: 0,
        });
        let ui = self.ui();
        view::import(
            &ui.global::<Cal>(),
            &im.from,
            &im.path,
            &im.first,
            im.n,
            &into,
            &clash,
        );
    }

    fn do_import(&self) {
        let Some((text, cal)) = self
            .m
            .borrow()
            .import
            .as_ref()
            .map(|i| (i.text.clone(), i.cal))
        else {
            return;
        };
        match self.dt("import", |c| c.import_ics(text.as_bytes(), cal)) {
            Some((added, clashes)) => {
                dprintln!("calendar: import {}", added);
                self.note(&match (added, clashes) {
                    (0, _) => "Already in Calendar".to_string(),
                    (1, _) => "1 event added".to_string(),
                    (n, _) => format!("{n} events added"),
                });
                let opened = self.m.borrow().opened;
                self.m.borrow_mut().import = None;
                if opened {
                    self.leave();
                } else {
                    self.back();
                }
            }
            None => self.note("Couldn't add the events"),
        }
    }

    /// Save `uid`'s .ics (or everything) through Files' Save.
    fn export(&self, uid: Option<&str>, name: &str) {
        let Some(text) = self.dt("export", |c| c.export_ics(uid)) else {
            self.note("Nothing to export");
            return;
        };
        self.m.borrow_mut().saving = Some(text);
        if let Err(e) = self.app.open(files::KEY, files::SAVE, name) {
            dprintln!("calendar: Files not opened ({})", e);
            self.m.borrow_mut().saving = None;
        }
    }

    // --- forms ---------------------------------------------------------------------------------------------------

    fn new_event(&self, day: i64, all_day: bool) {
        let (p, cal) = {
            let m = self.m.borrow();
            (m.prefs, 0u8)
        };
        let now = self.m.borrow().now;
        // the next whole hour today, 09:00 on another day
        let hour = if day == now.today() {
            (now.local.rem_euclid(DAY) / 3600 + 1).min(23)
        } else {
            9
        };
        let start = if all_day {
            day * DAY
        } else {
            day * DAY + hour * 3600
        };
        let end = if all_day {
            start + DAY
        } else {
            start + i64::from(p.length) * 60
        };
        let d = EventDraft {
            cal,
            all_day,
            start,
            end,
            reminder: Some(if all_day { 900 } else { p.reminder }),
            ..EventDraft::default()
        };
        self.edit_event(d, "New event", Form::Event);
    }

    fn edit_event(&self, d: EventDraft, title: &str, form: Form) {
        {
            let mut m = self.m.borrow_mut();
            m.draft = d;
            m.form = form;
        }
        let ui = self.ui();
        let cal = ui.global::<Cal>();
        let m = self.m.borrow();
        if form == Form::Anniversary {
            let since = ymd(m.draft.start.div_euclid(DAY)).year;
            let next = (ymd(m.now.today()).year - since).max(1) as u16;
            view::anniversary_form(&cal, title, &m.draft, &m.cals, since, next);
        } else {
            view::event_form(&cal, title, &m.draft, &m.cals);
        }
        drop(m);
        self.push(if form == Form::Anniversary {
            ANNIV_FORM
        } else {
            EVENT_FORM
        });
    }

    fn new_anniversary(&self, day: i64) {
        let d = EventDraft {
            kind: Kind::Anniversary,
            all_day: true,
            start: day * DAY,
            end: (day + 1) * DAY,
            repeat: Repeat::Every {
                freq: calendar_core::Freq::Yearly,
                interval: 1,
                days: 0,
                count: 0,
                until: None,
            },
            reminder: Some(900),
            ..EventDraft::default()
        };
        self.edit_event(d, "New anniversary", Form::Anniversary);
    }

    fn edit_todo(&self, t: Todo, title: &str) {
        let now = self.m.borrow().now;
        let list = self
            .m
            .borrow()
            .cals
            .first()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        view::todo_form(&self.ui().global::<Cal>(), title, &t, &now, &list);
        {
            let mut m = self.m.borrow_mut();
            m.todo = t;
            m.form = Form::Todo;
        }
        self.push(TODO_FORM);
    }

    /// The form's rows after a change to the draft.
    fn fields(&self) {
        let ui = self.ui();
        let cal = ui.global::<Cal>();
        let m = self.m.borrow();
        match m.form {
            Form::Todo => {
                let list = m.cals.first().map(|c| c.name.clone()).unwrap_or_default();
                view::todo_fields(&cal, &m.todo, &m.now, &list);
            }
            _ => view::event_fields(&cal, &m.draft, m.cals.get(usize::from(m.draft.cal))),
        }
    }

    fn save(&self) {
        let ui = self.ui();
        let cal = ui.global::<Cal>();
        let title = cal.get_f_title().trim().to_string();
        if title.is_empty() {
            self.note("Add a title");
            return;
        }
        let form = self.m.borrow().form;
        let ok = if form == Form::Todo {
            let mut t = self.m.borrow().todo.clone();
            t.title = title;
            t.notes = cal.get_f_notes().into();
            t.priority = view::priority(cal.get_f_priority());
            self.dt("to-do", |c| c.todo_put(&t)).is_some()
        } else {
            let mut d = self.m.borrow().draft.clone();
            d.title = title;
            d.place = cal.get_f_place().into();
            d.notes = cal.get_f_notes().into();
            self.dt("event", |c| c.event_put(&d)).is_some()
        };
        if ok {
            self.back();
        } else {
            self.note("Couldn't save");
        }
    }

    fn all_day_toggled(&self) {
        let on = self.ui().global::<Cal>().get_f_all_day();
        {
            let mut m = self.m.borrow_mut();
            let len = i64::from(m.prefs.length) * 60;
            let d = &mut m.draft;
            let day = d.start.div_euclid(DAY);
            d.all_day = on;
            if on {
                let last = (d.end - 1).div_euclid(DAY).max(day);
                d.start = day * DAY;
                d.end = (last + 1) * DAY;
            } else {
                d.start = day * DAY + 9 * 3600;
                d.end = d.start + len;
            }
        }
        self.fields();
    }

    fn pick_date(&self, ask: Ask, day: i64) {
        let d = ymd(day);
        let ui = self.ui();
        let cal = ui.global::<Cal>();
        cal.set_dp_year(d.year);
        cal.set_dp_month(i32::from(d.month));
        cal.set_dp_day(i32::from(d.day));
        cal.set_s_date(true);
        self.m.borrow_mut().ask = ask;
    }

    fn pick_time(&self, ask: Ask, t: i64) {
        let s = t.rem_euclid(DAY);
        let ui = self.ui();
        let cal = ui.global::<Cal>();
        cal.set_tp_hour((s / 3600) as i32);
        cal.set_tp_minute((s / 60 % 60) as i32);
        cal.set_s_time(true);
        self.m.borrow_mut().ask = ask;
    }

    pub fn date_set(&self, y: i32, mo: i32, d: i32) {
        let Ok(month) = u8::try_from(mo) else { return };
        let Ok(dd) = u8::try_from(d) else { return };
        let day = Date {
            year: y,
            month,
            day: dd,
        }
        .days();
        let ask = core::mem::replace(&mut self.m.borrow_mut().ask, Ask::None);
        match ask {
            Ask::Starts | Ask::AnniversaryDate => {
                let all_day = {
                    let mut m = self.m.borrow_mut();
                    let dr = &mut m.draft;
                    let len = dr.end - dr.start;
                    dr.start = day * DAY + dr.start.rem_euclid(DAY);
                    dr.end = dr.start + len;
                    dr.all_day
                };
                if !all_day {
                    let t = self.m.borrow().draft.start;
                    self.pick_time(Ask::StartTime, t);
                }
            }
            Ask::Ends => {
                let all_day = {
                    let mut m = self.m.borrow_mut();
                    let dr = &mut m.draft;
                    if dr.all_day {
                        dr.end = (day.max(dr.start.div_euclid(DAY)) + 1) * DAY;
                    } else {
                        dr.end = (day * DAY + dr.end.rem_euclid(DAY)).max(dr.start + 60);
                    }
                    dr.all_day
                };
                if !all_day {
                    let t = self.m.borrow().draft.end;
                    self.pick_time(Ask::EndTime, t);
                }
            }
            Ask::Due => {
                let mut m = self.m.borrow_mut();
                m.todo.due = Some(day * DAY);
                m.todo.due_all_day = true;
            }
            _ => {}
        }
        self.fields();
    }

    pub fn time_set(&self, h: i32, min: i32) {
        let secs = i64::from(h.clamp(0, 23)) * 3600 + i64::from(min.clamp(0, 59)) * 60;
        let ask = core::mem::replace(&mut self.m.borrow_mut().ask, Ask::None);
        {
            let mut m = self.m.borrow_mut();
            let dr = &mut m.draft;
            match ask {
                Ask::StartTime => {
                    let len = dr.end - dr.start;
                    dr.start = dr.start.div_euclid(DAY) * DAY + secs;
                    dr.end = dr.start + len;
                }
                Ask::EndTime => {
                    dr.end = (dr.end.div_euclid(DAY) * DAY + secs).max(dr.start + 60);
                }
                _ => {}
            }
        }
        self.fields();
    }

    fn go_day(&self, day: i64) {
        {
            let mut m = self.m.borrow_mut();
            m.sel = day;
            let d = ymd(day);
            m.year = d.year;
            m.month = d.month;
        }
        self.refresh();
    }

    /// A choice sheet's answer.
    fn chosen(&self, i: i32) {
        let ask = core::mem::replace(&mut self.m.borrow_mut().ask, Ask::None);
        let k = usize::try_from(i).unwrap_or(0);
        match ask {
            Ask::WeekStart => {
                // Monday, Saturday or Sunday
                let ws = [0u8, 5, 6][k.min(2)];
                self.m.borrow_mut().prefs.week_start = ws;
                self.set_pref(key::WEEK_START, Value::Int(i32::from(ws)));
            }
            Ask::OpenOn => {
                let v = k.min(3) as u8;
                self.m.borrow_mut().prefs.view = v;
                self.set_pref(key::VIEW, Value::Int(i32::from(v)));
            }
            Ask::Reminder => {
                let r = view::REMINDERS[(k + 1).min(5)].unwrap_or(0);
                self.m.borrow_mut().prefs.reminder = r;
                self.set_pref(key::REMINDER, Value::Int(i32::from(r)));
            }
            Ask::Length => {
                let l = view::LENGTHS[k.min(view::LENGTHS.len() - 1)];
                self.m.borrow_mut().prefs.length = l;
                self.set_pref(key::LENGTH, Value::Int(i32::from(l)));
            }
            Ask::Calendar => {
                self.m.borrow_mut().draft.cal = k as u8;
                self.fields();
            }
            Ask::ImportCalendar => {
                if let Some(im) = self.m.borrow_mut().import.as_mut() {
                    im.cal = k as u8;
                }
                self.show_import();
            }
            Ask::Since => {
                let year = ymd(self.m.borrow().now.today()).year - i32::try_from(k).unwrap_or(0);
                {
                    let mut m = self.m.borrow_mut();
                    let d = ymd(m.draft.start.div_euclid(DAY));
                    let day = Date {
                        year,
                        month: d.month,
                        day: d.day.min(pim_ics::date::days_in(year, d.month)),
                    }
                    .days();
                    m.draft.start = day * DAY;
                    m.draft.end = (day + 1) * DAY;
                }
                let next = u16::try_from(k).unwrap_or(0).max(1);
                let ui = self.ui();
                let cal = ui.global::<Cal>();
                cal.set_f_since(format!("{year} · shows “{next}” next time").into());
                self.fields();
            }
            _ => {}
        }
        self.render();
    }

    fn choose(&self, ask: Ask, title: &str, choices: &[String], current: usize) {
        view::choose(&self.ui().global::<Cal>(), title, choices, current);
        self.m.borrow_mut().ask = ask;
    }

    fn choose_calendar(&self, ask: Ask, current: u8) {
        let names: Vec<String> = self
            .m
            .borrow()
            .cals
            .iter()
            .map(|c| c.name.clone())
            .collect();
        self.choose(ask, "Calendar", &names, usize::from(current));
    }

    /// A shell dialog's answer.
    pub fn answer(&self, i: u16) {
        let ask = core::mem::replace(&mut self.m.borrow_mut().ask, Ask::None);
        let (event, series) = {
            let m = self.m.borrow();
            (m.event.clone(), m.series.clone())
        };
        match (ask, i) {
            // This event / All events
            (Ask::EditWhich, 0 | 1) => {
                let only = (i == 0).then_some(event.recur);
                match self.dt("event", |c| c.event_get(&event.uid, only)) {
                    Some(d) => self.edit_event(d, "Edit event", form_of(&series)),
                    None => self.note("Event not found"),
                }
            }
            (Ask::DeleteWhich, _) => {
                let repeats = event.repeats;
                let only = match (repeats, i) {
                    (true, 0) => Some(event.recur),
                    (true, 1) | (false, 0) => None,
                    _ => return,
                };
                if self
                    .dt("delete", |c| c.event_del(&event.uid, only))
                    .is_some()
                {
                    self.note("Event deleted");
                    self.back();
                }
            }
            _ => {}
        }
    }

    fn ask(&self, ask: Ask, q: &Query) {
        self.m.borrow_mut().ask = ask;
        if let Err(e) = self.app.query(q) {
            dprintln!("calendar: dialog not shown ({})", e);
        }
    }

    // --- to-dos --------------------------------------------------------------------------------------------------

    fn toggle(&self, k: usize) {
        let Some(mut t) = self.m.borrow().todos.get(k).cloned() else {
            return;
        };
        let before = t.clone();
        t.done = !t.done;
        t.completed = t.done.then(|| e32::time::utc_now().0);
        if self.dt("to-do", |c| c.todo_put(&t)).is_some() {
            self.m.borrow_mut().undo = Some(before);
            let text = if t.done {
                format!("“{}” done", t.title)
            } else {
                format!("“{}” not done", t.title)
            };
            if let Err(e) = self.app.note(&text, "Undo") {
                dprintln!("calendar: note not shown ({})", e);
            }
        }
    }

    pub fn undo(&self) {
        if let Some(t) = self.m.borrow_mut().undo.take() {
            let _ = self.dt("to-do", |c| c.todo_put(&t));
        }
    }

    fn clear_done(&self) {
        let done: Vec<String> = self
            .m
            .borrow()
            .todos
            .iter()
            .filter(|t| t.done)
            .map(|t| t.uid.clone())
            .collect();
        for uid in &done {
            let _ = self.dt("to-do", |c| c.todo_del(uid));
        }
        self.m.borrow_mut().show_done = false;
        self.note(&format!("{} cleared", done.len()));
    }

    // --- search --------------------------------------------------------------------------------------------------

    fn open_search(&self) {
        let (today, hidden) = {
            let m = self.m.borrow();
            (m.now.today(), m.prefs.hidden)
        };
        // a year back and ahead (DTSRV's range limit is 1000 days)
        let items = self.dt("query", |c| {
            c.event_query((today - 365) as i32, (today + 365) as i32, hidden)
        });
        self.m.borrow_mut().search = items.unwrap_or_default();
        self.push(SEARCH);
        self.searched("");
    }

    pub fn searched(&self, q: &str) {
        let ui = self.ui();
        let cal = ui.global::<Cal>();
        let filter = cal.get_filter();
        let found = {
            let m = self.m.borrow();
            view::search(&cal, &m.now, q, filter, &m.search, &m.todos, &m.cals)
        };
        self.m.borrow_mut().found = found;
    }

    // --- the taps ------------------------------------------------------------------------------------------------

    pub fn act(&self, what: &str, i: i32) {
        let k = usize::try_from(i).unwrap_or(usize::MAX);
        let (view, sel, today) = {
            let m = self.m.borrow();
            (m.view, m.sel, m.now.today())
        };
        let ui = self.ui();
        let cal = ui.global::<Cal>();
        match what {
            "back" => self.back(),
            "tab" => self.tab(i.clamp(0, 3)),
            "today" => {
                self.go_day(today);
            }
            "add" => {
                if view == TODO {
                    self.edit_todo(
                        Todo {
                            due: Some(today * DAY),
                            due_all_day: true,
                            ..Todo::default()
                        },
                        "New to-do",
                    );
                } else {
                    view::new_sheet(&cal, sel);
                }
            }
            "new" => match i {
                0 => self.new_event(sel, false),
                1 => self.new_event(sel, true),
                2 => self.new_anniversary(sel),
                _ => self.edit_todo(
                    Todo {
                        due: Some(sel * DAY),
                        due_all_day: true,
                        ..Todo::default()
                    },
                    "New to-do",
                ),
            },
            "menu" => match i {
                0 => self.open_search(),
                1 => {
                    let m = self.m.borrow();
                    view::month_sheet(&cal, m.year, m.month);
                }
                2 => {
                    self.m.borrow_mut().cal_edit = None;
                    self.push(CALENDARS);
                    self.render();
                }
                3 => {
                    self.m.borrow_mut().picking = true;
                    if let Err(e) = self.app.open(files::KEY, files::PICK, "ics") {
                        dprintln!("calendar: Files not opened ({})", e);
                        self.m.borrow_mut().picking = false;
                    }
                }
                4 => self.export(None, "calendar.ics"),
                _ => {
                    self.push(SETTINGS);
                    self.render();
                }
            },
            "pick-month" => {
                let m = self.m.borrow();
                view::month_sheet(&cal, m.year, m.month);
            }
            "go-month" => {
                let (y, mo) = (cal.get_pick_year(), cal.get_pick_month());
                let month = u8::try_from(mo).unwrap_or(1).clamp(1, 12);
                let first = Date {
                    year: y,
                    month,
                    day: 1,
                }
                .days();
                let day = if ymd(today).year == y && ymd(today).month == month {
                    today
                } else {
                    first
                };
                self.go_day(day);
            }
            "prev" | "next" => {
                let s: i64 = if what == "prev" { -1 } else { 1 };
                match view {
                    MONTH => {
                        let mut m = self.m.borrow_mut();
                        let n = i64::from(m.year) * 12 + i64::from(m.month) - 1 + s;
                        m.year = n.div_euclid(12) as i32;
                        m.month = (n.rem_euclid(12) + 1) as u8;
                        m.sel = if ymd(today).year == m.year && ymd(today).month == m.month {
                            today
                        } else {
                            Date {
                                year: m.year,
                                month: m.month,
                                day: 1,
                            }
                            .days()
                        };
                        drop(m);
                        self.refresh();
                    }
                    _ => {
                        // Week's arrows and Day's strip swipe move a week
                        self.m.borrow_mut().sel += s * 7;
                        self.refresh();
                    }
                }
            }
            // a month cell; a second tap on the selected day opens it
            "day" => {
                let (ws, year, month) = {
                    let m = self.m.borrow();
                    (m.prefs.week_start, m.year, m.month)
                };
                let day = view::grid_start(year, month, ws) + i64::from(i);
                if day == sel {
                    self.tab(DAY_VIEW);
                } else {
                    self.m.borrow_mut().sel = day;
                    self.render();
                }
            }
            "open-day" => self.tab(DAY_VIEW),
            "week-day" => {
                let ws = self.m.borrow().prefs.week_start;
                self.m.borrow_mut().sel = view::week_of(sel, ws) + i64::from(i);
                self.tab(DAY_VIEW);
            }
            "strip-day" => {
                let ws = self.m.borrow().prefs.week_start;
                self.m.borrow_mut().sel = view::week_of(sel, ws) + i64::from(i);
                self.render();
            }
            "event" => {
                let item = self.m.borrow().items.get(k).cloned();
                if let Some(it) = item {
                    self.open_instance(it);
                }
            }
            "pin" => {
                if let Err(e) = self
                    .app
                    .open(other::CLOCK, datetime_core::pages::clock::ALARMS, "")
                {
                    dprintln!("calendar: Clock not opened ({})", e);
                }
            }
            "todo" => {
                let t = self.m.borrow().todos.get(k).cloned();
                if let Some(t) = t {
                    self.edit_todo(t, "Edit to-do");
                }
            }
            "toggle" => self.toggle(k),
            "show-done" => {
                self.m.borrow_mut().show_done = true;
                self.render();
            }
            "clear-done" => self.clear_done(),
            // the event page
            "edit" => {
                let (event, series) = {
                    let m = self.m.borrow();
                    (m.event.clone(), m.series.clone())
                };
                if event.repeats {
                    self.ask(
                        Ask::EditWhich,
                        &Query {
                            text: "Change this event or all of the series?".into(),
                            buttons: vec![
                                "This event".into(),
                                "All events".into(),
                                "Cancel".into(),
                            ],
                            destructive: false,
                        },
                    );
                } else {
                    self.edit_event(series.clone(), "Edit event", form_of(&series));
                }
            }
            "duplicate" => {
                let mut d = self.m.borrow().series.clone();
                d.uid.clear();
                d.only = None;
                self.edit_event(d.clone(), "New event", form_of(&d));
            }
            "delete" => {
                let repeats = self.m.borrow().event.repeats;
                let buttons = if repeats {
                    vec![
                        "Delete this event".into(),
                        "Delete all events".into(),
                        "Cancel".into(),
                    ]
                } else {
                    vec!["Delete".into(), "Cancel".into()]
                };
                self.ask(
                    Ask::DeleteWhich,
                    &Query {
                        text: "Delete this event?".into(),
                        buttons,
                        destructive: true,
                    },
                );
            }
            "share-open" => {
                let e = self.m.borrow().event.clone();
                cal.set_share_text(view::share_text(&e).into());
                cal.set_s_share(true);
            }
            "share" => {
                let e = self.m.borrow().event.clone();
                let text = view::share_text(&e);
                match i {
                    0 => {
                        if let Err(err) = self.app.open(other::MESSAGES, other::NEW_MESSAGE, &text)
                        {
                            dprintln!("calendar: Messages not opened ({})", err);
                        }
                    }
                    1 => {
                        let name = format!("{}.ics", file_name(&e.title));
                        self.export(Some(&e.uid), &name);
                    }
                    _ => {
                        if self.app.set_clipboard(&text) {
                            self.note("Copied");
                        }
                    }
                }
            }
            // the forms
            "save" => self.save(),
            "all-day" => self.all_day_toggled(),
            "starts" => {
                let t = self.m.borrow().draft.start;
                self.pick_date(Ask::Starts, t.div_euclid(DAY));
            }
            "ends" => {
                let d = self.m.borrow().draft.clone();
                let last = if d.all_day { d.end - DAY } else { d.end };
                self.pick_date(Ask::Ends, last.div_euclid(DAY));
            }
            "date" => {
                let t = self.m.borrow().draft.start;
                self.pick_date(Ask::AnniversaryDate, t.div_euclid(DAY));
            }
            "due" => {
                let t = self.m.borrow().todo.due.unwrap_or(today * DAY);
                self.pick_date(Ask::Due, t.div_euclid(DAY));
            }
            "since" => {
                let y = ymd(today).year;
                let years: Vec<String> = (0..80).map(|k| (y - k).to_string()).collect();
                let start = ymd(self.m.borrow().draft.start.div_euclid(DAY)).year;
                self.choose(
                    Ask::Since,
                    "Since",
                    &years,
                    usize::try_from(y - start).unwrap_or(0),
                );
            }
            "repeat" => {
                let d = self.m.borrow().draft.clone();
                view::repeat_sheet(&cal, &d.repeat, d.start);
                cal.set_s_repeat(true);
            }
            "repeat-set" => {
                let start = self.m.borrow().draft.start;
                if let Some((_, r)) = view::repeat_choices(start).into_iter().nth(k) {
                    self.m.borrow_mut().draft.repeat = r;
                }
                self.fields();
            }
            "reminder" => {
                let (form, d, t, ring) = {
                    let m = self.m.borrow();
                    (m.form, m.draft.clone(), m.todo.clone(), m.prefs.ring)
                };
                match form {
                    Form::Todo => view::reminder_sheet(&cal, t.reminder, t.due_all_day, ring),
                    _ => view::reminder_sheet(&cal, d.reminder, d.all_day, ring),
                }
                cal.set_s_reminder(true);
            }
            "reminder-set" => {
                let ring = cal.get_reminder_ring();
                {
                    let mut m = self.m.borrow_mut();
                    if m.form == Form::Todo {
                        let all_day = m.todo.due_all_day;
                        m.todo.reminder = view::reminder_at(k, all_day);
                    } else {
                        let all_day = m.draft.all_day;
                        m.draft.reminder = view::reminder_at(k, all_day);
                    }
                    m.prefs.ring = ring;
                }
                self.set_pref(key::RING, Value::Bool(ring));
                self.fields();
            }
            "calendar" => {
                if view == IMPORT {
                    let c = self.m.borrow().import.as_ref().map_or(0, |i| i.cal);
                    self.choose_calendar(Ask::ImportCalendar, c);
                } else {
                    let c = self.m.borrow().draft.cal;
                    self.choose_calendar(Ask::Calendar, c);
                }
            }
            "list" => {}
            "chosen" => self.chosen(i),
            // the birthday page: Phone, Messages and Contacts
            // shortcut: their pages take a number, not a contact id (NCT2 has no id page); they open on their
            // dialer, new message and list until phone gets a page by contact id
            "call" => self.open_other(other::PHONE, other::DIAL),
            "message" => self.open_other(other::MESSAGES, other::NEW_MESSAGE),
            "contact" => self.open_other(other::CONTACTS, other::CONTACT_LIST),
            "bd-row" => {
                let gift = {
                    let m = self.m.borrow();
                    let first = m
                        .event
                        .title
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string();
                    m.todos
                        .iter()
                        .find(|t| !first.is_empty() && t.title.contains(&first))
                        .cloned()
                };
                if let (0, Some(t)) = (i, gift) {
                    self.edit_todo(t, "Edit to-do");
                }
            }
            // search
            "filter" => {
                let q: SharedString = cal.get_query();
                self.searched(&q);
            }
            "hit" => {
                let f = self.m.borrow().found.get(k).copied();
                match f {
                    Some(Found::Event(n)) => {
                        let it = self.m.borrow().search.get(n).cloned();
                        if let Some(it) = it {
                            self.open_instance(it);
                        }
                    }
                    Some(Found::Todo(n)) => {
                        let t = self.m.borrow().todos.get(n).cloned();
                        if let Some(t) = t {
                            self.edit_todo(t, "Edit to-do");
                        }
                    }
                    None => {}
                }
            }
            // settings
            "set-week-start" => {
                let ws = self.m.borrow().prefs.week_start;
                let days = ["Monday".to_string(), "Saturday".into(), "Sunday".into()];
                let cur = match ws {
                    5 => 1,
                    6 => 2,
                    _ => 0,
                };
                self.choose(Ask::WeekStart, "Week starts on", &days, cur);
            }
            "set-open-on" => {
                let v = self.m.borrow().prefs.view;
                let names: Vec<String> = view::VIEWS.iter().map(|s| (*s).into()).collect();
                self.choose(Ask::OpenOn, "Open on", &names, usize::from(v));
            }
            "set-reminder" => {
                let r = self.m.borrow().prefs.reminder;
                let names: Vec<String> = view::REMINDERS[1..]
                    .iter()
                    .map(|m| view::reminder_text(*m, false))
                    .collect();
                let cur = view::REMINDERS[1..]
                    .iter()
                    .position(|m| *m == Some(r))
                    .unwrap_or(0);
                // a default always reminds: the sheet starts at "At the start" (REMINDERS[1])
                self.choose(Ask::Reminder, "Default reminder", &names, cur);
            }
            "set-length" => {
                let l = self.m.borrow().prefs.length;
                let names: Vec<String> = view::LENGTHS
                    .iter()
                    .map(|m| view::length_text(*m))
                    .collect();
                let cur = view::LENGTHS.iter().position(|m| *m == l).unwrap_or(2);
                self.choose(Ask::Length, "Default length", &names, cur);
            }
            "set-week-numbers" => {
                self.m.borrow_mut().prefs.week_numbers = i != 0;
                self.set_pref(key::WEEK_NUMBERS, Value::Bool(i != 0));
            }
            "set-ring" => {
                self.m.borrow_mut().prefs.ring = i != 0;
                self.set_pref(key::RING, Value::Bool(i != 0));
            }
            "set-birthdays" => {
                self.m.borrow_mut().prefs.birthdays = i != 0;
                self.set_pref(key::BIRTHDAYS, Value::Bool(i != 0));
            }
            "set-zone" => {}
            // Calendars
            "cal" => {
                let n = self.m.borrow().cals.len();
                self.m.borrow_mut().cal_edit = (k < n).then_some(k);
                self.render();
            }
            "cal-shown" => {
                let n = self.m.borrow().cals.len();
                if k == n {
                    let on = !self.m.borrow().prefs.birthdays;
                    self.m.borrow_mut().prefs.birthdays = on;
                    self.set_pref(key::BIRTHDAYS, Value::Bool(on));
                } else if k < 32 {
                    let hidden = self.m.borrow().prefs.hidden ^ (1 << k);
                    self.m.borrow_mut().prefs.hidden = hidden;
                    self.set_pref(key::HIDDEN, Value::Bytes(hidden.to_le_bytes().to_vec()));
                }
                self.render();
            }
            "swatch" => {
                let edit = self.m.borrow().cal_edit;
                let c = edit.and_then(|e| self.m.borrow().cals.get(e).cloned());
                if let (Some(e), Some(c)) = (edit, c) {
                    let colour = view::SWATCHES[k.min(view::SWATCHES.len() - 1)];
                    let _ = self.dt("calendar", |d| {
                        d.calendar_put(Some(e as u8), &c.name, colour)
                    });
                    self.refresh();
                }
            }
            "cal-add" => {
                let n = self.m.borrow().cals.len();
                let colour = view::SWATCHES[n % view::SWATCHES.len()];
                let name = format!("Calendar {}", n + 1);
                if let Some(i) = self.dt("calendar", |d| d.calendar_put(None, &name, colour)) {
                    self.m.borrow_mut().cal_edit = Some(usize::from(i));
                }
                self.refresh();
            }
            // Add to Calendar
            "import" => self.do_import(),
            // Ringing
            "snooze" => {
                self.ring_act(dt::action::SNOOZE);
                self.leave();
            }
            "dismiss" => {
                self.ring_act(dt::action::DISMISS);
                self.leave();
            }
            "ring-open" => {
                self.ring_act(dt::action::DISMISS);
                let uid = self.m.borrow().event.uid.clone();
                {
                    let mut m = self.m.borrow_mut();
                    m.opened = false;
                    m.ring = None;
                }
                self.tab(MONTH);
                self.open_uid(&uid);
            }
            _ => dprintln!("calendar: {} {}?", what, i),
        }
    }

    fn open_other(&self, key: &str, page: u16) {
        if let Err(e) = self.app.open(key, page, "") {
            dprintln!("calendar: {} not opened ({})", key, e);
        }
    }

    /// An occurrence's page: a birthday's, or the event with its series.
    fn open_instance(&self, it: Instance) {
        if it.kind == Kind::Birthday {
            let (now, todos) = {
                let m = self.m.borrow();
                (m.now, m.todos.clone())
            };
            let first = it.title.split_whitespace().next().unwrap_or("").to_string();
            let linked: Vec<Todo> = todos
                .into_iter()
                .filter(|t| !t.done && !first.is_empty() && t.title.contains(&first))
                .collect();
            // the birth year from the age shown
            let born =
                (it.years > 0).then(|| ymd(it.start.div_euclid(DAY)).year - i32::from(it.years));
            view::birthday(
                &self.ui().global::<Cal>(),
                &now,
                &it,
                born,
                &linked,
                "9:00 the day before",
            );
            self.m.borrow_mut().event = it;
            self.push(BIRTHDAY);
            return;
        }
        let series = self
            .dt("event", |c| c.event_get(&it.uid, None))
            .unwrap_or_else(|| EventDraft {
                uid: it.uid.clone(),
                cal: it.cal,
                kind: it.kind,
                all_day: it.all_day,
                start: it.start,
                end: it.end,
                title: it.title.clone(),
                place: it.place.clone(),
                ..EventDraft::default()
            });
        self.show_event(it, series);
    }

    pub fn clear_data(&self) {
        let r = self.dt("clear", |c| c.clear(dt::mask::EVENTS | dt::mask::TODOS));
        let (mut n, mut refused) = (0, 0);
        for spec in neoui_proto::settings::registry::cleared_with("calendar") {
            n += 1;
            if let Err(e) = self
                .app
                .set_setting_raw(spec.name, &spec.default.to_value())
            {
                dprintln!("calendar: clearing {} refused ({})", spec.name, e);
                refused += 1;
            }
        }
        dprintln!(
            "calendar: data cleared (stores {}, {} keys, {} refused)",
            r.is_some(),
            n,
            refused
        );
        dprintln!("calendar: quit");
    }

    fn note(&self, text: &str) {
        if let Err(e) = self.app.note(text, "") {
            dprintln!("calendar: note not shown ({})", e);
        }
    }

    /// Tell the UI server what shows (protocol 1.8), ask for the keyboard a field wants, the badge, and draw.
    pub fn sync(&self) -> Result<(), i32> {
        let ui = self.ui();
        let view = self.m.borrow().view;
        let layers = ui.global::<Kit>().get_layers() > 0;
        let page = Page {
            title: "Calendar".into(),
            can_back: layers || view != MONTH || self.m.borrow().opened,
            awake: view == RING,
            ..Page::default()
        };
        if self.shown.borrow().as_ref() != Some(&page) {
            if let Err(e) = self.app.page(&page) {
                dprintln!("calendar: page not set ({})", e);
            }
            *self.shown.borrow_mut() = Some(page);
        }
        let kf = ui.global::<KeyboardFocus>();
        let want = (kf.get_open(), kf.get_seq(), kf.get_mode());
        let had = self.keyboard.get();
        if want.0 != had.0 || (want.0 && (want.1, want.2) != (had.1, had.2)) {
            self.keyboard.set(want);
            let mode = if want.0 {
                want.2.max(1)
            } else {
                proto::KEYBOARD_HIDDEN
            };
            let h = self.app.keyboard(mode).unwrap_or(0);
            kf.set_text_h(h as f32);
        }
        let badge = u32::try_from(ui.global::<Cal>().get_todo_badge()).unwrap_or(0);
        if self.badge.get() != badge {
            match self.app.badge(0, badge) {
                Ok(()) => self.badge.set(badge),
                Err(e) => dprintln!("calendar: badge not set ({})", e),
            }
        }
        self.app.draw()
    }
}

fn form_of(d: &EventDraft) -> Form {
    if d.kind == Kind::Anniversary {
        Form::Anniversary
    } else {
        Form::Event
    }
}

fn view_name(v: i32) -> &'static str {
    match v {
        MONTH => "month",
        WEEK => "week",
        DAY_VIEW => "day",
        TODO => "todo",
        EVENT => "event",
        EVENT_FORM => "event-edit",
        TODO_FORM => "todo-edit",
        ANNIV_FORM => "anniversary-edit",
        BIRTHDAY => "birthday",
        SEARCH => "search",
        SETTINGS => "settings",
        CALENDARS => "calendars",
        IMPORT => "import",
        RING => "ring",
        _ => "?",
    }
}

/// A file name from an event's title: letters, digits, spaces and dashes.
fn file_name(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let s = s.trim();
    if s.is_empty() {
        "event".into()
    } else {
        s.into()
    }
}

/// The largest .ics NEOCAL reads (DTSRV's import limit).
const ICS_MAX: usize = 256 << 10;

fn read_all(f: &File<'_>) -> Result<Vec<u8>, i32> {
    let size = usize::try_from(f.size()?).unwrap_or(0);
    if size > ICS_MAX {
        return Err(-9); // KErrOverflow
    }
    let mut buf = vec![0u8; size];
    let mut at = 0;
    while at < size {
        let n = f.read_at(at as i32, &mut buf[at..])?;
        if n == 0 {
            break;
        }
        at += n;
    }
    buf.truncate(at);
    Ok(buf)
}

/// A file by its full path (Files' Add to Calendar opens page IMPORT with one).
fn read_path(path: &str) -> Result<Vec<u8>, i32> {
    let fs = Fs::connect()?;
    let f = File::open(&fs, path, mode::READ | mode::SHARE_READERS_ONLY)?;
    read_all(&f)
}

/// Every tap of the screens goes to [`Ctx::act`]; the pickers and search to theirs.
pub fn bind(ctx: &alloc::rc::Rc<Ctx>, ui: &CalendarApp) {
    let cal = ui.global::<Cal>();
    let weak = alloc::rc::Rc::downgrade(ctx);
    let c = weak.clone();
    cal.on_act(move |what, i| {
        if let Some(ctx) = c.upgrade() {
            ctx.act(&what, i);
        }
    });
    let c = weak.clone();
    cal.on_date_set(move |y, m, d| {
        if let Some(ctx) = c.upgrade() {
            ctx.date_set(y, m, d);
        }
    });
    let c = weak.clone();
    cal.on_time_set(move |h, m| {
        if let Some(ctx) = c.upgrade() {
            ctx.time_set(h, m);
        }
    });
    let c = weak;
    cal.on_searched(move |q| {
        if let Some(ctx) = c.upgrade() {
            ctx.searched(&q);
        }
    });
}
