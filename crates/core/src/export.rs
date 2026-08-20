// The export seam: one track of this session becomes the write a box will take.
//
// Port of the write half of `js/roll-bridge.js` — `rollNotesToDevice` and
// `rollPLocksToDevice` — plus the assembly `js/main.js` does between them and
// `safeWriteTrack` in its "Send to box" handler. `import.rs` is the other
// direction and this sits beside it deliberately: every rule about what a fetch
// keeps has a mirror here about what a write carries back, and a rule that only
// exists on one side is how a round trip loses something.
//
// **`core` still parses no bytes** (PLAN.md §3). Everything below hands
// `protocol` a *description* — an encoder-shaped note, a lane, a `TrackWrite` —
// and `protocol` is what puts a byte anywhere. Nothing here can reach a device
// either: a `TrackWrite` is inert until `safe_write_track` is handed one, which
// is what keeps PLAN.md §7 rule 2 true while this exists. A caller that builds
// one and sends it any other way is the bug that rule is about.
//
// Six decisions, each a place the JS's answer does not port directly.
//
//   1. **A step becomes a byte, so a note between steps is rounded and said
//      so.** The model holds `step` as an `f64` because the *engine* schedules
//      in fractional steps; a trig record holds it in one byte. The JS passes
//      its float straight into a `Uint8Array` store, which truncates in silence.
//      Rounding to the nearest step is the honest conversion and the warning is
//      the honest report. What this deliberately does *not* do is fold the
//      fraction into `micro`: the box's micro-timing is a separate field with
//      its own range, and quietly turning a position into a timing offset would
//      be inventing behaviour no oracle has.
//   2. **A step no byte can name is dropped here rather than wrapped.** `as u8`
//      on step 300 is 44 — a note silently moved three bars earlier. The roll
//      cannot author one; a hand-edited project file can. Steps past the
//      pattern's own length are *not* filtered here: `encode_track_notes`
//      already drops those and reports the count, and two layers counting the
//      same drop is how a report stops adding up.
//   3. **An unknown condition is dropped and its note is kept.** `cond` is a
//      `String` in the model and a `'static` menu label on the wire, so a
//      condition this build does not know cannot be written. Refusing the whole
//      write would lose the music over a label; writing a *near* condition would
//      be worse than either. The note goes, the condition does not, and the
//      count is warned about.
//   4. **A lane with a parameter number and no box is refused.** The JS accepts
//      it, because a browser drives one box and a lane could only have come from
//      it. This app drives several, and `paramId` 44 is filter cutoff on a DT2
//      and something else on a DN2 — the same argument `audition.rs` makes for
//      refusing to *play* such a lane, with more force, because this one lands
//      in a pattern. A lane with a *name* and no box is still written: the name
//      is canonical across boxes and `describe_param` resolves it in the
//      destination's own table, which is exactly what cross-device copy does.
//   5. **The track's lanes are the truth, so `plocks` is always `Some`.**
//      `TrackWrite::plocks: None` means "leave the pool alone" and `Some(vec)`
//      means "these are the track's lanes", freeing any the box holds and this
//      track does not. That is the same bargain the notes and the conditions
//      make — the track is being replaced — and it is what `js/main.js` says in
//      as many words.
//   6. **Swing travels, and it reaches the whole slot.** The JS sends the
//      pattern's swing on every write. It is one byte per *pattern*, so it
//      changes the feel of all sixteen tracks in the destination — which is why
//      `safe_write_track` refuses to touch it unless asked, and why the confirm
//      dialog (`write_impact_lines`, with the box's current swing beside ours)
//      has to say so before anyone agrees to it.
//
// What this deliberately leaves to someone else: nothing is clamped to the
// destination track's LEN. A pattern longer than the track it lands on is stored
// in full and heard as far as the box's own LEN — the confirm dialog is where
// that gets said, because only the re-fetch knows what the destination's length
// is.

use std::collections::BTreeMap;

