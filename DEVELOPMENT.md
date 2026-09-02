# digi-roll-studio — development notes

`PLAN.md` is the architecture and the rules. This is the other half: how the
thing was actually built, what the tests can and cannot say, and the lessons that
kept repeating. Source comments cite it as `DEVELOPMENT.md` and by lesson number;
those numbers are stable.

Everything below was distilled from eighteen build sessions. **Almost every
lesson here escaped a green test suite at least once**, most of them more than
once. That is the whole reason this file exists. The exceptions are the three that
a test suite could not have caught: §16, which needed the box's own screen, §17,
which was wrong prose about code the tests were passing over, and §19, which is
a box accepting a write and quietly substituting something else.

---

## Working on this

```sh
cargo build --release
cargo test -p digi_protocol --test all   # the dev loop: one crate, 345 tests, ~2s
cargo test --workspace                   # before a commit; 1,855 tests, ~7s, no hardware
cargo clippy --workspace --all-targets   # clean as of 2026-08-23; keep it that way
cargo run -p digi_roll_studio
```

**If `cargo test --workspace` is taking minutes, clean `target/` before you
change anything else.** On 2026-08-31 that command took **1425 seconds** to run
1671 tests that execute in under three. Almost none of it was the tests, and —
against the obvious guess — almost none of it was inherent link cost either.
`target/` had grown to **75 GiB across 1.1 million files**, because cargo never
collects the artifacts of renamed targets, changed profiles or deleted test
files, and on macOS the dev profile leaves each binary's debug info in thousands
of separate `.rcgu.o` files that are then never swept. Every cargo invocation
was stat-ing that pile. A single `cargo clean` took the same command from
**1425s to 18s with no source change at all**; a full cold rebuild of all 185
crates is **28 seconds**.

So: `cargo clean` is maintenance, not a last resort — reach for it when the loop
feels slow, and do not conclude anything about build times from a `target/` that
has not been swept. Everything below was measured on a clean one.

The protocol suites read `.syx` captures from `crates/protocol/tests/fixtures/`,
committed so the tests run anywhere.

**A new integration test goes in `crates/<crate>/tests/all/`, and gets a `mod`
line in that crate's `tests/all/main.rs`.** Cargo builds one executable per
`.rs` file placed *directly* under `tests/`, so the old layout — 31 files across
four crates — linked 38 executables, and the app crate's ten each linked the
whole `egui`/`eframe` tree. Collapsing them to one target per crate leaves every
file, name and test exactly where it was and, on a clean `target/`, halves the
workspace run: **18s to 10s**, 38 binaries to 11, all 1671 tests still passing.
Worth having, but note the size of it against the paragraph above — this is the
2x, the sweep was the 80x. Its real value is that it stops the *growth*: the
`mod` line is the only cost of adding a file, where a `.rs` dropped straight
into `tests/` puts that crate back to paying for another whole link.

Shared helpers live in `tests/all/common/`, declared once in `main.rs` and
reached as `use crate::common::…` from the sibling modules.

**Clippy is part of the loop as of 2026-08-23**, and it was installed late enough
to be worth saying what it is for here. It runs clean, which is the only state in
which it is worth running at all: forty warnings nobody triages is forty warnings
that hide the forty-first. The settled policy is in the root `Cargo.toml` —
three lints allowed workspace-wide with reasons, everything else at its default —
and every exception beyond those three is a per-site `#[allow]` carrying its
argument in a comment.

Two things that first run established, both worth keeping in mind before
"fixing" anything it reports:

- **It found one real defect**, `absurd_extreme_comparisons` in `midi::device`:
  `next_msg_id >= 0xffff` on a `u16`, which reads as a range guard and can only
  mean an overflow guard. The behaviour was right and the code said something
  else — and the same two lines were copied into a test that claimed to mirror
  them, which is lesson 5 again. Now one `next_msg_id` function that both call.
- **Several of its suggestions would change behaviour.** `.min().max()` on `f64`
  is not a worse spelling of `clamp`: the two disagree on `NaN`, and four sites
  here rely on the chain *absorbing* one — see
  `protocol::pattern::micro_steps_to_byte`, which is on the write path to a box.
  `PLAN.md` §7 rule 3 covers the rest: `protocol::sevenbit` and
  `protocol::pattern` refuse three cosmetic lints each, because a byte-for-byte
  port's shape is what makes it diffable against the JS when a capture disagrees.
  **A lint is an opinion, and this codebase has already argued the other side in
  writing.**

