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

> **On what this file used to be, and what it became.** Through development this
> was a 4,000-line working document carrying every phase in full, plus a session
> log beside it. Both were trimmed for the public repo — the blow-by-blow went,
> along with the two design-handoff packages some UI comments cite by path.
>
> It then grew straight back past 4,000 lines, and **not with the same kind of
> material**: §9 and §10 are now four fifths of the file, and they are a
> hardware ledger rather than a plan. Every offset in them was paid for with a
> box, a capture and usually a reading off its screen, so the size is the point
> and trimming it would be deleting measurements. What that costs is
> navigability, which the map below is for.

**Two things to know before navigating it.** §1–§8 are the plan and are short.
**§9 and §10 are the ledger and are four fifths of the file** — §9 is what has
and has not touched a box, §10 is the +Drive, the kit builder and, under a
heading that does not say so, **the entire Analog Four protocol log**: pattern
layout, trig bytes, p-lock pool, parameter ids, scalings, kit and patch names.
It ended up there because that is where the +Drive work already was. The title
now says both, but the *number* cannot move — source comments cite `PLAN.md
§10`, `§10.3`, `§10.5` and `§10.6 step 3`.

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

Thirty-four `.syx` captures live in `crates/protocol/tests/fixtures/` (1.8 MB),
plus the 24 `.bin` preset files under `fixtures/drive/`. Ten of the `.syx` are
the original DT2/DN2 set — condition and p-lock captures, fresh/swing patterns,
the per-note chord — and **twenty-one are the Analog Four's**, one per question
its lanes and p-lock pool were mapped one at a time by (§10).

Every expected value in the *digi* suites was read out of the captures **by the
JS original first** and then written down in Rust, so those tests pin
digi-roll's hardware-verified behaviour rather than the port's own output. The
A4 suites have no such oracle — digi-roll never spoke gen-1 — so their expected
values come from the box's own screen instead, which is why §10 records a
screen reading beside almost every offset.

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
  thing this protocol supports. ~~There is no working-buffer dump request
  anywhere in `digi_protocol`~~ — **the A4 has six (`0x68`–`0x6d`, 2026-08-31,
  §10), and the DN2's `0x68`/`0x69`/`0x6a` "answer at index 1 and nowhere
  else" now reads like the same working-state family misread as slot requests
  — worth a one-sweep re-probe on a digi.** Nothing yet listens for the
  Program Change a box sends when its pattern changes — so "whatever it is on,
  whatever slot that is" still needs protocol work before it needs a control.
  Worth knowing before anyone promises it in a tooltip.

- **The A4's transfer path is proven on one round trip, and its refusals are
  not.** Built and run on 2026-08-31: a pattern off the box, edited in the roll,
  and back onto the box, first time. What has no cable behind it is everything
  that is not the happy path — a whole-bank dump against the quiet period, the
  wrong box answering, a cancel mid-send, a session that runs out of slots.
  Tests, no hardware. §9 has the list, and it is the same "it works" / "it
  refuses properly" split §10.6 step 6 drew for the preset load.
- ~~**An A4 pattern carries steps and pitches and nothing else.**~~ **Closed
  2026-09-01.** Velocity, length, micro-timing and trig condition are four named
  lanes now, each graded against the box (`a4_pattern.rs`'s offset table marks
  them `hardware`), and the p-locks read, draw, edit and travel. Closing it took
  exactly the method this bullet predicted — captures aimed at one lane at a
  time — over three sessions in §10. What remains unmapped is narrower and named
  there: the TRC menu's byte-to-condition table past its measured extent, and
  the FX and CV tracks' p-lock id space, which is carried back byte-exact rather
  than interpreted.
- **A trigless trig cannot survive a round trip through this app**, because the
  model holds notes and a trigless trig is a trig with no note. An import counts
  them and drops them; an export writes none. A pattern that leans on them comes
  back different, and the import report is the only warning.

**Honest summary:** a verified protocol foundation; a sequencer that has driven
two real boxes in sync; read, write and restore all proven on hardware from the
app's own buttons on the digis, and a whole-pattern round trip proven on the A4;
a session that saves and reopens; and installers for macOS and Windows that
people have installed and run. What is missing is surface, not seams — and the
refusals, which are the half of every path this project keeps having to be
reminded is unverified.

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

  **The settings are saved with the session** (2026-09-02), so a project recalls
  the arrangement it was written by and not only the notes that came out — genre,
  progression, seed, feel and every row, including where each one points. They
  ride in `Session::generator` as an opaque value core carries and never reads:
  `GenContext` is `crates/generator`'s type, and core naming it would close a
  dependency cycle. `ui::generate` owns the encoding, being the one place that
  depends on both crates. Key and scale are the exception and stay Harmony's, as
  they always were. Moving a slider marks the file dirty without opening a
  history step — it edits no note.

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

  **Chord lead** (2026-09-02) is a transcription of the Analog Four's factory
  A01: straight eighths, two pedal tones held per bar while the root leaps
  octaves under them, the voicing thinning where the root reaches the pedals.
  Its chords are ordinary same-step notes, which an A4 export carries as the
  trig's note plus ARP NO2–NO4 offsets — the box's own way of playing a chord on
  a polyphonic kit with the arp off. §10, "Chords reach the A4".
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
  Nothing is fetched from it or written to it, ~~and that is the box's own
  testimony rather than a gap: it advertises no `0x6x` dump request at all~~
  — **the testimony was about the other namespace, and the box answers
  `0x60`–`0x6d` (2026-08-31, "The A4 answers dump requests").** Kept struck as
  the sentence the whole misreading grew from.
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

**v0.3.0 (2026-09-01) is the release that finishes the third box**, and the
minor bump is the same argument v0.2.0 made from the other end: that one put an
Analog Four on the desk as a box the digis' assumptions did not fit, and this one
is where it stops being a special case. Its patterns fetch and write, its p-locks
read, draw, edit and travel, and its tracks name their sounds — through the same
panels, the same ceremony and the same model as a Digitakt's.

- **A4 p-lock lanes are editable**, all thirteen parameters. The ids were
  measured on 2026-09-01 morning and the *scalings* the same afternoon —
  `examples/a4_scale_probe`, one knob to each end-stop, the number read off the
  box's own screen. `writable_params_for("A4")` returned nothing that morning
  and returns thirteen now, which makes the A4's picker exactly as full as a
  digi's.
- **The Edit panel lists them**, which it had refused to do for a reason that
  was never about listing: the section was gated on `model.spec()`, `None` on
  this box forever, so a track carrying sixty-one named lanes showed none of
  them while the strip under the roll drew all sixty-one correctly. Listing and
  authoring are separate questions now and each is asked on its own terms.
- **The FX and CV tracks are read-only and byte-exact**, by decision rather than
  by omission. Their p-lock id space has never been swept, so nothing here can
  name their knobs; what the box has is carried back exactly as it came.
- **Patch names, off the box's edit buffer.** The A4's kit was mapped from
  captures that had been on disk for a day — 128 of them, checked before a
  single offset was written down — and `0x68` reads the kit the box has *loaded*,
  unsaved edits included. That is something no digi can answer: `ui::sync`'s own
  header says a dump request names a stored slot and nothing asks a box what it
  is playing. On this box something does.
- **Four sounds against six tracks**, so `PatchSound` gained a fourth shape.
  `NoSound` is not `Unnamed` — an unnamed slot is one a later read might fill,
  and the FX and CV tracks never will be.
- **Two sentences the app was telling users, both false.** "Analog Four plays
  over MIDI but has no patch names to read" was wrong in both halves; the
  Presets panel's "answers no dump request for a kit track's sound" was
  *strictly true* and still had to go, because it opened with the six words that
  had been the headline wrong claim about this box. One `grep` for the phrase
  found both plus four stale doc comments — and a green test that had been
  holding the first one in place.

**What this release cost in probe bugs is the part worth carrying forward**, and
there were four. A scaling probe that assumed every parameter's screen shows a
decimal (`OFF` and `L64` do not). One that read a knob's *change* rather than its
*value*, so a knob already at the end-stop being asked for looked like a knob
nobody touched. One that rejected two correct measurements for a slope the app
does not model anyway. And a target list built from eleven parameters against a
table of thirteen, which let three consecutive runs report themselves complete
over a table they were not covering.

The last is the one to remember: **a list that is a subset of what it measures
cannot say so.** The run looks clean and the gap only surfaces when somebody
counts. It is now asserted before a port is opened. The other three share a
shape too — each was an assumption about the *box* that the probe encoded as if
it were an assumption about the *format*, and the box is the thing the probe
exists to ask.

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
- **The first +Drive write, and the first project read** — 2026-08-30, A4 0195.
  A 2 MB project read off the box four times to the same digest across a power
  cut and a factory reset, and three sound files written to `/soundbanks/P` and
  read back identical but for the box's own location stamp. The write layouts
  came off a spy capture of Elektron's Transfer rather than out of a probe; see
  the two sections above for what the probing cost.
- **The A4's pattern format, read rather than probed** — 2026-08-30, A4 0195.
  Eight dumps off the box's own front-panel SysEx menu gave the gen-1 framing,
  the checksum, the seven-bit order (**MSB-first**, which the digis turn out to
  share — see the correction in §10) and the trig and note lanes, validated by
  decoding a pattern back into the arpeggio a musician had played into it. The box had a pattern path the whole
  time; this project had been reading its silence in the wrong namespace.
- **The A4 trig model, confirmed from the write side** — 2026-08-31, A4 0195.
  One 14,843-byte pattern carrying all four trig states authored by
  `a4_pattern::build_trig_probe`, sent to A16, and read off SYN1's step LEDs:
  **3, 5 and 12 lit and the other 61 dark**, which is the prediction to the step.
  It closes §10's open item 2 and it is the first thing on this list whose result
  could not have been obtained by reading anything — `(01,c0)` and `(09,c0)` are
  a bit that is *set* and must be ignored, so only handing it to the box could
  settle it. Two extras the run gave for free: steps 3 and 12 showed as
  **trigless** trigs rather than merely lit, so the box's reading of byte 1 bit 1
  matches ours, and step 1 went dark, so a `clear_trig` authored here takes.
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
- **`copy_track`** — the **box-to-box** copy has no caller, so nothing can drive
  it. The in-app whole-track copy is a different function (`core::track_clip`,
  bound to Shift+C/Shift+V) and does work; §1 has the distinction.
- **A multi-chunk +Drive write.** Single-chunk is verified; everything above
  16 KiB is refused rather than guessed, so no project has ever been written
  back to a box. This is what stands between the A4 and whole-project backup
  and restore, and between the DT2 and kit saving.

  > It is **not** what stands between the A4 and a pattern write, which this
  > bullet used to claim on the grounds that the box answers no `0x6x`. It has
  > a gen-1 SysEx dump path off its own front panel, captured and decoded
  > 2026-08-30 — see §10's entry. The `0x6x` absence is real and means only
  > that the A4 is not on the gen-2 dump protocol.
- ~~**Sending anything to an A4 over gen-1 SysEx.**~~ **Closed 2026-08-30**, and
  the bullet outlived it by a day: a pattern was sent to A16 and the box
  displayed it, and on 2026-08-31 `build_trig_probe` sent four authored trig
  states and the front panel showed each one as predicted. Both runs are in this
  section. The stale sentence is kept struck rather than deleted because it is
  the third instance of the same failure in this file — a caveat written once,
  at the moment it was true, and never read again by anyone who could falsify
  it. `ui::transfer`'s header has the general form.

  What is still true and is the reason the caution survives the strike: **this is
  the box that needs a power cycle after a body it cannot parse**~~, and it answers
  no dump request, so nothing on this side can check what landed~~ — **the second
  clause fell on 2026-08-31: `0x64` fetches any pattern slot, and A16's probe
  trigs were read back over the wire** (§10, "The A4 answers dump requests").
  The power-cycle hazard stands on its own.
- **"Read patch names" on the *second* box.** One box answered on 2026-08-22 and
  the other has not been tried, so `NotThisBox` and the DT2/DN2 difference in kit
  layout are still fake-only.
- ~~**Decoding a +Drive preset's contents.**~~ **Closed 2026-08-29**, and this
  bullet is the reason the register itself needed auditing: it went on asserting
  that "**no preset's tag mask has been read off the +Drive itself**" while four
  entries *below it in this same section* recorded the tag tables calibrated
  24-of-24 exact on three boxes and 2,206 presets indexed off three libraries.
  A caveat kept in the "what has not" list is not read by the person adding to
  the "what has" list, which is the failure mode this whole section exists to
  guard against, turned on itself.
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

### The A4 transfer path, end to end in the app — 2026-08-31, A4 0195

> **Superseded the same day it was written — the round trip stands, the shape
> does not.** Everything below describes `ui::a4`: the listener IN, the
> baseline-bound OUT, the typed consent, the six surfaces. That panel was built
> on "the A4 answers no dump request", the probe below refuted it within hours,
> and the panel is gone — the A4 now moves patterns through `ui::transfer` and
> `ui::sync` like the digis (§10, "The A4 joins the digi transfer path"). Kept
> unrewritten because the hardware claim in its first paragraph is real and is
> the evidence the allowlist row rests on: the bytes crossed both ways and the
> box played them. What is struck from the record is only that this shape was
> the necessary one.

**A pattern came off the box into the app, was edited in the roll, and went back
onto the box. Neil ran it the same day it was built and it worked first time.**
That is a full round trip through every layer built below — `SysExInbox` →
`receive_patterns` → `import_a4_pattern` → the roll → `export_a4_pattern` →
`send_pattern` — and it is the claim this section exists to make.

**What it is not is exhaustive, and Neil said so rather than being asked.** One
slot, one small edit, one send. Untouched: a whole-bank dump against the quiet
period, the wrong box answering, a cancel mid-send, an mk1-shaped refusal, the
overflow path, and any pattern leaning on trigless trigs. Those are branches with
tests and no cable behind them, which is a weaker claim than this paragraph's and
a stronger one than yesterday's.

**This entry was written the day before as "no box" and is rewritten rather than
appended to**, because a register that accretes corrections underneath a stale
headline is how §1's summary came to be wrong twice.

The A4 now moves whole patterns in both directions from the app's own panel:
`digi_midi::a4_transfer` is the wire, `digi_core::a4_transfer` is the seam, and
`ui::a4` is the sixth surface in the DATA TRANSFER group. 46 tests, none of which
needs a box, and two screenshots.

**Six things the build settled that were open, or were settled wrongly.**

- **The A4 is a *push* device, and every transfer in this app was built for a
  pull.** `ElektronDevice` sends a `0x60` request, retries it, and matches a
  reply against it; the A4's supported-opcode list has no `0x6x` in it at all.
  So IN is a **listener** — `SysExInbox` held open, drained on a poll, ended by a
  quiet period rather than by a message count, because a person may dump one
  pattern or a whole bank and nothing on the wire says which they chose. The
  consequence reaches the screen: the armed state is **an instruction, not a
  spinner**, because the next thing that has to happen is somebody walking to a
  piece of hardware.

- **A frame that is not an A4 pattern is recorded rather than dropped**, and the
  reason is `0x54`: a *project-settings* dump on a Digitakt II and a *pattern*
  here, the same opcode byte carrying a different message. A three-box desk can
  put a DT2 dump into this window. `is_a4_pattern` keys on `(family, dump_type)`
  and the panel reports the family that answered, so a mis-cabled receive says
  *the wrong box answered* rather than *nothing arrived* — which are the same
  silence and different fixes.

- **OUT is a read-modify-write, and that is the format's decision rather than a
  shortcut.** Of the 751 bytes in an A4 track, `a4_pattern` names 259; the rest
  are recorded as *shape* — six unnamed per-step lanes, a defaults block, a
  27-byte tail. Synthesising a payload means inventing 10 KB of a 13 KB message,
  and the box's answer to a body it cannot parse is a power cycle. So the
  unmapped bytes are **carried, not chosen**, which is the bargain `safe_write`
  strikes for the digis by re-fetching — taken at receive time instead, because
  this box cannot be re-fetched.

  It also **sidesteps the question `a4_plocks` is blocked on**. That module has
  no writer because whether the box *requires* the compacted pool order it
  produces is a write test no dump can settle (§10.6). An RMW never reorders the
  pool, so it never asks. The blocked item is not closed; it is routed around.

- **So there is a rule a user meets: you cannot send to a slot you have not
  received from.** `A4ExportError::NoBaseline`, and the panel states it where it
  is met rather than greying a button silently. The baseline rides on the
  `Pattern` and is hex in the project file — 26 KB a pattern — because a session
  reopened tomorrow must still be able to send, and a baseline in a side table
  would need maintaining at every slot operation with a stale-bytes failure an
  RMW cannot detect.