use digi_protocol::conditions::cond_key;
use digi_protocol::params::{describe_param, device_kind_key, param_table_for};
use digi_protocol::pattern::{Note as DeviceNote, Spec};
use digi_protocol::plocks::LaneWrite;
use digi_protocol::safe_write::TrackWrite;
use digi_protocol::trig_cond::TrigSetting;

use crate::device::{DeviceId, DeviceModel};
use crate::model::{Note, PLockLane, Pattern};
use crate::session::{PatternRef, Session};

/// The last step a trig record can name: it holds the step in one byte.
const MAX_STEP: f64 = u8::MAX as f64;

/// The largest p-lock parameter id a lane can carry. `0xFF` marks a *free* lane,
/// so a lane claiming it would erase itself; `params::Param::validate` holds the
/// curated tables to the same rule.
const MAX_PARAM_ID: u16 = 0xFE;

/// One track's write, and everything about it that did not fit.
///
/// The warnings are written to be shown verbatim, and they are the only way this
/// reports trouble — a lane that cannot be written and a note off the end of the
/// pattern are both losses a person should agree to rather than errors that stop
/// a write, which is the same call `apply_track_plocks` makes about a full pool.
#[derive(Debug, Clone)]
pub struct TrackExport {
    pub write: TrackWrite,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    NoSuchDevice(DeviceId),
    NoSuchSlot { device: DeviceId, slot: PatternRef },
    /// The source is a sequence-live-only model, which has no pattern format to
    /// write out of.
    LiveOnly(&'static DeviceModel),
    NoSuchTrack { track: usize, tracks: usize },
    /// A slot past the 256 a dump message's one-byte pattern index can name.
    NotOnTheWire(PatternRef),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchDevice(id) => write!(f, "no device {id:?} in this session"),
            Self::NoSuchSlot { slot, .. } => {
                write!(f, "this box has no slot {}", slot.label())
            }
            Self::LiveOnly(model) => write!(
                f,
                "{} plays over MIDI but has no patterns to write",
                model.display
            ),
            Self::NoSuchTrack { track, tracks } => write!(
                f,
                "track {} does not exist — this pattern has {tracks}",
                track + 1
            ),
            Self::NotOnTheWire(slot) => write!(
                f,
                "{} is past the last slot a dump message can name",
                slot.label()
            ),
        }
    }
}

impl std::error::Error for ExportError {}

/// Model notes → the encoder's shape, each with the trig settings that travel
/// with it.
///
/// The port of `rollNotesToDevice`, which drops the id the box has no concept of
/// and passes the rest through. Pitch, velocity, length and micro-timing are
/// *not* clamped: the encoder masks the two seven-bit fields itself and a length
/// off the wire is representable by construction, so clamping here would only be
/// able to change values that arrived correct. Order is preserved — the encoder
/// sorts by `(step, pitch)` itself, and it is the one that has to.
///
/// Paired rather than merged because `pattern::Note` is the hardware-verified
/// encode shape and adding three fields to it would edit the struct
/// `encode_track_notes` reads (PLAN.md §7 rule 3). The same decision
/// `trig_settings_from_notes` is built on, which is what consumes this.
pub fn notes_for_device(notes: &[Note]) -> (Vec<(DeviceNote, TrigSetting)>, Vec<String>) {
    let mut out = Vec::with_capacity(notes.len());
    let mut off_the_byte = 0usize;
    let mut off_the_grid = 0usize;
    // Counted per label rather than per note: eight trigs carrying one unknown
    // condition is one thing that went wrong, not eight.
    let mut unknown: BTreeMap<String, usize> = BTreeMap::new();

    for note in notes {
        let step = note.step.round();
        if !(0.0..=MAX_STEP).contains(&step) {
            off_the_byte += 1;
            continue;
        }
        if step != note.step {
            off_the_grid += 1;
        }
        let cond = match note.cond.as_deref() {
            None | Some("") => None,
            Some(key) => match cond_key(key) {
                Some(known) => Some(known),
                None => {
                    *unknown.entry(key.to_owned()).or_default() += 1;
                    None
                }
            },
        };
        out.push((
            DeviceNote {
                step: step as u8,
                pitch: note.pitch,
                velocity: note.velocity,
                len_steps: note.len,
                micro: note.micro,
            },
            TrigSetting { prob: note.prob, fill: note.fill, cond },
        ));
    }

    let mut warnings = Vec::new();
    if off_the_byte > 0 {
        warnings.push(format!(
            "{off_the_byte} note{} sat outside the 0–{} steps a trig record can name and {} \
             not sent",
            plural(off_the_byte),
            MAX_STEP as u16,
            were(off_the_byte),
        ));
    }
    if off_the_grid > 0 {
        warnings.push(format!(
            "{off_the_grid} note{} sat between steps and {} rounded onto the nearest one — \
             the boxes store a trig on a whole step, with micro-timing as its own offset",
            plural(off_the_grid),
            were(off_the_grid),
        ));
    }
    for (key, count) in unknown {
        warnings.push(format!(
            "{count} trig{} {} the condition {key:?}, which is not on the boxes' COND menu — \
             {} sent without it",
            plural(count),
            if count == 1 { "carries" } else { "carry" },
            were(count),
        ));
    }
    (out, warnings)
}

