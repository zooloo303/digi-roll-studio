# Plugin devices — implementation plan

**Status: P0 complete; P1 native helper implemented and tested, exit gates still open.** Updated 2026-09-15.

Goal: add Gearmulator Machinedrum and Monomachine AU/VST3 instruments as DRS
devices whose parts are mapped to DRS tracks, while hardware development and
releases continue independently. The supporting research and source references
are in [PLUGIN_DEVICES_FEASIBILITY.md](PLUGIN_DEVICES_FEASIBILITY.md).

## 1. Delivery shape

Use a **dedicated branch and Git worktree**. A worktree is a second working folder
for the same repository, checked out on a different branch. Both folders can be
built and edited without switching the branch underneath another task.

The first usable beta will support:

- One MD instance with 16 DRS tracks and one MM instance with six; multiple
  instances remain independently addressable.
- DRS note sequencing, audition, keyboard thru, mute/solo, scenes and songs.
- Floating plugin editors, stereo mixing, audio device/buffer settings and meters.
- Coherent host transport and sample-positioned events.
- Session save/reopen, offline devices when a plugin is missing, and useful errors
  when a helper or plugin fails.
- macOS VST3 first. AU is a subsequent explicit milestone in this plan.

Parameter lanes follow the first usable beta, including a defined distinction
between persistent automation and per-trig parameter locks. Windows VST3 follows
the macOS host proof. Existing hardware builds remain available on all current
platforms throughout.

MD/MM pattern fetch/write, embedding the emulator, a full effects rack, audio
recording, per-track audio stems, offline bounce and unrestricted generic-plugin
support are future scope. Plugin devices initially have live-only transfer
capabilities. Firmware and plugin installation are external prerequisites.

## 2. Branches, worktrees and integration

### Repository state observed

| Work | Branch | Folder / state |
| --- | --- | --- |
| Stable development | `master` | `/Users/neilward/Projects/digi-roll-studio`, at `fa30bee` |
| Active Syntakt beta | `codex/syntakt-write-beta` | `/private/tmp/digi-roll-syntakt-write-beta`, at `ce91211`, clean when inspected |
| Earlier Syntakt work | `codex/syntakt-readonly-beta` | Git reports its old worktree as prunable; no cleanup is part of this task |
| Plugin devices | **Proposed:** `codex/plugin-devices` | **Proposed:** `/Users/neilward/Projects/digi-roll-studio-plugin-devices` |

These are observations, not assumptions for a later session: inspect branch heads,
worktree state and local edits again before starting. No plugin branch or worktree
has been created as part of writing this plan.

### Starting implementation

1. Preserve/commit the reviewed planning documents through the normal repository
   workflow so the new checkout contains them. Untracked files do not travel into
   a new worktree automatically.
2. Create the plugin branch from the current local `master`, independently of the
   unmerged Syntakt branch. Proposed command from the main checkout:

   ```sh
   git worktree add ../digi-roll-studio-plugin-devices -b codex/plugin-devices master
   ```

3. Run plugin work from that folder. Its `target/`, native build directory and
   `dist/` stay local to the worktree. Do not share a mutable Cargo target or CMake
   build directory with Syntakt work. Use a durable project folder rather than a
   temporary directory for this longer-running feature.
4. Keep a progress/evidence section in this plan on the feature branch. Commit
   small coherent steps; publish a draft PR when implementation is reviewable.

### Keeping the streams independent

- Syntakt can finish, merge and ship from `master` without any plugin milestone.
- Plugin work periodically merges `master` **into the plugin branch**, especially
  after a hardware feature merges. This keeps a shared branch's history stable;
  avoid rebasing a branch other sessions are using.
- Do not repeatedly merge the two unfinished feature branches into one another.
- Extract a shared abstraction into a small prerequisite PR only if it is useful,
  behavior-preserving and independently tested. Syntakt need not adopt it before
  finishing. Otherwise keep it within the plugin branch until integration.
- Before each milestone, and immediately after Syntakt lands, test the combined
  tree. Use a disposable integration branch/worktree if Syntakt has not merged
  yet; do not use that combined experimental branch as the release source.
