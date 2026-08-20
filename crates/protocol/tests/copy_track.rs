//! `js/elektron/copy-track.js` → `protocol::copy_track`.
//!
//! **The JS test file is not the whole oracle here, and that is the thing to know
//! before trusting this suite.** `test/copy-track.test.js` covers notes, chord
//! truncation and target isolation; it contains no occurrence of `plock`, `lane`
//! or `prob`, so the three hardest things `copyTrack` does — translating p-lock
//! lanes between two boxes' numbering, carrying the track's PROB default, and
//! carrying the per-trig conditions — had **no test on either side of the port**.
//! It also runs against `dumps/digitakt2-project-*.syx`, the ~16 MB project
//! captures this repo deliberately does not commit, so it is `skipIf`-skipped
//! unless those are present.
//!
//! So every expectation below was derived by running the JS `copyTrack` against
//! **the fixtures committed here**, on 2026-08-19, and transcribing what it
//! returned. The oracle is still digi-roll; the inputs are ours.
//!
//! Two witnesses had to be seeded, which is the discipline the swing bug taught:
//!
//! * **The track PROB carry.** Every committed fixture is at PROB 100, so a copy
//!   that never carried PROB would have been byte-invisible — the sixth instance
//!   of this repo's escape class, and the third time PROB specifically has had no
//!   witness. `seeded_prob` writes 37 into a source before copying it.
//! * **A chord that does not fit.** The fattest chord in any fixture is three
//!   notes and both boxes take four, so truncation could not fire. `dn2_chord`
//!   builds one the way the DN2 stores them, as the JS test does.
//!
//! The p-lock translation needed no seeding: `digitakt2-A01-plock-final` carries
//! ten lanes whose paramIds include the two that collide across the boxes.

mod common;

use std::collections::BTreeMap;

use common::payload;
use digi_protocol::copy_track::{
    copy_track, copy_track_from_bytes, describe_chord_drops, plock_lanes_for_target,
    truncate_chords, ChordDrop, CopyResult,
};
use digi_protocol::pattern::{
    decode_pattern_kit, diff_payloads, dn2_spec, dt2_spec, describe_offset, track_notes, Note,
    PatternKit, Spec,
};
use digi_protocol::plocks::{read_track_plocks, PoolLane};
use digi_protocol::trig_cond::{
    apply_track_prob, read_track_prob, read_track_trig_settings, TrigSetting,
};

const DT2_CONDITIONS: &str = "digitakt2-A01-conditions-2026-08-02.syx";
const DT2_PLOCKS: &str = "digitakt2-A01-plock-final-2026-08-04.syx";
const DN2_CHORDS: &str = "digitone2-pernote-chords-2026-08-04.syx";
const DN2_FRESH: &str = "dn2-fresh-A01.syx";
const DN2_FRESH_A02: &str = "dn2-fresh-A02.syx";

// --- Compact shapes the expectations are written in ----------------------------

/// `(step, pitch, velocity, length in steps, micro in 1/24 steps)` — the five
/// fields that must survive a hop between boxes.
type Musical = (u8, u8, u8, f64, i8);

fn musical(notes: &[Note]) -> Vec<Musical> {
    notes
        .iter()
        .map(|n| {
            (
                n.step,
                n.pitch,
                n.velocity,
                n.len_steps,
                (n.micro * 24.0).round() as i8,
            )
        })
        .collect()
}

fn written(result: &CopyResult) -> Vec<Musical> {
    musical(&result.notes.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>())
}

fn read_back(spec: &Spec, payload: &[u8], track: usize) -> Vec<Musical> {
    let kit = decode_pattern_kit(spec, payload).expect("target decodes");
    musical(&track_notes(&kit, track))
}

