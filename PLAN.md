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
  or `app` reaches it. This is the **box-to-box** copy — the in-app whole-track
  copy exists and is bound to Shift+C/Shift+V in the TRACKS grid
  (`core::track_clip`), so what is missing is copying between two boxes' payloads,
  not copying a track. The last item in Phase 6.
- **The track headers are short of §5's own words**: no level meter, no port
  shown, no device colour inherited by the tracks.
- ~~**Song mode does not exist.**~~ Built 2026-08-22: `core::song`, the walker in
  `Scheduler`, and the rail's fifth panel. **Nothing in it has met a box or a
  screen** — see §9, which is where that gap is the whole point. What is knowingly
  left out is ROW TEMPO (§2's argument) and syncing a song back to a box's own
  song slots, which is the next session's work.
- **Crash-safety.** Saving is manual; there is no autosave, so a crash takes the
  session.
- **MIDI import reads only the first note-bearing track**, and cannot offset it.
  The reporting half is fixed; "first track wins" needs a track chooser.
- **Paste has no caller.** `edit_ops::place_clipboard` is complete and reachable
  from nothing, because pasted notes land at the playhead and the playhead cannot
  be moved. The playhead is the prerequisite, and its own open question is what a
  drag means under polymeter.

**Honest summary:** a verified protocol foundation; a sequencer that has driven
two real boxes in sync; read, write and restore all proven on hardware from the
app's own buttons; a session that saves and reopens; and installers for macOS and
Windows that people have installed and run. What is missing is surface, not
seams.

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

/// The arrangement: rows of scenes, played in order. `Session::song` is an
/// `Option`, so a project written before song mode loads and saves unchanged.
pub struct Song {
    pub name: String,
    pub rows: Vec<SongRow>,      // up to 99, as the box's SONG ROW range is
    pub end: EndAction,          // Loop | Stop — the box's END row
}

pub struct SongRow {
    pub label: String,           // LABEL: Intro, Verse, Fill, or a pattern name
    pub scene: usize,            // PTN — a scene, not a pattern. See below
    pub repeats: u16,            // ROW PLAY COUNT
    pub length_steps: Option<u16>,          // ROW LENGTH; None = the scene's cycle
    pub muted_tracks: BTreeMap<DeviceId, u32>,  // ROW MUTE, a mask per box
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
  `[Track; 16]`. v1 shipped DT2 and DN2 profiles only; the A4 arrived on
  2026-08-28 as exactly that — a table entry plus a `sysex: None`, no model
  surgery, as predicted. Note the count this line guessed at was wrong: the A4
  is **6** tracks, not 4, because the sequencer counts the FX and CV tracks
  alongside the four voices. That the guess was wrong and cost nothing is the
  argument for the field.
- **`sysex: None` means sequence-live-only.** Such a device edits and plays
  normally over MIDI; fetch and write are unavailable, and the UI says so rather
  than failing at write time.
- **Tempo is per session; swing is per pattern.** Tempo is one clock the studio
  masters and sends to every port. Swing stays the per-pattern byte it is on the
  box, so the DT2 and DN2 can swing differently and write-back stays a
  byte-for-byte match. Note the consequence: `Pattern` has no `tempo_bpm`, but
  the DT2/DN2 pattern struct *does* (`pattern.tempo_offset`) — see §7 rule 8.
- **A song row names a scene, not a pattern.** On the box a row names one
  pattern on one box; a scene here is already one pattern per box chosen
  together, so a row that names a scene moves the DT2 and the DN2 at the same
  boundary and needs no second pattern-resolution path. `Scheduler::commit_scene`
  stays the only thing in the app that moves a box onto a pattern.
- **ROW LENGTH counts reference steps — 1/16 at 1x, session-wide** — because the
  box has one length per pattern and this app has per-track lengths and per-track
  SCALE. `None` means *the scene's own cycle*, which is a per-track fact once
  SCALE is involved and so is answered by the engine
  (`scheduler::scene_cycle_seconds`) rather than written into the model. An
  untouched row therefore behaves exactly like a queued scene change, and a
  shortened one cuts every track mid-pattern, which is what a fill row is.
- **ROW MUTE substitutes for the pattern's own mute, and does not stack with
  it.** A row can silence a track the pattern plays *and* sound one the pattern
  mutes. A box absent from the mask inherits — a third state, and not the same as
  a mask of zero, which is a user who has unmuted everything on that row and
  means it. Solo is not part of the substitution: it is the desk, not the
  arrangement.
- **ROW TEMPO is not modelled, and the SONG panel shows the session tempo as a
  read-only report instead.** The engine dates every event from one start instant
  as `next_step × step_seconds`, so moving `bpm` mid-run rescales the whole
  timeline retroactively; per-row BPM needs a piecewise tempo map through
  `engine::time`, the scheduler's cursor deadlines and clock counter, and the
  transport's elapsed→steps publish. That is a bigger job than the chaining, and
  it would also fix the existing mid-play `SetTempo` rescale. Decided
  2026-08-22 — the column is visibly inherited rather than missing.
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
- **A song row's boundary is taken in the same walk as the trigs and the queued
  scene**, and after the queued scene, so a switch the user asked for and a row
  boundary on the same instant leave the song in charge. A repeat of a row does
  *not* re-commit its scene unless the row carries an explicit ROW LENGTH: a row
  playing four times is four passes of one pattern, as on the box, so `1ST` fires
  once — while a truncated row genuinely re-launches its patterns each repeat and
  `1ST` genuinely fires again. `END: STOP` is recorded as a moment and the
  transport turns it into the stop; the scheduler has no `Instant` and cannot stop
  anything.
- **A cursor is anchored to *when* its pattern started, not only to which step**
  (`TrackCursor::origin_at`, 2026-08-22). A step's length is per track — SCALE —
  so dating events as `next_step × step_seconds` read the incoming pattern's step
  length off the outgoing pattern's step count: a 2x track switching onto a 1x one
  four steps in put the incoming pattern's step 1 at eight steps, half a bar of
  silence. **Pre-existing, and audible in pattern mode with nothing but the scene
  pill involved** — found by a song-mode test, because song mode switches scenes
  constantly.
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
| `LST` | unknowable | **simulated in song mode** — the track's last pass before the row changes. Pattern mode has no answer and the trig plays |

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
holds the editing tools — Edit, Harmony, Generate, Song, and Session, the only
place this app can save what you have been doing — one panel open at a time,
clicking the open one closing it. The right panel holds everything per-device: ports,
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
- **SONG panel** — the arrangement, in the two halves the box merges because it
  has an encoder under each column and this has a mouse: `ROWS`, one monospace
  line per row (playhead, number, label, scene, ×plays, length, mute mark), and
  `ROW`, every field of the *selected* row with real controls. Clicking a row
  selects it; the `▶` beside it moves the playhead — the box's `[UP]`/`[DOWN]`
  are a selection, not a jump. The PTN/SONG mode pill is in the transport bar's
  zone 5 beside the scene, and the same toggle is at the top of the panel: the
  mode belongs beside PLAY and the arrangement belongs beside the rows.
- **Piano roll** for the selected track. Ghosting the other tracks behind it is
  parked.
- **Trig lane** under the roll — per-step PROB/FILL/COND, ported from
  `js/triglane.js`.
- **P-lock lane strip** — ported from `js/plocklane.js`, with the parameter
  tables, the pool reader and the audition path under it.
- **The Edit panel** — velocity, length and PROB; the roll's zoom; swing,
  duplicate bar, clear; the lane list; MIDI file import and export; undo and
  redo. Every gesture at the top of `js/pianoroll.js` is ported.
- **Zoom on the roll**, which the JS has no equivalent of — it scrolls a `div`
  and the browser sizes it. Cmd/ctrl+wheel or a trackpad pinch over the grid,
  0.5x to 4x, **holding the cell under the pointer still**; the Edit panel's VIEW
  slider is the same number from the panel. `PianoRoll::zoom` was a field the
  grid had multiplied by since the roll shipped with nothing able to move it off
  1.0 — `DEVELOPMENT.md` lesson 7's second half.
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
- **The Generate panel** — genre, key, bars, progression, seed, and a row per
  part with its role, destination, density and register. The three melodic rows
  default to bass C3–C5, chords C5–C7 and lead C6–up, **an octave above the
  design's own figures**: those put a generated part low enough that every run
  wanted transposing up by hand, and the window heights are untouched, so this
  moved the floor rather than the range. A row's ↻ re-rolls that row and whatever
  answers it below.

  **Lead (call) and Lead (response)** are a pair of roles that trade phrases
  across two tracks. Row order is the pairing, as it is everywhere else here: a
  response answers the nearest call above it. The turn grid is derived from the
  pattern rather than configured — one bar trades at the half bar, two and four
  bars trade bar for bar, eight bars trade two bars at a time — and the call goes
  quiet in the response's turn, which is the feature rather than a gap. What
  travels between the two rows is what the call *actually played*, turn by turn,
  so the answer quotes, inverts or resolves a phrase that really sounded; a
  response also closes onto a chord tone where the call deliberately steps off
  the root. Avoidance was never enough for this: two parts not colliding is not
  two parts taking turns.
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
| **12** | song mode: scene chaining | ✅ 2026-08-22 — rows of scenes with play count, length, mute and END; `LST` simulable at last; **ROW TEMPO deliberately not built** (§2) |
| **13** | pattern progress indicators, by track | ✅ 2026-08-19 |

Then, all on hardware or on a screen rather than in a suite: the Setup-panel and
track-lane redesign (2026-08-19), the full UI pass (2026-08-19), a bug-fix pass
that found four faults sharing one egui mechanism (2026-08-19), a Windows
cross-compile audit (2026-08-19, §8), and three rounds of tester feedback on the
MVP1 candidate (2026-08-20) — the last of which found the only bug in this project
so far that could be *heard*: the transport reading 174 BPM while the boxes played
120. `DEVELOPMENT.md` lesson 5 is that bug's general form.

Then packaging and three releases: the `.dmg` and the Inno installer, built by CI
off a version tag and left as a draft (2026-08-21, v0.1.0); Techno as a fifth
genre (v0.1.1); and **v0.1.2, which is the first release shaped entirely by a
stretch of real use rather than by a phase** — the roll's zoom, the generator's
registers lifted an octave, every genre's basslines roughly doubled in length,
and ↻ made to do the thing its tooltip had always claimed. `DEVELOPMENT.md`
lesson 7 has the zoom and the ↻; both are that lesson's shape, and the ↻ is a
form of it this project had not seen.

**v0.1.3 (2026-08-23) is the phase release the list had been missing** — song
mode, which is phase 12 and the last unbuilt phase in §6, plus the two things
building it turned up. It is the first release in this repo written down *before*
it was tagged rather than after, which is the correction v0.1.2's own entry asked
for.

- **Song mode**, §2's `Song` and the walker in `Scheduler`: rows of scenes with
  ROW PLAY COUNT, ROW LENGTH, ROW MUTE and an END row, a SONG panel in the rail's
  fifth slot, and the PTN/SONG pill in the transport bar. **Confirmed working by
  Neil on 2026-08-23** — "pretty much exactly as expected" — which is this
  project's own standard of evidence and the only kind §9 accepts. ROW TEMPO is
  deliberately absent and the panel says so.
- **`LST` stopped being unsimulable**, because a row knows when it ends. §4's
  condition table had carried it as unsimulated "until pattern chaining exists"
  since Phase 4.
- **A four-month-old scene-switch bug**, found by a song-mode test and audible in
  pattern mode: `TrackCursor::origin_at`. `DEVELOPMENT.md` lesson 10 is its
  general form, and it is the lesson this release is actually about.
- **Clippy, installed and clean** (2026-08-23) — one deny-level defect in
  `midi::device`, about ten real tidy-ups, and eleven refusals that are now
  written down as refusals rather than as warnings nobody reads. `DEVELOPMENT.md`
  "Working on this" has the policy; the root `Cargo.toml` has the three
  workspace-wide exceptions and their reasons.

**v0.2.0 (2026-08-28) is the release that put a third box on the desk**, and
the minor bump rather than another patch is the point: every release before it
was DT2-and-DN2 software gaining features. This one is the first to support a
box the two digis' assumptions did not fit, and it found out what those
assumptions were by being wrong about them twice in one day.

- **The Analog Four, live-only and played.** Clock and transport receive, notes
  on channels 1–6, 64-step patterns, and all fourteen published CC/NRPN
  parameters swept against the box. §9's "The A4 plays" is the register entry.
  Nothing is fetched from it or written to it, and that is the box's own
  testimony rather than a gap: it advertises no `0x6x` dump request at all.
- **Two assumptions the box refuted.** That a 2013 box could not answer the
  identity API — it answers on the first try — and that a box able to name
  itself could name its dump family, which made `Product.family` an
  `Option<u8>`. The machinery built on the first guess is deleted.
- **A dead control, found by reading and confirmed by dragging it.** The A4's
  VOL fader resolved its chart through the *SysEx* spec, so it was drawn,
  draggable and silent. `DEVELOPMENT.md` lessons 4 and 5 in one defect, and the
  first time this project has had a box where "has a spec" and "has a chart"
  give different answers.
- **The kit builder gained a third box** and a caveat with it (§10): the A4
  advertises `0x53`–`0x56`, and nothing on its +Drive has been listed yet.

It is also the second release written down before it was tagged, which v0.1.2
asked for and v0.1.3 started.

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
what has actually met hardware, because a green suite cannot say. Every
entry below was recorded on a DT2 at OS 1.15B (build 0070) and a DN2 at OS 1.10D
(build 0049). Both boxes moved to 1.15C (0071) and 1.10E (0050) on 2026-08-21;
the fetch-edit-write round trip was re-run on the new OSes that day, and every
entry dated 2026-08-21 or later is on those builds. Nothing earlier has been
re-run on them.

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
- **"Read patch names", against a box that answered** — 2026-08-22, closing the
  last item this section had open. The named-slot path shipped dry-green on
  2026-08-20 by a deliberate call, with four failure modes covered by fakes and
  the live read covered by nothing; a real box has now answered a kit fetch the
  way `PatternIo`'s fake does. **Which of the two boxes was not recorded**, so
  what this closes is "a box answers" rather than "both do" — see below.
- **v0.1.2 in real use** — 2026-08-22. The registers, the longer basslines and ↻
  were all reported from a stretch of playing rather than from a test, which is
  how the three of them came to be the release.

### The +Drive and preset layer — 2026-08-26, DT2 0071 / DN2 0050

A whole day's hardware session, recorded here because most of it was ruling
things out and the negatives are what stop them being re-tried.

**Verified working:**

- **`0x63` sound fetch** — 256 fetches (128 project-pool slots × 2 boxes), zero
  timeouts, zero undecodable. First time that opcode had ever been sent.
- **`0x62` standalone kit fetch** — 22528 bytes DT2, 10752 DN2, matching the
  `KitSpec` sizes that until now nothing had exercised beyond striding between
  names.
- **`0x6b`** — the *active* kit's per-track sound, index 0–15, payload = a 5-byte
  wrapper then one whole sound struct. Confirmed against Overbridge's own KIT
  TRACK PRESETS pane, all sixteen in order.
- **The sound struct's `tagMask` at +8**, `u32be`. Calibrated exactly: DN2 pool
  slot 1 `BD BRASSY KICK` = `0x04100021` → Kick, Percussion, Noisy, Vintage,
  matching the device's own display bit for bit. `sound::TAG_NAMES` is correct.
- **The `0x53` +Drive file API, on both boxes.** `/projects` (128),
  `/soundbanks` (8 × 256), `/kits` (8 × 128). 1,189 occupied presets on the DN2,
  148 on the DT2. Every preset entry's size equals the `0x6b` payload size.

  **The inference drawn from that here was wrong, and reading a file proved it
  on 2026-08-28.** This bullet used to conclude that `0x54`/`0x55`/`0x56`
  therefore returns "a container `sound::decode_sound_dump` already reads". It
  does not: the sizes match at the *entry* level and diverge inside, where the
  struct is shorter than the payload and one of the three lengths is not in
  `KNOWN_SOUND_SIZES` at all. Two sizes agreeing is not two formats agreeing —
  the same shape as §9's level bug, where two things that photograph identically
  came apart the moment a third box asked.

**Ruled out, so nobody repeats them:**

- The project sound pool is **not** a browsable library — 0/128 named on the DT2,
  1/128 on the DN2. Presets in use live in the kit.
- A whole-project dump carries **no** sounds: 128 × `0x50` + 1 × `0x54`, zero
  `0x53`. The comment in `midi/src/device.rs` claiming otherwise is wrong.
- Dump requests `0x67`, `0x6c`, `0x6d`, `0x6e`: silent at eleven index values.
  `0x68`/`0x69`/`0x6a` answer at **index 1 and nowhere else**.
- **A dump request's payload is ignored** — 15 argument shapes, byte-identical
  replies. There is no bank/slot argument riding in it.
- `0x09` Query works (`sample_file.interleaved_stereo_support` → `Bool(true)` on
  both boxes) but **`None` means "unknown key"**, not "key exists": the empty
  key and deliberate nonsense answer `None` too. 23 preset-shaped keys: nothing.

**Two traps, both of which cost hours:**

1. **`0x53` means two things.** Under a dump header it is a Sound dump; under the
   API header (`10 00`) it is List. The DN2 advertising `50–5E` is that file
   API's opcode list, **not** dump response types — reading it as the latter is
   how this project concluded the DN2 had no +Drive. It has 1,189 presets on it.
2. **The safety rule inverts between namespaces.** "`0x5n` stores, so code that
   never sends `0x5n` cannot write" holds for the *dump* mechanism only. In the
   API namespace `0x57`/`0x58`/`0x59` write and **`0x5C` deletes**.
   `drive::assert_read_only_file_op` is the positive allowlist that replaces it.

Not attempted, and gated: the DT2 advertises seven API ids elk-herd does not
document — `0x17`, `0x18`, `0x19`, `0x28`, `0x29`, `0x36`, `0x46`. They sit in
the DirDelete/FileDelete/FileWrite families and that box holds 2.07 GB of
samples. **Do not sweep them without a +Drive backup.**

Writes to the +Drive are unexplored here and carry a known problem from the
source document: only single-chunk writes have ever succeeded, chunk *count* is
the failing variable regardless of size, and the checksum is crc32 seeded with
**zero**.

### What has not

- **A DT2 on Windows.** The WinMM path has met a DN2 (above) and no DT2, so the
  larger of the two payloads has only ever gone out over CoreMIDI. See §8.
- **Linux**, beyond the observation that the chunking is correct for ALSA.
- **`copy_track`** — it has no caller, so nothing can drive it.
- **"Read patch names" on the *second* box.** One box answered on 2026-08-22 and
  the other has not been tried, so `NotThisBox` and the DT2/DN2 difference in kit
  layout are still fake-only.
- **Decoding a +Drive preset's contents.** The *reading* half landed 2026-08-28:
  `0x54`/`0x55`/`0x56` are implemented and hardware-verified, so the browser can
  now open a preset and get its bytes. What it cannot do is *understand* them —
  the struct inside the payload is shorter than the payload (299 on the DT2, 319
  or 359 on the DN2, only one of which is in `KNOWN_SOUND_SIZES`), a DT2 file
  holds a second `BEEFBACE` at 1060, and names are Windows-1252. **So still no
  preset's tag mask has been read off the +Drive itself**, which is the claim
  that matters to §10.3 and the one that has not moved.
- **The DT2 half of the dump-index sweep.** Piped through `tail` twice and lost.
  The DN2 was the target; that data was simply not collected.
- **Paging a listing.** Every call used `start = 0, count = 0` (list everything)
  and every collection fitted one reply, so the `next_cursor` path is untested.
  Per the source document a made-up `start` returns zero entries.

### Not verified on a screen

The register that matters as much as the hardware one, because **a green suite
says nothing about what can be seen** — `DEVELOPMENT.md` lessons 1 and 8, which
between them cost five tofu glyphs, four layout faults, a light-mode panel that
was 15.3% of the window, a velocity bar that rendered as flat colour, and a hover
box that never appeared. None of those failed a test at any point.

Phases 9, 10 and 11 each had their screen list opened and closed the same day,
which is the turnaround to aim for.

**A sweep on 2026-08-22 closed the backlog this list had accumulated**, and it is
recorded as a sweep rather than as seven checks because that is what it was —
Neil's words were that he had eyeballed everything in the UI and was "fairly
sure", then named seven things: the zoom, the basslines, the melodic registers,
the pencil at 20x20, the ⓘ tooltip, the Session panel, and "Read patch names".
So **both ends of the zoom range** (0.5x, where a row is six pixels and the
velocity bar's one-pixel floor already failed once, through 4x under an
eighty-pixel step column), **the pencil**, **the ⓘ's tooltip against the right
edge of a 320px column**, and **the Session panel's close-guard modal and the
Backups list's `Export…`** are all closed. The hedge is kept on purpose: a sweep
is weaker evidence than an itemised check, and it is the right strength of claim
for what happened.

**Song mode is the whole of what 2026-08-22 added and none of it has been seen or
heard.** It ships with 27 new tests across `core`, `engine` and the link, and that
is exactly the evidence this section exists to discount. Owed, and itemised so it
cannot be closed by a sweep:

- **The SONG panel drawn at all**, at the tool panel's pinned 330px: the row
  list's monospace columns lining up, `▶` in the playhead column (it renders in
  the transport bar, so this is the same glyph in a new place), the painted
  row-order arrows, and the 16-wide mute grid wrapping rather than clipping.
- **A row list long enough to scroll** — the `ScrollArea` is capped at 190px and
  99 rows is a real number.
- **The pointer moving on hardware**: two boxes, a song of three rows, and the
  playhead mark arriving when the boxes arrive rather than when the mouse does.
- **`END: STOP` heard**, which is the one behaviour here that could be *heard*
  going wrong — the transport stopping while a note is held is the failure a user
  cannot fix from the UI, and `nothing_left_sounding` asserts it against a
  recording sink rather than against a box.
- **`LST` on a box**, against the same trig on the same pattern in song and
  pattern mode. The rule that anything unsimulated plays means a mistake here is
  a trig playing when it should not, which is quieter to notice than silence.
- **The `TrackCursor::origin_at` fix in the case that found it**: a scene switch
  between two patterns whose same-numbered track carries a different SCALE. A test
  pins the second, and half a bar of silence is the kind of thing a test can pin
  and an ear should confirm.

What is carried forward as still owed a look:

- Whether a brightness ramp is distinguishable at 12 px.
- The three roll gestures announced *only* by a cursor icon — velocity,
  micro-timing and duplicate. `pass_cursor` can prove the roll **asked** for an
  icon, not that the platform drew one. The pencil is confirmed and these are
  not: the pencil is an image this app uploads, and these are names it hands the
  platform.
- **The three states a sweep cannot walk into**, which is why they survive it: a
  note at velocity 3 at 0.5x zoom, the hover readout on an idle pointer over a
  note carrying both a PROB and a COND on the top row, and the track-cell
  tooltip's never-fetched branch — the one that must say "no patch read from the
  box" and must never present a track name as a sound name.
- The tooltip that reads "1 notes", left deliberately.

**A verification note should say whether a control was looked at or driven** —
"present" and "usable" photograph identically, and that distinction has cost this
project two bugs.

**Feature requests from users**
- transpose track
- clicking on the piano itself should sound that midi note
- draggable plock area height, users want to see the plock area taller to make changes to the settings easier
- 'check for app updates' option
-

### The Analog Four arrives — 2026-08-28, A4 OS 1.55B (build 0195)

Support for this box was written 2026-08-24, four days before it existed on the
desk, from two of Elektron's manuals. **The central assumption was wrong, and
one command found it.**

**What was assumed:** that a 2013 box predates the identity API the DT2/DN2
speak, so it would never answer `0x01` and auto-connect would have to bind it on
its USB port name alone. This produced an `answers_identity` flag, a
`NAME_ONLY_PRODUCTS` table, and an `adopt_by_name` path in `ui::autoconnect`.

**What the box does:** answers on the first try — product id **4**, name
"Analog Four", OS **1.55B**, build **0195**. It takes the ordinary handshake
path. All three of those mechanisms were deleted on 2026-08-28; only the
port-name guess (`Elektron Analog Four`) held.

**The finding worth keeping.** The same reply lists the opcodes the box
supports: `01,02,03,04,06,07,09` and then `50`–`5e`. That is every file and
store opcode and **not one `0x6x` dump request**. So the A4 is *identifiable but
not dumpable*, which had not been a distinction this codebase could express —
`Product.family` was a `u8` because every box that could name itself could name
its dump family too. It is an `Option<u8>` now. `sysex: None` is therefore
correct on the box's own testimony, not for want of a probe sweep the way the
DN2's family byte once was.

**Which also means the A4 is a third box for §10**, since `0x53`–`0x56` and the
`0x57`–`0x59` write trio are all in that list.

**Left for a session with ears on it**, and that session is the one below —
this paragraph is kept because the *shape* of what it owed is still the right
shape: clock and transport receive, live notes on channels 1–6, 64-step
patterns, and the `A4_PARAMS` CC/NRPN entries. Until they were played, the A4's
entry in this register was "it identifies", and nothing more: a param table
that is *present* and one that is *right* photograph identically, which is this
section's whole reason to exist.

**One correction to that list before anything was played.** It named "the
thirteen `A4_PARAMS` CC/NRPN entries — particularly Track Level CC 95 and
Mute", and it is wrong twice. **Track Level is not one of the thirteen**: it is
`params::track_level_midi("A4")`, deliberately outside the table because no
paramId for it has been measured (that function's own doc comment gives the
reasoning, and it is the file's central rule). And **Mute is not anywhere** —
grep the workspace and there is no CC or NRPN mapping for it on any box.
`Track::mute` is local state; `scheduler.rs`'s `audible` check simply declines
to emit the note, and nothing goes on the wire. So the thing to verify was
**fourteen claims** — thirteen table entries plus one function — and one
phantom. A verification list that names a thing which does not exist is its own
small instance of lesson 3: it sends the next session looking for a feature it
is not looking at.

### The A4 plays — 2026-08-28, second half of the day

Neil at the box, the app driven from this machine, and **nothing below was
inferred from a code path** — each line is something he reported hearing or
seeing. The rule for the session was his: "the app sent it" is not "the box
played it".

**Clock and transport receive — verified, by ear.** `GLOBAL > MIDI CONFIG >
MIDI SYNC` with `CLOCK RECEIVE` and `TRANSPORT RECEIVE` on. The app set to 174
BPM — a tempo no box here idles at, so following it cannot be confused with
happening to agree — and all three boxes started together, ran at 174, and
stopped together. The two halves were asked for separately because they fail
separately: transport is the box's play indicator following PLAY/STOP, clock is
the box running at *our* number rather than its own.

**Live notes on channels 1–6 — verified.** The factory map holds: rows 1–4
reach SYNTH 1–4, row 5 the FX track, row 6 the CV track, on MIDI channels 1–6,
with `Track::new`'s `index % 16` default and nothing configured on the box.
Driven from an authored project rather than a drawn one
(`examples/a4_test_sessions.rs`, `a4-channels.json`): six rows, one note each,
two steps apart, pitches climbing in fourths, so a note landing on the wrong
track is audible as well as visible. **Rows 5 and 6 were confirmed by sight,
not by ear** — the FX and CV tracks trig and make no sound of their own, so
"did you hear it" is the wrong question for them and the right one is whether
the box's track lights. That is the weaker of the two claims and is recorded as
the weaker one.

**64-step patterns — verified.** `a4-64-steps.json`: row 1 sixty-four steps
long with a note on step 1 and step 64, row 2 sixteen steps long with one note
on its downbeat as a ruler. The test is a **count** — exactly four ruler ticks
per lap of row 1 — because a lone note at step 63 sounds identical whether the
lap is 64 steps or the pattern is simply short. Four ticks is what the box did.

**The CC/NRPN table — fourteen claims, fourteen confirmed, nothing changed.**
Every entry in `A4_PARAMS` and `track_level_midi("A4")` was swept 0–127 on the
box, **NRPN and CC as separate sweeps**, with Neil watching the named parameter
on the A4's own screen. All fourteen moved the parameter the table names. The
two manuals were right, *including* the entries sourced from the Analog Keys
OS 1.51C appendix rather than the mk1 OS 1.0 one — CC 95 and NRPN 1/100 for
track level, the two flagged going in as likeliest wrong.

| | NRPN | CC |
|---|---|---|
| TRACK LEVEL | 1/100 | 95 |
| FLTR1 FREQ | 1/40 | 18 (+LSB 50) |
| FLTR1 RESO | 1/41 | 89 |
| FLTR1 ENV DEPTH | 1/44 | 102 |
| FLTR OVERDRIVE | 1/42 | *none, and none needed* |
| CHORUS / DELAY / REVERB SEND | 1/55, 1/56, 1/57 | 91, 92, 93 |
| PAN | 1/58 | 10 |
| AMP VOLUME | 1/59 | 7 |
| LFO1 / LFO2 DEPTH A | 1/87, 1/97 | 24 (+56), 26 (+58) |
| OSC1 / OSC2 LEVEL | 1/4, 1/24 | 69, 78 |

Three details worth more than "it moved", because none of them would have shown
up as movement:

- **The two 14-bit pairs cross the full range on the MSB alone.** `lfo1.depth`
  and `lfo2.depth` were swept with their LSB held at 0 and travelled the whole
  span, so the box reads the pair the way the table assumes.
- **Both are bipolar in the direction the table claims** — negative through
  centre to positive, so `bipolar: true` and the box's zero point agree.
- **CC 7 is a real parameter on this box.** It is AMP VOLUME here and it is
  absent from both digis' appendices, which is the one place the three boxes
  are asserted to genuinely differ. `track_level_midi`'s doc comment names CC 7
  as the obvious wrong guess *for the digis*; the A4 is the exception, and it is
  now the confirmed exception rather than the claimed one.

**Every entry keeps `plock: None`, and nothing here could have changed that.**
A paramId comes from locking a knob on hardware and reading the dump back, and
this box answers no dump request at all. Hearing a parameter and being able to
store it stay different capabilities — the same distinction the bug below turned
on, arriving twice in one day from opposite directions.

**The boring result is the point.** The table was *present* before today and is
*right* now, and this section exists because those two photograph identically.

### The bug this session found, and no test could have

**The A4's VOL fader was dead, and the field was drawn and draggable anyway.**
`EngineLink::send_track_level` resolved the box's parameter chart through
`device.model.spec()?.device` — the *SysEx* spec. The A4's `sysex` is `None`,
so the chart lookup returned `None` before it ever reached
`track_level_midi("A4")`, which has had CC 95 / NRPN 1/100 in it since
2026-08-24. Every ingredient was present and the gate asked the wrong question.

**`plocks::CuratedPLocks` had taken exactly this correction on 2026-08-24** —
its doc comment spells out the reasoning, "hearing a lane and parsing a dump
are different capabilities", and keys off `params::device_kind_key(model.key)`
instead. The same rule lived in a second place and the fix did not travel:
**lesson 5, four days apart, in two files a hundred lines from each other.**

**Why 1,507 green tests said nothing.** Every level test in
`app/tests/engine_link.rs` runs on `two_box_session()`, and a DT2 and a DN2
each have a spec *and* a chart — so "has a spec" and "has a chart" return the
same answer on both, and the wrong one is indistinguishable from the right one.
That is lesson 4's shape precisely: **a fixture that makes two different rules
agree.** It needed the third box to tell them apart, and the third box is the
first this project has ever had where the two come apart at all.

The regression test is `a_live_only_box_still_gets_its_level_fader`, and it is
the first test in the file to build a session by hand rather than take
`two_box_session()`, which is the actual lesson for the next live-only box.

**The trap that cost the first hour, and it is not the A4's.** With the **DT2
in clock-send mode, no box receives anything at all** — not the DN2, not the
A4, and not the DT2 itself. Make the DT2 a slave and all three follow the app
immediately. Worth writing down because of how it presents: it looks exactly
like the app failing to send, it is not specific to the box you just plugged
in, and the natural response is to start debugging the newest thing on the desk
rather than the oldest. The app is not involved in the mechanism — it never
listens to incoming clock (`EngineLink::send_clock` is the only clock decision
it makes, and it is a send) — so a second master on the desk is a desk problem
that this register should name rather than a defect to chase in code.

### `0x5b` stores one sound onto one track — 2026-08-28, DT2 0071

§10.6 step 3, and the answer is **yes**. `0x5b` with a track index is a
per-track sound store into the box's **active kit**, exactly as the
response-is-request-minus-0x10 rule predicted from the working `0x6b`.

- **The accepted payload is the `0x6b` payload verbatim**, 5-byte wrapper
  included — the same relationship `store_pattern_kit`'s `0x50` has with `0x60`.
  It worked on the first shape tried, so the *struct only* variant the probe
  carries as its alternative has never had to run and remains untested.
- **Verified on both witnesses.** The bytes read back as the name that was sent,
  and the DT2's own screen showed it. That is §9's standard, and this section
  claims nothing the screen did not also say.
- **Restoring is the same call**: re-sending the original bytes puts the track
  back, confirmed by re-read.
- **Nothing was audible or destructive**, because the probe changed only the
  16-byte name at struct +12 and carried every machine and parameter byte
  verbatim. `examples/probe_sound_store.rs` is kept, the way
  `probe_drive_read.rs` and `browse_drive_dn.rs` were.

**The trap this probe had to be built around, and it is not the box's.** A store
gets no reply, so the only evidence is a re-read — and if a box has MIDI thru or
port echo on, **our own `0x5b` comes back at our input carrying the name we just
sent**, which `fetch_kit_track_sound` cannot tell from the box answering. It
fails in the one direction that matters: it manufactures a *positive*.
`device.rs`'s header records this for loopback ports; a box with thru enabled is
the same hazard on a real cable. The guard is **two reads that must agree** —
`fetch_dump` drains before it sends, so a one-shot echo cannot be seen twice.
Any future store probe in this namespace needs the same guard, because every one
of them will be verified by re-reading.

**A pause nothing drives is not a pause.** The first run restored the name a
fifth of a second after storing it, so the screen witness this design rests on
was unreachable — the run printed as though somebody had looked. The fix is
`--hold`, and it is a **clock rather than a keypress**: these probes are launched
through a pipe with no terminal on stdin, where `read_line` returns end-of-file
immediately and the hold silently does not happen. Same shape as the cliclick
note in `DEVELOPMENT.md` — a wait that nothing drives is not a wait.

The **A4** cannot be tried at all — it answers no `0x6x` dump request, so it has
no `0x6b` to mirror, and the probe skips it with that reason rather than
silently.

### The DN2 says the same thing — 2026-08-29, DN2 0050

The second box, which is what turns one box proving an opcode into an opcode.
Same probe, unchanged, `--write --box Digitone --hold 45` on track 16.

- **Positive on both witnesses again.** Two agreeing reads returned `PROBE5B`,
  and the box's own screen showed it during the hold. Restore confirmed by
  re-read: T16 back to `PRESET 16`.
- **The struct size does not matter to the opcode.** The DN2's payload is 364
  bytes to the DT2's 1114 — 359 and 1109 behind the same 5-byte wrapper — and
  the store took the wrapped shape on the first try on both. §10.1's "the DN2 is
  its own column" was right about sizes and wrong about nothing here: the
  opcode's *shape* is common, only the length differs.
- **So `0x5b` is the load path for both digis**, not a DT2 finding the DN2 was
  assumed to share.

**What is still untested is the same thing on both boxes.** The wrapped payload
worked first on the DT2 and first again on the DN2, so the *struct only*
alternative `probe_sound_store.rs` carries has now failed to run twice. It is
untested rather than rejected, and two positives on the wrapped shape are a
reason to stop caring about it, not evidence against it.

**Nothing new was learned about the traps**, which is worth one line: the echo
guard passed silently, the `--hold` clock did its job with nobody having to
press anything, and neither had to be rediscovered. Both were built on the DT2
run the day before, and this run is what a register earns you.

### What 24 preset files say about the container — 2026-08-29

`capture_drive_presets.rs`, eight presets from `/soundbanks/A` on each of the
three boxes, committed under `tests/fixtures/drive/` with a manifest. Read-only,
through the same allowlist. Four findings, and the third is the one that matters.

**The layout is common to all three boxes.** Head magic, then a u32 at +4, then
the tag mask at +8, then the name at +12 — the A4 included. `THE SAW` reads out
of `be ef ba ba | 00 00 00 05 | 05 84 00 03 | 54 48 45 20 53 41 57` exactly as
§9's struct map predicts, so `BEEFBABA` is the same shape under a different
magic rather than a different format.

**Names are Windows-1252 and decode cleanly** — `BLÅ VIND`, `BLÅ LOFI BASS`,
`SYNTHVÅG`. The mangling recorded on 2026-08-28 is `chars16`'s encoding
assumption and nothing about the file. That is a decoder to write, not a format
to reverse.

**The A4 has no foot magic anywhere in the file, and that is the blocker.**
`BACEF00C` appears at 331 in every DT2 file and at 351 or 391 in every DN2 one,
and **not once in any A4 file.** `decode_sound` calls the foot check "the
point" — it is what makes guessing a size safe — so the A4 cannot be decoded by
relaxing the head magic, which was the obvious fix and is the wrong one. The
third box again separates two rules that looked like one, for the third time in
four days.

**Struct size is not a per-box constant, so `KNOWN_SOUND_SIZES` is the wrong
shape.** One DN2 bank holds both 319 and 359, and the size tracks the u32 at +4:
`0` → 319, `1` → 359. The DT2 is 299 at `0`, so that word is scoped per box and
is not the dump struct's version field, which §9 records as DT2 3 / DN2 2. **The
size should be found by locating the foot, not by consulting a table** — which
works on both digis and, again, not on the A4.

| box | container at | magic | +4 | struct | file |
|---|---|---|---|---|---|
| DT2 | 36 | `BEEFBACE` | 0 | 299 | 1157 |
| DN2 | 36 | `BEEFBACE` | 0 / 1 | 319 / 359 | 407 |
| A4 | 31 | `BEEFBABA` | 5 | **no foot** | 409 |

Every file is `declared + 43` bytes on all three boxes, declared being the size
at +27 — so the 43-byte tail is a constant worth naming and is not yet read.

**Tag masks came back populated and varied** (`0x00902000`, `0x04880804`,
`0x0400c088`), which is what §10.3's index needs and the reason the captures
carry real names rather than blanked ones.

## 10. The kit builder — scope, 2026-08-26

Phase 14, and the first phase scoped on top of a hardware session rather than
ahead of one. §9's +Drive entry is the evidence base; this is what to build on it.

The user workflow, as asked for: open a preset browser for the selected box and
track, see the +Drive's presets by bank with their tags, double-click one to load
it onto the current track, and once up to sixteen are placed, save the result as
a kit on the device.

**v1 is browse and load. Kit saving is deferred**, and when it comes it targets a
`/kits/<bank>` file on the +Drive — the destination decided 2026-08-26, because
that is what "saves to the device" means to someone who has used Transfer. §10.4
records what that will cost, so the deferral is a schedule decision rather than an
open question.

**There are three boxes for this now, not two.** This section was scoped on
2026-08-26 against a DT2 and a DN2. The A4's supported-opcode reply lists
`0x53`–`0x56` and the `0x57`–`0x59` write trio, so it publishes the same
`0x53` file API — which is a separate question from its having no `0x6x` dump
request at all, and the whole reason `Product.family` became an `Option<u8>`.
**The A4's +Drive was read on 2026-08-28** and it is now a third column rather
than a reason to point a probe: it lists, opens and reads. Its container magic
is `BEEFBABA` where the digis' is `BEEFBACE` and its header is 31 bytes where
theirs is 36 — the third box again telling two rules apart, and the reason
`container_offset` finds the magic rather than trusting a constant. Note also
that the A4 is
`sysex: None`, so it has no `KitSpec` and no `Spec::device` — §10's code must
key parameter and preset lookups off the **model key**, the way
`plocks::CuratedPLocks` and (since 2026-08-28) `EngineLink::send_track_level`
both do. That is the trap §9's level bug already sprang once.

### 10.1 The three steps are not equally built

| step | protocol | what is missing |
|---|---|---|
| browse | List, read and the container layer all ship, verified on all three boxes | a tag index; the A4 decodes not at all |
| load | `0x5b` per-track sound store, hardware-verified on both digis | the panel and audition mode; the A4 has no path at all |
| save | nothing | everything, plus a widened write guard |

### 10.2 Reading a preset — the real v1 work

**The transport half of this is done** — `0x54` Open, `0x55` Read and `0x56`
Close were derived by probe and implemented on 2026-08-28, and
`drive_read_file` returns a preset's bytes on all three boxes. The probe was
needed because the source document names the opcodes and specifies the argument
layout of none of them; `probe_drive_read.rs` is kept, the way
`browse_drive_dn.rs` was.

**Built 2026-08-29: `drive::decode_drive_preset` turns a preset file into a
`Sound`.** §9's entry of the same date is the evidence base — 24 files across
three boxes — and the layer is pinned against those files by
`tests/drive_preset.rs`. Two of the three jobs were smaller than recorded and
the fourth thing is the one that stayed open:

- **The struct is measured, not looked up.** `struct_size` finds `BACEF00C`
  after the head, and the length falls out of where it lands. `KNOWN_SOUND_SIZES`
  could never have carried this: one DN2 bank holds both 319 and 359.
- **Names are Windows-1252, and that was a one-line fix in the wrong file.**
  `chars16` used `from_utf8_lossy`, so every byte over 0x7F became U+FFFD and
  `BLÅ VIND` read as `BL? VIND`. The listing side had used `cp1252_char` since
  it was ported — elk-herd's `argString0win1252` — so **the two halves of this
  crate disagreed about the box's encoding for three days** and only a file with
  an `Å` in its name could show it. `chars16` now shares the one decoder.
- **The A4 is refused by name**, `DriveError::UndecodableContainer`, rather than
  falling out as a `BadHead` that reads like corruption.

**The open question, and it is not small: the A4 has no foot magic at all.** Not
once in any of eight files, which `no_a4_capture_contains_a_foot_magic_anywhere`
asserts directly so that a firmware which starts emitting one is a failing test
rather than a discovery nobody makes. The foot is what lets `decode_sound` trust
a size, so the A4 needs its extent established some other way — the `+4` word,
the 43-byte tail, or a capture of Transfer reading one.

**The trap that shaped the API, and it is worth naming.** The obvious fix was to
let the head magic vary, since `container_offset` already accepts both. That
would have "worked": an A4 preset would decode to a plausible name, a plausible
tag mask and a **wrong length** — silently, and exactly what the foot check
exists to prevent. It is the `0x5b` echo hazard's shape in a parser: the failure
manufactures a positive. `an_a4_preset_is_refused_as_the_a4_rather_than_as_corruption`
is the guard, and it asserts the specific error for that reason.

A DT2 file also carries a **second `BEEFBACE` at 1060**, so finding the magic is
not the same as finding the sound — `container_offset` takes the first, and a
test now says so before someone turns it into a reverse search.

Two things are already known and make the rest cheap. Every preset entry's size
equals the `0x6b` payload size, so what comes back is a container
`sound::decode_sound_dump` reads today, tag mask included. And the read path
reports one crc32 per chunk — seeded with **zero**, not all-ones — which
reproduces cleanly per slice.

### 10.3 Tags are not in the listing, and that shapes the whole browser

A `ListEntry` carries name, index, size, permissions and occupancy. It does **not**
carry a tag mask. Tags live at sound-struct `+8`, inside the file. So the panel the
user described — banks down the side, `Bass-Glitchy` visible before you click —
cannot be drawn from a directory listing. It needs every preset read: **1,189 on
the DN2**, 148 on the DT2.

That is a scan, not a browse. So:

- The index is built once, persisted, and keyed by device *and* bank, so a second
  open of the panel is instant and a box that gains presets can have one bank
  rebuilt rather than all eight.
- The scan is cancellable and shows progress, because it is the longest-running
  read this app has ever done and it is the first thing a user meets.
- A bank with no index yet still lists — names and slots work immediately, tags
  fill in behind. Browsing must never block on tagging.
- `Sound::tags()` and the calibrated `TAG_NAMES` are already right, so the filter
  UI is a bit-mask test and nothing more.

**Paging is untested.** Every call in §9 used `start = 0, count = 0` and every
collection fitted one reply, so `next_cursor` has never run. A 256-entry bank is
the case that will find it.

### 10.4 Loading onto a track — where "instantly" goes wrong

The splice itself is trivial and already proven by the sizes: a preset file is
`5 + sound_size`, a kit slot is `sound_size` at `kit_base + 60 + track*sound_size`,
and DN2 359 / DT2 1109 match exactly on both sides.

The problem is that **there is no "load a sound onto a track" store path in this
codebase.** The only store is `store_pattern_kit`, an `0x50` carrying the whole
pattern and kit, through the full `safe_write_tracks` ceremony — re-fetch, confirm,
stash, backup, encode, send, read back, compare. Wired to a double-click that is
one backup per audition, and the ring holds fifty. Twenty auditions evict twenty
real backups: precisely the failure `safe_write_tracks`'s own doc says it was made
plural to avoid, arrived at from the other direction.

**Decided: audition mode.** One backup when the kit builder opens, then loads run
without a per-click backup or dialog. Recovery is to the state the panel opened in,
which is the honest unit here — nobody wants to step back through nineteen
auditions. The panel must say so plainly; a quiet backup policy change is worse
than a slow one.

**The probe that makes this genuinely instant — run 2026-08-28, and it works.**
`0x6b` returns the *active* kit's per-track sound for index 0–15, a 5-byte
wrapper then one sound struct. Every store in the dump namespace is its request
minus `0x10`, so `0x5b` with a track index was the shape a per-track sound store
would take. **It is that store.** §9's entry is the evidence: the payload goes
out exactly as `0x6b` returned it, the box's own screen shows the change, and
re-sending the original bytes restores.

So the load path is decided: **a double-click is one ~1 KB `0x5b`, not a 127 KB
`0x50`.** That buys most of the problem this subsection was written about — a
per-audition *pattern* write is what made backup-per-click ruinous, and at a
kilobyte the ring pressure goes with it.

`ElektronDevice::store_kit_track_sound` is the call, and its doc carries the
caveat that outranks the convenience: **the active kit is a working buffer.**
There is no `0x50` that puts one back, so what makes a load recoverable is the
box discarding an unsaved kit when the pattern is reloaded — hardware behaviour,
not a backup this code took. Audition mode still stands and the panel must still
say plainly that recovery is to the state it opened in. What changed is that a
load is now cheap, not that it is free.

**One thing this does not settle.** Only the *wrapped* payload shape has ever
been accepted — it worked first on the DT2 and first again on the DN2, so the
struct-only alternative the probe carries has still never run. The DN2 half is
no longer open: `0x5b` was confirmed there on 2026-08-29, same shape, both
witnesses, 364-byte payload against the DT2's 1114.

Note also that `write_gate` keys on an OS-build allowlist, so loading is refused on
any build not yet write-verified. That is correct and should stay; the panel needs
to explain it rather than grey out silently.

### 10.5 What saving a kit will cost, when it comes

Recorded now so v2 starts from evidence:

- **Multi-chunk +Drive writes are broken.** Six chunks at 2,048 and two at 8,192
  both failed with `Invalid package checksum; corrupt transfer`, under every
  checksum variant tried. Chunk *count* is the failing variable, not size. One
  16,384-byte chunk committed and read back byte-exact.
- A DN2 kit is **10,795 bytes** and fits that single chunk. A DT2 kit is roughly
  17.8 KB and does not. So DN2 kit saving is reachable on today's knowledge and
  DT2 kit saving is blocked on a reverse-engineering problem with no timeline —
  settling it wants a capture of Transfer writing a multi-chunk file.
- `assert_read_only_file_op` will have to admit `0x57`/`0x58`/`0x59`. It currently
  admits List/Open/Read/Close and the module doc justifies that by saying there is
  no kit-builder reason to mutate a +Drive. **That sentence stops being true in
  v2**, and the guard should be widened deliberately, opcode by opcode, with
  `0x5A` Move, `0x5B` Copy and `0x5C` **Delete** still refused. The safety property
  in this namespace is the allowlist and nothing else.
- `0x59` WriteClose is the commit. Nothing lands without it, which makes an
  abandoned write harmless and is worth relying on.
- Verify-after-write earns its keep here more than anywhere: a chunking bug that
  silently truncates is exactly this API's failure mode. A stored kit stamps its
  own slot index at container byte `+24`, so a correct copy into a different slot
  legitimately differs in that one byte and a naive comparison will cry wolf.

### 10.6 Order of work

1. ~~Probe `0x54`/`0x55`/`0x56` argument layouts against a box; derive, then
   parse.~~ **Done 2026-08-28**, derived on all three boxes and the same answer
   from each — `probe_drive_read.rs` is kept. Read addresses a chunk by
   **sequence number, not a byte offset**, which is the one place the renumbered
   API is a genuinely different call rather than gen-1 with new opcodes.
2. ~~`ElektronDevice::drive_read_file`, read-only, guarded as `drive_list` is.~~
   **Done 2026-08-28**, `device.rs:716`, verified on fifteen files across three
   boxes with three independently-sourced sizes agreeing on every one. The
   *parse* half of step 1 is what is left, and it is the container layer that
   step 4 needs — see §10.2.
3. ~~Probe `0x5b` as a per-track sound store (§10.4). Decide the load path on
   the result.~~ **Done — positive on a DT2 2026-08-28 and on a DN2 2026-08-29**,
   see §9 and §10.4. The load path is `0x5b` on both digis, wrapped payload,
   and the A4 has no `0x6b` to mirror so it has no load path at all.
4. The tag index: scan, persist, cancel, resume per bank. **Unblocked
   2026-08-29** — `decode_drive_preset` gives a tag mask per file, so the scan
   has something to scan. Digis only: the A4 lists and does not decode.
5. The panel — sixth rail slot, following `Sidebars`/`Tool`, worker thread and
   `mpsc` like `transfer.rs` and `sync.rs`.
6. Load-to-track on the path step 3 chose, with audition mode and its backup.
7. Exercise paging on a 256-entry bank.

Steps 1–3 are hardware work and cannot be done from a desk without a box, and
all three are now done. Steps 4–7 can be built
against fixtures and only need a box to be believed — which is
§9's standard, and the one this project keeps.
