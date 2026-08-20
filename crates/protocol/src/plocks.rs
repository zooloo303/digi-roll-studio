//! The 80-lane p-lock pool: per-step parameter automation, read off a
//! pattern-kit payload.
//!
//! Port of the *read* half of `js/elektron/plocks.js`. [`crate::params`] is the
//! table that says what a `paramId` means and how its stored word scales; this
//! is where those words actually live.
//!
//! Like [`crate::trig_cond`] and [`crate::pattern_settings`], these compose
//! *onto* a payload rather than going through `decode_pattern_kit` — that path
//! is hardware-verified and stays untouched (PLAN.md §7 rule 3).
//!
//! Layout, from `spec.pattern.{p_locks_index, num_p_locks, p_lock_size}` so
//! nothing here hard-codes a number:
//!
//! ```text
//!   80 lanes × 258 bytes, each lane:
//!     +0    param_id  u8    which parameter this lane automates, FF = free
//!     +1    track     u8    which track it belongs to, FF = free
//!     +2    128 × u16be, one value per step
//! ```
//!
//! A lane is keyed by `(param_id, track)`: one lane automates one parameter on
//! one track, so a track using four p-locked parameters holds four lanes, and
//! all sixteen tracks share the same pool of 80.
//!
//! ## What is measured, and what is inferred
//!
//! Every claim below is measured, and the committed fixtures are the evidence:
//!
//! * **A free lane is `FF FF` followed by 256 zero bytes** — not `FFFF` values,
//!   which is what both format docs originally said. Exactly 160 `FF`s and
//!   20480 zeros across the region, in all 128 patterns of the DT2 project dump
//!   and in every DN2 fixture.
//! * **The region ends exactly at `pattern.name_offset`** — 80 × 258 fills it
//!   with no slack, on both boxes.
//! * **Inside an allocated lane, `FFFF` marks a step with no lock.** This was an
//!   inference until the first real lane was captured (a DN2, 2026-08-04). It
//!   could never have been `0x0000`: zero is a legal value for most parameters.
//! * **A lane value is wider than 7 bits.** That first captured lane held
//!   `0x3F29`, just under NRPN's 14-bit ceiling, so a lane is not storing the
//!   0–127 number a CC would. [`crate::params`] has the scaling: the stored word
//!   is the display value × 256.
//! * **The box does not compact the pool.** It clears a freed lane in place and
//!   claims the lowest free lane including holes — visible in the DN2 Phase 0
//!   fixture, which holds lanes 0 and 2–10 with lane 1 free between them.
//!
//! ## The write half
//!
//! [`apply_track_plocks`] carries the same class of subtlety as
//! [`crate::trig_cond::apply_track_trig_settings`]: it scrubs before it writes,
//! per lane rather than wholesale, because the pool is shared with fifteen
//! other tracks — and a write that skips the scrub leaves a lock behind for the
//! next trig to inherit. Its policy, and the reason for each rule, is on that
//! function. It is composed onto a payload the same way, *after*
//! `encode_track_notes`, and it is what [`crate::safe_write::safe_write_track`]
//! reaches for when a caller has lanes to write.

use crate::pattern::Spec;

/// A lane header byte meaning "this lane is unused".
pub const FREE: u8 = 0xFF;

/// Per-step "no lock here" inside an allocated lane.
pub const NO_VALUE: u16 = 0xFFFF;

/// The largest value a step can hold, since [`NO_VALUE`] takes the top of the
/// range. The write path will need this; the reader states it because it is the
/// other half of what `NO_VALUE` means.
pub const VALUE_MAX: u16 = 0xFFFE;

/// One allocated lane, as the pool stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolLane {
    /// Which of the 80 lanes this is. Kept because the box does not compact the
    /// pool: a lane's index is where a write must put it back.
    pub lane: usize,
    /// The box's own page-ordered parameter index. Meaningless without knowing
    /// which box — see [`crate::params`].
    pub param_id: u8,
    pub track: u8,
    /// One entry per step, `None` where the step has no lock. **Stored words,
    /// not display values**: the scaling belongs to the parameter table, and a
    /// lane whose `param_id` is not curated has no scaling at all yet still has
    /// to survive a round trip byte-exact.
    pub values: Vec<Option<u16>>,
}

fn lane_start(spec: &Spec, lane: usize) -> usize {
    spec.pattern.p_locks_index + lane * spec.pattern.p_lock_size
}

fn check_track(spec: &Spec, track_index: usize) -> Result<(), String> {
    if track_index >= spec.pattern.num_tracks {
        return Err(format!("no track {track_index}"));
    }
    Ok(())
}

/// A stored word, or [`NO_VALUE`] past the end of the payload.
///
/// **A deliberate deviation from the JS**, which indexes past the end and reads
/// `undefined`, producing a *value of zero* rather than an absent one. Zero is a
/// legal lock. A truncated payload should lose the lock, not invent one — the
/// same rule `trig_cond` follows for its lanes, and the same reason: a pattern
/// we cannot fully read must still open.
fn word_at(payload: &[u8], offset: usize) -> u16 {
    match (payload.get(offset), payload.get(offset + 1)) {
        (Some(&hi), Some(&lo)) => u16::from_be_bytes([hi, lo]),
        _ => NO_VALUE,
    }
}

