//! Pattern-level settings that live outside the note data. Swing is the first
//! one; anything else found in the settings tail belongs here too.
//!
//! Ported from `js/elektron/pattern-settings.js`. These *compose onto* a payload
//! rather than going through `decode_pattern_kit`/`encode_track_notes` — those
//! are hardware-verified and stay untouched (PLAN.md §7 rule 3) — which is the
//! same shape the track PROB byte's accessors take.

use crate::pattern::Spec;

/// Swing sits 24 bytes past the pattern name on both boxes, in the settings tail
/// (DT2 88764, DN2 88812). Derived from each spec's own `name_offset` rather than
/// added to the specs, so the device tables need no edit.
///
/// It is stored as the *offset from straight*, not the percentage the box shows:
/// 0 = 50% (straight), 30 = 80% (as far as the boxes go).
///
/// Hardware-verified 2026-08-04 on a DN2 (OS 1.10D, build 0049), and the three
/// captures that experiment produced are committed under `tests/fixtures/`: in a
/// fresh project two untouched patterns are byte-identical, A01 with swing 78%
/// held 28 where the blanks held 0, and moving it to 65% changed that one byte to
/// 15 and nothing else — one edit, one predicted byte.
const SWING_FROM_NAME: usize = 24;

/// Straight.
pub const SWING_MIN: u8 = 50;
/// As far as the boxes go.
pub const SWING_MAX: u8 = 80;

/// Where this spec keeps the swing byte.
pub fn swing_offset(spec: &Spec) -> usize {
    spec.pattern.name_offset + SWING_FROM_NAME
}

/// A pattern's swing as the box displays it, 50–80.
///
/// A byte past the top of the range reads as straight rather than erroring: it
/// would mean the field has moved, and a pattern we cannot fully read must still
/// open — the rule `read_track_prob` and `cond_from_byte` both follow.
///
/// One deviation from the JS, and it is a bug fix rather than a drift: reading
/// past the end of a short payload yields `undefined` there, so the JS returns
/// `50 + undefined` = `NaN`. A truncated payload is unreadable in exactly the way
/// an out-of-range byte is, so it takes the same answer.
pub fn read_swing(spec: &Spec, payload: &[u8]) -> u8 {
    match payload.get(swing_offset(spec)) {
        Some(&byte) if byte <= SWING_MAX - SWING_MIN => SWING_MIN + byte,
        _ => SWING_MIN,
    }
}

/// Write a pattern's swing into a payload, in place. Exactly one byte moves.
///
/// `None` means straight, because there is no way to store "unset" — the same
/// bargain the track PROB byte makes with 100%.
///
/// This is per *pattern*, unlike everything else that gets written to a box: it
/// changes the feel of all sixteen tracks in the slot, not just the one being
/// written. A caller writing a single track must say that out loud rather than
/// let it be discovered on playback (PLAN.md §6, Phase 6's confirm dialog).
///
/// Returns `false` and touches nothing if the payload is too short to hold the
/// byte — a short buffer is a caller bug, not something to panic the audio thread
/// over.
pub fn apply_swing(spec: &Spec, payload: &mut [u8], swing: Option<f64>) -> bool {
    let v = match swing {
        // `(x + 0.5).floor()`, not `f64::round`: JS rounds halves toward +∞ and
        // Rust rounds them away from zero. The clamp below hides the difference
        // for every in-range value, but the two agreeing by luck is not the same
        // as the two agreeing — see `micro_steps_to_byte`.
        Some(s) => (s + 0.5).floor().clamp(SWING_MIN as f64, SWING_MAX as f64) as u8,
        None => SWING_MIN,
    };
    match payload.get_mut(swing_offset(spec)) {
        Some(byte) => {
            *byte = v - SWING_MIN;
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::{dn2_spec, dt2_spec};

    #[test]
    fn swing_sits_at_the_offsets_the_hardware_mapping_named() {
        assert_eq!(swing_offset(&dt2_spec()), 88764);
        assert_eq!(swing_offset(&dn2_spec()), 88812);
    }

    #[test]
    fn a_payload_too_short_to_hold_the_byte_is_straight_not_a_panic() {
        let spec = dn2_spec();
        assert_eq!(read_swing(&spec, &[]), SWING_MIN);
        assert!(!apply_swing(&spec, &mut [0u8; 16], Some(72.0)));
    }
}
