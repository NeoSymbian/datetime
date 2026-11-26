// SPDX-License-Identifier: GPL-3.0-only
//! What NEOCLOCK keeps and shares between its callbacks: the connection to the UI server, DTSRV's client, the model
//! (its own settings and the copies of DTSRV's alarms and timer), and the moves every screen makes (open a view,
//! describe the page, ask for the keyboard, draw).

use alloc::string::String;
use core::cell::{Cell, RefCell};
use datetime_core::alarm::{Alarm, Alarms};
use datetime_core::next::Next;
use datetime_core::stopwatch::Stopwatch;
use datetime_core::timer::{DEFAULT_MS, Timer};
use dt_client::Client;
use e32::{dprintln, time};
use neoapp::App;
use neoui_proto::clock::Local;
use neoui_proto::clock::face::LocalTime;
use neoui_proto::clock::world::Cities;
use neoui_proto::settings::{Bytes, keys};
use slint::ComponentHandle;

use clock_ui::{ClockApp, KeyboardFocus, Kit};

/// The time a refresh shows.
#[derive(Clone, Copy, Debug)]
pub struct Now {
    pub local: LocalTime,
    /// UTC, Unix seconds.
    pub utc: i64,
    /// The local zone, seconds east of UTC.
    pub tz: i32,
    /// UTC, Unix milliseconds (the timer and the stopwatch).
    pub wall_ms: i64,
    pub second: u32,
}

impl Now {
    pub fn read() -> Self {
        let t = time::now();
        let (utc, tz) = time::utc_now();
        Self {
            local: LocalTime {
                year: t.year,
                month: t.month,
                day: t.day,
                weekday: t.weekday,
                hour: t.hour,
                minute: t.minute,
            },
            utc,
            tz,
            wall_ms: neoapp::wall_ms(),
            second: t.second,
        }
    }
}

/// The footer tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Clock = 0,
    Alarms = 1,
    Timers = 2,
}

/// What covers the tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    None,
    Alarm,
    Sound,
    Cities,
    SetTime,
    Settings,
    Ringing,
}

impl View {
    /// The name `ClockState.view` uses (and the log: "clock: view settings").
    pub fn name(self) -> &'static str {
        match self {
            View::None => "",
            View::Alarm => "alarm",
            View::Sound => "sound",
            View::Cities => "cities",
            View::SetTime => "set-time",
            View::Settings => "settings",
            View::Ringing => "ringing",
        }
    }
}

/// An alarm being edited: a copy of an existing one (`id` > 0) or a new one (0).
pub struct Draft(pub Alarm);

pub struct Model {
    pub tab: Tab,
    /// The Timers tab's switch: false Timer, true Stopwatch.
    pub stopwatch_shown: bool,
    pub view: View,
    /// Where Alarm sound goes back to (the editor or Settings).
    pub sound_from: View,
    // the settings (clock.*)
    pub h24: bool,
    pub analog: bool,
    pub seconds: bool,
    pub date: bool,
    pub snooze_min: i32,
    pub snooze_count: i32,
    pub sound: String,
    pub shade_next: bool,
    pub cities: Cities,
    /// The shell's zone city (`time.zone`, "" = none chosen).
    pub zone_name: String,
    pub stopwatch: Stopwatch,
    // DTSRV's, read on every WATCH
    pub alarms: Alarms,
    pub timer: Timer,
    pub next: Next,
    /// The idle timer's duration as the steppers set it (DTSRV gets it on Start).
    pub timer_ms: u32,
    pub draft: Option<Draft>,
    /// Set time: the fields, and the catalogue city whose zone they use.
    pub set_time: Local,
    pub zone: usize,
    /// The ring Ringing shows (`dt_client::ring` id), and the view under it.
    pub ring: Option<u32>,
    pub before_ring: View,
    /// Add city's search.
    pub query: String,
    /// The city a long-press asked to remove (the shell's dialog answers).
    pub removing: Option<usize>,
    pub visible: bool,
}

