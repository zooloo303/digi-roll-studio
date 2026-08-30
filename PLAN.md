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
- **`generator::default_parts()` ships bass, chords and lead and no drum voice**,
  so hearing drums at all means adding every voice by hand each time. Raised by
  Neil on 2026-08-19, the day Phase 7's exit criterion was met, and parked rather
  than guessed at: **how many voices, which tracks and which box are all open**.
  Ask before picking it up — the answer decides whether the default set assumes a
  two-box desk.
- **"Read the kit the box has loaded *right now*" is a wire question, not a UI
  one.** The Setup panel's picker asks for a *stored* slot, which is the honest
  thing this protocol supports. There is no working-buffer dump request anywhere
  in `digi_protocol`, and nothing listens for the Program Change a box sends when
  its pattern changes — so "whatever it is on, whatever slot that is" needs
  protocol work before it needs a control. Worth knowing before anyone promises it
  in a tooltip.

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
  matching the device's own display bit for bit.

  **Extended to all three boxes on 2026-08-29, and the table split in two.** See
  "The tag tables are calibrated on three boxes" below: one global `TAG_NAMES`
  was right for the digis and wrong for most of an A4.
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

  **True of the two digis, and generalised to "boxes" too quickly.** The A4's
  project sound pool *is* populated: its Overbridge Sound Browser shows at least
  25 named, tagged sounds in it — `KICK L`, `SNARE ARM`, `303CLONE`, `GOOMY
  BASS` — carrying the same tag vocabulary as the +Drive library. Not
  investigated, and flagged rather than acted on: it may be a second browsable
  source on that box. Noted 2026-08-29. This is the fourth time the third box
  has split a rule that read as one.
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
- **REVERT, and the load's four refusals.** The load itself is done — a
  double-click put a preset on a track of a DT2 and of a DN2 on 2026-08-29, see
  below — and what that run did not touch is everything that is *not* the happy
  path: putting a track back, an mk1 preset being refused by name, the A4's
  refusal being legible, and the OS-build gate speaking through this path. **The
  distinction is the whole point of this register**: "it loads" and "it refuses
  properly" are different claims, and only the first has been made by a box.

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

### The Presets panel on three boxes — 2026-08-29, DT2 0071 / DN2 0050 / A4 0195

Neil's session, the first time anything in §10 met hardware from inside the app.

> **The two buttons were renamed on 2026-08-29**, after this session and the one
> below it: REFRESH is now **LIST** and SCAN is **READ TAGS**. This record and
> the screenshot notes keep the names of the day rather than being rewritten —
> see §10.6 step 5 for why they moved.

**Working:** on the DT2 and the DN2, pick a bank, REFRESH, SCAN — names and tags
come back for that bank's presets, and the tag chips narrow the list. That is
§10.6 steps 2, 4 and 5 confirmed end to end against two boxes.

**The A4 lists and refuses to be tagged, which is the design** — REFRESH returns
its patch names, SCAN stops at the first preset. Its tag calibration is the next
session's work.

> **Closed the same day.** The calibration landed on 2026-08-29 and the A4 now
> scans and tags like a digi; see "The tag tables are calibrated on three boxes"
> above and §10.2. The refusal *state* is still the design and still reachable —
> for the next box with an unmapped container, not for this one.

**Two things the session changed, and neither was a bug in the protocol layer:**

1. **The browser was scoped to one bank and it should never have been.** See
   §10.6 step 5 — the picker now opens on ALL and the search crosses the library.
2. **The A4's refusal answered by deleting its own button.** Also §10.6 step 5.
   Worth keeping as a general note: *a control that disappears is not a reply to
   the press that made it disappear.* The state was right and the reporting of it
   was not, and no test could have found that — the state machine was correct.

### The scan that tagged nothing — 2026-08-29, DN2 0050

**`Tagged 0 preset(s), 388 skipped in 2s`**, and then, on the next run,
**`first skip — /soundbanks/B/205: no sound container magic in 407 bytes`**.
Recorded in full, including a wrong diagnosis, because the wrong one is the
instructive part.

**The arithmetic, from the index files:** DN2 banks A (256/256), E (162/162) and
H (3/3) were complete; B held 204 of 256, C held 48 of 256, D held 128 of 256.
801 tagged, 1,189 occupied, **388 missing — and 388 skipped.** `scan_bank`
selected exactly the right work and every read of it was passed over.

**The wrong answer, believed for one round.** A pre-pass added the same day asked
all eight banks for their slots before reading anything, to show one library-wide
count — eight extra List round trips, and the only change to the read sequence
between scans that had worked and one that did not. `drive_read_file` is
Open/Read/Close with no recovery, so a failed Open never Closes and every later
read fails: a tidy cascade that explained "zero successes" perfectly. It was
wrong.

**What it actually is.** 407 bytes is *exactly* the length of a good DN2 preset
file — every committed capture is 407 with `BEEFBACE` at offset 36. So the read
**succeeded** and the **decode** failed: those slots hold something this parser
does not recognise. The per-bank scan history says the same thing and had done
all along — bank D indexed 1–100, skipped 101–228, then indexed **229–256**. A
box whose transfer session had died does not come back for the last 28.