/// One lane, or `None` when the lane is free.
///
/// A lane past the end of the payload reads as free, for the reason [`word_at`]
/// gives.
pub fn read_lane(spec: &Spec, payload: &[u8], lane: usize) -> Option<PoolLane> {
    let o = lane_start(spec, lane);
    let (param_id, track) = (*payload.get(o)?, *payload.get(o + 1)?);
    if param_id == FREE && track == FREE {
        return None;
    }
    let values = (0..spec.track.num_steps)
        .map(|step| match word_at(payload, o + 2 + step * 2) {
            NO_VALUE => None,
            v => Some(v),
        })
        .collect();
    Some(PoolLane { lane, param_id, track, values })
}

/// Every allocated lane in the pattern, in lane order.
pub fn read_all_plocks(spec: &Spec, payload: &[u8]) -> Vec<PoolLane> {
    (0..spec.pattern.num_p_locks)
        .filter_map(|lane| read_lane(spec, payload, lane))
        .collect()
}

/// One track's lanes, in lane order.
///
/// A lane whose header is half-free (`param_id` set and `track` `FF`, or the
/// reverse) is not this track's business and is left for [`read_all_plocks`] to
/// report: this answers *what automation does track N carry*, and a malformed
/// lane carries none.
pub fn read_track_plocks(
    spec: &Spec,
    payload: &[u8],
    track_index: usize,
) -> Result<Vec<PoolLane>, String> {
    check_track(spec, track_index)?;
    Ok(read_all_plocks(spec, payload)
        .into_iter()
        .filter(|l| l.track as usize == track_index)
        .collect())
}

/// How many lanes are free right now — the budget a write has to fit inside.
pub fn free_lane_count(spec: &Spec, payload: &[u8]) -> usize {
    spec.pattern.num_p_locks - read_all_plocks(spec, payload).len()
}

/// Does this lane hold a value on a step with no trig?
///
/// A **trigless lock**. The box can hold them and v1 deliberately does not model
/// them, so such a lane is shown read-only and passed through byte-exact rather
/// than edited into a lie. `live_steps` is the steps that have trigs.
pub fn lane_has_trigless_values(lane: &PoolLane, live_steps: &[usize]) -> bool {
    lane.values
        .iter()
        .enumerate()
        .any(|(step, v)| v.is_some() && !live_steps.contains(&step))
}

// --- Writing -----------------------------------------------------------------

/// One lane a caller wants this track to end up with.
///
/// The write's counterpart to [`PoolLane`], and deliberately narrower: a caller
/// says *which parameter* and *what values*, never which of the 80 lanes — that
/// is [`apply_track_plocks`]'s to decide, because keeping a lane where the box
/// already put it is one of its rules. `track` is likewise absent: the write
/// names one track once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneWrite {
    pub param_id: u8,
    /// Stored words, `None` for a step with no lock. Short arrays leave the
    /// remaining steps unlocked, as the JS does.
    pub values: Vec<Option<u16>>,
}

impl LaneWrite {
    pub fn new(param_id: u8, values: Vec<Option<u16>>) -> Self {
        Self { param_id, values }
    }

    fn is_empty(&self) -> bool {
        !self.values.iter().any(Option::is_some)
    }
}

/// A lane read off a payload, asked for again as-is — what a caller does when it
/// is rewriting a track it just read.
impl From<&PoolLane> for LaneWrite {
    fn from(l: &PoolLane) -> Self {
        Self { param_id: l.param_id, values: l.values.clone() }
    }
}

/// Reset one lane to the form the boxes leave a never-used lane in: `FF FF` and
/// 256 zero bytes. Measured across every fixture; see the module doc.
fn free_lane(spec: &Spec, payload: &mut [u8], lane: usize) {
    let o = lane_start(spec, lane);
    payload[o] = FREE;
    payload[o + 1] = FREE;
    payload[o + 2..o + spec.pattern.p_lock_size].fill(0);
}

/// Write one lane's header and values.
fn write_lane(spec: &Spec, payload: &mut [u8], lane: usize, param_id: u8, track_index: usize, values: &[Option<u16>]) {
    let o = lane_start(spec, lane);
    payload[o] = param_id;
    payload[o + 1] = track_index as u8;
    for step in 0..spec.track.num_steps {
        // The one clamp that survives the port: `NO_VALUE` written as a *value*
        // would read back as an unlocked step, so the sentinel is not writable
        // even though it fits the type. See the note on `apply_track_plocks`.
        let word = match values.get(step).copied().flatten() {
            Some(v) => v.min(VALUE_MAX),
            None => NO_VALUE,
        };
        payload[o + 2 + step * 2..o + 4 + step * 2].copy_from_slice(&word.to_be_bytes());
    }
}