- **`can_sysex` had stopped answering the question people asked it**, and
  `ui::presets` had already written a paragraph explaining why it does not ask.
  That field means "has a gen-2 `Spec`", which was the same set as "can transfer
  a pattern" until this box. `DeviceModel::pattern_route` is now the honest
  field — `LiveOnly`, `Request`, `FrontPanelDump` — data on the row, per §6's
  second carried-forward decision. One place working around a name is a
  workaround; a second would have been a pattern.

- **Windows cannot pace an A4 send, and the reasoning is already in this repo
  pointed the other way.** `midir`'s WinMM backend refuses any chunk not
  beginning `0xF0`, which `device::SEND_CHUNK` derives at length — so a paced A4
  send there fails on packet 2 of 58 and leaves the box holding half a message.
  `CAN_PACE` is false there and the pacing collapses to one call, which is safe
  and, on the 2026-08-30 measurement, useless: **the expected result on Windows
  is that nothing happens**, and the panel says so before the user spends a slot.
  Reasoned from `midir`'s source, not measured — no A4 has met a Windows build.

**Three faults the screenshots found and no test could have**, which is lesson 8
earning its keep for the fourth time:

1. The A4's box row still read **"· live only"**. The Add-a-box dropdown had been
   fixed and this second copy of the label had not — two places writing the same
   sentence, and the fix reached one.
2. The IN group said **"Analog Four plays over MIDI but has no pattern dumps to
   fetch"** for a box that had, by then, moved patterns in both directions in the
   suite.
3. The A4 appeared in the IN frame **twice** — once as that dead row, once under
   FRONT-PANEL DUMP. `ui::transfer`, `ui::sync` and `ui::write` now filter on
   `pattern_route`, because a box with its own group does not belong in the
   group it is not part of.

**A seventh thing, and it is the one that cost something: the panel shipped an
instruction that was false, and this file had already measured it false.** The
SEND tooltip said *the box must be in SETTINGS > SYSEX DUMP > SYSEX RECEIVE*. It
must not — the entry above, from 2026-08-30, records the A4 taking a 14 KB dump
at its ordinary menu with SYSEX RECEIVE never entered, and Neil's round trip
confirmed it again by not arming the box at all. The wrong sentence was copied
out of `examples/a4_pattern_send`, which had carried it since before the finding
and which nobody re-read while building on top of it.

Worth more than a typo because **the two claims are not equally safe**. "Put the
box in receive mode" describes an interlock: a step that would catch a stray
message. There is none — there is no arming step between a 14 KB SysEx and an
overwritten pattern slot — so the instruction did not merely misdescribe the
box, it invented a safety net over the one place this path has none. The panel
now says the true and louder thing, in the panel rather than in a tooltip, and
the typed consent is the only interlock there is.

This is `DEVELOPMENT.md` lesson 3's comment-level twin for the third time in this
repo (`ui::transfer`'s header is the second), with a new wrinkle: the stale line
was not merely *left* somewhere nobody looked, it was **propagated** — copied
into a new file by someone who took an example as documentation. An example is
code, and code goes stale in the direction of whatever it was true about first.

**What the round trip did not settle, and what still needs a cable:** the
quiet-period timing against a real multi-pattern bank dump, and the refusals —
a receive with the wrong box answering, a cancel mid-send, a session whose slots
run out. Every one has a test and none has hardware, which is the same
distinction §10.6 step 6 drew between "it loads" and "it refuses properly".

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

> **The refusal was the thing that was wrong, 2026-09-01.** The A4 loads from
> this panel now — see "The A4 loads presets, and struct version 5 is why it
> looked like it could not" in §10. What this entry got right is that a legible
> refusal is worth building; what it shows is that a refusal is also a claim,
> and this one had a three-day life.

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
store opcode and **not one `0x6x` dump request**. ~~So the A4 is *identifiable
but not dumpable*~~ — **wrong, 2026-08-31: the advertised list describes the
API namespace only, and the box answers `0x60`–`0x6d` in the dump namespace
that list never described** (§10, "The A4 answers dump requests"). What the
finding still bought is real: `Product.family` was a `u8` because every box
that could name itself could name its dump family too, and it is an
`Option<u8>` now — but the A4's answer to it should be `Some(0x06)`, not the
`None` this paragraph concluded. "Correct on the box's own testimony" was this
file's fourth lesson-11 instance being written down *as the moral of avoiding
the previous three*.

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
A paramId comes from locking a knob on hardware and reading the dump back, ~~and
this box answers no dump request at all~~ — **as of 2026-08-31 it does (`0x64`,
§10), so the read-back half of that loop no longer needs a walk to the front
panel.** Hearing a parameter and being able to store it stay different
capabilities — the same distinction the bug below turned on, arriving twice in
one day from opposite directions.

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
`app/tests/all/engine_link.rs` runs on `two_box_session()`, and a DT2 and a DN2
each have a spec *and* a chart — so "has a spec" and "has a chart" return the
same answer on both, and the wrong one is indistinguishable from the right one.
That is lesson 4's shape precisely: **a fixture that makes two different rules
agree.** It needed the third box to tell them apart, and the third box is the
first this project has ever had where the two come apart at all.

The regression test is `a_box_with_no_gen2_spec_still_gets_its_level_fader`, and
it is the first test in the file to build a session by hand rather than take
`two_box_session()`, which is the actual lesson for the next box that does not
fit the fixture. It was named `a_live_only_box_…` until 2026-09-01: the A4 has
not been live-only since 2026-08-31, and the property under test was never
live-only-ness but the absence of a gen-2 `Spec`.

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

The **A4** cannot be tried at all — ~~it answers no `0x6x` dump request, so it
has no `0x6b` to mirror~~ **it has a `0x6b`, but not this one: the A4 answers
`0x6b` with its `0x65` twin (working-state, index ignored), not a kit-track
sound (2026-08-31, §10)**. The skip stays right and the probe's stated reason
is stale: the A4 has no kit-track-sound *request*, so there is still nothing
for a store to mirror.

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
and **not once in any A4 file.**

> **Wrong twice over, corrected 2026-08-30.** The A4 has a foot magic; it is
> `BABEFACE`, and it sits at 377 in all eight A4 captures and at the end of the
> A4 project's payload. It appears in **no** digi file, exactly as `BACEF00C`
> appears in no A4 file — the two boxes have a magic *pair* each, `BEEFBABA`/
> `BABEFACE` against `BEEFBACE`/`BACEF00C`, and only the head halves had ever
> been compared. "Not once in any A4 file" was a true statement about
> `BACEF00C` and the conclusion drawn from it was about foot magics in general.
>
> This is lesson 11's error in its purest form: searching for a constant that
> belongs to the other box and reading the absence as a property of this one.
> It is the *third* time on this page — the `0x10` DirList sweep and the `0x6x`
> dump sweep are the other two — which is why the lesson is now its own entry
> rather than a paragraph here. `decode_sound` calls the foot check "the
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
| A4 | 31 | `BEEFBABA` | 5 | foot `BABEFACE` @377 | 409 |

Every file is `declared + 43` bytes on all three boxes, declared being the size
at +27.

> **`+27` as a `u16` was right for all 24 and still wrong.** Corrected
> 2026-08-30. The field is a **`u32be` at +25**; the low half of it is the `u16`
> at +27, and every capture here — 366, 364, 1114 — fits in sixteen bits, so no
> fixture could tell the two readings apart. An **A4 project** did: `/projects/1`
> is 2,061,057 bytes declaring 2,061,014, and the old reading returns **29,398**.
> The header also now reads end to end with nothing spare — payload `u32be` at
> 25, trailer length `u16be` at 29, header ends at 31 — where the old reading
> left two unexplained bytes at 25. `a_payload_larger_than_a_u16_declares_its_whole_size`
> pins it, and names the project capture in its own body precisely because the
> committed fixtures cannot fail it.

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
to splice a sound into a kit slot, and ~~the A4 answers no `0x6x` dump request
at all — no `0x6b`, so no `0x5b`~~ **the A4's `0x6b` exists and is not
kit-track-sound (2026-08-31, §10: it is the `0x65` working-state twin)** — so
no load-onto-track path exists for it in this codebase regardless of what its
files look like. The conclusion survives its premise's correction; the extent
was never on the critical path, and it only looked like it was because the foot
check is what `decode_sound` happens to be built around.

**Tag masks came back populated and varied** (`0x00902000`, `0x04880804`,
`0x0400c088`), which is what §10.3's index needs and the reason the captures
carry real names rather than blanked ones.

## 10. The kit builder, and the Analog Four protocol log

Two things under one number, for the reason the map at the top of this file
gives: this was scoped as the kit builder on 2026-08-26 (below), and the A4's
gen-1 format was then mapped underneath it because the +Drive work was already
here. **If you are looking for the A4's pattern layout, trig bytes, p-lock pool,
parameter ids or scalings, they are in the dated entries in the second half of
this section**, not under a heading with the A4's name at the top level.

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
`0x53` file API — a separate question from the dump namespace, which this
sentence used to say the A4 had none of. It has one: `0x60`–`0x6d`, 2026-08-31,
below.
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
`tests/all/drive_preset.rs`. Two of the three jobs were smaller than recorded and
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

**The A4 carries no `BACEF00C`** — not once in any of eight files, which
`the_a4_has_its_own_foot_magic_and_not_the_digis` asserts directly so that a
firmware which starts emitting one is a failing test rather than a discovery
nobody makes. The foot is what lets `decode_sound` trust a size. That much was
true and is still true; everything this section used to conclude from it was not.

**And it was read as "no foot magic", which is a different claim and a false
one** (corrected 2026-08-30). `BACEF00C` is the *digis'* constant. The A4 has
`BABEFACE` at 377 in all eight, and no digi file carries it — a magic **pair**
per product, of which only the head halves had ever been compared. The test now
asserts both directions on both families, because one that only looks for the
other box's constant can only confirm what it already assumed. Nothing below
changes: a *declared* length is a better witness than a searched-for magic
whether or not the magic exists.

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

### A whole project read off an A4 — 2026-08-30, A4 0195

The first time anything here read a +Drive **project**, and the first real
exercise of `drive_read_file`'s multi-chunk loop: every one of the fifteen files
that path was verified on in August was a preset that fitted a **single** 4 KB
chunk, so the sequence check and the short-chunk terminator had never run at
length against hardware.

`/projects/1`, **2,061,057 bytes in ~8 s** — about 503 chunks, no stalls, no
sequence errors. Read **four times**: twice before a power cut, once after it,
and once after Neil factory-reset the box. All four agree on
`fnv1a64 = 0x578814f25ffddb84`, which also settles that the slot's `PRESETS`
project is Elektron's factory one rather than anything of Neil's.

Two structural facts, from `capture_drive_project.rs`:

- **The listing's size is an allocation, not a length.** `/projects` reports
  2,097,152 for the slot — exactly 2 MiB — where the file is 2,061,057. Presets
  do not behave this way (their listing size is the file size exactly), so
  "listing size == file size" is a fact about small files and not about the API.
- **The container is the same one presets use.** `FILE_MAGIC` at 0, the OS build
  that wrote it at +9 (`0195`), the project name at payload+8, and 645
  `BEEFBABA` containers — one at payload+0, then **161 groups of four** from
  1,673,702 onward at a 2,410-byte group stride, four synth tracks to a group,
  every one of them named. The first 1.6 MB carries no container at all.

### The +Drive write path, captured and verified — 2026-08-30, A4 0195

**Writing works.** Three files written to an Analog Four's `/soundbanks/P` and
read back, each identical to its source but for two bytes the box stamps itself.
The layouts were not derived; they were **read off the wire from Elektron's own
Transfer 1.10.4**, and the two days before that are the argument for why.

**What guessing bought, and what it cost.** Six candidate `0x58` bodies were put
to the box on 2026-08-29. Three earned a clean refusal (`Invalid sequence
number`) and three earned silence — and on this box **a body it cannot parse
takes down the whole SysEx API**, not just the file layer: it stops answering
`0x01` Device while a DT2 and a DN2 on the same bus answer normally, and it
needs a power cycle. Four power cycles produced three true facts (`0x57` is
elk-herd's order; the body has a length field; a refused chunk tears the
transfer down) and no working write. That hazard is not in any document and is
now in `probe_drive_write.rs`'s header, because "the box went offline" is the
wrong conclusion to draw from it.

**One capture settled all of it.** A CoreMIDI spy on `To Elektron Analog Four`
while Transfer uploaded one sound — 502 messages, of which exactly one `0x57`,
one `0x58` and one `0x59`:

```text
  0x57 WriteOpen   u32be total-len, path\0            -> ok, u32be fd
  0x58 Write       u32be fd, u32be offset, u32be crc,
                   u32be chunk-len, data              -> ok, fd, offset, taken
  0x59 WriteClose  u32be fd, u32be 1                  -> ok, fd, total-len
```

Three things no sweep would have reached. The body has **four** u32 fields, in
an order no gen-1 opcode uses. `0x59`'s second field is **not** the total length
— symmetry with `0x56` Close says it should be, and it is a literal `1`. And the
checksum is a zero-seeded CRC32 that is **then inverted**: the captured field
read `0xdf008194` where the plain zero-seeded value is `0x20ff7e6b`. The source
document records the zero seed and not the inversion, which is the likeliest
single reason its multi-chunk writes were refused on checksum.

**The two bytes that differ are the box doing its job.** A file carries its own
bank and slot at `+23` and `+24`, zero-based, and the box rewrites them at
commit. `/soundbanks/A/1` written to `P/2` read back `(0x0f, 0x01)`; the same
source to `P/5` read back `(0x0f, 0x04)`; `A/3` to `P/9` read back `(0x0f,
0x08)`. `drive::differs_only_by_location` is the check that accepts exactly that
and still catches a corruption or a truncation.

**Still single-chunk only, and deliberately.** Transfer's upload was 373 bytes
and fitted one chunk, so `offset = 0` is the only value any evidence covers —
and at one chunk a byte offset and a sequence number are indistinguishable.
`drive_write_file` refuses anything over 16 KiB rather than pick one and ship a
silent corruption. Settling it wants a capture of Transfer writing a *large*
file, which is the same ask as before and now a much cheaper one, because the
capture rig exists.

### The A4 has a pattern path after all — 2026-08-30, A4 0195

**"The A4 exposes no pattern path and answers no `0x6x`" was a true sentence
about the wrong namespace.** The box's supported-opcode reply lists `50`–`5e`
and no `0x6x`, and this document read that as *the A4 cannot transfer a
pattern*. What it actually establishes is that the A4 is not on the **gen-2
dump protocol** — which is the only one elk-herd documents, and the only one
this codebase could speak. The A4 is a 2013 box and has a **SysEx Dump menu on
its own front panel**: send and receive for pattern, kit, sound, pattern+kit.
Neil confirmed it in about a minute, after two sections of this plan had been
written on the assumption it did not exist.

That is lesson 11 for the fourth time, and the most expensive shape of it yet:
absence of the *other* generation's opcode read as absence of the capability.
The `0x10` DirList sweep, the `0x6x` dump sweep and the `BACEF00C` foot magic
are the other three.

**Nothing was probed. Eight dumps were captured and read.** The rig is the one
lesson 13 built, minus a step: the spy driver exists to watch what another app
*sends*, and here the box is the sender, so an ordinary MIDI Monitor source is
enough. `local/decode_mmon.py` decodes the `.mmon` and the captures are in
`local/a4-check/`.

#### The framing, verified on all eight captures

```text
00 20 3C   Elektron manufacturer id
06         product byte for the A4      — the identity API calls this box 4
00         device id
54 / 53    message type: 0x54 pattern, 0x53 sound
01 01      constant across every capture, unidentified
NN         slot index, zero-based       — A01 -> 0, A16 -> 15, Sound 01 -> 0
...        payload, seven-bit packed
CC CC      checksum: 14-bit sum from offset 9, two 7-bit bytes
LL LL      length: total - 8
```

> **This is the gen-2 dump header, field for field — 2026-08-31.** Written up as
> a gen-1 discovery, and it is `protocol::build_dump_message`'s output with
> different values in it: `product` is gen-2's `family`, and the `01 01`
> "constant across every capture, unidentified" is the `version` field this
> codebase has always emitted. The checksum starts in the same place and the
> length is the same `encoded + 5`. `parse_sysex` reads an A4 pattern dump with
> its checksum and count verified and **nothing added**, which was found by
> handing it a capture — a check available from the first minute of this section
> and not run for two days. DEVELOPMENT.md lesson 17.
>
> What actually differs is the *meaning* of the opcode: `0x54` is
> `DUMP_PROJECT_SETTINGS` on the digis and a pattern here, so a `dump_type` is
> only meaningful alongside its `family`.

Checksum and length verify on all eight, across both message types and sizes
two orders of magnitude apart (413 bytes and 14,841). **We can therefore emit a
message the box's own checksum will accept**, which is what makes this a write
path rather than a read format.

**The slot byte is why the checksum's start was nearly recorded wrong.** A01 has
`b[8] = 0`, so summing from 8 and from 9 give the same answer and one capture
cannot separate them. A16 has `b[8] = 15` and separates them immediately. The
length field has the mirror problem — every pattern is the same size, so only
the 413-byte *sound* dump could settle that it counts `total - 8`. Two of the
five facts above exist because the capture set varied along an axis the
question did not obviously need, which is §12's finding restated.

#### The seven-bit packing runs the other way

**The A4's gen-1 packing is MSB-first** — the MSB byte's bit 6 carries the
*first* payload byte. ~~`sevenbit.rs`, ported from elk-herd for the gen-2 boxes,
runs bit 0 to byte 0. A decoder must take the order as a parameter; the two
generations do not share it.~~

> **The struck sentence was never true — corrected 2026-08-31.** `sevenbit.rs`
> is `head |= 1 << (6 - i)`: the first data byte's high bit goes to header bit 6,
> which is the gen-1 order. It and the A4 decoder are **the same function on
> every input**, ragged tails included, and `sevenbit.rs` passes this section's
> own `BEEFBABA` arbiter on the sound dump. So it takes no bit-order parameter
> and the two generations *do* share this primitive.
>
> The order that produced the four wrong rounds below was a hand-written
> `msb_first=False` — believed to be what `sevenbit.rs` did, and never compared
> against it. Everything else in this subsection stands. DEVELOPMENT.md lesson
> 17, and `a4_pattern::sevenbit_is_shared_across_generations` is the test.

The wrong order survived four rounds of analysis here, because it produces bytes
that look entirely plausible — offsets, strides and diffs all read as structure.
What caught it was a **constant**: `BEEFBABA` sits at offset 0 of the sound dump
under MSB-first and appears nowhere at all under the other reading. Every byte
offset derived before that check was wrong, and none of them looked it.

#### The pattern layout

A pattern payload unpacks to **12,974 bytes**, and that number arrives twice
from unrelated directions: it is the decoded length of a SysEx pattern dump, and
it is the measured stride of `/projects/1`'s leading 1.67 MB (best lag 12,974,
correlation 0.83 — see the project-read entry above, whose 13,076 estimate was
arithmetic and is superseded). **The SysEx dump and the project file's pattern
record are the same object.** The first 1.6 MB of an A4 project is its pattern
array; the `BEEFBABA` containers from 1,673,702 onward are the sound pool.