**Two things this leaves, and one of them is the real defect:**

- **Closed the same day: those 388 files are Digitone mk1 presets.** Their
  container magic is ASCII `DN1S` at byte 31 — flush with the payload like an
  A4's, not five bytes in behind a `SOUND_WRAPPER` like the DN2's own — with a
  foot at 329 and so a 302-byte struct against the native 319. **One box's
  library holds two container formats**, 388 of 1,189, which is the common case
  rather than a curiosity. See "A DN2's +Drive is two formats" below.

  Two corrections fell out of it. The wrapper is a property of the **file**, not
  of the box that answered — a DN2 serves both kinds from one +Drive — and the
  head-bytes diagnostic that was supposed to settle this **printed 16 bytes,
  which is exactly the prefix every DN2 file shares**, so its first run proved
  only that a file had arrived. At 48 it named the format immediately.
- **Closed: a skip that could not be read.** `ScanReport` counted skips and
  discarded every reason, so 388 of them said nothing, and the first diagnosis
  had to be reconstructed by parsing index files with Python — which produced a
  confident wrong answer. `first_skip` carries the box's own words, the panel
  prints them under the count, and **a run that tags nothing is red rather than
  amber**: everything skipped is a failed run wearing a partial success. The
  lesson is not about presets. **A count of failures is not a diagnosis, and an
  error that reports only a magnitude will be guessed at.** Both fixes landed
  before the cause was known, and the second one is what found it.

Two smaller things the same session settled: a bank with nothing left to do no
longer rewrites its index file, and DN2 banks F and G hold no presets, so the
1,189 live in six banks and not eight.

### The Presets panel on a screen — 2026-08-29, two shots

Taken with the `Sidebars::default` flip, and with a real index seeded from the
committed captures through `PresetIndex` so the populated state was the app's own
read path rather than a mock. **Closed by looking:**

- **The rail draws six rows** — Edit, Harmony, Generate, Song, Presets, Session
  — with Presets marked by the cyan left border and no clipping at 86px. The
  placement decision reads correctly: the composing tools run together and
  Session stays at the foot.
- **Both empty states draw** at the pinned 330px, and the buttons behave: SCAN
  is offered on an unscanned bank and **gone** on a complete one, which is
  `Tagging::offers_scan` doing its job where it can be seen.
- **The tag chips wrap onto two rows rather than clipping** — eight of them at
  330px, `Percussion 1 … Soft 1`.
- **`BLÅ LOFI BASS`, `BLÅ MEOW`, `BLÅ SQ CHIP` and `BLÅ VIND` render with their
  Å**. The Windows-1252 fix of the same date had never been seen on a screen;
  the whole point of that bug was that only a file with an `Å` in its name could
  show it, and this is that file drawn in the app.

**And one thing the shot found that no test would have.** The BANK header's
caption was the whole sentence `No tags yet — SCAN reads every preset in this
bank`, which squeezed out the section rule and said very nearly what the TAGS
section says an inch below. Split: `Tagging::caption` is the count for the header
(`not scanned`, `8 tagged`, `412 of 1189 tagged`) and the explaining happens once,
in the section that is about tags. This is `DEVELOPMENT.md` lesson 8 paying for
itself again — a green suite and duplicated prose photograph identically.

**A second, unlooked-for result: the tag calibration is corroborated on a DT2.**
§9 calibrated `TAG_NAMES` against one DN2 preset's display. The eight DT2
captures decode, through those same names, to `BLUE HH` → Hi-Hat, `BAM BASS` →
Bass, `BAM TICK` → Percussion, `ACIDD` → Synth, `BLÅ MEOW` → Sound Fx and `BLÅ
VIND` → Texture, Noisy, Soft. Eight for eight semantically, on a second box, from
files nobody chose for this purpose. Not a calibration against a display and so
not a replacement for one — but it is exactly the check a wrong bit order would
have failed, and it did not.

### The tag tables are calibrated on three boxes — 2026-08-29

**24 of 24, exact, and the source was a screenshot rather than hardware.** Neil
captured Overbridge 2.26.9's Sound Browser for an A4, a DT2 and a DN2. Each shows
the whole 32-cell filter grid *and* a tag column for the presets beside it — and
the presets on screen are `/soundbanks/A/1..8`, which are exactly the 24 files
already committed under `tests/fixtures/drive/`. So both sides of the check were
produced by different software reading different copies of the same data, which
is the only reason it is worth asserting. Pinned by
`every_capture_decodes_the_tags_its_box_displays`.

**The grid *is* the bit order.** Read left-to-right, top-to-bottom, its 4×8 block
is bit 0 through bit 31, on all three boxes.

**The two digis share one table exactly; the A4 has its own.** That is measured,
not assumed — the DT2's and DN2's grids are the same 32 names in the same 32
positions, so `TAG_NAMES_DIGI` serves both. The A4 barely overlaps:

| bit | digis | A4 |
|---|---|---|
| 0 | Kick | **Bass** |
| 1 | Snare | **Lead** |
| 10 | Bass | Kick |
| 11 | Lead | Snare |
| 18 | Acoustic | **Hard** |
| 22 | Hard | **Dark** |
| 25 | Bright | **Acid** |
| 29 | Loop | **Input** |

