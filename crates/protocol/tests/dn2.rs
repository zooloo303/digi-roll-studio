//! Digitone II decode and encode, against real hardware captures.
//!
//! Port of `test/dn2.test.js`. The DN2's trig pool is `perNote` rather than
//! `quad`: one record per sounding note, so a chord is several consecutive
//! records sharing (track, step).

mod common;

use common::*;
use digi_protocol::pattern::*;

const COND: &str = "digitone2-A01-conditions-2026-08-02.syx";
const FRESH: &str = "dn2-fresh-A01.syx";
const CHORDS: &str = "digitone2-pernote-chords-2026-08-04.syx";

const WRITE_TRACK: usize = 2;

fn decoded(name: &str) -> PatternKit {
    decode_pattern_kit(&dn2_spec(), &payload(name)).expect("decode")
}

fn bassline() -> Vec<Note> {
    notes_from(&[
        (0, 36, 110, 2.0, 0),
        (3, 39, 90, 1.0, -2),
        (6, 41, 127, 4.0, 5),
        (10, 36, 64, 1.0, 0),
    ])
}

#[test]
fn capture_is_a_0x15_family_pattern_kit() {
    let bytes = fixture_bytes(COND);
    let messages = digi_protocol::protocol::split_sysex_stream(&bytes);
    assert_eq!(messages.len(), 1);
    let dump = messages[0].dump.as_ref().unwrap();
    assert_eq!(dump.family, digi_protocol::protocol::FAMILY_DIGITONE_2);
    // 89088-byte pattern struct + the 10752-byte v3 kit.
    assert_eq!(dump.payload.len(), 99840);
    assert!(dump.checksum_ok && dump.count_ok);
}

#[test]
fn reads_the_struct_versions_this_os_generation_uses() {
    let p = decoded(COND);
    assert_eq!(p.version, 3);
    assert_eq!(p.kit.version, 3);
}

