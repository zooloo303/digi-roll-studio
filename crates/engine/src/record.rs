//! Where a note that has just been played lands on the armed track's grid.
//!
//! MIDI_RECORD_DESIGN.md §4.2, the pure half. The engine thread is the only
//! thread that knows what time it is *and* where every cursor stands, so it is
//! the only place this arithmetic can be done — but the arithmetic itself needs
//! neither, which is why it is a free function here rather than a method on
//! [`crate::transport::EngineThread`]. Everything below is tested without a
//! clock, a port or a box.
//!
//! # The one number it is measured against
//!
//! `TrackCursor::origin_at` — *when this track's current pattern began* — and
//! not `TransportState::position_millisteps`. The readout is global, is scaled
//! by the UI for display, and after a scene switch it and a cursor disagree by
//! however far into the old pattern the switch was taken. Placing against it
//! would put a note recorded straight after a scene change on the wrong step,
//! and only on tracks whose SCALE differs from 1x, which is the kind of bug
//! that gets called "timing feels off" for a month.
//!
//! # Swing is not subtracted, deliberately
//!
//! A swung track plays its odd steps late, so a player following the groove
//! plays late too, and subtracting the swing offset here would place those hits
//! back on the straight grid — which is what we want, but it is also what
//! *doing nothing* gives us. LIVE REC on the box records against the straight
//! grid and applies swing on playback; [`crate::scheduler`] already does the
//! second half, so this doing the first half by omission is the two halves
//! agreeing rather than an oversight.

pub use digi_core::record::{PlacedEvent, PlacedKind};

use digi_midi::live_input::LiveKind;

/// Where one arrival fell: the decomposition of an unrounded step position into
/// the three numbers the take and the model want.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Whole step within the pattern, 0-based.
    pub step: u64,
    /// Fraction of a step from that grid point, in (−0.5, 0.5]. Zero under
    /// QUANTIZE.
    pub micro: f64,
    /// Which trip through the pattern.
    pub pass: u64,
}

/// A live note kind as the take's kind.
///
/// A free function and not `impl From`, because the orphan rule forbids one:
/// both types are foreign to this crate, and this crate is the only one that
/// can see both. See `digi_core::record`'s header for why there are two.
pub fn placed_kind(kind: LiveKind) -> PlacedKind {
    match kind {
        LiveKind::NoteOn { pitch, velocity } => PlacedKind::NoteOn { pitch, velocity },
        LiveKind::NoteOff { pitch } => PlacedKind::NoteOff { pitch },
    }
}

