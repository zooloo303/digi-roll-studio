# MIDI file import — design for implementation

Status: design, agreed with Neil 2026-09-05. Nothing below is built. The only
import that exists is `core::midifile::midi_file_to_notes`, which reads the
first note-bearing track of a file into the selected track and cannot offset
it — the open item in `PLAN.md` §1 ("MIDI import reads only the first
note-bearing track"). This document replaces that item with a full design and
a build order. Read `PLAN.md` §2 (the model) and §7 (rules carried over) first;
this document assumes both.

## 0. The problem in one paragraph

A Standard MIDI File is a linear timeline of (track, channel) parts, of any
length, in any meter. The Elektron world is a grid: a slot holds at most 128
steps (64 on the A4), a step holds one trig, a scene holds one slot per box,
and a song holds at most 99 rows. So an import is not a codec. It is three
jobs that the current function collapses into one:

1. **Analyse** the file into parts and facts about them. Pure, no decisions.
2. **Fit** the timeline onto the grid: slice into pattern-sized segments,
   dedupe repeats, allocate slots, produce scenes and song rows. Pure; every
   constraint lives here.
3. **Map** parts onto destinations. The user decides; this is the only stage
   with a dialog.

Then one commit, one undo entry, session only. **No hardware is touched by an
import.** Getting the result onto boxes stays with the existing sync path and
the open item about syncing a song's slots to a box.

## 1. Vocabulary

| word | meaning |
|---|---|
| **part** | one (MTrk index, MIDI channel) pair with at least one note. The unit of mapping. |
| **score** | the parsed file: parts, tempo map, time-signature map, markers. |
| **segment** | a bar-aligned slice of the timeline that becomes one pattern per destination box, one scene, one song row. |
| **plan** | the fully resolved result of fitting a score under a mapping: patterns, scenes, rows, report. Pure data, applied in one step. |
| **destination** | `(DeviceId, track index)` on the desk. |

## 2. Facts about the codebase the implementer must use

- `core::model::Note { step: f64, pitch, len: f64, velocity, micro: f64, prob, fill, cond }`. Step is fractional-capable but the box grid is whole steps plus micro. `edit_ops::clamp_micro` bounds micro to ±0.49; `edit_ops::clamp_velocity`; `lengths::snap_len_fine` snaps a length to what a box can store; `lengths::LEN_MIN = 0.125`.
- `edit_ops::BAR_STEPS = 16` and `MAX_STEPS = 128` are 4/4 assumptions. The import must **not** use `BAR_STEPS` for a bar; it computes steps per bar from the file's time signature (§3.3). `MAX_STEPS` is only a global ceiling; the real ceiling is `DeviceModel::max_steps` per destination box.
- `Track { length_steps, scale: TrackScale, notes, plocks: Vec<PLockLane>, channel, .. }`. `Pattern { name, swing, source: Option<Source>, tracks (private; `tracks()`, `track_mut(i)`) }`. `Device.patterns: Vec<Arc<Pattern>>`, addressed by `session::PatternRef { bank, index }` with `slot()`/`from_slot()`/`label()`.
- `Session::add_scene(name, like)` copies slots from `like` (pass `Some(current_scene)` so boxes not in the import keep their slot rather than snapping to A01). `Session::set_slot_in_scene`, `Session::add_song_row(scene)`, `Song::row_mut(i)` to set `label`/`repeats`. `song::MAX_ROWS = 99`.
- Undo: `core::history::History` — `begin(Content::of(session))` … `commit(session)`. `ui::generate::apply_plan` is the precedent for "build a pure plan, then write it onto the session as ordinary state that history covers as one step". Follow it exactly.
- The Edit panel import lives in `crates/app/src/ui/edit.rs` around `midi_file_to_notes` (replace notes, `length_steps`, clear `plocks`, clear `pattern.source`, clear roll selection, set `Status::Imported`). The destructive-note `const` there has a third sentence about the first-track limit, pinned by the test `the_destructive_note_admits_the_first_track_limit`. Both change when Phase B lands.
- `digi_protocol::params::{param_table_for, MidiMap { cc, cc_lsb, nrpn }}` is the curated per-box CC table; used for the deferred CC→p-lock work (§7).
- `DeviceModel` is a data table (`core::device`): "track count comes from `DeviceModel`, never from a constant". New per-box facts go there as fields (§6), not as `match` arms.
- The export writes 96 TPQN, 24 ticks per 16th (`track_to_midi_file`). Tests can build fixtures by exporting and by hand-assembling type-0 bytes; no hardware, ever.

## 3. Stage 1 — analyse: `core::midifile::score`

Add a full parser beside the existing one, in `midifile.rs` or a new
`midifile/score.rs`. **`midi_file_to_notes` stays, reimplemented as a thin
wrapper**: score the file, take the first part, fit it as a single segment of
`max_steps`, return the same `Imported`. Every existing test must stay green
unchanged.

### 3.1 Types

```rust
pub struct Score {
    pub division: u16,                 // ticks per quarter (SMPTE already refused)
    pub tempo: Vec<(u64 /*tick*/, f64 /*bpm*/)>,
    pub meters: Vec<(u64, Meter)>,     // time signature changes; at least one, tick 0, default 4/4
    pub markers: Vec<(u64, String)>,   // meta 0x06
    pub parts: Vec<Part>,
    pub end_tick: u64,                 // last note-off or end-of-track, whichever is later
}

pub struct Meter { pub num: u8, pub den: u8 }   // den is the actual denominator (4, 8, 16), not the log2 byte

pub struct Part {
    pub mtrk: usize,
    pub channel: u8,                   // 0-15
    pub name: String,                  // track name (0x03) or instrument name (0x04) meta, else "Track {n} ch {c+1}"
    pub program: Option<u8>,
    pub notes: Vec<RawNote>,           // ticks, sorted by on
    pub stats: PartStats,
    pub cc: BTreeMap<u8, usize>,       // cc number -> event count (for §7; collect now, cheap)
    pub sustain_events: usize,         // CC64 count
    pub pitch_bend_events: usize,
}

pub struct RawNote { pub on: u64, pub off: u64, pub pitch: u8, pub velocity: u8 }

pub struct PartStats {
    pub notes: usize,
    pub pitch_lo: u8, pub pitch_hi: u8,
    pub first_bar: u32, pub last_bar: u32,
    pub max_simultaneous: u8,          // most notes sharing one quantised step
    pub off_grid_ratio: f32,           // share of notes whose on-tick is not within ±1/48 step of a 16th
    pub looks_like_drums: bool,        // channel 10, or program in a GM percussion bank
    pub looks_like_triplets: bool,     // off_grid_ratio > 0.5 and the off-grid notes sit near thirds of a step
}
```

### 3.2 Parsing rules

- Header: type 0 and type 1 accepted. **Type 2 is refused** (`MidiFileError::IndependentSequences`): its tracks share no timeline. SMPTE division refused as today. Zero notes across all parts is `Ok(Score)` with empty `parts`; the callers decide the message.
- Split by (MTrk, channel). Type-0 files carry a whole arrangement in one MTrk split by channel; DAWs export type-1 tracks that hold several channels. Splitting on both is the only honest unit. Merging is a mapping choice (§5), never a parse default.
- Note pairing: keep the existing `open` list semantics (in-place replace on a repeated note-on, never-released notes get one 16th of length). Note-on with velocity 0 is a note-off.
- Track-name meta belongs to the MTrk; every part split out of that MTrk inherits it, with " ch N" appended when the MTrk yields more than one part.
- Tempo (meta 0x51): record every change. **Only the first is ever offered** to the session (§5.5); the rest are counted into the report as `tempo_changes_dropped`. The engine has one clock (`PLAN.md` §2, ROW TEMPO argument).
- Meter (meta 0x58): record every change with its tick. Default 4/4 at tick 0 if absent.
- Markers (meta 0x06), cue points (0x07) as markers too. Text trimmed; empty dropped.
- Running status, SysEx (`F0`/`F7`) skipped, unknown meta skipped, all as today. `Reader` stays; the `vlen` warning stays.

### 3.3 Grid arithmetic (shared by stages 1 and 2)

- `per16 = division / 4` ticks per step (f64).
- `steps_per_bar(meter) = meter.num * 16 / meter.den`. Integer for den ∈ {1,2,4,8,16}. For den = 32 it can be fractional: round **up** and set `report.meter_approximated`. 3/4 → 12, 6/8 → 12, 7/8 → 14, 5/4 → 20, 4/4 → 16.
- Bar boundaries come from walking the meter map; write one function `bar_starts(score) -> Vec<(u64 tick, Meter)>` and use it everywhere. A meter change always starts a new bar.
- Tick → step: `f = (tick - origin_tick) / per16`; `step = js_round(f)` (keep `js_round`, it is the export oracle's rounding); `micro = clamp_micro(f - step)`. Length: `js_round((off - on)/per16).max(1.0)` as today, then `snap_len_fine` against room left in the track.

### 3.4 Tests

Hand-built byte fixtures in the test module (no files needed): type-0 with two channels → two parts; type-1 with a multi-channel track → parts named "… ch 1"/"… ch 10"; 3/4 and 6/8 meters → steps per bar 12; meter change mid-file → bar starts; markers collected; type-2 refused; truncated refused; tempo list; `looks_like_drums` on channel 10; `max_simultaneous` on a chord; `off_grid_ratio` on a swung file exported by `track_to_midi_file` after adding micro-timing. Plus: the wrapper `midi_file_to_notes` returns byte-identical `Imported` to the old implementation on every existing test input.

## 4. Stage 2 — fit: new module `core::midi_import`

Pure functions from `(Score, Mapping, Options, &Session)` to `ImportPlan`.
Nothing here writes to a session; `apply` (§5.6) does.

### 4.1 Inputs

```rust
pub struct Options {
    pub pattern_bars: u8,              // 1, 2, 4, 8 — default §4.2
    pub trim_leading_silence: bool,    // default true
    pub start_slot: BTreeMap<DeviceId, PatternRef>,  // per destination box; default: first blank slot
    pub overwrite_occupied: bool,      // default false
    pub apply_tempo: bool,             // default: true only when no track in the session has a note
    pub label_rows_from_markers: bool, // default true
}

pub struct Mapping {
    pub parts: Vec<PartMapping>,       // one per score.parts, same order
}

pub enum PartMapping {
    Skip,
    Track { device: DeviceId, track: usize, overflow: Overflow, scale: TrackScale },
    /// Drum part fanned out: one destination track per pitch (§4.5).
    Drums { device: DeviceId, tracks: Vec<(u8 /*pitch*/, usize /*track*/)> },
}

pub enum Overflow { KeepLowest, KeepHighest, SplitTo { device: DeviceId, track: usize } }
```

### 4.2 Segments

- Compute `origin_tick`: tick 0, or when `trim_leading_silence`, the start of the bar containing the first note of any **mapped** part.
- Default `pattern_bars`: the largest of 8, 4, 2, 1 such that `pattern_bars × max(steps_per_bar over the meters in use) ≤ min(max_steps over destination boxes)`. A DT2 alone in 4/4 defaults to 8 bars; a desk with an A4 defaults to 4; 3/4 on a digi still gets 8 (96 steps). The dialog shows the value and lets the user lower it.
- Walk `bar_starts` from `origin_tick`. Cut a segment every `pattern_bars` bars **and** at every meter change. Stop after the bar containing `end_tick`.
- Per segment, per destination box: build a fresh `Pattern` (from the device's blank-pattern constructor, name `"{file stem} {n}"`, swing 50). Every track in it — mapped or not — gets `length_steps = segment_steps`. `Session::scene_boundary_steps` takes the **longest** track across every box in the scene, so with every track at the segment length one segment is exactly one scene cycle, and a shorter blank track could not shorten it anyway. `scheduler::scene_cycle_seconds` is the time-domain version and is what a `ThreeHalves` track (§4.7) has to agree with: check that a scaled track of `segment_steps × 1.5` steps yields the same cycle seconds before offering the suggestion.
- Notes: a note whose `on` falls in the segment is placed there with `step` relative to the segment start. A note whose `off` crosses the segment end is **clamped** at it, as the current import clamps at track end, and counted (`report.clamped_at_boundary`). Notes are never carried into the next segment.
- Last segment: `length_steps` is rounded up to whole bars of actual content (at least one bar), not padded to `pattern_bars`, so a 90-bar song does not end with six bars of silence.

### 4.3 Dedupe and repeats — the part that makes this usable

A 90-bar song is 12 eight-bar segments naively. Most are identical. Hash and
reuse:

- For each `(segment, box)` build a **canonical form**: every track's notes sorted by `(step, pitch)`, each as `(step as u16, (micro × 384) as i16, pitch, steps_to_length_byte(len), velocity)`, plus `length_steps` per track. Ids excluded (see `ui::generate::music_of` for the same trick). Compare by equality in a `BTreeMap<Canonical, PatternRef>`; hashing is only an index.
- Identical `(segment, box)` forms share one slot. Slots are allocated in first-appearance order from `start_slot`.
- A scene is the tuple of slots across destination boxes. Identical **consecutive** scenes collapse into one song row with `repeats` incremented. Identical non-consecutive scenes reuse the scene (do not create a second scene pointing at the same slots).
- Segments that are entirely empty for every box still occupy a row (silence is music) but share one blank pattern per box.

Expected outcome, and a test to pin it: an 8-bar loop repeated four times produces one pattern per box, one scene, one row with `repeats = 4`.

### 4.4 Slot allocation

- A slot is **blank** when `pattern.source.is_none()` and every track has no notes and no p-lock lanes. Add `Pattern::is_blank()`.
- Fill forward from `start_slot` taking blank slots only. With `overwrite_occupied` the walk takes every slot from `start_slot` in order and the report lists what it replaces (pattern name, note count), the way `generate.rs`'s `row_summary` reports "replacing N notes".
- Running out of slots, or out of song rows (`MAX_ROWS`), or of tracks for a drum fan-out, **refuses the whole plan before anything is placed** — `PlanError::{NoFreeSlots{device, needed, free}, TooManyRows{needed}, NotEnoughTracks{device, needed, free}}`. Partial imports are not offered.

### 4.5 Drums fan out

A GM drum part is one channel with many pitches; on a DT2 each drum is its
own track. So `PartMapping::Drums` places every hit of pitch *p* on the track
mapped for *p*:

- Default fan-out (built by the dialog, editable there): rank the part's pitches by hit count, take the first `free tracks on that box` pitches, name each destination track from the GM percussion map (36 Kick, 38 Snare, 42 Closed Hat, 46 Open Hat, 39 Clap, 37 Rimshot, 41/43/45/47/48/50 Toms, 49/57 Crash, 51/59 Ride, 56 Cowbell, 54 Tambourine, 69/70 Shaker, 75 Clave; anything else "Perc {p}"). Unranked pitches are dropped and counted (`report.drum_pitches_dropped`).
- On a **sample track** the incoming pitch would transpose the sample, so every placed note takes the trigger pitch the generator already uses for drum voices: `digi_generator::parts::drums::DRUM_TRIGGER_PITCH` (60, with the reasoning in that file's header). Move the constant into `core` so both crates read one number rather than agreeing by coincidence. On a DN2 or A4 the pitch is kept, because their drum sounds are pitched by design.
- Polyphony is irrelevant on a fanned-out drum part: one pitch per track. Two hits of the same pitch on the same step (flams) keep the later one, counted.

### 4.6 Polyphony

One step holds one trig, and a trig **stores** at most four notes on every
box this app knows: `protocol::pattern`'s `spec.trig.max_notes` is 4 for the
DT2 and the DN2, and the A4 carries a root plus NO2–NO4 (hardware-verified
2026-09-02). Expose that as `DeviceModel::notes_per_trig` (§6) and read the
cap from there, never as a literal. What a DT2 sample track *sounds* when
handed a four-note trig is a hardware question the import does not need
answered: the storage cap is the constraint. Per placed part, after
quantising, group notes by step. If a step holds more than the cap:

- `KeepLowest` / `KeepHighest` drop the rest, counted (`report.notes_over_polyphony`).
- `SplitTo` moves notes above the cap (lowest-first stay) onto a second destination track in the same pattern. That track is then ordinary: it gets the same length, and it is subject to the same cap (overflow beyond two tracks is dropped and counted).
- `Note.prob/fill/cond` stay `None`. Notes sharing a step already have to agree on those (`edit_ops::adopt_step_trig`); with all `None` they do.

### 4.7 Timing

- Off-grid notes become micro-timing, as today. Nothing is quantised flat.
- `PartMapping::Track.scale` defaults to `One`. When `stats.looks_like_triplets`, the dialog **suggests** `ThreeHalves`, at which a 16th-triplet is exactly one step, and the fit then uses `per16 × 2/3` as the tick-per-step for that part. It is a suggestion because the same ratio can be a loose human take. Note the consequence for the segment: a 3/2 track wraps at `segment_steps × 1.5` steps to cover the same time, and `max_steps` is checked against that; if it does not fit, the suggestion is not offered.
- Velocity via `clamp_velocity`. Length via `snap_len_fine(len, room_left)`.

### 4.8 Output

```rust
pub struct ImportPlan {
    pub patterns: Vec<(DeviceId, PatternRef, Pattern)>,   // unique patterns to write
    pub scenes: Vec<PlannedScene>,                        // name, slots per device
    pub rows: Vec<(usize /*scene idx into `scenes`*/, String /*label*/, u16 /*repeats*/)>,
    pub tempo_bpm: Option<f64>,                           // offered, applied only if Options::apply_tempo
    pub report: ImportReport,
}

pub struct ImportReport {
    pub parts_mapped: usize, pub parts_skipped: usize,
    pub notes_placed: usize,
    pub segments: usize, pub unique_patterns: BTreeMap<DeviceId, usize>, pub rows: usize,
    pub clamped_at_boundary: usize,
    pub notes_over_polyphony: usize,
    pub drum_pitches_dropped: usize,
    pub same_step_duplicates: usize,
    pub tempo_changes_dropped: usize,
    pub pitch_bend_dropped: usize,
    pub cc_dropped: usize,                // until §7 lands, every CC event
    pub meter_approximated: bool,
    pub replacing: Vec<(DeviceId, PatternRef, String /*old name*/, usize /*old notes*/)>,
}
```

Row labels (`label_rows_from_markers`): the marker whose tick equals the
segment's start, else the previous row's label, else `"Row {n}"`. Collapsed
repeats keep the first label. Free `String`, so any marker text is legal.

### 4.9 Tests (all pure, no UI)

- Segmenting: 4/4 90 bars at 8 → 12 segments, last is 2 bars long; 3/4 at 8 → 96-step patterns; meter change at bar 5 cuts a segment there.
- Dedupe: loop × 4 → 1 pattern, 1 row, `repeats = 4`; ABAB → 2 patterns, 2 scenes, 4 rows; non-consecutive reuse does not add scenes.
- Boundary clamp counted; last segment trimmed to content.
- Slot allocation skips occupied, refuses when short, overwrite lists what it replaces.
- Drum fan-out: ranking, naming, root-note substitution on a DT2, pitch kept on a DN2, cap by free tracks.
- Polyphony: each `Overflow` policy; `SplitTo` obeys the cap on the second track.
- Triplet scale: 16th-triplets land on whole steps at `ThreeHalves`.
- Wrapper parity: `midi_file_to_notes` ≡ old behaviour (see §3.4).

## 5. Stage 3 — map, dialog, apply

### 5.1 Two gestures, one engine

- **Into this track** — the existing Edit panel button, upgraded. Score the file; if it has exactly one part and fits `max_steps`, behave exactly as today (one click, no dialog). Otherwise open a small chooser: part (dropdown, showing name/channel/notes/range), start bar, bar count (default all, capped to `max_steps / steps_per_bar`), scale suggestion when triplets are detected. The result is `fit` with one part, one destination, one segment of the chosen bars starting at the chosen bar. This closes the `PLAN.md` §1 item on its own.
- **As a song** — new, entered from the SONG panel ("IMPORT MIDI FILE…") and the SCENES panel. The full flow below.

Both share §3 and §4. The Edit panel keeps its replace semantics: notes, `length_steps`, clear `plocks`, clear `source`, clear roll selection.

### 5.2 The song dialog

An egui modal following the v2 panel rules (title bar with context, `?` reference, amber destructive note where a plan overwrites occupied slots).

Top: file name, tempo (with the "set session tempo" checkbox), meter(s), bars, markers count.

Middle: one row per part. Columns: name, ch, notes, range, poly (max simultaneous), grid (off-grid %, with a "3/2?" chip when triplets are detected), CC (count, greyed until §7), destination, policy. Destination is a dropdown of `Skip`, every `(box, track)` labelled as the TRACKS grid labels them (box name, `T{n}`, track name), and for drum-like parts `Drums → {box}` which expands an inline sub-table of pitch → track with the GM name. Policy shows only when poly exceeds the destination's `notes_per_trig`.

Options row: pattern length (bars, from the allowed set), start slot per destination box (slot picker as the Setup panel's), trim leading silence, overwrite occupied.

Bottom: the live plan summary, recomputed on every change (`fit` is pure and fast): "N unique patterns on DT2, M on DN2 · S scenes · R rows · P notes placed · X clamped at boundaries · Y dropped for polyphony · Z CC events not imported". Then IMPORT / CANCEL. IMPORT is disabled while the plan errors (`PlanError` shown in place of the summary).

### 5.3 Auto-mapping defaults

Applied when the dialog opens; everything is editable.

1. Drum-like parts → the first box whose default track kind is sample-based (the DT2), as `Drums`, fan-out per §4.5.
2. Name match: a part name that contains a destination track's name (case-insensitive, either direction; "Kick" ↔ "BD 909" needs a tiny synonym list: kick/bd/bass drum, snare/sd, hat/hh, clap/cp, bass, lead, pad, chord, keys/piano) takes that track if free. Track names on the desk come from fetched kits, so this catches the common case for free.
3. Polyphonic parts (max simultaneous > 1) → a box with `notes_per_trig > 1`, first free track.
4. Monophonic parts with `pitch_hi < 55` → first free track on a synth box (bass).
5. Everything else → first free track anywhere; when none, `Skip`.

"Free" means not already taken by an earlier rule in this pass. Existing notes on a destination track do not block mapping, because the plan writes into **fresh** patterns in fresh slots, never into the current pattern.

### 5.4 What the dialog never does

Merge two parts onto one track silently (the user can map two parts to the same destination; the summary then says "2 parts merged on DN2 T3" and the fit unions the notes before the polyphony pass). Clamp pitch. Invent `prob`/`fill`/`cond`. Change tempo without the checkbox. Touch a box.

### 5.5 Tempo

`Score.tempo[0]` is offered. Default on only when no track anywhere in the session has a note; otherwise off. Applied via the same path the transport uses for a tempo edit, inside the same history step.

### 5.6 Apply

```rust
pub fn apply_import(session: &mut Session, plan: ImportPlan) -> AppliedImport
```

One history step, `generate.rs` style: caller does `history.begin(Content::of(session))`, then `apply_import`, then `history.commit`. Order inside:

1. Write every `plan.patterns` into its slot (`device_mut(id).pattern_mut(slot)`), replacing the `Arc<Pattern>` whole.
2. Create scenes with `add_scene(name, Some(session.current_scene))` then `set_slot_in_scene` for each destination box. Non-destination boxes keep their current slot.
3. Append rows with `add_song_row(scene)`, then set `label` and `repeats` through `song_mut().row_mut(i)`. `length_steps` stays `None`. Mutes inherit.
4. Apply tempo if `plan.tempo_bpm` and the option is set.
5. Leave `current_scene` where it was. Return what was created (first new scene index, first new row index, counts) so the panel can select the first imported scene and print the report in the console.

If the song has zero rows before the import, the imported rows form the whole song; `Session::song` becomes `Some`.

## 6. `DeviceModel` additions

Add as a field in the table, per the model's own rule:

- `notes_per_trig: u8` — how many notes one trig stores. Derivable today:
  `spec.trig.max_notes` is 4 for both digi specs in `protocol::pattern`, and the
  A4's chord path is root plus three offsets. Set 4 for all three entries and
  add a test that asserts each digi entry equals its spec's `max_notes`, so the
  two tables cannot drift.

No `root_note` field is needed: the sample-track trigger pitch already exists
as `DRUM_TRIGGER_PITCH` (§4.5); relocate it rather than duplicate it.

## 7. Deferred, but designed for

None of these block v1; each hangs off types above.

- **CC → p-lock lanes.** For a part on a destination box, map each CC number through `param_table_for(kind)` by `MidiMap.cc`; unmapped CCs are counted. Build one `PLockLane { name: Some(param.name), device_kind, values }` per matched CC, sampling the last CC value at or before each **trig's** step (locks ride on trigs; values on trig-less steps are dropped and counted — trigless locks are not modelled). `Part.cc` already collects the counts.
- **Sustain pedal.** Before quantising, extend each note's `off` to the next CC64 ≤ 63 after it when CC64 was ≥ 64 at its `off`. Piano exports need this or every chord is a stab.
- **Swing detection.** If the median micro of notes on odd 16ths is consistently positive and the even ones are on-grid, offer the pattern swing byte (`50 + round(ratio)`, capped 80) and zero those micros. Offered, never automatic.
- **Loop compression.** A track whose segment content is an exact repetition of its first *k* bars can take `length_steps = k × steps_per_bar` — polymeter for free. Interacts with scene cycle derivation; leave until §4.2's boundary rule is proven on a screen.
- **Sync the imported song to the boxes.** Already an open item; the plan's `patterns` list is exactly the set of slots that needs writing.

## 8. Build order and acceptance

Each phase is one PR-sized change with its own tests; do not start the next
without the previous one green.

**A — Score.** `Score`/`Part` parser; `midi_file_to_notes` as wrapper. Accept: all existing tests unchanged and green; §3.4 tests added. No UI change.

**B — Into this track, with a chooser.** Edit panel chooser (part, start bar, bars, scale suggestion). Update the destructive-note `const`'s third sentence and its pinning test. Accept: a multi-track DAW file imports the chosen part from the chosen bar; a single-part file still imports in one click; `PLAN.md` §1 item struck through with the date.

**C — Fit.** `core::midi_import` with `Options`, `Mapping`, `fit`, `ImportPlan`, `ImportReport`, `PlanError`, `Pattern::is_blank`, `DeviceModel::notes_per_trig`, and `DRUM_TRIGGER_PITCH` relocated to `core`. Accept: §4.9 tests; no UI.

**D — Song import.** Dialog, auto-mapping, `apply_import`, history, console report, SONG/SCENES entry points. Accept: a real DAW export (Neil to supply one under `crates/core/tests/fixtures/midi/`, committed like the `.syx` captures) imports into scenes and rows, plays from the SONG panel on a DT2+DN2 desk with the engine, undoes in one step, and survives a save/reopen.

**E — §7 items**, in the order listed, each behind its own tests.

## 9. Decisions Neil owns

1. Whether "Into this track" stays a separate Edit-panel chooser (this design) or becomes the same dialog with one row. Design assumes separate: the Edit panel gesture is a two-field question and should stay one.
2. On a mixed desk, whether the default pattern length follows the tightest box (this design: 4 bars when an A4 is present) or lets the digis take 8-bar patterns with the A4 on 4-bar ones and the row cycling twice. The second needs the scene-cycle rule to be settled first; the design takes the first and leaves the second as a later option.

## 10. Rules carried over

From `PLAN.md` §7 and the existing import's own comments, and binding here:
never merge parts silently; never clamp pitch (the roll grows a row);
never invent trig conditions; report what was dropped, in the console and
in the dialog, before the button is pressed; a refusal names the reason and
the number; the session file must round-trip an imported song unchanged; and
nothing in this feature sends a byte to a box.
