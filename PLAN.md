# digi-roll-studio — architecture and build plan

A standalone native Rust sequencer built from the digi-roll protocol work.
Decided 2026-08-13: **a pattern is one box's tracks — up to 16 — and a session
holds several boxes at once.** A pattern mirrors the DT2/DN2 pattern struct so
it maps 1:1 onto that box's pattern slot; a session sequences a DT2 *and* a DN2
(16 tracks each, 32 in total) side by side, and the model is sized from the
device rather than fixed at 16 so a 4-track or 12-track box fits later without
reshaping it.

This is the architecture, the decisions behind it and the rules that are not up
for renegotiation. Source comments across the workspace cite it by section —
`PLAN.md §7 rule 3` and the like — and those numbers are stable. `DEVELOPMENT.md`
is the companion: how the thing was built, and the lessons that kept repeating.

> **On what this file used to be.** Through development this was a 4,000-line
> working document carrying every phase in full, plus a session log beside it.
> Both were trimmed to this for the public repo. Nothing load-bearing was
> dropped — the model, the rules, the risks and the verification status are all
> here — but the blow-by-blow is gone, and so are the two design-handoff
> packages some UI comments cite by path.

---

## 1. Where the port actually stands

Audited 2026-08-13 against the JS original, line by line, and kept current since.

### Verified against hardware captures

| File | Verdict |
|---|---|
| `protocol/sevenbit.rs` | Faithful port of `js/elektron/sevenbit.js`. |
| `protocol/protocol.rs` | Faithful. API + dump framing, checksum14, stream split. Exercised by every fixture load. |
| `protocol/pattern.rs` | Decode, encode, diff and annotation all pinned by fixture tests against DT2 and DN2 captures. |
| `core/*` | The §2 session model. `Session → Device → Pattern → Track`, with the device table driving track count. |

Twelve `.syx` captures live in `crates/protocol/tests/fixtures/` (1.4 MB): eight
DT2/DN2 condition and p-lock captures, three fresh/swing DN2 patterns, and the
per-note chord capture. Every expected value in the suites was read out of them
**by the JS original first** and then written down in Rust, so the tests pin
digi-roll's hardware-verified behaviour rather than the port's own output.

### What Phase 1 found, because the class of bug is the point

- `protocol.rs` — `uint14be` returned `[u16; 2]` where `[u8; 2]` was declared.
  The workspace **did not compile**, contrary to what the docs claimed.
- `crates/app/src/ui/mod.rs` was missing entirely, so the app crate could never
  have built.
- 691 build artifacts were committed with no `.gitignore`.
- **`encode_track_notes` produced non-deterministic bytes.** It grouped notes
  into a `HashMap<u8, Vec<Note>>` and iterated it to claim trig-pool records.
  Rust seeds each `HashMap` separately, so the same pattern encoded to a
  different byte layout on each run — records landed in different pool slots,
  and both the minimal-diff contract and the read-back verify stopped meaning
  anything. Now a `BTreeMap`, matching the JS `Map`'s insertion order after its
  `(step, pitch)` sort. Five round-trip tests fail if the `HashMap` is put back.
- **Micro-timing rounding disagreed with the JS at exactly −n.5 ticks.** Rust's
  `f64::round` rounds halves away from zero; `Math.round` rounds them toward
  +∞. Now one `micro_steps_to_byte` helper that does what the JS does.
- `rtmidi` removed from `crates/midi` — declared, never called, and it needs a
  system C library plus bindgen, so `cargo test --workspace` failed on a clean
  machine. Phase 3 added `midir` in its place.

### Still open

- **`protocol::copy_track` has no caller.** Ported 2026-08-19; nothing in `core`
  or `app` reaches it, so no UI can copy a track. The last item in Phase 6.
- **The track headers are short of §5's own words**: no level meter, no port
  shown, no device colour inherited by the tracks.
- **Song mode does not exist.** `Scene` has no `chain` field. Phase 12.
- **Crash-safety.** Saving is manual; there is no autosave, so a crash takes the
  session.
