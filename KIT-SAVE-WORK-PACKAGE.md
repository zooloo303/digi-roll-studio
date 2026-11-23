# WP-KIT-SAVE-01 — Save the current DN2 kit to +Drive

Status: proposed handoff; documentation only. Prepared 2026-11-14.
Suggested executor: Sol. No agent has been dispatched. No hardware work is
authorized by this document or by the request to write it.

## Outcome and fixed scope

Add **Save kit to device…** to Kit builder for **Digitone II only**. The saved
kit must appear under the chosen name in the hardware's kit library, load with
the intended sounds/settings, and survive a power cycle. Changing the working
buffer, sending a kit dump, or writing a file that merely lists is insufficient.

Save a freshly captured **complete current working kit**, including track
sounds, levels, MIDI-track configuration, FX and mixer data. Preserve unknown
fields. Do not reconstruct it from preset names, audition backups, sixteen
sound reads, or `Track::patch`: those do not describe the whole current kit.

Fixed decisions:

- Empty, writable destination slots only. No overwrite, delete, move or copy.
- DN2 and specifically verified firmware/build combinations only. No inferred
  support for DT2, A4, original Digitone or Syntakt.
- Retain the existing 16,384-byte single-chunk write limit. Refuse larger files.
- No multi-chunk implementation, sample transfer, project save, kit loader,
  cross-version conversion, or new offline kit editor in this package.
- Preserve all audition backups after saving. Saving a library entry does not
  prove that the current pattern/project was saved or change Revert semantics.
- Match existing worker, identity, firmware, busy-state and recovery conventions.
  Opening the panel and editing fields perform no hardware writes.

## Evidence and prerequisites

Read these sources before editing; do not treat historical notes as fresh tests.

| Evidence | What it establishes / does not establish |
|---|---|
| `PLAN.md` §10.5 | Historical DN2 kit size 10,795 bytes; DT2 roughly 17.8 KB. DN2 transport size fits our single-chunk limit. This does not establish a complete current-kit capture or serializer. |
| `crates/midi/src/device.rs`: `drive_write_file` | Existing WriteOpen → one Write → WriteClose, accepted-length and committed-length checks. Reuse; do not relax the limit. |
| `crates/protocol/src/drive.rs` | Write framing/checksum, file-header/trailer constants, listing parsers and location comparison. Trailer interpretation and kit-specific normalization require evidence. |
| `crates/midi/src/device.rs`: `fetch_pattern_kit` | Gen-2 stored-pattern route. Do **not** silently use this as the live working kit. The A4 working-kit method is not a DN2 route. |
| `crates/app/examples/probe_drive_write.rs` | Existing same-device file-copy probe. A successful copy is not proof that a newly assembled kit is valid. Do not run this example without explicit hardware authorization. |
| `crates/app/src/ui/presets.rs` | Current state owner, jobs, original-target completion handling and audition backups. Extend this owner. |

