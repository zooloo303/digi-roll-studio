//! The p-lock lane pool, against the hardware fixtures.
//!
//! Ported from the read half of `test/plocks.test.js`. Every expected value here
//! was read out of the JS original first — `node --input-type=module -e` against
//! `~/Projects/digi-roll/js/**`, pointed at *these* fixture files — and then
//! written down, so this pins digi-roll's hardware-verified behaviour rather
//! than this port's output.
//!
//! The five DT2 fixtures and one DN2 fixture dated 2026-08-04 are the Phase 0
//! p-lock experiments: knobs locked one at a time on track 1 and the pattern
//! dumped after each, which is how the paramId tables in `protocol::params` were
//! measured. The 2026-08-02 condition captures predate all of it and hold an
//! empty pool, which is what makes them useful here.
//!
//! The write half — the JS suite's `applyTrackPLocks` block — is ported at the
//! bottom of this file. Its one deviation from the JS is where the JS suite runs
//! it: against the two ~16 MB project dumps, which are deliberately not
//! committed (PLAN.md Phase 1). The 2026-08-02 condition captures stand in, and
//! they are the same class of evidence for this purpose — a real pattern whose
//! pool is empty — on the same track indices the JS used (DT2 track 10, DN2
//! track 2). Every expectation was re-derived against *these* files under node
//! (`/tmp/plock-write-derive.mjs`, recipe in this header) before being written
//! down.

mod common;

use common::payload;
use digi_protocol::pattern::{describe_offset, diff_payloads, dn2_spec, dt2_spec, Spec};
use digi_protocol::plocks::{
    apply_track_plocks, free_lane_count, lane_has_trigless_values, read_all_plocks, read_lane,
    read_track_plocks, LaneWrite, PoolLane, FREE, NO_VALUE, VALUE_MAX,
};

const DT2_EMPTY: &str = "digitakt2-A01-conditions-2026-08-02.syx";
const DN2_EMPTY: &str = "digitone2-A01-conditions-2026-08-02.syx";
const DT2_ONE: &str = "digitakt2-A01-plock-fltrfreq-2026-08-04.syx";
const DT2_FOUR: &str = "digitakt2-A01-plock-4params-2026-08-04.syx";
const DT2_ELEVEN: &str = "digitakt2-A01-plock-11params-2026-08-04.syx";
const DT2_FINAL: &str = "digitakt2-A01-plock-final-2026-08-04.syx";
const DN2_FINAL: &str = "digitone2-A01-plock-final-2026-08-04.syx";

/// `(lane, param_id, track, [(step, stored word)])` — the shape the JS reports
/// and the shape these expectations read best in.
fn summarise(lanes: &[PoolLane]) -> Vec<(usize, u8, u8, Vec<(usize, u16)>)> {
    lanes
        .iter()
        .map(|l| {
            let held = l
                .values
                .iter()
                .enumerate()
                .filter_map(|(s, v)| v.map(|v| (s, v)))
                .collect();
            (l.lane, l.param_id, l.track, held)
        })
        .collect()
}

// --- an empty pool -----------------------------------------------------------

#[test]
fn a_pattern_that_never_held_a_lock_has_no_allocated_lane_at_all() {
    for (name, spec) in [(DT2_EMPTY, dt2_spec()), (DN2_EMPTY, dn2_spec())] {
        let payload = payload(name);
        assert_eq!(read_all_plocks(&spec, &payload), vec![], "{name}");
        assert_eq!(free_lane_count(&spec, &payload), 80, "{name}");
        assert_eq!(read_lane(&spec, &payload, 0), None, "{name}");
        assert_eq!(read_lane(&spec, &payload, 79), None, "{name}");
    }
}

#[test]
fn a_free_lane_is_two_ff_bytes_and_then_zeros_not_ffff_values() {
    // Both format docs said `FFFF` for an unused value; the fixtures say
    // otherwise, and the fixtures win. This is the claim the write path will
    // depend on when it frees a lane, so it is pinned here rather than left as
    // prose in the module doc.
    let spec = dt2_spec();
    let payload = payload(DT2_EMPTY);
    let start = spec.pattern.p_locks_index;
    let size = spec.pattern.p_lock_size;
    let region = &payload[start..start + spec.pattern.num_p_locks * size];
    assert_eq!(region.iter().filter(|&&b| b == FREE).count(), 160);
    assert_eq!(region.iter().filter(|&&b| b == 0).count(), 20480);
    // And the region ends exactly where the pattern name begins: no slack.
    assert_eq!(start + spec.pattern.num_p_locks * size, spec.pattern.name_offset);
}