/// `(step, prob, fill, cond)`, only for steps that hold something.
type Setting = (u8, Option<u8>, Option<bool>, Option<&'static str>);

fn settings(spec: &Spec, payload: &[u8], track: usize) -> Vec<Setting> {
    read_track_trig_settings(spec, payload, track)
        .expect("track exists")
        .into_iter()
        .map(|(step, s)| (step, s.prob, s.fill, s.cond))
        .collect()
}

/// `(param_id, [(step, stored word)])` for each lane the track holds, in pool
/// order. The stored words matter more than anything else here: the whole point
/// of the translation is that they may have to change, and by how much.
type Lane = (u8, Vec<(usize, u16)>);

fn lanes(spec: &Spec, payload: &[u8], track: usize) -> Vec<Lane> {
    read_track_plocks(spec, payload, track)
        .expect("track exists")
        .into_iter()
        .map(compact_lane)
        .collect()
}

fn compact_lane(l: PoolLane) -> Lane {
    (
        l.param_id,
        l.values
            .iter()
            .enumerate()
            .filter_map(|(s, v)| v.map(|w| (s, w)))
            .collect(),
    )
}

// --- Fixture and witness builders ---------------------------------------------

fn kit_of(spec: &Spec, payload: &[u8]) -> PatternKit {
    decode_pattern_kit(spec, payload).expect("fixture decodes")
}

/// A source with a non-default track PROB, because no fixture has one. Written
/// through `apply_track_prob`, so the witness is made by the same code the read
/// side is pinned against rather than by hand-poking a byte.
fn seeded_prob(payload: &[u8], track: usize, prob: u8) -> Vec<u8> {
    let mut out = payload.to_vec();
    apply_track_prob(&dt2_spec(), &mut out, track, Some(prob)).expect("seeding PROB");
    out
}

/// A DN2 payload with one fat chord on `step` of track 1, built the way the box
/// stores them: consecutive per-note pool records sharing `(track, step)`, with
/// the step's trig bits set. Ported from the JS test's `dn2WithChord`, and it
/// exists for the same reason: no committed capture has a chord wider than three.
fn dn2_chord(step: u8, voices: &[(u8, u8)]) -> Vec<u8> {
    let spec = dn2_spec();
    let mut p = payload(DN2_FRESH);
    for (i, &(pitch, velocity)) in voices.iter().enumerate() {
        let o = spec.pattern.trig_pool + i * 6;
        // track 0, this step, pitch, velocity, length byte 14 (one step), no micro
        p[o..o + 6].copy_from_slice(&[0, step, pitch, velocity, 14, 0]);
    }
    let w = spec.pattern.tracks_offset + step as usize * 2;
    p[w] |= 0x03;
    p[w + 1] |= 0x81;
    p
}

/// A `Note` in the compact form the pure-policy tests are written in.
fn n(step: u8, pitch: u8, velocity: u8) -> (Note, TrigSetting) {
    (
        Note {
            step,
            pitch,
            velocity,
            len_steps: 1.0,
            micro: 0.0,
        },
        TrigSetting::default(),
    )
}

fn pitches(pairs: &[(Note, TrigSetting)]) -> Vec<u8> {
    pairs.iter().map(|(n, _)| n.pitch).collect()
}

// --- Notes across a device hop -------------------------------------------------

#[test]
fn dn2_notes_cross_into_a_dt2_pattern_note_for_note() {
    let (dn2, dt2) = (dn2_spec(), dt2_spec());
    let source = payload(DN2_CHORDS);
    let target = payload(DT2_CONDITIONS);
    let r = copy_track(
        &dn2,
        &kit_of(&dn2, &source),
        &source,
        0,
        &dt2,
        &target,
        5,
        "the Digitakt II",
    )
    .expect("copy");

    // Three three-note chords, and every field of every note intact — including
    // the two the JS test's own fixture could not exercise: a fractional length
    // and a negative micro-timing.
    let expected: Vec<Musical> = vec![
        (0, 60, 127, 1.0, 0),
        (0, 63, 52, 1.0, 0),
        (0, 67, 69, 1.0, 0),
        (4, 62, 40, 3.25, 0),
        (4, 65, 40, 2.5, 0),
        (4, 69, 40, 2.0, 0),
        (8, 61, 40, 1.0, 2),
        (8, 64, 40, 1.0, -9),
        (8, 68, 40, 1.0, -14),
    ];
    assert_eq!(written(&r), expected);
    assert_eq!(read_back(&dt2, &r.payload, 5), expected);
    assert_eq!(r.dropped, 0);
    assert!(r.drops.is_empty());
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);

    // The source track really is what we said it was, so the assertion above is
    // a comparison rather than a coincidence.
    assert_eq!(musical(&track_notes(&kit_of(&dn2, &source), 0)).len(), 9);
}

