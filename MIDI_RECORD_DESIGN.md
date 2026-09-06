# MIDI recording — design for implementation

Status: designed and agreed with Neil 2026-09-05; **phases A–D built and played
on hardware the same day** — a DT2 and an A4, Neil's words "works perfectly".
Phase E is half closed: what has been driven is playing and recording, and the
three checks in §11's "What is left" have not. Read §11 first if you are picking
this up: it is the record of where the built thing differs from the
design below, and two of those differences are ones the design could not have
compiled as written.

Before this, the app opened a MIDI input for exactly two things — the identity
handshake (`midi::device::ElektronDevice`) and SysEx dumps
(`midi::ports::SysExInbox`) — and nothing anywhere listened for a note. This
document adds the third: play a connected keyboard and have the notes land in
the selected track of the piano roll, in time, while the pattern loops. Read
`PLAN.md` §2 (the model) and §4 (the engine) first; this document assumes both,
and it follows `MIDI_IMPORT_DESIGN.md` in shape because that shape worked.

## 0. The problem in one paragraph

A keyboard produces note-on and note-off at wall-clock moments. The model wants
a `Note` at a whole step of one track's grid, with a micro offset, a length and
a velocity — and *which* step depends on where that track's cursor is, which
under polymeter and SCALE is not where the transport bar's readout is. So
recording is three jobs, on three threads, and the design is mostly about which
job goes where:

1. **Capture**: an input port open on a driver thread, stamping each message
   with the moment it arrived. Dumb and fast. (`midi` crate.)
2. **Place and monitor**: the engine thread — the only thread that knows what
   time it is and where every cursor stands — echoes each note straight to the
   selected track's box so it can be heard, and, while a take is running,
   converts each arrival into *this track, this step, this micro, this pass*.
   (`engine` crate; pure arithmetic in one file.)
3. **Take**: the UI thread — the only thread that owns the model — pairs ons
   with offs, applies the overdub rule and the polyphony cap, writes notes
   into the track, and closes the take as one undo step. (`core::record` for
   the rules, `app::record` for the glue.)

Nothing in this feature writes a byte of SysEx. The only thing a box receives
is the notes you play, on the channel the selected track already plays on.

## 1. Vocabulary

| word | meaning |
|---|---|
| **record input** | the one input port the recorder listens to. Session-level, chosen in Setup. Not a box's bound input, which stays for SysEx. |
| **armed** | REC is lit. Nothing is captured until the transport is also running. |
| **take** | the span from the first captured note-on while armed and playing to the moment STOP is pressed (or REC is switched off). One undo entry. |
| **pass** | one trip through the armed track's `length_steps`. A take usually spans several; overdub means every pass adds. |
| **thru** | echoing a played note to the selected track's port and channel, always, armed or not, playing or stopped. |
| **placed event** | a note-on or note-off after the engine has converted its arrival time to `(step, micro, pass)` on the armed track. |
| **live event** | a raw note-on/off as it came off the port, stamped with an `Instant`. |

## 2. Decisions taken, 2026-09-05

Neil's answers, in the order they were asked. Each has consequences below.

1. **Source: one chosen input port in Setup**, all channels merged. The
   incoming channel is ignored on the way in; thru rewrites it to the track's.
2. **Thru always on**, to the selected track's port and channel, playing or
   stopped. Selecting a track is how you choose what the keyboard sounds.
3. **Overdub, looping like the box's LIVE REC.** Every pass adds notes. A new
   note on an occupied `(step, pitch)` replaces the old one. One undo entry
   per take.
4. **As played, with a QUANTIZE toggle.** Micro-timing is kept; the toggle
   snaps new notes to the step and their lengths to whole steps while it is on.
5. **REC arms, PLAY runs it, STOP ends the take.** REC while stopped also
   starts the transport from the top. No count-in.