**Exactly two of the thirty-two positions agree** — Mine at 30 and Favourite at
31. Every other bit means something else. Names do recur across the two
vocabularies (Noisy, Glitch, Bass, Kick) but never in the same place, and that is
worse than no overlap at all: it is what lets a mis-decoded mask read as an
ordinary list of tags. So `tag_names_for` keys on the identity slug and there is
**no default table** — a box whose grid nobody has read (the mk1 `digitakt`)
names nothing at all, which renders as a mask with no labels rather than as a
confident lie.

**What made this exact rather than exact-looking, and it is a method worth
keeping.** Three photographs of the A4's own screen got *three* positions wrong —
bit 7 read as "STAB" (Strings), bit 14 as "AMB" (Atmosphere), bit 25 as "ARP"
(Acid) — because the A4 truncates its tag row at four entries, so `THE SAW` shows
four tags and carries six. §9's standard is the device's own display and here it
was **not sufficient**. A desktop editor rendering the same data settled it in one
screenshot, because it lays all 32 cells out at once and truncates nothing. The
photographs were enough to be confident and not enough to be right; using both is
what closed it.

**The failure this guards against does not look like a failure.** `THE SAW`'s
mask through a digi's table reads Kick, Snare, Acoustic, Soft, Dark, Vintage —
six real tag names, the right count, five of them wrong, one right by
coincidence. There is nothing in that output to notice. It is §9's standing
lesson in a new place: a wrong answer with the right shape.

### Three libraries indexed, whole — 2026-08-29

The scan run against all three boxes, end to end, after the A4 calibration and
the `DN1S` container landed:

| box | build | presets indexed | banks |
|---|---|---|---|
| Digitone II | 0050 | **1,189** — the whole library, both formats | A–E, H |
| Analog Four | 0195 | 869 | A–D |
| Digitakt II | 0071 | 148 | A |

**This is the entry §10.6 step 4 was waiting for.** The index persists, a second
open is instant, and a second scan resumes to a no-op — which is the property
that made the `DN1S` fix cost one button press rather than a nine-minute rebuild:
the 388 had never been recorded as *failures*, only as absent, so they were still
missing and the resume picked them up.

Worth keeping in mind before "record the failures" gets built, which was the
obvious next improvement when a run reported `Tagged 0 preset(s), 388 skipped`
for the third time. Recording them would have made that message honest and would
have **blocked this fix from reaching an existing index**. A parser gap and a
genuinely unreadable file look identical at the moment of failure and differ
entirely afterwards, so anything persisted about a failure has to be keyed to the
parser that produced it.

### A DN2's +Drive is two formats — 2026-08-29

**388 of a Digitone II's 1,189 presets are Digitone *mk1* sounds**, and they are
a different container, not a corrupt one:

```text
              magic       at   ver  foot   struct
  DN2 native  BEEFBACE    36   0    351    319     wrapper, then the container
  DN2 mk1     DN1S        31   4    329    302     flush with the payload
  A4          BEEFBABA    31   5    none   366     flush, sized by the header
```

`DN1S` is ASCII — the only container magic on any box legible in a hexdump, and
the thing that named the format the moment enough bytes were printed to see it.
The fields sit where every other container puts them: version at `+4`, tag mask
at `+8`, name at `+12`.

**The tag vocabulary is the DN2's own, and that had to be checked rather than
assumed.** The expected answer was a third table — the A4 had just cost a day by
being one — and the masks read through `TAG_NAMES_DIGI` looked *suspicious* in
the way a wrong table looks: `Cowbell` set on four of the first five files. It
was not a mis-decode. Overbridge lists these presets in the DN2's own browser
under the DN2's own 32-cell grid, and `/soundbanks/C/1..8` decode to exactly the
tags it shows, 8/8 exact and in order. The box re-maps mk1 tags into its own
vocabulary. That library really is tagged Cowbell a lot.

**So the rule that generalises is about routing, not about boxes.**
`container_offset` searching for the magic — rather than trusting a per-box
constant — is what made a third format a two-line change. And
`decode_drive_preset`'s `_ =>` arm, written as "unreachable today, deliberately
kept", is where `DN1S` was diagnosed: it carried the magic that named it.

### A DN2 ignores an mk1 sound under `0x5b` — 2026-08-29, DN2 0050

**The probe:** `examples/probe_mk1_store`, written for one question. §10.6 step 6
refused Digitone mk1 presets on the reasoning that nobody had ever handed one to
a box under a store opcode — which is a good reason to refuse and a bad reason
to *keep* refusing, because 388 of a DN2's 1,189 presets are mk1 and the refusal
is a third of a library.

**What made it worth asking rather than assuming.** An mk1 payload is 364 bytes
and a native DN2 payload is 364 bytes, so the length check in `midi::preset_load`
— the one that asks the box's own `0x6b` reply how long a payload should be —
*passes* for an mk1 preset. Nothing about the size refuses it. And the DN2
clearly uses these sounds: it lists them, loads them from its own browser, and
re-maps their mk1 tag bits into its own vocabulary. A conversion path exists
inside the box; the question was whether `0x5b` is on the near side of it.

