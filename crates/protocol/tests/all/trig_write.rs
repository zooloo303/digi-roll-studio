//! The write half of the trig-condition lanes — encode + apply, minimal diff.
//!
//! Ported from `test/trig-write.test.js`. Same shape as the minimal-diff
//! property test in `roundtrip.rs`: the point is not that the new bytes land,
//! but that nothing else moves when they do.
//!
//! Every expected value below — down to the per-region diff counts — was
//! derived by running the JS original under node against **these** fixture
//! files first (`node /tmp/trig-write-derive.mjs`, the recipe preserved in this
//! header), so the suite pins digi-roll's hardware-verified behaviour rather
//! than this port's output. The fixtures here are byte-identical to
//! digi-roll's own copies (checked with `cmp` on 2026-08-18).
//!
//! Two deviations from the JS suite, both forced by what is committed here:
//!
//! * The JS runs against the two ~16 MB project dumps on tracks 10 and 2;
//!   those dumps are deliberately not committed (PLAN.md Phase 1), so this
//!   suite runs against the condition captures on track 0 — which is
//!   *stronger* where it differs: track 0 of both captures carries real
//!   box-written conditions on every live step, so the write's scrub is
//!   exercised against hardware bytes rather than against empty lanes.
//! * The JS suite's chord-truncation and cross-device-copy cases live with
//!   `copy-track.js`, which is a later Phase 6 item; its "survives the piano
//!   roll" case goes through `roll-bridge.js`, whose Rust home is
//!   `core::import` and its own tests. Only those cases are deferred, and this
//!   note is where they are recorded rather than quietly dropped.


use std::collections::BTreeMap;

use crate::common::payload;
use digi_protocol::pattern::{
    decode_pattern_kit, describe_offset, diff_payloads, dn2_spec, dt2_spec, encode_track_notes,
    track_notes, Note, Spec, TRIG_ENABLED,
};
use digi_protocol::trig_cond::{
    apply_track_prob, apply_track_trig_settings, read_track_prob, read_track_trig_settings,
    trig_settings_from_notes, TrigSetting,
};
use digi_protocol::conditions::NONE;

const DT2_FIXTURE: &str = "digitakt2-A01-conditions-2026-08-02.syx";
const DN2_FIXTURE: &str = "digitone2-A01-conditions-2026-08-02.syx";
const TRACK: usize = 0;

