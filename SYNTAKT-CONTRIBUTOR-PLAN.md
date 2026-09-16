# Syntakt follow-up contribution plan

Proposed follow-up to PR #7 for Tomokiiiiii, if interested. This is a menu of
bounded contributions, not an assignment or a promise to deliver every item.
Baseline: `v0.5.5` (`9caea90`), following the merge of PR #7. Start new PRs from
current `master`; agree the scope of each before beginning hardware work.

## First: reconcile the tested baseline (maintainer)

The beta.3 hardware report tested `ce91211`, on `codex/syntakt-write-beta`.
That is not the same source tree as the release tag. A direct comparison shows
remaining changes in shared write/sync/restore UI, core/protocol code and tests.
Before the next beta, audit and integrate the intended remaining fixes, including
backup selection, scene-slot notice and restore wording. Do not merge the beta
branch wholesale: its version and release workflow also differ. Record the exact
resulting commit and rerun the relevant checks. This is maintainer work, not a
request for the contributor to repeat completed fixes.

Correct stale module-level comments in `syntakt_pattern.rs` that still describe
it as read-only with no write path. Keep historical capture notes dated and intact.

## What exists today

- Pattern import and guarded note writes, with destination re-fetch, backup,
  confirmation, readback and verification. Writes send `0x50`, including the
  destination's kit; they do not author sounds.
- Note, velocity and length preserve default-following `FF` fields independently
  when appropriate; existing explicit locks remain explicit.
- The p-lock pool is located: 80 records of 130 bytes at offset 12,783, with
  `(parameter ID, track)` headers and 64 big-endian 16-bit values. `FFFF` means
  no value; free headers are `FF FF` with zero-filled value bytes.
- Insertion by parameter ID was observed. Ordering for equal IDs on different
  tracks, deletion, compaction and full-pool behavior still need explicit evidence.
- Only ID 29 has a measured name, FLTR RESO on the tested track. Existing value
  captures support display × 256 for those examples, not every parameter type.
- P-lock values do not enter or leave the session. The reader reports occupied
  steps, and import counts nonempty lanes that remain on the box. Parameter-only
  trigs are preserved on write but are not represented as editable trigs.
- The kit is opaque; B13's role remains unresolved. PER TRACK scale, DIN and
  Windows hardware behavior need separate coverage from the reported Mac USB,
  OS 1.40 build 0082, PER PATTERN runs.

## Suggested PR sequence

### 1. Map a small useful set of p-lock parameters

**Suggested first contribution:** capture evidence and fixture tests for FLTR
RESO plus a few useful filter/amp controls. No live write behavior needs to change.

For each control, record machine, track, firmware/build, displayed name/value,
raw parameter ID and raw value. Change one setting at a time; capture before,
after and return-to-baseline. Include minimum, midpoint, maximum and fractional
values where available; test negative and enumerated controls separately.
Compare digital and analog machines and the same parameter on two tracks before
claiming IDs are shared. Do not copy the A4 or DN2 parameter table as fact.

**Done when:** committed captures and a README establish each mapping and its
scope; tests decode the exact raw values; supported ranges/scales and unresolved
cases are explicit. Keep raw precision even when the screen rounds it. Leave
unknown IDs unnamed and noneditable. A narrow proven table is a complete PR.

### 2. Establish pool mutation and parameter-only trig rules

Use hardware-authored patterns to measure insertion before/between/after existing
records, equal IDs on multiple tracks, deletion of one lock versus the last lock,
empty records, and full-pool behavior. Include locks on steps 1 and 64 and locks
stored beyond the playing length. Test adding a parameter-only trig, converting
between note and parameter-only trigs, and clearing each kind.

**Done when:** fixtures establish allocation/order rules and the relationship
between trig state and pool contents. Tests distinguish genuine lock trigs from
residual trig bytes with no pool values. Document whether note deletion retains
locks, clears them, or turns the step into a lock trig; do not invent this policy
from the current note-only writer.

### 3. Implement a contained p-lock pool writer

Build on PRs 1–2 in the protocol layer, with offline tests first. Extend the raw
reader to retain values and enough record information to preserve unknown data.
Compose edits into a freshly fetched destination, with an explicit policy for
replacing supported lanes on selected tracks. Preserve unsupported IDs, other
tracks, out-of-scope steps, kit bytes and unrelated pattern fields. If hardware
requires record movement, assert untouched lane contents by identity as well as
checking the allowed byte-diff regions.

Capacity must be checked for the entire write before anything is sent. Unknown
record forms, unsupported mappings and insufficient capacity need a clear refusal
or an explicit loss policy; never silently truncate or evict existing lanes.
Cross-machine or cross-device copying must validate parameter meaning, not just
match a numeric ID. Keep existing firmware, consent and backup gates.