/// Model p-lock lanes → the lanes `apply_track_plocks` writes, for the box they
/// are aimed at.
///
/// The port of `rollPLocksToDevice`. Four reasons a lane does not make it, each
/// reported rather than silent:
///
/// * it belongs to the other box's parameter numbering — crossing boxes is
///   copy-track's job, and that translates by name first;
/// * it carries a parameter *number* and no box at all, which is decision 4 on
///   this module and the one place this is stricter than the JS;
/// * its parameter has no measured p-lock slot, so there is no byte to write it
///   into. Phase 0 measured all eleven on both boxes, so this is the path
///   waiting for whatever joins the tables next — and it is the difference
///   between "you can hear this" and "you can send this";
/// * it holds no values at all, which is not worth a word: an empty lane would
///   claim one of the pattern's eighty for nothing, and `apply_track_plocks`
///   refuses it anyway.
pub fn lanes_for_device(lanes: &[PLockLane], device_kind: &str) -> (Vec<LaneWrite>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();

    for lane in lanes {
        match lane.device_kind.as_deref() {
            Some(kind) if kind != device_kind => {
                warnings.push(format!(
                    "the {} lane wasn't sent — it belongs to a {kind}'s parameter numbering, \
                     not this {device_kind}'s",
                    lane.param().label,
                ));
                continue;
            }
            None if lane.name.is_none() => {
                warnings.push(format!(
                    "the {} lane wasn't sent — it names a parameter by number with no box to \
                     read that number against, and the boxes number their parameters \
                     differently",
                    lane.param().label,
                ));
                continue;
            }
            _ => {}
        }

        // Resolved against the *destination's* table, which is what makes a
        // named lane portable: the name wins, so a lane authored here lands on
        // the right knob of whichever box it is sent to.
        let param = describe_param(
            param_table_for(device_kind),
            lane.name.as_deref(),
            lane.param_id,
            device_kind_key(device_kind),
        );
        let Some(plock) = param.plock else {
            warnings.push(format!(
                "the {} lane wasn't sent — digi-roll can play that parameter over MIDI but \
                 hasn't yet measured which p-lock slot the pattern format stores it in, so it \
                 can't write it into the pattern",
                param.label,
            ));
            continue;
        };
        if plock.id > MAX_PARAM_ID {
            // Unreachable from a curated table (`Param::validate` refuses it) and
            // from a box (its ids are bytes), so this is the hand-edited project
            // file — where truncating to a `u8` would aim the lane at a real
            // parameter, silently and wrongly.
            warnings.push(format!(
                "the {} lane wasn't sent — parameter number {} is past the {MAX_PARAM_ID} a \
                 pattern's lane can hold",
                param.label, plock.id,
            ));
            continue;
        }

        let values: Vec<Option<u16>> = lane
            .values
            .iter()
            .map(|v| v.and_then(|display| param.stored_from_display(f64::from(display))))
            .collect();
        if !values.iter().any(Option::is_some) {
            continue;
        }
        out.push(LaneWrite { param_id: plock.id as u8, values });
    }
    (out, warnings)
}

