//! Musical time → seconds.
//!
//! Everything here is in **seconds since the transport started**, as `f64`, and
//! nothing is quantised to a tick grid. PLAN.md §4 is explicit about why:
//! micro-timing is 1/24 of a step and swing is a percentage offset on odd steps,
//! and neither swing value is a whole number of 1/24ths. Quantising to 96 PPQN
//! would be exact for one and wrong for the other.
//!
//! Only the MIDI clock runs on a grid — 24 PPQN, which is the protocol's, not a
//! choice. (96 PPQN survives as the resolution for Standard MIDI File *export*,
//! matching `js/midi.js`: 96 TPQN, 24 ticks per 16th step. That is a different
//! problem and does not live here.)

use digi_core::model::TrackScale;

/// MIDI clock resolution. Fixed by the protocol: `0xF8` twenty-four times a
/// quarter note.
pub const CLOCK_PPQN: u32 = 24;

/// Steps to a quarter note. A step is a 16th.
pub const STEPS_PER_BEAT: f64 = 4.0;

/// Straight, and as far as the boxes go. Same range the pattern byte holds.
pub const SWING_MIN: u8 = 50;
pub const SWING_MAX: u8 = 80;

/// Seconds per pattern step (a 16th) at this tempo.
pub fn step_seconds(bpm: f64) -> f64 {
    60.0 / bpm / STEPS_PER_BEAT
}

/// Seconds per MIDI clock tick.
pub fn clock_tick_seconds(bpm: f64) -> f64 {
    60.0 / bpm / CLOCK_PPQN as f64
}

/// Seconds per step for a track running at `scale`.
///
/// SCALE is the box's per-track clock multiplier: at 2x a track plays twice as
/// fast, so its steps are half as long. This is the other half of what makes
/// polymeter real — tracks differ in step *length* as well as in step *count*.
pub fn track_step_seconds(bpm: f64, scale: TrackScale) -> f64 {
    step_seconds(bpm) / scale.multiplier()
}

/// How far swing pushes an odd step late, as a fraction of a step.
///
/// **This is not the formula `js/midi.js` uses, and the deviation is
/// deliberate — decided 2026-08-13.** That file computes
/// `((swing - 50) / 50) * (stepMs / 3)`, while the comment directly above it says
/// "at 66% the odd step lands 2/3 through the pair" — which its own formula does
/// not produce (it gives 0.111 of a step, putting the odd 16th 55.6% through the
/// pair). PLAN.md §4 states a third figure again, "30% of a step at the maximum".
///
/// What is implemented is the reading the JS comment describes and the boxes
/// document: swing is the percentage of the way through a *pair* of steps that
/// the odd step lands. So the offset from straight is `(swing - 50) / 50` steps —
/// 0 at 50%, exactly 1/3 of a step at 66.7% (the odd 16th two thirds through the
/// pair, a triplet feel), and 0.6 of a step at 80%.
///
/// PLAN.md §7 rule 3 is not what this crosses: rule 3 protects the
/// hardware-verified *encode/decode* internals, and the swing byte's mapping is
/// untouched (`protocol::pattern_settings`, pinned against three DN2 captures).
/// This is the browser preview's playback approximation, which never touched
/// hardware and which contradicted itself.
///
/// Even steps are never displaced; `step_in_pattern` is the position within the
/// pattern, not the absolute step, which is what the JS uses and what ties the
/// swing to the box's own step numbering.
pub fn swing_offset_steps(swing: u8, step_in_pattern: u64) -> f64 {
    if step_in_pattern % 2 == 0 {
        return 0.0;
    }
    (swing.clamp(SWING_MIN, SWING_MAX) as f64 - SWING_MIN as f64) / 50.0
}

/// The gap left at the end of a note so back-to-back notes retrigger cleanly,
/// and the floor that stops it eating a short note whole.
///
/// Ported from `js/midi.js`: `Math.max(1, n.len * stepMs - 8)`, in milliseconds.
/// A 0.125-step note at a fast tempo is shorter than the 8 ms gap, which is what
/// the floor exists for.
pub const NOTE_TAIL_GAP_SECONDS: f64 = 0.008;
pub const MIN_NOTE_SECONDS: f64 = 0.001;