- Merge plugin functionality into `master` only after its acceptance gates pass,
  initially behind an opt-in build feature. A plugin milestone must not change
  the normal hardware release's dependencies or packaging unexpectedly.

### Where conflicts are actually likely

The current Syntakt branch changes the device registry, app shell, transfer UI,
workspace manifest/lockfile and release workflow. Most plugin code can be additive.

| Area | Separation rule |
| --- | --- |
| `crates/core/src/device.rs` | Preserve hardware profile entries and transfer routes. Add plugin backend metadata in a new module with a small integration diff. |
| `crates/core/src/model.rs`, session/project | Stage destination and persistence changes separately. Keep old hardware project fixtures and add Syntakt fixtures after integration. |
| `crates/app/src/main.rs` and device UI | Put plugin lifecycle/UI in new modules; keep shell wiring small and integrate against current `master`. |
| Hardware protocol, safe-write and transfer panels | Syntakt retains its implementation. Plugin hosting does not modify codecs or pretend to support FETCH/SEND. Recheck any new exhaustive capability matches after merges. |
| `Cargo.toml` / `Cargo.lock` | Preserve Syntakt's version changes; reconcile dependencies and regenerate the lockfile with Cargo. Avoid blanket conflict resolutions. |
| Packaging and CI | Add an independent plugin workflow/script first. Keep stable and Syntakt release jobs intact. |
| `PLAN.md` / `DEVELOPMENT.md` | Preserve stable section numbers; this file owns plugin detail, with only a short index link in the main plan. |

## 3. Isolation extends beyond Git

Complete this before running an experimental DRS build beside the normal app:

- Use a distinct display name, **Digi-Roll Studio — Plugin Preview**, bundle/app
  identity and artifact names. Do not replace the installed stable app.
- Add an explicit runtime profile/data-root mechanism. It must consistently
  cover settings, crash recovery, backups, caches, logs, recent files and the
  helper's IPC endpoint. This is new functionality, not an existing command flag.
- Keep stable paths as the default. The preview launcher selects its own root
  without changing `HOME` or other system directory variables.
- Audit the shared data-root function: today recovery uses
  `digi_protocol::backup_stash::app_data_dir()`. Isolating only egui's settings
  would leave recovery and backup files shared.
- Open copies of working sessions and save plugin experiments under their own
  filenames. Keep plugin configuration/firmware-selection behavior visible:
  third-party plugins may use their own global settings outside DRS's profile.
- Start the preview with hardware autoconnect off. Worktrees do not isolate MIDI
  ports: only one running DRS build should own a physical device during a test.
  Mixed hardware/plugin tests deliberately assign the needed ports.
- Build preview artifacts into the plugin worktree's `dist/`; retain commit,
  helper protocol version and plugin versions in diagnostic reports.

These measures protect ongoing work and make failures attributable to a particular
build. No changes to hardware write-confirmation rules are part of this feature.

## 4. Architecture boundaries

### Proposed modules

| Component | Responsibility |
| --- | --- |
| `crates/core/src/plugin_device.rs` | Serializable plugin identity, instance configuration, MD/MM profile and routing metadata; no loading or audio APIs |
| `crates/core/src/routing.rs` | Resolve a track to a hardware endpoint or plugin part; shared by playback, thru, audition and controls |
| `crates/plugin_host_client/` | Versioned IPC, lifecycle, bounded event transport, status/errors; Rust-only default build |
| `native/plugin-host/` | Separate C++/JUCE executable: plugin loading, floating editors, audio device, rendering and stereo mixing |
| `crates/app/src/plugins.rs` | App integration, instance lifecycle, dirty-state notifications and snapshots |
| `crates/app/src/ui/plugins.rs` | Plugin device creation, editor action, audio settings and status |
| `.github/workflows/plugin-host.yml` | Optional native builds and host integration checks, independent of normal release jobs |

Names are proposed, not existing files. Pin the native toolchain/dependencies after
the host spike; confirm the selected JUCE licensing route before distributing it.
An Apple-native AU implementation is the fallback if JUCE does not fit.

### Contracts to establish early

1. **A model and a connection are different things.** Hardware profiles keep their
   current registry/capability data. A plugin instance adds a backend and instrument
   profile; it does not become a fictitious operating-system MIDI port.
