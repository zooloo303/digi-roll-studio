# Native plugin-host P1 evidence — 2026-09-15

**Verdict: native VST3 path works; P1 exit gate is not passed.** Both user ROMs
booted, floating editors worked, audio was rendered and delivered to CoreAudio,
and all six MM channel probes produced audio. MD part playback is partial and
both emulators together exceeded the callback budget. No shared DRS contracts
were changed. AU, DRS IPC, hardware alignment and packaged hosting remain untested.

## Reproduction and provenance

See [native README](../native/plugin-host/README.md) for pinned dependency/license
route, exact build and run commands, scenario schema and local-only storage rules.
Implementation began from `463afd4`, clean `codex/plugin-devices`. `master` and the
Syntakt worktree were inspected but not modified. Apple arm64, clang 21.0.0,
macOS 27.0 SDK, CMake 3.31.6. Initial JUCE build failed on a removed screenshot API;
the tracked, narrowly scoped compatibility patch allowed a successful Release build.

Actual initial configure command (downloaded source override):

```sh
local/plugin-host/tools/bin/cmake -S native/plugin-host -B local/plugin-host/build \
  -DCMAKE_BUILD_TYPE=Release \
  -DFETCHCONTENT_SOURCE_DIR_JUCE="$PWD/local/plugin-host/JUCE-4f43011b96eb0636104cb3e433894cda98243626"
local/plugin-host/tools/bin/cmake --build local/plugin-host/build -j 6
local/plugin-host/tools/bin/ctest --test-dir local/plugin-host/build --output-on-failure
```

`bootstrap.sh` also supports a fresh checksum-verified JUCE fetch without the
source override. The consolidated VST3 proof passed, including actual process
context checks and native-ID parameter events. Rust default and preview tests and
Clippy were also rerun; exact totals are in the plan's progress entry.

User ROM sizes: 8,388,608 bytes each. FNV-1a fingerprints match the allowlist in
inspected Gearmulator source `e35ef1423964eb8840d5430a3e92aa9a2711cda8`:
MD `33b7c1a9e29f43fd`, MM `e1c1b461b6d0f21b`. The tested **binary** is alpha.11,
not a build from that newer inspected source. No ROM bytes are tracked.

## Runs and raw local evidence

All artifacts below are under ignored `local/plugin-host/`. Each successful run
has its exact scenario, log, WAV, parameter report and opaque instance snapshots.
The tracked [summary JSON](plugin-host-p1-results.json) contains measurements only.

| Run | Configuration / observation |
| --- | --- |
| `probe-01` | Wrong vendor directory (`Gearmulator`); both wrappers loaded dummy devices, reported missing firmware and rendered silence. Explicitly **not** a successful boot. |
| `probe-02` | Correct `Gearmulator Preview` ROM directory; both plugins, 48 kHz/256, 30 seconds offline, 44 MIDI events, nonzero audio. |
| `live-01` | Both, 48 kHz/512, 60 rendered seconds, editors requested, explicit MacBook Pro Speakers. Callback budget failed. |
| `recall-editors` | Fresh instances from `live-01` snapshots, 48 kHz/512, 90 rendered seconds, speakers. Both booted editors inspected; MM part 6 selected and MM window closed, exposing MD. Original track-1 levels retained. |
| `state-muted` | Initial level-zero writes after loading snapshots were overwritten by boot. This is a **failed control test**, despite successful process exit. |
| `state-recalled` | Fresh process, original snapshots, 30 seconds offline. Original MD/MM track-1 levels and nonzero audio retained. |
| `parameter-zero` | Two native-ID level-zero events at frame 384000 (8 seconds), after boot, before notes. Both final level readbacks zero; MD track-1 window became silent, MM track-1 effectively silent. |
| `fresh-level-recall/{mutated,original}` | Separate fresh processes restore level-zero snapshots as zero, then original snapshots as 112/127 MD and 99/127 MM; assertions passed for both. |
| `md-panel` | MD alone, speakers, 48 kHz/512, 120 rendered seconds while stopped; no scheduled notes. Inspected global slot 1 and its trigger map. Zero measured callback overruns in this **idle, single-plugin** run. |
| `mm-parts`, `md-parts` | Each plugin separately, one expected part unmuted at a time, three-second slots; note on/off on candidate destination. `windows.json` records stereo peak/RMS per part. |

UI permission checks initially failed, then succeeded after the user enabled
Computer Use. Screens observed: MM `KIT:01 SUPERWAVES`, MD `KIT:01 TRX UW`.
MM part-6 selection changed the displayed synthesis page. MD global slot 1 showed
OS 1.63; map entries 1–13 were read from the panel. The bounded run ended before
entries 14–16 were inspected. Native editors were visibly drawn and interacted
with; close/reopen stress and focus/resize coverage are incomplete.