- **Packaging.** No app bundle yet — it runs from a checkout.

**Honest summary:** a verified protocol foundation; a sequencer that has driven
two real boxes in sync; read, write and restore all proven on hardware from the
app's own buttons; a session that saves and reopens. What is missing is surface
and shipping, not seams.

**Banks are cut rather than outstanding**, decided 2026-08-18.

---

## 2. The model

Four levels, because the hardware has four: a session holds boxes, a box holds
pattern slots, a slot holds tracks, a track holds notes.

```rust
pub struct Session {
    pub name: String,
    pub tempo_bpm: f64,          // one clock for the whole session
    pub devices: Vec<Device>,    // a DT2 and a DN2 is the target case
    pub scenes: Vec<Scene>,
    pub current_scene: usize,
}

/// One physical box. Identity is the instance, not the model — two DT2s are
/// two devices.
pub struct Device {
    pub id: DeviceId,
    pub name: String,            // user label: "DT2", "DN2", "Syntakt"
    pub model: DeviceModel,
    pub patterns: Vec<Pattern>,  // slots, addressed as the box does: bank + index
    pub io: DeviceIo,            // ports + identity
}

/// Everything the model needs to know about a box. Data, not an enum arm with
/// hard-coded 16s — a 4-track A4 or 12-track Syntakt is a table entry.
pub struct DeviceModel {
    pub key: &'static str,       // "DT2", "DN2"
    pub display: &'static str,
    pub num_tracks: usize,       // 16 for DT2/DN2, 4 for A4, 12 for Syntakt
    pub max_steps: u16,          // 128
    pub sysex: Option<&'static Spec>,  // None ⇒ live-play only, no fetch/write
}

/// One pattern per device, chosen together. Switching scene switches every box.
pub struct Scene {
    pub name: String,
    pub slots: HashMap<DeviceId, PatternRef>,   // bank + pattern index per box
}

pub struct Pattern {
    pub name: String,
    pub swing: u8,               // 50..=80. Per pattern — re-times this box's tracks
    pub tracks: Vec<Track>,      // len == device.model.num_tracks, invariant
    pub source: Option<Source>,  // box/pattern this came from
}

pub struct Track {
    pub name: String,
    pub length_steps: u16,       // per-track → polymeter, as on the box
    pub scale: TrackScale,       // clock multiplier, 1/8x .. 2x
    pub track_prob: u8,          // 100 = always; the default an unlocked trig runs at
    pub kind: TrackKind,         // Audio | Midi — DT2 carries this as a kit mask
    pub notes: Vec<Note>,
    pub plocks: Vec<PLockLane>,
    // live-play routing, not part of the box's pattern struct
    pub out_port: Option<PortId>,
    pub channel: u8,
    pub mute: bool,
    pub solo: bool,
}
```

Decisions behind that shape:

- **Track count comes from `DeviceModel`, never from a constant.** `tracks` is a
  `Vec` with a `len == num_tracks` invariant enforced at construction, not a
  `[Track; 16]`. v1 ships DT2 and DN2 profiles only; A4 (4) and Syntakt (12) are
  then a table entry plus a `sysex: None`, with no model surgery.
- **`sysex: None` means sequence-live-only.** Such a device edits and plays
  normally over MIDI; fetch and write are unavailable, and the UI says so rather
  than failing at write time.
- **Tempo is per session; swing is per pattern.** Tempo is one clock the studio
  masters and sends to every port. Swing stays the per-pattern byte it is on the
  box, so the DT2 and DN2 can swing differently and write-back stays a
  byte-for-byte match. Note the consequence: `Pattern` has no `tempo_bpm`, but
  the DT2/DN2 pattern struct *does* (`pattern.tempo_offset`) — see §7 rule 8.
- **Scenes are how the boxes stay together.** Each box keeps its own slot bank,
  addressed the way the box addresses it. A scene names one slot per box;
  switching scene switches all of them at the next pattern boundary, which is how
  chaining behaves on the hardware.
