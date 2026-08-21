// Copy/paste of a whole track, at the level of `digi_core::model::Track` — not
// a device payload. `protocol::copy_track` already does the hardware-level
// version of this (source bytes → target bytes on a fetched pattern); this is
// its in-app sibling, copying whatever is sitting in the session right now,
// with no fetch and no write involved. The two share their policy on purpose:
// see below for exactly which parts of that policy transfer unchanged and
// which don't apply at this layer at all.
//
// **What travels: `notes`, `plocks`, `length_steps`, `scale`, `track_prob`.
// What does not: `out_port`, `channel`, `mute`, `solo`, `level`, `name`, `patch`.**
// The routing fields belong to the destination track's place in the studio,
// not to the music being copied — the same call `protocol::copy_track` makes
// for sounds and kit (its own doc comment: "Sounds, kit and the pattern's own
// settings belong to the target and are left exactly as they were"). Swing
// stays out for the identical reason `protocol::copy_track` gives: it is the
// whole pattern's byte, not one track's, so copying it here would re-time
// every other track already in the destination's pattern.
//
// ## Two things can fail to cross, both reported and never guessed at
//
// * **Chords.** See [`MAX_CHORD_NOTES_PRECEDENT`] below for which cap this
//   uses and why.
// * **p-lock lanes**, when the paste crosses device kinds. See
//   [`plock_lanes_for_target`], which carries the *rule*
//   `protocol::copy_track::plock_lanes_for_target` uses — translate by
//   canonical parameter name, drop and say so when there is no equivalent —
//   but not its rescaling arithmetic, because it does not apply at this
//   layer. See that function's doc comment for why.
//
// ## Where this deliberately does not duplicate `protocol::copy_track`
//
// That module reads and writes device-native bytes: a lane's stored word is
// scaled per-box (`ParamDesc::display_from_stored`/`stored_from_display`), so
// translating a lane between two boxes' numbering has to round-trip through
// both scalings. `core::model::PLockLane` never holds that stored word at
// all — its own doc comment states the invariant: `values` are already on
// the parameter's *display* axis, "MIDI 0–127 for a curated parameter" — and
// both `DT2_PARAMS` and `DN2_PARAMS` describe that axis identically (0–127
// for both tables; see `digi_protocol::params`). So a curated lane's values
// need no rescale at all when the name resolves on the target's table: this
// is a strictly simpler translation than the byte-level one, not a smaller
// copy of it, because the app's own import path already paid the rescaling
// cost once, at fetch time.

use digi_protocol::params::{param_by_name, param_by_plock_id, param_table_for};

use crate::chords::MAX_CHORD_NOTES;
use crate::model::{Note, PLockLane, Track, TrackScale};

/// The chord-width cap this module truncates pasted notes to, and the reason
/// it is [`MAX_CHORD_NOTES`] rather than a per-device figure the way
/// `protocol::copy_track::truncate_chords` reads `target_spec.trig.max_notes`.
///
/// **Investigated before writing this**, per the brief: the app's own chord
/// tool (`crate::chords::chord_for_cell`, `harmonise`) already caps a step at
/// `MAX_CHORD_NOTES` — four — and applies that *one* number to a DT2 track
/// and a DN2 track alike. `crate::chords`' own doc comment calls this "both
/// boxes' `spec.trig.max_notes`", and `chords::tests::the_four_note_cap_is_both_boxes_own_limit`
/// pins `dt2_spec().trig.max_notes == dn2_spec().trig.max_notes == MAX_CHORD_NOTES`
/// — so the two device profiles genuinely agree at the protocol layer too,
/// and there is no case today where a per-device cap here would differ from
/// the uniform one already in use. Following the existing precedent exactly,
/// as the brief asks, means this module never asks which device it is
/// truncating for.
pub const MAX_CHORD_NOTES_PRECEDENT: usize = MAX_CHORD_NOTES;