#[test]
fn dt2_notes_cross_into_a_dn2_pattern_note_for_note() {
    let (dt2, dn2) = (dt2_spec(), dn2_spec());
    let source = payload(DT2_CONDITIONS);
    let target = payload(DN2_FRESH);
    let r = copy_track(
        &dt2,
        &kit_of(&dt2, &source),
        &source,
        0,
        &dn2,
        &target,
        0,
        "the Digitone II",
    )
    .expect("copy");

    let expected: Vec<Musical> = (0..15).map(|s| (s, 60, 100, 1.0, 0)).collect();
    assert_eq!(written(&r), expected);
    assert_eq!(read_back(&dn2, &r.payload, 0), expected);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

// --- Per-trig conditions -------------------------------------------------------

#[test]
fn trig_conditions_cross_with_their_notes() {
    let (dt2, dn2) = (dt2_spec(), dn2_spec());
    let source = payload(DT2_CONDITIONS);
    let target = payload(DN2_FRESH);
    let r = copy_track(
        &dt2,
        &kit_of(&dt2, &source),
        &source,
        0,
        &dn2,
        &target,
        0,
        "the Digitone II",
    )
    .expect("copy");

    // All three lanes, every shape the DT2 conditions capture holds: a PROB of 0
    // (which is not "unset"), a FILL with no COND, a COND with no PROB, and the
    // five relative conditions.
    let expected: Vec<Setting> = vec![
        (0, Some(0), Some(true), Some("1:4")),
        (1, Some(5), Some(true), Some("!2:5")),
        (2, Some(10), Some(true), Some("6:6")),
        (3, Some(15), Some(true), Some("4:7")),
        (4, Some(20), Some(true), Some("!8:8")),
        (5, Some(25), Some(true), None),
        (6, None, Some(true), Some("LST")),
        (7, Some(35), None, Some("!LST")),
        (8, Some(40), Some(false), Some("1:2")),
        (9, Some(100), Some(false), Some("2:2")),
        (10, Some(50), Some(false), Some("1:3")),
        (11, Some(55), Some(false), Some("!1:3")),
        (12, Some(60), Some(false), Some("2:3")),
        (13, Some(65), Some(false), Some("!2:3")),
        (14, Some(70), Some(false), Some("3:3")),
    ];
    assert_eq!(settings(&dn2, &r.payload, 0), expected);
}

#[test]
fn settings_on_a_step_with_no_note_do_not_travel() {
    let (dt2, dn2) = (dt2_spec(), dn2_spec());
    let source = payload(DT2_CONDITIONS);

    // The fixture is the witness: sixteen steps of settings over fifteen notes.
    // Step 16 (index 15) holds PROB 75 and nothing to run it on.
    let source_settings = settings(&dt2, &source, 0);
    assert_eq!(source_settings.len(), 16);
    assert_eq!(source_settings[15], (15, Some(75), Some(false), None));
    assert!(
        !track_notes(&kit_of(&dt2, &source), 0)
            .iter()
            .any(|n| n.step == 15),
        "step 16 must have no note for this test to mean anything"
    );

    let target = payload(DN2_FRESH);
    let r = copy_track(
        &dt2,
        &kit_of(&dt2, &source),
        &source,
        0,
        &dn2,
        &target,
        0,
        "the Digitone II",
    )
    .expect("copy");

    let carried = settings(&dn2, &r.payload, 0);
    assert_eq!(carried.len(), 15);
    assert!(
        !carried.iter().any(|(step, ..)| *step == 15),
        "a setting with no trig under it must not travel: {carried:?}"
    );
}

#[test]
fn an_empty_source_track_clears_the_target_track_and_its_conditions() {
    let (dn2, dt2) = (dn2_spec(), dt2_spec());
    let source = payload(DN2_FRESH);
    let target = payload(DT2_CONDITIONS);
    assert_eq!(read_back(&dt2, &target, 0).len(), 15, "target starts full");
    assert_eq!(settings(&dt2, &target, 0).len(), 16);

    let r = copy_track(
        &dn2,
        &kit_of(&dn2, &source),
        &source,
        0,
        &dt2,
        &target,
        0,
        "the Digitakt II",
    )
    .expect("copy");

    assert!(r.notes.is_empty());
    assert!(read_back(&dt2, &r.payload, 0).is_empty());
    // The scrub rule: a step that lost its trig must not leave a condition behind
    // for the next trig to inherit.
    assert!(
        settings(&dt2, &r.payload, 0).is_empty(),
        "{:?}",
        settings(&dt2, &r.payload, 0)
    );
}

// --- The track's PROB default --------------------------------------------------

#[test]
fn the_tracks_prob_default_travels() {
    let (dt2, dn2) = (dt2_spec(), dn2_spec());
    let source = seeded_prob(&payload(DT2_CONDITIONS), 0, 37);
    assert_eq!(read_track_prob(&dt2, &source, 0), Ok(37));

    let target = payload(DN2_FRESH);
    assert_eq!(read_track_prob(&dn2, &target, 0), Ok(100), "target default");

    let r = copy_track(
        &dt2,
        &kit_of(&dt2, &source),
        &source,
        0,
        &dn2,
        &target,
        0,
        "the Digitone II",
    )
    .expect("copy");

    assert_eq!(read_track_prob(&dn2, &r.payload, 0), Ok(37));
}

#[test]
fn the_prob_carry_reaches_the_track_it_names_and_no_other() {
    let (dt2, dn2) = (dt2_spec(), dn2_spec());
    let source = seeded_prob(&payload(DT2_CONDITIONS), 0, 37);
    let target = payload(DN2_FRESH);
    let r = copy_track(
        &dt2,
        &kit_of(&dt2, &source),
        &source,
        0,
        &dn2,
        &target,
        9,
        "the Digitone II",
    )
    .expect("copy");

    assert_eq!(read_track_prob(&dn2, &r.payload, 9), Ok(37));
    for t in (0..16).filter(|t| *t != 9) {
        assert_eq!(
            read_track_prob(&dn2, &r.payload, t),
            Ok(100),
            "track {} PROB moved",
            t + 1
        );
    }
}

// --- p-lock lanes: the translation --------------------------------------------

/// Source lanes for the translation tests, straight off the DT2 capture.
fn dt2_source_lanes() -> Vec<PoolLane> {
    read_track_plocks(&dt2_spec(), &payload(DT2_PLOCKS), 0).expect("track 1")
}

#[test]
fn the_fixture_carries_the_lanes_these_tests_need() {
    // Nothing below means anything if this drifts, and it is cheaper to fail
    // here than to work out why an expectation went quiet.
    let ids: Vec<u8> = dt2_source_lanes().iter().map(|l| l.param_id).collect();
    assert_eq!(ids, vec![44, 65, 46, 74, 63, 64, 62, 29, 30, 31]);
}

#[test]
fn lanes_are_translated_by_parameter_name_not_by_number() {
    let (out, warnings) = plock_lanes_for_target(&dt2_source_lanes(), "DT2", "DN2");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        out.iter().map(|l| l.param_id).collect::<Vec<_>>(),
        // 44→74 filter.cutoff, 65→95 amp.pan, 46→76 filter.envDepth,
        // 74→104 fx.overdrive, 63→93 delay, 64→94 reverb, 62→92 chorus,
        // and the three LFO depths share their numbering on both boxes.
        vec![74, 95, 76, 104, 93, 94, 92, 29, 30, 31],
    );
}

#[test]
fn the_number_that_means_two_different_knobs_is_translated_not_carried() {
    // This is the test the whole module exists for. **74 is overdrive on a DT2
    // and filter frequency on a DN2**, and 44 is filter frequency on a DT2 and
    // nothing curated on a DN2. So a copy that carried paramIds across unchanged
    // would put the DT2's overdrive automation on the DN2's filter — audibly
    // wrong, byte-plausible, and invisible to every other assertion here.
    //
    // Note what would *not* catch it: lanes 29/30/31 are the same number on both
    // boxes, so a suite that only checked the LFO depths would pass with the
    // translation deleted.
    let source = dt2_source_lanes();
    let overdrive = source
        .iter()
        .find(|l| l.param_id == 74)
        .expect("the DT2 capture's overdrive lane");
    let cutoff = source
        .iter()
        .find(|l| l.param_id == 44)
        .expect("the DT2 capture's filter lane");

    let (out, _) = plock_lanes_for_target(&source, "DT2", "DN2");
    let by_source_step = |values: &[(usize, u16)]| values.iter().map(|(s, _)| *s).collect::<Vec<_>>();

    let translated_overdrive = out
        .iter()
        .find(|l| l.param_id == 104)
        .expect("overdrive must land on the DN2's 104");
    assert_eq!(
        by_source_step(&compact_lane(overdrive.clone()).1),
        translated_overdrive
            .values
            .iter()
            .enumerate()
            .filter_map(|(s, v)| v.map(|_| s))
            .collect::<Vec<_>>(),
        "the overdrive lane's steps must arrive unchanged, only its number moves",
    );

    let translated_cutoff = out
        .iter()
        .find(|l| l.param_id == 74)
        .expect("the DT2's filter must land on the DN2's 74");
    assert_eq!(
        by_source_step(&compact_lane(cutoff.clone()).1),
        translated_cutoff
            .values
            .iter()
            .enumerate()
            .filter_map(|(s, v)| v.map(|_| s))
            .collect::<Vec<_>>(),
    );

    // And nothing kept a DT2 number that means something else on a DN2.
    assert!(
        !out.iter().any(|l| l.param_id == 44),
        "44 is not a curated DN2 parameter and must not survive the hop",
    );
}