**Answer: no change.** A real mk1 payload (`/soundbanks/B/205` — the same preset
whose failure to decode produced the 388-skip diagnostic) went out under `0x5b`
and the track kept the native preset that had been put there as a
before-picture. **The control is what makes this a refusal rather than a dead
box:** the very next store on the same track in the same session — the restore —
was accepted and verified. So the box parses the head magic and declines.

**Two gates were refusing, and only one of them should have been.** The first
attempt never reached the wire: `plan_track_sound_store` rejected the payload
because `decode_sound_dump` knows one head magic, so the guard that asks *are
these bytes a sound* was silently also answering *is this a format this box
takes*. An mk1 payload validates exactly as strongly as a native one — `DN1S` at
+0, `BACEF00C` at its measured end, both checked by `decode_dn1_sound`. The two
questions are now separated: the store guard validates bytes, and
`drive::preset_load_payload` owns the format policy where a caller can see it.
Nothing in the app can reach the store with an mk1 payload; the probe can, which
is the point.

### The load runs on both digis — 2026-08-29, DT2 0071 / DN2 0050

**Neil's words: "dbl-click loads the selected patch to DT2 and DN2."** So the
whole path is proven end to end on the two boxes that have one — the gesture,
the track the roll's selection resolves to, the payload cut out of the +Drive
file, the length check against the box's own `0x6b` reply, the store, and the
read-back. This is what §10.6 step 6 was waiting for and it landed the same day
it was built.

**The A4 does not load, and its refusal is legible** — confirmed by Neil the
same day: double-clicking an A4 preset shows the LOAD section explaining that the
box has no such message. That is decision 4's lesson holding on the exact box
that taught it, and it was the item on this list with a precedent for going
wrong.

What one more session should close, in the order that a wrong answer would
matter — **none of it touched by the run above**, which exercised the happy path
on two boxes and nothing else:

- **The mk1 marks, which have never been drawn at all.** Not merely unverified
  on hardware — *unverified on a screen*, which by §9's own standard is the
  weaker position of the two. The shot was attempted on 2026-08-29 and the app
  came up windowless after too many relaunch cycles, which
  `screenshot-a-panel-without-clicking` records as a known limit and which no
  amount of green tests answers. What is unseen: a dim `mk1` beside a row's
  name, the row's own text dimmed with it, and the LOAD section's standing
  refusal when such a row is picked.

  The marks come from `IndexEntry::format`, which **no index on this desk has
  yet** — the field is hours old. `BankIndex::missing` counts a format-less
  entry as missing, so the next READ TAGS backfills exactly those and is a no-op
  afterwards; until it runs, a library shows no marks and behaves as it did
  before. **So the first thing to do with a DN2 connected is press READ TAGS and
  watch 388 rows go dim** — which is the check and the fix in one gesture.
- **The mk1 refusal in the panel**, which Neil met on 2026-08-29 before any of
  the above existed: it names *Digitone mk1* and it is correct. What changed
  since is where it appears — the LOAD section rather than the top of the panel
  — and that it now fires with no round trip when the format is known.
- **REVERT after several auditions.** Load three different presets onto one
  track, press REVERT, and the track holds what it did before the first one —
  not the second-to-last. The backup is real bytes off the box, so this is the
  half of audition mode that the happy path never exercises.
- **The length check on a good preset.** It did not fire on either box, which is
  the right outcome and is only the absence of an error. A refusal here would be
  a finding, not a bug — a payload length nobody has mapped.
- **The OS-build gate refuses in words.** `write_gate` is checked inside the
  store, so a box on an unlisted build refuses at the port and the panel shows
  its wording. Nothing has ever seen that message come out of *this* path.
- **Two panels holding each other off, in the write direction.** A load must be
  refused while Setup is sending, and Setup must be refused while a load is out.
  The scan half of this rule has never been exercised by hand either.
- **The verify's two reads, against a box with MIDI thru on.** The echo refusal
  is ported straight from the probe's own finding and has never been triggered
  deliberately. Turning thru on for one load is the cheapest way to see it.

**And the thing no list could specify, now answered by use rather than by
arithmetic:** how long a load takes. Five round trips and a 400ms settle is
roughly a second on paper; it was not reported as sluggish, which is the only
verdict that matters and is worth more than the paper figure. If it ever is, the
settle and the second verify read are the two knobs, and both were chosen to be
wrong in the safe direction.

### Still not verified on a screen — the Presets panel, 2026-08-29

What the two shots could not reach, itemised so it cannot be closed by a sweep:

- **The preset list's `ScrollArea` at its 360px cap against a bank of 256.** The
  shot had eight rows.
- **The tag chips as controls.** They drew, and nothing hovered or clicked one:
  an unselected `toggle_value` at rest looks like a label in this style, so
  whether a chip reads as pressable — and what a *selected* one looks like — is
  unanswered. The filtered state, and the "N hidden because they have not been
  scanned" line with it, has never been on screen.
- **A half-scanned bank**, which is the only state that draws the partial line
  under the grid.
- **The whole ALL view as it now stands.** The two shots above were taken of the
  one-bank browser, before Neil's session rescoped it, so the bank column on each
  row, the ALL entry at the top of the picker, and the `N bank(s) unread` caption
  have never been drawn. The findings those shots *did* close — six rail rows, the
  chips wrapping, `BLÅ` rendering — are unaffected, because none of them moved.
