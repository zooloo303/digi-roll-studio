//! Cross-device copy: one track's notes from any decoded pattern into any other
//! pattern, on the same box or a different one.
//!
//! Ported from `js/elektron/copy-track.js`. The note model is the interchange
//! format. Decode the source with its own [`Spec`], take [`track_notes`], hand
//! those notes to the target's [`encode_track_notes`] — there is deliberately no
//! bytes-level DT2↔DN2 converter, because the two pattern structs only look
//! alike; the note model is the thing both boxes genuinely agree on.
//!
//! **What crosses:** trig bits, note, velocity, length, micro-timing, the three
//! per-trig conditions (PROB/FILL/COND), the track's own PROB default (what
//! unlocked trigs run at) and the track's p-lock lanes. Sounds, kit and the
//! pattern's own settings belong to the target and are left exactly as they were
//! — this is the same read-modify-write of one track as
//! [`crate::safe_write`]'s, with the notes coming from somewhere else.
//!
//! **Nothing here sends a byte.** Like [`crate::trig_cond`]'s write half, these
//! are pure payload functions: they return a buffer and a caller has to hand it
//! to `safe_write_track` for anything to reach a box. Building this does not
//! create a write path.
//!
//! Two things can fail to cross, and both are **reported, never guessed at**:
//!
//! * **Chords.** A DT2 trig holds at most four note slots; a DN2 trig has no
//!   such limit, so a fat DN2 chord does not always fit. See
//!   [`truncate_chords`].
//! * **p-lock lanes.** A `paramId` is a number in one box's own numbering, so
//!   lanes cannot be carried byte-for-byte between boxes the way notes and
//!   conditions can. See [`plock_lanes_for_target`].
//!
//! Conditions need no cross-device policy: the DT2 and DN2 store them
//! identically and share one 76-value COND list (hardware-verified 2026-08-02),
//! so nothing can be dropped for want of a target-side equivalent. If that ever
//! stops being true, the place to say so is `warnings`, alongside chord drops —
//! loudly, never silently.
//!
//! ## Three deviations from the JS, all from the types
//!
//! 1. **`deviceNotesToEncoder` does not port.** The JS has two note shapes —
//!    `trackNotes`'s `lenSteps` and the encoder's `len` — and a function to
//!    convert. Rust has one [`Note`], returned by [`track_notes`] and accepted
//!    by [`encode_track_notes`], so there is nothing to convert and no chance of
//!    a field being dropped in the conversion.
//!
//! 2. **The source payload is required, not optional.** `copyTrack`'s
//!    `sourcePayload = null` mode silently carries *less*: no conditions, no
//!    PROB default, no p-lock lanes — a copy that looks like a copy and is not.
//!    A [`PatternKit`] only ever comes from [`decode_pattern_kit`], so every
//!    caller has the payload already, and making it a parameter rather than an
//!    option removes a way to get a quiet partial copy.
//!
//! 3. **Trig settings travel as pairs, not as fields on a note.** The JS
//!    mutates notes with `attachTrigSettings`; here a note is paired with its
//!    [`TrigSetting`], which is the shape [`trig_settings_from_notes`] already
//!    takes (PLAN.md §7 rule 3: `pattern::Note` is the hardware-verified encode
//!    shape and stays untouched).
//!
//! **Swing deliberately does not travel.** It belongs to the whole pattern, so
//! carrying it would let a one-track copy silently re-time the fifteen tracks
//! already in the target slot — the opposite of what this function promises. The
//! "send to box" path does write it, because there the roll's pattern *is* the
//! pattern; here the target is somebody else's.

use crate::params::{param_by_name, param_by_plock_id, param_table_for};
use crate::pattern::{decode_pattern_kit, encode_track_notes, track_notes, Note, PatternKit, Spec};
use crate::plocks::{apply_track_plocks, read_track_plocks, LaneWrite, PoolLane, FREE};
use crate::trig_cond::{
    apply_track_prob, apply_track_trig_settings, read_track_prob, read_track_trig_settings,
    trig_settings_from_notes, TrigSetting,
};