/// Place a moment on a track's grid.
///
/// * `origin_at` — when the track's current pattern began, in seconds since the
///   transport started.
/// * `step_secs` — `time::track_step_seconds(bpm, scale)`, this track's own
///   step length.
/// * `length_steps` — the track's `length_steps`, which is what a pass is a
///   trip through.
/// * `t` — the moment, in the same seconds `origin_at` is in.
///
/// **Nearest step, not the one just gone.** A hit 40% of a step late belongs to
/// the step it was aiming at, and rounding down would put every human note one
/// step early. The consequence at the end of a pattern is the interesting one
/// and is a test of its own: a hit just late of the last step rounds *onto step
/// 0 of the next pass*, carrying a negative micro — which is where the box puts
/// it, and it is why `micro` is signed at all.
///
/// A moment before `origin_at` — a key struck in the instant between PLAY and
/// the first step — is placed on step 0 rather than wrapped to the end of a
/// pass that has not happened. Its micro is then more negative than the model
/// can hold, and `edit_ops::clamp_micro` in the take is what absorbs that.
pub fn place(
    origin_at: f64,
    step_secs: f64,
    length_steps: u16,
    t: f64,
    quantize: bool,
) -> Placement {
    let length = length_steps.max(1) as u64;
    // A zero or negative step length is a nonsense tempo or SCALE reaching this
    // far. Guarded rather than trusted, for `SetTempo`'s reason: the number
    // comes off a session file, and this divides by it.
    let elapsed = if step_secs > 0.0 {
        (t - origin_at) / step_secs
    } else {
        0.0
    };
    let abs = elapsed.round().max(0.0);
    let micro = if quantize { 0.0 } else { elapsed - abs };
    let abs = abs as u64;
    Placement { step: abs % length, micro, pass: abs / length }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 120 bpm, scale 1x: a step is 0.125 s.
    const STEP: f64 = 0.125;

    #[test]
    fn a_hit_on_the_beat_is_that_step_with_no_micro() {
        let p = place(0.0, STEP, 16, 4.0 * STEP, false);
        assert_eq!(p.step, 4);
        assert_eq!(p.pass, 0);
        assert!(p.micro.abs() < 1e-12);
    }

    #[test]
    fn a_hit_a_little_late_keeps_its_step_and_a_positive_micro() {
        let p = place(0.0, STEP, 16, 4.2 * STEP, false);
        assert_eq!(p.step, 4);
        assert!((p.micro - 0.2).abs() < 1e-9, "{}", p.micro);
    }

    #[test]
    fn a_hit_a_little_early_keeps_the_step_it_was_aiming_at() {
        // The rule that stops every human note landing one step early.
        let p = place(0.0, STEP, 16, 3.8 * STEP, false);
        assert_eq!(p.step, 4);
        assert!((p.micro + 0.2).abs() < 1e-9, "{}", p.micro);
    }

    /// The case the design calls out by name: just late of the last step of a
    /// 16-step pattern, which rounds forward onto step 0 of the next pass and
    /// keeps a negative micro. Anything else would either invent a step 16 or
    /// drag the note back a whole step.
    #[test]
    fn a_hit_just_late_of_the_last_step_is_step_zero_of_the_next_pass() {
        let p = place(0.0, STEP, 16, 15.7 * STEP, false);
        assert_eq!(p.step, 0);
        assert_eq!(p.pass, 1);
        assert!((p.micro + 0.3).abs() < 1e-9, "{}", p.micro);
    }

    #[test]
    fn passes_count_trips_through_this_tracks_own_length() {
        // Two tracks, one clock: 64 steps in, a 16-step track is on its fifth
        // pass and a 64-step track is on its second. That is polymeter, and it
        // falls out of dividing by each track's own length.
        let short = place(0.0, STEP, 16, 64.0 * STEP, false);
        assert_eq!((short.step, short.pass), (0, 4));
        let long = place(0.0, STEP, 64, 64.0 * STEP, false);
        assert_eq!((long.step, long.pass), (0, 1));
    }

    /// SCALE is a per-track step *length*, so a 2x track passes two steps in the
    /// time a 1x track passes one. The caller hands the track's own
    /// `track_step_seconds`; this test is what says the arithmetic honours it.
    #[test]
    fn a_double_scale_track_counts_twice_as_many_steps_in_the_same_second() {
        let one = place(0.0, STEP, 16, 8.0 * STEP, false);
        let two = place(0.0, STEP / 2.0, 16, 8.0 * STEP, false);
        assert_eq!(one.step, 8);
        assert_eq!(two.step, 0, "16 steps at 2x is a whole pass");
        assert_eq!(two.pass, 1);
    }

    /// The reason placement is measured off `origin_at` and not off zero: a
    /// scene change re-anchors the pattern, and a note played a beat after the
    /// switch is on step 4 of the *new* pattern, not step 4 + wherever the
    /// switch happened.
    #[test]
    fn placement_is_measured_from_where_the_pattern_began() {
        let origin = 9.5 * STEP;
        let p = place(origin, STEP, 16, origin + 4.0 * STEP, false);
        assert_eq!((p.step, p.pass), (4, 0));
    }

    #[test]
    fn quantize_zeroes_the_micro_and_leaves_the_step_alone() {
        let loose = place(0.0, STEP, 16, 4.4 * STEP, false);
        let tight = place(0.0, STEP, 16, 4.4 * STEP, true);
        assert_eq!(loose.step, tight.step);
        assert!(loose.micro > 0.3);
        assert_eq!(tight.micro, 0.0);
    }

    /// A key struck between PLAY and the first step. Step 0 of pass 0 is the
    /// only honest answer — wrapping it to step 15 of a pass that has not
    /// happened would record a note into the future.
    #[test]
    fn a_moment_before_the_pattern_began_lands_on_step_zero() {
        let p = place(1.0, STEP, 16, 0.5, false);
        assert_eq!((p.step, p.pass), (0, 0));
        assert!(p.micro < -0.5, "and it says how far before: {}", p.micro);
    }

    #[test]
    fn a_nonsense_step_length_places_everything_on_step_zero_rather_than_dividing_by_it() {
        let p = place(0.0, 0.0, 16, 3.0, false);
        assert_eq!((p.step, p.pass, p.micro), (0, 0, 0.0));
    }

    #[test]
    fn a_zero_length_track_is_treated_as_one_step_rather_than_a_modulo_by_zero() {
        let p = place(0.0, STEP, 0, 3.0 * STEP, false);
        assert_eq!((p.step, p.pass), (0, 3));
    }

    #[test]
    fn a_live_kind_crosses_the_layer_unchanged() {
        assert_eq!(
            placed_kind(LiveKind::NoteOn { pitch: 60, velocity: 99 }),
            PlacedKind::NoteOn { pitch: 60, velocity: 99 }
        );
        assert_eq!(
            placed_kind(LiveKind::NoteOff { pitch: 60 }),
            PlacedKind::NoteOff { pitch: 60 }
        );
    }
}