fn boxes() -> [(Spec, Vec<u8>, &'static str); 2] {
    [
        (dt2_spec(), payload(DT2_FIXTURE), "DT2"),
        (dn2_spec(), payload(DN2_FIXTURE), "DN2"),
    ]
}

/// The track's notes with the JS test's three locks attached: the first three
/// live steps get `{25, ON, 2:4}`, `{—, OFF, !1ST}` and `{0, —, —}`, every
/// other note carries no setting — so a write scrubs whatever the box had
/// there, which is the caller's-notes-are-the-truth rule.
fn notes_with_conditions(spec: &Spec, payload: &[u8]) -> Vec<(Note, TrigSetting)> {
    let kit = decode_pattern_kit(spec, payload).expect("the fixture decodes");
    let notes = track_notes(&kit, TRACK);
    let mut steps: Vec<u8> = notes.iter().map(|n| n.step).collect();
    steps.sort_unstable();
    steps.dedup();
    let locks = BTreeMap::from([
        (steps[0], TrigSetting { prob: Some(25), fill: Some(true), cond: Some("2:4") }),
        (steps[1], TrigSetting { prob: None, fill: Some(false), cond: Some("!1ST") }),
        (steps[2], TrigSetting { prob: Some(0), fill: None, cond: None }),
    ]);
    notes
        .into_iter()
        .map(|n| {
            let setting = locks.get(&n.step).copied().unwrap_or_default();
            (n, setting)
        })
        .collect()
}

/// Encode notes and apply their conditions — exactly what the safe-write flow
/// will do, and what the JS test's `encodeWithConditions` does.
fn encode_with_conditions_on(
    spec: &Spec,
    payload: &[u8],
    track: usize,
    notes: &[(Note, TrigSetting)],
    track_prob: Option<u8>,
) -> Vec<u8> {
    let plain: Vec<Note> = notes.iter().map(|(n, _)| n.clone()).collect();
    let (mut out, dropped) =
        encode_track_notes(spec, payload, track, &plain).expect("the encode succeeds");
    assert_eq!(dropped, 0, "nothing in these fixtures gets dropped");
    apply_track_trig_settings(spec, &mut out, track, &trig_settings_from_notes(notes))
        .expect("the apply succeeds");
    if track_prob.is_some() {
        apply_track_prob(spec, &mut out, track, track_prob).expect("the PROB apply succeeds");
    }
    out
}

fn encode_with_conditions(
    spec: &Spec,
    payload: &[u8],
    notes: &[(Note, TrigSetting)],
    track_prob: Option<u8>,
) -> Vec<u8> {
    encode_with_conditions_on(spec, payload, TRACK, notes, track_prob)
}

/// The extended minimal-diff contract: step words, the shared pool, this one
/// track's three condition lanes and its one PROB-default byte. Everything
/// else must be byte-identical. Returns the per-region diff counts so callers
/// can pin the exact figures node produced.
fn diffs_by_region_on(
    spec: &Spec,
    track: usize,
    before: &[u8],
    after: &[u8],
) -> BTreeMap<&'static str, usize> {
    let base = spec.pattern.tracks_offset + track * spec.track.size;
    let t = &spec.track;
    let regions: [(&'static str, usize, usize); 6] = [
        ("step words", base, base + t.num_steps * 2),
        ("pool", spec.pattern.trig_pool, spec.pattern.p_locks_index),
        ("cond", base + t.trig_cond, base + t.trig_cond + t.num_steps),
        ("fill", base + t.trig_fill, base + t.trig_fill + t.num_steps),
        ("prob", base + t.trig_prob, base + t.trig_prob + t.num_steps),
        ("track prob", base + t.track_prob, base + t.track_prob + 1),
    ];
    let mut counts = BTreeMap::new();
    for d in diff_payloads(before, after, 100_000) {
        let region = regions.iter().find(|&&(_, lo, hi)| d.offset >= lo && d.offset < hi);
        match region {
            Some((name, _, _)) => *counts.entry(*name).or_insert(0) += 1,
            None => panic!(
                "unexpected byte change at {} ({})",
                d.offset,
                describe_offset(spec, d.offset)
            ),
        }
    }
    counts
}

fn diffs_by_region(spec: &Spec, before: &[u8], after: &[u8]) -> BTreeMap<&'static str, usize> {
    diffs_by_region_on(spec, TRACK, before, after)
}

#[test]
fn touches_nothing_outside_the_step_words_the_pool_and_this_tracks_lanes() {
    // The exact counts are node's, against these fixtures: the write claims
    // the same step words (0 diffs there), rewrites the pool records, sets the
    // three locks and scrubs the box's own conditions off the other steps.
    let want: [BTreeMap<&str, usize>; 2] = [
        BTreeMap::from([("cond", 14), ("fill", 14), ("prob", 15), ("pool", 147)]),
        BTreeMap::from([("cond", 4), ("fill", 4), ("prob", 3), ("pool", 16)]),
    ];
    for ((spec, payload, name), want) in boxes().into_iter().zip(want) {
        let notes = notes_with_conditions(&spec, &payload);
        let out = encode_with_conditions(&spec, &payload, &notes, None);
        assert_eq!(diffs_by_region(&spec, &payload, &out), want, "{name}");
    }
}

#[test]
fn leaves_every_other_tracks_lanes_untouched() {
    for (spec, payload, name) in boxes() {
        let notes = notes_with_conditions(&spec, &payload);
        let out = encode_with_conditions(&spec, &payload, &notes, None);
        for other in 0..spec.pattern.num_tracks {
            if other == TRACK {
                continue;
            }
            assert_eq!(
                read_track_trig_settings(&spec, &out, other),
                read_track_trig_settings(&spec, &payload, other),
                "{name} track {}",
                other + 1
            );
        }
    }
}