impl Model {
    /// The settings; the alarms and the timer come from DTSRV ([`Ctx::reload`]).
    pub fn load(app: &App) -> Self {
        // clock.tab keeps the old numbering: 0 Clock, 1 Alarms, (2 World,) 3 Timer, 4 Stopwatch
        let tab = app.setting(keys::CLOCK_TAB).unwrap_or(0);
        let m = Self {
            tab: match tab {
                1 => Tab::Alarms,
                3 | 4 => Tab::Timers,
                _ => Tab::Clock,
            },
            stopwatch_shown: tab == 4,
            view: View::None,
            sound_from: View::Settings,
            h24: app.setting(keys::CLOCK_24H).unwrap_or(true),
            analog: app.setting(keys::CLOCK_FACE).unwrap_or(0) == 1,
            seconds: app.setting(keys::CLOCK_SECONDS).unwrap_or(true),
            date: app.setting(keys::CLOCK_DATE).unwrap_or(true),
            snooze_min: app.setting(keys::CLOCK_SNOOZE).unwrap_or(5),
            snooze_count: app.setting(keys::CLOCK_SNOOZE_COUNT).unwrap_or(3),
            sound: app
                .setting(keys::CLOCK_SOUND)
                .unwrap_or_else(|_| "Dawn".into()),
            shade_next: app.setting(keys::CLOCK_SHADE_NEXT).unwrap_or(true),
            cities: blob(app, keys::CLOCK_CITIES, Cities::decode).unwrap_or_default(),
            zone_name: match app.setting_raw("time.zone") {
                Ok(neoui_proto::settings::Value::Str(s)) => s,
                _ => String::new(),
            },
            stopwatch: blob(app, keys::CLOCK_STOPWATCH, Stopwatch::decode).unwrap_or_default(),
            alarms: Alarms::new(),
            timer: Timer::default(),
            next: Next::default(),
            timer_ms: DEFAULT_MS,
            draft: None,
            set_time: Local {
                year: 2026,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                tz: 0,
            },
            zone: 0,
            ring: None,
            before_ring: View::None,
            query: String::new(),
            removing: None,
            visible: true,
        };
        dprintln!(
            "clock: state loaded (tab {}, {} cities, stopwatch running {})",
            tab,
            m.cities.as_slice().len(),
            m.stopwatch.is_running()
        );
        m
    }

    /// The settings others may change (Settings → Clock, Date & time's 24-hour).
    pub fn reload_settings(&mut self, app: &App) {
        let fresh = Self::load(app);
        self.h24 = fresh.h24;
        self.analog = fresh.analog;
        self.seconds = fresh.seconds;
        self.date = fresh.date;
        self.snooze_min = fresh.snooze_min;
        self.snooze_count = fresh.snooze_count;
        self.sound = fresh.sound;
        self.shade_next = fresh.shade_next;
        self.cities = fresh.cities;
        self.zone_name = fresh.zone_name;
    }

    /// `clock.tab` as saved.
    pub fn tab_number(&self) -> i32 {
        match (self.tab, self.stopwatch_shown) {
            (Tab::Clock, _) => 0,
            (Tab::Alarms, _) => 1,
            (Tab::Timers, false) => 3,
            (Tab::Timers, true) => 4,
        }
    }
}

/// A saved blob decoded: `None` when unset, unreadable or damaged (logged).
fn blob<T, E: core::fmt::Display>(
    app: &App,
    key: neoui_proto::settings::Key<Bytes>,
    decode: fn(&[u8]) -> Result<T, E>,
) -> Option<T> {
    let bytes = match app.setting(key) {
        Ok(Bytes(b)) => b,
        Err(e) => {
            dprintln!("clock: cannot read {} ({})", key.name(), e);
            return None;
        }
    };
    if bytes.is_empty() {
        return None;
    }
    decode(&bytes)
        .map_err(|e| dprintln!("clock: {} unreadable ({}), starting fresh", key.name(), e))
        .ok()
}

/// The analog face's dial for one size and ink, drawn once (the hands change every second, the dial doesn't).
#[derive(Default)]
pub struct Analog {
    pub dial: alloc::vec::Vec<datetime_core::analog::Rgba>,
    pub size: usize,
    pub ink: [u8; 3],
}

pub struct Ctx {
    pub app: App,
    pub ui: slint::Weak<ClockApp>,
    pub m: RefCell<Model>,
    /// DTSRV's session; `None` until it answers (connected again on the next use).
    pub dt: RefCell<Option<Client>>,
    pub analog: RefCell<Analog>,
    /// The ring last drawn: (fraction in 1/1000, done) — redrawn only when it moves.
    pub ring_drawn: Cell<(i32, bool)>,
    /// The page last described to the shell.
    page: RefCell<Option<neoapp::Page>>,
    /// The keyboard last asked for: (open, field seq, mode).
    keyboard: Cell<(bool, i32, i32)>,
}

impl Ctx {
    pub fn new(app: App, ui: &ClockApp, m: Model) -> Self {
        Self {
            app,
            ui: ui.as_weak(),
            m: RefCell::new(m),
            dt: RefCell::new(None),
            analog: RefCell::default(),
            ring_drawn: Cell::new((-1, false)),
            page: RefCell::new(None),
            keyboard: Cell::new((false, 0, 0)),
        }
    }

    pub fn ui(&self) -> ClockApp {
        // the window lives as long as the app
        self.ui.upgrade().expect("the window")
    }

