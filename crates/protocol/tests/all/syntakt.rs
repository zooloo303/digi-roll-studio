//! The Syntakt read map, against the captures it was measured from.
//!
//! Every number here was read off the box's own screen while the pair was
//! taken, so a failure means the decoder stopped agreeing with the hardware
//! rather than with a previous version of itself. The captures live in
//! `dumps/syntakt-2026-09-10/`; the working is in that folder's READMEs.
//!
//! **Nothing here writes.** There is no encoder for this box and no firmware
//! allowlist entry, and these tests do not imply either.

use digi_protocol::pattern::length_byte_to_steps;
use digi_protocol::syntakt_pattern as st;

fn dump(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../dumps/syntakt-2026-09-10")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// The state at the end of the session: two trigs on track 7, one of them fully
/// locked and one of them untouched.
fn final_state() -> Vec<u8> {
    dump("stride-H01/vel-64.bin")
}

#[test]
fn the_captures_announce_the_version_every_snapshot_has_carried() {
    for name in ["req-60-idx-00.bin", "req-61-idx-00.bin", "stride-H01/vel-64.bin"] {
        let d = dump(name);
        assert_eq!(st::struct_version(&d), Some(st::STRUCT_VERSION), "{name}");
        assert!(st::looks_like_pattern(&d), "{name}");
    }
}

/// The combined dump carries the pattern in the same place the standalone one
/// does, so a decoder must not care which request produced its bytes.
#[test]
fn the_pattern_region_is_the_same_in_both_requests() {
    let combined = dump("req-60-idx-00.bin");
    let standalone = dump("req-61-idx-00.bin");
    assert_eq!(
        &combined[..st::PATTERN_BYTES],
        &standalone[..st::PATTERN_BYTES],
        "0x60 and 0x61 disagree inside the pattern region"
    );
}

#[test]
fn an_untouched_pattern_has_no_trigs_anywhere() {
    let empty = dump("req-61-idx-00.bin");
    for track in 0..st::NUM_BLOCKS {
        assert_eq!(st::trig_count(&empty, track), 0, "block {track}");
        assert!(st::track_notes(&empty, track).is_empty(), "block {track}");
    }
}

/// The positional bit is not a trig. An empty pattern reads `00 10` on every
/// even step, and a decoder that tested for "not zero" would report 32 trigs a
/// track on a pattern with nothing in it.
#[test]
fn the_positional_bit_is_not_mistaken_for_a_trig() {
    let empty = dump("req-61-idx-00.bin");
    let evens_carry_it = (0..st::NUM_STEPS)
        .filter(|s| s % 2 == 1)
        .all(|s| st::trig_word(&empty, 0, s).unwrap().1 & st::TRIG_POSITIONAL != 0);
    assert!(evens_carry_it, "the empty pattern should carry the positional bit on every even step");
    assert_eq!(st::trig_count(&empty, 0), 0);
}

#[test]
fn the_two_trigs_we_placed_are_where_we_placed_them() {
    let d = final_state();
    let notes = st::track_notes(&d, 6); // track 7
    assert_eq!(notes.len(), 2);
    // Steps 5 and 16 as the box counts them, zero-based here.
    assert_eq!(notes.iter().map(|n| n.step).collect::<Vec<_>>(), vec![4, 15]);
}

/// The locked trig, field by field, against what the box displayed when each
/// lock was set: E5, velocity 64, 1/32, +1/48 of a bar.
#[test]
fn a_fully_locked_trig_decodes_to_what_the_box_showed() {
    let d = final_state();
    let note = st::track_notes(&d, 6)[0];
    assert_eq!(note.note, 64, "E5 is MIDI 64 in the numbering where 60 is C5");
    assert_eq!(note.velocity, 64);
    assert_eq!(length_byte_to_steps(note.length_byte), 0.5, "the box showed 1/32");
    assert_eq!(note.micro_ticks, 8, "the box showed +1/48 of a bar, a third of a step");
    assert_eq!(
        note.micro_ticks as i32 * 3,
        st::MICRO_TICKS_PER_STEP,
        "a third of a step is eight ticks, so a step is twenty-four"
    );
    assert_eq!(note.locked, st::Locks { note: true, velocity: true, length: true });
}

/// The untouched trig has no locks at all and must resolve to the track's
/// defaults — which is what the box itself showed for it: D5, 100, 1/16.
#[test]
fn an_unlocked_trig_resolves_to_the_track_defaults() {
    let d = final_state();
    let note = st::track_notes(&d, 6)[1];
    assert_eq!(note.locked, st::Locks::default(), "nothing was locked on this one");
    assert_eq!(note.note, 62, "D5");
    assert_eq!(note.velocity, 100);
    assert_eq!(length_byte_to_steps(note.length_byte), 1.0, "1/16 is one step at SCALE 1x");
    assert_eq!(note.micro_ticks, 0);
    assert_eq!(st::defaults(&d, 6), Some((62, 100, 14)));
}

/// A default is not a lock. Both trigs on track 7 report velocity 64 and 100
/// respectively, but only one of them would survive an edit to the track
/// default — and the decoder has to keep saying which.
#[test]
fn a_lock_at_the_default_value_is_still_a_lock() {
    let d = final_state();
    let notes = st::track_notes(&d, 6);
    assert!(notes[0].locked.velocity);
    assert!(!notes[1].locked.velocity);
    assert_eq!(notes[1].velocity, st::defaults(&d, 6).unwrap().1);
}

/// The whole pattern, counted. Ten of the thirteen blocks are as they were
/// before the session; track 7 gained the two trigs above.
#[test]
fn the_trig_counts_across_the_pattern_are_stable() {
    let before = dump("stride-H01/before.bin");
    let after = final_state();
    let counts = |d: &[u8]| (0..st::NUM_BLOCKS).map(|t| st::trig_count(d, t)).collect::<Vec<_>>();
    assert_eq!(counts(&before), vec![8, 4, 6, 8, 9, 1, 0, 0, 16, 0, 0, 48, 0]);
    assert_eq!(counts(&after), vec![8, 4, 6, 8, 9, 1, 2, 0, 16, 0, 0, 48, 0]);
}

/// The lane offsets are the Analog Four's, which is the fact that says where to
/// look when something here is unexplained.
#[test]
fn the_lane_layout_matches_the_analog_four() {
    use digi_protocol::a4_pattern as a4;
    assert_eq!(st::BLOCK_BASE, a4::TRACK_BASE);
    assert_eq!(st::TRIG_LANE, a4::TRIG_LANE);
    assert_eq!(st::NOTE_LANE, a4::NOTE_LANE);
    assert_eq!(st::VELOCITY_LANE, a4::VELOCITY_LANE);
    assert_eq!(st::LENGTH_LANE, a4::LENGTH_LANE);
    assert_eq!(st::MICRO_LANE, a4::MICRO_TIMING_LANE);
    assert_eq!(st::NO_LOCK, a4::NO_NOTE);
    // And the two that differ, recorded so a copy-paste from the A4 fails here
    // rather than silently reading the wrong track.
    assert_ne!(st::BLOCK_STRIDE, a4::TRACK_STRIDE);
    assert_ne!(st::DEFAULT_NOTE, a4::DEFAULT_NOTE);
}

#[test]
fn indices_off_the_end_are_none_rather_than_a_panic() {
    let d = final_state();
    assert!(st::trig_word(&d, st::NUM_BLOCKS, 0).is_none());
    assert!(st::trig_word(&d, 0, st::NUM_STEPS).is_none());
    assert!(st::defaults(&d, st::NUM_BLOCKS).is_none());
    assert!(st::track_notes(&d, st::NUM_BLOCKS).is_empty());
}
