# Native plugin-host P1 evidence — 2026-09-15

**Follow-up: all sixteen MD parts now pass; concurrent 48k/512 rendering removes
sustained two-plugin overload in the measured workload. Silent preparation also
removed early startup misses; 512 still had later xruns, while one 48k/1024 run
passed with zero misses/xruns. P1 remains open.** See the follow-up sections below.

**Original-run verdict (superseded for these two blockers): native VST3 path works; P1 exit gate is not passed.** Both user ROMs
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

## Original next gates (see follow-up revisions below)

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

## MD silence: wrong host-note map, not eight silent machines

The original probe used the physical Machinedrum's nonchromatic firmware map:
`36,38,40,41,43,45,47,48,50,52,53,55,57,59,60,62`. Gearmulator intercepts the
range **36–51** and triggers consecutive panel pads **1–16** instead. Notes outside
that intercepted range still take the firmware MIDI path, explaining why the old
probe happened to pass parts 10–16 and part 1, but failed parts 2–9.

For example, note 38 triggers pad 3, not pad 2. The old probe unmuted part 2 and
muted pad 3, yielding silence. Changing machines, output routing, firmware note-map
entries, or using direct MIDI CC instead of wrapper parameters did not fix that
mismatch. Clearing all mutes produced audio, but did not prove correct part routing.
A track-2-only note scan found note 37. A full synthesized-impulse scan confirmed
consecutive notes 36–51 for all sixteen parts (small following windows contained
filter tails, not additional destinations).

