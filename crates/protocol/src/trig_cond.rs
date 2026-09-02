//! The three per-step trig lanes — PROB, FILL, COND — and the track's own PROB
//! default, read off and written into a pattern-kit payload.
//!
//! Port of `js/elektron/trig-cond.js`, both halves. `conditions.rs` is the
//! codec (byte ↔ value); this is where those bytes actually live. The read half
//! landed with the import path (Phase 5); the write half landed with Phase 6.
//!
//! Like [`crate::pattern_settings`], these compose *onto* a payload rather than
//! going through `decode_pattern_kit` — that path is hardware-verified and stays
//! untouched (PLAN.md §7 rule 3).
//!
//! Storage (hardware-mapped 2026-08-02, identical on DT2 and DN2 — the [V2]
//! sections of digi-roll's format docs): three 128-byte lanes inside the track
//! struct, one byte per step, `FF` meaning nothing stored.
//!
//! ```text
//!   track +256  COND   menu index 0-75
//!   track +384  FILL   01 ON / 00 OFF
//!   track +512  PROB   the percentage itself
//! ```
//!
//! The offsets come from each spec's `track.trig_cond/trig_fill/trig_prob`, so
//! nothing here hard-codes a number.
//!
//! These are pure functions over a payload plus a spec — no bytes travel to any
//! device from here. They compose with the hardware-verified encode in
//! `pattern.rs` rather than reaching into it (PLAN.md §7 rule 3): a write-path
//! caller runs `encode_track_notes` first and hands the fresh payload it
//! returned to [`apply_track_trig_settings`].

use std::collections::{BTreeMap, BTreeSet};

use crate::conditions::{
    cond_from_byte, cond_to_byte, fill_from_byte, fill_to_byte, prob_from_byte, prob_to_byte,
    NONE, PROB_MAX, PROB_MIN,
};
use crate::pattern::{Note, Spec};

/// What one step of one track has locked. All three are per *trig*: notes
/// sharing a step share these values, which is the rule
/// `digi_core::edit_ops::adopt_step_trig` upholds on the model side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrigSetting {
    pub prob: Option<u8>,
    pub fill: Option<bool>,
    pub cond: Option<&'static str>,
}

impl TrigSetting {
    /// Nothing locked. The three lanes hold `FF` for such a step, so the write
    /// path stores nothing and the UI shows nothing.
    pub fn is_default(&self) -> bool {
        self.prob.is_none() && self.fill.is_none() && self.cond.is_none()
    }
}

/// Byte offset of one step's byte in one of the three lanes.
fn lane_offset(spec: &Spec, lane_start: usize, track_index: usize, step: usize) -> usize {
    spec.pattern.tracks_offset + track_index * spec.track.size + lane_start + step
}

/// A byte off the payload, or the "nothing stored" sentinel past its end.
///
/// The same deviation from the JS that [`crate::pattern_settings::read_swing`]
/// takes, and for the same reason: indexing past a short payload gives
/// `undefined` in JS, which `probFromByte` passes straight through as a
/// probability. A truncated payload is unreadable in exactly the way an
/// out-of-range byte is, so it takes the same answer — nothing is stored here.
fn byte_at(payload: &[u8], offset: usize) -> u8 {
    payload.get(offset).copied().unwrap_or(NONE)
}

fn check_track(spec: &Spec, track_index: usize) -> Result<(), String> {
    if track_index >= spec.pattern.num_tracks {
        return Err(format!("no track {track_index}"));
    }
    Ok(())
}

/// Every stored trig setting on one track: step → what is locked there.
///
/// Steps whose three lane bytes are all "none" are left out, so a pattern with
/// no conditions yields an empty map. Steps are **not** filtered by whether
/// their trig is live: deleting a trig on the box clears its COND byte and
/// leaves FILL and PROB behind (verified on hardware — the DT2 fixture carries
/// exactly that), and the caller, which knows the live steps, decides what to
/// do with the leftovers. The import path only ever asks about steps that have
/// notes.
///
/// A `BTreeMap` rather than a `HashMap`, as everywhere else in this crate:
/// callers walk it and Rust randomises `HashMap` order per process.
pub fn read_track_trig_settings(
    spec: &Spec,
    payload: &[u8],
    track_index: usize,
) -> Result<BTreeMap<u8, TrigSetting>, String> {
    check_track(spec, track_index)?;
    let mut out = BTreeMap::new();
    for step in 0..spec.track.num_steps {
        let setting = setting_at(spec, payload, track_index, step);
        if !setting.is_default() {
            out.insert(step as u8, setting);
        }
    }
    Ok(out)
}