/// One step's chord that did not survive a paste, and what became of it.
/// Mirrors `protocol::copy_track::ChordDrop` in shape; a separate type because
/// the note types differ (this crate's `Note::step` is `f64`, protocol's is a
/// `u8` trig index).
#[derive(Debug, Clone, PartialEq)]
pub struct ChordDrop {
    pub step: f64,
    /// The notes that survived, in rank order (highest velocity first).
    pub kept: Vec<Note>,
    /// The notes that did not, in the same order.
    pub dropped: Vec<Note>,
}

/// Keep at most `max_notes` per step, dropping and reporting the rest.
///
/// Same tie-break as `protocol::copy_track::truncate_chords`, for the same
/// reason: highest velocity survives — the notes carrying the chord — and a
/// tie keeps the lower pitch, so a voicing loses its top extensions before its
/// root. Notes come back sorted by `(step, pitch)`.
pub fn truncate_chords(notes: &[Note], max_notes: usize) -> (Vec<Note>, Vec<ChordDrop>) {
    let mut by_step: std::collections::BTreeMap<u64, Vec<&Note>> = std::collections::BTreeMap::new();
    for note in notes {
        // `f64` has no `Ord`; steps are trig positions and land on whole
        // numbers in every case this app produces one, so the bits are stable
        // to key on. A NaN step cannot occur — nothing in this crate builds
        // one — and if it ever did, grouping it apart from every other step
        // is the safe failure, not a panic.
        by_step.entry(note.step.to_bits()).or_default().push(note);
    }

    let mut kept: Vec<Note> = Vec::new();
    let mut drops = Vec::new();
    for group in by_step.into_values() {
        if group.len() <= max_notes {
            kept.extend(group.into_iter().cloned());
            continue;
        }
        let mut ranked = group;
        ranked.sort_by(|a, b| b.velocity.cmp(&a.velocity).then(a.pitch.cmp(&b.pitch)));
        let step = ranked[0].step;
        drops.push(ChordDrop {
            step,
            kept: ranked[..max_notes].iter().map(|n| (*n).clone()).collect(),
            dropped: ranked[max_notes..].iter().map(|n| (*n).clone()).collect(),
        });
        kept.extend(ranked[..max_notes].iter().map(|n| (*n).clone()));
    }
    kept.sort_by(|a, b| a.step.partial_cmp(&b.step).unwrap_or(std::cmp::Ordering::Equal).then(a.pitch.cmp(&b.pitch)));
    (kept, drops)
}

/// Human-readable lines for whatever [`truncate_chords`] had to drop. Steps
/// are 1-based, matching the box's own count and `protocol::copy_track`'s
/// `describe_chord_drops`.
pub fn describe_chord_drops(drops: &[ChordDrop]) -> Vec<String> {
    drops
        .iter()
        .map(|d| {
            let notes = d
                .dropped
                .iter()
                .map(|n| format!("note {} (vel {})", n.pitch, n.velocity))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "step {}: a trig holds {} notes at most, so {notes} {} dropped",
                d.step as i64 + 1,
                d.kept.len(),
                if d.dropped.len() == 1 { "was" } else { "were" },
            )
        })
        .collect()
}

/// A named or resolvable canonical parameter name for `lane`, read against
/// `lane_kind`'s own curated table.
///
/// **Usually a no-op.** `import::lane_to_model` already resolves a curated
/// lane's name at fetch time, so `lane.name` is normally already `Some` for
/// anything this can translate. This exists for the lane that got here some
/// other way — hand-built, or read out of a hand-edited project file — with a
/// `param_id` but no name, the same gap `param_by_plock_id` closes for
/// `protocol::copy_track`.
fn canonical_name(lane: &PLockLane, lane_kind: &str) -> Option<String> {
    lane.name.clone().or_else(|| {
        lane.param_id
            .and_then(|id| param_by_plock_id(param_table_for(lane_kind), id))
            .map(|p| p.name.to_string())
    })
}