/// One track of one pattern, described as a write to one slot of one box.
///
/// `spec` is the *destination's* — the box that answered, not the model this
/// pattern belongs to — because a lane resolves against the box it is aimed at.
/// The mirror of `Fetched::spec` on the import side, and for the same reason:
/// handing this the wrong box's spec is how a write becomes plausible nonsense.
///
/// This does not check that the destination *has* the track: only the box knows
/// how many it has, and `encode_track_notes` refuses one it does not, with the
/// re-fetched payload in front of it.
pub fn track_write(
    spec: &Spec,
    pattern: &Pattern,
    track_index: usize,
    into: PatternRef,
) -> Result<TrackExport, ExportError> {
    let track = pattern
        .track(track_index)
        .ok_or(ExportError::NoSuchTrack { track: track_index, tracks: pattern.num_tracks() })?;
    let index = into.wire_index().ok_or(ExportError::NotOnTheWire(into))?;

    let (notes, mut warnings) = notes_for_device(&track.notes);
    let (plocks, lane_warnings) = lanes_for_device(&track.plocks, spec.device);
    warnings.extend(lane_warnings);

    Ok(TrackExport {
        write: TrackWrite {
            index,
            track_index,
            notes,
            // The track's own PROB default. Always sent: it is one byte of this
            // track's own defaults, the model always holds a value for it, and
            // leaving it would keep whatever the destination track happened to
            // have under trigs that came from here.
            track_prob: Some(track.track_prob),
            // Decision 5 — the track's lanes are the truth, including when there
            // are none.
            plocks: Some(plocks),
            // Decision 6 — and it reaches all sixteen tracks in the slot.
            swing: Some(f64::from(pattern.swing)),
        },
        warnings,
    })
}