#[test]
fn translation_goes_through_the_display_axis() {
    // A cross-device value is rescaled out of the source's stored words, onto the
    // shared 0–127 display axis, and into the target's — so a difference in
    // either box's scaling is handled rather than assumed away.
    //
    // Both boxes measured at ×256 on every curated parameter, so the *factor* is
    // currently a no-op and cannot witness anything. The **rounding** can: three
    // lanes in this capture hold words that are not whole MIDI steps, and going
    // through the axis quantises them. A copy that carried the words raw while
    // translating the ids would leave all three unchanged and pass every other
    // assertion in this file.
    let (out, _) = plock_lanes_for_target(&dt2_source_lanes(), "DT2", "DN2");
    let at = |param_id: u8, step: usize| {
        out.iter()
            .find(|l| l.param_id == param_id)
            .and_then(|l| l.values.get(step).copied().flatten())
    };

    // 16386 = 64.0078 × 256 → 64 → 16384.
    assert_eq!(at(74, 4), Some(16384), "filter.cutoff step 5");
    // 18433 = 72.004 × 256 → 72 → 18432, on all three LFO depth lanes.
    assert_eq!(at(29, 12), Some(18432), "lfo1.depth step 13");
    assert_eq!(at(30, 0), Some(18432), "lfo2.depth step 1");
    assert_eq!(at(31, 4), Some(18432), "lfo3.depth step 5");

    // The words that already sat on a whole step come through untouched, which is
    // what says the quantisation above is rounding rather than damage.
    assert_eq!(at(74, 0), Some(0));
    assert_eq!(at(74, 8), Some(32512));
    assert_eq!(at(104, 12), Some(32512));
}

#[test]
fn a_same_box_copy_carries_lanes_byte_exact() {
    // The short-circuit is not an optimisation. The round trip through the
    // display axis throws away the box's sub-MIDI fine bits, so a same-box copy
    // that went through the translation would quantise 16386 to 16384 and 18433
    // to 18432 for no reason at all — losing resolution the destination can hold
    // perfectly well.
    let source = dt2_source_lanes();
    let (out, warnings) = plock_lanes_for_target(&source, "DT2", "DT2");
    assert!(warnings.is_empty());
    assert_eq!(
        out.iter().map(|l| l.param_id).collect::<Vec<_>>(),
        vec![44, 65, 46, 74, 63, 64, 62, 29, 30, 31],
    );
    for (lane, original) in out.iter().zip(&source) {
        assert_eq!(
            lane.values, original.values,
            "lane {} lost its fine bits on a same-box copy",
            lane.param_id
        );
    }
    // Named explicitly, so the two words that carry fine bits cannot go quiet.
    let cutoff = &out[0];
    assert_eq!(cutoff.values[4], Some(16386));
    assert_eq!(out[7].values[12], Some(18433));
}

#[test]
fn an_uncurated_param_id_is_dropped_and_reported() {
    // Every measured paramId is in both tables, so the reachable failure is a
    // lane on a parameter Phase 0 never measured — which is what an imported
    // pattern full of automation this app does not understand looks like.
    let lane = PoolLane {
        lane: 0,
        param_id: 7,
        track: 0,
        values: vec![Some(100), None, Some(200)],
    };
    let (out, warnings) = plock_lanes_for_target(&[lane], "DT2", "DN2");
    assert!(out.is_empty(), "a lane we cannot name must not be guessed at");
    assert_eq!(
        warnings,
        vec![
            "p-lock lane on DT2 parameter 0x07 wasn't copied — digi-roll doesn't know which \
             parameter that is yet, so it can't say what it would be on a DN2"
                .to_string()
        ],
    );
}

#[test]
fn an_uncurated_lane_survives_a_same_box_copy() {
    // The mirror of the test above, and the reason the short-circuit is checked
    // before the tables: on one box a paramId needs no interpretation, so a lane
    // this app cannot name still copies rather than being dropped for want of a
    // meaning it does not need.
    let lane = PoolLane {
        lane: 3,
        param_id: 7,
        track: 0,
        values: vec![Some(100), None, Some(200)],
    };
    let (out, warnings) = plock_lanes_for_target(std::slice::from_ref(&lane), "DN2", "DN2");
    assert!(warnings.is_empty());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].param_id, 7);
    assert_eq!(out[0].values, lane.values);
}

#[test]
fn no_lanes_means_no_lanes_and_no_warnings() {
    assert_eq!(plock_lanes_for_target(&[], "DT2", "DN2"), (vec![], vec![]));
    assert_eq!(plock_lanes_for_target(&[], "DT2", "DT2"), (vec![], vec![]));
}