- **DT2's MIDI tracks are not tracks 17–32.** `dt2_spec()` carries
  `midi_mask_offset` in the *kit* — a mask over the same 16 tracks. `TrackKind`
  reads from that mask. DN2 has no mask and no fallback.

Three things this buys that the browser app cannot do:

- **Per-track length is real polymeter.** Tracks wrap independently, which is how
  the boxes behave and what digi-roll's 8-slot model could never express.
- **`PRE` and `NEI` trig conditions become honestly simulable.** `js/midi.js`
  documents them as unsimulable *because digi-roll plays one track at a time and
  keeps no history*. With a full box of tracks and a per-track condition history,
  both can be evaluated for real. Both are **per device** — `NEI` reads track
  *n−1* of the same box, never across boxes.
- **Swing lands in its correct scope** — one per-pattern byte that re-times that
  box's tracks.

Routing fields (`out_port`, `channel`, `mute`, `solo`) are studio state, not
pattern bytes. They persist with the project file and are never encoded into a
dump, which keeps the minimal-diff contract clean. `DeviceIo` is likewise session
state: ports are matched by name on load, and a missing port disables that
device's I/O without touching its patterns.

Solo is session-wide, not per device: soloing a DT2 track silences DN2 tracks
too, which is the only reading that makes sense at a mixing desk.

---

## 3. Crate layout

```
core       model, device profiles, edit ops, project file (serde). No I/O.
protocol   SysEx: sevenbit, framing, pattern structs, safe-write.
engine     transport, clock, scheduler, condition evaluation, note tracking.
midi       port enumeration + I/O, on midir.
generator  seeded pattern generator, ported from js/gen.
app        egui UI.
```

`engine` depends on `core` + `midi`, never on `protocol`. Playing a pattern and
writing one to a box are separate concerns and stay that way.

`core` owns the `DeviceModel` table and depends on `protocol` for two things
only: the `Spec` type as an opaque "this model can do SysEx" handle, and the two
hardware-verified LEN byte conversions. The graph is acyclic — `protocol` knows
nothing of `core`. `import.rs` and `export.rs` are the two bridges between the
model and the pattern format, so they hand `protocol` a `Spec` and take back
notes, lanes, trig settings and a `TrackWrite`. **`core` still parses no bytes**,
which is the rule actually being defended. A `devices` crate below both stays on
the table if `engine` ever needs building without `protocol` in the tree.

**Why `midir`, not `rtmidi`.** `midir` is not pure Rust — it is FFI to the same
native APIs RtMidi wraps: CoreMIDI, ALSA, WinMM. The difference that matters is
that those ship **with the OS**, so there is no third-party library to install on
macOS or Windows (Linux still wants `libasound2-dev` via pkg-config). `rtmidi`
needed `brew install rtmidi` plus bindgen on every machine that builds this,
which is unacceptable for something meant to run standalone. Adding `midir`
pulled eight crates and no C toolchain.

---

## 4. The realtime engine

The new work. Two things do not transfer from the browser.

### Timing: the Web MIDI lookahead trick does not port

`js/midi.js` runs a coarse 25 ms pump that hands `MIDIOutput.send()` future
timestamps, so interval jitter never reaches the wire. **`midir` has no
timestamped send** — `send()` is immediate. The driver will not do the scheduling
for us. So the engine thread must itself wake at the right moment:

- A dedicated thread, not the UI thread and not an audio callback.
- Events computed into a sorted queue over a ~50 ms horizon.
- Sleep to just before each event, then a short spin to the deadline. ~1 ms
  jitter is achievable this way; a plain `thread::sleep` is not.
- Later optimisation, macOS only: CoreMIDI *does* accept scheduled packet
  timestamps. Not for v1.

### Resolution

Note events are **not** quantised to a tick grid. Positions are computed as
fractional steps in `f64` and converted to absolute deadlines, because
micro-timing is 1/24 step (hardware-verified) and swing is a percentage offset on
odd steps that is not an integer number of 1/24ths.