/// One step's chord that did not fit the target's trig, and what became of it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChordDrop {
    pub step: u8,
    /// The notes that survived, in rank order (highest velocity first).
    pub kept: Vec<Note>,
    /// The notes that did not, in the same order.
    pub dropped: Vec<Note>,
}

/// Everything one copy produced.
#[derive(Debug, Clone)]
pub struct CopyResult {
    /// The target payload with one track replaced. A fresh buffer; the input is
    /// never mutated.
    pub payload: Vec<u8>,
    /// The notes actually written, paired with the settings that went with them.
    pub notes: Vec<(Note, TrigSetting)>,
    /// Notes [`encode_track_notes`] itself could not place.
    ///
    /// **Provably 0 today, and no test in `tests/all/copy_track.rs` can witness a bug
    /// that forces it to 0** — said plainly because a deliberate-bug pass planted
    /// exactly that and the suite stayed green. Truncation runs first, with the
    /// target's own `trig.max_notes`, and both boxes hold 128 steps, so there is
    /// nothing left for the encoder to refuse.
    ///
    /// It stays because that proof rests on two facts about the *specs* rather
    /// than about this code: a target with fewer steps than its source, or a
    /// truncation limit read off the wrong spec, both put notes in here. The
    /// second of those is pinned by
    /// `the_truncation_limit_comes_from_the_target_not_the_source`; the first has
    /// no box to witness it. A non-zero value here means one of those two
    /// happened, and it is worth surfacing rather than swallowing.
    pub dropped: usize,
    pub drops: Vec<ChordDrop>,
    /// Everything a person needs to know about what did not cross, in words.
    /// Chord drops first, then lane problems.
    pub warnings: Vec<String>,
}

/// Keep at most `max_notes` per step, and report the rest.
///
/// When a step has more notes than the target's trig can hold, keep the
/// **highest-velocity** notes — they are the ones carrying the chord — and on a
/// tie keep the **lower pitches**, which keeps the root and body of the voicing
/// rather than the top extensions. The dropped notes are always returned: a
/// chord must never quietly lose a note.
///
/// Notes come back sorted by `(step, pitch)`, which is the order
/// [`encode_track_notes`] wants.
pub fn truncate_chords(
    notes: &[(Note, TrigSetting)],
    max_notes: usize,
) -> (Vec<(Note, TrigSetting)>, Vec<ChordDrop>) {
    // A step's notes, in the order given. `BTreeMap` because the drops are
    // walked in step order and `HashMap` would randomise it per process.
    let mut by_step: std::collections::BTreeMap<u8, Vec<&(Note, TrigSetting)>> =
        std::collections::BTreeMap::new();
    for pair in notes {
        by_step.entry(pair.0.step).or_default().push(pair);
    }

    let mut kept: Vec<(Note, TrigSetting)> = Vec::new();
    let mut drops = Vec::new();
    for (step, group) in by_step {
        if group.len() <= max_notes {
            kept.extend(group.into_iter().cloned());
            continue;
        }
        let mut ranked = group;
        // Highest velocity first; lower pitch wins a tie. `sort_by` is stable,
        // so notes equal on both keys stay in the order the source gave them.
        ranked.sort_by(|a, b| b.0.velocity.cmp(&a.0.velocity).then(a.0.pitch.cmp(&b.0.pitch)));
        drops.push(ChordDrop {
            step,
            kept: ranked[..max_notes].iter().map(|p| p.0.clone()).collect(),
            dropped: ranked[max_notes..].iter().map(|p| p.0.clone()).collect(),
        });
        kept.extend(ranked[..max_notes].iter().map(|&p| p.clone()));
    }
    kept.sort_by(|a, b| a.0.step.cmp(&b.0.step).then(a.0.pitch.cmp(&b.0.pitch)));
    (kept, drops)
}

/// Human-readable warning lines for whatever [`truncate_chords`] had to drop.
///
/// Steps are 1-based here and nowhere else in this crate, because these lines go
/// on a screen beside a box that counts its steps from 1.
pub fn describe_chord_drops(drops: &[ChordDrop], target_name: &str) -> Vec<String> {
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
                "step {}: {target_name} holds {} notes per trig, so {notes} {} dropped",
                d.step as usize + 1,
                d.kept.len(),
                if d.dropped.len() == 1 { "was" } else { "were" },
            )
        })
        .collect()
}