#[test]
fn round_trips_notes_with_all_three_fields_back_out_again() {
    // Decode the written payload and re-attach what the lanes hold: every note
    // must come back with exactly the trio it was sent with — the three locked
    // steps with their locks, every other step with nothing, the box's old
    // conditions on those steps scrubbed.
    for (spec, payload, name) in boxes() {
        let notes = notes_with_conditions(&spec, &payload);
        let out = encode_with_conditions(&spec, &payload, &notes, None);
        let kit = decode_pattern_kit(&spec, &out).expect("the written payload decodes");
        let stored = read_track_trig_settings(&spec, &out, TRACK).expect("the lanes read");
        let back: Vec<(u8, TrigSetting)> = track_notes(&kit, TRACK)
            .into_iter()
            .map(|n| (n.step, stored.get(&n.step).copied().unwrap_or_default()))
            .collect();
        let sent: Vec<(u8, TrigSetting)> = notes.iter().map(|(n, s)| (n.step, *s)).collect();
        assert_eq!(back, sent, "{name}");
    }
}

#[test]
fn is_byte_identical_on_a_second_pass_writing_twice_changes_nothing() {
    for (spec, payload, name) in boxes() {
        let notes = notes_with_conditions(&spec, &payload);
        let first = encode_with_conditions(&spec, &payload, &notes, None);
        let second = encode_with_conditions(&spec, &first, &notes, None);
        assert_eq!(diff_payloads(&first, &second, 100_000), vec![], "{name}");
    }
}

#[test]
fn writes_the_track_prob_default_without_disturbing_anything_else() {
    for (spec, payload, name) in boxes() {
        let notes = notes_with_conditions(&spec, &payload);
        let out = encode_with_conditions(&spec, &payload, &notes, Some(30));
        // Same regions as before plus exactly the one PROB-default byte.
        diffs_by_region(&spec, &payload, &out); // panics on anything outside
        assert_eq!(read_track_prob(&spec, &out, TRACK), Ok(30), "{name}");
    }
}

#[test]
fn leaves_every_other_tracks_prob_default_alone() {
    for (spec, payload, name) in boxes() {
        let notes = notes_with_conditions(&spec, &payload);
        let out = encode_with_conditions(&spec, &payload, &notes, Some(30));
        for other in 0..spec.pattern.num_tracks {
            if other == TRACK {
                continue;
            }
            assert_eq!(
                read_track_prob(&spec, &out, other),
                read_track_prob(&spec, &payload, other),
                "{name} track {}",
                other + 1
            );
        }
    }
}

#[test]
fn keeps_an_explicit_100_percent_trig_lock_distinct_from_the_track_default() {
    // The user's case, end to end: a 30% track with one trig pinned at 100.
    for (spec, payload, name) in boxes() {
        let mut notes = notes_with_conditions(&spec, &payload);
        let step = notes[0].0.step;
        for (n, s) in notes.iter_mut() {
            if n.step == step {
                s.prob = Some(100);
            }
        }
        let out = encode_with_conditions(&spec, &payload, &notes, Some(30));
        assert_eq!(read_track_prob(&spec, &out, TRACK), Ok(30), "{name}");
        let stored = read_track_trig_settings(&spec, &out, TRACK).expect("the lanes read");
        assert_eq!(
            stored.get(&step),
            Some(&TrigSetting { prob: Some(100), fill: Some(true), cond: Some("2:4") }),
            "{name}"
        );
    }
}