**Swing, settled 2026-08-13.** The sources disagreed. `js/midi.js` computes
`((swing - 50) / 50) * (stepMs / 3)` — 0.2 of a step at swing 80 — while its own
comment one line above says "at 66% the odd step lands 2/3 through the pair",
which that formula does not produce. Settled in favour of the reading the comment
describes and the boxes document: **swing is the percentage of the way through a
*pair* of steps that the odd step lands, so the offset from straight is
`(swing - 50) / 50` steps** — 0 at 50%, exactly 1/3 of a step at 66.7% (a triplet
feel), 0.6 at 80%.

This does not cross §7 rule 3. Rule 3 protects the hardware-verified
*encode/decode* internals, and the swing byte's mapping is untouched — see
`protocol::pattern_settings`, pinned against three DN2 captures. What changed is
the browser preview's playback approximation, which never touched hardware and
contradicted itself. `engine::time::swing_offset_steps` carries the argument, and
a test states the property rather than pinning magic numbers: an odd step at
offset *d* sits `(1 + d) / 2` through its pair, for every swing value the byte
can hold.

Quantising to 96 PPQN would be exact for micro and wrong for swing. Only the MIDI
clock runs on a grid: 24 PPQN, `0xF8`. (96 PPQN stays the resolution for
**Standard MIDI File export**, matching `js/midi.js`: 96 TPQN, 24 ticks per 16th
step.)

### One thread, several boxes

The session's devices share one engine thread and one clock. Two boxes is not two
sequencers side by side; it is one event queue whose entries carry a port.

- Every scheduled event is `(deadline, PortId, MidiMsg)`, sorted by deadline
  across all devices, so a DT2 trig and a DN2 trig on the same step go out back
  to back with no per-device drift.
- **MIDI clock goes to every device's out port**, all from the same tick counter —
  `0xF8` at 24 PPQN, start/stop/continue together. Nothing gets its own clock.
  Which ports receive clock is per-device, since a box slaved to something else
  must not be fought over.
- `midir` connections are one per port and events are batched per port per
  wake-up, so a tick is N sends, not N × tracks.
- **Scene change is a queued command**, taken at the next pattern boundary —
  for a session, the boundary of the *longest* track across all devices in the
  outgoing scene, so polymetric tracks are not cut mid-cycle. An "immediate"
  setting exists. On switch, every pending note-off from the outgoing scene still
  fires.

### Note lifetime

An active-note table keyed by `(port, channel, pitch)`, each entry carrying its
note-off deadline. Non-negotiable, because note lengths are fractional and tracks
have independent lengths, so note-offs routinely fall after their track has
wrapped.

- Stop flushes every pending note-off.
- `Panic` sends All Notes Off (CC 123) + All Sound Off (CC 120) on every channel
  in use.
- Re-triggering a sounding pitch sends its note-off first.

### Per-trig conditions

Ported from `shouldPlay` in `js/midi.js`, keeping its two-level PROB model: a
trig's own PROB lock overrides the track default, including an explicit 100. The
RNG stays injectable so tests are deterministic.

| Condition | Browser | Here |
|---|---|---|
| `PROB`, `1ST`, `A:B` | simulated | simulated |
| `PRE` | not simulable | **simulated** — last conditional result on this track |
| `NEI` | not simulable | **simulated** — last conditional result on track *n−1* **of the same device**; track 1 has no neighbour and plays |
| `FILL` | no FILL button | **simulated** — a FILL toggle in the transport |
| `LST` | unknowable | still unsimulated until pattern chaining exists |

**Anything unsimulated plays, so the sequencer is never quieter than the box** —
keep that rule.

### Threading

UI owns the model. On edit it publishes an `Arc<Session>` via `arc_swap` — one
snapshot for the whole session, not one per device, so the boxes can never pick
up halves of an edit. The engine picks the new one up at the next bar boundary
(or immediately, a setting). The engine never locks and never allocates on its
thread. Playhead position, per-device track positions and active-note count go
back to the UI through a lock-free cell for display.

