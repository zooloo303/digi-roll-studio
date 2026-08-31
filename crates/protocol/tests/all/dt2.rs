//! Digitakt II decode and encode, against a real hardware capture.
//!
//! Port of `test/dt2.test.js`. The JS suite runs against a whole-project dump
//! (128 patterns, 16 MB); this one runs against the single-pattern captures that
//! are small enough to commit, so the pattern-stream and blank-pattern cases
//! move to what these fixtures can actually show.


use crate::common::*;
use digi_protocol::pattern::*;

const COND: &str = "digitakt2-A01-conditions-2026-08-02.syx";
const PLOCK: &str = "digitakt2-A01-plock-final-2026-08-04.syx";

/// Track 3 (index 2) is empty in every DT2 fixture — a clean target to write to.
const WRITE_TRACK: usize = 2;

fn decoded(name: &str) -> PatternKit {
    decode_pattern_kit(&dt2_spec(), &payload(name)).expect("decode")
}

/// The bassline from the JS suite, including a two-note chord on step 10.
fn bassline() -> Vec<Note> {
    notes_from(&[
        (0, 36, 110, 2.0, 0),
        (3, 39, 90, 1.0, -2),
        (6, 41, 127, 4.0, 5),
        (10, 36, 64, 1.0, 0),
        (10, 48, 64, 1.0, 0),
    ])
}

#[test]
fn capture_is_one_checksummed_pattern_kit() {
    let bytes = fixture_bytes(COND);
    let messages = digi_protocol::protocol::split_sysex_stream(&bytes);
    assert_eq!(messages.len(), 1);
    let dump = messages[0].dump.as_ref().unwrap();
    assert_eq!(dump.family, digi_protocol::protocol::FAMILY_DIGITAKT_2);
    assert_eq!(dump.index, 0);
    // 89088-byte pattern struct + the 22528-byte v4 kit.
    assert_eq!(dump.payload.len(), 111616);
    assert!(dump.checksum_ok && dump.count_ok);
}

#[test]
fn reads_the_struct_versions_this_os_generation_uses() {
    let p = decoded(COND);
    assert_eq!(p.version, 4);
    assert_eq!(p.kit.version, 4);
}

