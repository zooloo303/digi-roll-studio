// The LEN scale: note lengths a box can actually store.
//
// Ported from the length half of js/roll-bridge.js. The byte conversions
// underneath are already in `protocol` and are hardware-verified; this is only
// the snapping built on top of them, so a fine resize shows exactly what will
// land on the box rather than a number that quietly rounds on write.

use digi_protocol::pattern::{length_byte_to_steps, steps_to_length_byte};

/// The shortest note the boxes can store: length byte 0. Everything below two
/// steps is held in 1/16-step increments, so this is a real musical value rather
/// than a rounding artefact.
pub const LEN_MIN: f64 = 0.125;

fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}

/// Snap a length in steps to the nearest value a box can hold.
///
/// `max_steps` is the room left in the pattern. Snapping picks the *nearest*
/// representable length, which can round up past that room, so the result is
/// walked back down the length scale until it fits. Pass `f64::INFINITY` for
/// "no limit", which is what the JS default argument does.
pub fn snap_len_fine(steps: f64, max_steps: f64) -> f64 {
    let want = clamp(steps, LEN_MIN, max_steps.max(LEN_MIN));
    let mut byte = steps_to_length_byte(want);
    while byte > 0 && length_byte_to_steps(byte) > max_steps {
        byte -= 1;
    }
    length_byte_to_steps(byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected values read out of js/roll-bridge.js first, then written down
    // here — the Phase 1 method.
    #[test]
    fn snaps_to_the_scale_the_box_stores() {
        assert_eq!(snap_len_fine(1.1, f64::INFINITY), 1.125);
        assert_eq!(snap_len_fine(2.1, f64::INFINITY), 2.125);
    }

    #[test]
    fn snapping_is_idempotent() {
        for &v in &[0.4_f64, 1.1, 2.1, 7.9, 31.0] {
            let once = snap_len_fine(v, f64::INFINITY);
            assert_eq!(snap_len_fine(once, f64::INFINITY), once, "for {v}");
        }
    }

    #[test]
    fn never_returns_less_than_the_shortest_storable_note() {
        assert_eq!(snap_len_fine(0.01, 16.0), LEN_MIN);
        assert_eq!(snap_len_fine(-5.0, 16.0), LEN_MIN);
    }

    #[test]
    fn walks_back_down_the_scale_to_fit_the_room_left() {
        // Nearest-snapping 1.1 gives 1.125, which does not fit in one step of
        // room, so it must come down rather than overrun the pattern.
        assert!(snap_len_fine(1.1, 1.0) <= 1.0);
    }
}