/// Translate one track's p-lock lanes from the source box's numbering into the
/// target's.
///
/// A `paramId` is meaningless without knowing which box: **74 is filter
/// frequency on a DN2 and overdrive on a DT2**, so carrying the number across
/// would move the wrong knob. Lanes are translated by *canonical parameter
/// name*: find what the source `paramId` means in the source's curated table,
/// look that name up in the target's, and rescale the values through both
/// descriptors — the two boxes may store the same knob differently.
///
/// A lane that cannot be translated is **dropped and reported**, never guessed
/// at. Since Phase 0 filled both curated tables (2026-08-04) that means a lane
/// whose `paramId` is not among the eleven measured entries, or a parameter one
/// box has and the other does not — the same policy as chord truncation, and for
/// the same reason: silently moving a lock onto the wrong knob is worse than not
/// moving it.
///
/// **Copying between two slots on the same box short-circuits to a straight
/// carry.** That is not just an optimisation: the round trip through the display
/// axis quantises to whole MIDI steps (see
/// [`crate::params::ParamDesc::display_from_stored`]), so a same-box copy that
/// went through the translation would throw away the box's sub-MIDI fine bits
/// for no reason at all.
pub fn plock_lanes_for_target(
    lanes: &[PoolLane],
    source_kind: &str,
    target_kind: &str,
) -> (Vec<LaneWrite>, Vec<String>) {
    if lanes.is_empty() {
        return (Vec::new(), Vec::new());
    }
    if source_kind == target_kind {
        return (
            lanes
                .iter()
                .map(|l| LaneWrite::new(l.param_id, l.values.clone()))
                .collect(),
            Vec::new(),
        );
    }

    let from_table = param_table_for(source_kind);
    let to_table = param_table_for(target_kind);
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for lane in lanes {
        let hex = format!("0x{:02x}", lane.param_id);
        // The source paramId has to resolve to a parameter we know, or there is
        // nothing to translate *by*. `param_by_plock_id` only matches measured
        // entries, so a match here means the source side is scalable too.
        let Some(from) = param_by_plock_id(from_table, u16::from(lane.param_id)) else {
            warnings.push(format!(
                "p-lock lane on {source_kind} parameter {hex} wasn't copied — digi-roll doesn't \
                 know which parameter that is yet, so it can't say what it would be on a \
                 {target_kind}"
            ));
            continue;
        };
        let to = param_by_name(to_table, from.name);
        let Some(to) = to.filter(|p| p.writable()) else {
            warnings.push(format!(
                "p-lock lane “{}” wasn't copied — {}",
                from.label,
                if to.is_some() {
                    format!("digi-roll hasn't measured where a {target_kind} stores it")
                } else {
                    format!("the {target_kind} has no equivalent parameter")
                }
            ));
            continue;
        };
        let (from_desc, to_desc) = (
            from.describe(crate::params::device_kind_key(source_kind)),
            to.describe(crate::params::device_kind_key(target_kind)),
        );
        // Unreachable from a curated table — `Param::validate` refuses an id
        // above 0xFE and a test walks both tables through it — but a silent
        // `as u8` here would turn a hypothetical 0x144 into 0x44, which is a
        // real parameter on a DT2. Refuse rather than narrow.
        let id = to_desc.plock.map(|p| p.id).unwrap_or(u16::from(FREE));
        if id >= u16::from(FREE) {
            warnings.push(format!(
                "p-lock lane “{}” wasn't copied — parameter number {id} is past the {} a \
                 pattern's lane can hold",
                from.label,
                FREE - 1,
            ));
            continue;
        }
        out.push(LaneWrite::new(
            id as u8,
            // Out of the source's stored words, onto the shared display axis,
            // then into the target's stored words — so a difference in either
            // box's scaling is handled rather than assumed away.
            lane.values
                .iter()
                .map(|v| {
                    v.and_then(|w| from_desc.display_from_stored(w))
                        .and_then(|display| to_desc.stored_from_display(f64::from(display)))
                })
                .collect(),
        ));
    }
    (out, warnings)
}