/// One step's stored setting, or `None` when nothing is stored there.
pub fn read_step_trig_setting(
    spec: &Spec,
    payload: &[u8],
    track_index: usize,
    step: usize,
) -> Result<Option<TrigSetting>, String> {
    check_track(spec, track_index)?;
    let setting = setting_at(spec, payload, track_index, step);
    Ok((!setting.is_default()).then_some(setting))
}

fn setting_at(spec: &Spec, payload: &[u8], track_index: usize, step: usize) -> TrigSetting {
    let t = &spec.track;
    TrigSetting {
        prob: prob_from_byte(byte_at(payload, lane_offset(spec, t.trig_prob, track_index, step))),
        fill: fill_from_byte(byte_at(payload, lane_offset(spec, t.trig_fill, track_index, step))),
        cond: cond_from_byte(byte_at(payload, lane_offset(spec, t.trig_cond, track_index, step))),
    }
}

/// One track's default probability, 0–100 — the other half of the hardware's
/// probability model: a trig with no PROB lock of its own runs at these odds.
///
/// Unlike the three per-step lanes there is no "nothing stored" case; the byte
/// is always a real percentage, 100 by default. An out-of-range byte reads as
/// 100 rather than erroring: it would mean the field has moved, and a pattern we
/// cannot fully read must still open — the rule `cond_from_byte` and
/// `read_swing` both follow.
pub fn read_track_prob(spec: &Spec, payload: &[u8], track_index: usize) -> Result<u8, String> {
    check_track(spec, track_index)?;
    let offset =
        spec.pattern.tracks_offset + track_index * spec.track.size + spec.track.track_prob;
    Ok(match payload.get(offset) {
        Some(&byte) if byte <= PROB_MAX => byte,
        _ => PROB_MAX,
    })
}

// --- The write half ------------------------------------------------------------

/// One past the last byte this track's three lanes occupy — what a payload must
/// hold before a write into them can be honest.
fn lanes_end(spec: &Spec, track_index: usize) -> usize {
    let t = &spec.track;
    [t.trig_cond, t.trig_fill, t.trig_prob]
        .into_iter()
        .map(|lane| lane_offset(spec, lane, track_index, t.num_steps))
        .max()
        .unwrap()
}

/// Write one track's trig settings into a payload, in place.
///
/// Callers pass the fresh copy [`crate::pattern::encode_track_notes`] returned,
/// so this is the only mutation of an already-cloned buffer.
///
/// Every one of the track's steps is cleared to `FF` first. That is not
/// tidiness — the box scrubs these lanes when *it* creates a trig, and a write
/// path that bypasses trig creation has to do the same, or a fresh trig
/// silently inherits a deleted one's probability. Verified on hardware:
/// deleting a trig clears its COND byte but leaves FILL and PROB behind, and
/// the DT2 condition fixture carries exactly that leftover on step 16.
///
/// Steps past the track's `num_steps` are skipped, as the JS skips them.
/// Nothing outside this track's three lanes is touched.
///
/// Two deviations from the JS, both from the types rather than the behaviour:
/// a payload too short to hold this track's lanes is refused, where a JS typed
/// array silently drops writes past its end — a write that half-lands is worse
/// than one that refuses. And an unknown COND label is an error mid-write, as
/// the JS throws mid-write; either way the caller discards the buffer, which is
/// a fresh clone every time.
pub fn apply_track_trig_settings(
    spec: &Spec,
    payload: &mut [u8],
    track_index: usize,
    by_step: &BTreeMap<u8, TrigSetting>,
) -> Result<(), String> {
    check_track(spec, track_index)?;
    if payload.len() < lanes_end(spec, track_index) {
        return Err(format!(
            "payload too short ({} bytes) to hold track {}'s trig-condition lanes",
            payload.len(),
            track_index + 1
        ));
    }
    let t = &spec.track;
    for lane in [t.trig_cond, t.trig_fill, t.trig_prob] {
        let start = lane_offset(spec, lane, track_index, 0);
        payload[start..start + t.num_steps].fill(NONE);
    }
    for (&step, setting) in by_step {
        if step as usize >= t.num_steps || setting.is_default() {
            continue;
        }
        let cond = cond_to_byte(setting.cond).map_err(|e| e.to_string())?;
        payload[lane_offset(spec, t.trig_cond, track_index, step as usize)] = cond;
        payload[lane_offset(spec, t.trig_fill, track_index, step as usize)] =
            fill_to_byte(setting.fill);
        payload[lane_offset(spec, t.trig_prob, track_index, step as usize)] =
            prob_to_byte(setting.prob);
    }
    Ok(())
}