// --- the Phase 0 captures ----------------------------------------------------

#[test]
fn the_first_lane_ever_captured_reads_back_off_the_dt2() {
    // One knob, three steps. 0x0000 / 0x4002 / 0x7F00 — and 0x4002 is the
    // interesting one: it is MIDI 64 plus fine resolution the box keeps in the
    // low byte, which is why a lane value is not the 0–127 number a CC carries.
    let spec = dt2_spec();
    let lanes = read_all_plocks(&spec, &payload(DT2_ONE));
    assert_eq!(summarise(&lanes), [(0, 44, 0, vec![(0, 0), (4, 16386), (8, 32512)])]);
    assert_eq!(free_lane_count(&spec, &payload(DT2_ONE)), 79);
}

#[test]
fn four_knobs_claim_four_lanes_in_the_order_they_were_locked() {
    let spec = dt2_spec();
    assert_eq!(
        summarise(&read_all_plocks(&spec, &payload(DT2_FOUR))),
        [
            (0, 44, 0, vec![(0, 0), (4, 16386), (8, 32512)]),
            (1, 45, 0, vec![(0, 25600)]),
            (2, 65, 0, vec![(4, 0)]),
            (3, 46, 0, vec![(8, 8192)]),
            (4, 74, 0, vec![(12, 32512)]),
        ]
    );
}

#[test]
fn all_eleven_measured_parameters_read_back_with_their_param_ids() {
    // This is the capture the DT2 half of `protocol::params` was measured from,
    // so these paramIds and this table have to agree — the test below checks
    // that they do.
    let spec = dt2_spec();
    let payload = payload(DT2_ELEVEN);
    assert_eq!(
        summarise(&read_all_plocks(&spec, &payload)),
        [
            (0, 44, 0, vec![(0, 0), (4, 16386), (8, 32512)]),
            (1, 45, 0, vec![(0, 25600)]),
            (2, 65, 0, vec![(4, 0)]),
            (3, 46, 0, vec![(8, 8192)]),
            (4, 74, 0, vec![(12, 32512)]),
            (5, 63, 0, vec![(0, 32512)]),
            (6, 64, 0, vec![(4, 16384)]),
            (7, 62, 0, vec![(8, 8192)]),
            (8, 29, 0, vec![(12, 18433)]),
            (9, 30, 0, vec![(0, 18433)]),
            (10, 31, 0, vec![(4, 18433)]),
        ]
    );
    assert_eq!(free_lane_count(&spec, &payload), 69);
}

#[test]
fn every_captured_param_id_is_one_the_curated_table_names() {
    // The join between this module and `protocol::params`: the tables were
    // measured *from* these bytes, so a table edit that drifts from the capture
    // should fail here rather than at a box.
    use digi_protocol::params::{param_by_plock_id, DN2_PARAMS, DT2_PARAMS};

    for (name, spec, table) in [
        (DT2_ELEVEN, dt2_spec(), DT2_PARAMS),
        (DN2_FINAL, dn2_spec(), DN2_PARAMS),
    ] {
        for lane in read_all_plocks(&spec, &payload(name)) {
            assert!(
                param_by_plock_id(table, lane.param_id as u16).is_some(),
                "{name}: lane {} holds paramId {} and no curated parameter claims it",
                lane.lane,
                lane.param_id
            );
        }
    }
}