#[test]
fn finds_the_trigs_the_pattern_is_known_to_contain() {
    let p = decoded(COND);
    let counts: Vec<usize> = (0..16).map(|t| track_trig_count(&p, t)).collect();
    assert_eq!(counts, vec![8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let steps: Vec<u8> = track_notes(&p, 0).iter().map(|n| n.step).collect();
    assert_eq!(steps, (0u8..8).collect::<Vec<_>>());
}

#[test]
fn reads_pattern_level_settings_from_the_plus_48_shifted_tail() {
    let p = decoded(COND);
    assert_eq!(p.tempo_bpm, 120.0);
    assert_eq!(p.kit_index, 0);
    assert_eq!(p.kit.name, "KIT 1");
    assert_eq!(p.kit.sound_names[0], "PRESET 1");
    assert_eq!(p.kit.sound_names[15], "PRESET 16");
    assert_eq!(p.kit.midi_mask, 0); // location unmapped on DN2 — always 0
}

#[test]
fn decodes_a_blank_pattern_as_blank() {
    let p = decoded(FRESH);
    assert!((0..16).all(|t| track_trig_count(&p, t) == 0));
    assert_eq!(p.name, "");
    assert_eq!(p.tempo_bpm, 120.0);
    assert!(p.tracks.iter().all(|t| t.trigs.is_empty()));
}

#[test]
fn rejects_payloads_it_cannot_decode_safely() {
    let spec = dn2_spec();
    let err = decode_pattern_kit(&spec, &[0u8; 10]).unwrap_err();
    assert!(err.contains("too short"), "{err}");

    let mut alien = payload(COND);
    alien[3] = 9;
    let err = decode_pattern_kit(&spec, &alien).unwrap_err();
    assert!(err.contains("version 9"), "{err}");
}

// --- Write path ---------------------------------------------------------------

#[test]
fn round_trips_notes_through_encode_then_decode() {
    let spec = dn2_spec();
    let (out, dropped) =
        encode_track_notes(&spec, &payload(COND), WRITE_TRACK, &bassline()).unwrap();
    assert_eq!(dropped, 0);
    let p = decode_pattern_kit(&spec, &out).unwrap();
    assert_eq!(track_notes(&p, WRITE_TRACK), bassline());
}

#[test]
fn writes_the_records_the_js_encoder_writes() {
    // Track 1 owns the pool's first eight records, so track 3's start at
    // 18996 + 8×6. Byte-level ground truth, from the JS encoder on this fixture.
    let spec = dn2_spec();
    let (out, _) = encode_track_notes(&spec, &payload(COND), WRITE_TRACK, &bassline()).unwrap();
    let recs = 18996 + 48;
    assert_eq!(
        &out[recs..recs + 24],
        &[
            2, 0, 36, 110, 30, 0, // length byte 30 = two steps
            2, 3, 39, 90, 14, 254, // micro 0xFE = −2 ticks
            2, 6, 41, 127, 46, 5, // length byte 46 = four steps
            2, 10, 36, 64, 14, 0,
        ]
    );
}

#[test]
fn round_trips_chords_as_consecutive_per_note_records_sharing_track_and_step() {
    // Hardware-verified: a 3-note chord on one trig is stored as three
    // consecutive records with the same track/step, one note each.
    let spec = dn2_spec();
    let chord = notes_from(&[(0, 60, 100, 1.0, 0), (0, 64, 100, 1.0, 0), (0, 67, 100, 1.0, 0)]);
    let (out, dropped) = encode_track_notes(&spec, &payload(FRESH), 0, &chord).unwrap();
    assert_eq!(dropped, 0);
    assert_eq!(
        &out[18996..18996 + 18],
        &[0, 0, 60, 100, 14, 0, 0, 0, 64, 100, 14, 0, 0, 0, 67, 100, 14, 0]
    );
    let pitches: Vec<u8> = track_notes(&decode_pattern_kit(&spec, &out).unwrap(), 0)
        .iter()
        .map(|n| n.pitch)
        .collect();
    assert_eq!(pitches, vec![60, 64, 67]);
}

#[test]
fn drops_chord_notes_past_max_notes_and_reclaims_delete_residue_records() {
    let spec = dn2_spec();
    let fat = notes_from(&[
        (0, 60, 100, 1.0, 0),
        (0, 62, 100, 1.0, 0),
        (0, 64, 100, 1.0, 0),
        (0, 65, 100, 1.0, 0),
        (0, 67, 100, 1.0, 0),
    ]);
    let (out, dropped) = encode_track_notes(&spec, &payload(FRESH), 0, &fat).unwrap();
    assert_eq!(dropped, 1); // the fifth pitch, over the maxNotes: 4 cap
    assert_eq!(track_notes(&decode_pattern_kit(&spec, &out).unwrap(), 0).len(), 4);

    // A record the box half-blanked on delete (track/step/note 0xFF, stray
    // micro) is dead space the encoder may claim.
    let mut residue = payload(FRESH);
    residue[18996..19002].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);
    let (reused, _) =
        encode_track_notes(&spec, &residue, 0, &[note(0, 60, 100, 1.0, 0)]).unwrap();
    assert_eq!(&reused[18996..19002], &[0, 0, 60, 100, 14, 0]);
}

#[test]
fn touches_only_the_track_step_words_and_the_trig_record_pool() {
    let before = payload(COND);
    let (after, _) = encode_track_notes(&dn2_spec(), &before, WRITE_TRACK, &bassline()).unwrap();
    let track_base = 4 + WRITE_TRACK * 1187;
    for d in diff_payloads(&before, &after, 100_000) {
        let in_step_words = d.offset >= track_base && d.offset < track_base + 256;
        let in_pool = d.offset >= 18996 && d.offset < 68148;
        assert!(
            in_step_words || in_pool,
            "unexpected byte change at {} ({})",
            d.offset,
            describe_offset(&dn2_spec(), d.offset)
        );
    }
}

#[test]
fn leaves_other_tracks_alone_when_rewriting_one_track() {
    let spec = dn2_spec();
    let before = track_notes(&decoded(COND), 0);
    let (after, _) = encode_track_notes(&spec, &payload(COND), WRITE_TRACK, &bassline()).unwrap();
    assert_eq!(track_notes(&decode_pattern_kit(&spec, &after).unwrap(), 0), before);
}

// --- The per-note chord capture -----------------------------------------------

/// Ground truth captured read-only from a Digitone II (OS 1.10D, build 0049) on
/// 2026-08-04: chords entered on the box itself through NOTE EDIT, one variable
/// per step — velocities on step 1, lengths on step 5, micro-timing on step 9.
/// This is the dump that proved the boxes store all three per note rather than
/// per trig.
#[test]
fn reads_back_exactly_the_per_note_values_seen_on_the_hardware() {
    let p = decoded(CHORDS);
    assert_eq!(
        by_pitch(&track_notes(&p, 0)),
        notes_from(&[
            (0, 60, 127, 1.0, 0),
            (0, 63, 52, 1.0, 0),
            (0, 67, 69, 1.0, 0),
            (4, 62, 40, 3.25, 0),
            (4, 65, 40, 2.5, 0),
            (4, 69, 40, 2.0, 0),
            (8, 61, 40, 1.0, 2),
            (8, 64, 40, 1.0, -9),
            (8, 68, 40, 1.0, -14),
        ])
    );
}

#[test]
fn a_written_chord_keeps_every_note_its_own_velocity_length_and_micro() {
    // The regression this fixture exists for, at the byte level: the values must
    // differ across a chord's records rather than repeat the first note's.
    let spec = dn2_spec();
    let notes = track_notes(&decoded(CHORDS), 0);
    let (out, dropped) = encode_track_notes(&spec, &payload(CHORDS), 0, &notes).unwrap();
    assert_eq!(dropped, 0);

    for (step, field) in [(0usize, "velocity"), (4, "length"), (8, "micro")] {
        let recs: Vec<[u8; 6]> = (18996..68148)
            .step_by(6)
            .filter(|&o| out[o] == 0 && out[o + 1] == step as u8)
            .map(|o| out[o..o + 6].try_into().unwrap())
            .collect();
        assert_eq!(recs.len(), 3, "step {step}: expected a 3-note chord");
        let idx = match field {
            "velocity" => 3,
            "length" => 4,
            _ => 5,
        };
        let distinct: std::collections::BTreeSet<u8> = recs.iter().map(|r| r[idx]).collect();
        assert_eq!(distinct.len(), 3, "{field} was mirrored across the chord");
    }
}

// --- Diff annotation ----------------------------------------------------------

#[test]
fn names_every_region_that_differs_between_a_populated_and_a_blank_pattern() {
    // This replays the experiment that mapped the DN2 format. An "unknown" range
    // label here would mean struct drift — everything the populated pattern
    // touches is in a region the spec claims to understand.
    let spec = dn2_spec();
    let ranges = diff_annotated_ranges(&payload(COND), &payload(FRESH), |o| {
        describe_offset(&spec, o)
    });
    assert!(!ranges.is_empty());
    assert!(
        ranges.iter().all(|r| !r.label.contains("unknown per-step")),
        "unmapped region touched: {:?}",
        ranges.iter().find(|r| r.label.contains("unknown per-step"))
    );
    assert!(ranges.iter().any(|r| r.label.contains("step word")));
    assert!(ranges.iter().any(|r| r.label.contains("trig-record pool")));
}

#[test]
fn describe_offset_names_the_regions_the_diffing_lab_proved_out() {
    let s = dn2_spec();
    assert_eq!(describe_offset(&s, 0), "pattern struct version");
    assert_eq!(
        describe_offset(&s, 4 + 2 * 1187),
        "track 3 step word, step 1 (hi byte)"
    );
    assert_eq!(
        describe_offset(&s, 4 + 2 * 1187 + 1152),
        "track 3 defaults, default note"
    );
    assert_eq!(
        describe_offset(&s, 18996 + 6 + 2),
        "trig-record pool, record #1, note"
    );
    assert_eq!(describe_offset(&s, 88788), "pattern name");
    assert_eq!(describe_offset(&s, 88804), "pattern tempo (u32, BPM × 120)");
    assert_eq!(describe_offset(&s, 88816), "kit index");
    assert_eq!(describe_offset(&s, 89088 + 8), "kit +8");
}
