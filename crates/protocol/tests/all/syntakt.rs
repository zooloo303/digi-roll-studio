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

/// Tempo, swing and pattern length, each against what the box displayed.
#[test]
fn the_pattern_level_fields_read_what_the_box_showed() {
    let at_130 = dump("stride-H01/tempo-130.bin");
    let at_100 = dump("stride-H01/tempo-100.bin");
    assert_eq!(st::tempo_bpm(&at_130), Some(130.0));
    assert_eq!(st::tempo_bpm(&at_100), Some(100.0));

    // Straight before the edit, 60% after — the digis' convention, where the
    // byte is the offset from 50 rather than the percentage itself.
    assert_eq!(st::swing_percent(&at_100), Some(50));
    assert_eq!(st::swing_percent(&dump("stride-H01/swing-60.bin")), Some(60));

    // A raw step count. The donated pair recorded 16 → 32 here; this pattern
    // is 64 steps long.
    assert_eq!(st::pattern_length_steps(&at_100), Some(64));
}

/// Swing is the same field the digis have, so it needs no conversion of its
/// own — and a decoder that treated the byte as a percentage would report 0%.
#[test]
fn a_swing_byte_is_an_offset_and_not_a_percentage() {
    let straight = dump("stride-H01/tempo-100.bin");
    assert_eq!(straight[st::SWING], 0);
    assert_eq!(st::swing_percent(&straight), Some(st::SWING_STRAIGHT_PERCENT));
}

/// The pattern-level fields sit inside the region both requests share, so they
/// read the same whichever dump they came from.
#[test]
fn the_pattern_level_fields_are_inside_the_shared_region() {
    for offset in [st::TEMPO + 3, st::PATTERN_LENGTH, st::SWING] {
        assert!(offset < st::PATTERN_BYTES, "{offset} is past the pattern region");
    }
}

/// The four condition points that were read off the box, each in a different
/// part of the menu. They fix where the three regions begin and end.
#[test]
fn the_condition_bytes_read_what_the_box_showed() {
    use digi_protocol::syntakt_pattern::SyntaktCond as C;
    for (name, want) in [
        ("cond-50.bin", C::Probability(50)),
        ("cond-ratio.bin", C::Ratio { a: 1, b: 2 }),
        ("cond-pre.bin", C::Logic { name: "PRE", negated: false }),
        ("cond-100.bin", C::Probability(100)),
    ] {
        let d = dump(&format!("stride-H01/{name}"));
        assert_eq!(st::step_condition(&d, 6, 4), Some(want), "{name}");
    }
}

/// A step with no condition reads `ff`, the same "unset" the other lanes use —
/// and the trig is still a trig.
#[test]
fn a_trig_without_a_condition_reads_none() {
    let d = dump("stride-H01/cond-before.bin");
    assert_eq!(d[4 + 983 * 6 + st::CONDITION_LANE + 4], st::NO_LOCK);
    assert_eq!(st::step_condition(&d, 6, 4), None);
    assert!(st::plays_note(&d, 6, 4), "the trig is still there");
}

/// The lane is the A4's, the table is not. Recorded because the two facts
/// arrive together and it would be easy to carry the second across with the
/// first: this box's logic block has five pairs where the A4 has four, which is
/// what moves the ratios two later.
#[test]
fn the_condition_lane_is_the_a4s_but_the_table_is_not() {
    use digi_protocol::a4_pattern as a4;
    assert_eq!(st::CONDITION_LANE, a4::CONDITION_LANE);
    assert_eq!(st::CONDITION_LOGIC.len(), 5);
    // The A4 puts 1:2 at 30; this box puts it at 32, which is measured.
    assert_eq!(digi_protocol::a4_conditions::from_byte(30), Some(
        digi_protocol::a4_conditions::A4Cond::Ratio(1, 2)
    ));
    assert_eq!(st::CONDITION_RATIO_BASE, 32);
}

// --- The round trip ----------------------------------------------------------

/// Every capture in the folder, so a new one joins the proof by existing.
fn every_capture() -> Vec<(String, Vec<u8>)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../dumps/syntakt-2026-09-10");
    let mut out = Vec::new();
    for base in [dir.clone(), dir.join("stride-H01")] {
        let mut entries: Vec<_> = std::fs::read_dir(&base)
            .unwrap_or_else(|e| panic!("reading {}: {e}", base.display()))
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "bin"))
            .collect();
        entries.sort();
        for path in entries {
            let bytes = std::fs::read(&path).expect("reading a capture");
            if st::looks_like_pattern(&bytes) {
                out.push((path.file_name().unwrap().to_string_lossy().into_owned(), bytes));
            }
        }
    }
    assert!(out.len() >= 12, "expected the captured patterns, found {}", out.len());
    out
}