Cost of a whole-session snapshot: an edit to one DT2 note clones the `Arc`s of
everything. That is cheap only if `Pattern` and `Track` are themselves behind
`Arc`s inside `Session`, so a snapshot is ~40 pointer bumps rather than a deep
copy of 32 tracks of notes. Built that way from the start.

---

## 5. UI

### The shell, decided 2026-08-17

Two collapsible side panels and one row of two panes above the roll, after
digi-roll's rail:

```text
transport ─ spans the window, never moves
rail │ tool panel │ PATTERNS │ SCENES │ Setup
     │            │      piano roll   │
```

**Left is what you are composing, right is what you are composing on.** The rail
holds the editing tools — Edit, Harmony, Generate, and Session, the only place
this app can save what you have been doing — one panel open at a time, clicking
the open one closing it. The right panel holds everything per-device: ports,
clock, identity, and the fetch/write-back, because a transfer is aimed at one
box's slot and belongs beside that box rather than beside the notes.

Patterns and scenes are **side by side, not stacked**: a scene is built out of
patterns that already exist, so the two are one sitting, and stacked full-width
bars spent height the roll needs. The rail cannot be hidden — it is what reopens
the tool panel — and the transport carries the `SETUP` toggle for the other one.

The elements:

- **Device strip** — one group per box in Setup: name, model, in/out port,
  connection state, current slot, whether it takes clock.
- **Transfer group** — one block per box: which slot to fetch off the box, which
  slot of this session it lands in, what the last fetch brought across. The two
  slot spaces are different sizes and are offered as such. **Writing back is
  offered in its own groups below** — SEND TO BOX for one track, BACKUPS for a
  whole slot — deliberately not folded into this one, because a fetch is
  read-only and a send is not.
- **Track headers, grouped by device** — a DT2 group of 16 and a DN2 group of 16
  in one scrolling list, with a fold triangle per box; a folded group says which
  of its tracks the roll is editing. Name, mute/solo, channel, length and scale
  are there; **level meter, port and colour are not**, and device colour
  inherited by its tracks is what would make the roll readable across boxes.
- **Scenes pane** — the §2 scenes, showing which slot each box is on and what is
  queued.
- **Piano roll** for the selected track. Ghosting the other tracks behind it is
  parked.
- **Trig lane** under the roll — per-step PROB/FILL/COND, ported from
  `js/triglane.js`.
- **P-lock lane strip** — ported from `js/plocklane.js`, with the parameter
  tables, the pool reader and the audition path under it.
- **The Edit panel** — velocity, length and PROB; swing, duplicate bar, clear;
  the lane list; MIDI file import and export; undo and redo. Every gesture at the
  top of `js/pianoroll.js` is ported.
- **The Harmony panel, the tinted rows and chord draw**, from `js/chords.js`. The
  key is on the session rather than in the panel, because the generator reads it
  and because it is saved. The roll gained three marks with it: the scale wash,
  the root's stronger wash, and the chord ghost — **none of them a glyph**,
  deliberately, so the tofu class of `DEVELOPMENT.md` lesson 1 cannot reach them.

  Four rules, cited from the source as §5 (and, in older comments, as "§11"
  after the phase that built it):

  1. **The tint is visual only.** A key washed over a row constrains nothing —
     you can still draw, drag and resize a note outside the scale. Harmony is an
     aid, not a mode.
  2. **Harmony stays one panel.** Key, scale, chord quality and harmonise are one
     sitting, not a menu and a mode and a dialog.
  3. **Chord draw only claims empty space.** Dragging on an empty row draws a
     chord under the ghost; **a note you click on still moves, resizes and deletes
     exactly as before.** That is the whole affordance — the gesture has to be
     free to be discovered without taking anything away.
  4. **A chord is one trig with several notes**, per `js/chords.js` — never
     several trigs.