Firmware was selected by supplying the plugin's documented ROM search directory
through isolated symlinks. Its interactive file chooser was not exercised.

## Audio, parts, parameters and state

- Both render while the host is stopped. Probe audio before the first note was
  zero. This demonstrates externally triggered audio in stopped mode, **not**
  ownership of notes during host play (factory patterns may run on play).
- MM channels 1–6, note 60, each with only the intended part unmuted, produced
  window RMS approximately `0.00192, 0.00426, 0.00567, 0.00934, 0.01275, 0.00149`.
  This supports all-six-channel routing in this factory state. Velocity response,
  repeated notes, dense sequencing, negative controls and onset timing remain open.
- MD candidate map: `36,38,40,41,43,45,47,48,50,52,53,55,57,59,60,62`, channel 1.
  Solo probe: parts 1 and 10–16 have clear audio; parts 2–9 are at one 24-bit LSB
  or less. Panel map 1–13 agrees with candidate notes. Do not label all 16 parts
  compatible. Need to distinguish kit/machine/sample/output configuration from
  firmware or hosting behaviour with a deliberately known sounding kit.
- Enumerated **2513 MD / 2445 MM** wrapper parameters, including synthesized MIDI
  CC/program parameters, not just the 416/348 instrument controls. Native IDs are
  reported separately from list positions. MD track-1 level: index 24, ID 49978837;
  MM: index 56, ID 53672921. Initial normalized values were 112/127 and 99/127.
- Post-boot level zero succeeded. In `state-recalled` versus `parameter-zero`,
  MD's 10–10.5 s window RMS changed from 0.02457 to 0; MM's 19–19.5 s window
  changed from 0.002695 to approximately 1.4e-7. These are digital render values
  after 0.25 mix gain, not acoustic loudness measurements.
- `live-01` snapshots: MD 12,587,700 bytes; MM 4,197,242 bytes. Fresh instances
  restored the original levels and displayed factory kits. The explicit save/mutate/fresh-process level recall assertions also passed for both zero and original snapshots. This is partial recall
  evidence, **not** proof of arbitrary changed kits, user samples, UW RAM, or a
  bit-identical audio stream. Full changed-kit/user-sample musical recall gate
  remains open; the deterministic fixture does pass that lifecycle exactly.

## Performance limits — no latency/compatibility promise

| Run | Rendered duration | Sum of callback work | Max block | Blocks over budget |
| --- | ---: | ---: | ---: | ---: |
| Both offline, 48k/256 | 30 s | 35.822 s | 164.51 ms | 5429 / 5625 |
| Both speakers/editors, 48k/512 | 60 s | 71.820 s | 193.11 ms | 5528 / 5625 |
| Fresh recall, speakers/editors, 48k/512 | 90 s | 109.065 s | 99.22 ms | 8391 / 8438 |
| MD alone idle, speakers, 48k/512 | 120 s | 75.475 s | see JSON | 0 |

Callback work is measured wall time inside render; it is not total process CPU.
Over-budget counts are not an OS xrun counter. Later code adds device-reported
xruns, device output latency, startup and render wall time; older runs did not
capture those fields. Plugin-reported latency also changed with runtime setup
(289 samples in the initial offline probe; 32 in the first speaker run); neither
is a measured end-to-end latency. A process sample during recall showed rendering
in both emulator processors, and approximately 4.1 GiB process footprint at that
instant (peak 5.2 GiB). Do not extrapolate this one observation to steady state.

The helper currently processes instances serially. Aggregate emulator cost exceeded
one callback's time budget on this machine. Investigate workload/configuration and
parallel rendering before expanding DRS integration. No jitter, acoustic latency,
clean listening result, dense automation timing or 30-minute stability result has
been established. The full real-plugin 44.1/48 kHz × 128/256/512 matrix is still open;
only the deterministic fixture covered that matrix.

## Next gates

1. Known sounding MD kit and verified all-16 note/part mapping, including main/aux
   routing and sample dependencies; confirm map entries 14–16 on the panel.
2. Empty internal patterns/receive configuration; play/stop/continue without
   duplicate notes, and sustained realtime performance with **both** plugins.
3. Deliberately changed kit/sound and level state saved and restored into fresh
   instances; negative/missing firmware and corrupted-state behaviour.
4. Measure onset jitter, output latency, xruns, startup and CPU across the real
   matrix, with editors open/closed. Only then choose a supported configuration.
5. Retain P2–P7 as unopened integration milestones. No reason yet to change shared
   routing/model/UI contracts or package this helper for users.
