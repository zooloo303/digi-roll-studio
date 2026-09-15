# DRS native host — P1 proof

Independent macOS VST3 executable. Not a Cargo workspace member and not included
in either DRS package. AU is disabled. This is a bounded experiment, not the DRS
backend or a production realtime host. See [P1 evidence](../../docs/PLUGIN_HOST_P1_EVIDENCE.md).

## Build and dependency route

```sh
sh native/plugin-host/bootstrap.sh
sh native/plugin-host/fetch-gearmulator.sh
```

Run from the plugin worktree. Dependencies, Python environment, plugins and build
output go under ignored `local/plugin-host/`. CMake fetches JUCE by commit and
SHA-256; the Gearmulator downloader verifies its release archive checksum.
No system plugin installation or Homebrew changes are required.

Tested toolchain: Apple clang 21.0.0 (`clang-2100.3.34.2`), macOS 27.0 SDK,
arm64, CMake 3.31.6, Unix Makefiles, Release. The machine has Command Line Tools;
full Xcode was not required. Python 3.9/3.12 ran the scripts.

Hosting dependency: **JUCE 7.0.12**, commit
`4f43011b96eb0636104cb3e433894cda98243626`.
Archive SHA-256: `efaeeed2ca988d3ade7b21dd01dca5c79fd9a2fb5884ea6140b31a7a87e61053`.
Choose the **GPLv3 route** for the helper, JUCE modules, and the VST3 SDK bundled
in that exact JUCE tree. This avoids making a JUCE 8 AGPL/commercial decision for
DRS. The JUCE module headers explicitly offer GPLv3 as an alternative; the bundled
SDK's LICENSE.txt likewise offers GPLv3. Preserve corresponding source, license
notices, and our patch with any future binary distribution. No commercial licence
was purchased and no proprietary SDK agreement is relied on.

Primary license references:

- [JUCE 7.0.12 module licence](https://github.com/juce-framework/JUCE/blob/4f43011b96eb0636104cb3e433894cda98243626/modules/juce_audio_processors/juce_audio_processors.h)
- [Bundled VST3 SDK licence](https://github.com/juce-framework/JUCE/blob/4f43011b96eb0636104cb3e433894cda98243626/modules/juce_audio_processors/format_types/VST3_SDK/LICENSE.txt)
- [Gearmulator alpha.11](https://github.com/joelanders/gearmulator-md-mm/releases/tag/mdmm-v0.1.0-alpha.11)

`patch-juce.py` disables JUCE 7's unused native-window snapshot function because
SDK 27 rejects its removed `CGWindowListCreateImage` API. The patch is explicit,
idempotent, applied before building juceaide, and does not replace editor drawing.
Native window screenshots through JUCE are consequently unsupported. Maintaining
this older hosting dependency on new SDKs is a follow-up cost.

Gearmulator binary pin: `mdmm-v0.1.0-alpha.11`, macOS arm64 PGO archive SHA-256
`9bfd89b84918fe3842dc0cbd97b14563977b425d3b90078dc886c7cf49d61dfc`.
The wrappers report version **2.2.9**; retain the release pin as well as that version.
These downloaded plugins are local test inputs, not bundled DRS dependencies.

## Run the supplied firmware proof

```sh
host_bin="$PWD/local/plugin-host/build/drs-plugin-host_artefacts/Release/DRS Plugin Host Preview.app/Contents/MacOS/DRS Plugin Host Preview"
plugin_dir="$PWD/local/plugin-host/gearmulator/Gearmulator-Elektron-macOS-arm64-PGO"
python3 native/plugin-host/run-gearmulator.py "$host_bin" "$plugin_dir" \
  /Users/neilward/Projects/digi-roll-studio/local/elektron_sps1-1uw_os1.63.bin \
  /Users/neilward/Projects/digi-roll-studio/local/elektron_sfx6-60_os1.32b.bin \
  local/plugin-host/my-proof
```

The output directory must be new. ROMs are symlinked from the supplied paths into
`OUTPUT/gearmulator/Gearmulator Preview/{Machinedrum,Monomachine}/roms/`.
`GEARMULATOR_DATA_ROOT` confines this tested release's data to that output tree.
Do not copy ROMs, snapshots, plugin data, or these local directories into Git or
packages. State snapshots may contain large emulated-memory/sample payloads.
No `HOME` override is used. No MIDI hardware ports are enumerated or opened.

Add `--editors --device 'MacBook Pro Speakers' --seconds 60 --block 512` for a
bounded CoreAudio/editor run. Audio-device names are exact and explicitly selected;
the helper does not open the default device first. Without `--device`, it renders
offline on a worker while the GUI message loop remains active. Offline block costs
are **not** device xruns or an end-to-end latency measurement. Max duration is 120
rendered seconds; launcher wall timeout is 180 seconds (slow runs can time out).

Outputs: `scenario.json`, `host.log`, `mix.wav` (24-bit stereo), `report.json`,
and one opaque `.state` per instance. The mix gain is 0.25 per instance. All inputs
and auxiliary output buses are disabled; main stereo is required. No limiter or
latency compensation is implemented. Audition at a sensible speaker volume.

## Scenario contract (prototype only)

The executable accepts one absolute JSON path. Required keys: `output` (existing
absolute directory), `plugins` (1–8 objects with absolute VST3 `path`). Optional:

- `rate` (default 48000), `block` (256), `seconds` (10, maximum 120), `editors`.
- `device`: exact CoreAudio output name; omit for offline rendering.
- Plugin `stateIn`: absolute state path restored into a fresh instance before
  prepare. Plugin `parameters`: initial `{index,value}` pairs, normalized 0–1.
  Gearmulator boot can overwrite these; use post-boot events for the actual proof.
- `events`: `{sample,instance,channel,note,velocity}`. Absolute render-frame
  timestamp, zero-based instance, MIDI channel 1–16, note/velocity 0–127; velocity
  zero is note-off. Stable order, at most 4096 events, delivered at block offsets.
- `parameterEvents`: `{sample,instance,id,value}`. Native VST3 parameter ID string,
  normalized value, block-aligned render frame. At most 4096 events; controls are
  applied before notes in that block. Sample-accurate parameter automation is not
  claimed. Parameter IDs are scoped to plugin format/class, never globally unique.
- `transport`: `{sample,ppq,bpm,playing}`. Block-aligned change points. Default
  stopped, PPQ 0, 120 BPM. Musical seconds/samples and PPQ freeze while stopped;
  render frames always advance. This is an immutable test timeline, not live IPC.

Transport seeks do not cancel the preauthored render-frame schedule. Dynamic
queueing, generations, panic, device changes, crash recovery, untrusted scanning,
production persistence and DRS scheduler integration remain later work. The
harness preallocates its buffers/schedules, but cannot guarantee third-party
callback behaviour or hard realtime safety. Whole-run capture is in memory, then
written after callbacks stop; loads/state/file I/O stay outside the render callback.

## Tests

```sh
local/plugin-host/tools/bin/ctest --test-dir local/plugin-host/build --output-on-failure
python3 native/plugin-host/tests/gearmulator_parts.py "$host_bin" \
  local/plugin-host/my-proof local/plugin-host/my-mm-parts --model MM
# Same command with --model MD and a different new output directory.
```

One consolidated native runner loads the built fixture **through VST3**. It tests
six rate/block combinations, exact impulses across block boundaries, mixing,
stopped processing, explicit zero, native-ID parameter-before-note order, fresh
process state restore, transport process context, and rejected input/missing plugin.
Fixture tests need a macOS GUI session, but no audio device, ROM, or real plugin.
The optional part probe is diagnostic, and writes measurements rather than
asserting that every nonzero window proves complete compatibility.

For explicit fresh-process level recall after a baseline and post-boot zero run:

```sh
python3 native/plugin-host/tests/gearmulator_recall.py "$host_bin" \
  local/plugin-host/live-01 local/plugin-host/parameter-zero \
  local/plugin-host/new-level-recall
```

The original run directories named here are local evidence, not repository
fixtures. For a new post-boot zero run, copy the baseline scenario into a new
output directory, omit `device`, set `editors` false, point each `stateIn` at the
baseline snapshot, and add:

```json
"parameterEvents": [
  {"sample":384000,"instance":0,"id":"49978837","value":0},
  {"sample":384000,"instance":1,"id":"53672921","value":0}
]
```

Use 48 kHz/512 frames and 30 seconds; keep `GEARMULATOR_DATA_ROOT` pointed at the
baseline's isolated `gearmulator` folder so its firmware symlinks remain available.
Run `"$host_bin" /absolute/new-scenario.json`. Pass the resulting directory as the
mutated-state argument. The recall script asserts values against each saved run's
report after rendering fresh instances for twelve seconds.