- **Transport** — play/stop/continue, tempo, swing, FILL, panic, clock
  master/slave.

The look — spacing, the colour tokens in `ui::mod`, the panel and slider-row
rules — came from two design-handoff packages, in a first pass (Setup panel and
track lanes) and a full second pass (transport, side panels, whole window). Some
UI comments cite those packages by path; they are working design references and
are not in the public repo. The tokens they define live in `ui::mod`, which is
the authority the code actually reads.

---

## 6. Phases — how it was built

Thirteen phases plus four UI passes, 2026-08-12 to 2026-08-20. `DEVELOPMENT.md`
has the lessons; this is the order.

| | | |
|---|---|---|
| **1** | truth first | ✅ 2026-08-13 — the protocol layer pinned against real captures, and the four bugs above found |
| **2** | the session model | ✅ 2026-08-13 — §2, replacing digi-roll's 8 slots |
| **3** | MIDI I/O | ✅ 2026-08-13 — `midir` in, `rtmidi` out; enumeration, identity handshake, dump reads |
| **4** | the engine | ✅ 2026-08-13 — all four criteria met on hardware; **a DT2 and a DN2 on one clock, in sync** |
| **5** | UI | ✅ 2026-08-18 — the shell, the roll rewritten against egui's `interact` model, the trig and p-lock lanes |
| **6** | writing to a box | ⚠ one item open — all five safety rules as one function, **run on hardware**; a caller for `copy_track` is parked |
| **7** | the generator, and the Generate panel | ✅ 2026-08-19 — seeded, nine modules, per-genre; a six-row arrangement played and synced byte-identical |
| **8** | the session file | ✅ 2026-08-18 — `core::project` round-trips a whole session through JSON, with a close guard |
| **9** | the Edit panel, and the last four roll gestures | ✅ 2026-08-18 — velocity reachable by hand for the first time |
| **10** | the desk: Setup, auto-connect, mass send | ✅ 2026-08-18 |
| **11** | the Harmony panel | ✅ 2026-08-19 — `js/chords.js` ported; a four-note chord written to a box and fetched back intact |
| **12** | song mode: scene chaining | **not built** — `Scene` has no `chain` field. A point release |
| **13** | pattern progress indicators, by track | ✅ 2026-08-19 |

Then, all on hardware or on a screen rather than in a suite: the Setup-panel and
track-lane redesign (2026-08-19), the full UI pass (2026-08-19), a bug-fix pass
that found four faults sharing one egui mechanism (2026-08-19), a Windows
cross-compile audit (2026-08-19, §8), and three rounds of tester feedback on the
MVP1 candidate (2026-08-20) — the last of which found the only bug in this project
so far that could be *heard*: the transport reading 174 BPM while the boxes played
120. `DEVELOPMENT.md` lesson 5 is that bug's general form.

**The decisions worth carrying forward**, each of which changed the shape of the
thing rather than a line of it:

1. **A pattern is one box's tracks, and a session holds several boxes** (§2).
   Everything else follows from it — polymeter, honest `PRE`/`NEI`, swing in its
   right scope.
2. **Track count is data, not a constant.** A new Elektron box should be a table
   entry.
3. **One clock, one queue, one thread** (§4) — not one sequencer per box.
4. **The engine never sees `protocol`** (§3). Playing and writing stay separate.
5. **The five write-safety rules are one function** (§7 rule 1), so no caller can
   skip a step.
6. **Verification means hardware or pixels, not a green suite.** The whole of
   `DEVELOPMENT.md` is this decision's receipt.

---

## 7. Rules carried over from digi-roll

These are not up for renegotiation in the port.

1. **The five write-safety rules, as one function.** Backup, minimal diff,
   firmware allowlist, verify after write, throwaway projects only.
2. **Always re-fetch the target pattern immediately before encoding.** Never
   write back a payload captured earlier.
3. **Do not "improve" the hardware-verified encode/decode internals.** They were
   mapped byte-by-byte against real hardware. Port them; compose them. Deviating
   from JS behaviour is a bug even when the Rust looks nicer.