impl Session {
    /// One track of one of this session's slots, described as a write.
    ///
    /// The mirror of [`Session::import_pattern`], and the end of the write path
    /// on this side of the wire: `track_write` → `safe_write_track` →
    /// `PatternIo::send_pattern_kit`. Nothing here sends anything.
    ///
    /// `spec` is the destination box's, per [`track_write`]. The caller is the
    /// one holding the identity handshake, so the caller is the one that can say
    /// whether the box on the cable is the box this pattern is for — refusing a
    /// mismatch is `js/main.js`'s `connectForSend` rule and belongs with the
    /// button, not here.
    pub fn track_write(
        &self,
        spec: &Spec,
        device: DeviceId,
        from: PatternRef,
        track_index: usize,
        into: PatternRef,
    ) -> Result<TrackExport, ExportError> {
        let d = self.device(device).ok_or(ExportError::NoSuchDevice(device))?;
        if !d.can_sysex() {
            return Err(ExportError::LiveOnly(d.model));
        }
        let pattern = d
            .pattern(from.slot())
            .ok_or(ExportError::NoSuchSlot { device, slot: from })?;
        track_write(spec, pattern, track_index, into)
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn were(n: usize) -> &'static str {
    if n == 1 {
        "was"
    } else {
        "were"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_protocol::pattern::dt2_spec;

    /// Expectations derived by running the JS against the same inputs:
    ///
    /// ```text
    /// cd ../digi-roll && node --input-type=module -e "
    /// import { rollNotesToDevice, rollPLocksToDevice } from './js/roll-bridge.js';
    /// import { makeNote, makePLockLane } from './js/state.js';
    /// console.log(JSON.stringify(rollNotesToDevice([
    ///   makeNote(3, 40, 2, 90, -5/24, {}),
    ///   makeNote(0, 60, 0.25, 100, 0, { prob: 35, fill: true, cond: '2:4' }),
    /// ])));
    /// console.log(JSON.stringify(rollPLocksToDevice([
    ///   makePLockLane({ name: 'filter.cutoff', deviceKind: 'DT2', values: { 3: 100 } }),
    /// ], 'DT2')));"
    /// ```
    fn note(step: f64, pitch: u8, len: f64, velocity: u8, micro: f64) -> Note {
        Note::new(step, pitch, len, velocity, micro)
    }

    fn locked(mut n: Note, prob: Option<u8>, fill: Option<bool>, cond: Option<&str>) -> Note {
        n.prob = prob;
        n.fill = fill;
        n.cond = cond.map(str::to_owned);
        n
    }

    fn lane(
        name: Option<&str>,
        param_id: Option<u16>,
        kind: Option<&str>,
        at: &[(usize, u16)],
    ) -> PLockLane {
        let mut values = vec![None; 128];
        for (step, v) in at {
            values[*step] = Some(*v);
        }
        PLockLane::new(
            name.map(str::to_owned),
            param_id,
            kind.map(str::to_owned),
            false,
            values,
        )
        .expect("a lane with one of the two identities")
    }

    /// The steps a lane actually holds, so a 128-entry vec is not compared by eye.
    fn held(lane: &LaneWrite) -> Vec<(usize, u16)> {
        lane.values
            .iter()
            .enumerate()
            .filter_map(|(step, v)| v.map(|w| (step, w)))
            .collect()
    }

    #[test]
    fn a_note_goes_out_with_everything_it_carries_and_nothing_it_does_not() {
        let (out, warnings) = notes_for_device(&[
            note(3.0, 40, 2.0, 90, -5.0 / 24.0),
            locked(note(0.0, 60, 0.25, 100, 0.0), Some(35), Some(true), Some("2:4")),
        ]);
        assert!(warnings.is_empty());
        // Order is untouched: the encoder sorts by (step, pitch) itself, and
        // sorting twice is how two sorts get to disagree.
        assert_eq!(
            out.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            vec![
                DeviceNote { step: 3, pitch: 40, velocity: 90, len_steps: 2.0, micro: -5.0 / 24.0 },
                DeviceNote { step: 0, pitch: 60, velocity: 100, len_steps: 0.25, micro: 0.0 },
            ]
        );
        assert_eq!(out[0].1, TrigSetting::default());
        assert_eq!(
            out[1].1,
            TrigSetting { prob: Some(35), fill: Some(true), cond: Some("2:4") }
        );
    }

    #[test]
    fn a_fractional_length_survives_to_the_byte_that_can_hold_it() {
        // The DN2 fixture's 4.75-step trig is why `len` is fractional at all: it
        // used to come home as 5 and go back to the box a quarter-step too long.
        let (out, warnings) = notes_for_device(&[note(0.0, 60, 4.75, 90, 0.0)]);
        assert_eq!(out[0].0.len_steps, 4.75);
        assert!(warnings.is_empty());
    }

    #[test]
    fn a_note_between_steps_is_rounded_onto_one_and_the_move_is_reported() {
        // Nothing in the roll can author this today; a hand-edited project file
        // can, and the box has nowhere to put it. Rounding silently would move a
        // note by half a step with nothing said.
        let (out, warnings) = notes_for_device(&[
            note(4.5, 60, 1.0, 100, 0.0),
            note(9.4, 60, 1.0, 100, 0.0),
            note(2.0, 60, 1.0, 100, 0.0),
        ]);
        assert_eq!(out.iter().map(|(n, _)| n.step).collect::<Vec<_>>(), vec![5, 9, 2]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("2 notes sat between steps"), "{}", warnings[0]);
    }

    #[test]
    fn a_step_no_byte_can_name_is_dropped_rather_than_wrapped() {
        // `300 as u8` is 44 — the note would land three bars early, in silence.
        // The encoder's own drop for steps past the pattern's 128 is *not*
        // duplicated here: it counts those and this would double-count them.
        let (out, warnings) = notes_for_device(&[
            note(300.0, 60, 1.0, 100, 0.0),
            note(-1.0, 60, 1.0, 100, 0.0),
            note(200.0, 60, 1.0, 100, 0.0),
        ]);
        assert_eq!(out.iter().map(|(n, _)| n.step).collect::<Vec<_>>(), vec![200]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("2 notes sat outside the 0–255 steps"), "{}", warnings[0]);
    }

    #[test]
    fn a_condition_no_box_has_is_dropped_and_its_note_is_kept() {
        // The note is the music and the label is a label. Refusing the write
        // would lose the first over the second; sending a *near* condition would
        // be worse than either.
        let (out, warnings) = notes_for_device(&[
            locked(note(0.0, 60, 1.0, 100, 0.0), None, None, Some("9:9")),
            locked(note(1.0, 60, 1.0, 100, 0.0), None, None, Some("9:9")),
            locked(note(2.0, 60, 1.0, 100, 0.0), None, None, Some("1:4")),
            locked(note(3.0, 60, 1.0, 100, 0.0), None, None, Some("")),
        ]);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].1.cond, None);
        assert_eq!(out[2].1.cond, Some("1:4"));
        // An empty string is "no condition", exactly as `cond_to_byte` reads it.
        assert_eq!(out[3].1.cond, None);
        assert_eq!(warnings.len(), 1, "one label, one warning: {warnings:?}");
        assert!(warnings[0].contains("2 trigs carry the condition \"9:9\""), "{}", warnings[0]);
    }