/// Write one track's default probability into a payload, in place. Exactly one
/// byte moves. `None` means "the box default", 100 — there is no way to store
/// "unset", so a pattern that never met a box writes the same value the box
/// would already be holding, and the diff stays empty.
///
/// Values over 100 clamp to 100, as in the JS. (The JS also rounds fractional
/// input; `u8` makes that case unwritable here.)
pub fn apply_track_prob(
    spec: &Spec,
    payload: &mut [u8],
    track_index: usize,
    prob: Option<u8>,
) -> Result<(), String> {
    check_track(spec, track_index)?;
    let offset =
        spec.pattern.tracks_offset + track_index * spec.track.size + spec.track.track_prob;
    let byte = payload.get_mut(offset).ok_or_else(|| {
        format!(
            "payload too short to hold track {}'s PROB default (a byte at offset {offset})",
            track_index + 1
        )
    })?;
    *byte = prob.map_or(PROB_MAX, |p| p.clamp(PROB_MIN, PROB_MAX));
    Ok(())
}

/// Notes → the per-step settings the write path stores.
///
/// All three fields are per trig, so a step's value comes from its **first**
/// note in the encoder's own `(step, pitch)` order — mirroring exactly how
/// `encode_track_notes` takes velocity/length/micro from the first note of a
/// chord. The first note wins even when its setting is all-default, and steps
/// where it is are left out, so they stay `FF`.
///
/// The input is pairs rather than notes because the Rust encoder note does not
/// carry the trio the JS note does — `pattern::Note` is the hardware-verified
/// encode shape and stays untouched (PLAN.md §7 rule 3). A caller walks its own
/// model notes once and hands each encoder note over with its setting.
pub fn trig_settings_from_notes(notes: &[(Note, TrigSetting)]) -> BTreeMap<u8, TrigSetting> {
    let mut sorted: Vec<&(Note, TrigSetting)> = notes.iter().collect();
    sorted.sort_by(|a, b| a.0.step.cmp(&b.0.step).then(a.0.pitch.cmp(&b.0.pitch)));
    let mut seen = BTreeSet::new();
    let mut out = BTreeMap::new();
    for (note, setting) in sorted {
        if !seen.insert(note.step) || setting.is_default() {
            continue;
        }
        out.insert(note.step, *setting);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::{dn2_spec, dt2_spec};

    #[test]
    fn a_payload_too_short_to_hold_the_lanes_reads_as_nothing_stored() {
        for spec in [dt2_spec(), dn2_spec()] {
            assert_eq!(read_track_trig_settings(&spec, &[], 0), Ok(BTreeMap::new()));
            assert_eq!(read_step_trig_setting(&spec, &[], 0, 0), Ok(None));
            assert_eq!(read_track_prob(&spec, &[], 0), Ok(PROB_MAX));
        }
    }

    #[test]
    fn refuses_a_track_the_spec_does_not_have() {
        let spec = dt2_spec();
        let n = spec.pattern.num_tracks;
        assert!(read_track_trig_settings(&spec, &[], n).is_err());
        assert!(read_step_trig_setting(&spec, &[], n, 0).is_err());
        assert!(read_track_prob(&spec, &[], n).is_err());
    }

    // --- the write half. Every expectation below was derived by running the JS
    // under node first (`node /tmp/trig-write-derive.mjs` against the committed
    // fixtures; the recipe is written into tests/all/trig_write.rs's doc comment).

    fn blank(spec: &Spec) -> Vec<u8> {
        vec![0u8; spec.pattern.size]
    }

    fn note_at(step: u8, pitch: u8) -> Note {
        Note { step, pitch, velocity: 100, len_steps: 1.0, micro: 0.0 }
    }

    const LOCKED: TrigSetting =
        TrigSetting { prob: Some(10), fill: Some(true), cond: Some("PRE") };

    #[test]
    fn the_write_refuses_the_track_and_the_truncation_the_read_refuses() {
        for spec in [dt2_spec(), dn2_spec()] {
            let n = spec.pattern.num_tracks;
            let mut payload = blank(&spec);
            let by_step = BTreeMap::from([(0u8, LOCKED)]);
            assert!(apply_track_trig_settings(&spec, &mut payload, n, &by_step).is_err());
            assert!(apply_track_prob(&spec, &mut payload, n, Some(50)).is_err());
            // A payload the lanes do not fit in is refused, not half-written —
            // the deviation from the JS typed array documented on the function.
            let mut short = vec![0u8; 16];
            assert!(apply_track_trig_settings(&spec, &mut short, 0, &by_step).is_err());
            assert!(apply_track_prob(&spec, &mut short, 0, Some(50)).is_err());
            assert_eq!(short, vec![0u8; 16], "a refused write moved a byte");
        }
    }

    #[test]
    fn a_step_past_the_track_is_skipped_and_the_scrub_still_runs() {
        // Derived under node: applyTrackTrigSettings with only step 200 leaves
        // the track with no settings at all — the out-of-range step is skipped
        // and the scrub has cleared everything else.
        let spec = dt2_spec();
        let mut payload = blank(&spec);
        apply_track_trig_settings(&spec, &mut payload, 0, &BTreeMap::from([(0u8, LOCKED)]))
            .unwrap();
        apply_track_trig_settings(&spec, &mut payload, 0, &BTreeMap::from([(200u8, LOCKED)]))
            .unwrap();
        assert_eq!(read_track_trig_settings(&spec, &payload, 0), Ok(BTreeMap::new()));
    }

    #[test]
    fn an_unknown_condition_label_is_refused_rather_than_encoded() {
        let spec = dt2_spec();
        let mut payload = blank(&spec);
        let bad = TrigSetting { prob: None, fill: None, cond: Some("9:99") };
        let err = apply_track_trig_settings(&spec, &mut payload, 0, &BTreeMap::from([(0u8, bad)]))
            .unwrap_err();
        assert!(err.contains("9:99"), "{err}");
    }

    #[test]
    fn track_prob_writes_exactly_one_byte_with_the_js_clamp_and_default() {
        // Derived under node: 250 → 100, null → 100.
        let spec = dn2_spec();
        let mut payload = blank(&spec);
        apply_track_prob(&spec, &mut payload, 3, Some(250)).unwrap();
        assert_eq!(read_track_prob(&spec, &payload, 3), Ok(100));
        assert_eq!(payload.iter().filter(|&&b| b != 0).count(), 1, "exactly one byte moves");
        // The stored *byte* is the clamped 100, not the raw 250. The read alone
        // cannot pin this — it treats any out-of-range byte as 100, so an
        // unclamped write would read back correctly while putting a byte on the
        // box that the format never defines. Same escape class as the params
        // table's sentinel clamp (DEVELOPMENT.md, 2026-08-18).
        assert_eq!(payload.iter().copied().find(|&b| b != 0), Some(100), "the raw byte is clamped");
        apply_track_prob(&spec, &mut payload, 3, Some(30)).unwrap();
        assert_eq!(read_track_prob(&spec, &payload, 3), Ok(30));
        apply_track_prob(&spec, &mut payload, 3, None).unwrap();
        assert_eq!(read_track_prob(&spec, &payload, 3), Ok(100));
    }

    #[test]
    fn the_first_note_of_a_step_wins_even_when_its_setting_is_all_default() {
        // The JS chord case, derived under node: on step 3 the lower pitch is
        // first in (step, pitch) order and wins; on step 5 the first note is
        // all-default, so the step stays out even though a later note is locked.
        let notes = vec![
            (note_at(3, 72), TrigSetting { prob: Some(60), fill: Some(true), cond: Some("3:4") }),
            (note_at(3, 60), TrigSetting { prob: Some(99), fill: Some(false), cond: Some("PRE") }),
            (note_at(5, 40), TrigSetting::default()),
            (note_at(5, 41), TrigSetting { prob: Some(30), fill: None, cond: None }),
        ];
        let want = BTreeMap::from([(
            3u8,
            TrigSetting { prob: Some(99), fill: Some(false), cond: Some("PRE") },
        )]);
        assert_eq!(trig_settings_from_notes(&notes), want);
    }

    #[test]
    fn what_the_write_stores_the_read_gives_back() {
        for spec in [dt2_spec(), dn2_spec()] {
            let mut payload = blank(&spec);
            let by_step = BTreeMap::from([
                (0u8, TrigSetting { prob: Some(25), fill: Some(true), cond: Some("2:4") }),
                (1u8, TrigSetting { prob: None, fill: Some(false), cond: Some("!1ST") }),
                (2u8, TrigSetting { prob: Some(0), fill: None, cond: None }),
            ]);
            apply_track_trig_settings(&spec, &mut payload, 5, &by_step).unwrap();
            assert_eq!(read_track_trig_settings(&spec, &payload, 5), Ok(by_step), "{}", spec.device);
        }
    }
}
