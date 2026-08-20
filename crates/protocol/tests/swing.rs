//! Swing: the one pattern-settings byte, ported from `test/swing.test.js`.
//!
//! Every expected value here was read out of the JS original first
//! (`node --input-type=module -e` against `~/Projects/digi-roll/js/**`) and then
//! written down, so these pin digi-roll's hardware-verified behaviour rather than
//! this port's own output.
//!
//! Swing is a per-*pattern* byte and the boxes store it as an offset from
//! straight, which is the whole reason it needs pinning: `Pattern.swing` in the
//! session model is the 50–80 percentage the box displays, and nothing before now
//! could turn that into the byte a dump carries.
//!
//! **One case from the JS suite is missing and cannot be ported here.** Its DT2
//! half reads all 128 patterns out of `dumps/digitakt2-project-*.syx` to show that
//! exactly one edited pattern holds a non-straight byte and the other 127 hold 0.
//! That capture is ~16 MB and deliberately not committed (PLAN.md §6, Phase 1).
//! The DT2 *offset* is still pinned below, by `swing_offset`, and the DN2
//! experiment — which is where the mapping actually came from — ports whole.

mod common;

use common::payload;
use digi_protocol::pattern::{diff_payloads, dn2_spec, dt2_spec, ByteDiff, Spec};
use digi_protocol::pattern_settings::{
    apply_swing, read_swing, swing_offset, SWING_MAX, SWING_MIN,
};

/// A payload big enough to hold the swing byte, for the maths-only cases.
fn blank(spec: &Spec) -> Vec<u8> {
    vec![0u8; spec.pattern.size + 64]
}