#[test]
fn finds_the_trigs_the_pattern_is_known_to_contain() {
    let p = decoded(COND);
    let counts: Vec<usize> = (0..16).map(|t| track_trig_count(&p, t)).collect();
    assert_eq!(counts, vec![15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let steps: Vec<u8> = track_notes(&p, 0).iter().map(|n| n.step).collect();
    assert_eq!(steps, (0u8..15).collect::<Vec<_>>());
}

#[test]
fn reads_pattern_level_settings() {
    let p = decoded(COND);
    assert_eq!(p.name, "");
    assert_eq!(p.tempo_bpm, 120.0);
    assert_eq!(p.kit_index, 0);
    assert_eq!(p.kit.name, "KIT 1");
    assert_eq!(p.kit.sound_names[0], "PRESET 1");
    assert_eq!(p.kit.sound_names[15], "PRESET 16");
    assert_eq!(p.kit.midi_mask, 0); // every track is a sample track
}

#[test]
fn joins_pool_records_with_track_defaults() {
    let p = decoded(COND);
    assert_eq!(p.tracks[0].length_steps, 16);
    assert_eq!(
        (
            p.tracks[0].default_note,
            p.tracks[0].default_velocity,
            p.tracks[0].default_length
        ),
        (60, 100, 14)
    );
    // Every trig is a plain grid trig on this capture: the edits it carries are
    // trig conditions, which live in per-step lanes the note decoder ignores.
    assert!(track_notes(&p, 0)
        .iter()
        .all(|n| n.pitch == 60 && n.velocity == 100 && n.len_steps == 1.0 && n.micro == 0.0));
}

#[test]
fn ignores_trig_records_left_behind_by_deleted_trigs() {
    // The pool holds quads for steps 0–15, but only 0–14 still have their trig
    // bit set: step 15's trig was deleted on the box and its quad lingered.
    let p = decoded(COND);
    let keys: Vec<u8> = p.tracks[0].trigs.keys().copied().collect();
    assert_eq!(keys, (0u8..=15).collect::<Vec<_>>());
    assert_eq!(track_notes(&p, 0).len(), 15);
}

#[test]
fn rejects_payloads_it_cannot_decode_safely() {
    let spec = dt2_spec();
    let err = decode_pattern_kit(&spec, &[0u8; 10]).unwrap_err();
    assert!(err.contains("too short"), "{err}");

    let mut alien = payload(COND);
    alien[3] = 9; // unheard-of pattern struct version
    let err = decode_pattern_kit(&spec, &alien).unwrap_err();
    assert!(err.contains("version 9"), "{err}");
}

// --- Write path ---------------------------------------------------------------

#[test]
fn round_trips_notes_through_encode_then_decode() {
    let spec = dt2_spec();
    let (out, dropped) =
        encode_track_notes(&spec, &payload(COND), WRITE_TRACK, &bassline()).unwrap();
    assert_eq!(dropped, 0);
    let p = decode_pattern_kit(&spec, &out).unwrap();
    assert_eq!(track_notes(&p, WRITE_TRACK), bassline());
}

#[test]
fn writes_the_quads_the_js_encoder_writes() {
    // Byte-level ground truth, taken from the JS encoder on this same fixture.
    // Track 1 owns the first sixteen quads, so track 3's land at 18948 + 16×24.
    let spec = dt2_spec();
    let (out, _) = encode_track_notes(&spec, &payload(COND), WRITE_TRACK, &bassline()).unwrap();
    let quads = 19332;
    assert_eq!(
        &out[quads..quads + 24],
        // A lone note still mirrors across its whole quad, as the box leaves it:
        // unused slots carry a 0xFF note and the first note's other three bytes.
        &[
            2, 0, 36, 110, 30, 0, 2, 0, 255, 110, 30, 0, 2, 0, 255, 110, 30, 0, 2, 0, 255, 110,
            30, 0
        ]
    );
    assert_eq!(
        &out[quads + 24..quads + 48],
        // −2 micro ticks is 0xFE; length byte 14 is one step.
        &[
            2, 3, 39, 90, 14, 254, 2, 3, 255, 90, 14, 254, 2, 3, 255, 90, 14, 254, 2, 3, 255, 90,
            14, 254
        ]
    );
    assert_eq!(
        &out[quads + 48..quads + 72],
        &[
            2, 6, 41, 127, 46, 5, 2, 6, 255, 127, 46, 5, 2, 6, 255, 127, 46, 5, 2, 6, 255, 127,
            46, 5
        ]
    );
    assert_eq!(
        &out[quads + 72..quads + 96],
        // The chord: two filled note slots in one quad, two 0xFF ones.
        &[2, 10, 36, 64, 14, 0, 2, 10, 48, 64, 14, 0, 2, 10, 255, 64, 14, 0, 2, 10, 255, 64, 14, 0]
    );
    // ...and nothing claimed beyond the four trigs.
    assert_eq!(&out[quads + 96..quads + 120], &[0xff; 24]);
}

#[test]
fn touches_only_the_track_step_words_and_the_trig_record_pool() {
    let before = payload(COND);
    let (after, _) = encode_track_notes(&dt2_spec(), &before, WRITE_TRACK, &bassline()).unwrap();
    // Offsets written out longhand rather than read from the spec: if the spec
    // drifts, this test is meant to notice.
    let track_base = 4 + WRITE_TRACK * 1184;
    for d in diff_payloads(&before, &after, 100_000) {
        let in_step_words = d.offset >= track_base && d.offset < track_base + 256;
        let in_pool = d.offset >= 18948 && d.offset < 68100;
        assert!(
            in_step_words || in_pool,
            "unexpected byte change at {} ({})",
            d.offset,
            describe_offset(&dt2_spec(), d.offset)
        );
    }
}

#[test]
fn leaves_other_tracks_alone_when_rewriting_one_track() {
    let spec = dt2_spec();
    let before = track_notes(&decoded(COND), 0);
    let (after, _) = encode_track_notes(&spec, &payload(COND), WRITE_TRACK, &bassline()).unwrap();
    assert_eq!(track_notes(&decode_pattern_kit(&spec, &after).unwrap(), 0), before);
}

#[test]
fn clears_a_track_when_given_no_notes_freeing_its_pool_quads() {
    let spec = dt2_spec();
    let (out, _) = encode_track_notes(&spec, &payload(COND), 0, &[]).unwrap();
    let p = decode_pattern_kit(&spec, &out).unwrap();
    assert_eq!(track_notes(&p, 0), Vec::new());
    assert!(p.tracks[0].trigs.is_empty());
    // Every quad the track owned, including the residue one, is back to 0xFF.
    assert_eq!(&out[18948..18948 + 16 * 24], &[0xff; 384][..]);
}

#[test]
fn drops_out_of_range_steps_and_chord_notes_past_four_slots() {
    let spec = dt2_spec();
    let mut over = vec![note(200, 60, 100, 1.0, 0)];
    for pitch in [60, 64, 67, 71, 74] {
        over.push(note(0, pitch, 100, 1.0, 0));
    }
    let (out, dropped) = encode_track_notes(&spec, &payload(COND), WRITE_TRACK, &over).unwrap();
    assert_eq!(dropped, 2); // the step-200 note and the fifth chord pitch
    let pitches: Vec<u8> = track_notes(&decode_pattern_kit(&spec, &out).unwrap(), WRITE_TRACK)
        .iter()
        .map(|n| n.pitch)
        .collect();
    assert_eq!(pitches, vec![60, 64, 67, 71]);
}

#[test]
fn refuses_when_the_pool_has_no_free_quad_for_a_new_trig() {
    let mut jammed = payload(COND);
    for o in (18948..68100).step_by(6) {
        jammed[o] = 6; // every record claimed by track 7
        jammed[o + 1] = 0;
    }
    let err = encode_track_notes(
        &dt2_spec(),
        &jammed,
        WRITE_TRACK,
        &[note(0, 60, 100, 1.0, 0)],
    )
    .unwrap_err();
    assert!(err.contains("full"), "{err}");
}

#[test]
fn refuses_to_encode_into_a_struct_version_it_does_not_vouch_for() {
    let mut alien = payload(COND);
    alien[3] = 9;
    let err = encode_track_notes(&dt2_spec(), &alien, WRITE_TRACK, &bassline()).unwrap_err();
    assert!(err.contains("refusing to write"), "{err}");
}

#[test]
fn the_plock_capture_decodes_as_two_populated_tracks() {
    // A second, independent DT2 capture: four grid trigs on track 1 and one on
    // track 2, with p-lock lanes the note decoder does not read yet.
    let p = decoded(PLOCK);
    let counts: Vec<usize> = (0..16).map(|t| track_trig_count(&p, t)).collect();
    assert_eq!(counts, vec![4, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let steps: Vec<u8> = track_notes(&p, 0).iter().map(|n| n.step).collect();
    assert_eq!(steps, vec![0, 4, 8, 12]);
    // The pool stored track 1's quads out of step order (8, 0, 4, 12). Decoding
    // is keyed by step, so the notes still come back in step order.
    assert_eq!(
        p.tracks[0].trigs.keys().copied().collect::<Vec<_>>(),
        vec![0, 4, 8, 12]
    );
}

// --- Pure functions -----------------------------------------------------------

#[test]
fn elektron_length_byte_scale_maps_the_landmark_values() {
    assert_eq!(length_byte_to_steps(0), 0.125);
    assert_eq!(length_byte_to_steps(2), 0.25);
    assert_eq!(length_byte_to_steps(14), 1.0); // the DT2 default trig length
    assert_eq!(length_byte_to_steps(30), 2.0);
    assert_eq!(length_byte_to_steps(46), 4.0);
    assert_eq!(length_byte_to_steps(62), 8.0);
    assert_eq!(length_byte_to_steps(78), 16.0);
    assert_eq!(length_byte_to_steps(94), 32.0);
    assert_eq!(length_byte_to_steps(110), 64.0);
    assert_eq!(length_byte_to_steps(126), 128.0);
    assert_eq!(length_byte_to_steps(127), f64::INFINITY);
}

#[test]
fn every_length_byte_round_trips_through_steps_and_back() {
    for v in 0u8..=127 {
        assert_eq!(steps_to_length_byte(length_byte_to_steps(v)), v, "byte {v}");
    }
}

#[test]
fn micro_timing_bytes_round_trip() {
    for ticks in -23i8..=23 {
        let byte = ticks as u8;
        assert_eq!(micro_byte_to_steps(byte), ticks as f64 / 24.0);
        assert_eq!(micro_steps_to_byte(micro_byte_to_steps(byte)), byte);
    }
    // Clamped to the range the box stores, and rounded the way JS Math.round
    // does — halves toward +∞, so −0.5 ticks is 0 and not −1.
    assert_eq!(micro_steps_to_byte(2.0), 23);
    assert_eq!(micro_steps_to_byte(-2.0), (-23i8) as u8);
    assert_eq!(micro_steps_to_byte(-0.5 / 24.0), 0);
    assert_eq!(micro_steps_to_byte(0.5 / 24.0), 1);
}

#[test]
fn bank_name_names_pattern_slots_the_way_the_box_does() {
    assert_eq!(bank_name(0), "A01");
    assert_eq!(bank_name(15), "A16");
    assert_eq!(bank_name(16), "B01");
    assert_eq!(bank_name(127), "H16");
}