- **The A4's state**, which is the one that must not read as a fault: the tag
  grid gone, the bank still listing, no retry button, and the explanation
  saying *why* — the tag bits have never been checked against that box's display
  — rather than something that sounds like a broken cable.
- **A scan running**, on hardware: the progress line moving per preset, STOP
  taking effect within one round trip, and the note it leaves saying the work was
  kept. Also the panel **closed** mid-scan and reopened, which is the path where
  the worker's own save is the only thing that keeps nine minutes of reading.
- **A selection change mid-scan.** Press READ TAGS on a DN2, click a DT2 track, and
  the DN2's presets must not appear under the DT2. This is guarded in `poll` and
  the guard has never been exercised by a hand.
- **The two panels holding each other off**: READ TAGS greyed with its reason while
  Setup is sending, and OUT greyed while a scan runs. Both are one-line
  conditions and neither has been seen refusing anything.

**And the measurement, which is the reason to do this early rather than last:**
no bank of 1,189 has ever been scanned against a box. §10.3's nine minutes is
arithmetic. The panel's rate readout is built to answer it in one run, and until
that run happens the cancel/resume design is justified by a number nobody has
seen.

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

**Two things the sweeps did not separate, and both are cheap to settle.** Carried
here from the session's own notes so they outlive them:

- **PAN's scope.** CC 10 is the General MIDI pan controller, so a box may answer
  it globally rather than per track. It was swept on channel 1 with track 1 on
  screen, which cannot tell "track 1's pan" from "every track's pan". The test is
  one sweep: send CC 10 on channel 2 and watch track 1.
- **CC 7 against CC 95.** Both moved their named bar — AMP VOLUME on the amp page,
  TRACK LEVEL via the app's VOL field — so they are not the same knob as far as
  anyone looked. But nobody watched *both* bars during *one* sweep, which is what
  would prove it.

Neither blocks anything today: both entries are `auditable` and neither is
p-lockable, so the cost of being wrong is a knob moving that should not have.

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

> **The observation held; "and that is the blocker" did not.** Corrected
> 2026-08-29. The foot makes *guessing* a size safe, and the A4 never needed to
> guess — its file header declares the payload length. Relaxing the head magic
> was indeed the wrong fix; the right one was to take the size from the better
> witness and keep every check that still applies. §10.2 has the full account.

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
at +27.

### The 43 bytes, and a correction — 2026-08-29, same captures

The line above was as far as measuring got the first time, and reading it as
"a 43-byte tail" was wrong. The 43 splits, and the split is the same on all 24
files:

```text
  31-byte header | payload (= declared) | 12-byte trailer
                                           crc?  len  AAA1DAAA
```

**The header is 31 bytes on every box, not 36 on the digis and 31 on the A4.**
That is what §10.2 and `container_offset`'s doc both said, and it was an
inference from where the container landed rather than a measurement. What
actually differs is that **the digis' payload opens with the five-byte
`SOUND_WRAPPER`** — the same wrapper a `0x6b` kit-track-sound payload carries in
front of its struct — and the A4's does not. So the container is flush with the
payload on an A4 and five bytes into it on a digi.

The trailer repeats the payload length as a u32 and closes with `AAA1DAAA`.
The four bytes before that are checksum-shaped and are **not** a zlib crc32 of
the payload under a zero seed, which was the obvious guess given the read path's
crc32 is zero-seeded. Unidentified, and nothing needs it yet.

`every_capture_has_a_31_byte_header_and_a_12_byte_trailer` pins all of this
across all 24 files, so the next reading of it is a measurement too.

**Why this matters beyond tidiness: it dissolves the A4's extent problem.** Its
payload is 366 bytes and its container starts at byte zero of that payload, so
the struct is the declared length and needs no foot magic to find. §10.2 had
this filed as an open reverse-engineering question with no timeline; it is not
one.

**And then it turns out nothing needs the answer.** An extent buys the ability
to splice a sound into a kit slot, and **the A4 answers no `0x6x` dump request
at all** — no `0x6b`, so no `0x5b`, so no load-onto-track path exists for it in
this codebase regardless of what its files look like. The extent was never on
the critical path; it only looked like it was because the foot check is what
`decode_sound` happens to be built around.

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
| browse | List, read, the container layer, the tag index and the panel all ship | nothing — done, and on three boxes |
| load | ships and is hardware-verified end to end on both digis — double-click, payload, length check, store, read-back; the A4's refusal and the mk1 refusal both met a user | REVERT and the OS-build gate untested on a box; the mk1 row marks need one READ TAGS to appear; mk1 presets browse and do not load (the box was asked); the A4 has no path at all |
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
- **The A4 routes on its own container magic**, `BEEFBABA`, rather than falling
  out as a `BadHead` that reads like corruption. It was refused by name for
  three days; since 2026-08-29 the same distinction selects its sizing rule.

### The A4 browses and tags — closed 2026-08-29, and both stated blockers were wrong