#[test]
fn a_write_to_one_track_scrubs_no_further_than_that_track() {
    // The one case these fixtures can witness a scrub reaching past its track.
    // Every OTHER track's lanes are pristine `FF` here, so a scrub-every-track
    // bug is byte-invisible when the write lands on track 1 itself — planting
    // exactly that bug failed nothing until this test existed. Writing to empty
    // track 6 turns track 1's box-written conditions into the canary: derived
    // under node, they survive intact (16 settings on the DT2, 7 on the DN2)
    // and every changed byte stays inside track 6's own regions.
    for (spec, payload, name) in boxes() {
        let before = read_track_trig_settings(&spec, &payload, TRACK).expect("track 1 reads");
        assert!(!before.is_empty(), "{name}: the canary must be real");
        let notes = vec![(
            Note { step: 0, pitch: 60, velocity: 100, len_steps: 1.0, micro: 0.0 },
            TrigSetting { prob: Some(10), fill: Some(true), cond: Some("PRE") },
        )];
        let out = encode_with_conditions_on(&spec, &payload, 5, &notes, None);
        diffs_by_region_on(&spec, 5, &payload, &out); // panics outside track 6's regions
        assert_eq!(read_track_trig_settings(&spec, &out, TRACK), Ok(before), "{name}");
        let written = read_track_trig_settings(&spec, &out, 5).expect("track 6 reads");
        assert_eq!(
            written,
            BTreeMap::from([(
                0u8,
                TrigSetting { prob: Some(10), fill: Some(true), cond: Some("PRE") }
            )]),
            "{name}"
        );
    }
}

// --- the apply functions on their own ------------------------------------------
//
// The write-half cases of `test/conditions.test.js`, ported here rather than
// into tests/trig_cond.rs so that file stays the read half's. These pin the
// appliers alone, without an encode in front of them.

#[test]
fn apply_alone_scrubs_the_lanes_and_touches_nothing_else() {
    let spec = dt2_spec();
    let payload = payload(DT2_FIXTURE);
    // An empty map scrubs: every lane byte of the track goes to `FF`.
    let mut scrubbed = payload.clone();
    apply_track_trig_settings(&spec, &mut scrubbed, TRACK, &BTreeMap::new()).unwrap();
    for lane in [spec.track.trig_cond, spec.track.trig_fill, spec.track.trig_prob] {
        for step in 0..spec.track.num_steps {
            assert_eq!(lane_at(&spec, &scrubbed, lane, step), NONE, "lane +{lane} step {step}");
        }
    }
    // And nothing outside the track's three lanes moves, checked byte by byte.
    let mut out = payload.clone();
    apply_track_trig_settings(
        &spec,
        &mut out,
        TRACK,
        &BTreeMap::from([(2u8, TrigSetting { prob: Some(33), fill: Some(false), cond: Some("NEI") })]),
    )
    .unwrap();
    let base = spec.pattern.tracks_offset + TRACK * spec.track.size;
    let in_lanes = |i: usize| {
        [spec.track.trig_cond, spec.track.trig_fill, spec.track.trig_prob]
            .iter()
            .any(|lane| i >= base + lane && i < base + lane + spec.track.num_steps)
    };
    let strayed: Vec<usize> =
        (0..payload.len()).filter(|&i| payload[i] != out[i] && !in_lanes(i)).collect();
    assert_eq!(strayed, Vec::<usize>::new());
}

#[test]
fn a_marker_in_another_tracks_lane_survives_the_write() {
    // The JS seeds a byte in track 2's PROB lane and proves writing track 1
    // leaves it — the unit-level twin of the canary test above.
    let spec = dt2_spec();
    let mut payload = payload(DT2_FIXTURE);
    let marker = spec.pattern.tracks_offset + spec.track.size + spec.track.trig_prob + 7;
    payload[marker] = 42;
    let mut out = payload.clone();
    apply_track_trig_settings(
        &spec,
        &mut out,
        TRACK,
        &BTreeMap::from([(0u8, TrigSetting { prob: Some(10), fill: Some(true), cond: Some("PRE") })]),
    )
    .unwrap();
    assert_eq!(out[marker], 42);
}

#[test]
fn round_trips_settings_on_the_edge_steps() {
    // Steps 0, 7, 63 and 127 — the last is the final byte of each lane, where
    // an off-by-one in the lane bounds would land.
    let spec = dt2_spec();
    let mut out = payload(DT2_FIXTURE);
    let written = BTreeMap::from([
        (0u8, TrigSetting { prob: Some(0), fill: Some(true), cond: Some("PRE") }),
        (7u8, TrigSetting { prob: Some(100), fill: Some(false), cond: Some("!8:8") }),
        (63u8, TrigSetting { prob: Some(55), fill: None, cond: None }),
        (127u8, TrigSetting { prob: None, fill: None, cond: Some("2:4") }),
    ]);
    apply_track_trig_settings(&spec, &mut out, TRACK, &written).unwrap();
    assert_eq!(read_track_trig_settings(&spec, &out, TRACK), Ok(written));
}