#[test]
fn an_all_empty_lane_is_translated_but_never_allocated() {
    // `plock_lanes_for_target` translates the id of a lane with no values in it,
    // because it does not know whether the caller is about to fill it; the
    // allocation policy is `apply_track_plocks`'s, and it refuses to claim one of
    // eighty slots to say nothing.
    let lane = PoolLane {
        lane: 0,
        param_id: 44,
        track: 0,
        values: vec![None, None],
    };
    let (out, warnings) = plock_lanes_for_target(&[lane], "DT2", "DN2");
    assert!(warnings.is_empty());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].param_id, 74);
    assert!(out[0].values.iter().all(Option::is_none));
}

#[test]
fn a_cross_device_copy_lands_its_lanes_in_the_target_pool() {
    // The translation, through the whole function and back out of the bytes —
    // which is the only thing that says `apply_track_plocks` was handed the
    // translated lanes rather than the source ones.
    let (dt2, dn2) = (dt2_spec(), dn2_spec());
    let source = payload(DT2_PLOCKS);
    let target = payload(DN2_FRESH);
    let r = copy_track(
        &dt2,
        &kit_of(&dt2, &source),
        &source,
        0,
        &dn2,
        &target,
        0,
        "the Digitone II",
    )
    .expect("copy");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);

    assert_eq!(
        lanes(&dn2, &r.payload, 0),
        vec![
            (74, vec![(0, 0), (4, 16384), (8, 32512)]),
            (95, vec![(4, 0)]),
            (76, vec![(8, 8192)]),
            (104, vec![(12, 32512)]),
            (93, vec![(0, 32512)]),
            (94, vec![(4, 16384)]),
            (92, vec![(8, 8192)]),
            (29, vec![(12, 18432)]),
            (30, vec![(0, 18432)]),
            (31, vec![(4, 18432)]),
        ],
    );
}

#[test]
fn a_same_box_copy_lands_its_lanes_unchanged() {
    let dt2 = dt2_spec();
    let source = payload(DT2_PLOCKS);
    let r = copy_track(
        &dt2,
        &kit_of(&dt2, &source),
        &source,
        0,
        &dt2,
        &source,
        5,
        "the Digitakt II",
    )
    .expect("copy");

    assert_eq!(
        lanes(&dt2, &r.payload, 5),
        vec![
            (44, vec![(0, 0), (4, 16386), (8, 32512)]),
            (65, vec![(4, 0)]),
            (46, vec![(8, 8192)]),
            (74, vec![(12, 32512)]),
            (63, vec![(0, 32512)]),
            (64, vec![(4, 16384)]),
            (62, vec![(8, 8192)]),
            (29, vec![(12, 18433)]),
            (30, vec![(0, 18433)]),
            (31, vec![(4, 18433)]),
        ],
    );
    // And track 1's own lanes are still where they were — a one-track copy shares
    // the pool with the other fifteen and must not move theirs.
    assert_eq!(
        lanes(&dt2, &r.payload, 0),
        lanes(&dt2, &source, 0),
        "the source track's lanes moved"
    );
}

// --- Chord truncation ----------------------------------------------------------

const SIX_VOICES: [(u8, u8); 6] = [(36, 100), (43, 127), (48, 90), (55, 127), (60, 110), (67, 80)];

#[test]
fn keeps_the_four_highest_velocity_notes_and_reports_the_rest() {
    let (dn2, dt2) = (dn2_spec(), dt2_spec());
    let source = dn2_chord(0, &SIX_VOICES);
    assert_eq!(
        track_notes(&kit_of(&dn2, &source), 0).len(),
        6,
        "the synthesized chord must read back as six notes"
    );

    let target = payload(DT2_CONDITIONS);
    let r = copy_track(
        &dn2,
        &kit_of(&dn2, &source),
        &source,
        0,
        &dt2,
        &target,
        0,
        "the Digitakt II",
    )
    .expect("copy");

    // Velocities 127, 127, 110, 100 survive; 90 and 80 do not.
    assert_eq!(pitches(&r.notes), vec![36, 43, 55, 60]);
    // The policy ran here, so the encoder never had to drop anything itself.
    assert_eq!(r.dropped, 0);
    assert_eq!(r.drops.len(), 1);
    assert_eq!(r.drops[0].step, 0);
    assert_eq!(
        r.drops[0].dropped.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        vec![48, 67],
    );
    assert_eq!(
        r.warnings,
        vec!["step 1: the Digitakt II holds 4 notes per trig, so note 48 (vel 90), note 67 \
              (vel 80) were dropped"
            .to_string()],
    );
    assert_eq!(
        read_back(&dt2, &r.payload, 0)
            .iter()
            .map(|m| m.1)
            .collect::<Vec<_>>(),
        vec![36, 43, 55, 60],
    );
}

#[test]
fn keeps_the_lower_pitch_when_velocities_tie() {
    let (dn2, dt2) = (dn2_spec(), dt2_spec());
    let flat: Vec<(u8, u8)> = SIX_VOICES.iter().map(|&(p, _)| (p, 100)).collect();
    let source = dn2_chord(4, &flat);
    let target = payload(DT2_CONDITIONS);
    let r = copy_track(
        &dn2,
        &kit_of(&dn2, &source),
        &source,
        0,
        &dt2,
        &target,
        0,
        "the Digitakt II",
    )
    .expect("copy");

    // All six at one velocity: the four lowest pitches win, keeping the root and
    // body of the voicing rather than the top extensions.
    assert_eq!(pitches(&r.notes), vec![36, 43, 48, 55]);
    assert_eq!(
        r.drops[0].dropped.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        vec![60, 67],
    );
    assert_eq!(
        r.warnings,
        vec!["step 5: the Digitakt II holds 4 notes per trig, so note 60 (vel 100), note 67 \
              (vel 100) were dropped"
            .to_string()],
    );
}