**The A4 has no foot magic at all** — not once in any of eight files, which
`no_a4_capture_contains_a_foot_magic_anywhere` asserts directly so that a
firmware which starts emitting one is a failing test rather than a discovery
nobody makes. The foot is what lets `decode_sound` trust a size. That much was
true and is still true; everything this section used to conclude from it was not.

**Blocker one, the extent, was never a blocker.** Measuring the layout (§9, same
date) gives the A4's extent for free: its payload is 366 bytes and its container
starts at byte zero of it. The correction that mattered came a step later —
**the foot's job is to validate a *guessed* size.** `struct_size` finds the end
of a digi's struct by searching for it, and the magic landing is what proves the
search was right. The A4 needs no search: the file header *declares* the payload
length. A declared length is a **better** witness than a found one, so skipping
the foot check there gives up nothing at all. `decode_a4_sound` checks the head,
takes the length from the header, and refuses with `UnsizedContainer` when the
layout that declaration rests on does not hold.

**Blocker two, the calibration, was real and is done.** `sound::TAG_NAMES` was
calibrated on a DN2, and the A4's masks differ in character from every digi
capture — low bits set, which no digi file shows. Indexing them through a digi's
table would have published a guess about a field, which is exactly what §9's
standard exists to stop. It is now `TAG_NAMES_A4`, calibrated against
Overbridge 2.26.9's filter grid and checked on all eight A4 captures. See §9.

So `ScanError::BoxNotIndexable` is no longer returned for any box on this desk.
The variant stays for the next box with an unmapped container, and the panel
state it drives stays with it: a box that cannot be tagged still browses, still
answers the button that was pressed, and is never offered a retry it cannot use.

**The trap that shaped the API, and it is worth naming even now that it is
past.** The obvious fix was to let the head magic vary, since `container_offset`
already accepts both. That would have "worked": an A4 preset would decode to a
plausible name, a plausible tag mask and a **wrong length** — silently, and
exactly what the foot check exists to prevent. It is the `0x5b` echo hazard's
shape in a parser: the failure manufactures a positive. The resolution was not
to relax the check but to **replace the witness**, and the guard is now
`an_a4_preset_decodes_at_the_length_its_header_declares`, which asserts the
number 366 rather than merely that something came back.

**The same trap, one level up, is the live one.** A tag mask read through the
wrong box's table does not fail either — `THE SAW`'s mask decodes under a digi's
names to Kick, Snare, Acoustic, Soft, Dark, Vintage: the right *count* of real
tag names, five of six wrong, and nothing about the output that looks like a
bug. That is why `tag_names_for` takes a slug and has **no default table**, and
why `ui::presets::Library` carries the slug it was loaded for.

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
- `Sound::tags()` and the calibrated tag tables are already right, so the filter
  UI is a bit-mask test and nothing more. (`Sound::tags` takes a slug as of
  2026-08-29 — the *table* is what varies per box, not the bit-mask test.)

**Built 2026-08-29**, and the bullets above are now properties something holds
rather than decisions to remember. `preset_index::PresetIndex` is the store —
one JSON file per (device, bank), directory injectable the way `Stash`'s is —
and `preset_scan::scan_bank` is the reader. Three things the code settled that
this section had left open:

- **The mask is stored raw and named at display time.** A stored *label* rots
  the moment a tag table is recalibrated, and the tables have since moved twice
  — once corrected, once split per box. Every index written before either change
  still reads correctly, which is the whole return on that decision.
- **`occupied` is stored, so a bank that grew reads as incomplete** rather than
  as done. Comparing against the declared count instead of `entries.len()` is
  what makes "a box that gains presets rebuilds one bank" true rather than
  aspirational.
- **A bank path is filtered before it becomes a filename.** A +Drive path is
  data from a box, and a box that answered with a `..` in a directory name must
  not get to choose where this crate writes.

**The timings are still unmeasured, and the panel is now the instrument.** Every
figure in this section is one round trip's arithmetic multiplied by 1,189. So the
progress line reports **presets per second and a projection from the run in
progress** rather than from a constant, and it stays silent until five presets are
in — a projection from one round trip carries no information, and "9 hours left"
on the first tick is worse than nothing. The first real scan of a DN2 bank is
therefore the measurement, readable off the screen while it happens instead of
reconstructed from a stopwatch afterwards. That run wants a box on the desk and
has not happened.

`scan_bank` takes a `PresetSource` trait rather than an `ElektronDevice`, so
cancel, resume, skip-and-continue and the A4 stop are *tested* rather than
merely written — they are otherwise branches that only run with a box attached,
which is lesson 4's shape waiting to happen. The fake box returns the **real
committed captures**, so what those tests decode is what three boxes actually
sent.

**Paging is untested.** Every call in §9 used `start = 0, count = 0` and every
collection fitted one reply, so `next_cursor` has never run. A 256-entry bank is
the case that will find it.

### 10.4 Loading onto a track — where "instantly" goes wrong

