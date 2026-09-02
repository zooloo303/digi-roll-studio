//! The three per-step trig lanes, against the hardware fixtures.
//!
//! Ported from the read half of `test/conditions.test.js`. Every expected value
//! below was read out of the JS original first — `node --input-type=module -e`
//! against `~/Projects/digi-roll/js/**`, pointed at *these* fixture files — and
//! then written down, so this pins digi-roll's hardware-verified behaviour
//! rather than this port's output.
//!
//! The two fixtures were captured 2026-08-02 with known PROB/FILL/COND set by
//! hand on known steps of track 1. The DT2 one also carries a trig that was
//! *deleted* before the capture (step 16, index 15): the box cleared its COND
//! byte and left FILL and PROB behind, which is why the reader does not filter
//! by whether a step's trig is live.
//!
//! The write half of `js/elektron/trig-cond.js` is ported too (Phase 6,
//! 2026-08-18); its suite is `tests/all/trig_write.rs`, the port of
//! `test/trig-write.test.js`. This file stays the read half's.


use std::collections::BTreeMap;

use crate::common::payload;
use digi_protocol::pattern::{decode_pattern_kit, dn2_spec, dt2_spec, Spec, TRIG_ENABLED};
use digi_protocol::trig_cond::{
    read_step_trig_setting, read_track_prob, read_track_trig_settings, TrigSetting,
};

const DT2_FIXTURE: &str = "digitakt2-A01-conditions-2026-08-02.syx";
const DN2_FIXTURE: &str = "digitone2-A01-conditions-2026-08-02.syx";

/// `(step, prob, fill, cond)` in the order the expectations read best.
fn expected(rows: &[(u8, Option<u8>, Option<bool>, Option<&'static str>)]) -> BTreeMap<u8, TrigSetting> {
    rows.iter()
        .map(|&(step, prob, fill, cond)| (step, TrigSetting { prob, fill, cond }))
        .collect()
}

// --- the DT2 fixture ---------------------------------------------------------

#[test]
fn decodes_exactly_the_values_set_on_the_dt2_on_exactly_the_right_steps() {
    let spec = dt2_spec();
    let payload = payload(DT2_FIXTURE);
    // Steps 1-16 as Neil left them; step index is 0-based here. Step 5 had its
    // COND cleared, step 6 its PROB, step 7 its FILL, and step 15 is the
    // deleted trig whose FILL and PROB the box left behind.
    let want = expected(&[
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
        (15, Some(75), Some(false), None),
    ]);
    assert_eq!(read_track_trig_settings(&spec, &payload, 0), Ok(want));
}

#[test]
fn finds_nothing_on_any_other_dt2_track() {
    let spec = dt2_spec();
    let payload = payload(DT2_FIXTURE);
    for t in 1..spec.pattern.num_tracks {
        assert_eq!(
            read_track_trig_settings(&spec, &payload, t),
            Ok(BTreeMap::new()),
            "track {}",
            t + 1
        );
    }
}

#[test]
fn leaves_settings_on_a_step_whose_trig_was_deleted() {
    // The box clears COND on delete but leaves FILL and PROB behind. Step 16
    // (index 15) is dead and still carries bytes; nothing here filters it out,
    // because only the caller knows which steps have notes.
    let spec = dt2_spec();
    let payload = payload(DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).expect("the fixture decodes");
    assert_eq!(kit.tracks[0].steps[15] & TRIG_ENABLED, 0, "step 16 is not live");
    assert_eq!(
        read_step_trig_setting(&spec, &payload, 0, 15),
        Ok(Some(TrigSetting { prob: Some(75), fill: Some(false), cond: None })),
    );
}

#[test]
fn reads_one_step_at_a_time_consistently_with_the_whole_track_read() {
    for (spec, fixture) in [(dt2_spec(), DT2_FIXTURE), (dn2_spec(), DN2_FIXTURE)] {
        let payload = payload(fixture);
        let by_step = read_track_trig_settings(&spec, &payload, 0).expect("track 1");
        for step in 0..spec.track.num_steps {
            assert_eq!(
                read_step_trig_setting(&spec, &payload, 0, step),
                Ok(by_step.get(&(step as u8)).copied()),
                "{} step {step}",
                spec.device,
            );
        }
    }
}

// --- the DN2 fixture ---------------------------------------------------------

#[test]
fn decodes_the_same_way_at_the_same_track_relative_offsets_on_the_dn2() {
    let spec = dn2_spec();
    let payload = payload(DN2_FIXTURE);
    let want = expected(&[
        (0, None, None, Some("PRE")),
        (1, None, None, Some("!8:8")),
        (2, None, None, Some("2:4")),
        (3, None, None, Some("!2:4")),
        (4, Some(45), None, None),
        (5, None, Some(true), None),
        (6, None, Some(false), None),
    ]);
    assert_eq!(read_track_trig_settings(&spec, &payload, 0), Ok(want));
}

#[test]
fn leaves_the_plain_control_trig_with_nothing_stored() {
    let spec = dn2_spec();
    assert_eq!(read_step_trig_setting(&spec, &payload(DN2_FIXTURE), 0, 7), Ok(None));
}

// --- track-level PROB --------------------------------------------------------

#[test]
fn reads_the_track_prob_default_both_boxes_were_left_at() {
    for (spec, fixture) in [(dt2_spec(), DT2_FIXTURE), (dn2_spec(), DN2_FIXTURE)] {
        let payload = payload(fixture);
        for t in 0..spec.pattern.num_tracks {
            assert_eq!(read_track_prob(&spec, &payload, t), Ok(100), "{} track {}", spec.device, t + 1);
        }
    }
}

#[test]
fn reads_an_out_of_range_track_prob_byte_as_a_hundred_rather_than_failing() {
    // 0xAA is what a buffer full of a value no field would legitimately hold
    // looks like — and it is what a field that has *moved* would look like. The
    // pattern still has to open.
    for spec in [dt2_spec(), dn2_spec()] {
        let junk = vec![0xAAu8; spec.pattern.size];
        assert_eq!(read_track_prob(&spec, &junk, 0), Ok(100), "{}", spec.device);
    }
}

#[test]
fn a_junk_lane_byte_reads_as_no_lock_rather_than_a_condition() {
    // Same argument one level down: `0xAA` is past the end of the 76-entry COND
    // menu and past 100%, and FILL is neither 0 nor 1. All three decode to
    // "nothing locked" instead of inventing a value.
    for spec in [dt2_spec(), dn2_spec()] {
        let junk = vec![0xAAu8; spec.pattern.size];
        assert_eq!(read_track_trig_settings(&spec, &junk, 0), Ok(BTreeMap::new()), "{}", spec.device);
    }
}

#[test]
fn refuses_a_track_neither_box_has() {
    for spec in [dt2_spec(), dn2_spec()] {
        let n = spec.pattern.num_tracks;
        let payload = vec![0u8; spec.pattern.size];
        assert!(read_track_trig_settings(&spec, &payload, n).is_err(), "{}", spec.device);
        assert!(read_step_trig_setting(&spec, &payload, n, 0).is_err(), "{}", spec.device);
        assert!(read_track_prob(&spec, &payload, n).is_err(), "{}", spec.device);
    }
}

/// The lanes sit where the mapping said, on both boxes — the one thing that
/// would silently poison every expectation above if it drifted.
#[test]
fn the_three_lanes_sit_where_the_hardware_mapping_put_them() {
    for spec in [dt2_spec(), dn2_spec()] {
        let Spec { track, .. } = &spec;
        assert_eq!((track.trig_cond, track.trig_fill, track.trig_prob), (256, 384, 512), "{}", spec.device);
    }
}
