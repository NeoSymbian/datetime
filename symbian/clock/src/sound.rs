// SPDX-License-Identifier: GPL-3.0-only
//! The ring tone while Ringing shows (and a sound's preview): one period of a NeoAudio built-in tone
//! (`neoaudio::tones`, the PCM the phone rings with) written again and again to a NeoAudio stream, on a thread of its
//! own so the UI keeps running. A new tone or [`stop`] ends the one that plays.

use core::sync::atomic::{AtomicU32, Ordering};
use e32::dprintln;
use e32::ipc::{Session, TIpcArgs, TVersion};
use e32::sys::TPtrC8;

/// Bumped to stop what plays: a player runs while the value it started with is current.
static PLAYING: AtomicU32 = AtomicU32::new(0);

// shortcut: a fixed loudness (neoaudio's 0..=10); read the sound profile's alarm volume once Settings has one.
const VOLUME: u8 = 8;

/// Plays `tone` (`neoaudio::tones::*`): over and over while `repeat`, else one period (a preview).
pub fn play(tone: u8, repeat: bool) {
    let mine = PLAYING.fetch_add(1, Ordering::SeqCst) + 1;
    if tone == neoaudio::tones::NONE {
        return;
    }
    let r = e32::thread::spawn(move || {
        let r = run(tone, repeat, mine);
        if let Err(e) = r {
            dprintln!("clock: tone {} failed ({})", tone, e);
        }
        0
    });
    if let Err(e) = r {
        dprintln!("clock: tone thread not started ({})", e);
    }
}

/// Stops the tone that plays.
pub fn stop() {
    PLAYING.fetch_add(1, Ordering::SeqCst);
}

fn run(tone: u8, repeat: bool, mine: u32) -> Result<(), i32> {
    let (major, minor, build) = neoaudio::VERSION;
    let s = Session::connect(
        neoaudio::NAME,
        TVersion {
            // the contract's version fits
            major: major as u8,
            minor: minor as u8,
            build: build as u16,
        },
    )?;
    let mut a = TIpcArgs::new();
    a.int(0, neoaudio::tones::RATE as i32).int(1, 1).int(2, 0);
    s.send_receive(neoaudio::op::OPEN, &a)?;
    let pcm = neoaudio::tones::pcm(tone, VOLUME);
    'play: loop {
        // WRITE waits while the stream's queue (250 ms) is full, so this keeps pace with the sound
        for chunk in pcm.chunks(8_192) {
            if PLAYING.load(Ordering::SeqCst) != mine {
                break 'play;
            }
            let des = TPtrC8::new(chunk);
            let mut a = TIpcArgs::new();
            a.desc8(0, &des);
            s.send_receive(neoaudio::op::WRITE, &a)?;
        }
        if !repeat {
            // let the preview's queue play out
            e32::time::sleep(300_000);
            break;
        }
    }
    let _ = s.send_receive(neoaudio::op::STOP, &TIpcArgs::new());
    Ok(())
}
