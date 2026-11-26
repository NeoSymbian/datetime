# datetime

Clock and Calendar for [NeoSymbian](https://github.com/NeoSymbian): two apps, `NEOCLOCK.EXE` and `NEOCAL.EXE`, and
the resident server `DTSRV.EXE`, which owns every alarm, timer, event and to-do and rings them. The design is the
canvas's Clock and Calendar v2 boards (`clk2-*`, `cal2-*`, `dt2-*`); the plan is NeoSymbian/docs `datetime.md`
(DT0–DT7).

| Path | What | Licence |
|---|---|---|
| `crates/datetime-core` | The model: alarms, the timer, the stopwatch, the analog face (moved from neoui-proto in DT0) | Apache-2.0 |
| `crates/clock-ui` | Clock v2's screens (Slint) and their host snapshots `snapshots/clk2-*.png`, `dt2-ring-*.png` | GPL-3.0 (links Slint) |
| `ui-shared` | `@dt-shared`: TimeHero, TimeCard, ProgressRing, Ringing, the segmented switch (Clock and Calendar) | GPL-3.0 |
| `symbian/clock` | `NEOCLOCK.EXE` 0xE0008001: Clock · Alarms · Timers; the alarms and the timer through DTSRV | GPL-3.0 (links Slint) |

## Build

```sh
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test   # host
cd symbian && cargo fmt --all --check && cargo arm --workspace && cargo clippy-arm --workspace -- -D warnings
```

For a ROM, copy `symbian/exes.txt`'s lines and the staticlibs from `symbian/target/armv8a-neosymbian-eabi/release/`
into NeoSymbian/build's `rust/`. The pins (e32, Slint, neoui) must match neoui's `symbian/Cargo.toml`: one e32 and
one Slint per ROM. UIDs: this repo's block is 0xE0008xxx.