#[test]
fn a_dn2_target_truncates_at_four_too_and_says_so() {
    // Worth pinning because the JS module's own comment says "a DN2 trig has no
    // such limit", and the hardware agrees — the decoder reads six notes back off
    // the synthesized chord above. But **this repo's encoder caps a step at
    // `trig.max_notes` for both layouts**, which is 4 on both boxes, so a
    // DN2→DN2 copy of a five-voice chord loses a note.
    //
    // That ceiling is `encode_track_notes`'s and predates this module. What
    // copy-track is responsible for is that the loss is *reported*: without the
    // truncation policy those two notes would land in `dropped` as a number with
    // no words attached.
    let dn2 = dn2_spec();
    let source = dn2_chord(0, &SIX_VOICES);
    let target = payload(DN2_FRESH_A02);
    let r = copy_track(
        &dn2,
        &kit_of(&dn2, &source),
        &source,
        0,
        &dn2,
        &target,
        0,
        "the Digitone II",
    )
    .expect("copy");

    assert_eq!(pitches(&r.notes), vec![36, 43, 55, 60]);
    assert_eq!(r.dropped, 0);
    assert_eq!(r.drops.len(), 1);
    assert_eq!(
        r.warnings,
        vec!["step 1: the Digitone II holds 4 notes per trig, so note 48 (vel 90), note 67 \
              (vel 80) were dropped"
            .to_string()],
    );
}

#[test]
fn never_truncates_silently() {
    let (dn2, dt2) = (dn2_spec(), dt2_spec());
    let source = dn2_chord(0, &SIX_VOICES);
    let target = payload(DT2_CONDITIONS);
    let r = copy_track(
        &dn2,
        &kit_of(&dn2, &source),
        &source,
        0,
        &dt2,
        &target,
        0,
        "the Digitakt II",
    )
    .expect("copy");

    // One warning per drop, and no empty ones. There are no lane warnings here,
    // so the counts are directly comparable.
    assert_eq!(r.drops.len(), r.warnings.len());
    assert!(r.warnings.iter().all(|w| !w.is_empty()));
}

#[test]
fn a_steps_settings_survive_whichever_notes_do() {
    // Settings are attached before truncation and every note on a step carries
    // the same ones, which is what makes truncation safe. Copy a chord onto a
    // step whose settings we know, and check the survivors kept them.
    let (dn2, dt2) = (dn2_spec(), dt2_spec());
    let source = dn2_chord(0, &SIX_VOICES);
    // Seed the source step with a condition by copying the DT2 capture's, the
    // only committed source of real condition bytes.
    let mut seeded = source.clone();
    digi_protocol::trig_cond::apply_track_trig_settings(
        &dn2,
        &mut seeded,
        0,
        &BTreeMap::from([(
            0u8,
            TrigSetting {
                prob: Some(45),
                fill: Some(true),
                cond: Some("1:4"),
            },
        )]),
    )
    .expect("seeding a condition");

    let target = payload(DT2_CONDITIONS);
    let r = copy_track(
        &dn2,
        &kit_of(&dn2, &seeded),
        &seeded,
        0,
        &dt2,
        &target,
        0,
        "the Digitakt II",
    )
    .expect("copy");

    assert_eq!(pitches(&r.notes), vec![36, 43, 55, 60]);
    for (note, setting) in &r.notes {
        assert_eq!(
            (setting.prob, setting.fill, setting.cond),
            (Some(45), Some(true), Some("1:4")),
            "pitch {} lost the step's settings",
            note.pitch
        );
    }
    assert_eq!(
        settings(&dt2, &r.payload, 0),
        vec![(0, Some(45), Some(true), Some("1:4"))],
    );
}

// --- truncate_chords as a policy, with no bytes involved ----------------------

#[test]
fn truncate_chords_leaves_chords_that_already_fit_alone() {
    let notes = vec![n(0, 60, 100), n(0, 64, 90), n(4, 67, 80)];
    let (kept, drops) = truncate_chords(&notes, 4);
    assert_eq!(kept, notes);
    assert!(drops.is_empty());
}

#[test]
fn truncate_chords_ranks_by_velocity_then_lower_pitch() {
    let notes = vec![n(0, 60, 50), n(0, 64, 90), n(0, 67, 90), n(0, 72, 10)];
    let (kept, drops) = truncate_chords(&notes, 2);
    assert_eq!(pitches(&kept), vec![64, 67], "both at velocity 90");
    assert_eq!(
        drops[0].dropped.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        vec![60, 72],
    );
}

#[test]
fn truncate_chords_treats_each_step_independently() {
    let notes = vec![n(0, 60, 100), n(0, 64, 90), n(0, 67, 80), n(8, 36, 100)];
    let (kept, drops) = truncate_chords(&notes, 2);
    assert_eq!(
        kept.iter().map(|(n, _)| (n.step, n.pitch)).collect::<Vec<_>>(),
        vec![(0, 60), (0, 64), (8, 36)],
    );
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].step, 0);
}

#[test]
fn truncate_chords_returns_notes_sorted_by_step_then_pitch() {
    let notes = vec![n(8, 40, 100), n(0, 67, 100), n(0, 36, 100)];
    let (kept, _) = truncate_chords(&notes, 4);
    assert_eq!(
        kept.iter().map(|(n, _)| (n.step, n.pitch)).collect::<Vec<_>>(),
        vec![(0, 36), (0, 67), (8, 40)],
        "the encoder wants (step, pitch) order",
    );
}

#[test]
fn describe_chord_drops_reads_like_something_a_person_can_act_on() {
    let (_, drops) = truncate_chords(&[n(3, 60, 100), n(3, 64, 90), n(3, 67, 80)], 2);
    assert_eq!(
        describe_chord_drops(&drops, "the Digitakt II"),
        vec!["step 4: the Digitakt II holds 2 notes per trig, so note 67 (vel 80) was dropped"],
    );
}