fn specs() -> [(&'static str, Spec); 2] {
    [("DT2", dt2_spec()), ("DN2", dn2_spec())]
}

// --- swing encoding, on both specs -------------------------------------------

#[test]
fn reads_a_zero_byte_as_straight() {
    for (name, spec) in specs() {
        assert_eq!(read_swing(&spec, &blank(&spec)), SWING_MIN, "{name}");
    }
}

#[test]
fn round_trips_every_value_the_box_can_hold() {
    for (name, spec) in specs() {
        for v in SWING_MIN..=SWING_MAX {
            let mut buf = blank(&spec);
            assert!(apply_swing(&spec, &mut buf, Some(v as f64)));
            assert_eq!(read_swing(&spec, &buf), v, "{name} at {v}");
        }
    }
}

#[test]
fn stores_the_offset_from_straight_not_the_percentage() {
    for (name, spec) in specs() {
        let off = swing_offset(&spec);
        let mut buf = blank(&spec);
        apply_swing(&spec, &mut buf, Some(78.0));
        assert_eq!(buf[off], 28, "{name}");
        apply_swing(&spec, &mut buf, Some(SWING_MIN as f64));
        assert_eq!(buf[off], 0, "{name}");
    }
}

#[test]
fn moves_exactly_one_byte() {
    for (name, spec) in specs() {
        let before = blank(&spec);
        let mut after = before.clone();
        apply_swing(&spec, &mut after, Some(72.0));
        assert_eq!(
            diff_payloads(&before, &after, 1000),
            vec![ByteDiff {
                offset: swing_offset(&spec),
                sent: Some(0),
                read: Some(22),
            }],
            "{name}"
        );
    }
}

#[test]
fn clamps_out_of_range_requests_instead_of_writing_nonsense() {
    for (name, spec) in specs() {
        let mut buf = blank(&spec);
        apply_swing(&spec, &mut buf, Some(999.0));
        assert_eq!(read_swing(&spec, &buf), SWING_MAX, "{name}");
        apply_swing(&spec, &mut buf, Some(0.0));
        assert_eq!(read_swing(&spec, &buf), SWING_MIN, "{name}");
        apply_swing(&spec, &mut buf, Some(64.6));
        assert_eq!(read_swing(&spec, &buf), 65, "{name}");
    }
}

/// The JS rounds halves toward +∞ (`Math.round`) where Rust's `f64::round` rounds
/// them away from zero. Both give 65 for 64.5 and 64 for 63.5 because swing is
/// positive, but pinning it is what stops the next edit reintroducing the
/// −n.5 disagreement `micro_steps_to_byte` already had to fix once.
#[test]
fn rounds_halves_the_way_the_js_does() {
    for (name, spec) in specs() {
        let mut buf = blank(&spec);
        apply_swing(&spec, &mut buf, Some(64.5));
        assert_eq!(read_swing(&spec, &buf), 65, "{name}");
        apply_swing(&spec, &mut buf, Some(63.5));
        assert_eq!(read_swing(&spec, &buf), 64, "{name}");
    }
}

#[test]
fn treats_none_as_straight_there_is_no_unset_to_store() {
    for (name, spec) in specs() {
        let mut buf = blank(&spec);
        apply_swing(&spec, &mut buf, Some(70.0));
        apply_swing(&spec, &mut buf, None);
        assert_eq!(read_swing(&spec, &buf), SWING_MIN, "{name}");
    }
}

#[test]
fn reads_a_byte_past_the_range_as_straight_rather_than_erroring() {
    // A moved field must not stop a pattern opening.
    for (name, spec) in specs() {
        let off = swing_offset(&spec);
        let mut buf = blank(&spec);
        buf[off] = 0xff;
        assert_eq!(read_swing(&spec, &buf), SWING_MIN, "{name}");
        // The boundary itself: 30 is the top of the range, 31 is past it.
        buf[off] = 30;
        assert_eq!(read_swing(&spec, &buf), SWING_MAX, "{name}");
        buf[off] = 31;
        assert_eq!(read_swing(&spec, &buf), SWING_MIN, "{name}");
    }
}

/// The DT2 half of the JS suite needed the uncommitted 16 MB project dump. The
/// offset it was checking is pinned here instead, against both boxes.
#[test]
fn both_boxes_keep_swing_where_the_mapping_says() {
    assert_eq!(swing_offset(&dt2_spec()), 88764);
    assert_eq!(swing_offset(&dn2_spec()), 88812);
}

// --- the DN2 hardware capture the mapping came from --------------------------
//
// Three real captures, committed under `tests/fixtures/`: one fresh project's A01
// set to swing 78, the untouched A02 alongside it, and the same A01 after the box
// was moved to 65. One edit, one predicted byte.

#[test]
fn reads_78_off_the_pattern_that_was_set_to_78() {
    let spec = dn2_spec();
    assert_eq!(read_swing(&spec, &payload("dn2-fresh-A01.syx")), 78);
}

#[test]
fn reads_65_after_the_box_was_moved_to_65() {
    let spec = dn2_spec();
    assert_eq!(read_swing(&spec, &payload("dn2-swing-65.syx")), 65);
}

#[test]
fn reads_an_untouched_blank_as_straight() {
    let spec = dn2_spec();
    assert_eq!(read_swing(&spec, &payload("dn2-fresh-A02.syx")), SWING_MIN);
}

#[test]
fn is_the_only_byte_that_moved_between_78_and_65() {
    // The experiment itself: change one setting on the box, dump again, and see
    // that precisely one byte differs — at the predicted offset, by the predicted
    // amount.
    let spec = dn2_spec();
    assert_eq!(
        diff_payloads(
            &payload("dn2-fresh-A01.syx"),
            &payload("dn2-swing-65.syx"),
            100_000
        ),
        vec![ByteDiff {
            offset: swing_offset(&spec),
            sent: Some(28),
            read: Some(15),
        }]
    );
}

#[test]
fn writes_back_onto_real_hardware_bytes_without_disturbing_anything_else() {
    let spec = dn2_spec();
    let mut after = payload("dn2-swing-65.syx");
    assert!(apply_swing(&spec, &mut after, Some(78.0)));
    assert_eq!(
        diff_payloads(&after, &payload("dn2-fresh-A01.syx"), 100_000),
        vec![],
        "encoding 78 onto the 65 capture must reproduce the 78 capture byte for byte"
    );
}