6. **Polyphony overflow: keep the first four, drop the rest, count it in the
   console.** The same rule as the import's §4.6. On an A4 track the four are
   written as root plus NO2–NO4 by the existing chord path.
7. **Notes only.** Velocity and held length. CC, aftertouch, pitch bend and the
   sustain pedal are ignored, on capture *and* on thru, until §8.

## 3. Facts about the codebase the implementer must use

- **Time.** The engine thread's clock is `Instant`: `EngineThread::started_at`
  is set on `Start`, re-based on `Continue`, and every event's `at` is seconds
  since it (`transport.rs` ~375–381). `TrackCursor { next_step, origin,
  origin_at }` gives each track's position: `origin_at` is the second its
  current pattern began, `elapsed_steps() = next_step - origin`, and
  `Scheduler::step_in_pattern` already does the modulo. `time::track_step_seconds(bpm, scale)`
  is the step length. **Do not use `TransportState::position_millisteps` for
  placement** — it is the global readout, scaled by the UI for display
  (`workspace.rs` ~70), and after a scene switch it and a cursor disagree.
- **midir.** `MidiInput::connect(port, name, callback, data)`; the callback
  receives `(timestamp: u64, bytes: &[u8], &mut data)` on a driver thread.
  **Ignore midir's timestamp and stamp `Instant::now()` in the callback**: the
  u64 is microseconds on a per-backend epoch (CoreMIDI host time, ALSA queue,
  WinMM ms since `midiInStart`) and is not comparable to the engine's
  `Instant` without a calibration nobody wants to maintain. Default `Ignore`
  is fine here — SysEx, time code and active sensing all filtered — and is the
  *opposite* of what `SysExInbox` sets, which is why this is a new type.
- **The engine never touches `midir`.** `PortSink` is a trait, `midir_sinks()`
  in `app::engine` builds the real one, and `tests/all/engine_link.rs` drives
  `EngineLink` against a recording sink that logs `(PortId, bytes)`. Thru
  must be testable the same way.