#[test]
fn describe_chord_drops_says_were_for_more_than_one() {
    let drops = vec![ChordDrop {
        step: 0,
        kept: vec![n(0, 60, 100).0],
        dropped: vec![n(0, 64, 90).0, n(0, 67, 80).0],
    }];
    let line = &describe_chord_drops(&drops, "the box")[0];
    assert!(line.ends_with("note 64 (vel 90), note 67 (vel 80) were dropped"), "{line}");
}

// --- Isolation: everything the copy must not touch ----------------------------

/// Nothing outside the target track's own slice and the two shared pools may
/// move.
///
/// **Wider than the JS test's helper, on purpose.** That one allows changes only
/// in the first 256 bytes of the track's region, because the JS test never passed
/// a `sourcePayload` and so never wrote conditions, PROB or lanes. With those
/// carried, the trig-condition lanes and the PROB byte are in play too — all of
/// them inside the track's own `track.size` slice, which is the honest bound.
fn assert_only_track_and_pools_changed(spec: &Spec, before: &[u8], after: &[u8], track: usize) {
    let track_lo = spec.pattern.tracks_offset + track * spec.track.size;
    let track_hi = track_lo + spec.track.size;
    let pool_lo = spec.pattern.trig_pool;
    let pool_hi = spec.pattern.p_locks_index
        + spec.pattern.num_p_locks * spec.pattern.p_lock_size;

    let diffs = diff_payloads(before, after, 100_000);
    assert!(!diffs.is_empty(), "the copy changed nothing at all");
    for d in &diffs {
        let inside_track = d.offset >= track_lo && d.offset < track_hi;
        let inside_pools = d.offset >= pool_lo && d.offset < pool_hi;
        assert!(
            inside_track || inside_pools,
            "unexpected byte change at {} ({})",
            d.offset,
            describe_offset(spec, d.offset),
        );
    }
}

#[test]
fn only_the_target_track_and_the_shared_pools_change() {
    let (dn2, dt2) = (dn2_spec(), dt2_spec());
    let source = payload(DN2_CHORDS);
    let before = payload(DT2_CONDITIONS);
    let r = copy_track(
        &dn2,
        &kit_of(&dn2, &source),
        &source,
        0,
        &dt2,
        &before,
        5,
        "the Digitakt II",
    )
    .expect("copy");
    assert_only_track_and_pools_changed(&dt2, &before, &r.payload, 5);
}

#[test]
fn the_targets_other_fifteen_tracks_are_untouched() {
    let (dn2, dt2) = (dn2_spec(), dt2_spec());
    let source = payload(DN2_CHORDS);
    // A target that already has notes, conditions *and* lanes on other tracks —
    // so "untouched" is a claim with something to lose.
    let before = payload(DT2_PLOCKS);
    let r = copy_track(
        &dn2,
        &kit_of(&dn2, &source),
        &source,
        0,
        &dt2,
        &before,
        5,
        "the Digitakt II",
    )
    .expect("copy");

    for t in (0..16).filter(|t| *t != 5) {
        assert_eq!(
            read_back(&dt2, &r.payload, t),
            read_back(&dt2, &before, t),
            "track {}'s notes changed",
            t + 1
        );
        assert_eq!(
            settings(&dt2, &r.payload, t),
            settings(&dt2, &before, t),
            "track {}'s conditions changed",
            t + 1
        );
        assert_eq!(
            read_track_prob(&dt2, &r.payload, t),
            read_track_prob(&dt2, &before, t),
            "track {}'s PROB changed",
            t + 1
        );
        assert_eq!(
            lanes(&dt2, &r.payload, t),
            lanes(&dt2, &before, t),
            "track {}'s p-lock lanes changed",
            t + 1
        );
    }
}

#[test]
fn the_targets_kit_name_tempo_and_sounds_are_untouched() {
    let (dn2, dt2) = (dn2_spec(), dt2_spec());
    let source = payload(DN2_CHORDS);
    let before = payload(DT2_CONDITIONS);
    let r = copy_track(
        &dn2,
        &kit_of(&dn2, &source),
        &source,
        0,
        &dt2,
        &before,
        5,
        "the Digitakt II",
    )
    .expect("copy");

    let (a, b) = (kit_of(&dt2, &before), kit_of(&dt2, &r.payload));
    assert_eq!(b.kit.sound_names, a.kit.sound_names);
    assert_eq!(b.name, a.name);
    assert_eq!(b.tempo_bpm, a.tempo_bpm);
    assert_eq!(b.kit_index, a.kit_index);
    assert_eq!(b.kit.name, a.kit.name);
}

#[test]
fn neither_input_payload_is_mutated() {
    let (dn2, dt2) = (dn2_spec(), dt2_spec());
    let source = payload(DN2_CHORDS);
    let target = payload(DT2_CONDITIONS);
    let (source_before, target_before) = (source.clone(), target.clone());
    let r = copy_track(
        &dn2,
        &kit_of(&dn2, &source),
        &source,
        0,
        &dt2,
        &target,
        5,
        "the Digitakt II",
    )
    .expect("copy");

    assert_eq!(source, source_before, "the source payload was written to");
    assert_eq!(target, target_before, "the target payload was written to");
    assert_ne!(r.payload, target_before, "the copy produced nothing");
}

// --- Refusals and the byte-taking wrapper ------------------------------------

#[test]
fn a_track_index_the_pattern_does_not_have_is_refused() {
    let (dn2, dt2) = (dn2_spec(), dt2_spec());
    let source = payload(DN2_CHORDS);
    let target = payload(DT2_CONDITIONS);
    let kit = kit_of(&dn2, &source);

    assert!(copy_track(&dn2, &kit, &source, 16, &dt2, &target, 0, "box").is_err());
    assert!(copy_track(&dn2, &kit, &source, 0, &dt2, &target, 16, "box").is_err());
}