#[test]
fn the_dn2_numbers_the_same_knobs_differently_and_leaves_a_hole() {
    // Two things at once, both from the hardware. The DN2's filter frequency is
    // paramId 74 where the DT2's is 44 — and 74 on a DT2 is overdrive, which is
    // why a lane remembers whose numbering it uses. And **lane 1 is free while
    // lanes 2–10 are allocated**: the box cleared a lane in place and did not
    // compact the pool, which is the behaviour the write path must imitate.
    let spec = dn2_spec();
    let payload = payload(DN2_FINAL);
    let lanes = read_all_plocks(&spec, &payload);
    assert_eq!(lanes.iter().map(|l| l.lane).collect::<Vec<_>>(), [0, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    assert_eq!(read_lane(&spec, &payload, 1), None, "lane 1 is the hole");
    assert_eq!(
        summarise(&lanes[..1]),
        [(0, 74, 0, vec![(0, 0), (4, 16385), (8, 32512), (12, 8192)])]
    );
    assert_eq!(free_lane_count(&spec, &payload), 70);
}

// --- per-track reads ---------------------------------------------------------

#[test]
fn a_second_tracks_lane_belongs_to_that_track_alone() {
    // The DT2's final capture moved a lock onto track 2 (index 1), and the box
    // put it in lane 1 — the slot the resonance lane had held in the earlier
    // capture. One pool, sixteen tracks, and a read has to sort them out.
    let spec = dt2_spec();
    let payload = payload(DT2_FINAL);
    assert_eq!(
        summarise(&read_track_plocks(&spec, &payload, 1).unwrap()),
        [(1, 44, 1, vec![(0, 25601)])]
    );
    assert_eq!(
        read_track_plocks(&spec, &payload, 0).unwrap().iter().map(|l| l.lane).collect::<Vec<_>>(),
        [0, 2, 3, 4, 5, 6, 7, 8, 9, 10]
    );
    assert!(read_track_plocks(&spec, &payload, 2).unwrap().is_empty());
}

#[test]
fn a_track_the_pattern_does_not_have_is_an_error_not_an_empty_list() {
    // The difference matters: an empty list says "this track has no automation",
    // which is a lie about a track that does not exist.
    let spec = dt2_spec();
    let payload = payload(DT2_FINAL);
    assert_eq!(read_track_plocks(&spec, &payload, 16), Err("no track 16".into()));
    assert!(read_track_plocks(&spec, &payload, 15).is_ok());
}

// --- trigless locks ----------------------------------------------------------

#[test]
fn a_lock_on_a_step_with_no_trig_is_flagged_rather_than_silently_kept() {
    // v1 does not model trigless locks, so a lane holding one is shown read-only
    // and passed through byte-exact. The DT2's first lane holds steps 0, 4 and 8.
    let spec = dt2_spec();
    let lanes = read_all_plocks(&spec, &payload(DT2_ONE));
    let lane = &lanes[0];
    assert!(!lane_has_trigless_values(lane, &[0, 4, 8]));
    assert!(!lane_has_trigless_values(lane, &[0, 4, 8, 12]), "a trig with no lock is fine");
    assert!(lane_has_trigless_values(lane, &[0, 4]), "step 8 has a lock and no trig");
    assert!(lane_has_trigless_values(lane, &[]));
}

// --- the sentinels and a short payload ---------------------------------------

#[test]
fn a_half_free_lane_is_reported_but_belongs_to_no_track() {
    // `FF` in one header byte and a real value in the other. No fixture holds
    // one — this is synthesised by patching a byte — but the JS documents the
    // case deliberately, and the two readers have to disagree about it on
    // purpose: `read_all_plocks` reports it so a diff can see it, and
    // `read_track_plocks` excludes it because a malformed lane is no track's
    // automation. A write path that "tidied" such a lane would be editing bytes
    // it does not understand.
    let spec = dt2_spec();
    let mut payload = payload(DT2_ONE);
    let track_byte = spec.pattern.p_locks_index + 1;
    assert_eq!(payload[track_byte], 0, "lane 0 is track 1 in the capture");
    payload[track_byte] = FREE;

    let all = read_all_plocks(&spec, &payload);
    assert_eq!(all.len(), 1, "still allocated: only one header byte is free");
    assert_eq!(all[0].param_id, 44);
    assert_eq!(all[0].track, FREE);
    for t in 0..16 {
        assert!(read_track_plocks(&spec, &payload, t).unwrap().is_empty(), "track {t}");
    }
    // And free-lane accounting still counts it as taken, which is what matters
    // to a write: the slot is not available.
    assert_eq!(free_lane_count(&spec, &payload), 79);
}

#[test]
fn the_two_sentinels_are_what_the_captures_say_they_are() {
    assert_eq!(NO_VALUE, 0xFFFF);
    assert_eq!(VALUE_MAX, 0xFFFE);
    assert_eq!(FREE, 0xFF);
}

#[test]
fn a_truncated_payload_loses_locks_rather_than_inventing_them() {
    // A deliberate deviation from the JS, which indexes past the end and reads
    // `undefined` — producing a lock with a value of *zero*, which is a legal
    // value for most parameters and so indistinguishable from a real one. Losing
    // the lock is the honest failure; inventing one is not.
    let spec = dt2_spec();
    let full = payload(DT2_ONE);
    let cut = &full[..spec.pattern.p_locks_index + 4];
    let lane = read_lane(&spec, cut, 0).expect("the header survived the cut");
    assert_eq!(lane.param_id, 44);
    assert_eq!(lane.values[0], Some(0), "the one step whose bytes are still there");
    assert!(lane.values[1..].iter().all(Option::is_none));
    // And a lane whose header is past the end is free, not half-read.
    assert_eq!(read_lane(&spec, &full[..spec.pattern.p_locks_index], 0), None);
}

#[test]
fn every_lane_is_as_long_as_the_pattern_has_steps() {
    for (name, spec) in [(DT2_FINAL, dt2_spec()), (DN2_FINAL, dn2_spec())] {
        let spec: Spec = spec;
        for lane in read_all_plocks(&spec, &payload(name)) {
            assert_eq!(lane.values.len(), spec.track.num_steps, "{name} lane {}", lane.lane);
        }
    }
}

// --- the write half: writing lanes keeps the diff minimal --------------------
//
// Ported from `test/plocks.test.js`'s "writing lanes keeps the diff minimal"
// block. See the file header for the one substitution: the JS's project dumps
// are not committed, so the empty-pool condition captures stand in, on the
// track indices the JS used.

/// The two boxes as the JS suite sets them up: `(spec, payload, track, name)`.
fn write_boxes() -> [(Spec, Vec<u8>, usize, &'static str); 2] {
    [
        (dt2_spec(), payload(DT2_EMPTY), 10, "DT2"),
        (dn2_spec(), payload(DN2_EMPTY), 2, "DN2"),
    ]
}

/// A 128-long values array with locks on the given steps — the JS's `sparse`.
fn lane_of(param_id: u8, by_step: &[(usize, u16)]) -> LaneWrite {
    let mut values = vec![None; 128];
    for &(step, v) in by_step {
        values[step] = Some(v);
    }
    LaneWrite::new(param_id, values)
}

/// Apply onto a copy, returning `(payload, warnings)` — the JS's `write`.
fn write_onto(spec: &Spec, from: &[u8], track: usize, lanes: &[LaneWrite]) -> (Vec<u8>, Vec<String>) {
    let mut payload = from.to_vec();
    let warnings = apply_track_plocks(spec, &mut payload, track, lanes).expect("a track that exists");
    (payload, warnings)
}

/// `(lane index, param_id)` per lane on a track, which is what most of these
/// expectations are about.
fn placement(spec: &Spec, payload: &[u8], track: usize) -> Vec<(usize, u8)> {
    read_track_plocks(spec, payload, track)
        .expect("a track that exists")
        .iter()
        .map(|l| (l.lane, l.param_id))
        .collect()
}

fn pool_region(spec: &Spec) -> std::ops::Range<usize> {
    let start = spec.pattern.p_locks_index;
    start..start + spec.pattern.num_p_locks * spec.pattern.p_lock_size
}

#[test]
fn a_lane_write_touches_nothing_outside_the_p_lock_pool() {
    // The minimal-diff rule, which is the whole reason these functions compose
    // onto a payload instead of re-encoding one.
    for (spec, base, track, name) in write_boxes() {
        let (after, _) = write_onto(&spec, &base, track, &[lane_of(0x2a, &[(0, 100), (4, 8000)])]);
        let region = pool_region(&spec);
        let diffs = diff_payloads(&base, &after, 100_000);
        for d in &diffs {
            assert!(
                region.contains(&d.offset),
                "{name}: unexpected byte change at {} ({})",
                d.offset,
                describe_offset(&spec, d.offset)
            );
        }
        // 257 bytes: the lane's `track` header byte plus the 256 value bytes.
        // `param_id` happens to land on a byte that was already 0x2A-free —
        // the count is derived, not reasoned.
        assert_eq!(diffs.len(), 257, "{name}");
    }
}

#[test]
fn a_new_parameter_claims_the_lowest_free_lane_and_writes_only_that_lane() {
    for (spec, base, track, name) in write_boxes() {
        let (after, _) = write_onto(&spec, &base, track, &[lane_of(0x2a, &[(3, 4096)])]);
        let first_lane_end = spec.pattern.p_locks_index + spec.pattern.p_lock_size;
        assert!(
            diff_payloads(&base, &after, 100_000).iter().all(|d| d.offset < first_lane_end),
            "{name}: a byte moved outside lane 0"
        );
        let lanes = read_all_plocks(&spec, &after);
        assert_eq!(lanes.len(), 1, "{name}");
        assert_eq!((lanes[0].lane, lanes[0].param_id, lanes[0].track), (0, 0x2a, track as u8), "{name}");
    }
}

#[test]
fn values_round_trip_and_only_on_the_steps_that_had_them() {
    for (spec, base, track, name) in write_boxes() {
        let (after, _) = write_onto(
            &spec,
            &base,
            track,
            &[lane_of(0x2a, &[(0, 0), (3, 4096), (127, VALUE_MAX)])],
        );
        let lanes = read_track_plocks(&spec, &after, track).unwrap();
        let lane = &lanes[0];
        // 0 is a real value, not "unlocked" — the reason `NO_VALUE` is 0xFFFF
        // and not zero.
        assert_eq!(lane.values[0], Some(0), "{name}");
        assert_eq!(lane.values[3], Some(4096), "{name}");
        assert_eq!(lane.values[127], Some(VALUE_MAX), "{name}");
        assert_eq!(lane.values.iter().filter(|v| v.is_some()).count(), 3, "{name}");
    }
}

#[test]
fn an_unlocked_step_inside_an_allocated_lane_is_marked_ffff() {
    for (spec, base, track, name) in write_boxes() {
        let (after, _) = write_onto(&spec, &base, track, &[lane_of(0x2a, &[(3, 4096)])]);
        let o = spec.pattern.p_locks_index + 2;
        let at = |step: usize| u16::from_be_bytes([after[o + step * 2], after[o + step * 2 + 1]]);
        assert_eq!(at(3), 4096, "{name}");
        assert_eq!(at(0), NO_VALUE, "{name}");
        assert_eq!(at(127), NO_VALUE, "{name}");
    }
}

#[test]
fn each_parameter_gets_one_lane_in_the_order_asked_for() {
    for (spec, base, track, name) in write_boxes() {
        let (after, _) = write_onto(
            &spec,
            &base,
            track,
            &[lane_of(0x2a, &[(1, 10)]), lane_of(0x31, &[(2, 20)])],
        );
        assert_eq!(placement(&spec, &after, track), vec![(0, 0x2a), (1, 0x31)], "{name}");
    }
}

#[test]
fn a_lane_the_track_already_has_is_rewritten_in_place_rather_than_moved() {
    // Whether the box cares about lane order is unmeasured, so the safest write
    // is the one that moves fewest bytes — even when the caller's list has
    // reordered the parameters.
    for (spec, base, track, name) in write_boxes() {
        let (first, _) = write_onto(
            &spec,
            &base,
            track,
            &[lane_of(0x2a, &[(1, 10)]), lane_of(0x31, &[(2, 20)])],
        );
        let (second, _) = write_onto(
            &spec,
            &first,
            track,
            &[lane_of(0x31, &[(2, 99)]), lane_of(0x2a, &[(1, 10)])],
        );
        assert_eq!(placement(&spec, &second, track), vec![(0, 0x2a), (1, 0x31)], "{name}");
        let lanes = read_track_plocks(&spec, &second, track).unwrap();
        assert_eq!(lanes[1].values[2], Some(99), "{name}: the new value did land");
    }
}

#[test]
fn freeing_a_lane_restores_the_exact_form_the_fixtures_hold() {
    // The measured empty form is `FF FF` plus 256 zeros, and this is the test
    // that says so from the write side: the round trip through allocate-then-free
    // has to come back byte-identical, or the scrub is inventing bytes the box
    // never leaves behind.
    for (spec, base, track, name) in write_boxes() {
        let (written, _) = write_onto(&spec, &base, track, &[lane_of(0x2a, &[(1, 10), (5, 20)])]);
        let (freed, _) = write_onto(&spec, &written, track, &[]);
        assert_eq!(diff_payloads(&base, &freed, 100_000), vec![], "{name}");
    }
}

#[test]
fn a_lane_with_no_values_claims_no_slot_at_all() {
    for (spec, base, track, name) in write_boxes() {
        let (after, warnings) = write_onto(&spec, &base, track, &[lane_of(0x2a, &[])]);
        assert_eq!(read_all_plocks(&spec, &after), vec![], "{name}");
        assert_eq!(diff_payloads(&base, &after, 100_000), vec![], "{name}");
        assert_eq!(warnings, Vec::<String>::new(), "{name}: an empty lane is not a problem");
    }
}

#[test]
fn a_freed_lane_goes_straight_to_a_newly_arrived_parameter() {
    // The free list is recomputed after the frees, which is what makes swapping
    // one parameter for another reuse the same slot instead of drifting up the
    // pool every edit.
    for (spec, base, track, name) in write_boxes() {
        let (written, _) = write_onto(&spec, &base, track, &[lane_of(0x2a, &[(1, 10)])]);
        let (swapped, _) = write_onto(&spec, &written, track, &[lane_of(0x31, &[(1, 10)])]);
        assert_eq!(placement(&spec, &swapped, track), vec![(0, 0x31)], "{name}");
    }
}

#[test]
fn a_second_identical_pass_is_byte_identical() {
    for (spec, base, track, name) in write_boxes() {
        let lanes = [lane_of(0x2a, &[(1, 10)]), lane_of(0x31, &[(2, 20)])];
        let (first, _) = write_onto(&spec, &base, track, &lanes);
        let (second, _) = write_onto(&spec, &first, track, &lanes);
        assert_eq!(diff_payloads(&first, &second, 100_000), vec![], "{name}");
    }
}

#[test]
fn the_other_fifteen_tracks_lanes_are_left_exactly_where_they_were() {
    // The pool is shared, so this is the property that makes a one-track write
    // safe at all: another track's lane is neither moved nor freed, and our own
    // track clearing its lanes does not reach theirs.
    const OTHER: usize = 5;
    for (spec, base, track, name) in write_boxes() {
        let (with_other, _) = write_onto(&spec, &base, OTHER, &[lane_of(0x2a, &[(0, 1)])]);
        let (with_both, _) = write_onto(&spec, &with_other, track, &[lane_of(0x2a, &[(9, 2)])]);
        assert_eq!(
            read_track_plocks(&spec, &with_both, OTHER).unwrap(),
            read_track_plocks(&spec, &with_other, OTHER).unwrap(),
            "{name}"
        );
        // Same parameter, different track: our lane is a second one, not a
        // rewrite of theirs. Lane 0 is theirs, so ours lands in lane 1.
        assert_eq!(placement(&spec, &with_both, OTHER), vec![(0, 0x2a)], "{name}");
        assert_eq!(placement(&spec, &with_both, track), vec![(1, 0x2a)], "{name}");
        let (cleared, _) = write_onto(&spec, &with_both, track, &[]);
        assert_eq!(diff_payloads(&with_other, &cleared, 100_000), vec![], "{name}");
    }
}

#[test]
fn a_full_pool_warns_rather_than_failing_the_write() {
    // The rule that keeps `safe_write_track` honest: the notes still land, and
    // the shortfall comes back as something the caller has to show.
    for (spec, base, track, name) in write_boxes() {
        let all_80: Vec<LaneWrite> =
            (0u8..80).map(|k| lane_of(k, &[(0, k as u16)])).collect();
        let (filled, warnings) = write_onto(&spec, &base, track, &all_80);
        assert_eq!(free_lane_count(&spec, &filled), 0, "{name}");
        assert_eq!(warnings, Vec::<String>::new(), "{name}: exactly 80 fits");

        let mut one_too_many = all_80.clone();
        one_too_many.push(lane_of(200, &[(0, 1)]));
        let (after, warnings) = write_onto(&spec, &base, track, &one_too_many);
        assert_eq!(
            warnings,
            vec!["the pattern's 80 p-lock lanes are all in use, so 1 lane (parameter 200) was \
                  not written — free some p-locks on the box first"
                .to_string()],
            "{name}"
        );
        assert_eq!(read_track_plocks(&spec, &after, track).unwrap().len(), 80, "{name}");
    }
}

#[test]
fn one_parameter_asked_for_twice_warns_and_keeps_the_first() {
    for (spec, base, track, name) in write_boxes() {
        let (after, warnings) = write_onto(
            &spec,
            &base,
            track,
            &[lane_of(0x2a, &[(1, 10)]), lane_of(0x2a, &[(2, 20)])],
        );
        assert_eq!(
            warnings,
            vec![format!(
                "p-lock parameter 42 appears twice for track {} — the box holds one lane per \
                 parameter per track, so only the first was written",
                track + 1
            )],
            "{name}"
        );
        let lanes = read_track_plocks(&spec, &after, track).unwrap();
        assert_eq!(lanes.len(), 1, "{name}");
        assert_eq!(lanes[0].values[1], Some(10), "{name}: the first lane's value");
        assert_eq!(lanes[0].values[2], None, "{name}: the second's was not merged in");
    }
}

#[test]
fn the_no_value_sentinel_is_never_written_as_a_value() {
    // The half of the JS's clamp that survives the port. `Option<u16>` refuses
    // the negative and over-wide numbers a JS array could hold, but `0xFFFF`
    // fits the type and would read back as an *unlocked* step — so a lock would
    // vanish silently. Asserted through the read, which is where the loss would
    // show, and on the raw word, which is where it would be.
    for (spec, base, track, name) in write_boxes() {
        let (after, _) = write_onto(
            &spec,
            &base,
            track,
            &[lane_of(0x2a, &[(0, NO_VALUE), (1, VALUE_MAX), (2, 0)])],
        );
        let lanes = read_track_plocks(&spec, &after, track).unwrap();
        assert_eq!(lanes[0].values[0], Some(VALUE_MAX), "{name}: clamped, not lost");
        assert_eq!(lanes[0].values[1], Some(VALUE_MAX), "{name}");
        assert_eq!(lanes[0].values[2], Some(0), "{name}");
        let o = spec.pattern.p_locks_index + 2;
        assert_eq!(u16::from_be_bytes([after[o], after[o + 1]]), VALUE_MAX, "{name}: the raw word");
    }
}

#[test]
fn a_short_values_array_leaves_the_remaining_steps_unlocked() {
    // The JS tolerates a short array by reading `undefined`; the port has to say
    // what it does instead, and it does the same thing.
    for (spec, base, track, name) in write_boxes() {
        let (after, _) = write_onto(
            &spec,
            &base,
            track,
            &[LaneWrite::new(0x2a, vec![None, Some(7)])],
        );
        let lanes = read_track_plocks(&spec, &after, track).unwrap();
        assert_eq!(lanes[0].values.len(), spec.track.num_steps, "{name}");
        assert_eq!(lanes[0].values[1], Some(7), "{name}");
        assert!(lanes[0].values[2..].iter().all(Option::is_none), "{name}");
    }
}

#[test]
fn a_lane_read_off_a_payload_can_be_asked_for_again_unchanged() {
    // `LaneWrite: From<&PoolLane>` exists for the rewrite-what-you-read case, so
    // it has to actually round-trip. The Phase 0 fixtures are the only committed
    // patterns with real lanes, which makes them the right evidence here.
    for (name, spec) in [(DT2_FINAL, dt2_spec()), (DN2_FINAL, dn2_spec())] {
        let base = payload(name);
        let track = 0; // the Phase 0 experiments locked knobs on track 1
        let existing = read_track_plocks(&spec, &base, track).unwrap();
        assert!(!existing.is_empty(), "{name}: the fixture has lanes to re-ask for");
        let asked: Vec<LaneWrite> = existing.iter().map(LaneWrite::from).collect();
        let (after, warnings) = write_onto(&spec, &base, track, &asked);
        assert_eq!(warnings, Vec::<String>::new(), "{name}");
        assert_eq!(diff_payloads(&base, &after, 100_000), vec![], "{name}");
    }
}

#[test]
fn a_track_the_pattern_does_not_have_is_refused_before_a_byte_moves() {
    for (spec, base, _, name) in write_boxes() {
        let mut payload = base.clone();
        assert_eq!(
            apply_track_plocks(&spec, &mut payload, 16, &[]),
            Err("no track 16".to_string()),
            "{name}"
        );
        assert_eq!(payload, base, "{name}: the buffer is untouched");
    }
}

#[test]
fn a_payload_too_short_for_the_pool_is_refused_before_a_byte_moves() {
    // The same refusal `apply_track_trig_settings` makes: a JS typed array drops
    // stores past its end, so the scrub would land and the lanes would not.
    for (spec, base, track, name) in write_boxes() {
        let short_len = spec.pattern.p_locks_index + spec.pattern.p_lock_size;
        let mut payload = base[..short_len].to_vec();
        let before = payload.clone();
        let err = apply_track_plocks(&spec, &mut payload, track, &[lane_of(0x2a, &[(0, 1)])])
            .expect_err("a truncated pool is refused");
        assert!(err.contains("too short for the p-lock pool"), "{name}: {err}");
        assert_eq!(payload, before, "{name}: the buffer is untouched");
    }
}