#[test]
fn track_prob_moves_exactly_the_one_byte_the_spec_names() {
    // The JS asserts the moved-offset *list* is exactly the one spec offset, on
    // a buffer full of a value no field would hold — so a write to the right
    // track but the wrong field, or the right field of the wrong track, shows
    // up as the wrong index rather than as a plausible pass.
    for (spec, name) in [(dt2_spec(), "DT2"), (dn2_spec(), "DN2")] {
        let before = vec![0xAAu8; spec.pattern.size];
        let mut after = before.clone();
        apply_track_prob(&spec, &mut after, 7, Some(30)).unwrap();
        let moved: Vec<usize> = (0..before.len()).filter(|&i| before[i] != after[i]).collect();
        let want = spec.pattern.tracks_offset + 7 * spec.track.size + spec.track.track_prob;
        assert_eq!(moved, vec![want], "{name}");
        // And every legal percentage round-trips through the byte.
        for v in [0u8, 1, 30, 50, 99, 100] {
            apply_track_prob(&spec, &mut after, 3, Some(v)).unwrap();
            assert_eq!(read_track_prob(&spec, &after, 3), Ok(v), "{name} {v}%");
        }
    }
}

// --- stale leftovers on a reused step ------------------------------------------
//
// The DT2 capture carries the hazard the scrub exists for: step 16's trig was
// deleted on the box, and the box cleared its COND byte but left FILL and PROB
// behind.

fn lane_at(spec: &Spec, payload: &[u8], lane: usize, step: usize) -> u8 {
    payload[spec.pattern.tracks_offset + TRACK * spec.track.size + lane + step]
}

#[test]
fn starts_from_a_fixture_that_really_does_carry_leftovers() {
    let spec = dt2_spec();
    let payload = payload(DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).expect("the fixture decodes");
    assert_eq!(kit.tracks[TRACK].steps[15] & TRIG_ENABLED, 0, "step 16's trig is deleted");
    assert_eq!(lane_at(&spec, &payload, spec.track.trig_fill, 15), 0x00);
    assert_eq!(lane_at(&spec, &payload, spec.track.trig_prob, 15), 0x4b);
}

#[test]
fn lets_a_fresh_locked_note_on_that_step_win_over_the_leftovers() {
    let spec = dt2_spec();
    let payload = payload(DT2_FIXTURE);
    let notes = vec![(
        Note { step: 15, pitch: 60, velocity: 100, len_steps: 1.0, micro: 0.0 },
        TrigSetting { prob: Some(10), fill: Some(true), cond: Some("PRE") },
    )];
    let out = encode_with_conditions(&spec, &payload, &notes, None);
    let stored = read_track_trig_settings(&spec, &out, TRACK).expect("the lanes read");
    assert_eq!(
        stored.get(&15),
        Some(&TrigSetting { prob: Some(10), fill: Some(true), cond: Some("PRE") })
    );
}

#[test]
fn clears_the_leftovers_for_a_fresh_unlocked_note_on_that_step() {
    // Without the scrub this note would silently inherit PROB 75 and FILL OFF.
    let spec = dt2_spec();
    let payload = payload(DT2_FIXTURE);
    let notes = vec![(
        Note { step: 15, pitch: 60, velocity: 100, len_steps: 1.0, micro: 0.0 },
        TrigSetting::default(),
    )];
    let out = encode_with_conditions(&spec, &payload, &notes, None);
    assert_eq!(read_track_trig_settings(&spec, &out, TRACK), Ok(BTreeMap::new()));
    for lane in [spec.track.trig_cond, spec.track.trig_fill, spec.track.trig_prob] {
        assert_eq!(lane_at(&spec, &out, lane, 15), NONE, "lane at +{lane}");
    }
}