- **Six tracks of 751 bytes, from offset 4** — 4,506 bytes, then the payload
  goes sparse. Four synth tracks, FX and CV. Confirmed three ways: the non-zero
  density falls off a cliff at that boundary; a trig added to track 2 landed at
  `4 + 751`; and A01 shows content in blocks 0 and 3 alone.
- **Trigs: two bytes per step, 64 steps, at track base + 0.** Step 1 at +0,
  step 2 at +2, step 17 at +32 — measured, one capture each, all three from a
  cleared pattern. **Bit 0 of the first byte is the trig.** The second byte
  reads `0xc1` on a trigged step.
- **Notes: one byte per step, 64 steps, at track base + 128.** Step 1 at +128,
  step 2 at +129, step 17 at +144. `0xff` is no note; a fresh trig gets `0x30`.

> **The note names in this section are one octave low.** The A4 displays `0x30`
> as **C4**; this document was written under the `60 = C4` convention, which is
> not this box's. Confirmed 2026-08-30 by writing `0x30` and reading the box's
> own screen — so the correction comes from the hardware, not from a second
> opinion about conventions. `note_name` in `local/a4_pattern.py` is fixed.
> Intervals are unaffected, so the validation argument below still holds; only
> the labels were wrong.

**Validated end to end against A01 rather than against the fixtures that
produced it.** Decoding A01's track 1 under this layout gives 32 trigs on every
odd step carrying `A3 A4 A5 / A3 A4 A5 / A3 A4 G5 / G3 G4 G5 …` — an arpeggio
with a chord change, which is a musician's pattern and not a plausible-looking
byte run. Track 4 gives 19 trigs whose note lane is `0xff` except at steps 1,
17, 33 and 49, where it reads `A3 G3 D3 F3`: the roots, on the bar lines, of the
progression track 1 is playing. A layout that reproduces musical sense it was
not fitted to is being believed for the right reason.

#### What the captures already settle — 2026-08-30, second pass

Four of the five open items below moved without a new capture, by asking the
existing eight the right question. `local/a4_pattern.py` is the tool; PLAN's
own §12 finding again, that a capture set varying along an unasked axis answers
questions later.

- **The second trig byte's bit 0 marks a note trig.** Across all 51 trigs in
  A01, `byte1 & 1` is set on exactly the steps whose note lane is not `0xff` —
  51 of 51, no exceptions, on two tracks. `0xc0` is a trigless trig and `0xc1`
  carries a note. **Still worth the deliberate capture**, because A01 cannot
  separate "bit 0 means note" from "bit 0 is derived from the note lane", and
  those differ the moment we write one.

  > **Wrong, and the deliberate capture is why we know — 2026-08-31.** A
  > trigless trig is `(00,02)`, not `0xc0` with byte 0 bit 0 set. `0xc0` is a
  > note trig whose note was taken off again, and the box displays that step as
  > **empty**. The trig state is `byte1 & 0x03` alone. This bullet is the
  > cleanest correlation in the document and it is the worked example in
  > DEVELOPMENT.md lesson 16; see "The pool is the p-lock store, and the trig
  > bytes were wrong twice" below.
- **The first byte's `0x08` is positional, not state.** In an empty pattern the
  trig lane's first byte reads `00 08 00 08 …` for all 64 steps: bit 3 is set on
  even steps and clear on odd ones, in a pattern with nothing in it. So it
  carries no per-step information and a write must preserve it — `|= 0x01`, not
  `= 0x01`.
- **Track base + 448 is the per-track default note.** One byte per track, not
  four bytes once. In the two captures where a fresh trig's note came out `0x3c`
  and `0x3e`, those are exactly what tracks 1 and 2 already held at +448; in the
  three where +448 had been set to `0x30`, the fresh trig came out `0x30`. A
  fresh trig takes its note from this byte.
- **Byte 12,962 tracks the slot index.** `0xff` in the never-saved A16 dump and
  `0x0f` — which is A16 — in every dump taken after the pattern was saved.
- **The receive direction.** Closed the same day — see "The A4 takes a written
  pattern" below.

#### The emit path is byte-exact against the box's own message

**`local/a4_pattern.py build` reconstructed a 14,841-byte message the A4 itself
emitted, byte for byte, starting from a different dump.** Take the empty A16
capture, apply the eight bytes that separate it from the box's own
trig-on-step-1 capture, re-pack seven-bit MSB-first, recompute checksum and
length: the result is `cmp`-identical to the captured message.

That is the strongest pre-send evidence available without sending, and it is
worth more than each half separately. The decode was already validated by A01's
musical sense; this validates the **encode**, the checksum, the length field and
the ragged final group together, against a witness that cannot be argued with.
The remaining risk in a first send is not "are the bytes right" — it is whether
the box accepts a dump on its receive path at all.

**The ragged group is still a guess, and is fenced rather than resolved.**
12,974 is 1853×7+3, so the last group carries three bytes, and no capture has a
high bit set in it — which means both candidate bit orders encode it
identically and no capture can tell them apart. `encode7` refuses to emit when
that tail is not seven-bit clean, rather than pick one.

#### The A4 takes a written pattern — 2026-08-30, and pacing was the whole of it

**The first send did nothing at all, and the message was not the problem.** The
same bytes that later worked were sent, unpaced, to a box that was in SysEx
receive and demonstrably listening. Nothing happened, twice, either side of a
power cycle.

**The fix is arithmetic that should have been done before the first attempt.**
DIN MIDI is 31,250 baud — ten bits a byte, so 3,125 bytes a second. A
14,843-byte dump takes **4.75 seconds** to arrive over a cable, and that is the
rate a 2013 box was designed against. `MidiOutputConnection::send` hands
CoreMIDI the whole frame at once and it lands in microseconds. Delivering the
same frame in 256-byte pieces, 82 ms apart — 4.8 seconds, DIN rate to within a
tenth of a second — works. `a4_pattern_send` paces by default and `--single`
restores the burst, so the difference stays measurable rather than remembered.

**Three things were ruled out first, and only one of them cheaply.** The port
was unambiguous, and a CoreMIDI virtual-port loopback carried 32 KB byte-exact
— including the real file, chunked and unchunked — which put the fault past our
own framing. That loopback is `digi_midi`'s `sysex_loopback` example and is
worth keeping: "the bytes never left intact" and "the box declined them" are
different problems with no overlap in what you do next.

**What the box does not need is a receive mode.** It took the dump sitting at
its ordinary menu, with `SYSEX RECEIVE` never entered. There is no arming step
between a stray 14 KB SysEx and an overwritten pattern slot, which is worth
knowing before pointing anything else at this box.

**Transfer will relay a raw `.syx` unchanged.** The 2026-08-30 capture of
Transfer sending `send-01` is byte-identical to the file, so it is a second,
independent sender and a useful control — but it is not a witness for this
protocol. Transfer's largest outbound SysEx to this box in the whole capture is
**456 bytes**, because the file API chunks at the protocol level. It never had
to solve the problem this section is about.