2. **A part is not an instance or an audio bus.** A track targets an instance plus
   part/MIDI channel/drum mapping. Most tracks share their device's instance.
3. **Parameter routing follows the destination.** Never send DT2/Syntakt parameter
   identifiers to MD because a track's note output was rerouted. Start with MD/MM
   device-owned track groups; explicit overrides must validate control mappings.
4. **One musical timeline, two delivery paths.** The existing hardware-only
   transport remains the default. With audio enabled, publish a sample-clock and
   monotonic-time correlation; both physical MIDI deadlines and plugin sample
   positions derive from it. Do not independently advance two musical schedulers.
5. **Keep deadlines.** Extend event dispatch upstream of `PortSink::send`, which
   currently receives bytes only when they are due. Send plugin events ahead of
   their blocks with timestamps; do not push deadline-less messages to offset zero.
6. **Separate control from rendering.** Loads, state snapshots, editor operations
   and IPC replies are asynchronous. Audio consumes bounded preallocated queues;
   the callback never waits for DRS, performs file I/O or loads a plugin.
7. **Lifecycle is explicit.** Instances transition through loading, awaiting
   firmware/boot, ready, restoring, unavailable or failed states. Handle timeouts
   without freezing DRS. Continue rendering while transport is stopped.
8. **Stale work cannot play.** Tag scheduled batches with a transport generation;
   stop, seek and rerouting invalidate obsolete events. Define overflow handling
   that recovers note state and reports failure rather than leaving stuck notes.
9. **Host transport has one owner.** Supply BPM/play position/playing state once;
   Gearmulator already derives clock internally. Verify an empty internal pattern
   or appropriate receive configuration for DRS-owned note sequencing.
10. **Plugin discovery is bounded.** Start by selecting an installed instrument.
    Later scanning uses timeouts and an isolated scanner/helper, with a cache of
    failed candidates. Never load candidates synchronously on the DRS UI thread.

## 5. Milestones and exit criteria

Each milestone is a reviewable commit series or PR-sized slice on the feature
branch. Dependencies below are intentional; the native host spike can run before
any changes to DRS's shared model.

### P0 — Establish the preview workspace

- [x] Create the worktree and record the base commit and baseline test results.
- [x] Add preview identity, isolated runtime paths and a preview launcher.
- [x] Add opt-in `plugin-host` build plumbing; the helper remains a separately
      built executable. Default workspace tests require no C++/JUCE/plugin setup.
- [x] Add a preview packaging entry point without altering stable release output.

**Exit:** stable and preview settings/recovery paths demonstrably differ; preview
does not auto-open hardware ports; default build and tests still work.

### P1 — Prove the host, before modifying shared DRS behavior

- [x] Pin a tested Gearmulator build and host dependency/toolchain versions.
- [x] Load MD and MM VST3 in the helper, open/close editors and select user firmware.
      Firmware selected through isolated ROM paths; chooser interaction untested.
- [ ] Render continuously into the chosen audio device; mix both main stereo buses.
- [ ] Send known MD trigger notes and all six MM channels; verify configured maps.
- [ ] Supply transport data and demonstrate one sequencer owning note generation.
- [x] Enumerate parameters and prove one level parameter changes through the host.
- [ ] Save, mutate and restore state in fresh instances, checking kit/sound recall.
- [ ] Record CPU, latency, startup behavior and audio stability at representative
      sample rates/buffer sizes. An external-MIDI experiment can diagnose mapping
      separately, but does not replace this integrated-host proof.

**Exit:** audible evidence for both instruments, working editors and fresh-instance
recall, with an acceptable measured configuration. If not, resolve the host choice
here before undertaking the model/UI work. Firmware absence blocks audible tests,
not work on the host protocol or test instrument.

### P2 — Add backend and routing contracts

- [ ] Add plugin identity/configuration and typed destinations with hardware defaults.
- [ ] Migrate the existing `out_port` behavior without losing legacy overrides.
- [ ] Preserve stable IDs independently of runtime handles, filenames and load order.
- [ ] Add MD/MM live-only profiles: drum map for MD, channel/part mapping for MM.
- [ ] Centralize playback/thru/audition/level resolution; reject incompatible lanes.
- [ ] Add host IPC handshake/version checks and a deterministic fake host.