#[test]
fn copy_track_from_bytes_agrees_with_copy_track() {
    let (dn2, dt2) = (dn2_spec(), dt2_spec());
    let source = payload(DN2_CHORDS);
    let target = payload(DT2_CONDITIONS);
    let a = copy_track(
        &dn2,
        &kit_of(&dn2, &source),
        &source,
        0,
        &dt2,
        &target,
        5,
        "the Digitakt II",
    )
    .expect("copy");
    let b = copy_track_from_bytes(&dn2, &source, 0, &dt2, &target, 5, "the Digitakt II")
        .expect("copy from bytes");
    assert_eq!(a.payload, b.payload);
    assert_eq!(a.notes, b.notes);
    assert_eq!(a.warnings, b.warnings);
}

// --- Swing, which must not travel ----------------------------------------------

#[test]
fn swing_does_not_travel_in_either_direction() {
    // **This test exists because its absence let a planted bug through.** A
    // `copy_track` that carried the source's swing onto the target passed all 35
    // tests in this file: four of them already cross a swing boundary — the DT2
    // captures are at 50 and `dn2-fresh-A01` is at 78 — and not one of them
    // looked at the byte. Fourth instance of this repo's swing escape class, and
    // the second time the witness was already committed and simply unasserted.
    //
    // Swing belongs to the whole pattern, so carrying it would let a one-track
    // copy silently re-time the fifteen tracks already in the target slot.
    use digi_protocol::pattern_settings::read_swing;
    let (dt2, dn2) = (dt2_spec(), dn2_spec());

    // DT2 at 50 into a DN2 at 78: the target keeps 78.
    let source = payload(DT2_CONDITIONS);
    let target = payload(DN2_FRESH);
    assert_eq!(read_swing(&dt2, &source), 50, "source fixture swing");
    assert_eq!(read_swing(&dn2, &target), 78, "target fixture swing");
    let r = copy_track(
        &dt2,
        &kit_of(&dt2, &source),
        &source,
        0,
        &dn2,
        &target,
        0,
        "the Digitone II",
    )
    .expect("copy");
    assert_eq!(
        read_swing(&dn2, &r.payload),
        78,
        "the target's swing was pulled down to the source's",
    );

    // And a DN2 at 65 into a DT2 at 50: the target keeps 50.
    let swung = payload("dn2-swing-65.syx");
    let flat = payload(DT2_CONDITIONS);
    assert_eq!(read_swing(&dn2, &swung), 65);
    let r = copy_track(
        &dn2,
        &kit_of(&dn2, &swung),
        &swung,
        0,
        &dt2,
        &flat,
        5,
        "the Digitakt II",
    )
    .expect("copy");
    assert_eq!(
        read_swing(&dt2, &r.payload),
        50,
        "the target's swing was pushed up to the source's",
    );
}

#[test]
fn only_the_target_track_and_the_pools_change_across_a_swing_boundary() {
    // The byte-range half of the test above, and the one that catches a
    // pattern-wide write this file has not thought to name. The pair is chosen so
    // the source and target disagree about swing: with two swing-50 fixtures the
    // assertion is vacuous, which is exactly how the escape happened.
    use digi_protocol::pattern_settings::read_swing;
    let (dt2, dn2) = (dt2_spec(), dn2_spec());
    let source = payload(DT2_PLOCKS);
    let before = payload(DN2_FRESH);
    assert_ne!(
        u16::from(read_swing(&dt2, &source)),
        u16::from(read_swing(&dn2, &before)),
        "the fixtures must disagree about swing or this test proves nothing",
    );

    let r = copy_track(
        &dt2,
        &kit_of(&dt2, &source),
        &source,
        0,
        &dn2,
        &before,
        3,
        "the Digitone II",
    )
    .expect("copy");
    assert_only_track_and_pools_changed(&dn2, &before, &r.payload, 3);
}

// --- Where the truncation limit comes from ------------------------------------

#[test]
fn the_truncation_limit_comes_from_the_target_not_the_source() {
    // **Both boxes hold four notes per trig, so this cannot be witnessed with a
    // real spec** — reading `max_notes` off the source instead of the target
    // gives the same 4 either way, and every other test in this file passes.
    // A spec is a plain struct of hardware facts, so the witness is a target that
    // holds two, which is the same technique `dn2_chord` uses for a chord no
    // capture has: synthesize the case the desk cannot show you.
    //
    // This is also what keeps `CopyResult::dropped` at zero. The encoder caps a
    // step at the *target's* `max_notes`; truncation has to use the same number or
    // the encoder starts silently dropping what the policy thought it had kept.
    let dn2 = dn2_spec();
    let mut narrow = dt2_spec();
    narrow.trig.max_notes = 2;

    let source = dn2_chord(0, &SIX_VOICES);
    let (kept, drops) = digi_protocol::copy_track::track_notes_for_target(
        &dn2,
        &kit_of(&dn2, &source),
        &source,
        0,
        &narrow,
    )
    .expect("truncating for a two-note target");

    assert_eq!(
        pitches(&kept),
        vec![43, 55],
        "a two-note target must keep the two highest velocities",
    );
    assert_eq!(drops.len(), 1);
    assert_eq!(
        drops[0].dropped.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        vec![60, 36, 48, 67],
        "the rest are reported in rank order",
    );
    assert_eq!(
        describe_chord_drops(&drops, "a narrow box")[0],
        "step 1: a narrow box holds 2 notes per trig, so note 60 (vel 110), note 36 (vel 100), \
         note 48 (vel 90), note 67 (vel 80) were dropped",
    );

    // And the source's own limit is not what was used: four would have survived.
    assert_ne!(kept.len(), dn2.trig.max_notes);
}