4. **Velocity, length and micro are per note, not per trig.**
5. **Hardware is not part of the dev loop.** Work against fixtures and tests.
6. Keep the elk-herd attribution (BSD-2-Clause, © mzero). See `CREDITS.md`.
7. **A write names one device and one slot.** Nothing in the studio ever writes
   "the session". Firmware allowlist, backup and verify are per box, and the
   confirm dialog says which box and which pattern slot by name.
8. **The session's tempo is never written to a box.** The DT2/DN2 pattern struct
   has a tempo field (`pattern.tempo_offset`) that the session model deliberately
   does not mirror. Minimal diff therefore leaves those bytes untouched — the
   same treatment swing and trackProb already get from the generator.

---

## 8. Risks

- ~~**The port is unverified.**~~ Retired by Phase 1: the byte-level claims in §1
  are now passing tests against real captures.
- ~~**Timing on a non-realtime thread.**~~ ~~**Two ports make timing harder, not
  twice as hard — worse.**~~ Both retired 2026-08-13 by measuring rather than
  arguing: with both boxes on their own USB endpoints, the two ports came back at
  **7 µs mean and 28/29 µs worst, nothing over 1 ms** — symmetric, and roughly
  forty times inside the ~1 ms target. Symmetry is precisely what says the ports
  are not delaying each other.

  What this does *not* cover: ten seconds is not a set, and one Mac is not every
  Mac. Nor does it cover the far end of the cable — it measures when sends left,
  which is all a jitter figure can mean. If a long run or a loaded machine ever
  bites, the two fallbacks are unchanged: a sender thread per port fed from the
  shared queue (costs a handoff, isolates the ports), or on macOS the `coremidi`
  crate, whose scheduled packet timestamps move scheduling into the driver.
- ~~**`egui` 0.27 is old.**~~ Retired 2026-08-13: bumped to 0.36.1 *ahead of*
  Phase 5 rather than during it, because 0.27 could not start on this macOS at
  all. Doing it while the UI was ~415 lines cost three call-site changes; doing
  it after Phase 5 would not have.
- **Scope.** ~16,000 lines of JS, about a third of it tests.
- **Windows, opened 2026-08-19 and half-retired the same day.** The app compiles
  for `x86_64-pc-windows-msvc`, whole workspace, eframe and wgpu and `rfd`
  included — checked, not assumed. No `cfg(target_os)` arm in the app layer, no
  hardcoded unix path, and `Stash::default_dir` already resolves `%APPDATA%`.

  Two of the three MIDI concerns were already handled, one by luck and one by
  design. `midir`'s WinMM *input* uses 1 KB SysEx buffers and does **not**
  reassemble across them — its own source carries the TODO saying so — which
  would shred a 127 KB pattern read, except `midi::sysex_stream` already
  accumulates F0…F7 across callbacks. That was written for ALSA and covers WinMM
  unchanged.

  **The third was a real blocker, and it is the shape to remember: two platforms
  can want opposite things from the same code.** `midir`'s WinMM `send()` decides
  sysex-versus-short-message by testing `message[0] == 0xF0` and refuses any other
  send over three bytes. Only the *first* chunk of a split dump begins `0xF0`, so
  `paced_send`'s 4 KB chunking — which exists solely because CoreMIDI drops
  anything it cannot describe in a `UInt16` — would have failed on chunk 2 of 31,
  breaking SEND TO BOX, SYNC EVERY TRACK and restore on Windows while leaving
  identify, fetch and clock working. `SEND_CHUNK` is now `cfg`-conditional.

  **Answered on 2026-08-21:** a driver does swallow a whole pattern in a single
  `midiOutLongMsg`. A DN2 was identified, auto-connected and written to from an
  installed Windows build — so the `cfg`-conditional `SEND_CHUNK` is right, and
  the unchunked path it selects on Windows works against real hardware. What is
  still open is the DT2's larger payload, which no WinMM build has sent. The
  failure mode if a driver ever refuses one is loud rather than silent —
  `paced_send` propagates with `?`, and rule 1 has already taken the backup — so
  a bad answer costs a refused write, not a scrambled slot. Linux is unexamined
  beyond the observation that ALSA accepts sysex continuations, so the chunking
  is correct there.