/// Translate one track's p-lock lanes for a paste onto a `target_kind` box.
///
/// The rule is `protocol::copy_track::plock_lanes_for_target`'s: **a lane
/// crosses by canonical parameter name, never by its raw `paramId`** — 74 is
/// filter frequency on a DN2 and overdrive on a DT2 — and a lane that cannot
/// be named against the target's own table is **dropped and reported**,
/// never guessed at.
///
/// What is different from that function, and why: it rescales every value
/// through both boxes' stored-word scaling, because a `PoolLane`'s `values`
/// are the box's own uint16. `core::model::PLockLane::values` are already on
/// the shared MIDI 0–127 display axis (see this module's header comment), so
/// a curated lane's values are carried **unchanged** — same numbers, new
/// `name`/`device_kind`. Only the identity of the lane moves, not its data.
///
/// **Same-kind copies short-circuit to a straight carry**, exactly as the
/// protocol version does, and for a stronger reason here: an unnamed, raw
/// lane (`name: None`, only a `param_id`) is meaningless on a different box's
/// numbering and cannot be translated at all, but it is completely valid
/// carried onto another slot of the *same* kind of box, so the short-circuit
/// is not an optimisation, it is the only path a raw lane has.
fn plock_lanes_for_target(lanes: &[PLockLane], source_kind: &str, target_kind: &str) -> (Vec<PLockLane>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();

    for lane in lanes {
        let lane_kind = lane.device_kind.as_deref().unwrap_or(source_kind);
        if lane_kind == target_kind {
            let mut carried = lane.clone();
            carried.device_kind = Some(target_kind.to_string());
            out.push(carried);
            continue;
        }

        let Some(name) = canonical_name(lane, lane_kind) else {
            let what = match lane.param_id {
                Some(id) => format!("parameter 0x{id:02x}"),
                None => "a parameter".to_string(),
            };
            warnings.push(format!(
                "a p-lock lane on {lane_kind} {what} wasn't copied — digi-roll doesn't know which \
                 parameter that is yet, so it can't say what it would be on a {target_kind}"
            ));
            continue;
        };

        // Only presence and writability matter here — the id itself is
        // resolved fresh against the target's table when this lane is
        // eventually written, exactly as `export::lanes_for_device` already
        // does for a lane authored by name.
        if param_by_name(param_table_for(target_kind), &name).filter(|p| p.writable()).is_none() {
            warnings.push(format!(
                "the “{name}” lane wasn't copied — the {target_kind} has no equivalent parameter"
            ));
            continue;
        }
        out.push(
            PLockLane::new(
                Some(name),
                None,
                Some(target_kind.to_string()),
                lane.trigless,
                lane.values.clone(),
            )
            .expect("a name was just supplied, so this lane cannot be nameless"),
        );
    }
    (out, warnings)
}

/// A whole track's music, lifted out of the session — the clipboard's
/// contents. `source_kind` is the device model key ("DT2"/"DN2") the source
/// track's box carries, needed only if the paste crosses to a different kind
/// of box; a same-kind paste never consults it beyond the lane short-circuit.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackClip {
    pub source_kind: String,
    pub notes: Vec<Note>,
    pub plocks: Vec<PLockLane>,
    pub length_steps: u16,
    pub scale: TrackScale,
    pub track_prob: u8,
}

impl TrackClip {
    /// Lift what travels out of `track`. Everything else — `out_port`,
    /// `channel`, `mute`, `solo`, `level`, `name`, `patch` — is the destination's own
    /// and is never read here, so there is nothing in this type that could
    /// leak it into a paste by accident.
    pub fn copy_from(track: &Track, source_kind: impl Into<String>) -> Self {
        Self {
            source_kind: source_kind.into(),
            notes: track.notes.clone(),
            plocks: track.plocks.clone(),
            length_steps: track.length_steps,
            scale: track.scale,
            track_prob: track.track_prob,
        }
    }
}

