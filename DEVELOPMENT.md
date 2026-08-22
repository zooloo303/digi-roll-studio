# digi-roll-studio — development notes

`PLAN.md` is the architecture and the rules. This is the other half: how the
thing was actually built, what the tests can and cannot say, and the lessons that
kept repeating. Source comments cite it as `DEVELOPMENT.md` and by lesson number;
those numbers are stable.

Everything below was distilled from eighteen build sessions. **Each of the nine
lessons escaped a green test suite at least once**, most of them more than once.
That is the whole reason this file exists.

---

## Working on this

```sh
cargo build --release
cargo test --workspace          # no system dependencies, no hardware
cargo run -p digi_roll_studio
```

The protocol suites read `.syx` captures from `crates/protocol/tests/fixtures/`,
committed so the tests run anywhere.

**Where expected values come from.** Every phase so far derived them from the JS
original ([digi-roll](https://github.com/zooloo303/digi-roll)) *before* writing
them into a Rust test — `node --input-type=module -e` against its `js/**` is the
cheapest hardware-verified oracle available. Three rules came out of using it:

- **Where there is no oracle, say so in the file.**
  `crates/engine/tests/scheduler.rs` has none — `js/midi.js` sequences one track
  of one box, so nothing there can derive polymeter or two-port output — and its
  header says so in the first paragraph. Same for `crates/app/tests/engine_link.rs`.
- **Check the oracle covers what you are porting before trusting it.**
  digi-roll's `test/copy-track.test.js` contains no occurrence of `plock`, `lane`
  or `prob`, so the three hardest things `copyTrack` does had no test on either
  side of the port. That was visible before a line was written.
- **An oracle can be complete about the part it tests and *wrong* about the part
  it does not.** Phase 11 ported two files at once: `js/chords.js`, which its
  tests cover thoroughly, and `js/main.js`'s `harmonize` handler, which has no
  test at all and **contains a bug** — it tests for a note collision against the
  pattern while its own additions are still in a separate array, so two selected
  notes a third apart both add the fifth above the lower one. Ported faithfully,
  that would have written a duplicate pitch onto a trig holding four notes. The
  tell was structural and cheap to spot: **the tested file was pure and the
  untested one was the glue**, which is where a JS app keeps its decisions.

---

## Hardware examples, by safety class

Nothing in the dev loop needs a box (`PLAN.md` §7 rule 5). These exist for when
you deliberately want one, and they are ordered by what they can do to it.

- `cargo run -p digi_midi --example list_ports` — enumeration only.
- `cargo run -p digi_roll_studio --example identify_into_session` — read-only,
  two API requests per box.
- `cargo run -p digi_roll_studio --example fetch_pattern_kit` — read-only; adds
  one 0x60 pattern-kit request per box.
- `cargo run -p digi_roll_studio --example trig_write_dry_run` — **read-only.**
  Fetches A01 off each box, runs the trig-condition write half over the real
  payload *in memory*, prints which bytes moved and whether minimal diff held.
  Constructs no write opcode. The fastest check that a change has not broken
  minimal diff on a real pattern.
- `cargo run -p digi_roll_studio --example safe_write_track` — **read-only.**
  Runs the entire safe-write flow against a live pattern and swallows the send:
  the `Rehearsal` wrapper records the store instead of transmitting it, so **no
  0x50 opcode is constructed anywhere in the run**. Rehearsal backups go to a temp
  directory, not the real restore list.
- `cargo run -p digi_roll_studio --example safe_write_track -- --write` — **stores
  bytes in a slot.** Asks for the typed word `overwrite` per box and backs the
  whole destination pattern up first.
- `cargo run -p digi_engine --example jitter --release -- "<DT2>" "<DN2>"` —
  **sends** clock and notes for ten seconds, so a box on external clock will start
  playing. `--release` matters: a debug build measures the debug build.
- `cargo run -p digi_roll_studio` — **the only thing here that can change what is
  on a box's card.** It can also write a file that is not a session or a backup:
  the Edit panel's `Export…` writes a `.mid` wherever the dialog is pointed. From
  the moment a box has an out port and Play is pressed it sends clock, notes and
  parameter changes; Setup's Fetch adds read-only SysEx. Three things store bytes
  on a card, all inside the panel's `WRITES TO THE BOX` frame and all through the
  same five rules: **SEND TO BOX overwrites one track of one slot**, **SYNC EVERY
  TRACK does that to every track of every box in one press**, and **BACKUPS
  replaces a whole slot** — all sixteen tracks, the kit and its sounds.

  **Auto-connect is on by default and it opens ports by itself**, every three
  seconds, to ask an unclaimed Elektron-looking port who it is. Read-only — two
  API requests — but it is the first thing here that talks to a box without being
  pressed, so it is worth knowing about before wondering who is holding a socket.
  The checkbox is at the bottom of BOXES.

## The desk's own configuration, which the app cannot read or write

- **One clock master at a time, and it is a per-box rule.** When the app is
  master, every box needs CLOCK RECEIVE + TRANSPORT RECEIVE **on** and CLOCK SEND
  **off** — not because of what a box does to itself, but because its DIN out is a
  second master on the *other* boxes' inputs. With a DT2 set to send, **neither**
  box took sync. First thing to check when sync looks broken.
- **A factory DT2 or DN2 gives channels to tracks 1–8 only.** `TRACK 9-16 CH` are
  `OFF`, **channel 9 is `FX CONTROL CH`** and **channel 10 is `AUTO CHANNEL`**, so
  a 1:1 map aims track 9 at nothing, track 10 at whichever track is *selected* on
  the box, and 11–16 at nothing. Fix in `SETTINGS > MIDI CONFIG > CHANNELS`.
  `ui::tracks::channel_note` says so in amber beside the CH field.

---

## Lessons that keep repeating

### 1. A mark inherited from a port is still a mark you are shipping

Five instances of tofu boxes reaching the screen, the worst of them `✓` opening
the app's proudest sentence — `□ Wrote 3 notes to A01 T1 — verified
byte-identical` — and `→` in the "Takes our clock" tooltip, read `SETTINGS □ MIDI
CONFIG □ SYNC`, in the one tooltip someone reads *because* their sync is already
wrong. **No test can see a missing glyph.** `ui::mod`'s glyph section lists what
has been read off a real screen; anything else is a guess. Where a mark matters
and the font cannot be trusted, **draw it** — `paint_info_icon` is the ⓘ, drawn,
with a test that asserts against the paint list because absent and present are
indistinguishable to a test that only reads what the drawing code returned.

### 2. A committed witness that no test asserts on is worse than a missing one

The swing byte escaped **four times**. Fixtures crossing the boundary — DT2
captures at swing 50, `dn2-fresh-A01` at 78 — and not one test looked at the byte.
Cheaper to repeat than a missing fixture, and harder to notice.

**Measured.** `a_session_round_trips_through_the_project_file` asserted
`back == s` on a fixture that left every `#[serde(default)]` field at its default.
Dropping each field from the file one at a time, **seven of them broke nothing**:
`Note.cond`, `Track.plocks`, `Track.mute`, `Pattern.source`, `PLockLane.trigless`,
`PLockLane.values` and `DeviceIo.takes_clock` — all 709 tests green through every
one. `takes_clock` is the sharp one: its default is `true`, so **only a fixture
that sets it `false` can witness it going missing at all.** The rule that falls
out is mechanical — **a round-trip fixture must set every defaulted field to
something that is not its default**, or the assertion is decoration.

### 3. A panel that lies about what is built sends someone looking for a feature they are already looking at

`ui::tools` told users the trig lane and p-lock lanes were "not built" for a day
after both shipped. The rule is in that file: anything shipped gets its state line
rewritten **in the change that ships it**.

### 4. Check a test's body against its name

Seven tests have been found whose names promised more than they checked. The shape
to watch for: **an assertion whose expected value is also the default.** Those
pass before the code under test runs, so the plant that should break them cannot.

**Measured (Phase 10), and both were *new tests written that day*** — this is not
only a thing old suites do:

- A test asserting a mass send aimed at the slot the pattern came from, written
  against a pattern **sitting in A01 that had come from A01**. Deleting the whole
  provenance rule (`let into = from`) failed nothing. The fix is the case that can
  tell them apart: a pattern in A03 that came off C06.
- A test asserting auto-connect will not steal a port matched by *name* when the
  id has changed — with only one DT2 in the session, which already had ports. The
  "no room" rule refused the candidate first, so the name comparison could be
  deleted and the test still passed. The fix is a second, port-less box.

Both were caught by the plant, neither by review, and the tell in each case is
that **the fixture made two different rules give the same answer.**

### 5. A rule that lives in three places will be forgotten in one of them

Three deliberate bugs about backup-list freshness all escaped fifteen tests
because the freshness rule was restated in three callers. Asking the store instead
(`Stash::generation`, one `stat` a frame) made two of the three unrepresentable.

**Measured, and it was a *count*.** `ui::sync` counted a destination track's
trigs as `trigs.len()`; `safe_write_tracks` counts them with
`protocol::pattern::track_trig_count`, which reads the enabled bit. On the DT2
fixture those differ by one — that capture holds the leftovers of a trig deleted
on the box. Because the two numbers are *compared to each other*
(`changed_since_survey`), two ways of counting the same thing did not merely word
a dialog differently: **every write refused itself.** Found in ten seconds by the
guard it broke, which is the argument for guards that compare rather than assume.

**Measured again, and this is the sharpest instance — the rule was living in the
*callers* of the thing that should have owned it.** `TransportCommand::Snapshot`
replaced the engine's whole session and re-prepared the scheduler against it, and
took every field except `tempo_bpm`. That made the engine's clock a second source
of truth reachable only by an explicit `SetTempo` sent *beside* the snapshot, so
"a tempo edit is two calls" had to be remembered by each of the three places that
write `session.tempo_bpm`. Two remembered. The Generate panel's SET button set
the transport to 174 and left the boxes at 120 — **and `ui::session`'s open had
the identical bug on a path nobody had reported yet**, because `*session = loaded`
carries a tempo too. Three things generalise:

- **A command named for a whole-object replacement that quietly excludes one field
  is the same defect as a rule in three places, one layer down.** Every caller now
  has to know about the exclusion, and the name says they do not.
- **The two callers that worked were what hid it.** The BPM field and the
  fetched-tempo offer both send the pair, so the mechanism looked correct
  everywhere anyone had looked.
- **The symptom named the fix.** 174 on screen against 120 in the room is two
  numbers that are supposed to be one — this lesson's own smell — and the fix was
  to delete the second one, not to add a third caller that remembers.

### 6. The deliberate-bug pass is what finds all of the above

Plant a bug per claim, confirm exactly one test fails, and **take a plant that
fails *nothing* as the finding** rather than the nuisance. That is how lessons 1,
2 and 5 were each caught. When a plant genuinely cannot fail, say so in the doc
comment instead of faking a test for it.

Four passes, and the results differ in a way worth recording:

- **Phase 8.** `mark_edited` written `=` instead of `|=`, missed by every test in
  the new file because they all happened to call it with `false` before `true`.
  That marks a session clean on the first quiet frame after an edit — and the
  close guard reads exactly that flag, so the window would have shut over unsaved
  work.
- **Phase 9 — forty-two plants, five that failed nothing**, and the two real bugs
  were both **a clamp that was not where the value entered**. A trig off a box at
  velocity 0 became the default for new notes, so every note drawn afterwards was
  a note-off; the slider showed `1` the whole time, because `SliderClamping::Always`
  clamps what a widget *draws* and not what it was handed. **A control that clamps
  its display is not a control that clamps its value** — clamp at the setter.
- **Phase 10 — twenty-eight plants, three that failed nothing, and all three were
  the test's fault rather than the code's.** Two are lesson 4's shape. The third
  was a guard nothing constructed: `ConfirmArgs::one`'s `[only]` written
  `[only, ..]`, which would silently word a sixteen-track write as being about T1.
  No caller passes more than one track, so the case had to be **built by hand** —
  a `#[should_panic]` test — rather than declared untestable.
- **Phase 11 — forty-one plants, four survivors, all four the test's fault**, and
  two of them a class not named before: **a guard that cannot fire with the inputs
  anything constructs.** The three-octave snap search and `chord_pitches`' own
  four-note cap are dead arithmetic for every scale and voicing the app can build,
  and both were ported faithfully from the JS. The answer was not to delete them:
  it was to say so in the doc comment and pin each with a test that reaches it
  deliberately — an interval list no menu offers, a `max_notes` a caller could pass.

**Three answers to a plant that cannot fail**, in order of preference: construct
the case by hand; **delete the claim** (an `unwrap_or` no caller can reach — an
unreachable fallback asserts a value can be missing, which then has to be believed
everywhere); or say in the doc comment that no test can distinguish it.

**On running the pass:**

- **Script it.** Restore-patch-run-restore over one test target, in a loop, is
  what made forty-two plants affordable where five had been the norm.
- **Point each plant at the target that holds the test.** One "survivor" was the
  script running `--test safe_write` against a claim whose test lives in `--lib`.
- **Set `RUSTFLAGS="-C debuginfo=0"` and a scratch `CARGO_TARGET_DIR` before the
  first plant, not after the ninth.** It took a plant's cycle from three minutes
  to a few seconds — the first nine plants took an hour, the last thirty-two took
  twenty minutes.
- **Re-snapshot after implementing and before planting.** A snapshot taken before
  the feature was written cannot be restored from after planting without
  destroying the feature too.
- **A plant can hang a test rather than fail one.** Making a trackpad
  accumulator's threshold disagree with its subtraction turned a `while` into an
  oscillation, and the harness sat on it for ten minutes looking like a slow
  build. There is a 300-second timeout now that reports a hang as a catch. **Any
  `while` whose test and whose step are two different constants is this bug
  waiting for an edit.**

### 7. A function first, a button after

`safe_write_track`, `core::export`, `core::project`, `Stash::export` and
`copy_track` all landed complete with no caller, and each time the missing seam
above them was invisible from the layer above. When two of them got their buttons,
the seams turned out to be the whole of the work: a file dialog behind a trait, a
dirty flag wired to the one `edited` the frame already computes, and a close guard
that `App::on_exit` is too late to do.

**Phase 9 was the same lesson about a *field*.** `Note.velocity` was in the model,
carried through `core::export`, and had reached real hardware — with nothing in
the app able to set it, so every note this app ever sent went out at 100. **A
field with no control is as invisible from above as a function with no caller, and
harder to notice because the tests all pass a value in.**

**Phase 10 was the lesson inverted, and worth recording as the counter-case.**
`safe_write_tracks` was written *because* a button needed it — the mass send could
not obey `PLAN.md` §7 rule 7 with the singular one — so the function and its
caller landed together, and the seam (one backup per slot rather than per track)
was decided by the person who would have to live with the ring being full.

**And once more in the Generate panel, found by Neil looking at the old
browser app and asking what happened to two of its buttons (2026-08-20).**
`progressions::next_progression_for` was ported with the words *"The ↻ button"*
in its own doc comment, and `progression_note` with a caption to draw; both were
tested, and neither was called from anywhere. So the progression library was
reachable only by typing one of its entries out by hand, and the note each entry
carries was drawn nowhere at all. **A ported function that names the control it
is for, and has no caller, is a missing button — the doc comment is the report.**
The two buttons are now wired and pressed by tests.

**And a third time, the same shape as Phase 9's, found by someone asking for the
feature the field was already named after (2026-08-22).** `PianoRoll::zoom` was
`pub`, initialised to 1.0, and multiplied into the grid's cell size on every
frame since the roll shipped — and **nothing in the app, or in any test, ever
wrote it**. So the roll drew one size, `PLAN.md` §9 had an open screen check
about "the smallest zoom the roll draws" that no gesture could reach, and the
whole of the arithmetic that would make a zoom work was there and idle. Two
things generalise beyond lesson 7's usual form: **a field that is `pub` and never
written is the same finding as a function with no caller**, and it is *less*
visible, because the field is read every frame and so it looks alive from
anywhere the grid is built. And the tell was in the prose rather than the code —
a verification list asking someone to look at a state the app could not be put
into.

**And a fourth form on 2026-08-22, which is the least visible of the four and
the one this lesson had not seen before: a control that has a caller, and whose
caller does nothing.** A part row's ↻ in the Generate panel was wired, drawn,
hovered and tooltipped — and its handler bumped a variation counter and returned.
It wrote no music, changed nothing on screen, and said nothing, while the tooltip
promised the action in the present tense. Every previous sighting in this lesson
was a *seam* missing between two layers, which is at least a hole of a shape you
can look for; this one had the seam and an inert body behind it, so from the
layer above — and from any test asserting the counter moved — it read as finished.
**"Is it called?" is the wrong question one time in four. The question is what the
call does, and a tooltip written in the present tense is a claim the code has to
answer.** ↻ now generates that row and whatever answers it below, straight into
the session.

Still waiting: **`copy_track`** (needs a destination and somewhere to put its
`warnings`) and **the clipboard** — `place_clipboard`, `ClipNote` and
`clipboard_anchor` are complete, tested and called by nothing. The seam they want
was a caret the roll does not record; since 2026-08-19 it is **the playhead**,
which pasted notes land at, and that made a *movable* playhead the prerequisite
rather than the other way round. So this one is not waiting on a caller — it is
waiting on a `TransportCommand::Locate` that does not exist and on an answer for
what dragging a playhead means when every track has its own length.

### 8. A green suite says nothing about what can be seen

Lesson 1 is about glyphs. This is everything else: spacing, layout, theme,
overdraw, and whether a control that was *drawn* can actually be *used*.

- **Six pixels of spacing.** The session-wide `Auto-connect` checkbox sat flush
  under the DN2's `Takes our clock`, reading as a third checkbox of that box's own
  — a lie about what turning it off does. Nothing in the code was wrong and no
  test could have had an opinion. A separator fixed it. **Draw the thing and look
  at it.**
- **Four faults, one mechanism, 1,177 tests green.** The tool panel covered the
  piano roll entirely, slider value boxes never rendered, prose clipped instead of
  wrapping, and Generate's part cards walked further off the right edge the
  further down the list they sat. All four: **an egui widget that asks for more
  width than it is given *inflates the parent `Ui` in asking*,** so the next
  widget starts further right and the one after that further still. Three do this
  by default — `Separator` (greedy by design), a `DragValue` under `add_sized`
  (a *minimum*, not a cap), and `TextEdit` (default `desired_width` is
  `f32::INFINITY`). The fix that generalises is **allocate an exact rect, then
  clip a child `Ui` to it** (`ui::mod`'s `slider_row`). The tell for the
  accumulating variant is **damage that gets worse down the list** — that is
  always one element's overflow being read as the next one's budget, so measure
  the width once before the loop and hand the same number to every row.
- **A screenshot proves a control was *drawn*, not that it *works*.** A UI pass
  opened the scene popup, photographed it, and recorded its slot pickers as
  "confirmed present". They were present. They were also unusable — clicking one
  closed the popup it was drawn in — and the same fault made it impossible to give
  a Generate part a box. Both were found in the first sitting where someone tried
  to *use* the panels rather than read them. **So a verification note should say
  whether a control was looked at or driven**, because "present" and "usable"
  photograph identically. Both mechanisms are library invariants rather than faults
  in this repo's logic: `egui::Memory` holds **one** open popup per viewport, so a
  `ComboBox` inside a popup evicts its host, and
  `Style::interaction::selectable_labels` is **`true`**, so a plain label senses
  clicks and takes them from the row it sits in. Neither is written anywhere
  `cargo test` can read it. The fixes are `ui::working_popup` and
  `ui::install_style`, and one test **asserts the broken behaviour on a default
  `Context`** so the fix stays tied to the invariant rather than to a memory of a
  bug.
- **The app was never told what theme it was**, and this had been shipping the
  whole time. Run on a Mac in light mode, `install_style` set `selectable_labels`
  and nothing else, so egui followed the *OS* and every surface the app does not
  paint itself came up at `Visuals::light()`'s `panel_fill`, `#f8f8f8` — **15.3%
  of the window, measured by counting pixels rather than by eye.** The transport,
  rail, tracks and roll survived because each paints its own background; the
  casualty was the **Setup panel — the one open on launch, and the one that
  overwrites cards** — which went white under `TEXT_MUTED` labels. The fix is
  `ThemePreference::Dark`, the *preference* rather than the visuals, because the
  preference is what egui re-consults when the OS changes its mind. Three things
  generalise: **a hardcoded palette is a claim about the theme that nothing was
  enforcing** (40-odd fixed hexes and not one line saying "dark" — lesson 5's
  shape applied to a rule living in *no* place); **the panels that paint their own
  background hid the ones that don't**, so the damage was invisible in the theme
  it was developed in; and **the reading of a screenshot is not evidence, the
  pixels are** — the first read blamed the rail, which measured `#1a1e21` and was
  correct all along.
- **Three bugs found by sampling pixels, none of which failed a test at any
  point.** A velocity bar that rendered as flat colour: the bar is floored at one
  pixel so the quietest note still shows something, but `paint_notes` strokes the
  note with `StrokeKind::Middle`, which centres a 1px white line *on* `rect.max.y`
  — so the floor pixel was painted over by the very next call, and a note at
  velocity 3 sampled as uniform `#006EAE` across its whole interior. Every unit
  test passed **because they assert on the `Rect` the geometry function returns
  and never on a pixel.** Factoring geometry out so a test can reach it is right,
  and it buys nothing against a painter that overdraws it. And a hover box that
  never appeared: the dwell is 350ms of a still pointer, egui does not repaint a
  still pointer, so the frame that would have shown the box was never requested
  and the dwell never elapsed. **A timeout measured in frames needs the frames
  requested**, and every test passed because a test harness drives frames itself.

**On how the looking gets done.** Driving this app with `cliclick` works for
clicks and **does not work for hover**: `cliclick m:` warps the cursor, and a warp
generates no mouse-moved event, so egui never learns the pointer is there. Nothing
highlights, no tooltip opens, and the failure looks exactly like a broken feature.
Anything dwell- or hover-gated has to be checked by hand, or by a harness that
posts real move events. Related and cheaper: **do not try to drive the GUI while
someone is using the machine** — a click computed against a stale window position
can land in another application.

### 9. A `cfg` you cannot run is a claim nobody is checking

And two platforms can want opposite things from one function. `paced_send` splits
a dump into 4 KB chunks because **CoreMIDI** drops anything it cannot describe in
a `UInt16`. `midir`'s **WinMM** backend decides sysex-versus-short-message from
`message[0] == 0xF0` and refuses any other send over three bytes — so the very
thing that makes the write work on macOS breaks every write on Windows, on chunk 2
of 31. Neither platform is wrong and no single constant is right, which is why
`SEND_CHUNK` is `cfg`-conditional.

**The trap is what you do next.** The obvious test is
`#[cfg(target_os = "windows")]`, and it is worthless: it compiles away on every
machine anyone here develops on, so the rule it guards is checked by nobody until
a PC happens to run the suite. The fix is to **make the platform variable a
parameter rather than a constant read** — `paced_send` takes the chunk size, the
production caller passes `SEND_CHUNK`, and the tests pass both values on whatever
host they are on. One further test asserts that the constant *this* host compiled
with matches its own backend, so the two halves cannot drift apart. The plant
confirmed it: making `paced_send` ignore its argument fails the WinMM test on a
Mac.

The corollary, which cost nothing this time and will not always: **check whether
the port's own defence already covers the new platform.** WinMM hands long SysEx
back in 1 KB pieces and does not reassemble them — which would have shredded every
pattern read — except `midi::sysex_stream` already accumulates F0…F7 across
callbacks because *ALSA* does the same thing. That was written for a platform
nobody here runs either, and it is the only reason the read path needed no work.

---

## Rules that are not up for renegotiation

Full list in `PLAN.md` §7. The five that govern anything touching a box:

1. **Backup, minimal diff, firmware allowlist, verify after write, throwaway
   projects only** — as *one* function, so a caller cannot skip a step.
2. **Always re-fetch the target pattern immediately before encoding.** Never reuse
   anything captured earlier.
3. **A backup that cannot be stored is a write that does not happen.** The store
   is the only automatic copy, so its failure is rule 1 refusing rather than a log
   line.
4. **Nothing is written without a dialog** naming the slot, the track, the trigs
   being replaced and everything the write touches beyond them, with a button that
   says `Overwrite A01 T9` rather than OK.
5. **Hardware testing is never in the dev loop.** `cargo test --workspace` needs
   no system dependencies and no box.
