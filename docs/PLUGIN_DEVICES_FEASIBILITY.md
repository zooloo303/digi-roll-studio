# AU / VST3 devices in DRS: feasibility

Investigated 2026-09-15. **Recommendation: proceed with a small hosting prototype.**
Gearmulator MD/MM fit DRS's multi-track device concept particularly well. Integrated
hosting is a substantial new audio subsystem, however, rather than just two new
rows in the device table.

The staged implementation and independent branch/worktree strategy are in
[PLUGIN_DEVICES_PLAN.md](PLUGIN_DEVICES_PLAN.md).

## Evidence and limits

- DRS inspected at `fa30beec88e39e4069cf8e20ff57ac74280727e0` (v0.5.4).
- Gearmulator source inspected at `e35ef1423964eb8840d5430a3e92aa9a2711cda8`,
  branch `release/md-mm-alpha`. This is newer than the shipping alpha.11 source.
- [Alpha.11](https://github.com/joelanders/gearmulator-md-mm/releases/tag/mdmm-v0.1.0-alpha.11),
  released September 11, includes macOS AU, VST3 and standalone builds, with
  separate Apple Silicon/Intel packages; Windows has VST3 and standalone builds.
  Firmware is supplied by the user. These are alpha builds, and the macOS
  downloads are not notarized. Linux release availability was not established.
- This is source/documentation analysis. No plugin was installed, loaded into
  DRS, or audibly tested. No Gearmulator binaries were found in the standard
  system/user plugin folders inspected. CPU, audio latency and host compatibility
  remain measurements for the prototype. Upstream test descriptions are evidence
  of their coverage, not tests run here.

## The user-facing model

Add **Gearmulator MD** or **Gearmulator MM** in Devices, select the installed
plugin, open its editor to supply firmware/configure sounds, and play the tracks
from DRS. One plugin instance represents one whole machine:

| DRS device | DRS tracks | Destination inside the plugin |
| --- | --- | --- |
| Gearmulator MD #1 | 16 drum tracks | 16 machine parts, addressed through a configurable trigger-note map |
| Gearmulator MM #1 | 6 synth tracks | Six parts on their configured MIDI channels |
| Another MD or MM | Another track group | An independent plugin instance and saved state |
| Generic instrument, later | User-configured track group | Plugin instance, MIDI bus/channel, optional drum map |

Several DRS tracks must be able to target the **same** instance. Creating an
emulator per track would duplicate the whole machine, its CPU cost, kit, effects
and state. A separate instance is useful when the user deliberately wants another
machine.

The distinction between a musical part and an audio output matters: these plugins
do not expose one audio stem for every machine track. Their processor defines
stereo input and three stereo output buses: Main A/B, C/D and E/F. VST3 multi-output
is documented; AU bus negotiation still needs a real-host check. Start with the
main stereo pair and an instance volume/meter; add the auxiliary pairs later.
[Processor bus layout](https://github.com/joelanders/gearmulator-md-mm/blob/e35ef1423964eb8840d5430a3e92aa9a2711cda8/source/elektron/md/mdJucePlugin/mdPluginProcessor.cpp#L394).

## What DRS already provides

| Existing code | Reusable capability | Required extension |
| --- | --- | --- |
| `crates/core/src/device.rs` | Stable instance IDs, per-model track counts, live-only capability | Separate instrument profile from hardware/plugin connection; MD/MM profiles |
| `crates/core/src/model.rs` | Track notes, channel, output override, automation lanes | Typed destination and part/trigger routing; plugin parameter identities |
| `crates/core/src/session.rs` | Device groups, patterns, scenes, songs | Keep an instance alive across scene changes; define sound-state changes separately |
| `crates/engine/src/scheduler.rs` | Pure window-based timing, conditions, polymeter, note lifetime | Emit timestamped plugin events as well as hardware events |
| `crates/engine/src/event.rs` | Scheduled events already retain seconds from transport start | Plugin parameter events and typed endpoint IDs |
| `crates/engine/src/transport.rs` | Hardware MIDI timing, transport, panic | Audio-clock correlation and latency alignment |
| `crates/app/src/engine.rs` | Injectable sink factory, audition and keyboard thru | Resolve playback, thru, levels and lanes against the same destination |
| `crates/core/src/project.rs` | Validated JSON session persistence | Plugin IDs, format/version, state payloads and missing-plugin recovery |

DRS uses Rust/egui/eframe and `midir`. It has no audio device/render loop, plugin
loader, plugin editor host, or audio mixer. `TrackKind::Audio` currently refers to
an Elektron audio track; it does not mean DRS processes that track's audio.

`PortSink::send(port, bytes)` is a useful prototype seam, but it is too late for
accurate block scheduling: the transport waits for the deadline before invoking
it and omits the timestamp. Production plugin events should leave the scheduler
ahead of time and retain their intended sample positions.

## Three implementation routes

| Route | Feasibility / tradeoff |
| --- | --- |
| External Gearmulator standalone or existing plugin host, fed over virtual MIDI | Quickest musical proof. Existing DRS output overrides can send notes; create/configure the virtual MIDI port externally. Sounds and audio settings remain in the other app. Dedicated MD/MM profiles would make mapping usable. This proves sequencing, not integrated hosting. |
| DRS-managed plugin host using a small C++/JUCE helper | Recommended prototype architecture. Rust retains the sequencer/UI; helper owns plugin loading, editors, audio device and mixing. Start with VST3 for the cross-platform path; add macOS AU through the same abstraction. IPC timing and process lifecycle are real work. |
| Native AU host through Apple audio APIs | Viable if macOS-only is the deliberate scope. Requires Rust/Objective-C interop and native editor/window integration. VST3 and other platforms remain separate work. |
| Embed Gearmulator's emulator libraries directly | Technically plausible through C++ FFI, but DRS then owns firmware boot, resampling, state, panel UI and upstream integration. Does not establish general AU/VST hosting. Defer unless a tightly integrated emulator becomes the goal. |

[JUCE's format manager](https://docs.juce.com/master/classjuce_1_1AudioPluginFormatManager.html)
supports plugin discovery/instantiation abstractions, and its
[processor player](https://docs.juce.com/master/classjuce_1_1AudioProcessorPlayer.html)
connects a processor to audio/MIDI callbacks. These provide hosting building blocks;
DRS still needs its own timeline, routing, persistence and error handling.
[Apple AVAudioUnit](https://developer.apple.com/documentation/avfaudio/avaudiounit)
is the alternative native AU entry point.

Use a separate helper with floating plugin windows initially: it owns its native
UI event loop and contains plugin crashes outside DRS. One helper hosting all
instruments means a crash interrupts all software instruments; per-instance
isolation and shared-memory audio are a later, more expensive option. An in-process
C++ bridge is also possible, trading simpler communication for crash exposure and
JUCE/egui main-thread integration.

## Proposed architecture

```text
DRS patterns / scenes / keyboard input
                 |
         one musical timeline
                 |
      destination + timestamp + event
          /                       \
 hardware MIDI thread        DRS plugin-host helper
          |                  sample-offset event queues
     physical boxes          AU/VST3 instances + editors
                                   |
                             stereo mix -> audio device
```

Conceptually introduce `DeviceBackend::{Midi, Plugin}` and
`TrackDestination::{DeviceDefault, Midi, PluginPart}`. Keep stable plugin instance
IDs distinct from plugin class IDs and from process-local handles. A destination
describes instance, MIDI bus/channel and optional drum-trigger mapping; parameter
automation also identifies its target part and stable parameter ID.

Do not disguise a plugin as an OS port name. Do not resolve a rerouted track's
parameters using its original hardware model. Today `CuratedPLocks` and the level
sender resolve DT2/DN2/A4 tables from the owning device. A DT2 cutoff identifier
must never become an unrelated MD control merely because its output changed.

For the first product increment, give MD/MM their own device groups and live-only
profiles. Generic plugin profiles will need a runtime descriptor: the current
static model registry rejects unknown model keys and enforces fixed track counts.

## Compatibility details that determine success

### 1. Correct part routing

Monomachine parameter channels are base channel plus part index. Machinedrum
parameters use four consecutive channels, with four parts per channel and
different controller ranges for each part. The source includes explicit maps.
[MD/MM parameter mapping](https://github.com/joelanders/gearmulator-md-mm/blob/e35ef1423964eb8840d5430a3e92aa9a2711cda8/source/elektron/md/mdLib/mdautomation.cpp).

Machinedrum triggers use a customizable note map. The default begins with notes
36, 38, 40, 41 for parts 1–4; it is not sixteen chromatically consecutive notes.
Some other notes can select patterns or control transport, so the drum profile
must constrain/translate outgoing pitches. A piano-roll pitch cannot automatically
mean pitched playback of the selected drum; that needs a separate pitch-parameter
mapping. Confirm the loaded global settings rather than assuming defaults.
[Elektron manual, MIDI map and Appendix B](https://www.elektron.se/wp-content/uploads/2024/09/machinedrum_manual_OS1.63.pdf).

### 2. Decide who sequences

Recommended first mode: **DRS sequences notes; Gearmulator supplies sounds.** Use a
verified empty internal pattern or verified receive settings that prevent internal
pattern triggers. Muting the instrument is not an adequate substitute because it
may also silence externally triggered notes.

An optional later mode can let the emulated machine sequence its own patterns,
with DRS sending transport/pattern selections. That mode needs a clear ownership
rule to prevent double triggering.

Gearmulator's shared processor reads host BPM, PPQ position and playing state;
`MidiClock` converts these to clock/start/stop and seek-related messages internally.
Provide coherent playhead data and do not also feed DRS's ordinary hardware clock
stream to the same instance. Raw MIDI clock delivery is not a portable substitute
for host transport in VST3.
[Clock implementation](https://github.com/joelanders/gearmulator-md-mm/blob/e35ef1423964eb8840d5430a3e92aa9a2711cda8/source/synthLib/midiClock.cpp),
[VST3 MIDI model](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical%2BDocumentation/About%2BMIDI/Index.html).

### 3. Timing and continuous processing

Convert scheduled times into offsets within each audio block, using one transport
epoch correlated to the audio device's sample clock. Keep hardware MIDI sends on
their own thread, aligned to the same timeline. Account for audio device buffering,
plugin-reported latency, resampling and physical-box latency; offer measured track
or device offsets where needed. A 128-frame block at 48 kHz is 2.67 ms, not an
end-to-end latency claim.

Use bounded, preallocated queues and avoid IPC round trips, disk I/O, plugin loads,
state restoration or GUI calls in the audio callback. Preserve event ordering,
including parameter changes before their notes. Handle lookahead invalidation on
stop, tempo change, routing changes and scene transitions. The existing 50 ms
musical lookahead is a useful starting point, not 50 ms of required audio latency.

Keep processing while transport is stopped: firmware boot, tails and SysEx delivery
need the emulation to advance. Upstream explicitly documents stalled transfers
when the host stops processing.
[SysEx workflow](https://github.com/joelanders/gearmulator-md-mm/blob/e35ef1423964eb8840d5430a3e92aa9a2711cda8/doc/elektron_md_mm_sysex.md).

### 4. Automation and parameter locks

The source pins 416 MD parameters (26 per part) and 348 MM parameters (58 per part),
including level and mute. Parameter IDs are derived from page/part/index, with
tests guarding their contract. Enumerate wrapper-exposed IDs/ranges and retain
format identity; do not persist only parameter list positions or assume AU and
VST3 IDs/state are interchangeable.
[Parameter contract tests](https://github.com/joelanders/gearmulator-md-mm/blob/e35ef1423964eb8840d5430a3e92aa9a2711cda8/source/elektron/md/mdJucePlugin/mdAutomationParameterTest.cpp).

Use host parameters for integrated automation; validate sample/block timing through
the actual wrapper and firmware. VST3 CC handling uses parameter mappings and
cannot be treated as an arbitrary byte pipe.
[Steinberg communication model](https://steinbergmedia.github.io/vst3_dev_portal/pages/FAQ/Communication.html).

DRS's current live p-lock path leaves the most recently sent knob value in place.
True MD-style per-trig locks require restoring the base value on the appropriate
unlocked trig, and a policy for manual knob edits, kit changes and overlapping
notes. Automation is feasible immediately after hosting; exact hardware lock
semantics are a separate feature. MD/MM lane tables must also be added to the UI.

### 5. Session recall and capabilities

Persist plugin format/class identity, version, instance ID, routing, bus layout and
opaque plugin state. Treat paths as relink hints. Preserve an unavailable plugin's
state and tracks as an offline device. Add project-format migration deliberately;
old readers must not silently discard software instruments.

State may be megabytes, so evaluate an archive/binary payload store rather than
embedding repeated copies in scene snapshots or undo records. Capture state through
the host lifecycle outside the audio callback, with an atomic project-save policy.
Plugin UI edits must mark the DRS session dirty. Keep one instance through DRS scene
changes; loading a whole emulator snapshot on each scene is not a viable default.

Save/restore exists upstream, but it is not a promise to retain every volatile
memory buffer. MD UW RAM-machine recordings remain volatile; upstream advises
copying them to ROM slots for persistence. Firmware/factory baselines remain
external dependencies. Verify exact kit, automation and user-sample recall.
[State contract](https://github.com/joelanders/gearmulator-md-mm/blob/e35ef1423964eb8840d5430a3e92aa9a2711cda8/source/elektron/md/mdLib/mdstate.h).

Hosting does not implement DRS's FETCH/SEND/backup protocol for these machines.
Mark them live-only initially. MD/MM pattern/kit decoding and safe round trips are
a separate protocol project; the existing A4 gen-1 route is not interchangeable.

### 6. Packaging and dependencies

Prototype against user-installed plugins and user-supplied firmware. The published
release does not include firmware. Match host/plugin CPU architecture, pin a tested
version, and test scanner timeouts, missing components and failed boot explicitly.

DRS declares GPL-3.0-or-later and Gearmulator supplies a GPLv3 license. A hosting
dependency still needs its own version-specific review: current JUCE uses
AGPLv3/commercial licensing, so do not assume that adding the newest JUCE preserves
DRS's existing distribution terms without further decisions. Hosting an installed
plugin and redistributing/linking its emulator are different dependency choices.
[Gearmulator license](https://github.com/joelanders/gearmulator-md-mm/blob/e35ef1423964eb8840d5430a3e92aa9a2711cda8/LICENSE.md),
[current JUCE license](https://github.com/juce-framework/JUCE/blob/master/LICENSE.md).

## Proof of concept and delivery gates

1. **Musical compatibility:** Use an external host or standalone and virtual MIDI
   to prove MD part mapping and all six MM channels from DRS. Disable DRS hardware
   parameter lanes unless the destination map is known. Verify note duration,
   velocity, retriggering and the internal-sequencer ownership rule.
2. **Integrated host spike:** A minimal DRS-managed helper loads one MD and one MM
   VST3, opens their editors, accepts timestamped events, supplies host playhead
   data and mixes their main outputs. Load/restore before play; render while stopped.
3. **Core measurements:** At 44.1/48 kHz and 128/256/512 frames, record CPU load,
   dropouts, note onset jitter and latency with both plugins active and editors
   open/closed. Test dense drums, MM modulation, automation-before-note, explicit
   zero values, play/stop/continue and loops. No performance promise before this.
4. **First usable feature:** Devices UI, explicit track mapping, audition/thru,
   scenes, instance volume/meter, session save/reopen, missing-plugin recovery,
   orderly shutdown and helper-crash reporting. Old hardware sessions still load
   and play. Verify a physical box and software instruments together.
5. **Expand only after that:** AU compatibility, more host platforms, parameter
   lanes with defined reset semantics, auxiliary audio outputs, audio inputs,
   render/export, generic instrument profiles and stronger crash isolation.

Indicative engineering scale, not a schedule: an external MIDI experiment is a
small spike; integrated hosting is a several-week feature with timing and platform
unknowns; a general-purpose multi-format host with robust recovery is a larger
ongoing subsystem. The host spike should determine the estimate before committing
to the full scope.

**Decision:** Strong architectural fit and a credible implementation path. Begin
with shared MD/MM instances and VST3 hosting behind a backend abstraction; use AU
as the next compatibility target. Prove routing, transport and recall before
extending DRS's hardware transfer machinery or promising arbitrary-plugin support.