> **Read §10.6 step 6 beside this.** Two things below were settled differently
> by the work that shipped on 2026-08-29 and are corrected there rather than
> rewritten here: the splice is a **slice** of the preset file rather than an
> assembly, and audition mode's backup is the load's own pre-read rather than a
> sixteen-track sweep when the panel opens. The reasoning below is what earned
> both, and it reads as a record.

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
4. ~~The tag index: scan, persist, cancel, resume per bank.~~ **Done
   2026-08-29** — `preset_index.rs` persists one file per (device, bank) and
   `preset_scan.rs` fills it, cancellable per slot and resuming from what the
   index lacks. Fourteen tests, driven through the committed captures.

   **Run on hardware 2026-08-29**, which is what this entry was waiting for:
   all three boxes scanned end to end, the indexes persisted and reopened, and a
   second scan resuming to a no-op. A DN2's whole library — **1,189 presets** —
   an A4's 869 across banks A–D, a DT2's 148. The timing claims here are no
   longer arithmetic.
5. ~~The panel — sixth rail slot, following `Sidebars`/`Tool`, worker thread and
   `mpsc` like `transfer.rs` and `sync.rs`.~~ **Built 2026-08-29**, `ui/presets.rs`,
   fifth in the rail rather than sixth — above Session, because Session is the
   file panel and has been the last row since banks were cut. `scan_bank` is the
   worker's body unchanged; what the panel adds is a thread, a channel, an
   `AtomicBool` and a screen. Twenty-six tests, none of which needs a box, and
   two screenshots — see §9. Seven things the panel settled that this section had left open:

   - **It opens from the index, not from the box.** §10.3 promises a second open
     is instant, and the only way that is true is if the first thing the panel
     touches is a JSON file rather than a MIDI port. The consequence is worth
     having on purpose: **the browser works with the box switched off**, which
     is when a good deal of arranging actually happens. LIST and READ TAGS are
     the only two things that open a port, and they are two buttons because they
     are two reads — one round trip against up to 1,189 — which is how "browsing
     must never block on tagging" stops being a rule somebody has to remember.

     **They were called REFRESH and SCAN until 2026-08-29**, and neither word
     said which was which: both are read-only, both hit the box, both act on the
     banks in view, so the pair read as one action at two intensities. They
     differ on what they *return* — names and slots, or tags — which is the split
     the panel is built on and the one thing the labels left out. Renaming only
     one would have left it defined against a word that was also wrong.
   - **The index is keyed by the box that answered, and the refusal is stricter
     here than in `ui::transfer`.** A mis-cabled fetch imports wrong bytes into
     a session slot; a mis-cabled *scan* writes a DT2's 148 presets into
     `digitone2-soundbanks-A.json`, where every later session believes them. The
     store outlives the mistake, so the worker identifies before it lists.
   - **A result belongs to the box it was asked for, and that needed enforcing
     rather than intending.** A scan is minutes and the roll's selection is one
     click away: pick a DN2, press READ TAGS, click a DT2 track while waiting, and
     1,189 Digitone presets land under the Digitakt. `poll` compares against the
     box on screen, and the selection is settled *before* the channel is drained
     — on the one frame the selection moves, the other order applies a stale
     answer and then wipes it.
   - **The library is the browsing unit, not the bank** — added the same day
     after Neil put the panel on three boxes. The first build had a bank picker
     and nothing else, and the gap was immediate: the question a person has is
     *"where is there a bass patch"*, not *"what is in bank C"*, and eight banks
     behind a picker makes the user the search index. The picker now opens on
     **ALL**, the search box and the tag chips work across every bank at once,
     and a row carries the bank it came from because a name without an address
     cannot be acted on. Per-bank survives for a targeted rebuild — five seconds
     against nine minutes — which is the same reason `PresetIndex` keys by bank.
     **The store is per bank; the browser is not**, and that split is the whole
     change: not one line of `preset_index.rs` or `preset_scan.rs` moved.

     The trap it introduced, and the test that pins it: one scanned bank beside
     seven untouched ones reported *complete* — every preset it knew about was
     tagged — and took the READ TAGS button away with it, in exactly the state that
     most needs it. `Tagging::Partial` therefore counts `unread_banks`
     separately, because a bank nothing has listed or indexed has an **unknown**
     size rather than a zero one.
   - **A control that vanishes is not a reply.** The A4's refusal was expressed
     purely by removing the READ TAGS button, on the reasoning that the explanation
     belongs in the tag section. Neil pressed it on an A4 and reported that it
     "flashes and then the button disappears" — a press answered by deleting the
     thing pressed, which reads as a bug. The state is still a state; a
     `Note::Warn` now says so at the point of action as well.
   - **The one-desk rule now has a sixth surface, and it runs both ways.** The
     four Setup groups already hold each other off; a browser that refused while
     a write was out but let a write start during a nine-minute scan would be
     the rule with its dangerous half missing. `safe_write_tracks` is a
     re-fetch, a confirm, a backup, a send and a read-back, and a second
     connection held open across that ceremony is a way to fail it in the
     middle. `PresetsPanel::busy` blocks Setup and Setup blocks the panel.
   - **`sound::TAG_NAMES` still said "unverified ordering"**, three days after
     §9 calibrated it against a DN2's own display. The panel puts those names in
     front of a user as fact, so the doc was corrected — and with the distinction
     the A4 makes necessary: calibrated on the digis, a guess on anything else,
     which is what `BoxNotIndexable` actually rests on.

     **The distinction was right and the remedy was too weak.** A doc comment
     saying "a guess on anything else" does not stop the guess reaching a user;
     the array was still global, so an A4's bits were still named. Fixed
     2026-08-29 by splitting the table per box and removing the default, which
     makes the distinction structural rather than advisory. A comment is not a
     constraint.