- **The engine allocates as little as it can on its thread** (PLAN §4: "never
  locks and never allocates"). Today it already drops the `Vec` a `SendNow`
  carried. A placed event sent back over an `mpsc` allocates one node per
  note; §4.3 accepts that explicitly and names the ring buffer that replaces
  it if the jitter stats ever say so.
- **Commands** are `TransportCommand`, one channel, UI to engine only.
  `EngineLink` remembers `send_clock`, `fill`, `scene`, `song_mode` because a
  rebuild is a new scheduler that knows none of them; anything recording adds
  to the engine has to be remembered the same way.
- **Port resolution for a control the user is turning** is spelled once, in
  `EngineLink::send_track_level`: the track's `out_port` else the device's
  `io.output`, looked up in `self.ports`. Thru resolves its target with the
  same function, not a copy of it (DEVELOPMENT.md lesson 5).
- **The model.** `Note::new(step, pitch, len, velocity, micro)`; `edit_ops::clamp_micro`
  (±0.49), `clamp_velocity`, `lengths::snap_len_fine(len, max_steps)`,
  `lengths::LEN_MIN`. `edit_ops::adopt_step_trig` is what the roll calls when
  a note joins a step that already has trig-lane state; recorded notes call it
  too. `DeviceModel::notes_per_trig` is the cap (4 on all three boxes).
- **Undo.** `History::begin(Content::of(session))` … `commit(session)`. The
  shell (`main.rs` ~452–459) begins a step on the first `edited` frame and
  **commits every frame the pointer is up**, which would cut a take into one
  step per frame. §5.4 changes that guard.
- **Edits reach the engine** by setting `edited`, which snapshots the whole
  session at frame end (`EngineLink::sync`). `Scheduler::prepare` keeps
  `next_step` per `(device, track)`, so a note landing mid-play moves nothing
  else. Recorded notes need no new path: they are edits.
- **Repaint.** egui draws only when asked. The transport bar requests a
  repaint every frame while playing (`transport.rs` ~155), so a note placed
  during a take is drawn the next frame without new work. Thru cannot rely on
  the UI at all — while stopped there may be no frame for seconds — which is
  the whole reason thru lives on the engine thread (§4.2).
- **Selection.** `tracks::Selection { device: usize, track: usize }`, device
  being an index into `session.devices`. The engine's cursors key on
  `(DeviceId, track)`; the UI converts.
- **Keys.** The rail owns `E H G S P`, the grid `Shift+C/V`, Delete and the
  arrows; the transport owns plain Space via `space_tap`, whose two rules —
  first press of a hold only, `matches_exact` so chords stay free — apply to
  any new transport key. `R` is free.
- **Glyphs.** `●` U+25CF was a tofu box on the scene bar; it is *not* on the
  confirmed list in `ui/mod.rs`. REC is the word `REC`, no dot.
- **Session-level desk settings** (`harmony`, `song`, generator settings) are
  `#[serde(default)]` fields on `Session`; `project::FORMAT_VERSION` stayed at
  1 when `song` was added, and stays at 1 here.
- **Setup's port pickers** are `devices::picker` / `port_choices`, a
  `ComboBox` offering none-first-then-every-connected-port, tested headless.
  The record input uses the same widget with a session field as its target.
- **Tests** go in `crates/<crate>/tests/all/` with a `mod` line; sentence-case
  names; no hardware in a suite. The engine and `engine_link` suites already
  say they have no JS oracle, and neither does this — `js/midi.js` never
  listened to an input.

## 4. Stage by stage

### 4.1 Capture — `midi::live_input`

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveEvent {
    pub at: std::time::Instant,   // stamped in the callback, not midir's u64
    pub kind: LiveKind,
}
pub enum LiveKind { NoteOn { pitch: u8, velocity: u8 }, NoteOff { pitch: u8 } }

/// One channel-voice message to a `LiveKind`. Pure; the callback calls it.
/// Note-on with velocity 0 is a note-off (MIDI 1.0 §4.2). The channel nibble
/// is read and discarded — decision 1. Anything else returns `None` — decision 7.
pub fn parse_live(bytes: &[u8]) -> Option<LiveKind>;

pub struct LiveInput { _conn: MidiInputConnection<()>, pub port: PortBinding }
impl LiveInput {
    /// Open `binding` (id first, then name — `resolve_input`) and forward every
    /// parsed message down `tx`. A closed receiver is not an error: the engine
    /// that owned it has been rebuilt, and `EngineLink` reopens.
    pub fn open(binding: &PortBinding, tx: Sender<LiveEvent>) -> Result<Self, MidiError>;
}
```

The callback does nothing but `parse_live`, `Instant::now()` and `tx.send`.
Drivers deliver whole channel messages on all three backends, so there is no
running-status reassembly to write; the test for `parse_live` says so and pins
the velocity-0 rule, the channel-nibble discard and the six ignored kinds.

### 4.2 Place and monitor — `engine`

Two new commands and one new input to the thread:

```rust
pub enum TransportCommand {
    …
    /// Where thru goes: the selected track's resolved port and channel, or
    /// `None` to go quiet. Re-sent by the UI whenever the selection moves.
    SetMonitor(Option<(PortId, u8)>),
    /// Arm or disarm, naming the track a take would land on, and QUANTIZE.
    /// Re-sent on every change; remembered by `EngineLink` across rebuilds.
    SetRecord { armed: bool, target: Option<(DeviceId, usize)>, quantize: bool },
}

pub struct PlacedEvent {
    pub kind: LiveKind,
    /// Whole step within the armed track's pattern, 0-based.
    pub step: u64,
    /// Fraction of a step from that grid point, in (−0.5, 0.5]. Zero when
    /// QUANTIZE is on.
    pub micro: f64,
    /// Which trip through the pattern this fell in; the take reports it.
    pub pass: u64,
    /// Seconds since `started_at`, kept so a take can compute a held length
    /// exactly rather than from two rounded steps.
    pub at: f64,
}
```

`Transport::spawn` grows two parameters: `live_rx: Receiver<LiveEvent>` and
`placed_tx: Sender<PlacedEvent>`. Each pass of `run`, before `send_due`, the
thread drains `live_rx`:

- **Thru, every event, every state.** With a monitor set, the message is
  written as `MidiMsg::NoteOn/NoteOff` on the monitor channel and sent to the
  monitor port *immediately* through the sink — never queued behind
  scheduled events, for the reason `SendNow` gives. A 128-slot `[bool; 128]`
  of held thru pitches is kept so that `Stop`, `Panic` and a monitor change
  release them; without that, changing track while holding a chord leaves it
  ringing on the old box.
- **Placement, only while armed, playing and the target has a cursor.**
  `record::place(origin_at, step_secs, length_steps, t, quantize)` — pure,
  in a new `engine/src/record.rs`:
  `elapsed = (t − origin_at) / step_secs`; `abs = elapsed.round()`;
  `micro = elapsed − abs` (0 if quantize); `step = abs % length`;
  `pass = abs / length`. A hit just late of the last step rounds onto step 0
  of the next pass with a negative micro, which is what the box does. **Swing
  is not subtracted**: LIVE REC on the box records against the straight grid
  and swing is applied on playback, and the scheduler already does the
  second half. The `Scheduler` gains one small method that looks up the
  target cursor and the track's `scale`/`length_steps` and calls `place`.
- The placed event goes down `placed_tx`. One heap node per note-on and one
  per note-off; a fast player makes perhaps twenty a second. If
  `JitterStats` ever moves because of it, the replacement is a fixed-capacity
  ring in `TransportState` — designed for, not built.

Latency: the thread wakes at least every `IDLE_POLL` (5 ms), so thru adds at
most that plus one USB hop. Measured, not assumed — Phase A's acceptance
includes a spy-driver capture of keyboard-in to box-out. If it disappoints,
the fix is to park on a shorter poll only while a `LiveInput` is open; sending
thru from the driver callback through a second connection to the same port is
rejected, because two writers on one port can interleave with a running
four-message NRPN.

### 4.3 The take — `core::record`

Pure rules over placed events, owning no threads and no egui:

```rust
pub struct TakeOptions { pub notes_per_trig: usize, pub max_steps: f64, pub quantize: bool }

pub struct Take {
    /// Open notes by pitch: the on event's step/micro/velocity/at and the
    /// id of the provisional `Note` already in the track.
    held: [Option<Held>; 128],
    pub report: TakeReport,     // placed, replaced, dropped_full, passes, dropped_no_target
}

impl Take {
    /// A note-on: insert a provisional `Note` (len 1 step) at once so the
    /// roll shows it while the key is down; apply the overdub rule; apply
    /// the cap. A note-off: set the held note's length from the two `at`s,
    /// snapped by `snap_len_fine` (whole steps when `quantize`), floored at
    /// `LEN_MIN`, capped at the track's `length_steps`. Returns whether the
    /// track changed.
    pub fn push(&mut self, ev: PlacedEvent, track: &mut Track, o: &TakeOptions) -> bool;
    /// STOP with keys still down: every held note keeps its provisional
    /// length. Returns the report for the console.
    pub fn close(self, track: &mut Track) -> TakeReport;
}
```

Rules, each pinned by a test in `core/tests/all/record.rs`:

- **Overdub replace**: a note-on whose `step` and `pitch` match an existing
  note replaces that note's velocity, micro and (on release) length.
  Counted as `replaced`.
- **Cap**: the notes whose `step.floor()` equals the new step are counted; if
  already `notes_per_trig`, the arrival is dropped and `dropped_full` counts
  it. Replacement is checked first, so re-playing a note on a full step is a
  replace, not a drop.
- **Step trig state**: a note joining a step that already has one calls
  `adopt_step_trig`, exactly as the roll does.
- **Velocity** is stored as played through `clamp_velocity`; **micro** through
  `clamp_micro`, so a −0.5 rounds to the value the box can hold.
- **A note-off with no held note** (key was down before the take started, or
  the take started mid-hold) is ignored.
- **Passes** are reported as `max(pass) + 1`.
- Nothing here reads the pointer, the selection or the engine.

### 4.4 Glue — `app::record::Recorder`

Owns the `Receiver<PlacedEvent>`, the open `Take`, the armed flag and
QUANTIZE, and is ticked once per frame from the shell **before the workspace
draws**, so the roll shows this frame's notes:

1. Drain placed events. On the first one with no take open: resolve the
   selected track; `history.begin(before)` (the shell's `before` for this
   frame); open a `Take`. Then `push` each into that track; set `edited`.
2. If the target track has no port, post once per take: "REC: DT2 T7 is
   routed nowhere — recording anyway, but you will not hear it."
3. Take ends — STOP pressed, REC switched off, the selection moved, or the
   scene sounding changed — `close`, `history.commit`, and post the report:
   `REC: 23 notes onto DT2 T3 over 2 passes · 3 replaced · 1 dropped (step 5
   full)`. A take with nothing placed is not a take: no history step, no line.
4. Every frame, re-send `SetMonitor` when the resolved `(port, channel)` of
   the selection differs from the last one sent, and `SetRecord` when armed,
   target or quantize differ. Cheap, and it is what makes "select a track,
   play the keyboard, hear that box" hold without anyone remembering to tell
   the engine.

`EngineLink` gains `armed`, `record_target`, `quantize`, `monitor` fields
re-sent after every rebuild (alongside `fill` and `scene`), a `LiveInput` it
reopens on every rebuild against `session.record_input`, and `reroute` learns
to compare the record input too so a replugged keyboard comes back the same
way a replugged box does.

## 5. UI

### 5.1 Transport bar

- **Zone 1** gains `REC` after `▶▶`: an outline button that fills in the
  amber destructive treatment when armed — the one colour rule the bar has
  is *filled cyan means a thing you can press*, and armed-REC is a state, not
  a press. Tooltip: "Arm recording onto the selected track — or press R.
  Play the keyboard; STOP ends the take." Pressing REC while stopped arms and
  calls `engine.play(session)`. Disabled, with a tooltip saying why, when no
  record input is set, when nothing is selected, and in song mode (§9).
- **Zone 4** gains a `QUANT` pill beside `FILL`, same widget, same lit/unlit
  treatment. Tooltip names what it snaps: new notes to the step, their
  lengths to whole steps; existing notes are never touched.
- **Zone 3**: the position readout is unchanged. While a take is open the
  roll's playhead is drawn in the REC colour so the state is visible over the
  notes, not only in the corner.

### 5.2 Key

`R` toggles armed, read in `transport::shortcuts` next to Space with the same
`space_tap` discipline (first press of a hold; `matches_exact`; not while a
field has the keyboard; not while a modal is waiting). Per
`verify-platform-sends-the-event`: Phase D's acceptance is the key working
in the running app, not a synthetic `Event::Key` in a test.

### 5.3 Setup

A **RECORD INPUT** row in the MIDI PORTS section: one `picker` over
`list_inputs()`, target `session.record_input: Option<PortRef>`, none first.
The status strip's "a port will not open" rule extends to it, so an unplugged
keyboard opens the strip the way an unplugged box does. `EngineLink::failures`
carries the open error text; the console gets it once.

### 5.4 The shell

Two lines in `main.rs`:

- The per-frame commit becomes
  `if !pointer.any_down() && !self.recorder.take_open() { history.commit(..) }`
  — a take holds the step open the way a drag does.
- `recorder.tick(..)` runs after `transport::shortcuts` (so REC-while-stopped
  can start the transport this frame) and before the panels draw.

## 6. Where things live

| crate | new | changed |
|---|---|---|
| `midi` | `live_input.rs`: `LiveEvent`, `LiveKind`, `parse_live`, `LiveInput` | `lib.rs` re-exports |
| `engine` | `record.rs`: `place`, `PlacedEvent` | `transport.rs`: two commands, `live_rx`/`placed_tx`, thru + held table in `run`, release on Stop/Panic; `scheduler.rs`: target lookup calling `place` |
| `core` | `record.rs`: `Take`, `TakeOptions`, `TakeReport` | `session.rs`: `record_input: Option<PortRef>` (serde default) |
| `app` | `record.rs`: `Recorder` | `engine.rs`: remembered fields, `LiveInput` lifecycle, `reroute`; `ui/transport.rs`: REC, QUANT, `R`; `ui/ports.rs` or `setup.rs`: RECORD INPUT picker; `ui/pianoroll.rs`: playhead colour while recording; `main.rs`: §5.4 |

## 7. Build order and acceptance

Each phase is one PR-sized change with its own tests; do not start the next
without the previous one green and clippy clean.

**A — Capture, and a console line.** `midi::live_input` with `parse_live`
tests. Temporary: `EngineLink` opens the first input port whose name contains
a `DRS_RECORD_INPUT` env value and posts every note to the console. Accept: a
real keyboard's notes appear in the console with pitch and velocity, on this
Mac, before any button exists (DEVELOPMENT.md lesson 7). No session change
yet; the env hook is deleted in C.

**B — Thru.** `SetMonitor`, the held table, release on Stop/Panic/monitor
change. `Recorder` skeleton doing only step 4 of §4.4. Accept:
`engine_link.rs` tests against the recording sink show a note-on arriving as
`0x90|ch` on the selected track's port and channel, the channel rewritten,
and the matching note-off sent on Stop. On the desk: select a DT2 track,
play, hear the DT2; select an A4 track, hear the A4; hold a chord and change
track, nothing rings. Spy-driver capture of the added latency, written into
this file's §4.2.

**C — Placement.** `engine::record::place` and the scheduler method.
`SetRecord`. `Session.record_input` and the Setup picker replace the env
hook. Accept: `engine/tests/all/record.rs` pins nearest-step rounding, the
late hit landing on step 0 of the next pass with negative micro, 2x SCALE, a
64-step track against a 128-step one from one clock, placement after a
`commit_scene` moved `origin_at`, and quantize zeroing micro. Save/reopen
keeps the record input.

**D — The take.** `core::record::Take` with every §4.3 rule tested;
`Recorder` steps 1–3; REC and QUANT on the bar; `R`; the shell's commit
guard; the playhead colour. Accept: on the desk, arm, play a bar onto a DT2
track over two passes, hear it play back on the third pass, STOP, one Cmd+Z
removes the whole take. A fifth note on one step is dropped and the console
says which step. `shell_keys.rs` gains `R` next to Space's tests.

**E — On the box.** Write a recorded pattern to a DT2 with the existing
`safe_write` path; read its screen for one trig's micro against the roll's
value; fetch it back and confirm the round trip matches to the byte, as the
`.syx` suites do. Record onto an A4 track with a triad and confirm the box
shows the root with NO2–NO4 set. Then strike the item from `PLAN.md` §1 with
the date and add the §9 ledger entry.

## 8. Deferred, but designed for

- **CC to p-lock lanes**, and CC thru. `parse_live` gets a `Control` kind;
  the take maps a controller through `params::param_table_for` to a lane at
  the placed step. The engine's thru path forwards it unchanged.
- **Sustain pedal** holding note-offs until CC 64 falls. A `held_by_pedal`
  flag in `Take`.
- **Channel filter** on the record input, for a split controller.
- **Song mode takes**, once "which pattern is under this track" during a row
  change has an answer the take can follow (§9).
- **A count-in**, if the position readout alone turns out not to be enough.
- **A ring buffer for placed events**, if the jitter stats move.
- **Step recording** while stopped: place at the caret. Blocked on the
  playhead work `PLAN.md` §1 names under Paste.

## 9. Decisions Neil owns

1. **Does STOP disarm?** Design: yes — STOP ends the take *and* switches REC
   off, so a stray key after stopping does not land in the next PLAY. The
   box leaves REC lit; one press re-arms. Flip if the box's habit wins.
2. **Song mode.** Design: REC is disabled while walking the song, with a
   tooltip. A take could instead follow the row walk and split at each row.
3. **Selection change mid-take**: design closes the take and opens a new one
   on the new track (two undo steps). The alternative is one step across both.
4. **Held-note display**: design inserts a provisional one-step note on
   key-down so the roll shows it; the alternative shows nothing until release.

## 10. Rules carried over

From `PLAN.md` §7 and this document: recording writes no SysEx and touches no
slot on a box — the only bytes a box receives are the notes you play, on the
channel that track already uses; never clamp pitch (the roll grows a row, as
the import does); never invent trig conditions; a dropped note is counted and
named in the console before the take ends; one take is one undo step; the
session file must round-trip a recorded track and the record input unchanged;
and no test anywhere in this feature needs a box or a keyboard.

---

## 11. What was built, and where it differs — 2026-09-05

Phases A–D are in. Every acceptance in §7 that does not need a box or a keyboard
is met by a test; phase E and the two on-the-desk lines in phases B and D are
not, and are the whole of what is outstanding.

**63 new tests**, none of which needs hardware: `midi::live_input` (7, in-module),
`engine::record` (13 in-module + 9 in `engine/tests/all/record.rs`),
`core/tests/all/record.rs` (22), `app/tests/all/record.rs` (6),
`app/tests/all/engine_link.rs` (8 more), `app/tests/all/transport_space.rs`
(5 more, for `R`), `core/tests/all/session.rs` (2, for the record input in the
file). Clippy is clean.

### Two things the design could not compile as written

1. **`PlacedEvent` lives in `core::record`, not in `engine::record`.** §4.3 has
   `core::record::Take::push` taking an `engine::record::PlacedEvent` whose
   `kind` is a `midi::live_input::LiveKind` — and `core` depends on neither
   `engine` nor `midi`. So the placed event moved down to the crate that
   consumes it, `engine::record` re-exports the type so the design's path still
   resolves, and the note kind exists twice: `LiveKind` on the wire side and
   `PlacedKind` on the model side, converted by `engine::record::placed_kind`.
   A free function rather than a `From` impl, because both types are foreign to
   `engine` and the orphan rule forbids one.

2. **`TakeOptions` has a fourth field, `step_secs`.** A held length is
   `(off.at − on.at)` in seconds and a `Note::len` is in steps; `core` has no
   tempo and no SCALE to convert between them, so the caller hands it over.
   `app::record::take_options` computes it from `session.tempo_bpm` and the
   armed track's own `scale`, which is the same `time::track_step_seconds` the
   scheduler dates that track's events with.

### A factual correction to §3

**midir's default `Ignore` filters nothing.** §3 says the default filters SysEx,
time code and active sensing and that `SysExInbox` sets the opposite; all seven
of midir 0.11's backends initialise `ignore_flags` to `Ignore::None`, so
`SysExInbox`'s explicit `Ignore::None` is a no-op and the filtering this feature
wants had to be asked for. `LiveInput::open` sets `Ignore::All` — SysEx, timing
and active sensing — which matters on a desk where an Elektron box shares the
cable and emits active sensing several times a second.

### Smaller deviations, each with its reason

- **Phase A's `DRS_RECORD_INPUT` env hook was never built.** It exists in the
  plan to let a person confirm capture works before there is a button, and the
  same change that would have added it also adds the Setup picker that replaces
  it in phase C. A throwaway created and deleted in one commit is not a
  checkpoint; the hardware confirmation it stands for is phase E's.
- **The RECORD INPUT row is always visible**, directly under the device block
  and above DATA TRANSFER, rather than inside the `BOXES & MIDI PORTS`
  disclosure §5.3 names. That disclosure is collapsed by default, and REC's own
  tooltip sends you to this row to pick a keyboard — a row you are sent to has
  to be on screen when you arrive. §5.3's other half is kept exactly: a record
  input that will not open forces the device status strip open, the way an
  unplugged box does.
- **`space_tap` became `key_tap(ctx, session, key)`.** `R` wants the same two
  rules Space has — first press of a hold, `matches_exact` — for the same
  reasons, so there is one function rather than two that can drift. It carries
  one special case: `Key::name()` gives `"Space"` for the spacebar and the
  platform pushes `" "` beside it.
- **`devices::picker` was split.** Its combo-box half is now
  `devices::port_picker`, shared with the RECORD INPUT row, so the `.truncate()`
  rule that keeps the Setup panel 320px wide on ALSA exists once (lesson 5).
  `EngineLink::resolve_track_port` is the same treatment for the port-resolution
  rule §3 asked to be shared: `send_track_level` and thru now call it rather
  than each spelling it.
- **`EngineLink` gained an `InputFactory`**, the mirror of `SinkFactory`, so
  nothing in the test suite needs a keyboard plugged into the machine running
  it. The handle it returns is opaque — dropping it closes the port — so a test
  hands back the `Sender` it means to play into.
- **`Recorder::finish` reports `false`** rather than an edit. The notes were
  written on the frames they arrived on; closing a take moves nothing, and
  saying it did would cost a whole-session snapshot down the channel for a
  button release.

### §9's four decisions, as built

All four were taken as the design proposed: STOP disarms; REC is disabled in
song mode with a tooltip saying so; a selection change closes the take and opens
a new one; and a key going down inserts a provisional one-step note so the roll
shows a held chord as a chord.

### On the desk — 2026-09-05, DT2 0071 / A4 0195

Played the day it was built. A keyboard on the record input, thru heard on the
selected track's box, and takes recorded onto a DT2 track and an A4 track.
Neil's words: **"works perfectly."**

That closes the two on-the-desk acceptances in §7's phases B and D, and it
closes the only claim in this whole feature that no test could ever have made —
that the notes land where they were played, to an ear. The 63 tests say the
arithmetic is right; a box is the only thing that can say the arithmetic was the
right arithmetic.

It is recorded as a session of playing rather than as an itemised check, which
is the weaker of the two and is what it was. The DN2 was on the desk and was not
recorded onto.

### What is left

- **Phase E's write-back**: a recorded pattern written to a DT2 with the existing
  `safe_write` path, one trig's micro read off the box's screen against the
  roll's value, and the pattern fetched back and matched to the byte.
- **Phase E's A4 check**: a triad recorded onto an A4 track, and the box's own
  screen showing the root with NO2–NO4 set. The chord path this leans on is
  already hardware-verified (`PLAN.md` §10, "Chords reach the A4"); what is
  unverified is a *recorded* chord reaching it.
- **Phase B's spy-driver latency measurement**, which §4.2 says should be written
  back into that section. The number is not in this document because it has not
  been taken — §4.2's "at most `IDLE_POLL` plus one USB hop" is still a bound
  and not a measurement. Nobody has complained about the latency, which is
  evidence of a kind and not the kind §4.2 asked for.
- **Three disabled-REC tooltips** (§5.1) have not been on a screen: no record
  input, song mode, and nothing selected. Each is a sentence against the right
  edge of a 320px column.
- The §8 list is untouched and still deferred.