    /// Runs `f` on DTSRV's session, connecting first if needed; a dead session is dropped (the next call connects
    /// again).
    pub fn dt<T>(&self, what: &str, f: impl FnOnce(&Client) -> Result<T, i32>) -> Result<T, i32> {
        let mut slot = self.dt.borrow_mut();
        if slot.is_none() {
            *slot = Some(Client::connect().inspect_err(|e| {
                dprintln!("clock: DTSRV not there for {} ({})", what, e);
            })?);
        }
        let r = f(slot.as_ref().expect("connected above"));
        if let Err(e) = r {
            dprintln!("clock: DTSRV {} failed ({})", what, e);
            // KErrServerTerminated, KErrDied, KErrNotFound: connect again next time
            if matches!(e, -15 | -13 | -1) {
                *slot = None;
            }
        }
        r
    }

    /// Reads DTSRV's alarms, timer and next event again.
    pub fn reload(&self) {
        let alarms = self.dt("ALARM_LIST", Client::alarms);
        let timer = self.dt("TIMER", |c| c.timer(dt_client::timer::GET, 0));
        let next = self.dt("NEXT", Client::next);
        let mut m = self.m.borrow_mut();
        if let Ok(a) = alarms {
            m.alarms = a;
        }
        if let Ok(t) = timer {
            // the steppers start from the duration DTSRV last ran (a draft being set stays)
            if t.set_ms != m.timer.set_ms {
                m.timer_ms = t.set_ms;
            }
            m.timer = t;
        }
        if let Ok(n) = next {
            m.next = n;
        }
    }

    /// Saves a setting (a refusal is logged; the model keeps the value).
    pub fn save<T: neoui_proto::settings::Setting>(
        &self,
        key: neoui_proto::settings::Key<T>,
        value: T,
    ) {
        if let Err(e) = self.app.set_setting(key, value) {
            dprintln!("clock: saving {} failed ({})", key.name(), e);
        }
    }

    pub fn save_stopwatch(&self) {
        let b = self.m.borrow().stopwatch.encode();
        self.save(keys::CLOCK_STOPWATCH, Bytes(b));
    }

    pub fn save_cities(&self) {
        let b = self.m.borrow().cities.encode();
        self.save(keys::CLOCK_CITIES, Bytes(b));
    }

    /// Opens a view over the tabs ([`View::None`] closes it).
    pub fn open(&self, view: View) {
        self.m.borrow_mut().view = view;
        dprintln!("clock: view {}", view.name());
        crate::show::all(self, &Now::read());
    }

    /// Tells the shell the page when it changed: Back stays in the app while a view or a kit layer is open, or on
    /// the Alarms and Timers tabs (their Back goes to the Clock tab first, dt2-spec-map); Ringing is full screen and
    /// keeps the screen on.
    pub fn describe(&self) {
        let layers = self.ui().global::<Kit>().get_layers();
        let (view, tab) = {
            let m = self.m.borrow();
            (m.view, m.tab)
        };
        let page = neoapp::Page {
            title: "Clock".into(),
            can_back: view != View::None || layers > 0 || tab != Tab::Clock,
            full: view == View::Ringing,
            awake: view == View::Ringing,
        };
        if self.page.borrow().as_ref() == Some(&page) {
            return;
        }
        if let Err(e) = self.app.page(&page) {
            dprintln!("clock: cannot describe the page ({})", e);
        }
        *self.page.borrow_mut() = Some(page);
    }

    /// The page, the keyboard a focused field wants, and a frame.
    pub fn sync(&self) -> Result<(), i32> {
        self.describe();
        let ui = self.ui();
        let kf = ui.global::<KeyboardFocus>();
        let want = (kf.get_open(), kf.get_seq(), kf.get_mode());
        let had = self.keyboard.get();
        if want.0 != had.0 || (want.0 && (want.1, want.2) != (had.1, had.2)) {
            self.keyboard.set(want);
            let mode = if want.0 {
                want.2.max(1)
            } else {
                neoui_proto::ipc::KEYBOARD_HIDDEN
            };
            let h = self.app.keyboard(mode).unwrap_or(0);
            kf.set_text_h(h as f32);
        }
        self.draw()
    }

    /// Render the change; a scrolled list moves its rows instead of being redrawn (smooth-ui S2d).
    pub fn draw(&self) -> Result<(), i32> {
        let ui = self.ui();
        self.app.draw_scrolled(|| {
            let sc = ui.global::<clock_ui::Scroll>();
            let moved = (sc.get_dy(), sc.get_lists());
            sc.set_dy(0);
            sc.set_lists(0);
            let list = neoui_proto::Rect::new(
                sc.get_x() as i32,
                sc.get_y() as i32,
                sc.get_width() as u32,
                sc.get_height() as u32,
            );
            (moved, list)
        })
    }
}