/// How long a note of `len` steps actually sounds on a track at this step rate.
pub fn note_duration_seconds(len_steps: f64, track_step_seconds: f64) -> f64 {
    (len_steps * track_step_seconds - NOTE_TAIL_GAP_SECONDS).max(MIN_NOTE_SECONDS)
}

/// How far ahead of its trig a p-lock goes out, so the parameter is already
/// where the lane says by the time the note sounds. `js/midi.js` uses 2 ms.
pub const PLOCK_LEAD_SECONDS: f64 = 0.002;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_is_a_sixteenth_and_a_clock_tick_is_a_twentyfourth_of_a_beat() {
        // 120 bpm: a beat is 0.5 s, a 16th is 0.125 s, a clock tick 0.5/24.
        assert!((step_seconds(120.0) - 0.125).abs() < 1e-12);
        assert!((clock_tick_seconds(120.0) - 0.5 / 24.0).abs() < 1e-12);
        // 24 ticks per beat is 6 per step.
        assert!((step_seconds(120.0) / clock_tick_seconds(120.0) - 6.0).abs() < 1e-12);
    }

    #[test]
    fn scale_changes_how_long_a_track_step_lasts() {
        let bpm = 120.0;
        assert!((track_step_seconds(bpm, TrackScale::One) - 0.125).abs() < 1e-12);
        assert!((track_step_seconds(bpm, TrackScale::Two) - 0.0625).abs() < 1e-12);
        assert!((track_step_seconds(bpm, TrackScale::Half) - 0.25).abs() < 1e-12);
        assert!((track_step_seconds(bpm, TrackScale::Eighth) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn swing_never_moves_an_even_step() {
        for swing in SWING_MIN..=SWING_MAX {
            for step in [0u64, 2, 4, 30, 126] {
                assert_eq!(swing_offset_steps(swing, step), 0.0, "swing {swing} step {step}");
            }
        }
    }

    #[test]
    fn swing_puts_the_odd_step_that_percentage_through_the_pair() {
        // The property the JS comment describes, stated directly: an odd step at
        // offset `d` sits `(1 + d) / 2` of the way through its pair.
        for swing in SWING_MIN..=SWING_MAX {
            let d = swing_offset_steps(swing, 1);
            let through_the_pair = (1.0 + d) / 2.0 * 100.0;
            assert!(
                (through_the_pair - swing as f64).abs() < 1e-9,
                "swing {swing} put the odd step {through_the_pair}% through the pair"
            );
        }
    }

    #[test]
    fn swing_lands_on_the_named_values() {
        assert!((swing_offset_steps(50, 1) - 0.0).abs() < 1e-12, "straight");
        assert!((swing_offset_steps(80, 1) - 0.6).abs() < 1e-12, "maximum");
        // 66.7% is a triplet feel; the byte can only hold whole percentages, so
        // 67 is the closest the box gets to exactly 1/3.
        assert!((swing_offset_steps(67, 1) - 0.34).abs() < 1e-12);
    }

    #[test]
    fn swing_out_of_range_clamps_rather_than_flying_off() {
        assert_eq!(swing_offset_steps(0, 1), 0.0);
        assert_eq!(swing_offset_steps(255, 1), swing_offset_steps(SWING_MAX, 1));
    }

    #[test]
    fn a_short_note_at_a_fast_tempo_still_sounds() {
        // The case the JS floor exists for: 0.125 steps at 200 bpm is 9.4 ms,
        // and the 8 ms gap would leave 1.4 ms — the floor keeps it positive.
        let step = track_step_seconds(200.0, TrackScale::One);
        assert!(note_duration_seconds(0.125, step) >= MIN_NOTE_SECONDS);
        // And an absurdly short one does not go negative.
        assert_eq!(note_duration_seconds(0.001, step), MIN_NOTE_SECONDS);
    }

    #[test]
    fn a_normal_note_is_its_length_less_the_retrigger_gap() {
        let step = track_step_seconds(120.0, TrackScale::One);
        assert!((note_duration_seconds(4.0, step) - (0.5 - 0.008)).abs() < 1e-12);
    }
}