/// Everything one paste did, in words a person can read as well as numbers a
/// caller can count.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PasteReport {
    pub notes_pasted: usize,
    pub lanes_pasted: usize,
    pub chord_drops: Vec<ChordDrop>,
    /// Everything that did not cross, in words. Note problems first (past the
    /// end of a shorter target, then chord drops), then lane problems — the
    /// same chords-then-lanes order `protocol::copy_track::CopyResult`
    /// documents.
    pub warnings: Vec<String>,
}

impl PasteReport {
    /// Whether anything worth a status line happened at all. A paste of an
    /// empty clip onto an empty track is a real paste — it still replaces
    /// `length_steps`/`scale`/`track_prob` — but nothing about it is worth a
    /// consequence line if there is nothing to report and nothing was
    /// written. Kept for symmetry with `Harmonised::is_empty`; the UI decides
    /// on its own whether to call this at all.
    pub fn has_anything_to_say(&self) -> bool {
        !self.warnings.is_empty()
    }
}

/// Paste `clip` onto `target`, replacing exactly the fields
/// [`TrackClip::copy_from`] read and nothing else.
///
/// `target_kind` is the destination box's device model key, used only to
/// translate p-lock lanes when it differs from `clip.source_kind`.
/// `target_max_steps` is the destination device's `DeviceModel::max_steps` —
/// today always 128, since DT2 and DN2 share that figure, but taken as a
/// parameter rather than the constant so a future smaller-stepped device (a
/// case `PLAN.md` §2 already reserves room for) degrades to a reported drop
/// instead of a track carrying a note past its own end. No test built off
/// today's two device profiles can reach that branch — both cap at 128 — so
/// the coverage for it constructs the shorter length by hand, the same
/// answer `DEVELOPMENT.md` lesson 6 gives for a claim nothing today can
/// exercise honestly.
pub fn paste_track(clip: &TrackClip, target: &mut Track, target_kind: &str, target_max_steps: u16) -> PasteReport {
    let length_steps = clip.length_steps.clamp(1, target_max_steps.max(1));

    let mut off_end = 0usize;
    let mut notes: Vec<Note> = clip
        .notes
        .iter()
        .filter(|n| {
            let fits = n.step < f64::from(length_steps);
            if !fits {
                off_end += 1;
            }
            fits
        })
        .cloned()
        .collect();
    for note in &mut notes {
        // A fresh identity for every pasted note — the same reason
        // `ClipNote::into_note` reissues one: two notes sharing an id makes a
        // selection ambiguous, and these are landing in a different track
        // than the one that still holds the originals.
        note.reissue_id();
    }

    let (kept, drops) = truncate_chords(&notes, MAX_CHORD_NOTES_PRECEDENT);

    let mut warnings = Vec::new();
    if off_end > 0 {
        warnings.push(format!(
            "{off_end} note{} past step {length_steps} of the target track {} not copied — the \
             destination is shorter than the source",
            if off_end == 1 { "" } else { "s" },
            if off_end == 1 { "was" } else { "were" },
        ));
    }
    warnings.extend(describe_chord_drops(&drops));

    let (lanes, lane_warnings) = plock_lanes_for_target(&clip.plocks, &clip.source_kind, target_kind);
    warnings.extend(lane_warnings);

    let notes_pasted = kept.len();
    let lanes_pasted = lanes.len();
    target.notes = kept;
    target.plocks = lanes;
    target.length_steps = length_steps;
    target.scale = clip.scale;
    target.track_prob = clip.track_prob;

    PasteReport {
        notes_pasted,
        lanes_pasted,
        chord_drops: drops,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackKind;

    fn note(step: f64, pitch: u8, velocity: u8) -> Note {
        Note::new(step, pitch, 1.0, velocity, 0.0)
    }

    fn lane(name: Option<&str>, param_id: Option<u16>, kind: &str, at: &[(usize, u16)]) -> PLockLane {
        let mut values = vec![None; 128];
        for &(step, v) in at {
            values[step] = Some(v);
        }
        PLockLane::new(name.map(String::from), param_id, Some(kind.to_string()), false, values).unwrap()
    }

    // --- copy_from: exactly what travels, nothing else -----------------------

    #[test]
    fn copy_from_carries_music_and_nothing_studio_side() {
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![note(0.0, 60, 100)];
        track.plocks = vec![lane(Some("filter.cutoff"), None, "DT2", &[(0, 64)])];
        track.length_steps = 32;
        track.scale = TrackScale::Two;
        track.track_prob = 80;
        track.name = "Kick".into();
        track.channel = 5;
        track.mute = true;
        track.solo = true;
        track.out_port = Some("out-1".into());

        let clip = TrackClip::copy_from(&track, "DT2");
        assert_eq!(clip.notes, track.notes);
        assert_eq!(clip.plocks, track.plocks);
        assert_eq!(clip.length_steps, 32);
        assert_eq!(clip.scale, TrackScale::Two);
        assert_eq!(clip.track_prob, 80);
        assert_eq!(clip.source_kind, "DT2");
    }

    #[test]
    fn a_paste_never_touches_the_destination_s_routing_or_identity() {
        let mut source = Track::new(0, TrackKind::Audio);
        source.notes = vec![note(0.0, 60, 100)];
        let clip = TrackClip::copy_from(&source, "DT2");

        let mut dest = Track::new(1, TrackKind::Audio);
        dest.name = "Snare".into();
        dest.channel = 9;
        dest.mute = true;
        dest.solo = true;
        dest.out_port = Some("dest-port".into());

        paste_track(&clip, &mut dest, "DT2", 128);

        // The plant this test would catch: `paste_track` assigning `*dest =`
        // wholesale from a track built out of the clip, which would silently
        // hand the destination the source's name, channel, mute/solo state
        // and port — exactly the fields the brief says belong to the target.
        assert_eq!(dest.name, "Snare");
        assert_eq!(dest.channel, 9);
        assert!(dest.mute);
        assert!(dest.solo);
        assert_eq!(dest.out_port.as_deref(), Some("dest-port"));
        // And the music did travel.
        assert_eq!(dest.notes.len(), 1);
        assert_eq!(dest.notes[0].pitch, 60);
    }

    #[test]
    fn pasted_notes_get_fresh_ids_not_the_source_s() {
        let mut source = Track::new(0, TrackKind::Audio);
        source.notes = vec![note(0.0, 60, 100)];
        let source_id = source.notes[0].id;
        let clip = TrackClip::copy_from(&source, "DT2");

        let mut dest = Track::new(1, TrackKind::Audio);
        paste_track(&clip, &mut dest, "DT2", 128);

        assert_ne!(dest.notes[0].id, source_id, "a pasted note is not the same identity as the source's");
    }

    // --- chord width: the precedent this module follows -----------------------

    #[test]
    fn the_chord_cap_this_module_uses_is_the_roll_s_own_four_note_cap() {
        // Pins the precedent claim in this module's header rather than a bare
        // number, so a future change to `MAX_CHORD_NOTES` cannot silently
        // un-follow the roll's own limit.
        assert_eq!(MAX_CHORD_NOTES_PRECEDENT, MAX_CHORD_NOTES);
    }

    #[test]
    fn a_paste_truncates_a_fat_chord_to_four_notes_and_reports_it() {
        let mut source = Track::new(0, TrackKind::Audio);
        // Five notes on one step: more than either box's trig can hold.
        source.notes = vec![
            note(0.0, 60, 100),
            note(0.0, 64, 90),
            note(0.0, 67, 80),
            note(0.0, 72, 127),
            note(0.0, 76, 10),
        ];
        let clip = TrackClip::copy_from(&source, "DT2");
        let mut dest = Track::new(1, TrackKind::Audio);

        let report = paste_track(&clip, &mut dest, "DT2", 128);

        assert_eq!(dest.notes.len(), 4, "the trig this lands on cannot hold five notes");
        assert_eq!(report.notes_pasted, 4);
        // Highest velocity survives; the quietest (vel 10) is the one that goes.
        assert!(dest.notes.iter().all(|n| n.pitch != 76));
        assert_eq!(report.chord_drops.len(), 1);
        assert_eq!(report.chord_drops[0].dropped[0].pitch, 76);
        assert!(
            report.warnings.iter().any(|w| w.contains("note 76") && w.contains("step 1")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn a_tie_on_velocity_keeps_the_lower_pitch() {
        let mut source = Track::new(0, TrackKind::Audio);
        source.notes = vec![note(0.0, 60, 100), note(0.0, 64, 100), note(0.0, 67, 100), note(0.0, 72, 100), note(0.0, 48, 100)];
        let clip = TrackClip::copy_from(&source, "DT2");
        let mut dest = Track::new(1, TrackKind::Audio);
        paste_track(&clip, &mut dest, "DT2", 128);

        let pitches: Vec<u8> = {
            let mut p: Vec<u8> = dest.notes.iter().map(|n| n.pitch).collect();
            p.sort();
            p
        };
        // 72 is the highest pitch of an all-tied group of five, so it is the
        // one dropped; the four lowest survive.
        assert_eq!(pitches, vec![48, 60, 64, 67]);
    }

    #[test]
    fn a_chord_at_the_cap_exactly_is_not_touched() {
        let mut source = Track::new(0, TrackKind::Audio);
        source.notes = vec![note(0.0, 60, 100), note(0.0, 64, 100), note(0.0, 67, 100), note(0.0, 72, 100)];
        let clip = TrackClip::copy_from(&source, "DT2");
        let mut dest = Track::new(1, TrackKind::Audio);
        let report = paste_track(&clip, &mut dest, "DT2", 128);
        assert_eq!(dest.notes.len(), 4);
        assert!(report.chord_drops.is_empty());
        assert!(report.warnings.is_empty());
    }

    // --- p-lock lanes: same-kind carry, cross-device translate, cross-device drop --

    #[test]
    fn a_same_kind_paste_carries_an_unnamed_raw_lane_untouched() {
        // A lane with only a paramId is meaningless on a different box, but
        // perfectly valid landing on another slot of the *same* kind of box —
        // the only path a raw lane has, per this module's header comment.
        let mut source = Track::new(0, TrackKind::Audio);
        source.plocks = vec![lane(None, Some(200), "DT2", &[(3, 500)])];
        let clip = TrackClip::copy_from(&source, "DT2");
        let mut dest = Track::new(1, TrackKind::Audio);

        let report = paste_track(&clip, &mut dest, "DT2", 128);

        assert_eq!(dest.plocks.len(), 1);
        assert_eq!(dest.plocks[0].param_id, Some(200));
        assert_eq!(dest.plocks[0].values[3], Some(500));
        assert_eq!(report.lanes_pasted, 1);
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn a_cross_device_paste_translates_a_named_lane_by_name_with_no_rescale() {
        // filter.cutoff is curated on both tables at the same 0-127 display
        // axis, so the value crosses unchanged — the whole point of the
        // "already on the display axis" argument in this module's header.
        let mut source = Track::new(0, TrackKind::Audio);
        source.plocks = vec![lane(Some("filter.cutoff"), None, "DT2", &[(5, 90)])];
        let clip = TrackClip::copy_from(&source, "DT2");
        let mut dest = Track::new(1, TrackKind::Audio);

        let report = paste_track(&clip, &mut dest, "DN2", 128);

        assert_eq!(dest.plocks.len(), 1);
        assert_eq!(dest.plocks[0].name.as_deref(), Some("filter.cutoff"));
        assert_eq!(dest.plocks[0].device_kind.as_deref(), Some("DN2"));
        assert_eq!(dest.plocks[0].values[5], Some(90), "the display-axis value is unchanged");
        assert_eq!(report.lanes_pasted, 1);
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn a_cross_device_paste_drops_an_unnamed_raw_lane_and_says_so() {
        // The case this module's header calls out: an unnamed lane cannot be
        // translated to a different box's numbering at all.
        let mut source = Track::new(0, TrackKind::Audio);
        source.plocks = vec![lane(None, Some(200), "DT2", &[(0, 10)])];
        let clip = TrackClip::copy_from(&source, "DT2");
        let mut dest = Track::new(1, TrackKind::Audio);

        let report = paste_track(&clip, &mut dest, "DN2", 128);

        assert!(dest.plocks.is_empty(), "nothing crossed");
        assert_eq!(report.lanes_pasted, 0);
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(report.warnings[0].contains("wasn't copied"), "{}", report.warnings[0]);
        assert!(report.warnings[0].contains("DN2"));
    }

    #[test]
    fn a_cross_device_paste_of_a_lane_with_no_target_equivalent_drops_and_says_so() {
        // A named lane whose canonical name simply is not in the target's
        // table at all — constructed by hand, since today's two curated
        // tables happen to share every name. `param_by_name` failing for a
        // present, non-empty table is the branch this exercises.
        let mut source = Track::new(0, TrackKind::Audio);
        source.plocks = vec![lane(Some("not.a.real.parameter"), None, "DT2", &[(0, 10)])];
        let clip = TrackClip::copy_from(&source, "DT2");
        let mut dest = Track::new(1, TrackKind::Audio);

        let report = paste_track(&clip, &mut dest, "DN2", 128);

        assert!(dest.plocks.is_empty());
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("no equivalent parameter"), "{}", report.warnings[0]);
    }

    // --- length: the target's own limit, and what does not fit it -------------

    #[test]
    fn pasting_onto_a_shorter_target_clamps_length_and_drops_and_reports_off_end_notes() {
        // Unreachable through today's UI or device table — DT2 and DN2 both
        // cap at 128 — so this calls `paste_track` with a hand-supplied
        // shorter limit, the way this module's doc comment on
        // `target_max_steps` says to.
        let mut source = Track::new(0, TrackKind::Audio);
        source.length_steps = 32;
        source.notes = vec![note(2.0, 60, 100), note(10.0, 64, 100)];
        let clip = TrackClip::copy_from(&source, "DT2");
        let mut dest = Track::new(1, TrackKind::Audio);

        let report = paste_track(&clip, &mut dest, "DT2", 8);

        assert_eq!(dest.length_steps, 8);
        assert_eq!(dest.notes.len(), 1, "the note at step 10 does not fit an 8-step track");
        assert_eq!(dest.notes[0].pitch, 60);
        assert_eq!(report.notes_pasted, 1);
        assert!(report.warnings.iter().any(|w| w.contains("1 note") && w.contains("shorter")), "{:?}", report.warnings);
    }

    #[test]
    fn scale_and_track_prob_travel_with_the_notes() {
        let mut source = Track::new(0, TrackKind::Audio);
        source.scale = TrackScale::Half;
        source.track_prob = 42;
        let clip = TrackClip::copy_from(&source, "DT2");
        let mut dest = Track::new(1, TrackKind::Audio);
        dest.scale = TrackScale::Two;
        dest.track_prob = 100;

        paste_track(&clip, &mut dest, "DT2", 128);

        assert_eq!(dest.scale, TrackScale::Half);
        assert_eq!(dest.track_prob, 42);
    }

    #[test]
    fn pasting_an_empty_clip_clears_the_destination_s_music() {
        // A copy of a genuinely empty track is still a paste: the destination
        // ends up empty too, which is different from a no-op (the UI layer's
        // job is to refuse to call this at all for a same-cell paste).
        let source = Track::new(0, TrackKind::Audio);
        let clip = TrackClip::copy_from(&source, "DT2");
        let mut dest = Track::new(1, TrackKind::Audio);
        dest.notes = vec![note(0.0, 60, 100)];

        let report = paste_track(&clip, &mut dest, "DT2", 128);

        assert!(dest.notes.is_empty());
        assert_eq!(report.notes_pasted, 0);
        assert!(!report.has_anything_to_say());
    }
}
