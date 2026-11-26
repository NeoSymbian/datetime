// SPDX-License-Identifier: Apache-2.0
//! The UI server (`NeoUi`) as a service sees it (protocol 1.19 "Services"): settings, OPEN with flags, shade cards
//! and events. The parts of neoapp DTSRV needs, without neoapp: that links Slint, and DTSRV never draws.

use alloc::rc::Rc;
use alloc::vec;
use e32::ipc::{Session, TIpcArgs, TVersion};
use e32::sys::{TPtr8, TPtrC8};
use neoui_proto::ipc::{self as proto, AppEvent};
use neoui_proto::page::Notification;
use neoui_proto::settings::Value;
use neoui_proto::settings::value::ENCODED_MAX;

/// OPEN's flag (slot 3, 1.19): the screen comes on and the shade closes, as for an incoming call.
pub use neoui_proto::ipc::OPEN_WAKE;
const KERR_ARGUMENT: i32 = -6;

pub struct Ui(Rc<Session>);

impl Ui {
    pub fn connect() -> Result<Self, i32> {
        let (major, minor) = proto::VERSION;
        Session::connect(
            proto::SERVER,
            TVersion {
                major,
                minor,
                build: 0,
            },
        )
        .map(|s| Self(Rc::new(s)))
    }

    /// The session for [`next_event`], which keeps a NEXT_EVENT outstanding while the other calls go on.
    pub fn session(&self) -> Rc<Session> {
        self.0.clone()
    }

    /// A setting's value: stored, else its registered default.
    pub fn get(&self, name: &str) -> Result<Value, i32> {
        let name = TPtrC8::new(name.as_bytes());
        let mut buf = vec![0u8; ENCODED_MAX];
        let len = {
            let mut out = TPtr8::new(&mut buf);
            let mut args = TIpcArgs::new();
            args.desc8(0, &name).des8(1, &mut out);
            self.0.send_receive(proto::SETTING_GET, &args)?;
            out.len()
        };
        Value::decode(&buf[..len]).map_err(|e| e.to_symbian())
    }

    pub fn set(&self, name: &str, value: &Value) -> Result<(), i32> {
        let encoded = value.to_bytes();
        let (name, value) = (TPtrC8::new(name.as_bytes()), TPtrC8::new(&encoded));
        let mut args = TIpcArgs::new();
        args.desc8(0, &name).desc8(1, &value);
        self.0.send_receive(proto::SETTING_SET, &args).map(drop)
    }

    /// `AppEvent::Settings` whenever a key under `prefix` changes.
    pub fn watch(&self, prefix: &str) -> Result<(), i32> {
        let prefix = TPtrC8::new(prefix.as_bytes());
        let mut args = TIpcArgs::new();
        args.desc8(0, &prefix);
        self.0.send_receive(proto::SETTING_WATCH, &args).map(drop)
    }

    /// Opens app `key` on `page` with `arg`; `flags` [`OPEN_WAKE`].
    pub fn open(&self, key: &str, page: u16, arg: &str, flags: i32) -> Result<(), i32> {
        let (k, a) = (TPtrC8::new(key.as_bytes()), TPtrC8::new(arg.as_bytes()));
        let mut args = TIpcArgs::new();
        args.desc8(0, &k)
            .int(1, i32::from(page))
            .desc8(2, &a)
            .int(3, flags);
        self.0.send_receive(proto::OPEN, &args).map(drop)
    }

    /// Adds or replaces card `n.id`; an empty title removes it.
    pub fn notify(&self, n: &Notification) -> Result<(), i32> {
        let b = n.encode().map_err(|_| KERR_ARGUMENT)?;
        let d = TPtrC8::new(&b);
        let mut args = TIpcArgs::new();
        args.desc8(0, &d);
        self.0.send_receive(proto::NOTIFY, &args).map(drop)
    }

    pub fn remove_card(&self, id: u32) -> Result<(), i32> {
        self.notify(&Notification {
            id,
            ..Notification::default()
        })
    }

    /// Removes all of DTSRV's cards (a restart's leftovers).
    pub fn clear_cards(&self) -> Result<(), i32> {
        self.0
            .send_receive(proto::NOTIFY_CLEAR, &TIpcArgs::new())
            .map(drop)
    }
}

/// The next event from the UI server. It owns its session and buffer, so it can stay pinned in the main loop.
pub async fn next_event(session: Rc<Session>) -> Result<AppEvent, i32> {
    let mut ev = [0u8; proto::EVENT_LEN];
    {
        let mut des = TPtr8::new(&mut ev);
        let mut args = TIpcArgs::new();
        args.des8(0, &mut des);
        session.send(proto::NEXT_EVENT, &args).await?;
    }
    AppEvent::decode(&ev).ok_or(KERR_ARGUMENT)
}