**Exit:** old hardware sessions retain equivalent routing and MIDI events; two
instances of the same plugin remain distinct across serialization and reordering.
The combined Syntakt tree passes its model/capability tests.

### P3 — Integrate the audio timeline

- [ ] Dispatch scheduler events into timestamped helper queues and physical MIDI.
- [ ] Establish the audio/monotonic clock correlation and sample-offset conversion.
- [ ] Implement generation changes for play/stop/continue, scene changes and reroutes.
- [ ] Route live keyboard input with its own latency policy; handle notes on stop,
      disconnect, queue saturation and helper failure.
- [ ] Account for plugin-reported and device latency; add measurable alignment
      offsets for physical hardware. Keep the hardware-only timing path available.
- [ ] Handle audio-device loss, buffer/sample-rate changes and stalled helpers.

**Exit:** a deterministic test instrument receives scheduled events within one
sample of their intended positions, after declared compensation. No stale events
or stuck notes after transport changes. MD and MM sustain a documented 30-minute
playback at a selected supported configuration without observed audio dropouts.
Report physical-hardware alignment as measured results, not inferred sample accuracy.

### P4 — Make plugin devices usable in DRS

- [ ] Devices: add/select/remove instance, part mapping, status and Open Editor.
- [ ] Tracks: note sequencing, selection/audition, keyboard thru, mute/solo and names.
- [ ] Audio: output device, rate/buffer settings, instance gain, stereo meters.
- [ ] Keep instances alive through scenes and song transitions; do not reload whole
      emulator state for each scene. The initial beta uses one sound state per instance.
- [ ] Keep plugin transport semantics separate from hardware clock controls.
- [ ] Capability UI exposes live sequencing and disables unsupported pattern transfers.

**Exit:** a user adds MD/MM, maps parts, edits/plays a song and opens the plugin
editors entirely from the preview app. No duplicate triggers from internal patterns.

### P5 — Reliable sessions and first macOS VST3 beta

- [ ] Persist format/class/version, instance IDs, bus configuration, maps and state.
- [ ] Decide the container layout after measuring state sizes in P1. Keep state
      blobs out of repeated scene/undo copies; save manifest and blobs atomically.
- [ ] Add an explicit schema migration. Preserve ordinary hardware-only files in
      their supported format where possible; reject unsupported schemas clearly.
- [ ] New builds preserve plugin data as offline devices when hosting is disabled
      or an instrument is missing. Never discard blobs during load/save.
- [ ] Dirty-state/recovery accounts for plugin editor changes as well as DRS edits.
- [ ] Define the consistent save boundary between DRS edits and host snapshots;
      saving cannot silently report success after a host snapshot fails.
- [ ] Test firmware dependencies, corrupted state, relinking and user-sample recall,
      including the documented volatile MD RAM-sample limitation.
- [ ] Package the helper with the preview app, validate signatures, and test launch
      from the actual packaged artifact on a machine/configuration without a dev build.

**Exit:** close/reopen restores both instances and their routing/sounds. Missing or
failed plugins preserve work. Hardware/Syntakt sessions continue to load and play.
The first beta can be offered independently of the stable hardware release.

### P6 — Parameter lanes and lock semantics

- [ ] Enumerate stable wrapper parameter IDs, ranges, names and part association.
- [ ] Model plugin automation targets separately from hardware SysEx parameter IDs.
- [ ] Implement and label persistent automation first, with gesture/dirty feedback.
- [ ] Implement per-trig locks with explicit base-value restoration rules; cover
      manual edits, unlocked trigs, kit changes and simultaneous notes.
- [ ] Verify repeated values, zero, parameter-before-note ordering and dense lanes
      through real Gearmulator firmware, not just the fake host.

**Exit:** saved lanes recall the same controls, affect only their intended instance
and part, and exhibit the documented restoration behavior at musical boundaries.

### P7 — AU and Windows compatibility

- [ ] Run the same routing, editor, transport and save/reopen matrix for macOS AU.
- [ ] Keep AU/VST3 identity and state separate; changing format is not an automatic
      state conversion. Validate AU output buses instead of assuming VST3 behavior.
