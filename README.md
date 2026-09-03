# digi-roll-studio

Native Rust desktop sequencer for Elektron **Digitakt II**, **Digitone II** and
**Analog Four**, built from
[digi-roll](https://github.com/zooloo303/digi-roll) — a browser piano roll for
the two digis.

Where digi-roll edits one track at a time in a browser, this sequences a whole
studio: several boxes in one session, each with its own pattern, all playing to
a shared clock. A DT2 and a DN2 together is the target case — 32 tracks — and an
A4 adds six more.

**All three boxes fetch, edit and write**, through the same panels and the same
write ceremony. The two digis speak Elektron's gen-2 dump protocol; the Analog
Four speaks gen-1, which is a different pattern format and a slower wire, and
the app hides both facts behind one button.

The A4 was shipped live-only in August 2026 on the strength of its own reply
listing no dump request. **That was a misread** — the list describes the file
API namespace, not the dump namespace — and it took two corrections to notice.
The box answers `0x60`–`0x6d`, and since 2026-08-31 its patterns, per-step lanes
and p-locks round-trip like a Digitakt's — and since 2026-09-01 its +Drive
presets load onto its tracks, which was the last thing the digis could do that
it could not. What is still not gen-2 about it is recorded in `PLAN.md` §10: its
FX and CV tracks' p-lock parameters have never been swept, so those two tracks
are carried back byte-exact rather than interpreted, and they have no sound to
put a preset on.

The protocol work this stands on is ported from
**[elk-herd](https://github.com/mzero/elk-herd) by mzero** (BSD-2-Clause), which
is the de-facto public documentation of Elektron's SysEx protocol. Without it
there is nothing here. See [`CREDITS.md`](CREDITS.md) for what came from where.

---

## ⚠ This app can change what is on a box's card

From the moment a box has an out port and Play is pressed it sends clock and
notes. Three things store bytes on a card: **SEND TO BOX** overwrites one track
of one pattern slot, **SYNC EVERY TRACK** does that to every track of every box
in one press, and **BACKUPS** replaces a whole slot — all sixteen tracks, the
kit and its sounds. All three work on an Analog Four as well as a digi.

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
| Core model | several boxes, track count and pattern length per model (16/128 on the digis, 6/64 on the A4), scenes, and a song of scenes — `PLAN.md` §2 |
| Safe write | all five rules as one function, and **run on hardware** — one track of one slot, from the app's own button, verified byte-identical on a DT2 and a DN2 |
| Backups and restore | a local store of the last 50 patterns overwritten, plus 10 pre-restore snapshots; a store failure aborts the write. **A restore has been run on both boxes**, byte-identical |
| MIDI I/O | on `midir`; enumeration, identity handshake, dump reads and writes, all **run against all three boxes** — including the A4's DIN pacing, which is part of the contract rather than a tuning knob |
| Engine, transport, clock | **verified on hardware** — a DT2, a DN2 and an A4 playing one clock in sync |
| Analog Four | **a full transfer peer of the digis since 2026-08-31**, and every claim here was made by the box. Plays: clock and transport receive, notes on channels 1-6 (four synth voices, FX, CV), 64-step patterns, all fourteen published CC/NRPN parameters swept. Transfers: `0x64` fetches any of 128 slots, the write goes back DIN-paced through the same safe-write ceremony, and a round trip — off the box, edited in the roll, back on — ran first time. Carries: velocity, note length, micro-timing and trig condition as named lanes, thirteen p-lock parameters that read, draw and edit, and — since 2026-09-02 — chords, as the trig's note plus the ARP menu's NO2–NO4 offsets, which is how the factory A01 plays them; they sound on a polyphonic kit with the arp off. Names its tracks' sounds off the box's **edit buffer**, which no digi can answer. The FX and CV tracks are read-only and byte-exact by decision — their p-lock id space has never been swept |
| Session file | a whole session round-trips through JSON, wired to Save/Open with a close guard, **hardware-confirmed through a save/quit/reopen cycle**. Saving is manual — there is no autosave |
| Velocity, micro-timing | in the model, written to hardware, settable in the Edit panel and the roll |
| Harmony | key, scales, chord draw with a ghost, harmonise. A four-note chord written to a box and fetched back intact |
| Generator | seeded, nine modules, per-genre; a six-row arrangement has played and been synced to both boxes byte-identical. A **Chord lead** role (2026-09-02) transcribes the Analog Four's factory A01 — pedal tones held over a root that leaps octaves — and has played from an A4, as chords, through its ARP offsets |
| Preset browser | the selected box's whole +Drive soundbank library, searched and filtered by tag across every bank at once. Tags live inside each preset file, so the panel scans — cancellable, resumable, and cached per bank so a second open is instant and works with the box switched off. **Three libraries indexed whole on hardware**: a DN2's 1,189 presets (of which 388 are Digitone mk1 files, a second container format), an A4's 869, a DT2's 148. All three tag tables are calibrated **exactly** — 24 of 24 captures, checked against Overbridge's filter grid rather than the boxes' own screens, because an A4 truncates its tag row at four and shows only some of what a preset carries. The digis share one 32-cell table; the A4's overlaps it in **two** positions |
| Preset load | double-click puts a preset on a track, **run on all three boxes**. Two routes, one gesture: a digi addresses one kit track's sound, and the A4 — which looked for three days like a box that could not do this at all — has its whole working kit fetched, one 350-byte sound spliced in, and the kit sent back. That box replaces a track with an **init sound** rather than refusing a preset saved by an older OS, so the load converts the struct version first; the conversion is two bytes and was measured off the box's own sound pool, not derived. Foreign container formats are refused by magic and say so by name, per destination. What has not been run is REVERT, the mk1 refusal message, and the OS-build gate through this path |
| +Drive writes | the file-write trio (`0x57`/`0x58`/`0x59`) is implemented in `digi_midi` and hardware-verified for a **single chunk**, behind a second allowlist disjoint from the read one. **The app itself never calls it** — the only caller is one example, so nothing you can press writes a file to a +Drive. Above 16 KiB is refused rather than guessed, so no whole project has been written back. `0x5A` Move, `0x5B` Copy and `0x5C` **Delete** are implemented nowhere in this workspace and nothing can reach them |
| Copy-track | the in-app whole-track copy works — Shift+C/Shift+V in the TRACKS grid, re-reading the source at paste time so it survives a scene change. Clicking a cell and pressing Delete clears that track — trigs and p-lock lanes, undoable, with the track's own name, channel, port, length and scale left alone. Shift+Up/Shift+Down transposes the selected track an octave and Alt+Up/Alt+Down a semitone — the whole track, moved whole or refused, never clamped or dropped — with the same four moves on buttons in the Edit panel. The **box-to-box** copy, which translates p-lock lanes between two boxes' payloads by parameter name, is ported and still has **no caller** |
| Song mode | rows of scenes with play count, length, mute and an END row, plus the `LST` trig condition it makes answerable. **Nothing in it has met a box or a screen yet** — `PLAN.md` §9 has the list. Per-row tempo is deliberately not built: the session has one clock |
| Platforms | macOS, Windows and Linux all build, test and package on CI, and all three have been **installed and launched from their own artefact**. On Windows a DN2 was auto-connected and written to; a DT2 has not met a Windows build. On Linux (Arch/Omarchy, from the `.pkg.tar.zst`) a DN2 was auto-connected over ALSA and named its OS build — **no write has been run from a Linux build**, and the portable tarball has been installed but not yet run against a box |

**What is left before MVP1:** crash-safety — saving is manual, so a crash still
takes the session. Packaging is done: both installers are built, and each has
been installed and run on its own platform.

---

## Build

Requires Rust 1.95+ (the MSRV `egui`/`eframe` 0.36 set, and enforced by `rust-version`). Linux additionally wants `libasound2-dev` for ALSA; macOS
and Windows need nothing beyond the toolchain, which is the whole reason this
uses `midir` rather than `rtmidi`.

```sh
cargo build --release
cargo test --workspace          # 1,855 tests, no system dependencies
cargo run -p digi_roll_studio   # the app
```

### Packaging

```sh
packaging/macos/build-dmg.sh          # Digi-Roll-Studio-<ver>-macOS-AppleSilicon.dmg
packaging/linux/build-tarball.sh      # Digi-Roll-Studio-<ver>-Linux-x86_64.tar.gz
packaging/linux/build-pkg.sh          # digi-roll-studio-<ver>-1-x86_64.pkg.tar.zst
```

The Windows installer is built on Windows, by
`packaging\windows\build-installer.ps1`. Each artefact is built on the platform
it runs on: pushing a `v*` tag runs all four on CI and drafts a release with the
four assets attached — see [`packaging/README.md`](packaging/README.md), which
also explains why the macOS bundle has to be ad-hoc re-signed after it is
assembled and not before, and why Linux ships two downloads rather than one.

On Linux the tarball installs per-user into `~/.local` with the `install.sh`
inside it and needs no root; on Arch and derivatives prefer the package, which
is the only one of the four that can declare the libraries eframe and wgpu
`dlopen` at runtime and so refuse to install where the app would not start.

**Hardware is never part of the dev loop.** The protocol suites read `.syx`
captures from `crates/protocol/tests/fixtures/` — 34 real dumps, 1.8 MB,
committed so the tests run anywhere. Ten are DT2/DN2 and **twenty-one are the
Analog Four's**, one per question its lanes and p-lock pool were mapped one at a
time by; 24 `.bin` preset files sit under `fixtures/drive/`. The examples that
*do* talk to a box are listed in [`DEVELOPMENT.md`](DEVELOPMENT.md) by safety
class, most of them read-only — and that list is checked against the examples
directory by a command in the same section, because it has been incomplete
twice.

## Workspace

- `crates/core` — session/device/pattern/track model, edit ops, import/export, project file
- `crates/protocol` — SysEx seven-bit, Elektron protocol, pattern structs, byte lanes (trig conditions, p-locks, swing), safe-write, copy-track and the backup stash; the gen-1 Analog Four format in its own `a4_*` modules rather than behind a generation flag; the +Drive file API and preset/sound decoding
- `crates/generator` — seeded pattern generator
- `crates/midi` — port enumeration and I/O, on `midir`
- `crates/engine` — transport, clock, scheduling
- `crates/app` — egui UI

## Documents

- **[`PLAN.md`](PLAN.md)** — the architecture, the model, and the rules that are
  not up for renegotiation. Source comments cite it by section (`PLAN.md §7 rule
  3`); those numbers are stable. Its §9 and §10 are four fifths of it and are a
  **hardware ledger** rather than a plan — what has touched a box, and every
  Analog Four offset with the screen reading that graded it.
- **[`DEVELOPMENT.md`](DEVELOPMENT.md)** — how it was built, the hardware examples
  by safety class, your boxes' own MIDI settings that this app cannot reach, and
  eighteen lessons that each escaped a green test suite at least once.
- **[`packaging/README.md`](packaging/README.md)** — how the two downloads are
  built, why the asset filenames are load-bearing, and the three Windows-only
  things that never show up in a `cargo run`.
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