Source corroboration: inspected Gearmulator source commit
`e35ef1423964eb8840d5430a3e92aa9a2711cda8`,
[`Hardware::pumpScheduledMidi`, mdhardware.cpp](https://github.com/joelanders/gearmulator-md-mm/blob/e35ef1423964eb8840d5430a3e92aa9a2711cda8/source/elektron/md/mdLib/mdhardware.cpp#L1654),
intercepts notes 36–51, computes `padIndex = note - 36`, and injects panel pulses.
The tested binary remains alpha.11, not a build of that newer inspected source;
actual audio measurements establish the behavior in the binary. The physical
[OS 1.63 manual, Appendix B/C](https://www.elektron.se/wp-content/uploads/2024/09/machinedrum_manual_OS1.63.pdf)
explains the original firmware note map, CC controls, and test-machine SysEx.

The launch script and both MD probes now use the Gearmulator pad map. No physical
hardware routing or shared DRS device definitions were changed. Do not extrapolate
this plugin-specific mapping to a physical Machinedrum. The panel-trigger path also
means velocity, note-off, channel filtering and host-play interaction still require
their own tests; note-to-pad success does not certify those semantics.

`md-corrected-proof` restored the original factory kit in a fresh instance and
passed **16/16 positive windows and 16/16 level-zero controls** using wrapper
parameters. Each slot zeroed all other track levels, set the target to 0.8,
triggered its corrected note, then zeroed it and repeated that note. Positive
half-second RMS ranged from 0.001544 to 0.020403; every silent control was one
24-bit LSB (RMS 1.19209e-7). This removes the original silent-parts blocker.

## Concurrent-rendering follow-up — 2026-09-15

The host now supports `parallel: true`: persistent realtime-priority workers
process instances 2–8 while the callback processes instance 1. The callback joins
workers before mixing the same frame, so no additional audio-buffer delay is
introduced. Serial rendering remains available as a control (default false).
This is an experiment: synchronization can block, third-party code is uncontrolled,
and there is no audio-workgroup integration or crash isolation per instance.

At 48,000 Hz / 512 samples, the deadline is **10.6667 ms per callback**. The old
serial 60-second speaker run spent 71.8197 seconds in callbacks, averaging 12.768 ms,
and exceeded the budget in 5,528/5,625 blocks. Those are deadline estimates, not
the hardware driver's dropout count.

Follow-up runs used the same alpha.11 binaries and original state snapshots:

| Configuration | Median | p99 | Whole-run overruns | After 8 s | Device xruns |
| --- | ---: | ---: | ---: | ---: | ---: |
| Parallel offline, 30 s | 6.732 ms | 7.776 ms | 5 / 2813 | not separated | unavailable |
| Parallel speakers, editors closed, 120 s | 6.729 ms | 7.392 ms | 1 / 11250 | 0 / 10500 | 2 total; interval not captured |
| Parallel speakers, both editors open, repeated notes, 120 s | 6.958 ms | 7.718 ms | 1 / 11250 | 0 / 10500 | 1 total, 0 after warmup |

The last run delivered 218 note events throughout the run; both editors were
inspected, foreground switched to MD and its track 2 selected. Maximum callback
time after 8 seconds was 8.245 ms (77.3% of the deadline). The device-xrun interval
starts at frame 385024 (8.0213 s), the first UI timer poll after the 8-second mark.
Callbacks are counted exactly from the requested boundary; the xrun interval is
approximate and separately reported. Whole-run startup overruns/xruns remain visible.

**Sustained aggregate overload is resolved for this measured 48k/512 workload.**
Startup still produced an overrun/xrun; this is not a zero-dropout certification,
a 30-minute soak, a dense automation test, or qualification at other buffer sizes.
Device-reported output latency was 1346 samples; it is not measured acoustic latency.


Final corrected-map confirmation (`both-parallel-live-editors-corrected`): 120 s,
48k/512, both editors open, 218 MIDI events spanning all sixteen MD pads and six
MM parts. Median **6.721 ms**, p99 **7.457 ms**. After 8 s: **0/10500** callback
overbudgets, maximum **8.123 ms**; **0** device xruns after frame 386048 (8.0427 s).
Whole run: **2/11250** callback overbudgets and **2** device xruns, retaining startup
costs. This final run confirms the performance improvement using the corrected
MD routing, rather than just the original note schedule.

Raw follow-up artifacts are preserved under ignored
`local/plugin-host/p1-followup-2026-09-15/`, with each scenario, report, WAV, states,
log and window analysis. Scenarios retain original temporary output paths as
provenance; choose a new output directory when rerunning. Compact measurements
(including failed diagnostic variants) are under `followupRuns` in
`plugin-host-p1-results.json`. Native `ctest` passed; the proof covers twelve
serial/concurrent rate-buffer cases plus timing, state and invalid-input checks.

## Startup preparation follow-up — 2026-09-15

Cold-start control reproduced three deadline misses at render frames 24064,
25088 and 47616 (0.501, 0.523 and 0.992 seconds), plus four CoreAudio xruns.
This establishes an early-processing problem; it does not identify the exact
third-party boot/JIT operation responsible.

The host now optionally processes silent stopped-transport blocks before opening
CoreAudio, on a worker while the GUI message loop remains available. The same
persistent parallel workers are used for preparation and live processing. MIDI,
parameter and transport schedules are untouched until capture/device frame zero.
The Gearmulator launcher defaults to twelve realtime-paced seconds of preparation;
raw scenarios default to zero. Explicit `--warmup-seconds 0` retains the control.

All runs below used both alpha.11 plugins, both editors open, parallel rendering,
48 kHz and MacBook Pro Speakers; 512-sample blocks except the labeled 1024 run. Prepared runs each delivered
218 MIDI messages across the corrected MD pad map and all six MM parts.

| Run | Whole-run deadline misses | Device xruns | Maximum callback | Preparation wall time | Total startup |
| --- | ---: | ---: | ---: | ---: | ---: |
| No preparation, restored state (30 s) | 3/2813 | 4 | 16.631 ms | 0.000 s | 2.228 s |
| 8 s accelerated preparation, restored state (120 s) | 0/11250 | 0 | 9.727 ms | 5.198 s | 6.687 s |
| 8 s accelerated preparation, fresh boot (120 s) | 2/11250 | 2 | 42.700 ms | 5.407 s | 6.584 s |
| 12 s paced preparation, fresh boot (120 s) | 16/11250 | 16 | 14.862 ms | 12.001 s | 13.180 s |
| 12 s paced preparation, restored state (120 s) | 17/11250 | 14 | 15.098 ms | 12.001 s | 13.667 s |
| 12 s paced, fresh boot, 1024 samples (120 s) | 0/5625 | 0 | 18.623 ms | 12.012 s | 12.950 s |

The restored-state prepared run recorded 16 hypothetical deadline misses during
preparation (maximum 66.180 ms); these happened before any audio device was open.
They remain reported separately, rather than being discarded from an active audio
run. Live counters cover every callback from device start, including startup.
No counter is reset after device opening. Preparation advances plugin internal
state and adds launch time; it does not add a buffer of steady-state output delay.

**Paced preparation removes the observed early startup misses, but does not
establish dropout-free operation.** The paced fresh-boot run recorded 16 later
deadline misses from 86.869 to 105.653 seconds and 16 device xruns. Its first
86 seconds had no measured callback misses. A subsequent CPU snapshot showed the
Insta360 camera extension around 135% CPU and no recorded thermal warning. This
is possible competing load, not proof of the cause; other applications were
left running. The paced restored-state repeat also had clean startup but 17 later
misses and 14 device xruns, beginning at 31.285 seconds. Therefore the earlier
clean 512 run does not establish stable performance under the later session load.
This is not a readiness handshake, a guarantee for arbitrary states, a listening
certification, a 30-minute soak or qualification at other rates/buffer sizes.
P1 remains incomplete for its other gates, including transport/note ownership,
changed-kit/sample recall and broader performance characterization.

The accelerated eight-second preparation passed restored-state playback but
failed fresh boot: two 40–43 ms live callbacks at frames 167424 and 183296,
plus two device xruns. Thus rendered-time preparation alone is insufficient.
The final implementation paces each preparation block against a monotonic clock,
allowing wall-clock startup tasks to run, and uses twelve seconds. Because both
pacing and duration changed, these runs establish the combined setting, not
which individual timer or emulator operation caused the delayed work.

The final **48k/1024** fresh-boot comparison passed all 5,625 callbacks over
120 seconds with zero deadline misses and zero device xruns, both editors open
and 218 events. Maximum callback was 18.623 ms against a 21.333 ms deadline;
p99 was 14.203 ms. Reported output latency rose from 1,346 to 1,858 samples
(+512 samples / 10.667 ms); this is device-reported, not acoustic latency.
This is a provisional usable configuration under the measured session load,
not repeated qualification or a P1 exit pass. Use `--parallel --rate 48000 --block 1024 --warmup-seconds 12 --editors --device 'MacBook Pro Speakers'`.

Final native `ctest` passed (6.27 seconds); `git diff --check` was clean.
Native VST3 regression coverage includes silent preparation in serial/concurrent
mode, unchanged exact frame-zero and boundary impulses, parameter-before-note
ordering, transport context, and rejected negative/excessive preparation durations.
Raw scenarios, logs, WAVs, snapshots and reports are preserved under ignored
`local/plugin-host/p1-startup-2026-09-15/`; scenarios retain temporary output paths.
Compact metrics are in `startupPreparationRuns` in the tracked results JSON.
