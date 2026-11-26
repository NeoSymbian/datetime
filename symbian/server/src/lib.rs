// SPDX-License-Identifier: Apache-2.0
//! DTSRV.EXE (docs datetime.md DT1): the resident server that owns and rings the alarms and the timer, whether or not
//! Clock runs. A service of the UI server (protocol 1.19): the shell starts it at boot and again when it exits. It
//! serves dt-ipc 1.0 (`datetime_core::ipc`) to the apps, keeps `alarms.dat`/`timers.dat`, and on a ring opens
//! Clock's Ringing page (waking the screen) and posts a shade card; `dtsrv.next` tells the shell what rings next.
#![no_std]
extern crate alloc;

mod dt;
mod store;
mod ui;

use core::pin::pin;
use datetime_core::ipc;
use e32::rt::{self, Either};
use e32::server::{self, Server};
use e32::{dprintln, time};

fn main() -> i32 {
    let server = match Server::create(ipc::NAME) {
        Ok(s) => s,
        Err(e) => {
            dprintln!("dtsrv: {} taken ({}): exiting", ipc::NAME, e);
            return 0;
        }
    };
    e32::process::rendezvous(0);
    let ui = match ui::Ui::connect() {
        Ok(ui) => ui,
        Err(e) => {
            dprintln!("dtsrv: no UI server ({}): exiting", e);
            return e;
        }
    };
    let mut d = dt::Dt::start(ui);
    rt::block_on(async {
        let mut receive = pin!(server.receive());
        let mut event = pin!(ui::next_event(d.ui_session()));
        loop {
            let wait_us = d.tick();
            match rt::select(
                receive.as_mut(),
                rt::select(event.as_mut(), time::delay(wait_us)),
            )
            .await
            {
                Either::Left(Ok(msg)) => {
                    receive.set(server.receive());
                    let _ = server::dispatch(&mut d, msg);
                }
                Either::Left(Err(e)) => {
                    dprintln!("dtsrv: receive failed ({})", e);
                    receive.set(server.receive());
                }
                Either::Right(Either::Left(Ok(e))) => {
                    event.set(ui::next_event(d.ui_session()));
                    if !d.event(e) {
                        dprintln!("dtsrv: quit");
                        return 0;
                    }
                }
                Either::Right(Either::Left(Err(e))) => {
                    dprintln!("dtsrv: the UI server went away ({}): exiting", e);
                    return e;
                }
                Either::Right(Either::Right(())) => {}
            }
        }
    })
}

e32::main!(main);