**The write is confirmed from the box's own screen**, ~~which is the only
verifier available: the A4 answers no dump request, so nothing here can read
back what it wrote~~ — **a second verifier arrived 2026-08-31: `0x64` fetched
A16 back and it carried exactly the four trigs the probe wrote** (§10, "The A4
answers dump requests"). At the time of this entry the screen was the only
witness. `0x30` was written to track 1 step 1 and the box displays C4 on that
step. That is also where the octave correction above came from.

#### The pattern payload is now completely mapped

The p-lock pool was the missing region, and finding it accounts for every one of
the 12,974 bytes:

| Offset | Size | What |
|---|---|---|
| 0 | 4 | header |
| 4 | 6 × 751 | tracks — 4 synth, FX, CV |
| 4,510 | **128 × 66** | **p-lock pool: `[param_id][track]`, then one value per step** |
| 12,958 | 16 | tail; the slot marker sits at +4 (byte 12,962) |

Inside a 751-byte track: trigs at 0 (2 × 64), notes at 128, `0xff`-filled
per-step lanes at 192, 256 and **384**, a **zero lane at 320**, and a per-track
block from 448 that opens `30 64 0e 00 00 00 40` — default note C4, velocity
100, length 14, centre 64. **`0xff` in a per-step lane means "unset, use the
track default"**, which is exactly why a fresh trig inherits its note from +448.

> **Corrected 2026-08-31.** This paragraph read "192, 256 and 320, a zero lane
> at 384" until the fills were counted rather than remembered: 320 is the zero
> lane and 384 is `0xff`, unanimously across 18 track-instances in three
> captures.

**The block from +448 is not opaque — it holds two more per-step lanes**, at
+532 and +596, `0xff` in a cleared pattern and populated in A01 at exactly the
odd steps SYN1 has trigs on. Removing a note clears that step in both of them
alongside the note lane, so they are trig-attached. They are unnamed: the
values are note-range, which is suggestive and is not evidence.

**All 128 lanes are empty in both A16 and A01**, and every per-step lane in
those captures is `0xff`. So neither pattern carries a p-lock, and the locks
seen on the box after the write — PAN, both ENVs, both LFOs — **cannot have come
from the message**, whose lanes were all `ff ff`. They are pre-existing box or
kit state the write did not touch. That the first write into a slot did not
disturb them is the more useful half of the finding.

**The pool being the p-lock store is a hypothesis with a sharp test**, which is
the point of writing it down before the capture rather than after: lock one knob
on one step and exactly one lane should change its header from `ff ff` to a
parameter id, with the value at that step's index. Nothing changing refutes it,
and is worth as much. `a4_pattern.py pool` is the one command.

**Closed 2026-08-31, confirmed** — the entry below.

### The pool is the p-lock store, and the trig bytes were wrong twice — 2026-08-31, A4 0195

Nine captures, every one of them against a **cleared** A16 or a single change
from the capture before it, so no diff carries more than one variable. The pool
hypothesis above survived exactly as predicted. The trig model beside it did
not, and neither did the correction written for it the same morning.

#### The pool, confirmed, and it is the gen-2 header

Locking FLTR1 FREQ to 64 on SYN1 step 1 moved lane 0's header from `ff ff` to
`22 00` and put `0x40` at step 1, `0xff` at the other 63. That is the predicted
result to the byte.

**The header is `[param_id][track]`** — the same two bytes `protocol::plocks`
reads on the DT2 and DN2, on a box that disagrees with them about value width,
lane size and whether the pool compacts. Confirmed by locking the *same* parameter on
SYN2, which gives `22 01`: param id unchanged, track byte 1. That second capture
was necessary, not decorative. SYN1 is track 0, so `22 00` alone cannot separate
"the second byte is the track" from "the second byte is always zero" — the same
shape as A01's slot 0 being unable to settle whether the checksum starts at 8 or
9, and settled the same way, by varying the axis until zero stops being an
answer.

- **Param ids are per parameter, not per track.** `0x22` is FLTR1 FREQ and
  `0x23` is RESO — adjacent ids for adjacent knobs — and `0x22` appears on both
  SYN1's and SYN2's lanes.
- **The two fills are opposite, which matters for any reader.** A free lane is
  `ff ff` then 64 **zero** bytes; inside an allocated lane `0xff` is a step with
  **no lock**. `cmd_pool` shipped a `v != 0` filter that would have reported all
  64 steps of a real lane as locked.
- **The geometry was forced before the semantics were known.** The region is
  8,448 bytes, `128 × 66` is 8,448, and in a cleared pattern all 256 `0xff`
  bytes sit at predicted header positions with the other 8,192 zero. That is not
  a decomposition fitted to taste.

#### `80 80` is an extension lane, and both generations store the same word

The first p-lock capture allocated **two** lanes for one knob: lane 0 as above,
and a lane 1 whose header read `80 80` with `0x34` at the same step. Three
candidate readings — end-of-pool marker, companion field, low byte — and one
capture that separates them: **change only the lock's value.**

64 → 100 changed **two bytes in the whole 12,974-byte payload**:

```text
4512   lane 0, param 0x22 FLTR1 FREQ, SYN1     0x40 -> 0x64     64 -> 100
4578   lane 1, the 80 80 extension             0x17 -> 0x60     23 -> 96
```

RESO's lane came back byte-identical, which is what it was locked for: a control
that proves the box rewrote nothing but the one lock.

So `80 80` is bound to the lane before it and carries a second byte of the same
value. **The coarse byte is the displayed value** — `0x40` for 64, `0x64` for
100, measured twice — and the extension is sub-unit resolution beneath it. Four
takes of a displayed "64" produced fine bytes of 23, 52, 113 and 116: a knob
landing in different places inside one displayed integer, which is what a fine
byte must look like and what a marker, a count or a companion field could not.

`plocks.rs` records gen-2 storing display × 256 in a `u16be`. **Both generations
store the same 16-bit quantity**, gen-2 inline and gen-1 split across a lane and
its extension, because a gen-1 lane is 64 bytes where gen-2's is 128. `0x8080`
is a continuation marker rather than a parameter id, which is why it never
looked like a plausible one.

Two things about it are still open. **FREQ always allocates an extension and
RESO never has**, so either RESO is integer-valued or the box omits an extension
whose fine bytes are all zero — and that decides whether an encoder emits one
per lock or only when needed. And the fractional reading is inference: that the
coarse byte equals the display is measured, that the fine byte is 256ths of a
display unit is imported from gen-2.

#### Gen-1 compacts the pool; gen-2 does not

Adding SYN2's lock moved SYN1's existing RESO lane from index 2 to index 4, with
the new pair inserted ahead of it, leaving lanes ordered by `(param_id, track)`.
`plocks.rs` documents the opposite for the digis — "the box does not compact the
pool; it clears a freed lane in place and claims the lowest free lane including
holes". **A write path cannot assume a lane index survives an edit on this box**,
and `apply_track_plocks`'s scrub-then-write policy is built on the gen-2
behaviour.

#### The trig bytes: four states, and two wrong models before them

PLAN read the second trig byte as `0xc0` trigless / `0xc1` note, from A01, where
`byte1 & 1` tracked the note lane across 51 trigs on two tracks with no
exceptions. A cleared pattern with one deliberate trigless trig changed **two
bytes**, and refuted it:

| bytes | what it is | what the box shows |
|---|---|---|
| `(00,00)` | bare step | nothing |
| `(00,02)` | trigless trig | a trig |
| `(01,c1)` | note trig | a trig |
| `(01,c0)` | note trig with the note taken off | **nothing** |

Byte 0 bit 0 was **clear** on the trigless trig, and byte 1 was `0x02`. The
replacement model written that morning — `b0 & 1 or b1 & 2` — was also wrong,
and the fourth row is what settled it: `(01,c0)` has byte 0 bit 0 set and the
box displays an **empty step**. No dump can see that. It came from Neil looking
at an unlit LED on A01 SYN4, before and after a factory reset.

**The trig state is `byte1 & 0x03` alone**; byte 0 bit 0 is residue from a note
trig that used to be there. **Confirmed from the write side 2026-08-31** — see
§10 open item 2, now closed — which is the half no dump could reach: the box was
handed all four states authored by our code and displayed each one as this table
says. The regression is the argument: under the corrected
reader A01 SYN4 holds **4 trigs, on steps 1, 17, 33 and 49** — the roots on the
bar lines that this document already used to validate the layout, where both
earlier models counted 19. And `a4-pattern-A01-rmv-trig1` finally reads 31 trigs
against A01's 32, so the capture agrees with its own filename for the first
time. See DEVELOPMENT.md lesson 16.

**And the fifteen residue steps say two more things, measured 2026-08-31 while
building the write probe.** Four of them read `(09,c0)` rather than `(01,c0)` —
byte 0 carrying the positional bit `0x08` *and* the residue bit — so those two
bits are independently set in real data, which no single-parity capture could
have shown and which is why the probe tests both. And **every one of the fifteen
has `FF` in the note lane**: the residue the box leaves is a note trig that took
the *track default* and then lost its state bits, not one whose own note byte was
erased. That is what makes authoring residue a composition rather than a
primitive — `set_note_trig(None)` then `clear_trig` produces those bytes exactly.

#### A factory reset restored A01 to within one byte

A01 dumped after a factory reset differs from the 2026-08-30 capture by **1 byte
in 12,974**: SYN1 `+448`, `0x45` → `0x3c`. Every trig byte, note lane and
per-step lane is identical.

That anchors the trig finding to the exact bytes the analysis ran against, which
mattered because the reset happened between the capture and the observation. It
also confirms A01 is a factory pattern, so the baseline is reproducible. And the
one byte that did drift is the per-track default note — **it moved alone**,
which is independent confirmation that +448 is per-track and not per-step.

#### Two rig notes

**MIDI Monitor archives everything the box sent, not just the dump.** The
two-track capture carries a stray pitch bend from the A4. A short channel
message stores its bytes inline under `dataBytes` where a SysEx stores a
reference under `data`, so a loader that assumes every row is a SysEx does not
mis-parse — it raises and loses the whole capture. `load_mmon` skips non-SysEx
rows.

**A capture whose filename records an intention is not evidence of it.**
`A4-transfer-A16-plocks.mmon` has a payload byte-identical to the capture before
it: whatever it was taken to record, it recorded nothing. Dump a baseline
minutes before the change, and diff against that.

#### What the A4 pattern path still owes

**Items 1, 2 and 3 are closed** — each says so in its own entry, with the date
and the build. **What remains is 4 and 5**: a bit order waiting on a payload no
capture has produced, and a backup policy waiting on a decision.

One thing about *how* they closed is worth keeping, because it is the reason the
list was ordered this way. Items 1 and 2 closed the same day by opposite means:
item 1 was a port, finishable at a desk, and item 2 could only be closed by a
**write**, because it asks whether the box reads its own bytes the way it writes
them. No number of dumps would have moved it, which is why it outlived the
reading work by a morning.

1. ~~**Bring the gen-1 format into `protocol`.**~~ **Done 2026-08-31**, and it
   was a *smaller* job than this bullet says, in the two places the bullet was
   describing our own code from memory. `crates/protocol/src/a4_pattern.rs` and
   `a4_plocks.rs` are the port; `tests/all/a4.rs` is 19 tests against nine committed
   fixtures.

   **Two of the three planned changes were unnecessary.** `sevenbit.rs` needed
   no bit-order parameter — see the correction above — and the framing needed
   nothing at all, because it was already the gen-2 framing. The whole diff to
   existing code is two doc comments and one constant
   (`FAMILY_ANALOG_FOUR = 0x06`). DEVELOPMENT.md lesson 17 is what that turned
   into.

   **`plocks.rs` was left alone, and gen-1 got its own module rather than a
   generation parameter.** The remaining differences are real and they are not a
   width: a gen-1 value is a `u8` plus an optional *sibling lane*, which changes
   the lane→value mapping rather than the size of a field, and gen-1 compacting
   the pool inverts the premise `apply_track_plocks`'s scrub-then-write policy is
   built on. A `Generation` enum would have made a hardware-verified write policy
   conditional in order to express a structure it cannot represent. §7 rule 3
   pointed the same way.

   **The fixtures are committed**, which was the load-bearing half of this
   bullet: nine captures under `crates/protocol/tests/fixtures/analogfour-*.syx`,
   extracted from `local/a4-check/`. Before that they existed on one disk.
   `A4-transfer-A16-plocks` is deliberately not among them — its payload is
   byte-identical to the capture before it, and `every_fixture_is_a_distinct_pattern`
   is the assertion that keeps that class of file out.

   **The reader is complete; the pool writer is not, and refuses rather than
   guesses.** Trig and note writes are ported, including the one hardware has
   confirmed. A pool writer waits on items 3 and 4 below — see item 3.

   **`local/a4_pattern.py` stays, with a smaller job.** It is the tool that makes
   a `.mmon` legible in one command — loading MIDI Monitor archives, annotated
   diffs, `pool` — and none of that belongs in a shipped crate. What it is no
   longer is the authority on the format. `examples/a4_pattern_send.rs` now calls
   `protocol::a4_pattern` rather than carrying its own copy, which is how its
   copy came to be counting trigs by the model the box refuted and printing every
   note an octave low.
2. ~~**The write experiment on the trig bytes.**~~ **Closed 2026-08-31 on A4
   0195, and every one of the seven predictions held.** The model says
   `byte1 & 0x03` alone decides what the box shows; the box was handed all four
   states authored by our own code and displayed each one as predicted. **Steps
   3, 5 and 12 lit, the other 61 dark.**

   Three things that run settled, in the order they matter:

   - **The A4 reads these two bytes the way it writes them.** Everything behind
     the trig model came from dumps the box *sent*, and nothing in it established
     the other direction. It does now.
   - **The sharp half held.** `(01,c0)` on step 9 and `(09,c0)` on step 10 both
     stayed dark, so the box ignores byte 0 bit 0 at **both parities** of the
     positional bit. A set bit that must be ignored is the one thing no capture
     could ever have shown, and it is the reason this experiment existed.
   - **Steps 3 and 12 showed as *trigless* trigs, not merely as lit steps**,
     which is more than the experiment asked for. The prediction was about a
     lit LED; what Neil reported was the box's own interpretation of byte 1
     bit 1, matching what we authored. Step 12 carries `(08,02)`, so the
     positional bit does not interfere with it either.

   Step 1 is worth its own line: the probe cleared a trigless trig the box itself
   had written and the box showed it dark, so `clear_trig` takes as well.

   `set_trigless_trig` and `clear_trig` are no longer predictions, and
   `a4_pattern`'s doc comments say so with the date and the build. The probe is
   kept rather than deleted, for the reason `probe_drive_read.rs` is kept: it is
   the experiment the finding rests on, and a claim whose experiment has been
   thrown away is a claim on trust.

   **How it was built, 2026-08-31.** `a4_pattern::build_trig_probe`
   authors all four states onto SYN1 of A16 and
   `examples/a4_trig_probe.rs` writes the message and prints the prediction;
   three tests in `tests/all/a4.rs` pin it. Nothing here opens a port —
   `a4_pattern_send` remains the only thing in the repo that can write to a box,
   because the consent ceremony, the DIN pacing and the reply listener have no
   business existing twice.

   The layout, and the reason each row is in it:

   | step | authored | bytes | prediction | a disagreement means |
   |---|---|---|---|---|
   | 1 | the baseline's own trigless trig, cleared | `(00,00)` | dark | our clear does not take |
   | 3 | `set_trigless_trig` | `(00,02)` | **lit** | the box refuses a trigless trig we authored |
   | 5 | `set_note_trig`, `0x30` | `(01,c1)` | **lit** | the send did not land — discard the run |
   | 7 | nothing | `(00,00)` | dark | the write reached offsets it did not intend |
   | 9 | a note trig, then `clear_trig` | `(01,c0)` | dark | **the box honours byte 0 bit 0, and the model is wrong** |
   | 10 | the same, on an odd step | `(09,c0)` | dark | the same, with the positional bit set |
   | 12 | `set_trigless_trig` on an odd step | `(08,02)` | **lit** | the positional bit suppresses a trigless trig |

   **The prediction is one line — steps 3, 5 and 12 lit and nothing else — and
   the controls are what make a surprise readable.** Step 5 is the shape hardware
   has already accepted, so a dark step 5 says the cable failed rather than the
   model. Steps 9 and 10 carry the same state at both parities of the positional
   bit, because `0x08` shares byte 0 with the residue bit and one parity cannot
   separate them: the same trap as A01's slot 0 and the checksum start, avoided
   the same way.

   **Step 5 has to be read before the others, and that is not fussiness.** A send
   that never arrives leaves A16 holding whatever it held before, and *what that
   is, is not known* — the slot has been written twice by `a4_pattern_send` and
   edited by hand on the box since. So "nothing happened" has no predictable
   appearance and cannot be distinguished from "the model is wrong" by looking at
   the steps predicted dark. The second half of the guard is the whole-track
   count: the message replaces all 12,974 bytes, so a landed send leaves SYN1
   with exactly three live trigs and 61 dark steps. Three lit *and nothing else
   anywhere in the 64* is a result; those three plus something on step 40 is a
   partial write, and the run gets discarded rather than interpreted.

   Three things the build settled that this bullet had not thought about:

   - **The baseline is the `A16-trigless` capture, not the cleared one**, and
     that is worth a sentence because the cleared A16 is the obvious file to
     reach for. The trigless fixture is one change away from the message the box
     has already accepted — including byte 12,962, which reads `0f` there and
     `ff` in the cleared capture — so the experiment varies the trig bytes and
     nothing else. It also arrives carrying the box's own trigless trig on step
     1, which makes "can we clear something the box wrote" a free eighth question
     at no extra risk. `build_trig_probe` **checks** that baseline rather than
     documenting it: a baseline with something already on the probe's steps would
     frame and send perfectly well and its predictions would silently not hold.
   - **The prediction is hand-written and deliberately not computed from
     `TrigState::is_live`.** A prediction derived from the model under test agrees
     with it by construction and the experiment measures nothing. So `PROBE_STEPS`
     carries what the box is claimed to do, `ProbeStep::state` carries what our
     reader thinks, a test asserts the two agree, and the front panel is the third
     witness that can disagree with both.
   - **Residue is authored by composition, and a measurement says that is
     right.** `set_note_trig(None)` then `clear_trig` leaves `(b0|01, c0)` with
     the note lane at `FF` — which is byte for byte what all fifteen residue
     steps of A01 SYN4 hold. A `set_residue` primitive would have been one call
     and would have hidden both the derivation and the note lane.

   The part a desk could not do took one send and one look at sixteen LEDs.
3. ~~**The extension-lane rule.**~~ **Closed 2026-08-31 on A4 0195: FLTR1 RESO
   is integer-valued.** Four RESO locks on one SYN1 lane at **0, 50, 90 and
   127** — both ends of the range and two points inside it — and the pool
   allocated **no extension lane at all**. The competing reading, that the box
   omits an extension whose fine bytes are all zero, would need all four of those
   to have landed on a fine byte of exactly zero: four independent 1-in-256
   accidents. 0 and 127 in one lane also say RESO spans the full 0..127 as
   integers, which is what 128 discrete positions look like and what sub-unit
   resolution does not.

   `analogfour-A16-plock-reso4-freq64-2026-08-31.syx` is the capture, committed,
   and `four_reso_values_on_one_lane_allocate_no_extension` pins it.

   **The control paid for itself twice.** FREQ on the same four steps allocated
   its extension as always — so the capture is good — and the extension turned out
   to hold its fine byte at exactly steps 1, 5, 9 and 13 with `FF` at the other
   sixty, matching its parent lane position for position. Every p-lock captured
   before this one sat on step 1, so **"an extension is indexed per step" was
   inference from the lane geometry and is now a measurement.** What it still does
   not show is a fine byte *differing* between steps of one lane: all four read
   23, which is one gesture applied to four held trigs rather than four turns. A
   narrower gap than before, and not a closed one.

   **The encoder rule was the same under either answer** — emit an extension iff
   some fine byte is non-zero — so what this closes is confidence rather than the
   rule. **The pool writer is still blocked**, on the sibling unknown below:
   whether the box *requires* the compacted order it produces. That one is a write
   test, because no dump can show a device requiring something.

   **This is why `a4_plocks` has no write half**, alongside a second unknown
   found while porting: that the box *produces* a compacted, `(param_id,
   track)`-sorted pool is measured, but whether it *requires* one is not. Both
   are pinned as observations — `freq_always_has_an_extension_and_reso_never_does`
   and `a4_plocks::is_compacted` — rather than turned into rules. The reader is
   finished and the writer waits for the capture, which is the same refusal
   `build_pattern` makes about item 4 one level down.

   **How the capture was aimed, kept because it is the argument the dump rests
   on.** The two readings were not equally supported, which was measured while
   setting the experiment up and changed what the experiment had to be. The
   sentence above says "RESO never has" and the test loops over five fixtures,
   which reads as five independent RESO samples. It is one. Across every capture
   there is exactly **one distinct RESO lock** — SYN1, step 1, coarse 100 — seen
   four times because RESO was the *control* in those diffs and was deliberately
   not touched. FREQ, over the same fixtures, has **five distinct fine bytes and
   not one of them is zero**, which is what a fractional parameter looks like.

   So "the box omits an all-zero extension" requires RESO's single sample to have
   landed on a fine byte of exactly zero — a 1-in-256 accident — where "RESO is
   integer-valued" needs no accident at all. The prior favours integer-valued at
   roughly 256:1, and n=1 is why that is worth nothing yet.
   `the_reso_observation_rests_on_a_single_lock` pins the counts so the next
   reader does not have to re-derive them.

   **The capture, and it is one dump.** From a cleared A16, put **trigs on steps
   1, 5, 9 and 13 of SYN1** — every p-lock capture so far carries a trig on the
   locked step, so a lock lives on a trig and this is a prerequisite rather than
   a detail. Then p-lock **RESO on those four steps** to four clearly different
   values, with ordinary encoder turns rather than fine-adjust: an ordinary turn
   is exactly the gesture that produced FREQ's fractional fine bytes, so it is
   the gesture under test. Then p-lock **FREQ on the same four steps** as a
   control, since FREQ is known to allocate an extension and its lane is what
   says the capture is good. Save, dump from the front panel, and run
   `examples/a4_plock_extension_check.rs` over it.

   - RESO's lane **has** an extension → the box omits an all-zero one, and an
     encoder emits one only when some fine byte is non-zero.
   - RESO's lane **has none across four distinct values** → RESO is
     integer-valued, and the other reading would need four separate 1-in-256
     accidents.

   **Both outcomes give the same encoder rule** — emit an extension iff some fine
   byte is non-zero — and that is worth knowing before the capture rather than
   after, because it means the writer is unblocked either way. What neither
   outcome touches is a third possibility: that some parameter *requires* an
   extension even when its fine bytes are all zero. **No dump can show a device
   requiring something**, so that is a write test, and it is the same shape as the
   compaction question in the paragraph above.

   **The four-step control is not only a control.** Every p-lock in every capture
   so far sits on step 1, so a four-step FREQ lane is also the first measurement
   of an extension carrying a fine byte **per step** rather than the lane geometry
   implying it. That gap was found by the same count as the RESO one, and it costs
   nothing to close in the same dump — §12's finding again: vary the axis the
   question did not obviously need.
4. **The ragged final group.** 12,974 is 1853×7+3 and no capture has a high bit
   in the last three bytes, so both bit orders encode it identically and no
   capture can settle it. `encode7` refuses rather than guess. A payload that
   ends high-bit-set would settle it in one dump.

   **The refusal is ported and is now `a4_pattern::build_pattern`'s**, not
   `encode7`'s — `encode7` is shared with the gen-2 path, which has no such
   ambiguity and must not start refusing things on gen-1's behalf. Every
   committed A4 payload ends `00 00 00`, so the question stands exactly where it
   did.
5. **A backup before a write.** `safe_write_track` stashes what it is about to
   overwrite; `a4_pattern_send` does not, ~~because the A4 answers no dump
   request and so cannot be read first~~ — **it can be read first as of
   2026-08-31 (`0x64`, §10), so this stopped being a limitation of the box and
   became unbuilt integration.** ~~The backup has to be a front-panel dump, and
   nothing enforces that it happened.~~ A factory reset is a recovery path for
   the *factory* patterns and nothing else — it restored A01 to within one
   byte, and it destroys anything since.

#### What this does to §10.5

Writing a pattern to an A4 no longer depends on the multi-chunk +Drive write.
The route is a ~15 KB SysEx message to a box at whatever menu it happens to be
on — no receive mode, per this section's own measurement — not a 2 MB
read-modify-write of a whole project — which would have rewritten all 128
patterns, every kit and the sound pool to change 64 steps, and clobbered
anything touched on the box in between. Multi-chunk is still wanted for project
backup and restore and for DT2 kit saving. It is no longer what stands between
this project and an A4 pattern.

### The A4 answers dump requests — 2026-08-31, A4 0195

**"The A4 answers no dump request" was never tested; it was inferred, and the
inference was from the wrong namespace.** The claim rested on one fact since
2026-08-28: the supported-opcode reply lists `01,02,03,04,06,07,09` and
`50`–`5e`, and no `0x6x`. But 2026-08-30's trap #1 (§9) established that the
advertised list describes the **API namespace** — where `0x54` is FileOpen —
and the *dump* namespace is invisible to it: the box demonstrably speaks dump
messages in both directions, and those opcodes (`0x53`, `0x54` under a dump
header) are not in the advertised list either. Nobody connected the two facts,
and no request was ever sent. That is **lesson 11's fifth instance**, and the
most expensive shape yet, because two sections of this plan and a module's
whole design rationale (`midi::a4_transfer`'s "not a method on
`ElektronDevice`, deliberately") were built on it.

**One sweep settled it.** `examples/a4_dump_probe.rs` sends each request
opcode `0x60`–`0x6e` in the A4's own dump framing (family `0x06`, empty
payload — `build_dump_message`'s output exactly), listens raw rather than
through `fetch_dump`'s matcher so a surprising reply is a finding rather than
filtered noise, and asks `0x01` Device after every message so a wedge names
its culprit opcode. The box answered eleven of fifteen, never wedged, and
still answered `0x01` at the end.

The map, all checksums and counts verified, every reply saved under
`local/a4-check/dump-probe/`:

| request | response | object | count/addressing |
|---|---|---|---|
| `0x60` | a **417-frame stream** | the whole project | 128 kits + 128 pool sounds + 128 patterns + 16 × `0x55` + 1 × `0x56` + 4 × `0x57` |
| `0x61` | silent | — | |
| `0x62` | `0x52`, 2,410 B payload | kit | index = slot |
| `0x63` | `0x53`, 350 B | pool sound | index = slot |
| `0x64` | `0x54`, 12,974 B | **pattern** | index = slot, linear 0–127: 1 → A02, 15 → A16, 16 → B01, all verified |
| `0x65` | `0x55`, 1,304 B | unidentified — 16 per project | |
| `0x66` | `0x56`, 366 B | project settings — 1 per project | |
| `0x67` | `0x57`, 2,277 B | global — **4 per project**, the box's exact GLOBAL slot count | |
| `0x68`–`0x6d` | `0x58`–`0x5d` | the same six objects, **current state** | index ignored, echoed as 0 |
| `0x58` sent | — | **stores the working kit** | verified 2026-09-01; the preset-load path |
| `0x6e` | silent | — | |
| `0x6f` | untried | excluded from the sweep; `0x60` already streams the project | |

Three findings inside the map:

- **The formats are the front-panel formats, byte for byte.** A requested
  pattern is 14,843 wire bytes and `parse_pattern` reads it with nothing
  added — slot, trigs, checksum, count. The request path is not a second
  format to map; it is the mapped format, on demand.
- **`0x68`–`0x6d` are working-state requests.** Each returns its `-6`
  sibling's object with the index byte ignored (echoed as 0). During the sweep
  `0x6a`'s reply was byte-identical to saved A01; minutes later it had drifted
  by 29 payload bytes while every saved slot's reply stayed put. "Current
  pattern, unsaved edits included" is the reading; the sharp test — turn one
  knob, re-fetch, watch the bytes move — has a person in it and has not been
  run. Note the digi parallel nobody has re-read: on the DN2, `0x68`/`0x69`/
  `0x6a` "answer at index 1 and nowhere else" (§9) — those may be this same
  working-state family, misread as slot requests that mostly fail.
- **The first read-back of our own write.** `0x64` at index 15 returned A16
  carrying exactly the four SYN1 trigs `build_trig_probe` wrote on
  2026-08-31 — verify-after-write over the wire, the thing §7 rule 1 could
  not have on this box.

**What this unlocks, and what it does not.** `ElektronDevice::fetch_dump`
works on the A4 *today* — the reply echoes the requested index, so the strict
matcher matches; the probe bypassed it for discovery, not necessity. That
means re-fetch before encode, backup before write, and read-back verify — four
of the five safety rules this box was exempted from — are now wire questions
with known answers, and `a4_transfer`'s listener remains only for what it was
always right about: dumps a person starts from the front panel. What it does
*not* unlock by itself: the `0x5x` *store* side beyond patterns (kit/sound/
settings stores are untried), and the A4's `0x6b` is **not** gen-2's
kit-track-sound — it is the `0x65` twin — so §10's load-onto-track question
stays open. The 16 × `0x55` object is unidentified; its frames are saved.

`sysex: None` on the A4's product entry is now wrong twice over — the box has
a dump family (`0x06`, already in `protocol.rs` as `FAMILY_ANALOG_FOUR`) and
answers requests on it. Wiring that in is the integration step, not a probe.

### The A4 joins the digi transfer path — 2026-08-31, tests and a screen, no cable yet

**The integration step the entry above named is built: the Analog Four fetches
and writes from the same IN/OUT rows as the digis, through the same five-rule
ceremony, and `ui::a4` is gone.** The FRONT-PANEL DUMP groups, the listener with
its two-minute arm window, the typed `overwrite` consent, the SAVE .SYX button
and the "receive this slot first" rule all existed because the box could not be
asked for a pattern — and it can (`0x64`, above) — so the whole special case
collapsed into the paths that already had the safety rules.

What each layer got:

- **`PatternRoute::FrontPanelDump` became `RequestGen1`**, and both request
  routes label themselves "fetch + write": which generation of dump protocol a
  box speaks is plumbing, not a fact a person routing a desk acts on. The A4's
  product row finally carries `family: Some(0x06)`, and `wire_slots` is data on
  the model row (256 / 256 / 128), so no picker offers I01 to a box whose banks
  stop at H.
- **IN**: `ui::transfer` plans and lands both formats — the worker decodes with
  the format of the box that *answered* (decision 2 held), and an A4 landing
  reports its own losses (trigless trigs, the invented velocity/length, said in
  CAUTION rather than buried).
- **OUT**: `safe_write::a4_safe_write_tracks`, the gen-1 twin of
  `safe_write_tracks` — same gate (allowlist row `analogfour`/`0195`), same
  order (re-fetch, confirm, stash, send, read back, byte-compare), different
  insides (no `Spec`, two lanes per track, no p-locks/PROB/swing to carry, the
  send DIN-paced by the `PatternIo` impl). `ui::sync` and `ui::write` branch
  once at plan time (`JobWrites`/`PlannedWrite`) and once at ceremony time;
  everything between — the survey, the one dialog with per-row opt-out, the
  per-slot backup, `changed_since_survey` — is format-blind and shared.
- **The re-fetch retired the baseline.** `DumpBaseline` — 26 KB of hex per
  pattern in the project file, and the "you cannot send to a slot you have not
  received from" rule — existed because "the A4 cannot be re-fetched". Deleted
  whole: the write is composed on the destination read moments before the send,
  so the unmapped 10 KB is the destination's own, which is `safe_write`'s exact
  bargain on the digis. A day-old project file with a baseline in it still
  loads; serde drops the field.
- **Backups and restore came along for free — after two real bugs.**
  `Stash::payload` and `Stash::scan` keyed on the gen-2 `0x50`, so every A4
  backup was a file the restore list could show and never read (found by the
  first test that read one back); backups are framed per family now
  (`pattern_dump_type`, the A4's `0x54`), and `safe_restore_pattern_kit` runs
  unmodified. And `ui::presets::load_blocker` keyed on "has a dump family" as
  its proxy for "answers the kit-track `0x6b`" — giving the A4 its real family
  byte would have switched preset loads *on* for a box whose `0x6b` is a
  different message; it keys on the gen-2 route now.

**Evidence, and its honest boundary.** 46 new-or-moved tests, none needing a
box: the ceremony end to end against captures (`protocol/tests/all/
a4_safe_write.rs` — ordering, RMW byte-carry, cancel, cross-format refusals,
verify failure, restore round trip), the planner (`core::a4_transfer` — chords
to the root, past-64, off-grid, lanes-don't-travel), and the panel flow
(`app/tests/all/sync.rs` — an A4 desk and a mixed three-box desk through one
dialog). Screenshots of the running panel with three live boxes: the A4's rows
sit between the digis' with the same pickers, and its send button reads "Write
back to A01 · 2 tracks > A01" off provenance.

**Closed on 2026-09-01: the box.** A full round trip ran through the panels —
fetch DT2 A01 (11 notes, 4 tracks) and A4 A01 (39 notes, 2 tracks), edit SYN4's
notes in the roll, write back — and the verify line read "Wrote 39 notes to 2
tracks of A01 — verified byte-identical", with the pre-write backup at
`analogfour-A01-backup-2026-09-01T16-04-17.syx`. So the DIN-paced send, the
settle before the verify re-read, and the per-family backup framing all hold on
a cable, and the paragraph below is what was owed before that run.

**What no test here could claim, before that run: the box.** No A4 write had
gone through the new path on hardware. Its pieces have (the round trip and the `0x64` read-back of a
written slot, above) — that is why `0195` is in the allowlist — but the full
cycle as one flow, and the settle between the DIN-paced send and the verify
re-read, meet an A4 for the first time on the first press. The verify is the
net: a send the box ignored comes back as a loud byte-diff failure with the
backup named, not as a quiet "sent". On Windows that is the *expected* outcome —
`CAN_PACE` is false there, the send collapses to the one unpaced call that has
never worked, and the verify will say so.

### Six lanes get names — 2026-09-01, 132 patterns off the project stream

**The A4 import was honest about a real hole: velocity and length arrived at
this app's defaults, and the CAUTION line said so on every A4 fetch.** Closing
it needs the per-step lanes named, and the evidence that names six of the nine
came from a capture set nobody had mined — the 128 patterns the `0x60` project
stream returned on 2026-08-31, saved under `local/a4-check/dump-probe/`. That is
132 patterns and **5,307 live trigs** against the nine fixtures' 18
track-instances, and it is free: already on disk, no box, no knob.

Two measurements did the work.

**One — lane alignment.** For each candidate base, count the non-fill bytes that
land on a step with a trig against those that land on a bare step. A lane
aligned to the trig lane scores almost everything on trigs; a base one byte off
smears across both. The margins are not close:

```text
   base 456  478 on trigs / 2181 not      base 460  137 / 942
   base 457  371 / 1500                   base 461  297 / 1574
   base 458  281 /  804                   base 462  371 / 2288
   base 459  280 /   15   <-- the lane
```

That settles the shape of `+459`, which this table recorded as "64 bytes of
`FF`" — a claim built on nine captures that happened to have nothing in it. It
is a per-step lane like the rest, set on 280 trigs. The same scan confirms
`+320`, `+384`, `+532`, `+596` and `+660` on their documented bases, and shows
`+384` is `FF` on all 5,307 trigs: nothing in this project has ever written it.

**Two — the per-track default block at `+448`.** Its 11 bytes read
`30 64 0e 00 00 00 10 00 00 00 01`, and byte 0 was already known to be the
default note. Take the mode of each lane's set values, per track, across 792
track-instances, and ask which default it matches:

| lane | matches default | and no other |
|---|---|---|
| `+128` note | 0 — 44 times | yes |
| `+192` | 1 — 129 times | yes |
| `+256` | 2 — 48 times | yes |

Default 1 is `0x64` in **784 of those 792 track-instances**: 100, the velocity
Elektron ships. (This paragraph said *every* one of the 792 for a few hours, off
the top of a histogram rather than its tail. The other eight are a per-track
default somebody turned — `0x6e`, `0x7f`, `0x68`, and one `0xFF`, which is a
default that is itself unset and which `resolve` now has a case for. The pairing
argument never needed the stronger claim, and A07 on the box is one of the
eight.) Default 2 is `0x0e` in all but one. So the block is the
track's trig defaults in lane order — note, velocity, length — and the lanes at
`+192` and `+256` are velocity and length. The value ranges agree from the other
side: `+192` never exceeds `0x7f` in 3,959 set bytes and peaks at `0x64` (2,626)
then `0x7f` (646); `+256` clusters on short values with `0x0e` the mode and
`0x7f` a distinct spike of 139, which is what an INF entry at the top of a
length menu looks like.

Three more names come from a structural argument rather than a correlation.
`+532`, `+596` and `+660` hold note-range values, they are set on 167, 58 and 2
trigs, and **they nest**: only twice in 5,307 trigs is a later one set without
its predecessor. Independent fields do not nest. They are notes 2, 3 and 4 of a
trig — which this app currently drops on the floor, silently, on any A4 pattern
with a chord in it.

**What is still unnamed, and why the box has to say.** `+384` (never written in
this project) and `+459` (280 trigs, values clustered under `0x14` with
occasional larger ones). The A4's per-trig **condition is one of those two**:
unlike the digis, which spend three lanes on COND, FILL and PROB, the A4 puts
probability and trig condition on a single knob, so it is one enum and it fits
in one lane. A correlation cannot say which lane, cannot order the enum, and
cannot say what a `0x0e` length *sounds* like. Only the front panel can.

`a4_pattern::LANES` is this table as data, with `LaneEvidence` on each entry —
`Hardware` for the note lane, `Correlated` for the six named here, `Shape` for
the two that are not. `describe_offset` refuses to print a name for a `Shape`
lane, and a test enforces that: a diff that labelled `+459` "condition" would be
the model deciding the experiment before the box was asked, which is precisely
how the three refuted trig models happened.

### The knob protocol — what the box is being asked, and how

`examples/a4_lane_probe` makes each measurement one line of output. It polls the
**working** pattern (`0x6a`, reply `0x5a`, 12,974 bytes in the same layout) and
prints a named diff whenever a byte moves. Two properties make it the right
instrument:

- **It is read-only and needs no save.** `0x64` reads a stored slot, so mapping
  a field that way costs a save per measurement — and a save is the one step
  that can lose somebody's work. The edit buffer answers immediately: the
  2026-08-31 sweep got A01's 32 saved trigs *plus two the box had not saved*.
- **It names the lane, not the field.** The line is `SYN1 step 3 lane +459
  (UNNAMED): ff -> 06`. What was turned is the knob; what it wrote is the lane;
  nothing in the tool decides which is which.

The run, on A16 so nothing musical is at risk:

1. `cargo run -p digi_roll_studio --example a4_lane_probe -- --save local/a4-check/lanes`
2. Trigs on SYN1 steps 1-8. Then, one at a time, holding the trig:
   **step 1 VEL to minimum, step 2 VEL to maximum** — pins the velocity range
   and its ends against `+192`.
   **step 3 LEN to its shortest, step 4 LEN to INF** — pins the unit and the INF
   byte against `+256`, which is the one thing no capture can supply.
   **step 5 micro timing hard left, step 6 hard right** — pins the sign and the
   range of `+320`.
   **step 7 to 1% probability, then walk it: 50%, 99%, FILL, the inverse FILL,
   PRE, NEI, 1ST, LST, 1:2, 2:2** — every stop prints its own byte, and that
   list *is* the enum in the box's own order. Whichever lane the first turn
   writes is the condition lane, and the other of `+384`/`+459` stays nameless.
3. Anything left over: RETRIG on step 8, and a chord held on step 1, which
   should light `+532` and confirm the nesting argument from the write side.

Every captured payload lands in `local/a4-check/lanes/` as a `.syx`, so the
session leaves behind the fixtures the tests will need and no measurement has to
be repeated to be committed.

### The knobs answer — 2026-09-01, A4 0195

**Four lanes are hardware-measured now, and the condition lane is the one the
correlation had backwards.** Neil ran the probe against A16 and isolated one
field per step on SYN4; the fixture `analogfour-A16-lanes-2026-09-01.syx` is the
final state of that session and every byte in it is one turn of one knob.

| step | knob | lane | bytes |
|---|---|---|---|
| 1, 2 | VEL, min then max | `+192` | `0x01`, `0x7f` |
| 3, 4 | LEN, shortest then top | `+256` | `0x00`, `0x7f` |
| 5, 6 | micro timing, both ends | `+320` | `0xe9` (-23), `0x17` (+23) |
| 7 | TRC | **`+384`** | walked `0x00`-`0x1f` |

So velocity is 1-127 with a floor of **one, not zero**; length is 0-127 from
zero; micro timing is signed ±23, exactly Elektron's range, in a lane that
clears to zero rather than `FF` — so it has no "unset" and needs no fallback.

**The condition is `+384`.** That is the lane which is `FF` on all 5,307 trigs
of the project stream, and the value-distribution argument had therefore made it
the *less* likely of the two candidates: `+459` looked far more like a used
field. It was a good argument and it was wrong, which is the fourth time on this
format that a correlation nobody had put in front of the box pointed the wrong
way (DEVELOPMENT.md lessons 14, 16, 17). `+459` is still nameless and still set
on 280 trigs somewhere in this project — a real field with no knob found for it.

**What the walk does not give: the menu.** The bytes are an index into the TRC
list and the walk covered `0x00`-`0x1f`, but nobody recorded which label was on
the screen at which stop, so the byte-to-condition table is unmapped and
`CONDITION_SEEN_MAX` is where a hand stopped rather than where the menu ends.
Three things are known about the list from the box itself: it is one knob, so
probability and condition are one enum — the digis spend three lanes on COND,
FILL and PROB and this format spends one — and **there is no `LST`**, so
`conditions.rs`'s 76-entry gen-2 table is not this menu with entries missing,
it is a different menu. Whatever maps between them is a real translation, not a
subset.

**Two corrections the session forced.**

- **The working reply's index is the loaded slot.** The 2026-08-31 sweep read
  every `0x6a` reply back as index 0 and recorded "the box stamps zero" — but
  A01 was loaded, and A01 *is* slot 0. With A16 loaded every reply came back
  stamped 15. The request index is ignored; the reply index tells you which
  pattern the box is sitting on, which is how a session can notice somebody
  changed pattern under it.
- **Byte 4 of the default block is not the track's default condition**, or at
  least not on the evidence available. It moved `00 -> ff -> 00` with no trig
  held, which is exactly what a track-level default does — but it reads `0x00`
  on a cleared track, and a box does not ship every track defaulted to menu
  entry zero. One of those two readings is wrong. Recorded, not acted on: an
  unset condition byte means *no condition*, and there is deliberately no
  `effective_condition` to invent one.

**And a name that has to come off.** `+532`/`+596`/`+660` were called notes 2-4
of a chord from the nesting argument. The A4 is not polyphonic — chords come
from the ARP menu, and NO2/NO3/NO4 are its p-lockable note parameters. The
nesting is still real and still says these three lanes are one ordered group,
but "chord notes" was this app's word for it, not the box's.

### The second session — the length scale, the arp, and what retrig is not

**The A4 shares the digis' note-length scale, and two screen readings proved
it.** `0x00` shows `.125`, and the value below `INF` shows `128`. Those are the
two ends of the gen-2 curve `pattern::length_byte_to_steps` already ports from
libanalogrytm — piecewise linear, doubling every 16 values — and this format's
own per-track default falls on its third anchor: `0x0e`, which that curve puts
at exactly one step, is the default length in every A4 capture we hold. So an A4
length byte means on this box what it means on a Digitakt II, no table of its
own and no conversion at the boundary. `0x7f` is `INF`, which is why
`LENGTH_LONGEST_FINITE` exists separately: a clamp cannot target infinity.

**`+532`/`+596`/`+660` are the ARP menu's NO2/NO3/NO4, per trig.** Turned on
held trigs of SYN1, SYN3 and SYN4 and lit in that order. They had been called
"chord notes 2-4", from the nesting correlation — right about the shape, wrong
about the field, because the A4 is monophonic per track and its chords come from
the arpeggiator. The unit was open for a day: a fresh one takes `0x40` first,
which reads like the centre of an offset range rather than a note, and no screen
was read at that moment. **Closed 2026-09-02** — see "Chords reach the A4"
below: `0x40` is zero semitones, the menu runs -64..+63, and it was the factory
A01 that fixed it rather than a screen.

**There is no retrig on the A4.** The first guess for the last unnamed lane was
retrig; the box does not have it, and neither the DT2 nor the DN2 path in this
app carries retrig either, so nothing is owed on that side. `+459` is set on 280
trigs of the project stream and still has no knob — a real field with no name,
and the honest state of it is `LaneEvidence::Shape`.

**Two more things the second session recorded, neither acted on yet.**

- **Re-adding a trig clears its condition.** The box was watched removing the
  trig on step 7 (condition survived, at `0x1f`) and putting it back
  (condition went to `FF` in the same frame). `set_note_trig` here does not do
  that, so an authored trig landing on a step that once had a condition would
  inherit it where the box would not — a fidelity difference the write path has
  to decide on, not a primitive's business.
- **The second TRC sweep reached `0x40`**, where the first stopped at `0x1f`.
  Which is the argument for `CONDITION_SEEN_MAX` being documented as a ceiling
  somebody's hand stopped at rather than the length of the menu — the first
  sweep would have supported a 32-entry table, and it would have been wrong.

**And the menu is still the one thing outstanding.** Both sweeps recorded bytes;
neither recorded the label on the screen at each stop, so there is no
byte-to-condition table. Until there is, this crate can carry an A4 condition
through a round trip byte-exactly and cannot render one, name one, or translate
one to a digi's — and it says so rather than guessing.

### Three of the four fields now travel — 2026-09-01

**Velocity, length and micro timing go both ways, and the CAUTION line is
gone.** The import reads each lane, resolves an unset one through the track
default the way the note lane already did, and decodes it with the *gen-2*
codecs — `length_byte_to_steps`, `micro_byte_to_steps` — because the A4 shares
those encodings. The export runs the same functions backwards into a new
`A4Step`, which is what `A4TrackWrite::steps` now carries instead of a bare
pitch.

A01 is the test: its SYN1 was played in rather than stepped in, so it arrives at
velocity 127 with 25 notes at 1.8125 steps and seven at 1.75 — a phrase whose
note lengths *vary*, which is what a recorded take looks like and what a
1.0-for-everything import could never show.

**The trig condition is deliberately not carried, and that is what protects
it.** The lane is mapped; the menu behind it is not, so there is nothing to
render. The write therefore leaves those bytes alone, and because it composes
onto a freshly re-fetched destination, every condition on the box survives a
round trip untouched. **The box itself would not do that** — it zeroes a step's
condition when a trig is re-added there, watched on 2026-09-01 — and matching it
would be more faithful and strictly worse: this app can neither show the value
beforehand nor put it back after. The cost of not matching is narrow and
recorded: a newly authored trig on a step that once had a condition inherits it.

So the panel's caution line changed rather than vanished. It now appears only
when the pattern actually has conditions on it, counts them, and says both
halves — that they cannot be shown, and that writing back leaves them alone. A
line that said only the first half would read as damage.

Six new tests hold the two edges that matter: that a write lands in all four
lanes including their ends (velocity 1 is not velocity 0, micro timing is
signed), and that a write leaves the conditions and arp notes already on the
box. The containment test spells its allowed ranges out by hand rather than
deriving them from `LANES` — a test that took its expectation from the table
under test would follow that table into the condition lane, which is the thing
it exists to keep the writer out of.

### The TRC menu, from four labels — 2026-09-01, A4 0195

**The condition table is mapped, and four labels were enough because they
over-determine it.** Neil described the front panel first: from a trig with no
lock the knob lands on `100`, turning left walks down the percentages to `1%` at
the far left, turning right goes to `FILL` and on to `8:8` at the far right.
Then he set four trigs of A16 SYN1 to named values, and the probe read the
bytes:

```text
   step 1    1%     0x00
   step 5    75%    0x0d
   step 9    FILL   0x16
   step 13   8:8    0x40
```

Three separate things fall out, and each one could have contradicted the others:

- **`FILL` at `0x16` puts exactly 22 entries before it**, and Elektron's
  probability ladder is 22 values long. So the percentages are `0x00`-`0x15`,
  and `100%` — the value the knob lands on first — is `0x15`, immediately before
  `FILL`. That is the front panel's own description, reached from the byte end.
- **`75%` at `0x0d` is index 13**, and 75 is the fourteenth rung of that same
  ladder. A different ladder would have had to put 75 in the same place by
  coincidence.
- **`8:8` at `0x40` is index 64**, which only lands if the ratios carry **no
  negations**. With the digis' `!A:B` interleaved the menu would run to 97.

So the A4's TRC menu is 65 entries: 22 percentages, then `FILL`/`!FILL`,
`PRE`/`!PRE`, `NEI`/`!NEI`, `1ST`/`!1ST`, then 35 ratios from `1:2` to `8:8`.
`digi_protocol::a4_conditions` is the codec.

**It differs from the digis' menu in three ways at once**, which is why it is a
second module rather than a table added to `conditions.rs`:

| | digis | A4 |
|---|---|---|
| PROB | its own lane, 1-100 | *inside* the menu, 22 rungs |
| FILL | its own lane, ON/OFF | *inside* the menu |
| `LST`/`!LST` | present | **absent** |
| `!A:B` | present, 33 of them | **absent** |
| entries | 76 | 65 |

A gen-2 trig can set PROB *and* FILL *and* COND; an A4 trig holds one of the
three. So the translation loses in named ways and says which: a probability off
the ladder rounds to the nearest rung, an `LST` or a negated ratio has no
equivalent and is dropped, and a trig with more than one field set keeps the
one that most specifically names *when* it fires — COND, then FILL, then PROB.

**And the write path reversed itself, deliberately.** For the few hours between
`+384` being named and its menu being read, `a4_safe_write_tracks` left the
condition lane alone: a value this app could not display would have been
destroyed on every press, with no way to see it first or restore it after. Once
a condition could survive a round trip, leaving it alone became the lossy
choice instead — a condition removed in the roll would come straight back off
the box. The test that asserted the old behaviour now asserts the new one and
carries the reason both were right in turn. The ARP note lanes are the case that
kept the old answer: named, mapped, and still uncarried, because the model has
nowhere to put them.

**What is fitted rather than measured.** Four labels are hardware; the other 61
are arithmetic from them plus a percentage ladder taken from Elektron's other
boxes. The three structural facts above do not depend on that ladder — but
`41%` at `0x09` and `PRE` at `0x18` are predictions, and if a reading ever
disagrees it is the percentages that move.

### Parity, verified on the box — 2026-09-01, A4 0195

**A pattern built in the studio arrived on the A4 with every mapped field
intact, and a pattern read off the box went back unchanged in everything this
app does not carry.** Two write-backs, and between them they close the parity
claim.

**One — a studio pattern onto A01.** Ten notes, written and verified
byte-identical, then read back through `a4_lane_probe --slot 0` in the box's own
units:

```text
   step  note      VEL        LEN            micro     TRC
      1  A#4      127  (0x7f)         1  (0x0e)   +0     -
      9  A#4      109  (0x6d)    1.5625  (0x17)   +0     -
     13  B4        50  (0x32)         1  (0x0e)   -3     -
     15  C5       109  (0x6d)         1  (0x0e)   +0     75% (0x0d)
     31  C5       109  (0x6d)         1  (0x0e)   +0     1:2 (0x1e)
```

Per-note velocity, per-note length, micro timing and both kinds of condition, on
a pattern the box had never seen. The trig that carried both a PROB and a COND
came through as the COND with the write's warning naming the loss, which is the
one-of-three rule working on hardware.

**And the front panel confirmed a fitted label.** Step 31 reads `1:2` on the
box. `0x1e` is not one of the four anchors — it is where the arithmetic put the
first ratio — so the box agreeing is an independent check of the derived half of
the table, in the direction that matters: this app chose the byte and the box
named it. Step 15's `75%` is the control, and it reads `75%`.

**Two — A07 read off the box and written straight back.** A07 is the pattern
that uses `+459`, the lane nobody has a name for, on 20 trigs. Fifty-six of
12,974 bytes moved:

```text
   36  SYN1 length     FF -> 0x0e
   20  SYN1 velocity   FF -> 0x6e
   +459, arp notes 2-4, the condition lane, the whole p-lock pool: unchanged
```

Every byte that moved is the `FF`-to-explicit conversion, and nothing else moved
at all. So the read-modify-write bargain holds on hardware for four lanes this
app cannot read and an 8.4 KB pool it cannot write — which is the case where a
bug would have been silent and destructive.

**The `FF`-to-explicit conversion is real and is the cost of the model.** A
per-step lane reading `FF` means "follow the track default"; `Note::velocity` is
a `u8` with no way to say "unset", so a write states every lane. Those 56 trigs
sound identical today and no longer track the default if somebody turns it on
the box. Deliberate, and worth a person knowing before they write back a pattern
they did not build here.

**Two losses found by testing rather than by reading, one fixed.**

- **A trigless trig used to be deleted by a write-back.** A01 SYN1 step 33 is
  one; this model holds notes, so an import counts trigless trigs rather than
  carrying them, and the roll had nothing on screen to show. A write-back
  cleared it silently. **A user cannot intend to remove something they were
  never shown** — the same reasoning that kept the condition lane alone while
  its menu was unmapped — so a write now leaves a trigless trig where it finds
  one, and still deletes a note trig on a step the roll shows as empty, because
  that one *was* on screen.
- **A cross-slot copy does not carry the unmapped lanes.** "A07 to B01" composes
  onto B01, so the result has B01's p-locks, arp notes and `+459`. Same-slot
  write-back preserves them; a copy cannot. Inherent to the design rather than a
  bug, and it will surprise somebody.

### The p-lock writer, and the question that inverted — 2026-09-01, A4 0195

**The last blocker is measured and the writer shipped.** Whether the box
*requires* the compacted `(param_id, track)` order it produces was the open
question. It does not — and the answer changes what the encoder must do anyway,
in the opposite direction from the one the question implied.

#### Three writes to A16, one deviation each

Every variant was built from a ground truth the box itself authored (SYN1: FLTR1
FREQ 50 on step 1, RESO 100 on step 5), was byte-identical to it everywhere
except the one property under test, and went through the full ceremony.

| variant | what it broke | what the box did |
|---|---|---|
| A | keys out of order, packed, extensions adjacent | re-sorted; read-back **byte-identical to the baseline**, all 12,974 |
| B | one free lane wedged between two used ones | compacted; read-back **byte-identical to the baseline** |
| C | an `80 80` detached from the lane it extends | adopted it as the *preceding* lane's, and re-aligned its fine bytes |

**The box requires none of the three properties.** It parsed every scrambled
pool, lost no lock, and wrote back its own canonical form. The sorted-compacted
pool is a serialisation artefact of a box that holds p-locks keyed by
`(param_id, track)` and rebuilds them on ingest — which is also the cheaper
explanation of the 2026-08-31 "gen-1 compacts on edit" observation, since
shifting a lane at edit time is a multi-kilobyte memmove for a knob turn.

**And the engineering conclusion is the opposite of the finding.** Because the
box normalises, a pool written in any other order comes back *different from
what was sent*, so `a4_safe_write_tracks`' byte-exact read-back reports a
correct write as a failed one — 10 spurious diffs for the swapped pair, 132 for
the hole. **The encoder must emit canonical order, for the verify's sake rather
than the box's.** That is a stronger obligation than the original question
assumed: it holds even where the box would have forgiven us.

#### What variant C bought, which was not what it was for

C was the throwaway of the three — the encoder would never emit a detached
extension. It settled two things nothing else could.

**An `80 80` binds to the lane physically before it.** `read_all_plocks` has
always read one that way and it was inference: the box had never produced a pool
where a lane and its extension were apart, so nothing tested what "before it"
meant. Handed FREQ's orphaned extension sitting after RESO, the box **adopted it
as RESO's** — the reader's rule, confirmed from the write side on a payload the
box had never seen.

**And an extension is indexed per step**, confirmed by the box doing the
alignment itself: it rewrote exactly two bytes, putting a fine byte at RESO's
locked step and `NO_VALUE` at FREQ's. Previously measured only on the four-step
FREQ lane of the `reso4` capture.

**A third thing, free: the box stores an all-zero extension it is handed and
never allocates one itself.** So "emit an extension iff some fine byte is
non-zero" is a rule the box tolerates rather than enforces — worth knowing which.

#### How the box frees a lane

Caught while the probe was watching A16 being cleared: **`FF FF` into both id
bytes and all 64 values zeroed**, not filled with `NO_VALUE`. An extension lane
between two used lanes is freed the same way. The opposite of what this format's
two opposite fills would suggest, and what `free_lane` does.

#### The writer, and the one property it had to give up

`a4_plocks::apply_track_plocks` rebuilds all 128 lanes, where
`plocks::apply_track_plocks` edits in place. **So lanes belonging to tracks a
write never names change index**, which is the one thing the gen-2 policy exists
to prevent. It cannot be avoided on a box that sorts, so what is held instead is
that their *contents* do not change — same parameter, same values, same fine
bytes.

**Verified on hardware the same day.** A16 carries a two-track pool (SYN1 `0x22`
and `0x23`, SYN2 `0x24`); writing SYN1 through the real path — fetch →
`a4_pattern_to_model` → `a4_track_write` → `a4_safe_write_tracks` — moved 77 of
12,974 bytes, **read back byte-identical**, and SYN2's OVERDRIVE lane came back
with every byte intact having moved from lane 3 to lane 2.
`examples/a4_plock_containment.rs` is the run.

**And the front panel read it back, which is the half the byte-compare cannot
reach.** A pool this app composed from scratch coming back byte-identical proves
the box *stored* it; it does not prove the box *parsed* it, because storing bytes
without understanding them looks the same from the wire. Neil read A16 on the
box: SYN1 step 1 `FREQ 50`, step 5 `RESO 100`, SYN2 step 9 `OVERDRIVE` at max.
Three locks, two tracks, one of them on the track the write never named — every
one of them live. That is the same reasoning the trig model needed on 2026-08-31,
when an unlit LED refuted a model no dump could see (lesson 16), and it is why
the write experiments and the screen are two witnesses rather than one.

**One deliberate exception to byte-exact round-tripping.** A pool holding an
extension whose fine bytes are all zero comes back one lane shorter, because the
encoder emits one only when it carries something. The lane holds no information
— `A4Lane::word` reads a zero fine byte and an absent extension identically — and
the box never produces the shape, so the only way to meet one is to have sent
it. `tests/all/a4.rs` states it as its own test rather than letting the
round-trip test quietly exclude the case.

#### A third parameter id, and why the writer does not need a table

`0x24` is **OVERDRIVE**, named on the box 2026-09-01 (SYN2 step 9, max, stored as
127 with no extension) — joining `0x22` FLTR1 FREQ and `0x23` RESO. Adjacent ids
for adjacent knobs, for a third time.

Three ids out of an unknown total, and none of them in `params::A4_PARAMS` with a
p-lock slot. **So a gen-1 lane travels on the box's own `param_id`, not on a
curated name.** `core::a4_transfer` imports every pool lane by number and writes
it back by number, and the round trip is lossless with no parameter table at
all — where the gen-2 path resolves through `params` and drops what it cannot
name. Copying gen-2 here would have freed nearly every lane in the pool, since
"we cannot name it" would have become "we deleted it".

What that leaves undone is *authoring*: a lane drawn in the roll from a named
knob has no A4 id to be written under, and is refused by name with a warning
rather than aimed at a guess. An A4 parameter table is what closes that, and it
is display-and-authoring work, not round-trip work.

### The A4's p-lock parameter ids, measured — 2026-09-01, A4 0195

**Ninety-two parameters named, one knob turn each**, against
`examples/a4_param_probe.rs` watching the working pattern. No save, no slot
written: a knob turn on a held trig allocates a pool lane, the probe prints the
id, the operator names the knob. `params::A4_SYNTH_PLOCKS` is the table and
`tests` pin the count, the holes and each structural shape.

It closes the gap that made the p-lock writer round-trip-only. A lane off the
box now shows as `FLTR1 FRQ` rather than `param 0x22`.

#### Every shortcut was tried and every one was wrong

Each looked reasonable, and each would have produced a table that was right in
the region it was derived from and confidently wrong outside it:

- **`param_id = nrpn_lsb − 6`** fits FLTR1 FREQ, RESO and OVERDRIVE exactly —
  three for three, against Elektron's own appendix. It sends `osc1.level`
  (NRPN 4) to −2. The offset is *regional*: −6 across the filter, −10 across the
  FX sends. And the LFOs break it structurally rather than numerically — the
  NRPNs lay LFO1 and LFO2 ten apart, the pool **interleaves** them two apart. The
  two numberings are not one order with an offset, and the agreement in two
  regions was a coincidence of those regions being sequential in both.
- **"ids run consecutively in screen order"** holds for the ten filter knobs and
  fails on the very next page swept: the AMP page's envelope row is stride 3.
- **"a page is laid out by rows"** — refuted by AM2. **"by columns"** — refuted
  by the eight knobs after it. The real order pairs AM1/AM2 and then takes each
  row in turn, which is a third scheme neither guess covered.

Four layout schemes appear in one address space — sequential, interleaved by 2,
interleaved by 3, paired-then-sequential — and **no page predicts the next**.
That is the case for having measured all 92 rather than 20 and a rule.

#### What the sweep found on the way

- **TUN and FIN are one parameter.** They are the coarse and fine halves of a
  single lane, which is why the box locks both when either is turned and why a
  ten-knob page yields nine ids. Neil spotted the front-panel behaviour; the pool
  explained it.
- **The fine byte is 128ths, not 256ths** — and that corrects a shipped claim.
  TUNE is the only parameter whose fine byte the box displays a number for, so it
  is the only one that could be calibrated: FIN's −64…+63 maps onto bytes 64…127
  and 0…63, and the box carries from `fine 127` to `coarse + 1, fine 0`. The
  reading in `a4_plocks` was inference imported from gen-2, flagged as inference,
  and half the true value. No live bug — `word()` is still a faithful reversible
  packing and nothing scales an A4 word — but it would have become one the moment
  a scaling table landed.
- **"The coarse byte is the displayed value" is false in general.** Written as
  "measured twice" from FREQ, RESO and OVERDRIVE — all unipolar. TUNE is the
  first bipolar parameter it met: `coarse 63, fine 64` reads on the box as TUN
  **0**, not TUN −1.
- **Noise COL sits at `0x64`, 77 ids from the rest of its page.** Measured twice
  in independent sweeps. It looked like an artefact of a scrambled run and was
  not. A parameter appended by a later OS would land exactly there — inserting it
  at `0x16` would renumber everything above and break every saved pattern — but
  that is a hypothesis needing an early-OS box.

#### The id space is per track kind, which is why this is a synth-only table

Two knob turns on the **FX track** put locks on `0x1a` and `0x29` — both synth
parameters in the table above. So the same byte means different things on
different track kinds, a single flat table would have been wrong, and every FX
p-lock would have been labelled with a confident synth name.

`a4_lane_to_model` therefore looks up a label only for tracks 0–3 and leaves FX
and CV lanes showing their hex byte. Neither has been swept; both stay
notes-only by decision, until there is a use case.

#### Named is not editable, and the split is deliberate

`Param::plock` is documented as `None` until the paramId **and scaling** have
been measured. Only ids were measured here, so none of these went into
`A4_PARAMS` — `writable_params_for("A4")` is still empty and a test says so.
`PLockLane::label` carries the name, `ParamDesc::curated` stays false, and
`ui::plocklane::lane_is_editable` therefore still refuses. **A lane can now be
named and still not dragged**, because dragging needs a scaling and only TUNE has
one.

Nine ids inside the range are unmapped and stay that way rather than being
interpolated: `0x12`–`0x14`, `0x2c`, `0x32`, `0x60`–`0x63`.

### The A4 reads its patch names, off captures that were already on disk — 2026-09-01

**"Analog Four plays over MIDI but has no patch names to read" was wrong in both
halves, and the evidence to prove it had been committed for a day.**

`ui::sync::patch_read_blocker` asked `Device::can_sysex`. That field means "has a
gen-2 `Spec`", it is false on this box and always will be, and it had already
been the wrong question twice — `ui::presets` carries a paragraph explaining why
it does *not* ask it, and `PatternRoute` exists because `sysex.is_some()` had
stopped answering "can this box transfer a pattern". This is the third instance
of one field standing in for a capability it does not name, and DEVELOPMENT.md
lesson 3's fourth: a panel stating, in a sentence a person reads and believes, a
limitation the box does not have.

The box has not been live-only since 2026-08-31. And its kit has carried the
four synth tracks' sound names all along.

#### The layout, from 128 kits rather than one

`0x60` returns the whole project as a 417-frame stream, and 128 of those frames
are kits. The offsets were read off one capture and then **checked against all
128 before any of them was written down**:

```text
  2,410 bytes, the reply to 0x62 (stored) or 0x68 (working):
    +0     u32be    struct version — 11 in all 128
    +4     16 × u8  the kit's name — POLYTRON, STEPPA, DAWN …
    +20    12 × u8  unidentified; six u16be, plausibly the six track levels
    +32             sound 1  ┐ 350 bytes each, back to back — the same
    +382            sound 2  │ 0xBEEFBABA container the 0x53 pool-sound
    +732            sound 3  │ dump returns whole and `decode_a4_sound`
    +1082           sound 4  ┘ already reads
    +1432   978 × u8 unidentified — FX and CV live in here, and neither has
                     a sound to name
```

All four offsets carry the head magic in all 128 kits — **512 containers, no
exceptions**, which is what makes the 350-byte stride a structure rather than an
average. Every embedded sound's own version field is `6`, matching the pool
sound. Every name is printable ASCII, NUL-terminated inside its sixteen bytes.

`protocol::a4_kit` is seventy lines of offsets and `tests/all/a4_kit.rs` is eight
tests against three committed fixtures. **No cable was involved at any point.**

#### It reads the edit buffer, which is more than a digi can offer

`ui::sync`'s own section header says a dump request names a *stored* slot and
that nothing in the Elektron dump protocol as implemented here asks a box what
it is currently playing — so a digi's read gets A01's *saved* kit, true only
while the box sits on A01 with nothing unsaved.

**On this box something does ask.** `0x68` returns the working kit, unsaved
edits included. So the A4's read takes no slot at all, and Neil chose that over
mirroring the digis' shape.

That decision is not free, and the cost is provenance. A patch record's `from`
names a slot; an edit-buffer read never established one. Rather than let the
tooltip print "from A01" over bytes that said nothing of the kind,
`TrackPatch::live` records which kind of read it was, and three things key off
it: the origin phrase ("the kit the box had loaded"), the success line, and
**the staleness suffix, which is suppressed** — staleness is a claim about a
slot, and flagging every live read against a slot it never named would have made
the warning meaningless.

#### Why a pattern-to-kit link was looked for and not used

The obvious alternative — mirror the digis, ask for the kit belonging to the
slot the picker names — needs the pattern to say which kit it uses. **No byte in
the A4's 12,974-byte pattern payload does.** Scanned across all 128 patterns of
the project stream: no offset equals the pattern index, and no offset carries
128 distinct values. The only identity-like byte is the slot marker at 12,962,
which is the pattern's own slot.

Whether the link lives somewhere else (the 16 × `0x55` object is unidentified)
or whether the mk1 simply pairs pattern *n* with kit *n* is untested, and
guessing either would have made every A4 patch name conditional on an unproven
mapping. The edit-buffer read needs no mapping at all.

#### Four sounds, six tracks — a fourth `PatchSound`

The A4 sequences six tracks and its kit holds four. FX and CV are the
sequencer's, not the kit's, and none of `PatchSound`'s three shapes fits:
`Midi` is a gen-2 mask bit over a track that would otherwise hold a sound and
this box has no such mask, and `Unnamed` is a slot a later read might name,
which these two never will be. `PatchSound::NoSound` is the fourth, added for
exactly this. Three shapes were three because each answers a different question
(packet E, 2026-08-20, which deleted an `empty string means no sound` sentinel);
collapsing this into one of them would have been that mistake at one remove.

### The A4's p-lock lanes reach the Edit panel — 2026-09-01

**A gated section can hide a working feature as completely as an unbuilt one.**
`ui::edit::plock_group` opened by resolving `model.spec()` and, on `None`,
printing "This box has no SysEx spec, so there is no parameter table to author a
lane against" and returning. On the A4 that meant a track carrying sixty-one
named lanes off the box listed **none of them** — while the strip under the roll,
which is generic over `Track`, drew all sixty-one correctly the whole time.

The sentence was answering a question nobody had asked there. *Listing* a lane
needs only `PLockLane::param`, which resolves against the lane's own recorded
`device_kind`; a `Spec` is the *authoring* half's business, and it is not even
the right test for that. So the two are now separate:

- the list is ungated and keys off `model.key`, which every box has;
- authoring keys off `writable_params_for`, which asks the question that
  actually matters — has this parameter's p-lock slot been measured — rather
  than `auditable_params_for`, which asks whether it can be heard.

The switch costs the digis nothing: all eleven of each box's parameters are both
auditable and writable, and a test asserts that so the picker cannot quietly
lose an entry. It empties on the A4, which is correct — offering one there would
produce a lane `a4_lanes_for_write` then refuses by name.

**Two emptinesses, two sentences.** "Every parameter already has a lane here" in
front of an empty list is a flat contradiction, and the A4 is the first box able
to reach that state, so a box with nothing to author from says that instead.

### Five A4 p-lock scalings, read off the box's own screen — 2026-09-01

`a4_scale_probe` is the run, and it took four interactions per parameter: hold a
trig, turn one knob to each end-stop, type what the screen says. **End-stops
rather than mid-range values**, because a knob turned fully left is at its
minimum whatever the minimum is, where "about 64" is a reading with an unstated
error bar. Two points fix a line, and the range comes off the box rather than out
of `A4_PARAMS`.

| parameter | id | screen | coarse |
|---|---|---|---|
| `filter.cutoff` | `0x22` | 0..127 | 0..127 |
| `filter.resonance` | `0x23` | 0..127 | 0..127 |
| `osc1.level` | `0x06` | 0..127 | 0..127 |
| `fx.overdrive` | `0x24` | −64..63 | 0..127 |
| `filter.envDepth` | `0x26` | −64..63 | 0..127 |

**Every id the label join predicted was the id the box moved** — seven for seven,
including the two LFO depths below. That join was made by matching `A4_PARAMS`'
"FLTR1 FREQ" against `A4_SYNTH_PLOCKS`' "FLTR1 FRQ" *by eye*, which is not a
measurement, so the probe reports the lane that actually moved on every line and
the run is what turned it into one.

#### The bipolar result is not an offset in the scaling, and the probe said it was

The run's first version concluded that `fx.overdrive` and `filter.envDepth`
"need a new `PLockScaling` variant" because their screens read −64..63 against a
coarse byte of 0..127.

**They do not, and the reason is that this app already declines to mirror that
offset on two other boxes.** `Param::describe` gives *every* curated parameter
`min: MIDI_MIN, max: MIDI_MAX` — the display axis is the raw byte everywhere —
and all ten bipolar parameters the digis carry (`filter.envDepth`, `amp.pan`,
three LFO depths, on both DT2 and DN2) are `scaled_plock(id, 256)` with no
offset anywhere. A DT2's own ENV DEPTH screen reads −64..+63 too. The offset is
the gap between a box's *label* and its byte, and a third box is not the place
to start modelling it.

So all five take `scaled_plock(id, 256)`, unchanged machinery. The probe's
verdict was corrected in the same change, because a diagnostic that reaches the
wrong conclusion is worse than one that reaches none.

`fx.overdrive` is also the one place the appendix was doubted and held: the run
was set up expecting that flag to be wrong, and the box agreed with Elektron.

#### What curating actually cost, which was not the table edit

Adding five `plock` fields is four lines. Two things had to happen around them.

**The FX and CV tracks would have been corrupted.** `0x22` is now a curated id,
so `param_by_plock_id` answers for it — and the A4's id space is **per track
kind** (measured 2026-09-01: an FX lock landed on `0x1a` and `0x29`, both synth
parameters). A `PLockLane` does not carry its track. So an FX-track lane on
`0x22` would have resolved to `filter.cutoff`, become editable, had its stored
word `0x4000` read as a display value and clamped to 127, and been written back
as `0x7f00` — a lane nobody touched, changed and wrong.

The rule that prevents it: **on this box a bare id is not admissible evidence.**
`params::plock_id_identifies_parameter` is false for the A4, `PLockLane::param`
resolves by canonical *name* only, and `a4_lane_to_model` — the one place that
knows the track — is what sets that name, for synth tracks and nothing else.

**Neil's call for the first A4 release: FX and CV stay read-only**, their trigs
and locks preserved exactly as fetched, and nobody goes down the sweep rabbit
hole for them. That falls out of the rule above rather than needing a second
mechanism, and
`an_fx_lane_is_read_only_and_survives_the_round_trip_unchanged` is the test.

**A curated lane loses the box's fine byte**, which is the same accepted loss
`display_from_stored` already documents for the digis: the box records
sub-display-unit resolution from a knob landing between integers and this app's
axis is integers. It is 1/128 of one display unit, on a lane being edited. An
*uncurated* lane is still passed through untouched, which is why the FX/CV
promise above is byte-exact and this one is not.

#### The second run, and the same category error a third time

A second pass closed five more: `fx.delaySend` `0x2e`, `fx.reverbSend` `0x2f`,
`amp.pan` `0x30`, `lfo1.depth` `0x5c`, `lfo2.depth` `0x5e`. **Ten of thirteen
`A4_PARAMS` entries are now writable**, and the three left are named below rather
than counted.

Two probe bugs came out of that run, and both are worth keeping because they are
the same mistake at different depths.

**The screen's resolution is not the scaling either.** The first run rejected
both LFO depths for a slope other than 1: they read −128..127 across a coarse
byte of 0..127, so the box shows *two* display units per count. That was recorded
above as "a genuinely third shape ... nothing in `PLockScaling` expresses it",
and it was wrong for the reason the offset was wrong one paragraph earlier — a
DT2's LFO DEPTH has the identical screen and ships as plain
`scaled_plock(29, 256)`. The app addresses every other value of that screen on
every box, and always has. Having just corrected the offset case, the same error
was left standing in the slope case; what the probe now checks is that the coarse
byte spans **0..=127**, which is the property that would actually break
`scaled_plock` — an axis addressing words the box has no value for.

**A knob already at the end-stop does not move, and that defeated two different
mechanisms in turn.** `fx.chorusSend` was skipped in both of the first two runs
and took a third.

In run two its lane was sitting at coarse 0 from the previous session, so "turn
it fully left" changed nothing, the pool diff was empty, and the probe reported
the trig as unheld and moved on — never reaching the maximum prompt. The fix was
to read the expected id's lock **where it is** rather than infer it from what
changed, keeping the diff for what it is good for: naming a lane that moved when
the expected one did not, which is how a bad label join announces itself.

**That fix alone would not have closed it**, which is the part worth keeping.
Run three started from a cleared pattern, and a cleared pattern has no `0x2d`
lane at all — so turning a knob that is already at OFF generates no encoder
movement, the box allocates nothing, and there is no lock to read where it is.
Two different failures, one cause, and only one of them was in this repo. The
working procedure is to **nudge the knob off the end-stop first** so the box
allocates the lane, then turn to each end; it is written into the entry for
`fx.chorusSend` in `A4_PARAMS`, next to the measurement it cost.

`OFF` and `L64`/`CEN`/`R63` also had to be accepted: the probe assumed every
parameter's screen is a parseable decimal, which is why the three sends and pan
dropped out of the first run. That assumption was never checked before it was
shipped to a person with a box in front of them.

#### Closed: all thirteen, in four runs

**Every `A4_PARAMS` entry has a measured scaling**, so the A4's p-lock picker is
now exactly as full as a digi's and `writable_params_for` returns thirteen where
it returned nothing that morning. The last four:

| parameter | id | screen | coarse |
|---|---|---|---|
| `fx.chorusSend` | `0x2d` | OFF..127 | 0..127 |
| `osc2.level` | `0x07` | 0..127 | 0..127 |
| `amp.volume` | `0x31` | 0..127 | 0..127 |

**Thirteen for thirteen on the id, too.** Every parameter's lane was the one the
label join between `A4_PARAMS` and `A4_SYNTH_PLOCKS` predicted — a join made by
eye, which the probe reports against on every line precisely because matching
"FLTR1 FREQ" to "FLTR1 FRQ" is not a measurement.

**A fourth probe bug, and the worst kind.** `TARGETS` was built from eleven
parameters when `A4_PARAMS` holds thirteen: `osc2.level` and `amp.volume` were
dropped in the writing and never prompted for, so three consecutive runs reported
themselves complete over a table they were not covering. A list that is a subset
of the table it measures cannot say so — the run looks clean, and the gap only
surfaces when somebody counts. The probe now asserts `TARGETS` covers
`A4_PARAMS` before it opens a port. That is DEVELOPMENT.md lesson 11's shape
again: **the absence was never reported, so it was never suspected.**

What is left, and it is not blocking anything:

- The other 81 named ids have no `Param` entry to hang a scaling on and stay
  named-and-read-only. Giving one an entry means a `Param` — label, short name,
  bipolar flag, CC/NRPN — and only then a probe run.
- The FX and CV tracks are untouched by all of this: read-only, byte-exact, by
  the decision recorded above.

### The A4 loads presets, and struct version 5 is why it looked like it could not — 2026-09-01, A4 0195

The last thing the digis could do that this box could not, and the last entry in
the sequence of things "the A4 cannot do" that turned out to be things nobody had
asked it. `ui::presets` shipped a whole paragraph explaining the refusal — "no
message that puts a sound on one of its tracks … no cable or OS build changes
that" — and it was wrong the same way the two before it were: right about the
*message* it named, wrong about the feature.

**A digi addresses one kit track's sound; this box only addresses a kit.**
`0x6b`/`0x5b` is a gen-2 pair and the A4's `0x6b` is its working pattern. What it
has instead was already committed and unmined: a kit dump carries its four synth
sounds as 350-byte `0xBEEFBABA` containers at a fixed stride (§10, "The A4 reads
its patch names"), and a +Drive preset file holds the same container. So a load
is a **read-modify-write**: fetch the working kit with `0x68`, replace 350 of its
2,410 bytes, send it back as `0x58`.

Three findings on the way, and the middle one is the reason this took a day
rather than an hour.

**1. The box stores a `0x58` working kit.** `examples/a4_kit_store_probe`, in
three stages of increasing consequence. A kit sent back unchanged came back
byte-identical; a sound copied from SYN1 into SYN4 landed **byte for byte** with
the other three sounds, the kit's name and the FX and CV tracks' 978 bytes
untouched. The reply opcode doubling as the store opcode was a prediction from
gen-2 arithmetic; it holds here.

The probe was written only after `a4_kit::build_working_kit` was shown to
reproduce the box's own `0x58` message **byte for byte** against the committed
capture. That test is what made sending anything defensible: DEVELOPMENT.md
lesson 13 is that a body this box cannot parse takes its whole SysEx API down
until it is power-cycled.

**2. A +Drive preset is struct version 5, a kit slot takes version 6 — and the
box does not refuse the mismatch.** It stores the kit and replaces that track
with an **init sound named `SOUND 4`**. First run of the full load reported
exactly that: `SYN4 reads "SOUND 4" and not "THE SAW"`.

That is the worst failure mode available to this feature, and it is worth being
precise about why: had the verify compared anything weaker than the *name* — a
"the store went out" check, a byte count — the load would have reported success
with the user's sound thrown away. Two readings fitted the evidence and they were
opposites, so `examples/a4_sound_convert_probe` asked the one question that
separates them: **load two different presets onto the same slot and compare what
the box ends up holding.** A conversion leaves two different slots; a refusal
leaves the same init sound twice. It was 0 of 350 bytes different — a refusal.

**3. The conversion is two bytes, and the box supplied the evidence for it.** The
A4's project sound pool (`0x63` → `0x53`, 350 B, 128 slots) holds **version-6**
sounds, and on this box 28 of them share a name with a +Drive file — the same
sound in both versions, which is the pair needed to see what the conversion is
without guessing (`examples/a4_sound_pool_probe`). Across all 28:

| | version 5 (file) | version 6 (pool) |
|---|---|---|
| version word at `+4` | 5 | 6 |
| byte `+235` | **184**, every time | **0**, every time |
| everything else | the sound's own | the sound's own |

**And one pair differs in nothing else at all** — `DUAL OSCS`, where the box's
own version-6 sound is byte-for-byte the file with those two fields changed. That
is what makes this a conversion rather than a correlation: a byte belonging to a
universal rule would have to differ in *that* pair too, and none does. The bytes
that differ in the other 27 differ because the pool copy was edited — `+126`
moves in both directions across the set, which no version rule does.

`a4_kit::sound_for_kit` is those two bytes, and the pool probe checks it the only
way it can be checked: on the unedited pair it must reproduce the box's own
version-6 bytes exactly. It does.

**Then the load ran.** `THE SAW` off `/soundbanks/A/1` onto SYN4 of a live
POLYTRON kit, read back twice, then reverted byte for byte from the backup the
load handed back. The stored kit still held `BRE` throughout, which is the
recovery story this panel has always told — reloading the pattern discards an
unsaved kit — witnessed from the protocol side for the first time here, and
`--restore` puts one slot back from the stored kit without walking to the box.

What shipped with it:

- `protocol::a4_kit`: `sound_slot`, `splice_sound`, `sound_for_kit`,
  `build_working_kit`. The splice touches exactly its slot's stride and refuses
  anything that is not 350 bytes with the head magic at one end and the foot
  magic at the other — plus, since finding 2, anything that is not version 6.
- `protocol::drive::a4_preset_sound`: the **other cut** of a preset file. The
  digis' `preset_load_payload` slices the file's whole declared payload because
  on those boxes that payload *is* a `0x6b` reply; here the declaration says 366
  and a kit slot takes 350, so the cut is the stride and the **foot magic** is
  the witness that it came from the right place (`sound::A4_SOUND_MAGIC_FOOT`,
  recorded as "no foot at all" for two days).
- `midi::a4_preset_load`: the flow — four round trips, the echo defence over
  `0x68`/`0x58` that `preset_load` documents for `0x6b`/`0x5b`, and the same
  `LoadReport` the gen-2 path returns.
- `core::device::PresetLoad`: the route as a table row. `ui::presets` keyed this
  off `pattern_route()` and had to be told twice that a load route is its own
  axis; the field also carries the **slot count**, which is four here against a
  digi's sixteen, because the FX and CV tracks have no sound to put a preset on.

**Not verified:** nobody has *heard* one. The parameters that land are the file's
own bytes and the box accepts them as version 6 — and the unedited pair says
those bytes are what the box itself would have written — but that is an argument
about bytes, not an ear on a speaker. Also unverified on this route: the
OS-build gate speaking through it, and a load onto a box whose pattern is playing
at the time.

### Chords reach the A4, and a Chord lead role — 2026-09-02

**The ask** was a new generator role, "specific to the A4", that captures the
sound of the box's factory pattern A01 SYN1 — which, Neil pointed out, plays
chords through the ARP page's NO2/NO3/NO4 on a box whose tracks hold one note
per step. He expected two tranches: three new p-lock lanes in the roll, then the
role. The first tranche came out differently, and smaller.

**What A01 SYN1 actually is**, decoded off the fixture already on disk. Thirty-two
trigs on every odd step, velocity 127 throughout, no conditions, no micro timing,
lengths of 1.75–1.81 steps under a two-step gap. Only NO2 and NO3 are used, never
NO4. Under `0x40` as zero semitones the offsets resolve, bar by bar, to **two
pedal tones at fixed pitches while the root leaps octaves under them**: E6 and C7
over A4, A5, A6, A4, A5, A6; E6 and B6 over G6, G4, G5, …; E6 and C7 then A6 and
D6 over D; G6 and C7 over F; A6 and B6 over G. Common tones carry across the bar
line, and every third trig — where the root reaches the pedals' own register —
the voicing thins to one pedal. The harmony is A minor, i VII IV VI VII. **No
other centre than `0x40` makes 57 offsets reconstruct to the same pitches from
roots an octave apart**, which is how the unit was fixed without a screen; the
menu's range, -64..+63, Neil read off the box. Two facts from him the same day,
both required: **the chord needs a polyphonic kit** (a mono kit plays the root
alone — the kit is called POLYTRON for a reason), and it sounds with the arp MOD
off.

**Tranche 1 was not three lanes.** This app already models a chord as up to four
notes on one step — `MAX_CHORD_NOTES` is 4 because both digis hold four notes per
trig — and root plus NO2, NO3, NO4 is the same four. So `core::a4_transfer` now
carries a same-step chord as the lowest note plus ascending offsets, and brings
NO2–NO4 in as the upper notes of a chord on import. Fetching A01 draws 89 notes on
SYN1 where it drew 32; the "only the lowest goes" warning is gone, replaced by
two honest ones (a fifth distinct note, a note more than 63 semitones above the
root). The write puts all three lanes on every authored step, `FF` for OFF, by
the condition lane's rule: a lane left alone would keep the destination's offset
and sound a chord nobody drew. Cleared steps keep the box's bytes, as before. The
roll, chord draw, the Harmony panel and the existing Chords role all reach the A4
through this with no new UI. What the round trip does not preserve is the box's
own NO2/NO3 *order* on the two A01 steps that store the higher offset first;
ascending sounds the same with the arp off.

**Tranche 2 is `parts::chord_lead`**, a transcription rather than a port:
straight eighths, flat velocity, a length that breathes a quarter step before
the next trig, two pedals per bar chosen from the chord's tones and its ninth,
sixth and seventh, voice-led by least movement from the bar before, the root
cycling three octaves and occasionally leaping a fourth, the voicing thinning at
the top. One grammar in every genre — it is one pattern's idiom, not a genre's —
borrowing only the genre's chord p-lock lanes. Named **Chord lead**, Neil's
choice, and **not gated to the A4**: the notes are ordinary same-step chords, a
DN2 plays them natively, and Neil's correction stands that a DT2's *sample*
tracks are monophonic (its NO2–NO4 are a MIDI-track feature) — which is a
question for `core::export`'s treatment of chords on a DT2, not for this role.
A01's progression joined the library as `i VII IV VI`.

**Heard, the same day.** Neil ran both tests on the box: A01 fetched, written to
a spare slot on the POLYTRON kit and played — the chords are the chords — and
then a generated Chord lead row sent the same way, which he called better than
expected. Still open: the kit's poly config lives in the unmapped 978-byte tail,
so the app cannot yet say whether a destination kit will sound the chord or the
root alone — the import line and the role's docs say so in words instead.

### 10.5 What saving a kit will cost, when it comes

Recorded now so v2 starts from evidence:

- **Multi-chunk +Drive writes are broken.** Six chunks at 2,048 and two at 8,192
  both failed with `Invalid package checksum; corrupt transfer`, under every
  checksum variant tried. Chunk *count* is the failing variable, not size. One
  16,384-byte chunk committed and read back byte-exact.

  > **"Every checksum variant tried" did not include the right one.** The 2026-08-30
  > capture shows the box's checksum is a zero-seeded CRC32 **inverted**, which
  > is neither the plain zero-seeded value the document names nor a standard
  > `crc32`. Whether that alone explains the multi-chunk failures is untested —
  > the offset field's meaning past chunk one is still unknown — but the
  > checksum can no longer be treated as ruled out.
- A DN2 kit is **10,795 bytes** and fits that single chunk. A DT2 kit is roughly
  17.8 KB and does not. So DN2 kit saving is reachable on today's knowledge and
  DT2 kit saving is blocked on a reverse-engineering problem with no timeline —
  settling it wants a capture of Transfer writing a multi-chunk file.
- ~~`assert_read_only_file_op` will have to admit `0x57`/`0x58`/`0x59`.~~ **Done
  differently, 2026-08-29:** it was not widened. `assert_write_file_op` is a
  **second, disjoint** allowlist admitting exactly WriteOpen, Write and
  WriteClose, and `drive_write_request` is the only path to it. `0x5A` Move,
  `0x5B` Copy and `0x5C` **Delete** are unreachable from anywhere in the
  workspace. Two allowlists rather than one wider one means a read path cannot
  acquire a write by editing one line.
- `0x59` WriteClose is the commit. Nothing lands without it, which makes an
  abandoned write harmless and is worth relying on.
- Verify-after-write earns its keep here more than anywhere: a chunking bug that
  silently truncates is exactly this API's failure mode. A stored file stamps its
  own **bank and slot** at file bytes `+23` and `+24`, so a correct copy into a
  different slot legitimately differs in those two bytes and a naive comparison
  will cry wolf — measured three times on 2026-08-30, and
  `drive::differs_only_by_location` is the check that reads it right.

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
   see §9 and §10.4. The load path is `0x5b` on both digis, wrapped payload.
   ~~The A4 has no `0x6b` to mirror so it has no load path at all.~~ **It has
   one, 2026-09-01**, by the other route: `0x68` fetches its working kit, one
   sound is spliced in and `0x58` sends it back. See "The A4 loads presets, and
   struct version 5 is why it looked like it could not" above.
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
   just the `0x5b` under it. ~~The A4 does not load, as designed.~~ **All three
   boxes load, 2026-09-01**: the A4 by the other route — `0x68` fetch, splice,
   `0x58` — through the same panel and the same double-click, verified on 0195.
   §9 has what the digi run covered and the four refusals it did not; §10's "The
   A4 loads presets" has the A4's, including the init-sound trap that made the
   first attempt look like a refusal.

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