    #[test]
    fn a_named_lane_goes_out_through_its_measured_slot_and_scaling() {
        // paramId 44 and value × 256 are the numbers read back off the DT2 in the
        // Phase 0 round-1 capture, and what the JS gives for this lane.
        let (lanes, warnings) =
            lanes_for_device(&[lane(Some("filter.cutoff"), None, Some("DT2"), &[(3, 100)])], "DT2");
        assert!(warnings.is_empty());
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].param_id, 44);
        assert_eq!(held(&lanes[0]), vec![(3, 25600)]);
        // The whole step axis is offered, so a short lane cannot leave a lock the
        // caller meant to clear.
        assert_eq!(lanes[0].values.len(), 128);
    }

    #[test]
    fn a_lane_off_a_box_we_cannot_name_passes_its_word_through_untouched() {
        // Byte-exactness on the way back out is the whole reason the raw
        // descriptor's scaling is the identity: 0x2a is in neither table, and a
        // lane nobody can name must still survive a round trip.
        let (lanes, warnings) =
            lanes_for_device(&[lane(None, Some(0x2A), Some("DT2"), &[(2, 40000)])], "DT2");
        assert!(warnings.is_empty());
        assert_eq!(lanes[0].param_id, 0x2A);
        assert_eq!(held(&lanes[0]), vec![(2, 40000)]);
    }

    #[test]
    fn a_lane_belonging_to_the_other_box_is_refused_rather_than_aimed_at_a_guess() {
        let (lanes, warnings) =
            lanes_for_device(&[lane(None, Some(0x2A), Some("DN2"), &[(3, 64)])], "DT2");
        assert!(lanes.is_empty());
        assert_eq!(
            warnings,
            vec![
                "the DN2 param 0x2a lane wasn't sent — it belongs to a DN2's parameter \
                 numbering, not this DT2's"
            ]
        );
    }

    #[test]
    fn a_parameter_number_with_no_box_behind_it_is_refused() {
        // Stricter than the JS on purpose — decision 4. 44 is a real DT2
        // parameter and a different real DN2 one, and a lane that cannot say
        // which box it came from cannot say which of them it means.
        let (lanes, warnings) = lanes_for_device(&[lane(None, Some(44), None, &[(1, 64)])], "DT2");
        assert!(lanes.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("no box to read that number against"), "{}", warnings[0]);
    }

    #[test]
    fn a_named_lane_with_no_box_is_still_written_because_the_name_is_the_box() {
        // The other half of decision 4, and the case cross-device copy relies on:
        // a canonical name resolves in the destination's own table, so the lane
        // lands on the right knob rather than the right number.
        let (lanes, warnings) =
            lanes_for_device(&[lane(Some("filter.cutoff"), None, None, &[(1, 64)])], "DT2");
        assert!(warnings.is_empty());
        assert_eq!(lanes[0].param_id, 44);
        assert_eq!(held(&lanes[0]), vec![(1, 16384)]);
    }

    #[test]
    fn a_parameter_number_past_what_a_lane_can_hold_is_refused_not_truncated() {
        // 0x12C truncates to 44, which is filter cutoff — a lane nobody asked for,
        // on a knob everybody can hear. 0xFF is the free-lane sentinel, so a lane
        // claiming it would erase itself.
        for id in [0x12C, 0xFF] {
            let (lanes, warnings) = lanes_for_device(&[lane(None, Some(id), Some("DT2"), &[(0, 1)])], "DT2");
            assert!(lanes.is_empty(), "paramId {id:#x} was written");
            assert_eq!(warnings.len(), 1);
            assert!(warnings[0].contains("is past the"), "{}", warnings[0]);
        }
    }

    #[test]
    fn an_empty_lane_is_dropped_without_a_word() {
        // It would claim one of the pattern's eighty lanes to say nothing, and
        // there is nothing for a person to do about it.
        let (lanes, warnings) =
            lanes_for_device(&[lane(Some("amp.pan"), None, Some("DT2"), &[])], "DT2");
        assert!(lanes.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn a_write_carries_the_tracks_prob_default_its_lanes_and_the_patterns_swing() {
        let mut pattern = Pattern::for_model(&crate::device::DT2);
        pattern.swing = 65;
        let track = pattern.track_mut(2).unwrap();
        track.track_prob = 70;
        track.notes = vec![note(0.0, 60, 1.0, 100, 0.0)];
        track.plocks = vec![lane(Some("filter.cutoff"), None, Some("DT2"), &[(0, 100)])];

        let export = track_write(&dt2_spec(), &pattern, 2, PatternRef::new(1, 3)).unwrap();
        assert!(export.warnings.is_empty());
        assert_eq!(export.write.index, 19, "B04 is the twentieth slot on the wire");
        assert_eq!(export.write.track_index, 2);
        assert_eq!(export.write.track_prob, Some(70));
        assert_eq!(export.write.swing, Some(65.0));
        assert_eq!(export.write.notes.len(), 1);
        assert_eq!(export.write.plocks.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn a_track_with_no_lanes_asks_for_none_rather_than_leaving_the_pool_alone() {
        // `None` and `Some(vec![])` are different instructions to
        // `apply_track_plocks`: the first leaves the box's lanes where they are,
        // the second frees them. The track is being replaced, so the second is
        // the honest one — automation left behind would belong to trigs that no
        // longer exist.
        let mut pattern = Pattern::for_model(&crate::device::DT2);
        pattern.track_mut(0).unwrap().notes = vec![note(0.0, 60, 1.0, 100, 0.0)];
        let export = track_write(&dt2_spec(), &pattern, 0, PatternRef::new(0, 0)).unwrap();
        assert_eq!(export.write.plocks, Some(Vec::new()));
    }

    #[test]
    fn a_track_the_pattern_does_not_have_is_refused_before_anything_is_built() {
        let pattern = Pattern::for_model(&crate::device::DT2);
        assert_eq!(
            track_write(&dt2_spec(), &pattern, 16, PatternRef::new(0, 0)).err(),
            Some(ExportError::NoSuchTrack { track: 16, tracks: 16 }),
        );
    }

    #[test]
    fn a_slot_no_dump_message_can_name_is_refused() {
        // Sixteen banks of sixteen is the whole space one byte can carry, and
        // both boxes have exactly that. Q01 does not exist on either.
        let pattern = Pattern::for_model(&crate::device::DT2);
        let past_the_end = PatternRef::new(16, 0);
        assert_eq!(
            track_write(&dt2_spec(), &pattern, 0, past_the_end).err(),
            Some(ExportError::NotOnTheWire(past_the_end)),
        );
    }
}