6. ~~Load-to-track on the path step 3 chose, with audition mode and its
   backup.~~ **Built 2026-08-29** — `drive::preset_load_payload` decides the
   bytes, `midi::preset_load` runs the five round trips, and `ui::presets`
   carries the gesture: a **double-click loads onto the selected track**, with a
   LOAD button as the same thing spelled out. Twenty-four tests, none of which
   needs a box.

   **Run on hardware the same day** — a double-click loaded the selected preset
   onto a track of a DT2 (0071) and a DN2 (0050), which is the whole path and not
   just the `0x5b` under it. The A4 does not load, as designed. §9 has what that
   run covered and the four refusals it did not.

   Five things the work settled that this section had wrong or open:

   - **The splice is a slice, and §10.4's description of it was more work than
     the format needs.** It planned to lift the struct out of a preset file and
     put it behind the five-byte wrapper a `0x6b` returns. The 24-capture
     measurement makes that unnecessary and slightly dangerous: a digi preset
     file's *payload* already **is** a `0x6b` payload — a wrapper then a struct,
     to the byte — so a load sends `file[31 .. 31 + declared]` and assembles
     nothing. The lengths were measured from both ends independently and agree:
     DT2 1,114 and 1,114, DN2 364 and 364.

     The danger in the assembly version is the wrapper's fourth byte. It carries
     the word that selects the struct version — `00` on a 319-byte DN2 sound,
     `01` on a 359-byte one — so a splice that kept the *track's* wrapper and
     swapped in the *file's* struct would describe the sound that used to be
     there. Copying the payload whole cannot make that mistake, and there is no
     version of the assembly that is safer than not doing it.

   - **A DN2's mk1 presets browse and do not load, and the box confirmed it.**
     388 of 1,189 are `DN1S` files — spread across banks **B, C and D**, not
     confined to one, so browsing the whole library hits one about a third of
     the time. They read, they tag, the box lists them in its own browser.
     `0x5b` is refused for them by container magic, not by bank or by guess, and
     since 2026-08-29 that refusal is a measurement rather than a caution: see
     §9's probe. The browser marks them, and the mark is permanent.

   - **The load path's own witness is the box, and it is free.** No function
     over a file can say whether *this box* wants a payload of that length.
     Reading the target track first answers it, and that read was needed anyway
     — its bytes are the backup. So the length check costs nothing and is the
     one place the box gets a say.

   - **Audition mode's backup is that read, and §10.4's "one backup when the kit
     builder opens" could not be built as written.** The panel opens without
     touching a port at all — that is what makes browsing work with the box off
     — so a sixteen-track pre-read on open would spend nine round trips
     protecting an audition nobody has asked for. Keeping the **first** pre-read
     per track instead gives the same guarantee for nothing: REVERT goes back to
     what a track held before the auditioning started, which is §10.4's own
     definition of the honest unit. Backups survive a change of selected box,
     alone among the panel's state, and do not survive quitting; the panel says
     both.

   - **`decode_sound_dump` was refusing half a DN2's library, silently, and it
     is the guard inside the store.** It recovered a struct's size from
     `KNOWN_SOUND_SIZES`, which has never contained 319 (a DN2 v0 sound) or 299
     (a DT2's). A `0x6b` reply carrying one came back "not a sound struct" — so
     `plan_track_sound_store` would have refused to load it, on the grounds that
     it could not parse it. §10.2's finding, *measure the struct rather than
     look it up*, had been applied to +Drive files and not to dumps. It measures
     the foot now, and `sound::measure_struct_size` is the one copy of the rule.
     **A guard is only as honest as its parser**, and this one was found by the
     first test that put a real v0 sound on a fake box's track rather than by
     anything on a desk.
7. Exercise paging on a 256-entry bank.

Steps 1–3 are hardware work and cannot be done from a desk without a box, and
all three are now done. Steps 4–6 are built; step 7 is not. All four can be
built against fixtures and only need a box to be believed — which is §9's
standard, and the one this project keeps.

**And what the browser owes for the refusal, added the same day after Neil met
it:** a preset the box will not take now says so on its own row — a dim `mk1`,
a tooltip that does not promise a double-click will work, and a LOAD section
that refuses with no port opened. The mark comes from `IndexEntry::format`,
which records the container magic rather than a verdict, so a policy change
cannot make an old index wrong. An index written before the field reads as
*unknown* and draws nothing; `BankIndex::missing` counts those entries as
missing so the next READ TAGS backfills exactly them, resumable and cancellable
like any other scan and a no-op once done.

**What v1 does not do, recorded so it is a decision rather than an oversight:**
a load changes the box and leaves no mark in the session. `Track::patch` is the
nearest field and its own doc rules it out — it records the last *fetch* from a
stored slot, not what the box is playing — so writing a +Drive preset into it
would make a saved session assert a fetch that never happened. A track that
knows what was auditioned onto it wants a field of its own, and that is a
session-format change with nothing yet riding on it.