External reference, inspected 2026-11-14:
[SYXGRID kit save and writer](https://github.com/xrcstrecords/syxgrid-digitone-ii/blob/main/sysex-visualiser.html#L18750-L18879).
Relevant symbols: `driveReadKit`, `driveWriteKit`, `kitToVersionOf`,
`buildDn2prjPayload`, `driveWriteFile`, `driveVerifyFile`.
It implements kit-library writes using a device-read template and subsequent
readback/listing. Its comments record a kit that transferred but would not load
because of version words. Its chunk writer uses sequential chunk indices;
the documented multi-chunk validation includes an emulator, not our devices.
Use this as a protocol reference, not proof of compatibility. Record the exact
upstream commit used during implementation; the link above is a moving branch.
Do not copy third-party implementation text without resolving license compatibility.

### Mandatory evidence gate — complete before implementation

Produce `KIT-SAVE-EVIDENCE.md` with the following four rows, each backed by exact
local fixtures/capture paths, model, OS/build and field offsets where applicable:

1. **Live source:** a complete working-kit read route. A controlled unsaved
   sound edit **and** an unsaved FX/mixer edit appear in that read while the
   stored-pattern baseline remains unchanged. Identify how stable repeated
   reads and the target device are checked. A request number from another
   device or an upstream comment is not sufficient.
2. **File construction:** a hardware-authored DN2 kit file and the corresponding
   complete kit bytes. Account for header, payload, length, trailer/checksum,
   kit-name encoding/limit and version words. A no-edit decode/encode must be
   byte-identical. Do not assume the recorded 31 + 10,752 + 12 shape holds for
   every firmware version.
3. **Addressing:** paginated kit-bank listings establish raw directory indices,
   displayed bank/slot, occupancy and writability. Never infer zero/one-based
   numbering from the first returned row or choose a default path.
4. **Readback normalization:** establish exactly which location bytes the device
   rewrites, and their expected values for the destination. Local notes discuss
   bytes +23/+24; upstream comparison excludes 0x15–0x18. Resolve from fixtures;
   do not broaden the ignored range merely to make verification pass.

**If any row lacks evidence:** finish the evidence report, list the exact missing
capture and expected comparison, and return to the coordinating agent/user.
Do not invent a protocol, substitute a saved kit, enable Save, or conduct a
hardware experiment. Further reverse engineering is a separate assignment.
If the user later authorizes captures, name the device, test project and
operations first; reads and unsaved edits are distinct from library writes.

## Ordered implementation (only after the gate passes)

### 1. Pure codec and transaction contract

- Add `crates/protocol/src/kit_file.rs` (and module export) for the evidenced
  DN2 format. Inputs: complete working-kit bytes, same-device/same-build
  hardware-authored template, validated name and destination metadata.
- Validate all identified lengths, magic/version words and name constraints.
  Reject unsupported/compressed/version-mismatched forms unless the evidence
  explicitly covers them. No guessed conversion; tell the user what is unsupported.
- Preserve unrelated bytes, calculate required integrity fields, and implement
  destination-aware comparison allowing only the proven location changes.
- Add `crates/midi/src/kit_save.rs` with a mockable I/O trait and typed outcomes:
  refused before write, failed before commit, commit outcome uncertain,
  committed but verification failed, verified file/listing success.
- Report a lost commit reply as uncertain. Never automatically retry a write,
  claim rollback, or delete a potentially created file.

### 2. Exact save transaction

1. Capture device ID, port bindings, firmware/build and requested bank/raw slot
   independently of subsequent UI selection. Acquire the existing exclusive
   device-operation guard; identify the connected device and enforce the build gate.
2. Require transport stopped and ask the user to leave hardware controls/pattern
   unchanged during the operation. Refuse unstable repeated complete-kit reads.
   Do not claim atomic capture or protection against unobserved physical edits.
3. Read a valid same-device template during this connected session; invalidate
   it on reconnect/device/build change. If no kit exists, refuse and explain that
   the user must create one on the hardware first. Never borrow another device's header.
4. Build and validate the output. Preserve the captured bytes and intended file
   in a local operation record before writing; include identity, destination and
   source provenance. If recording fails, do not write.
5. Re-list the selected bank completely immediately before WriteOpen. Require
   the exact chosen slot to remain present, empty and writable. If it changed,
   refuse; never silently choose a different slot.
6. Call the existing `drive_write_file`. No cancellation once writing starts;
   honor its accepted-length and committed-length checks.
7. Independently read the file back, validate its format and exact payload using
   the narrow normalization from the gate; re-list and require the expected name
   and occupied slot. Preserve diagnostic bytes on any mismatch or uncertainty.
8. Update the original device's save result, never the currently selected device.
   Say which bank/slot was verified. Do not auto-load the kit, alter the active
   pattern, clear audition backups, or claim hardware loadability from readback alone.

### 3. UI integration

Extend `PresetsPanel` rather than creating another cache/job owner. Add a dialog
with kit name, bank, explicit empty-slot destination, source description and a
review summary naming the physical device. Invalid names must be explained,
not silently truncated. Do not offer occupied slots as selectable destinations.

The final **Save kit to device** button authorizes exactly the displayed
transaction. Disable it with a reason while offline, busy, unsupported, missing
evidence/template, or lacking a valid destination. Preserve shared busy guards
with Setup, scan, audition and revert. Poll completion after selection changes;
closing the surface must not discard an in-flight result. Errors identify the
original target and distinguish uncertain commit from definite refusal.

Until hardware acceptance below passes, keep the new save action unavailable
in the normal application; exercise the transaction through mocks and a guarded
acceptance harness. Do not advertise an unverified save feature as shipped.

## Required tests and acceptance

Automated tests use fixtures/fake I/O only:

- Byte-identical no-op codec round trip; changing only the kit name has only
  the evidenced differences; invalid/truncated/version-mismatched/oversize input refused.
- Whole-kit preservation, including FX, mixer, MIDI configuration and unknown bytes.
- Wrong device/build, absent template, stale/occupied/unwritable slot, incomplete
  listing, unstable source, offline and busy cases issue **zero writes**.
- Correct raw slot mapping and pagination; no automatic replacement destination.
- Short chunk acceptance prevents commit; failed/lost commit response is not
  reported as saved and causes no retry; failed readback/listing is not success.
- Only proven location changes pass verification; a one-byte unrelated change fails.
- Device/track selection changes and panel closure do not redirect the operation;
  save leaves audition backups intact.

Run `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`,
format checks for changed Rust, and `git diff --check`. Review native dialog
layout, disabled states, progress and failures using offline fixtures.

**Hardware acceptance requires a separate explicit user-authorized session.**
Use one backed-up DN2 test project and a user-chosen empty kit slot. Record
model, firmware/build, source changes, destination and payload hashes. Save a
kit with distinguishable track and FX/mixer settings; verify file/listing;
have the user load it on hardware and check those settings. After the user
safely saves unrelated work and power-cycles, confirm the entry remains and
still loads correctly. Re-list adjacent entries to check they were untouched.
Leave cleanup to the user; this package adds no delete operation.

Done means all automated checks plus that hardware result are recorded in
`DEVELOPMENT.md`, and README/PLAN accurately state the supported build and limits.
If hardware is unavailable, deliver tested code with the UI gate closed and
label acceptance pending; do not mark the whole package complete.

## Return format and later packages

Return: changed files, exact checks/results, evidence paths, supported build(s),
hardware acceptance status and remaining blockers. Preserve unrelated dirty
changes: this workspace already contains the uncommitted Kit builder work.

Later, separately assigned work: **DT2** kit format/sample-reference semantics
and multi-chunk validation; **A4** persistent kit-slot semantics. Neither is
part of DN2 completion. The upstream chunk-index implementation is a lead for
DT2 investigation, not authorization to remove our size guard.

Suggested handoff prompt:

> Execute WP-KIT-SAVE-01 in KIT-SAVE-WORK-PACKAGE.md. Follow the fixed scope and
> evidence gate exactly. If a gate is unsupported, return the specific evidence
> gap instead of choosing a protocol or expanding scope. Hardware actions need
> separate authorization. Preserve the existing uncommitted work. Report
> automated and hardware evidence separately; do not enable an unverified Save.