**Done when:** tests cover no-op preservation, add/edit/remove, multi-track writes,
unknown lanes, full-pool refusal, malformed input and untouched kit/track data.
Hardware write → independent readback → restore succeeds on a throwaway project,
with every payload difference accounted for and a byte-identical restore.

### 4. Carry supported automation through the app

Wire measured parameters through the session model, lane editor, import/export,
single-track write and whole-pattern sync. Represent parameter-only trigs without
creating notes that sound. Define copy/paste, clear and note-deletion behavior
explicitly. Preserve fine raw values through an unedited import/export cycle.

Do not present imported lanes as transferable until the write path supports them.
If a read-only view ships earlier, label it and retain the loss report. Report
unknown/unsupported lanes separately from empty records, and explain any skipped
steps before consent. Cross-device conversions must report unsupported controls.

**Done when:** fixture-backed tests exercise import → edit → export → safe write;
UI tests cover lane availability and loss reporting; hardware confirms multiple
parameters with distinct values, including a parameter-only trig and an existing
unknown lane. Existing note/default, length-skip and backup regressions still pass.

## Later, independently scoped work

| Work item | Evidence and completion criteria |
|---|---|
| PER TRACK scale | Capture unequal track lengths, scale multipliers and master settings; import the audible extent correctly and preserve hidden stored steps. Prove write/sync/restore behavior before advertising support. |
| Kit and machine identity, read-only first | Map machine identity and sound names with controlled captures; expose names only for proven fields. Keep sound/kit authoring as a separate proposal. Machine-aware p-lock naming may require this investigation earlier. |
| B13 / FX | Isolate FX sequencer edits and identify their block/pool effects. Keep the neutral B13 label until demonstrated; do not infer its role from the block count. |
| Remaining trig/settings fields | Sweep unmeasured condition menu entries, track defaults and other requested fields one at a time. Separate measured facts from inferred tables. |
| Windows and DIN transport | Find a tester with the relevant setup. Measure successful storage, pacing, independent readback and restoration; a passing Windows build alone does not establish hardware write support. |

## How to submit and test each slice

Suggested ownership: Tomokiiiiii can choose captures, protocol work or both;
maintainers handle shared UI integration, review and packaged betas. Agree any
session-model changes together before implementing them. One focused PR per slice
keeps hardware evidence and code review manageable.

Store captures under a dated `dumps/syntakt-YYYY-MM-DD/` folder. Include firmware,
OS, connection, machine/track, scale mode, slot, exact gestures, screen values,
expected changed regions and actual byte differences. Use distinct values across
steps and parameters so an ignored write cannot pass as a successful one.

Run `cargo test --workspace` and `cargo clippy --all-targets` and explain any
remaining warnings. Tests must work without attached hardware. Preserve the
beta.2 fixture regressions in `crates/core/tests/all/syntakt_write.rs` and the
protocol safe-write tests. Rehearse packaging with `workflow_dispatch` before
cutting a release tag, as required by DEVELOPMENT.md's release lessons.

For a write-enabled beta, identify the exact commit and package. On a throwaway
project: independently capture the destination, exercise cancel-before-send,
perform a deliberate edit, verify using an independent read, restore the created
backup and compare the full restored payload with the original. Include a
second untouched track and nonempty kit/pool data as preservation controls.
Document untested combinations rather than treating them as supported.

## Starting points in the repository

- `crates/protocol/src/syntakt_pattern.rs`: measured layout and current pool reader.
- `crates/protocol/src/safe_write.rs`: Syntakt composition and write safeguards.
- `crates/core/src/syntakt_transfer.rs`: model conversion and loss reports.
- `crates/midi/src/syntakt_transfer.rs`: framing, consent and transport.
- `crates/protocol/src/params.rs`, `crates/app/src/plocks.rs` and
  `crates/app/src/ui/plocklane.rs`: parameter metadata and lane presentation.
- `dumps/syntakt-2026-09-10/stride-H01/README.md`: original layout/pool evidence.
- `dumps/syntakt-2026-09-15/beta2-test/README.md`: default preservation regressions.
- `PLAN.md` §7 and `DEVELOPMENT.md`: invariants and hardware-testing lessons.

Suggested invitation, for the maintainer to send:

> If you fancy another Syntakt contribution, p-locks would be the most useful
> next step. We suggest starting with a small measured parameter map and capture
> tests, then tackling pool mutation and app support in separate PRs. Captures
> alone would be valuable if that is the part you prefer. We can handle the
> shared UI work and package betas for hardware checks. Nothing here needs to
> be taken on all at once.