/// Write one track's p-lock lanes into a payload, in place.
///
/// Returns the warnings — written to be shown to the user verbatim, and the only
/// way this reports trouble. **A full pool is not an error**: a write that
/// cannot fit every lane should still land the notes, loudly. The `Err` is
/// reserved for a track this pattern does not have, which is a caller bug.
///
/// The policy, and why:
///
/// * A lane the track already has for the same `param_id` is **rewritten where
///   it is**. Whether the box cares about lane order is unknown, so the safest
///   write is the one that moves fewest bytes.
/// * A lane the track has for a `param_id` no longer wanted is **freed** to the
///   measured empty form.
/// * A new `param_id` claims the **lowest-numbered free lane**, recomputed after
///   the frees, so emptying one parameter and adding another reuses the slot.
/// * Lanes belonging to other tracks are never read, moved or written. A
///   one-track write must not disturb the other fifteen.
/// * A lane with no values at all is not allocated — an all-`FFFF` lane would
///   claim a slot to say nothing.
///
/// Like [`crate::trig_cond::apply_track_trig_settings`] this scrubs before it
/// writes, and for the same reason: a step that lost its trig must not leave a
/// lock behind for the next trig to inherit. Unlike that function the scrub is
/// per lane rather than wholesale, because the pool is shared.
///
/// **One clamp does not port, because the type refuses what the JS had to
/// tolerate.** `applyTrackPLocks` rounds and clamps each value into `0..=0xFFFE`
/// because a JS array can hold `-5` or `0x123456`; `Option<u16>` cannot, so only
/// the sentinel half of that clamp is left — a `Some(0xFFFF)` becomes
/// [`VALUE_MAX`] rather than being stored as "unlocked". That half is the one
/// that matters and it is pinned by a test: the read cannot tell a written
/// `0xFFFF` from an empty step, so an unclamped write would lose a lock silently.
pub fn apply_track_plocks(
    spec: &Spec,
    payload: &mut [u8],
    track_index: usize,
    lanes: &[LaneWrite],
) -> Result<Vec<String>, String> {
    check_track(spec, track_index)?;
    let region_end = spec.pattern.p_locks_index + spec.pattern.num_p_locks * spec.pattern.p_lock_size;
    if payload.len() < region_end {
        // The same refusal `apply_track_trig_settings` makes, for the same
        // reason: a JS typed array drops stores past its end, so the scrub would
        // land and the lanes would not — the worst of both.
        return Err(format!(
            "payload is {} bytes, too short for the p-lock pool ending at {region_end}",
            payload.len()
        ));
    }
    let mut warnings = Vec::new();

    // What we want this track to end up with, in the order asked for — a Vec
    // rather than a map because "one lane per parameter, in the order asked for"
    // is a tested property and a sorted map would quietly reorder it.
    let mut wanted: Vec<(u8, &[Option<u16>])> = Vec::new();
    for lane in lanes {
        if lane.param_id == FREE || lane.is_empty() {
            continue;
        }
        if wanted.iter().any(|(id, _)| *id == lane.param_id) {
            warnings.push(format!(
                "p-lock parameter {} appears twice for track {} — the box holds one lane per \
                 parameter per track, so only the first was written",
                lane.param_id,
                track_index + 1
            ));
            continue;
        }
        wanted.push((lane.param_id, &lane.values));
    }

    // Existing lanes: rewrite the ones still wanted, free the rest.
    let mut reused: Vec<u8> = Vec::new();
    for existing in read_track_plocks(spec, payload, track_index)? {
        match wanted.iter().find(|(id, _)| *id == existing.param_id) {
            Some(&(id, values)) if !reused.contains(&id) => {
                reused.push(id);
                write_lane(spec, payload, existing.lane, id, track_index, values);
            }
            _ => free_lane(spec, payload, existing.lane),
        }
    }

    // New parameters claim free lanes. Recomputed after the frees, so a
    // parameter that just went away hands its slot to one that just arrived.
    let taken: Vec<usize> = read_all_plocks(spec, payload).iter().map(|l| l.lane).collect();
    let mut free: Vec<usize> = (0..spec.pattern.num_p_locks).filter(|l| !taken.contains(l)).collect();
    free.reverse(); // so `pop` hands back the lowest

    let mut dropped: Vec<u8> = Vec::new();
    for &(param_id, values) in &wanted {
        if reused.contains(&param_id) {
            continue;
        }
        match free.pop() {
            Some(lane) => write_lane(spec, payload, lane, param_id, track_index, values),
            None => dropped.push(param_id),
        }
    }
    if !dropped.is_empty() {
        let n = dropped.len();
        let list = dropped.iter().map(u8::to_string).collect::<Vec<_>>().join(", ");
        warnings.push(format!(
            "the pattern's {} p-lock lanes are all in use, so {n} lane{} (parameter{} {list}) \
             {} not written — free some p-locks on the box first",
            spec.pattern.num_p_locks,
            if n == 1 { "" } else { "s" },
            if n == 1 { "" } else { "s" },
            if n == 1 { "was" } else { "were" },
        ));
    }

    Ok(warnings)
}