**Where expected values come from.** Every phase so far derived them from the JS
original ([digi-roll](https://github.com/zooloo303/digi-roll)) *before* writing
them into a Rust test — `node --input-type=module -e` against its `js/**` is the
cheapest hardware-verified oracle available. Three rules came out of using it:

- **Where there is no oracle, say so in the file.**
  `crates/engine/tests/all/scheduler.rs` has none — `js/midi.js` sequences one
  track of one box, so nothing there can derive polymeter or two-port output —
  and its header says so in the first paragraph. Same for
  `crates/app/tests/all/engine_link.rs`.
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
- **The dump-namespace probes** — `fetch_preset_pool`, `probe_dump_types`,
  `probe_dump_args`, `sweep_dump_indices`, `probe_sound_library`. **Read-only,
  structurally:** every request goes through `assert_request_opcode`, which
  admits only `0x60`–`0x6e`, the *request* half of the protocol. Storing is
  `0x5n` and there is no `0x5n` in any of them. `0x6f` (whole project) is never
  sent by the sweeps either — it is not a store, but it streams megabytes where
  they want one small answer per opcode. Between them they are how the dump
  namespace was mapped: which opcodes answer, at which indices, with what
  payload in the request, and what collection `0x6b` turned out to address.
- `cargo run -p digi_roll_studio --example probe_query` — **read-only.** Probes
  the `0x09` Query API, the third read mechanism this protocol has, for anything
  preset-shaped. `digi_protocol::query` implements request-build and reply-parse
  and nothing else; the API has no documented write counterpart to have built
  one for.
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
- `cargo run -p digi_roll_studio --example browse_drive_dn` — **read-only.** Lists
  the +Drive through the `0x53` file API. Every call goes through
  `assert_read_only_file_op`, the allowlist admitting List, Open, Read and Close
  and nothing else.
- `cargo run -p digi_roll_studio --example probe_drive` — **read-only.** Walks
  the +Drive tree with `0x10` DirList only. The mutating opcodes of *that*
  numbering — `0x11` DirCreate, `0x12` DirDelete, `0x20` FileDelete, `0x21`
  ItemRename, the whole `0x4n` family — are not implemented in
  `digi_protocol::drive` at all. This is the run that found the split: a DT2
  answers `0x10` and a DN2 does not implement it.
- `cargo run -p digi_roll_studio --example probe_drive_read` — **read-only, and
  guarded rather than merely intended.** Derived the argument layouts of `0x54`
  Open, `0x55` Read and `0x56` Close by asking a box, because the source
  document names all three and specifies the layout of none. Every request goes
  out through `drive_file_request`, so `assert_read_only_file_op` sees the
  opcode — which is the safety property here, not "this file contains no write
  opcode", since in this namespace `0x5C` **deletes**.
- `cargo run -p digi_roll_studio --example read_drive_preset` — **read-only**,
  same allowlist. Reads whole preset files off three boxes and prints a report.
  The parsers' unit tests would pass just as happily if the read loop asked for
  the wrong chunk or closed a reader the box thought was open; this is what
  sees that.
- `cargo run -p digi_roll_studio --example capture_drive_file` /
  `capture_drive_project` / `capture_drive_presets` — **read-only**, same
  allowlist. The project one takes `--port` and reads megabytes, so it does not
  fan out across the desk. The presets one writes the bytes it reads **to disk
  on this machine**, which is how the container layer got built at a desk with
  no box on it — the captures under `crates/protocol/tests/fixtures/drive/` came
  from it.
- `cargo run -p digi_roll_studio --example recover_drive_write -- --port "<box>"` —
  **read-only as shown.** Diagnoses a box left deaf by an interrupted write:
  asks `0x53` and `0x01` and reports which answers. Adding `--close` sends
  `0x59` WriteClose against candidate handles, which **can commit a short file**
  in whatever slot the abandoned WriteOpen named.
- `cargo run -p digi_roll_studio --example probe_drive_write -- --port "<box>"
  --into <path> --from <path>` — **writes a file to the +Drive.** The only thing
  in this repo that does: `drive_write_file` lives in `digi_midi` and **no path
  in `crates/app` reaches it**, so nothing a user can press writes a +Drive
  file. It refuses a target the listing reports occupied,
  so it cannot overwrite; `0x59` is the commit and every failure path returns
  before it; and it verifies by reading back. Guarded by `assert_write_file_op`,
  a **second and disjoint** allowlist admitting WriteOpen, Write and WriteClose
  — `0x5A` Move, `0x5B` Copy and `0x5C` **Delete** are not implemented anywhere
  in this workspace and nothing here can reach them.

  **A malformed write can take a box's whole SysEx API down** until it is
  power-cycled — see lesson 13. That is a property of the box, not of this
  example, but this is the file that can provoke it.
- **The `0x5b` sound stores** — `probe_sound_store` and `probe_mk1_store`.
  These **put a sound onto one track of the box's active kit**, which is a
  different and smaller thing than the entries above: the active kit is the
  box's working state, so nothing reaches the card unless somebody saves the
  kit afterwards. Both are **read-only until told otherwise** — without
  `--write` they print what would be sent — and `--write` requires `--box <name
  fragment>` matching exactly one box, so a store can never fan out across the
  desk.

  **Both restore the original themselves, after every outcome including the
  negative ones**, and both write the restore to disk *before* the first byte
  goes out, so a process that dies mid-probe leaves the restore reachable
  rather than in a dead process. `--hold` pauses before restoring, which is the
  only way to actually read the box's screen — without it the restore lands in
  under a second and nobody sees anything.

  Treat them as `safe_write_track --write` anyway: a throwaway project, and a
  backup you took yourself. `probe_sound_store` settled whether `0x5b` stores
  at all (it does, on both digis); `probe_mk1_store` asks whether it will take
  a Digitone mk1 sound, which is what a third of a DN2's library is.
- `python3 local/decode_mmon.py <capture.mmon>` — **reads a file, touches no
  port.** Decodes a MIDI Monitor capture into raw SysEx messages and reports
  framing rather than assuming it. `decode7(data, msb_first=)` takes the bit
  order as a parameter because the two Elektron generations disagree on it —
  lesson 14, and the reason that argument is not defaulted silently.

  Capturing what a *box sends us* needs no spy driver; the spy exists for
  watching what another **app** sends to a destination. Ticking only the spy
  source when the box is the sender captures nothing and looks like a box that
  did not answer.
- `python3 local/a4_pattern.py show|diff|build|verify` — **`show`, `diff` and
  `verify` read files and touch no port.** Reads an A4 gen-1 pattern dump into
  its trig and note lanes, diffs two of them with every changed byte named, and
  `build` edits one and emits a sendable `.syx`; `pool` prints the 128-lane
  p-lock pool. `build` proves its encoder on
  the source file first — the source must survive decode → encode unchanged —
  rather than trusting a round-trip test written against other fixtures.
- `cargo run -p digi_midi --example sysex_loopback` — **touches no hardware.**
  Creates a virtual MIDI destination, sends to it and reassembles what arrives,
  so both ends are this process. Sweeps sizes, or carries a named `.syx`, and
  takes `--chunk` to exercise the paced delivery path. Separates "the bytes
  never left intact" from "the box declined them" without a capture, a click or
  a power cycle — see lesson 15.
- `cargo run -p digi_roll_studio --example a4_pattern_send -- <file.syx>` —
  **rehearses; opens no port.** Re-validates the file's framing, checksum,
  length and payload from the bytes on disk and prints the trigs it would
  write. `--send` **transmits**, after typed consent, and overwrites a pattern
  slot on the box. The validation deliberately repeats what the Python builder
  already did: the thing that must hold is not "the builder was correct" but
  "these bytes are well-formed", and a file can change between the two.
- **The Analog Four's probes**, which were absent from this list until
  2026-09-01 — a gap worth naming, since this list is where someone looks
  *before* touching a box and eight A4 examples had grown up outside it.

  That entry then claimed the gap was closed and it was not: **twelve more
  examples were still missing**, added 2026-09-01 in the same pass that audited
  the docs. Two of them store. The lesson is the one below the list.

  Read-only, and none of them ever makes the box save:

  - `a4_plock_extension_check <file.syx>` — **opens no port.** Reads a pool out
    of a file on disk.
  - `cargo run -p digi_roll_studio --example a4_dump_probe` — sweeps the dump
    *request* namespace `0x60`–`0x6e`, every opcode through
    `assert_request_opcode`, asking `0x01` Device after each so a wedge names
    its culprit. This is the run that disproved "the A4 answers no dump
    request".
  - `a4_lane_probe`, `a4_param_probe`, `a4_scale_probe` — all three watch the
    **working pattern** (`0x6a`) while a hand turns knobs on the box. Nothing is
    saved, no slot is written, and `FUNC`+`NO` undoes a whole session. They
    measure the per-step lanes, the p-lock parameter ids, and the p-lock
    scalings respectively.

    `a4_scale_probe` is the newest and it is the one that asks a *human* for a
    number: it prints the id it expects, you turn the knob to each end-stop and
    type what the box shows, and it reports what the two points fit. **It
    refuses to fit anything with a slope other than 1**, and it names three
    kinds of disagreement rather than smoothing them — a lane that is not the
    id the label join predicted, a `bipolar` flag the appendix got wrong, and
    two lanes moving in one turn. It prints the `params.rs` line each parameter
    earns and edits nothing, so "the box agreed" and "somebody read the run"
    stay two events.
  - `a4_param_check` — **sends channel-voice messages**, so the box makes sound
    and its parameters move. It stores nothing.
  - `a4_sound_pool_probe` — **read-only.** Reads the project's 128 pool sounds
    (`0x63`) and the +Drive's bank A, and reports which sounds exist in both —
    which is how the version-5-to-version-6 conversion the preset load needs was
    measured off the box rather than derived. `--slots` limits the sweep.

  These write to the box, and are the reason the group is not simply "read-only":

  - `a4_trig_probe`, `a4_plock_order_probe`, `a4_plock_containment` — each
    **overwrites a pattern slot** to settle a question a dump could not answer.
    The last two were what closed the p-lock writer's open items. Treat them as
    `safe_write_track --write`: a throwaway slot, and a backup you took
    yourself.
  - `a4_test_sessions` — **writes project files on this machine**, not to a box.
  - `a4_kit_store_probe` — **overwrites the kit the box is playing**, one sound
    at a time, and it is the gentlest write in this group: the working kit is an
    edit buffer, so reloading the pattern on the box discards everything it did.
    Three stages of increasing consequence — read, store the box's own bytes
    back, then load a real +Drive preset and revert it — and it stops at the
    first stage that fails. `--read-only` stops before anything is sent and also
    prints the *stored* kit beside the working one, which is what the box's undo
    would bring back; `--restore` puts one slot back from that stored kit
    without walking to the box; `--diff <path>` compares what the box now holds
    against the file it was sent, which is the mode that found the init-sound
    substitution (lesson 19).
  - `a4_sound_convert_probe` — **overwrites the kit the box is playing**, three
    times, then puts the slot back byte for byte. Asks what the box does with a
    sound it is handed: it splices the box's *own* SYN1 into another slot, then
    two different +Drive presets into that slot, and compares the box's two
    answers **to each other**. Same edit-buffer caveat, same undo.

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

**This list has to be complete or it is worse than absent**, and it has now been
incomplete twice — eight A4 examples in one pass, twelve more in the next,
including two that store into a box's active kit. A reader who consults it and
finds nothing concludes the example is safe, which is the one wrong answer a
safety register can give. So the check is mechanical and belongs with adding an
example, not with auditing the docs:

```sh
# Every example, against every example named here. Empty output is the invariant.
comm -23 <(ls crates/*/examples/*.rs | xargs -n1 basename | sed 's/\.rs$//' | sort -u) \
         <(grep -o '`[a-z0-9_]*`\|example [a-z0-9_]*' DEVELOPMENT.md \
           | sed 's/^example //; s/`//g' | sort -u)
```

The failure is not that somebody forgot. It is that **an example is added for a
question about a box, and the question is the interesting part** — the file gets
a header arguing its own safety case, at length, and that header is mistaken for
the place the safety case lives. It is not; this list is, because this list is
the one a person reads before the cable goes in.

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

**The sharper form, 2026-08-31: the lie was *copied forward*, and it invented a
safety interlock.** `ui::a4`'s SEND tooltip told users the Analog Four must be in
SETTINGS > SYSEX DUMP > SYSEX RECEIVE. It must not — PLAN.md §9 had measured on
2026-08-30 that the box takes a 14 KB dump sitting at its ordinary menu, and a
round trip the next day confirmed it by never arming the box at all. The sentence
came out of `examples/a4_pattern_send`, which had carried it since before the
measurement.

Three things generalise, and only the first is lesson 3 as already written:

- **An example is code, and it goes stale in the direction of whatever it was
  true about first.** It is nonetheless read as documentation, because it is the
  only prose next to a working call. Nothing marks the difference. So a stale
  example does not sit quietly being wrong — it gets *propagated* into the next
  file by someone reasonably treating it as the reference, which is how a
  one-file error became a two-file one on the day the second file was written.
- **The two claims were not equally safe, and that is why this is worse than a
  typo.** "Put the box in receive mode" describes an interlock: a step that would
  catch a stray message. There is none — there is no arming step between a 14 KB
  SysEx and an overwritten pattern slot. The instruction did not merely
  misdescribe the box, it **described a safety net over the one place the path
  has none**. When correcting a stale caveat, check whether the false version was
  more reassuring than the true one; if it was, the true one belongs on the panel
  rather than in a tooltip — which is where it lived for the rest of that day.
  (The panel itself is gone since later on 2026-08-31: the A4 writes through the
  digi ceremony now, whose confirm dialog, automatic backup and read-back verify
  are real interlocks rather than described ones, so the sentence's job is done
  by machinery. PLAN.md §10's "The A4 joins the digi transfer path" is the
  hand-off.)
- **The finding was already written down, in the file being edited.** PLAN.md §9
  had the paragraph, two thousand lines from where the panel entry was being
  added. Searching the repo for the claim before writing it would have cost one
  `grep` and found it. The habit worth having is to grep for a *hardware fact*
  before restating it, the same way one greps for a function before writing a
  second copy.

**The fourth instance, 2026-09-01: the panel was not lying about our code, it
was lying about the *box* — and it was doing it by asking the wrong field.**
Setup's patch-read row said "Analog Four plays over MIDI but has no patch names
to read". Both halves were false. The box had moved patterns since 2026-08-31,
and its kit had carried the four synth tracks' sound names all along. The
sentence came out of `patch_read_blocker` asking `Device::can_sysex`, which
means "has a gen-2 `Spec`" and had already been the wrong question twice
elsewhere — `ui::presets` carries a paragraph about not asking it, and
`PatternRoute` exists because the same field had stopped answering "can this box
transfer a pattern".

The Presets panel's sibling sentence was subtler and worth the contrast. It read
"The Analog Four answers no dump request for a kit track's sound", which is
**strictly true** — the qualifier is doing real work. But it opens with the six
words that had been the headline wrong claim about this box, and a person reads
the head of a sentence before its tail. It was reworded to lead with the narrow
fact ("has no message that puts a sound on one of its tracks") and to list what
does work.

**And the narrow fact was still a refusal that outlived its evidence.** On
2026-09-01 that paragraph was deleted: the A4 loads presets, through its *kit*
rather than through a kit-track message (PLAN.md §10, "The A4 loads presets").
Both rewordings were the box being described by which messages had been mapped
of it, which is the failure mode this whole lesson is about — a narrower true
sentence is still a claim about a box, and this one was three days from being
false. What replaced it is a table row, `core::device::PresetLoad`: the panel now
asks a field that says what the route *is* rather than a field it hopes implies
what it is not.

- **A refusal keyed on the wrong field goes stale silently, where a hardcoded
  string at least looks like something someone wrote.** `!device.can_sysex()`
  reads like a capability check and was never revisited when the capability
  arrived, because nothing about it looked like a claim. `!device.pattern_route()
  .transfers()` is the same shape and asks the question the sentence is about.
- **Grep for the phrase, not the field.** The two wrong sentences were in
  different crates and shared no code. What they shared was six words, and one
  `grep "no dump request"` across the workspace found both plus four stale doc
  comments — including one that had a *right* decision propped up by a wrong
  fact ("no `Spec` **and no pattern dumps**").
- **A test can pin a lie.** `an_a4_browses_and_has_no_load_path` asserted the
  message contained `"no dump request"`, so the false claim had a green test
  holding it in place for a day. An assertion on message *wording* has to be an
  assertion about the claim, not the string: it now checks the box's actual
  limitation is named and that the retired claim is **absent**.

The same day produced the mirror image, and it belongs here: a panel that hid a
feature that *was* built. `ui::edit`'s P-LOCK LANES section gated itself on
`model.spec()` and printed "this box has no SysEx spec" on the A4 — so a track
carrying sixty-one named lanes off the box listed none of them, while the strip
under the roll drew all sixty-one correctly the whole time. **Lesson 3 in both
directions: the panel's sentence and the panel's gate are the same kind of claim,
and only one of them looks like prose.**

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
nobody here ran at the time — Linux joined the desk on 2026-09-02, and lesson 20
is what it found — and it is the only reason the read path needed no work.

### 10. A new feature's tests are where the *old* feature's bugs come out

Song mode (2026-08-22) needed no change to how a box moves onto a pattern — a row
names a scene, and `commit_scene` had been switching scenes since Phase 4. So the
walker's first test that switched between two patterns whose same-numbered track
carried a different SCALE was not testing `commit_scene` at all. It failed anyway,
and the bug was four months old: a cursor's deadline was `next_step ×
step_seconds`, which reads the *incoming* pattern's step length off the *outgoing*
pattern's step count. A 2x track switching onto a 1x one four steps in put the
incoming pattern's step 1 at eight steps — half a bar of silence, in pattern mode,
reachable by clicking the scene pill.

**Nothing in the scene tests could have caught it, because every one of them
switched between patterns whose tracks were at the same SCALE.** The fixtures were
built to test switching, so they varied the thing under test and held the rest
still — which is what a good fixture does, and exactly why the gap survived. Song
mode found it by switching scenes for a different reason: it does it constantly,
against whatever the rows happen to name.

The lesson is not "write more tests". It is that **a feature built on top of an
existing one is the first thing that ever uses that one at volume**, so the hour
after a new suite goes green is the cheapest hour there will ever be for finding
what the old suite was holding still. Both halves are now pinned: the song test
that found it, and a scene test in pattern mode that states it without a song
anywhere near it.

The corollary is about where the fix went. The tempting repair was to special-case
the SCALE change inside `commit_scene`. What it actually needed was a second field
— `TrackCursor::origin_at`, *when* this pattern started, beside `origin`'s *which
step* — because the counter was only ever half a position. A bug that wants a
special case usually wants a missing field.

**A second instance, 2026-08-29, and this one had been shipped for three days.**
Load-to-track (PLAN.md §10.6 step 6) needed a fake box whose tracks hold real
`0x6b` payloads, so the first test built one out of the committed captures — a
DN2 track holding `MONOLOW`. Every test in the new suite failed at once, and not
on the new code: `decode_sound_dump` could not decode a real DN2 sound. It
recovered a struct's size from `KNOWN_SOUND_SIZES`, which has never held 319 (a
DN2 v0 sound) or 299 (a DT2's), and a `0x6b` reply's length is the *region*
rather than the struct.

Nothing existing could have caught it. The `0x53` sound-dump tests all build a
struct that fills its payload exactly — which is what a good fixture does, and
exactly why the gap survived — and the +Drive work had already learned the same
lesson from the other direction three days earlier and fixed it *there*:
§10.2's "the struct is measured, not looked up" was applied to preset files and
never carried back to dumps. Two copies of one rule, one of them corrected. That
is lesson 5 as well, and the two lessons meeting is what made this cheap to find
and embarrassing to have.

**The corollary is about what it was breaking.** `decode_sound_dump` is not only
a reader — it is the guard inside `plan_track_sound_store`, the check that
refuses to send bytes it cannot prove are a sound. So a DN2 v0 preset would have
been refused at the port *with a message saying it was not a sound struct*, and
the message would have looked like the guard working. **A guard is only as
honest as its parser**, and a refusal is a claim that deserves the same
suspicion as an acceptance.

**And the same guard, later the same day, was found to be answering a second
question nobody had asked it.** A probe went out to settle whether a DN2 accepts
a Digitone mk1 sound under `0x5b`, and never reached the wire: the guard turned
it away, because its decoder knew one head magic. Its job is *are these bytes a
sound* — and an mk1 payload is emphatically a sound, with a head magic and a
foot magic and a decoder of its own that has been in `sound.rs` for days. What
it was actually enforcing was *is this a format this box's kit takes*, which is
policy, and policy that lives inside a validator is policy nobody can find, read
or change on purpose.

Untangling them was two lines and one comment, and it is what let the box be
asked at all. The answer, for the record, was that the DN2 ignores an mk1 store
— so the policy stands. But it now stands somewhere a caller can see it
(`drive::preset_load_payload`), with a reason attached, and the browser can put
that reason on a row instead of spending five round trips discovering it.
**A validator that recognises one case has quietly become a policy**, and the
tell is that widening it feels dangerous for reasons nobody can state.

---

### 11. A count of failures is not a diagnosis, and one will be guessed at

The Presets panel's first library scan on a DN2 reported **`Tagged 0 preset(s),
388 skipped in 2s`** and nothing else. `ScanReport` counted skips and discarded
every reason, so 388 of them said exactly as much as one would have.

What followed is the part worth keeping. The only way to learn anything was to
parse the on-disk index files with Python — 801 tagged of 1,189 occupied, missing
exactly 388, spread across three banks. From that, a tidy story: a pre-pass added
the same day made eight extra List round trips before any read, `drive_read_file`
is Open/Read/Close with no recovery, so a failed Open never Closes and every later
read fails. It explained "zero successes" perfectly, it was the only thing that
had changed, and **it was wrong.**

Adding `ScanReport::first_skip` took ten minutes and the next run said
`/soundbanks/B/205: no sound container magic in 407 bytes`. The read had
*succeeded* — 407 bytes is exactly the length of a good DN2 preset file — and the
**decode** had failed. Nothing was stuck and nothing cascaded. The disproof had
even been in the data all along: bank D indexed 1–100, skipped 101–228, then
indexed **229–256**, and a dead transfer session does not come back for the last
28. It was read as a cascade because a cascade was already in mind.

Three things this leaves:

- **An error that reports only a magnitude will be guessed at**, and the guess
  will be confident, because a plausible mechanism plus a correlation in time
  feels like evidence. The cost is not the wrong theory; it is that the wrong
  theory gets *acted* on — a working feature was removed on it.
- **The cheapest fix was the diagnostic one.** Carrying the first failure's own
  words cost a struct field, and it settled in one run what an afternoon of
  inference had got backwards. Build it before the investigation, not after.
- **`407` is the trap in miniature.** The number was in the message from the
  start and looked like a fault, when it was the one length that proved the read
  had worked. `DriveError::NoContainer` now carries the head bytes too: a size
  that is also the correct size describes nothing.

Same shape as lesson 8's "present and usable photograph identically", one layer
down — here it was *failed* and *failed for this reason*.

**And then the head bytes were the trap a third time.** They were added at
sixteen, to keep the message on one line, on the reasoning that sixteen show "the
36-byte head's opening — every good capture begins `ac11d303 02000500 …`". Both
halves of that are true and together they are the defect: every good file begins
that way, so every bad one does too. Sixteen bytes was exactly the prefix a DN2
file cannot vary, so the first run printed a good capture's opening byte for byte
and proved only that a file had arrived. Widened to 48 — past the header, into
where the container magic belongs — the next scan named the format outright:
`DN1S`, Digitone mk1 presets, 388 of them.

So the lesson has a sharper edge than "build the diagnostic first". **A
diagnostic has to be sized to the thing it distinguishes, not to the thing it
proves exists.** Ask what two files it must tell apart, then check it covers the
bytes where they differ — the test that pins this asserts a good capture and an
odd one produce *different* strings, because asserting "the string is long" would
pass with the window back inside the shared prefix.

**One more, and it is about the reader rather than the code.** The first attempt
at reading those 48 bytes took them off a *screenshot of the panel* and found
`444e3153` — `DN1S`, the right answer, by accident: it sat at a half-byte offset
in a transcription with an odd number of hex characters. The string was real and
the reading of where it sat was not. `examples/capture_drive_file.rs` exists so
that never has to be repeated: it takes exact paths and writes the bytes out. A
96-character hex string read by eye is not evidence, and reading it more
carefully is not the fix.

### 12. Witnesses that agree unanimously can be blind in the same place

`file_declared_size` read a +Drive file's payload length as a `u16be` at `+27`.
It is a `u32be` at `+25`. The doc comment on the constant said "confirmed on
three boxes against three different sizes — 1114, 364 and 366", and that was
true: all three agree, on real hardware, across three products.

They agree because **all three fit in sixteen bits**. The low half of a `u32be`
at 25 and a `u16be` at 27 are the same two bytes, so every preset any of these
boxes stores reads identically under both. Thirty-two committed captures could
not fail the test that pinned it, and the test looked thorough — it walked the
whole fixture directory and asserted a count.

The first file that could tell them apart was an **A4 project**: 2,061,057 bytes
declaring 2,061,014, where the old reading returns 29,398 — the bottom sixteen
bits, which is not a number that announces itself as wrong. It is a plausible
size for a file.

- **A fixture set is a sample, and a sample has a shape.** Every one of those 32
  was a preset, because presets were what the work needed at the time. Unanimity
  across a homogeneous sample measures the sample, not the claim. Ask what
  *dimension* the fixtures vary along — these varied by box and by struct
  version and not once by **order of magnitude**, which was the only axis that
  mattered.
- **The corroboration that finally settled it was structural, not empirical.**
  Under the new reading the header closes exactly: payload `u32be` at 25,
  trailer length `u16be` at 29, header ends at 31, nothing spare. Under the old
  one, bytes 25 and 26 were unexplained and nobody had noticed. A layout with
  unaccounted bytes in it is a standing invitation to this bug.
- **The regression test names the evidence it cannot contain.** The project is
  two megabytes of someone's music and is not committed, so the test transcribes
  its 31-byte header and says in its own body that the fixtures cannot fail it.
  A test whose weakness is documented inside it is worth more than one that
  quietly passes for the wrong reason.

### 13. When a wrong guess costs hardware state, stop guessing and go and look

Two days went into deriving the `0x58` +Drive Write body by putting candidate
layouts to an Analog Four. Six candidates: three earned a clean refusal
(`Invalid sequence number`) and three earned silence — and on this box **a body
it cannot parse takes down the whole SysEx API**, not just the file layer. It
stops answering `0x01` Device, while a DT2 and a DN2 on the same bus answer
normally throughout, and it comes back only on a power cycle. Four power cycles,
each one a person walking to the desk.

The yield was three true facts and no working write. What settled it was
installing a CoreMIDI spy driver and capturing **Elektron's own Transfer**
uploading one sound to the same box — minutes of work, and the answer complete
and exact.

The three details that made guessing hopeless are worth naming, because they
are what a sweep cannot reach: the body carries **four** `u32` fields in an
order no earlier opcode uses; `0x59` WriteClose's second field is a literal `1`
where symmetry with `0x56` Close says total length; and the checksum is a
zero-seeded CRC32 **that is then inverted**. Any one of those alone defeats a
search over the other two.

- **Price the probe, not just the answer.** A sweep is the right tool when a
  wrong guess costs a round trip. It is the wrong tool when a wrong guess costs
  a power cycle, because the budget is now someone's patience and it runs out
  long before the search space does.
- **"No reply" is not a data point of the same kind as "refused".** A refusal is
  the box parsing your bytes and disagreeing; silence is it failing to parse
  them at all. The sweep treated them as one and drew a rule from the mix —
  "three `u32` fields get answered" — that the very next run falsified. Sort
  outcomes by *what the box did*, not by pass and fail.
- **A reference implementation is evidence, and it is usually installed
  already.** `Transfer.app` had been on this machine the whole time. The
  question "who else already speaks this protocol, and can I watch them?" is
  worth asking on day one, not after the fourth power cycle.
- **Retrying is not free once you are writing.** The generic request path
  re-sent every unanswered message three times, which is correct for a read and
  wrong for a write chunk: a chunk the box took and answered slowly gets
  delivered twice more to a transfer that has moved past it. Every wedge had
  three identical writes behind it.

### 14. A decode that produces structure is not a decode that is correct

The A4's gen-1 SysEx packs seven-bit data **MSB-first** — the MSB byte's bit 6
carries the first payload byte. `sevenbit.rs`, ported from elk-herd for the
gen-2 boxes, runs bit 0 to byte 0. The A4 captures were decoded with the gen-2
order for four rounds of analysis on 2026-08-30.

**It never once looked wrong.** Region boundaries appeared, strides came out as
round numbers, single-trig edits produced small localised diffs, and a track
stride of 735 bytes was measured twice and agreed with itself. All of it was an
artifact. The real stride is 751 and every offset derived in those rounds was
wrong.

- **Plausibility is not a witness, because a near-miss decode is still mostly
  right.** Flipping one bit in eight leaves byte boundaries, zero runs and
  repeat periods largely intact — which is exactly the evidence a structural
  analysis feeds on. The error hides in the place you are looking.
- **A constant is the arbiter.** `BEEFBABA` reads at offset 0 of an A4 sound
  dump under one order and appears **nowhere in the file** under the other.
  That is a yes/no question with no room to interpret, and it settled in one
  command what four rounds of structure could not. Find the magic, the name,
  the known-size field — anything the format *declares* — and check it before
  measuring anything.
- **The check was available from the first minute and was not run**, because
  the decode was producing results and results feel like progress. The habit
  worth building is to spend the first command on something falsifiable rather
  than on the first interesting-looking histogram.
- ~~**Two generations of one manufacturer do not share a primitive.**~~
  **Wrong, and it was wrong when it was written — corrected 2026-08-31.** They
  do share this one. `sevenbit.rs` puts the first data byte's high bit in header
  bit 6, which *is* the gen-1 order; it and the A4 decoder are the same function
  on every input, including every ragged tail length. The order that produced
  four wrong rounds was a hand-written `msb_first=False`, believed to be what
  `sevenbit.rs` did and never once compared against it.

  Everything else in this lesson stands: the A4 is MSB-first, `BEEFBABA` is
  still the arbiter that proved it, and the four rounds still happened. Only the
  blame was misplaced — onto a file that had been right all along. See lesson 17,
  which is what this paragraph turned into.

### 15. A format is not a protocol — the rate is part of the contract

The A4's first written pattern was refused in silence. The bytes were provably
right: the message was byte-identical to one the box had itself emitted, checked
by reconstructing it from a *different* dump. The port was right. A CoreMIDI
loopback carried the same 14,843 bytes byte-exact. Everything checkable was
checked, and the box did nothing.

**It was delivered too fast.** DIN MIDI is 3,125 bytes a second, so this dump
takes 4.75 seconds on a cable — the rate a 2013 box was built to receive at.
USB delivers it in microseconds. Pacing the same frame at DIN rate worked
first time.

- **"Correct bytes" and "a message the device will accept" are different
  claims**, and the first one is the one that is easy to prove. Every check
  available was a check on *content*. None of them could see timing, so none of
  them was ever going to fail.
- **The oldest box tells you its rate.** A format reverse-engineered from a
  device of a given era carries that era's assumptions about how fast data
  arrives. That is not in the byte layout and no amount of staring at a dump
  will reveal it.
- **Do the arithmetic before the first attempt.** Baud rate over message size is
  one division, and it would have predicted this before a byte was sent. It was
  done afterwards, as a hypothesis, to explain a failure it should have
  prevented.

**And the process lesson, which is lesson 13 arriving again from the side.**
When the send failed, the response was to reason about causes and build a fix
for the most plausible one. That fix happened to be right, which is the least
useful way to be right. Neil's question — "can't I just spy the Transfer app?" —
was the better instinct, and it was the instinct this document already records
as the thing that ended two days of guessing in 2026-08-30's earlier session.
**Having written the lesson down is not the same as reaching for it.** The
capture also paid immediately in a way the reasoning did not: it proved Transfer
relays a raw `.syx` unchanged, which turned a debugging step into a second
sender.

### 16. A model fitted to a rich capture will be confidently wrong, and only the screen can say so

The A4's two trig bytes were modelled three times in two days. The first two
models were built by correlating fields inside A01 — a real, musical, 51-trig
pattern — and both were wrong. The third came from a cleared pattern, a
two-byte diff, and Neil looking at an unlit LED.

**Model one.** In A01, `byte1 & 1` was set on exactly the steps whose note lane
was not `0xff`: 51 of 51, across two tracks, no exceptions. PLAN concluded
`0xc0` was a trigless trig and `0xc1` a note trig. That is as clean a
correlation as this project has ever had.

**What refuted it** was a cleared A16 with one deliberate trigless trig on step
1. Two bytes changed in 12,974. Byte 0 bit 0 was **clear**, not set, and byte 1
was `0x02`, not `0xc0`.

**Model two**, written the same morning to absorb that: a trig is `b0 & 1 or
b1 & 2`. It fit all three known states and was still wrong, because a fourth
state existed that no capture could interpret — `(01,c0)`, byte 0 bit 0 set,
which the box displays as an **empty step**. A01 SYN4 holds fifteen of them.
Model two counted them and reported 19 trigs where the box shows 4.

**What settled it** was the front panel. Step 2 of SYN4 was not lit, checked
either side of a factory reset. The trig state is `byte1 & 0x03` alone; byte 0
bit 0 is residue from a note trig that used to be there.

- **A rich capture is the worst place to fit a model, and it feels like the
  best.** Fifty-one agreeing trigs reads as more evidence than two changed
  bytes. It is less. A musical pattern varies everything at once, so a
  correlation across it is consistent with many models; a cleared pattern
  varies only what you touched, so a diff across it is consistent with few.
  Evidence is not measured in how many rows agree with you.
- **Prefer the capture where you control the baseline**, and take the baseline
  minutes before the change rather than reusing yesterday's. Six single-variable
  captures in one afternoon settled more than eight rich ones had in two days —
  the pool, its header, its extension lane, its ordering, and the trig bytes.
- **A state that displays as nothing is invisible to every check on your side of
  the cable.** Every model above was consistent with every byte we had. What
  separated them was whether the box *acts* on a bit, and the only instrument
  for that is the screen. This is lesson 13's "go and look" for a case where
  nothing was even failing.
- **Residue is the failure mode of a format that marks rather than clears**, and
  it makes wrong models over-count rather than under-count. Both bad models
  reported trigs the box does not show. When a decode's counts run high, suspect
  a field that the device stops honouring without erasing.
- **The regression that catches this is a count checked against hardware, not a
  round trip.** Every model round-tripped perfectly. What exposed models one and
  two was "the box shows 4 trigs and the tool says 19", and — quietly — that
  `a4-pattern-A01-rmv-trig1` read 32 trigs both before and after the removal it
  is named for. **A fixture whose name states an outcome is an assertion**; check
  the decode against the name, because when they disagree it is usually not the
  name that is wrong.

**Closed on 2026-08-31 by an experiment rather than by another capture**, and the
shape of it is the lesson's other half. Three models had been settled by *reading*
what the box sent; what none of them could touch was whether the box reads its own
bytes the way it writes them — `(01,c0)` is a bit that is **set** and must be
ignored, and no dump can show a device ignoring something. So all four states were
authored onto one cleared track and handed back to the box, with the prediction
written down first. Steps 3, 5 and 12 lit and the other 61 stayed dark.

- **A model confirmed in one direction is confirmed in one direction.** Read and
  write are separate claims about a format, and this one had two days of evidence
  for the first and none for the second. The gap did not show up as a doubt
  anywhere, because everything that could be checked agreed.
- **Write the prediction down before you look, and do not compute it from the
  model under test.** `PROBE_STEPS` carries the expected LEDs as a hand-written
  column; `ProbeStep::state` carries what the reader thinks; a test asserts the
  two agree. Had the prediction been `TrigState::is_live`, the experiment would
  have measured nothing and looked rigorous doing it.
- **The controls are what make a surprise readable, and one of them is not
  optional.** A send that never arrives leaves the slot holding something unknown,
  so three of the four predicted-dark steps are also what total failure looks like.
  The one step carrying a shape hardware had already accepted is what separates
  "the model holds" from "nothing happened" — read it first.
- **The box answered a question that was not asked.** Steps 3 and 12 did not just
  light; the box showed them as *trigless* trigs. An experiment aimed at one bit
  confirmed the box's whole interpretation of the byte, because the instrument was
  a screen with more on it than a yes and a no. Point the cheapest instrument at
  the question and read everything it happens to say.

### 17. A claim about your own code, written from reading it, is a guess

The A4 gen-1 port was scoped, over two days and in two documents, around three
differences from gen-2. **Two of them did not exist**, and both were assertions
about code sitting in this repository:

- **"`sevenbit.rs` runs bit 0 to byte 0, so it needs the bit order as a
  parameter."** It runs bit 6 to byte 0. `head |= 1 << (6 - i)` is the line, it
  has not changed since the port, and it is the gen-1 order exactly.
- **"Gen-1 framing is `mfr product device type 01 01 slot` with an unidentified
  constant."** That is the gen-2 dump header, field for field:
  `product` is `family`, and the "unidentified `01 01`" is the `version` field
  `build_dump_message` has always written. `parse_sysex` reads an A4 pattern
  dump, checksum and count verified, with nothing added.

So the port's actual diff to existing code is **two doc comments and one
constant**. The bit-order parameter, and the `Generation` enum that was going to
be threaded through `plocks.rs`, were both solutions to problems that were never
there — and threading a generation through `plocks.rs` would have made a
hardware-verified write policy conditional for no reason at all.

- **The cheap check was never run, again.** Lesson 14 says to spend the first
  command on something falsifiable. Both of these were falsifiable in one
  command each: run the two decoders against the same bytes, and hand a capture
  to `parse_sysex`. Each took under a minute and each deleted a planned change.
  Lesson 14 aimed that habit at *device* formats; it applies to your own source
  at least as strongly, because a foreign format at least gets captured before
  anyone writes about it.
- **Prose about code decays in the direction of more complexity, not less.** Both
  wrong claims made the codebase sound *less* capable than it was, and each
  justified new machinery. Nobody re-reads a paragraph that argues for extra
  work; it reads as diligence.
- **The wrong claim got copied twice before it was checked.** It went into
  PLAN.md §10, then DEVELOPMENT.md lesson 14, then a doc comment in
  `examples/a4_pattern_send.rs`, gaining authority at each stop — and the source
  it described was three directories away the whole time. This is §5's "a rule
  that lives in three places will be forgotten in one of them", in the variant
  where the thing forgotten is whether it was ever true.
- **The mistake is cheap to find and expensive to leave.** It cost nothing to
  correct and it had already shaped two documents, one example's design, and the
  scope of a port. `a4_pattern::sevenbit_is_shared_across_generations` is now the
  test that fails if anyone re-derives the old claim from the old prose.

### 18. "Does the box require X?" is rarely the question the code needs answered

The A4's p-lock pool is written compacted and sorted by `(param_id, track)`.
That it *produces* that order was measured on 2026-08-31. Whether it *requires*
it blocked the pool writer for a day, on the reasonable grounds that a pool
written in another order is a guess delivered to hardware.

**The answer was no, and the encoder still has to emit that order.** Three
single-variable writes to A16 — keys swapped, a hole between used lanes, an
extension detached from its parent — and each time the box parsed everything,
lost no lock, and wrote back its own canonical form. It requires none of the
three properties. But *because* it normalises, a pool written any other way
comes back different from what was sent, and `a4_safe_write_tracks` verifies by
comparing the read-back byte for byte: 10 spurious diffs for the swapped pair,
132 for the hole. A correct write would have reported as a failed one.

So the obligation on the encoder is real, stronger than the original question
implied, and **derived from our own verify rather than from the box**. Had the
experiment come back "the box requires it", the encoder would have been
identical. The day was not wasted — a guess delivered to hardware is still a
guess — but the thing it bought was the *reason*, not the rule.

Two habits come out of this:

- **Ask what the code does differently under each answer before running the
  experiment.** Here both answers produced the same encoder, which was knowable
  in advance and would have reordered the work: the writer could have been built
  first and the experiment run to explain it rather than to unblock it. The same
  shape as `a4_plock_extension_check`, which said so explicitly up front — "both
  outcomes give the same encoder rule, which is worth knowing before the capture
  rather than after" — and then the next open item was framed the old way anyway.
- **The constraint may live on your side of the wire.** "The box tolerates it"
  and "we may therefore do it" are different claims, and a verify step, a
  round-trip test or a diff-minimisation promise can each impose a requirement
  the hardware does not.

**The throwaway variant was the one that paid.** Variant C — an `80 80`
extension detached from the lane it extends — was included only because the rig
made it nearly free; the encoder would never emit one. It settled that an
extension binds to the lane physically *before* it, which `read_all_plocks` had
always assumed and nothing had ever tested, because the box had never produced a
pool where the two were apart. A reader's untested assumption is invisible
precisely while the box keeps agreeing with it by accident, and the only way to
find it is to hand the box something it would not have written.

It also produced the fixture that caught a real edge in the encoder an hour
later: a pool holding an all-zero extension, which the round-trip test then
failed on. **An experiment that corrupts state on purpose leaves behind the one
input nobody would have thought to construct.**

### 19. A box that accepts a write and substitutes something else is the failure a verify has to be aimed at

The A4 takes a whole kit over `0x58` and stores it. Hand it one whose SYN4 holds
a **struct version 5** sound — which is what every +Drive preset file on that box
is — and it does not refuse the kit, log anything, or leave the slot alone. It
stores the kit and puts an **init sound named `SOUND 4`** on that track. The
user's sound is gone, the write "succeeded", and nothing on the wire said so.

Everything that could plausibly have been checked instead would have passed:

- the store went out, all 58 packets of it;
- the box answered the next `0x68` immediately, so it had not wedged;
- the kit that came back was 2,410 bytes and decoded;
- SYN1–SYN3, the kit name and the FX and CV tracks were byte-perfect;
- the *only* wrong byte range was the 350 that had been asked for.

What caught it is that `load_a4_preset_onto_track` re-reads the kit and compares
**the name the preset was supposed to have landed under**. That is the narrowest
possible check and it is the only one that could have failed. The rule is not
"verify after writing" — every write path here already did — it is that a verify
must assert *the thing that was asked for*, not that the write happened. "The
store went out" and "the box is still talking" are both about the transport, and
this failure lives entirely on the other side of it.

**Two readings fitted the evidence, and they were opposites**: the box converted
the sound and merely renamed it (audition works, cosmetic problem), or the box
refused the sound and re-initialised (no load path at all). Guessing either way
would have shipped something — a name mismatch tolerated, or a feature abandoned.
The experiment that separated them is worth keeping as a shape: **send two
different inputs and compare the box's two answers to each other.** A conversion
leaves two different slots; a substitution leaves the same slot twice. It was 0
of 350 bytes apart, and 0 is not a number a conversion can produce.

The fix then came from the box as well, which is the other half of the lesson.
Rather than derive a version-5-to-6 conversion from the format, the A4's own
sound pool was read: 128 version-6 sounds, 28 of them sharing a name with a
version-5 +Drive file — the same sound in both versions, off the same box, with
nobody's hands in it. The conversion is two bytes, and **one pair differs in
nothing else**, which is what makes it a rule rather than a correlation. A box
that has already done the conversion you need is a better oracle than the format
document you do not have.

### 20. A fixed-width panel is a claim about strings the OS supplies

Setup is `Panel::right("setup").exact_size(320.0)`, and had been since the v2
pass, on the evidence of it looking right on macOS every day for months. The
first launch on Linux had every heading in it missing its first two characters:
`TRANSFER` read as `ANSFER`, `BACKUPS` as `CKUPS`.

Nothing about the panel was wrong. The port picker inside it was a `ComboBox` in
a horizontal layout, which inherits `TextWrapMode::Extend`, and `Extend` lays a
closed picker out at the full width of the name it is showing — `.width()` is
only a minimum. The name is the OS's:

    CoreMIDI   Elektron Digitone II                                     fits
    ALSA       Elektron Digitone II:Elektron Digitone II MIDI 1 28:0     ~348px

28px of overflow, in a panel that cannot grow. Because `exact_size` clamps the
rect it *reports* to the parent from the right-hand edge, those 28px moved the
boundary the central panel was told it could draw up to — so the workspace,
which is added after, painted over the left edge of everything in Setup. The
symptom appeared two panels away from the widget that caused it.

**Two things to take from it.** The first is that any widget rendering a string
the operating system chose is a portability question, and the answer is
`.truncate()` plus a hover carrying the full text — on ALSA the truncated tail
is the `client:port` numbers that tell two identically-named sockets apart, so
losing it silently is not free.

The second is how it gets caught next time, which is lesson 8's problem again:
**a layout claim is testable without a screen.** Draw the strip into a `Ui`
whose `max_rect` is the width the panel actually gives it, hand it the real ALSA
string, and assert `ui.min_rect().width() <= 320.0`. That measures 347.8 before
the fix and fails, which is a better report than the screenshot was — the
screenshot showed clipped headings and pointed at the wrong file.

### 21. The machine that builds it has never been the machine that runs it

Lesson 20 came off a Linux desk. The two bugs *after* it came off Linux CI, on
the first run of the two new jobs — and neither could have been found on the
desk, because both are things a developer's machine already has and a bare one
does not.

**The Arch job could not build.** `alsa-sys`'s build script stopped at `Package
'alsa' not found`. The PKGBUILD declares `alsa-lib` in `depends`, which is
correct, but `build-pkg.sh` passes `--nodeps`, so makepkg installs nothing and
the container has to arrive with it. It built fine on the desk for the only
reason that matters here: a machine that has ever built this app has `alsa-lib`
on it already. **A build dependency is only honestly stated by a machine that
does not have it.**

**The Linux job could not enumerate.** `enumerating_ports_never_fails_without_hardware`
asserts that listing ports on a machine with no MIDI hardware returns an empty
list rather than an error. On CoreMIDI and WinMM that holds. On ALSA, midir
enumerates by opening `/dev/snd/seq` — a node belonging to the `snd-seq` kernel
module, which a runner has no reason to load — so `MidiInput::new` fails before
it can report anything.

The interesting half is that **the app was already right and the test was
already wrong.** `ui::ports` puts that error in the panel, and "MIDI would not
start" is a better report than "no boxes found": it sends you to the missing
module instead of to your USB cable. Swallowing it to make the test pass would
have made a broken MIDI stack indistinguishable from an unplugged box. So the
test grew the precondition it always had — it holds wherever the sequencer
exists, which is every machine that can run this app and not the one that
builds it.

Both cost one rehearsal and no tag, which is lesson 9 again — a path nobody has
run is a claim nobody is checking, and a CI job is a path: **`workflow_dispatch`
before the tag, every time.** A tag that fails takes its version number with it,
and `draft-release` needs all four jobs, so either of these alone would have
produced a release with no assets at all.

---

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