/// Read one track's notes out of an already-decoded source pattern, paired with
/// their per-trig settings and truncated to what the target's trig can hold.
///
/// The settings are attached **before** truncation, and every note on a step
/// gets the same values — which is what makes truncation safe: whichever notes
/// survive, the step's settings survive with them.
pub fn track_notes_for_target(
    source_spec: &Spec,
    source_kit: &PatternKit,
    source_payload: &[u8],
    source_track: usize,
    target_spec: &Spec,
) -> Result<(Vec<(Note, TrigSetting)>, Vec<ChordDrop>), String> {
    let settings = read_track_trig_settings(source_spec, source_payload, source_track)?;
    let paired: Vec<(Note, TrigSetting)> = track_notes(source_kit, source_track)
        .into_iter()
        .map(|n| {
            let setting = settings.get(&n.step).copied().unwrap_or_default();
            (n, setting)
        })
        .collect();
    Ok(truncate_chords(&paired, target_spec.trig.max_notes))
}

/// The whole copy: source pattern + track → a new target payload.
///
/// `target_payload` must be the **freshly fetched** bytes of the pattern being
/// written, per PLAN.md §7 rule 2 — this function reads it, modifies a clone and
/// hands it back, and stale bytes here would overwrite whatever the box has
/// gained since.
///
/// Pass the same [`Spec`] twice to copy between two patterns on one box.
#[allow(clippy::too_many_arguments)]
pub fn copy_track(
    source_spec: &Spec,
    source_kit: &PatternKit,
    source_payload: &[u8],
    source_track: usize,
    target_spec: &Spec,
    target_payload: &[u8],
    target_track: usize,
    target_name: &str,
) -> Result<CopyResult, String> {
    let (notes, drops) = track_notes_for_target(
        source_spec,
        source_kit,
        source_payload,
        source_track,
        target_spec,
    )?;

    let encoder_notes: Vec<Note> = notes.iter().map(|(n, _)| n.clone()).collect();
    let (mut payload, dropped) =
        encode_track_notes(target_spec, target_payload, target_track, &encoder_notes)?;

    // Everything below mutates the buffer `encode_track_notes` just returned — a
    // fresh clone, so the caller's `target_payload` is untouched throughout.
    apply_track_trig_settings(
        target_spec,
        &mut payload,
        target_track,
        &trig_settings_from_notes(&notes),
    )?;

    // The track's PROB default is part of how the copied trigs sound, so it
    // travels with them.
    apply_track_prob(
        target_spec,
        &mut payload,
        target_track,
        Some(read_track_prob(source_spec, source_payload, source_track)?),
    )?;

    // p-lock lanes likewise: part of how the track sounds, and they live in bytes
    // `decode_pattern_kit` does not surface. Translation happens before the write
    // so an untranslatable lane is reported instead of aimed at a guess, and
    // `apply_track_plocks` adds its own warning if the target's 80 lanes are
    // already full.
    let source_lanes = read_track_plocks(source_spec, source_payload, source_track)?;
    let (lanes, mut lane_warnings) =
        plock_lanes_for_target(&source_lanes, source_spec.device, target_spec.device);
    lane_warnings.extend(apply_track_plocks(
        target_spec,
        &mut payload,
        target_track,
        &lanes,
    )?);

    let mut warnings = describe_chord_drops(&drops, target_name);
    warnings.extend(lane_warnings);
    Ok(CopyResult {
        payload,
        notes,
        dropped,
        drops,
        warnings,
    })
}

/// [`copy_track`] from raw source bytes, decoding the source itself.
///
/// The convenience a caller wants when the source came off a box or out of a
/// `.syx` and has not been decoded yet. `copy_track` takes the decoded kit
/// because the transfer panel already has one and decoding 100 KB twice to copy
/// one track is waste.
#[allow(clippy::too_many_arguments)]
pub fn copy_track_from_bytes(
    source_spec: &Spec,
    source_payload: &[u8],
    source_track: usize,
    target_spec: &Spec,
    target_payload: &[u8],
    target_track: usize,
    target_name: &str,
) -> Result<CopyResult, String> {
    let kit = decode_pattern_kit(source_spec, source_payload)?;
    copy_track(
        source_spec,
        &kit,
        source_payload,
        source_track,
        target_spec,
        target_payload,
        target_track,
        target_name,
    )
}