- [ ] Build/package/test Windows VST3, including native editor focus and audio setup.
- [ ] Decide Intel macOS support from available test coverage and artifact strategy.
- [ ] Keep Linux hardware CI green; add Linux plugin hosting only after establishing
      a tested Gearmulator build and host setup there.

**Exit:** macOS AU and VST3 plus Windows VST3 have evidence for the supported matrix;
publish unsupported combinations explicitly. Auxiliary outputs, audio inputs and
generic plugins can then be scoped as separate follow-ups.

## 6. Verification and merge gates

Use the existing consolidated Rust test layout described in `DEVELOPMENT.md`.
Expected event schedules come from independent examples/specifications, and native
integration uses a small deterministic instrument to distinguish host errors from
emulator behavior. Do not make firmware or installed commercial plugins a default
CI dependency.

| Layer | Checks |
| --- | --- |
| Normal Rust build | `cargo test --workspace`; `cargo clippy --workspace --all-targets`; normal release build |
| Opt-in Rust build | Same applicable checks with `--features plugin-host`, using a fake helper for ordinary tests |
| Native helper | Reproducible pinned build; deterministic instrument audio/MIDI/parameter/state tests; timeout/crash tests |
| Hardware regression | Old-session fixtures and equivalent event output; Syntakt fixture/write-safety regressions once integrated |
| Real plugins | MD/MM mapping, sound, editor lifecycle, transport, automation, save/reopen and sustained playback |
| Real mixed setup | One deliberately assigned hardware box alongside MD/MM; document clock/audio alignment and port ownership |
| Packaging | Launch installed preview, find helper, load plugin, isolate state, save/reopen, recover from missing helper/plugin |

At each shared-code milestone, merge the current `master` into the feature branch
and rerun checks relevant to the changed area. Before final integration, run the
full normal/opt-in matrix on the combined tree and verify the stable packaging path.
Hardware writes remain subject to the existing backup/confirmation/verification
workflow and are never part of automated default testing.

Fallback behavior must be tested: the default build runs without the native helper;
plugin failure does not corrupt the DRS session; stopping plugin support preserves
its saved devices as offline data. One helper initially contains all software
instruments, so a crash interrupts them together. Recovery must be explicit, with
no replay of obsolete queued notes.

## 7. Planning checkpoints

P0–P1 are the bounded feasibility investment. After their measurements, record the
host choice, dependency licensing route, supported configurations, session-state
size and revised effort estimate. P2–P5 form the first product increment; P6 and P7
are explicit follow-on milestones, not hidden requirements for Syntakt to ship.

The current estimate remains **several weeks for the first integrated beta**, with
additional work for automation semantics and platform qualification. Do not attach
a firm release date before P1 resolves the native hosting and timing unknowns.

### Progress / evidence

- 2026-09-15: feasibility report and implementation plan written; existing Syntakt
  branch/worktree and conflict areas inspected. No plugin implementation or runtime
  validation has been performed. P0–P7 remain open.

- 2026-09-15: P0 implemented on `codex/plugin-devices`, based on `fa30bee`, in
  `/Users/neilward/Projects/digi-roll-studio-plugin-devices`. Planning documents
  preserved under tracked `docs/`; original ignored local documents remain intact.
- Baseline: 2,135 workspace tests passed. With P0: 2,137 tests passed in each of
  default and `--features plugin-host` modes; Clippy passed for both modes.
  Normal release build also passed.
  Startup regression launches the actual executable without GUI/MIDI and checks
  the profile, backup/cache/recovery roots and hardware autoconnect policy.
- Preview feature selects a process-wide runtime profile before services start.
  macOS preview root is `~/Library/Application Support/digi-roll-studio-plugin-preview`;
  eframe settings use its `settings` child. Preset-index now shares the same root
  selector as recovery/backups. Stable identity and paths remain the default.
  No host logs, IPC endpoints or recent-file store exist yet; future ones must
  use this root. No claim is made about isolating third-party plugin preferences.
- Independent launcher and macOS bundle builder are in `packaging/plugin-preview/`.
  Preview bundle built and ad-hoc signature/plist verified; its packaged executable
  reported isolated paths and `hardware_autoconnect: false`. No GUI/audio test or
  installation was performed. Normal and preview builds need no C++/JUCE setup.