/// **The property everything else waits on.** Decode a captured pattern, write
/// the result straight back, and get the same bytes.
///
/// This is what has to hold before a write path is worth attempting, and it is
/// stronger than it looks: the decoder resolves `ff` to the track default, so a
/// writer that stored what it read would turn every inherited value into a lock
/// and move hundreds of bytes. It also proves the trig words survive — the
/// positional bit, and the `0x10` one track carries in byte 0 that nobody here
/// has explained — because they are OR-ed and masked rather than assigned.
#[test]
fn decoding_a_pattern_and_writing_it_back_changes_nothing() {
    for (name, original) in every_capture() {
        for track in 0..st::NUM_BLOCKS {
            let mut copy = original.clone();
            let notes = st::track_notes(&original, track);
            assert!(st::set_track_notes(&mut copy, track, &notes), "{name} block {track}");
            let moved: Vec<usize> =
                (0..original.len()).filter(|&i| copy[i] != original[i]).collect();
            assert!(
                moved.is_empty(),
                "{name} block {track}: writing back moved {} bytes, first at {:?}",
                moved.len(),
                &moved[..moved.len().min(6)]
            );
        }
    }
}

/// The same round trip with every block written in one pass, so a writer that
/// was faithful per block but stepped on its neighbours is caught too.
#[test]
fn writing_every_block_back_at_once_changes_nothing() {
    for (name, original) in every_capture() {
        let mut copy = original.clone();
        for track in 0..st::NUM_BLOCKS {
            let notes = st::track_notes(&original, track);
            st::set_track_notes(&mut copy, track, &notes);
        }
        assert_eq!(copy, original, "{name}: a full rewrite is not byte-identical");
    }
}

/// A write that changes one thing changes **only** that thing. The rule the
/// digis' encoder is held to, and the one that makes an unexplained byte safe
/// to carry rather than dangerous.
#[test]
fn changing_one_note_moves_exactly_one_byte() {
    let original = dump("stride-H01/plock-after.bin");
    let mut copy = original.clone();
    let mut notes = st::track_notes(&original, 6);
    notes[0].note = 72;
    notes[0].locked.note = true;
    assert!(st::set_track_notes(&mut copy, 6, &notes));

    let moved: Vec<usize> = (0..original.len()).filter(|&i| copy[i] != original[i]).collect();
    assert_eq!(moved, vec![4 + 983 * 6 + st::NOTE_LANE + 4], "one byte, the note lane's");
    assert_eq!(copy[moved[0]], 72);
}

/// Removing a trig leaves the bytes an empty step was measured to hold, and
/// leaves the positional bit alone.
#[test]
fn removing_a_trig_leaves_what_an_empty_step_holds() {
    let original = dump("stride-H01/plock-after.bin");
    let mut copy = original.clone();
    let kept: Vec<_> = st::track_notes(&original, 6).into_iter().filter(|n| n.step != 15).collect();
    assert!(st::set_track_notes(&mut copy, 6, &kept));

    let base = 4 + 983 * 6;
    assert!(!st::plays_note(&copy, 6, 15));
    for lane in [st::NOTE_LANE, st::VELOCITY_LANE, st::LENGTH_LANE, st::CONDITION_LANE] {
        assert_eq!(copy[base + lane + 15], st::EMPTY_LANE, "lane +{lane}");
    }
    assert_eq!(copy[base + st::MICRO_LANE + 15], st::EMPTY_MICRO);
    // Step 16 is even, so its word carries the positional bit — which is not
    // ours to clear.
    assert_eq!(copy[base + 15 * 2 + 1] & st::TRIG_POSITIONAL, st::TRIG_POSITIONAL);
}

/// **The outbound framing, checked against the box's own.**
///
/// A store on these boxes is an unsolicited dump *response*, so the message
/// this app would send to write a pattern has the same shape as the one the box
/// sends when asked for it. The captures keep both halves — the raw `.syx` the
/// box produced and the payload unpacked from it — so whether
/// `build_dump_message` produces what a Syntakt produces is answerable at rest,
/// with nothing connected and nothing sent.
///
/// This is worth having before any write is attempted. `DEVELOPMENT.md` lesson
/// 13 is an Analog Four whose whole SysEx API went down until a power cycle
/// because it was handed a body it could not parse, six times over two days.
/// Framing is exactly that class of unknown, and this removes it from the list
/// without touching hardware.
#[test]
fn the_message_this_app_would_send_matches_the_one_the_box_sent() {
    use digi_protocol::protocol::{build_dump_message, FAMILY_SYNTAKT};

    for (request, dump_type, index, stem) in [
        (0x60u8, 0x50u8, 0u8, "req-60-idx-00"),
        (0x61, 0x51, 0, "req-61-idx-00"),
        (0x62, 0x52, 0, "req-62-idx-00"),
    ] {
        let raw = dump(&format!("{stem}.syx"));
        let payload = dump(&format!("{stem}.bin"));
        let built = build_dump_message(FAMILY_SYNTAKT, dump_type, index, &payload);
        assert_eq!(
            built, raw,
            "request {request:#04x}: the framing this app builds is not the framing the box \
             produced for the same payload"
        );
    }
}
