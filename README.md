# digi-roll-studio

Native Rust desktop sequencer for Elektron Digitakt II / Digitone II, built from
[digi-roll](https://github.com/zooloo303/digi-roll) — a browser piano roll for the
same boxes.

Where digi-roll edits one track at a time in a browser, this sequences a whole
studio: several boxes in one session, each with its own pattern of up to 16
tracks, all playing to a shared clock. A DT2 and a DN2 together is the target
case — 32 tracks.

The protocol work this stands on is ported from
**[elk-herd](https://github.com/mzero/elk-herd) by mzero** (BSD-2-Clause), which
is the de-facto public documentation of Elektron's SysEx protocol. Without it
there is nothing here. See [`CREDITS.md`](CREDITS.md) for what came from where.

---

## ⚠ This app can change what is on a box's card

From the moment a box has an out port and Play is pressed it sends clock and
notes. SEND TO BOX overwrites one track of one pattern slot; BACKUPS replaces a
whole slot — all sixteen tracks, the kit and its sounds.

Every write goes through five rules that cannot be skipped:

1. the whole destination pattern is re-read and backed up first — **a backup that
   cannot be stored is a write that does not happen**,
2. the target's OS build is checked against an allowlist,
3. you agree to a dialog naming exactly what changes,
4. only the bytes that must change are changed, and
5. the result is read back and byte-compared.

**Use throwaway projects on your boxes anyway.** This is beta software that
writes to hardware you cannot easily undo.

Auto-connect is on by default and opens ports by itself every three seconds to
ask an unclaimed Elektron-looking port who it is. That is read-only — two API
requests — but it is worth knowing about before wondering what is holding a
socket. The checkbox is at the bottom of BOXES.

---

## Status

**MVP1 candidate** — effectively a beta, and being tested as one.

| | |
|---|---|
| SysEx seven-bit, framing, pattern decode/encode | ported and **verified** against real DT2/DN2 captures |
| Trig conditions, p-lock lanes, pattern settings | ported both directions, against the committed captures |
| Core model | several boxes, 16 tracks each, scenes — `PLAN.md` §2 |
| Safe write | all five rules as one function, and **run on hardware** — one track of one slot, from the app's own button, verified byte-identical on a DT2 and a DN2 |
| Backups and restore | a local store of the last 50 patterns overwritten, plus 10 pre-restore snapshots; a store failure aborts the write. **A restore has been run on both boxes**, byte-identical |
| MIDI I/O | on `midir`; enumeration, identity handshake, dump reads and writes, all **run against both boxes** |
| Engine, transport, clock | **verified on hardware** — a DT2 and a DN2 playing one clock in sync |
| Session file | a whole session round-trips through JSON, wired to Save/Open with a close guard, **hardware-confirmed through a save/quit/reopen cycle**. Saving is manual — there is no autosave |
| Velocity, micro-timing | in the model, written to hardware, settable in the Edit panel and the roll |
| Harmony | key, scales, chord draw with a ghost, harmonise. A four-note chord written to a box and fetched back intact |
| Generator | seeded, nine modules, per-genre; a six-row arrangement has played and been synced to both boxes byte-identical |
| Copy-track | ported, translating p-lock lanes between boxes by parameter name — **no caller yet**, so no UI can copy a track |
| Song mode | **not built** — `Scene` has no `chain` field. A point release |
| Platforms | macOS is where it is developed and hardware-tested. **Windows compiles** (whole workspace, `x86_64-pc-windows-msvc`) but no WinMM path has met a box yet. Linux is untested |

**What is left before MVP1:** crash-safety (saving is manual, so a crash takes
the session) and packaging (no app bundle yet — it runs from a checkout, though
it has its icon and the Dock shows it).

---

## Build

Requires Rust 1.95+ (the MSRV `egui`/`eframe` 0.36 set, and enforced by `rust-version`). Linux additionally wants `libasound2-dev` for ALSA; macOS
and Windows need nothing beyond the toolchain, which is the whole reason this
uses `midir` rather than `rtmidi`.

```sh
cargo build --release
cargo test --workspace          # 1,262 tests, no system dependencies
cargo run -p digi_roll_studio   # the app
```

**Hardware is never part of the dev loop.** The protocol suites read `.syx`
captures from `crates/protocol/tests/fixtures/` — 1.4 MB of real DT2 and DN2
dumps, committed so the tests run anywhere. The examples that *do* talk to a box
are listed in [`DEVELOPMENT.md`](DEVELOPMENT.md) by safety class, most of them
read-only.

## Workspace

- `crates/core` — session/device/pattern/track model, edit ops, import/export, project file
- `crates/protocol` — SysEx seven-bit, Elektron protocol, pattern structs, byte lanes (trig conditions, p-locks, swing), safe-write, copy-track and the backup stash
- `crates/generator` — seeded pattern generator
- `crates/midi` — port enumeration and I/O, on `midir`
- `crates/engine` — transport, clock, scheduling
- `crates/app` — egui UI

## Documents

- **[`PLAN.md`](PLAN.md)** — the architecture, the model, and the rules that are
  not up for renegotiation. Source comments cite it by section (`PLAN.md §7 rule
  3`); those numbers are stable.
- **[`DEVELOPMENT.md`](DEVELOPMENT.md)** — how it was built, the hardware examples
  by safety class, your boxes' own MIDI settings that this app cannot reach, and
  nine lessons that each escaped a green test suite at least once.
- **[`CREDITS.md`](CREDITS.md)** — elk-herd, digi-roll, and the third-party
  notices.

## License

GPL-3.0-or-later — see [`LICENSE`](LICENSE).

The elk-herd-derived parts of `crates/protocol` and `crates/midi` are
BSD-2-Clause, © mzero. BSD-2 is GPL-compatible, so the combined work ships under
the GPL while those files keep their original notices; [`CREDITS.md`](CREDITS.md)
reproduces the BSD-2 terms in full, as it must. Keeping that attribution intact is
`PLAN.md` §7 rule 6 and is not optional.

Not affiliated with, endorsed by, or supported by Elektron. "Digitakt" and
"Digitone" are their trademarks. The protocol knowledge here is
reverse-engineered and public.