- User supplied `elektron_sps1-1uw_os1.63.bin` (MD) and
  `elektron_sfx6-60_os1.32b.bin` (MM), each 8,388,608 bytes, in the original
  checkout's ignored `local/`. Files were not copied into tracked content or the
  preview bundle. Boot compatibility has not been tested.
- P1 remains open: Gearmulator VST3 was not found in standard plugin locations;
  CMake was not found on PATH. Host dependency/license selection, native helper,
  plugin installation, firmware boot, audio/editor/state evidence and performance
  measurements have not been implemented or validated. P2–P7 remain open.


### 2026-09-15 — P1 native VST3 proof implemented (partial, not an exit pass)

- Added independent `native/plugin-host/` CMake project: JUCE VST3 loader,
  floating editors, continuous stopped rendering, explicit CoreAudio output,
  main stereo mixing, absolute-frame MIDI schedules, block-boundary native-ID
  parameter events, coherent stopped/play/seek/tempo context, parameter reports,
  WAV evidence and per-instance state snapshots. No Rust/shared UI/model changes.
- Pinned JUCE 7.0.12 commit `4f43011b96eb0636104cb3e433894cda98243626` under its
  GPLv3 option, including the bundled VST3 SDK's GPLv3 route. Added checksum-verified
  dependency scripts and a narrow, tracked macOS SDK 27 compatibility patch.
  CMake 3.31.6 and Apple clang 21.0.0 built the helper and fixture independently.
- Downloaded Gearmulator alpha.11 ARM64 PGO locally, checksum pinned. Corrected
  firmware search root to this release's `Gearmulator Preview` vendor folder.
  Both supplied ROMs match supported fingerprints and booted real editors.
  ROMs remain in the original checkout; local-only symlinks select them.
- Real tests: both plugins rendered nonzero audio and ran through MacBook Pro
  Speakers. Both editors inspected; MM part 6 selected, MM editor closed to show
  MD. No MIDI hardware ports touched. MM all-six-channel solo probes produced
  audio; MD parts 2–9 remained effectively silent in the tested kit. Panel map
  entries 1–13 were read and match candidate notes; 14–16 need panel confirmation.
- Post-boot level-zero controls worked via native parameter IDs. Startup level
  writes were overwritten by firmware boot; that failed attempt is documented.
  Fresh processes restored both level-zero and original snapshots, with asserted
  level readbacks; editors also displayed factory kits. Full changed
  kit/user-sample recall remains open. Snapshots are about 12.6 MB MD / 4.2 MB MM.
- Both-plugin 48k/512 callback work exceeded budget in nearly every block. MD alone
  idle at 48k/512 had no measured overruns over 120 rendered seconds. Neither is
  an end-to-end latency measurement or a clean-listening/stability certification.
  **No supported two-plugin realtime configuration established; P1 is not passed.**
- Exact commands and measured/failed runs:
  [P1 evidence](PLUGIN_HOST_P1_EVIDENCE.md),
  [build/run instructions](../native/plugin-host/README.md),
  [compact measurements](plugin-host-p1-results.json).
- Verification commands from this worktree:
  `local/plugin-host/tools/bin/cmake --build local/plugin-host/build -j 6`;
  `local/plugin-host/tools/bin/ctest --test-dir local/plugin-host/build --output-on-failure`;
  `cargo test --workspace`; `cargo clippy --workspace --all-targets`;
  `cargo test --workspace --features plugin-host`;
  `cargo clippy --workspace --all-targets --features plugin-host`.
  Native consolidated proof passed: real fixture VST3 loading at 44.1/48 kHz ×
  128/256/512, exact block-boundary MIDI/mix impulses while stopped, parameter zero
  and parameter-before-note ordering, native IDs, fresh-process state recall,
  transport process context, invalid-input and missing-plugin failures.
  Rust tests: 2,137 passed in each mode; Clippy passed in each mode.
- Remaining P1 gates: all MD parts with a known sounding kit; play-mode sequencing
  ownership; full musical state recall; acceptable two-plugin realtime processing;
  real rate/buffer/editor matrix, measured onset jitter/output latency and sustained
  stability. P2–P7 have not started. No helper/plugin/ROM added to packaging.
