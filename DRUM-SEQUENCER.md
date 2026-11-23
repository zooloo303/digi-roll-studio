# Drum Steps

Implemented 2026-11-14 as a native egui editor alongside Piano Roll.

## Design

The TRACKS overview continues to select the device and track, and keeps its
existing length, scale, channel, mute and solo controls. The editor selector
underneath switches between **PIANO ROLL** and **DRUM STEPS**.

Drum Steps shows the selected device's current pattern as one hardware track
per row and sixteen steps per page. This is useful for sampled drums and synth
percussion alike: no General MIDI mapping or conversion is involved. The two
views share the same notes, selection, undo history, transport and session file.

The **BOX** buttons switch device focus directly. They share the same selection
as TRACKS and Kit Builder, and retain the track number where it exists on the
new box (otherwise the last available track).

- Track names and mute buttons stay beside the grid. Length and scale appear
  on two lines next to each name. Selecting a track reveals its row.
- Four-step shading makes beats readable. Filled cyan cells are occupied;
  their small bars show velocity. A white outline marks selected notes and an
  amber outline shows each track's own playhead. Recording uses the existing
  red indication on the selected track.
- Cells after a track's length are hatched and cannot be edited. Tracks with
  different lengths and scales keep independent playheads; shorter patterns
  are not repeated into empty cells.
- Pages cover the longest track in the current pattern. **Follow** follows the
  selected track's page during playback. Picking a page turns Follow off.
- The selected track's velocity, probability, condition, fill and existing
  parameter-lock lanes sit below the matrix with matching step columns.
  Parameter-lock editing and read-only restrictions reuse the Piano Roll's
  controls. Extra lanes use the existing strip's overflow indication.
- The matrix scrolls vertically. Narrow windows can scroll horizontally with
  the lanes; short windows can scroll the editor to reach its lower controls.

## Gestures

| Action | Result |
|---|---|
| Left-click an empty cell | Add a one-step note using NEW NOTE pitch and VEL |
| Left-click an occupied cell | Remove the notes whose starts fall in that cell |
| Left-drag horizontally | Paint the initial cell's add/erase action across the row; revisiting a cell does not toggle it again |
| Right-click / middle-click / other mouse buttons | No edit; right-click is reserved for a future context menu |
| Shift-click | Add/remove that cell's notes from the shared selection without changing them |
| Shift + left-drag vertically | Change velocity: up is louder, down is softer, one unit per pixel; a selected cell brings the selected group, an unselected cell selects its own notes |
| Delete / Backspace | Delete selected notes when no field has keyboard focus |
| Drag the velocity lane | Set the cell's velocity; touching a selected cell applies it to the selection |
| Click a track name | Select the track and show its detail lanes |
| M | Toggle that row's session mute |

NEW NOTE is a pitch default per device/track for the current app run. It starts
at the track's first existing pitch or MIDI note 60 (C5 in this app). VEL uses
the existing shared default. Changing these defaults does not retune existing
notes. Use the Edit/Harmony panels or Piano Roll for pitch and duration changes.

A cell groups all note starts in its step interval. Multiple notes display a
count; a single note with an off-grid start or micro-timing displays `~`.
A dot marks a probability, condition or fill setting. Switching editors does
not quantise notes, collapse chords or change their lengths. Erasing a cell
leaves its p-lock data intact, matching Piano Roll deletion. Trig/p-lock lanes
retain their existing integer-step semantics; use Piano Roll for individual
off-grid notes.

## Integration

`ui/drums.rs` renders and edits the ordinary core `Track` model.
`ui/workspace.rs` owns the view choice and transient drum-editor state. The
shell's existing change flag commits a whole gesture as one undo step, updates
the engine and marks the session dirty. Undo, reload and device/pattern changes
cancel held gestures. No new file format or MIDI/SysEx route is introduced.
Hardware writes still use the existing explicit write workflow.

Steps can be toggled with the left mouse button while transport playback
continues. Fast clicks are handled even when press and release arrive in one UI
frame. Follow holds the visible page through the click/paint or velocity
gesture, then resumes following playback.
Edits use the existing live engine snapshot and undo path.

The view, page, Follow and new-note pitch defaults are transient UI preferences.
Piano Roll remains the default at launch. This version is a view of one device
at a time; it does not provide a cross-device drum rack, audio sample preview,
or new retrigger/sound-lock data types.

## Offline preview and verification

```sh
cargo run -p digi_roll_studio --example drum_steps_preview
```

The preview uses the real workspace with a synthetic Digitakt II pattern and
an inert MIDI sink. Its Play button runs the real engine without opening MIDI
ports. Laptop/Narrow buttons expose the two checked window sizes, 1100 × 800
and 700 × 680. The normal app exposes the view through its new editor selector.

Fourteen focused drum tests exercise real egui pointer/key events, shared selection,
undo/redo, fast drags, erase, page offsets, short tracks, device changes,
chords/off-grid notes, p-lock preservation, velocity edits, fast playback clicks
and Follow page boundaries. They also check Shift-drag velocity, group/clamp/undo
behavior and gesture cancellation on device changes. Right/middle/extra buttons
cannot edit while left-click toggles both stopped and playing. A shared device
switcher test checks focus and valid track selection without changing patterns.
Native checks cover page/view switching, add/undo and independent playheads,
plus playback toggles in the full packaged app. The corrected macOS test build
was confirmed working by the user. No new hardware write was performed for
these editor fixes.