---

## 9. Verification status

**Hardware is never part of the dev loop** (§7 rule 5). `cargo test --workspace`
needs no system dependencies and no box. This section is the separate register of
what has actually met a DT2 and a DN2, because a green suite cannot say. Every
entry below was recorded on a DT2 at OS 1.15B (build 0070) and a DN2 at OS 1.10D
(build 0049). Both boxes moved to 1.15C (0071) and 1.10E (0050) on 2026-08-21;
the fetch-edit-write round trip was re-run on the new OSes that day, and the last
entry below is that run. Nothing else here has been.

### What has touched a box

- **Enumeration, identity handshake, dump reads** — Phase 3, 2026-08-13.
- **Transport and clock: a DT2 and a DN2 playing one clock in sync** — Phase 4,
  2026-08-13. Jitter measured at 7 µs mean, 28/29 µs worst (§8).
- **Fetch** — a pattern and kit read off both boxes into a session, 2026-08-17.
- **The first write, from the app's own button** — 2026-08-18. One track of one
  slot, verified byte-identical on a DT2 and a DN2.
- **Restore** — a whole slot put back from the backup stash on both boxes,
  byte-identical, 2026-08-18.
- **Save / quit / reopen** — a session file round-tripped through disk and the
  native dialogs, 2026-08-18.
- **A generated six-row arrangement** — played, and synced to both boxes
  byte-identical, 2026-08-19.
- **A four-note chord** — written to a box and fetched back intact, 2026-08-19.
- **The tempo fix** — a few sessions and the Generate panel's SET, confirmed
  against what the boxes actually played, 2026-08-20.
- **The new OSes** — DT2 1.15C (0071) and DN2 1.10E (0050), 2026-08-21. Patterns
  fetched off both boxes, edited in the app and written back, verified. This is
  the run those two builds are on the write allowlist for.
- **Windows, end to end** — 2026-08-21. The Inno installer built on a PC and
  installed; the app ran; auto-connect found a DN2 and claimed it; a pattern with
  a trig condition on it was written to the box and behaved as expected. The
  first box any WinMM build has met, and the answer to §8's open question.
- **Both installers, from a user's side** — 2026-08-21. The `.dmg` installs and
  the app opens and runs; the Windows setup `.exe` likewise.

### What has not

- **A DT2 on Windows.** The WinMM path has met a DN2 (above) and no DT2, so the
  larger of the two payloads has only ever gone out over CoreMIDI. See §8.
- **Linux**, beyond the observation that the chunking is correct for ALSA.
- **`copy_track`** — it has no caller, so nothing can drive it.
- **"Read patch names"** against a box, since the named-slot path landed
  2026-08-20.

### Not verified on a screen

The register that matters as much as the hardware one, because **a green suite
says nothing about what can be seen** — `DEVELOPMENT.md` lessons 1 and 8, which
between them cost five tofu glyphs, four layout faults, a light-mode panel that
was 15.3% of the window, a velocity bar that rendered as flat colour, and a hover
box that never appeared. None of those failed a test at any point.

Phases 9, 10 and 11 each had their screen list opened and closed the same day,
which is the turnaround to aim for. What is carried forward as still owed a look:

- The Session panel's close-guard modal, and the Backups list's `Export…`.
- Whether a brightness ramp is distinguishable at 12 px.
- The three roll gestures announced *only* by a cursor icon — velocity,
  micro-timing and duplicate. `pass_cursor` can prove the roll **asked** for an
  icon, not that the platform drew one.
- The tooltip that reads "1 notes", left deliberately.

**A verification note should say whether a control was looked at or driven** —
"present" and "usable" photograph identically, and that distinction has cost this
project two bugs.
