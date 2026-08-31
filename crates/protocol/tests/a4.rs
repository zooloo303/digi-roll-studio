//! The gen-1 Analog Four pattern format, against the nine captures it was
//! derived from.
//!
//! **These fixtures are the only copy.** They were extracted from
//! `local/a4-check/*.mmon` on 2026-08-31; `local/` is gitignored, so before this
//! suite existed every capture behind PLAN.md §10 lived on one disk. Each file
//! is one A4 pattern dump, `F0 … F7`, exactly as the box emitted it.
//!
//! The set, and what each one is for:
//!
//! | fixture | what it is |
//! |---|---|
//! | `A16-clear` | a cleared A16 — the baseline every 2026-08-31 diff is against |
//! | `A16-trigless-trk1-step1` | one deliberate trigless trig, which refuted two trig models |
//! | `A16-plock-fltfreq64` | FLTR1 FREQ locked to 64 on SYN1 step 1 |
//! | `A16-plock-freq64-reso100` | RESO added, which separates param id from track |
//! | `A16-plock-plus-syn2-freq64` | the same FREQ on SYN2, which proves the track byte |
//! | `A16-plock-freq64-reso100-base` | a fresh baseline minutes before the next change |
//! | `A16-plock-freq100-reso100` | FREQ 64 → 100 and nothing else, which proves the extension lane |
//! | `A16-plock-reso4-freq64` | four RESO values on one lane, which closed open item 3 |
//! | `A01` | a factory pattern — musical, and the only rich capture here |
//! | `A01-postreset` | A01 after a factory reset, which anchors the trig finding |
//!
//! **A capture whose filename records an intention is not evidence of it.** One
//! `.mmon` in the source set (`A4-transfer-A16-plocks`) had a payload
//! byte-identical to the capture before it — whatever it was taken to record, it
//! recorded nothing — and it is not in this set.
//! [`every_fixture_is_a_distinct_pattern`] is the assertion that keeps that kind
//! of file out.

mod common;

use common::{a4_pattern, fixture_bytes};

use digi_protocol::a4_pattern::{
    build_pattern, build_trig_probe, note_name, parse_pattern, read_track_states, read_track_trigs,
    set_note_trig, track_default_note, TrigState, NO_NOTE, NUM_TRACKS, PAYLOAD_LEN, PROBE_NOTE,
    TRACK_BASE, TRACK_STRIDE, TRIG_BYTE0_POSITIONAL,
};
use digi_protocol::a4_plocks::{
    free_lane_count, is_compacted, orphan_extension_count, read_all_plocks, read_track_plocks,
    NUM_LANES,
};

const CLEAR: &str = "analogfour-A16-clear-2026-08-31.syx";
const TRIGLESS: &str = "analogfour-A16-trigless-trk1-step1-2026-08-31.syx";
const FREQ64: &str = "analogfour-A16-plock-fltfreq64-2026-08-31.syx";
const FREQ64_RESO100: &str = "analogfour-A16-plock-freq64-reso100-2026-08-31.syx";
const PLUS_SYN2: &str = "analogfour-A16-plock-plus-syn2-freq64-2026-08-31.syx";
const BASE: &str = "analogfour-A16-plock-freq64-reso100-base-2026-08-31.syx";
const FREQ100: &str = "analogfour-A16-plock-freq100-reso100-2026-08-31.syx";
const RESO4: &str = "analogfour-A16-plock-reso4-freq64-2026-08-31.syx";
const A01: &str = "analogfour-A01-2026-08-30.syx";
const A01_POSTRESET: &str = "analogfour-A01-postreset-2026-08-31.syx";

const ALL: [&str; 10] = [
    CLEAR, TRIGLESS, FREQ64, FREQ64_RESO100, PLUS_SYN2, BASE, FREQ100, RESO4, A01, A01_POSTRESET,
];

// --- Framing -----------------------------------------------------------------

/// The claim that removed a whole planned change from this port: the gen-2 dump
/// framing in [`digi_protocol::protocol`] reads a gen-1 A4 pattern dump with
/// nothing added — checksum, count, seven-bit packing, slot and payload length.
#[test]
fn the_gen2_framing_parses_every_a4_capture() {
    for name in ALL {
        let p = a4_pattern(name); // panics unless all four checks hold
        assert_eq!(p.payload.len(), PAYLOAD_LEN, "{name}");
    }
    assert_eq!(a4_pattern(CLEAR).slot_name(), "A16");
    assert_eq!(a4_pattern(A01).slot_name(), "A01");
}

/// **The strongest pre-send evidence available without sending**, and the check
/// `local/a4_pattern.py build` makes before it writes a file: decode a message
/// the box itself emitted, re-frame the payload, and get the same bytes back.
///
/// It validates the encode, the checksum, the length field and the ragged final
/// group together, against a witness that cannot be argued with. It is run on
/// all nine rather than one because the thing being tested is the encoder, and a
/// single file cannot show that a payload's high bits land where the box puts
/// them.
#[test]
fn every_capture_survives_decode_then_encode_byte_exactly() {
    for name in ALL {
        let original = fixture_bytes(name);
        let p = a4_pattern(name);
        let rebuilt = build_pattern(p.slot, &p.payload).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(rebuilt, original, "{name}: rebuilt message differs from the box's own");
    }
}

/// A fixture whose name states an outcome is an assertion. Two of these files
/// would be worthless if they were copies of their baselines, and one such file
/// was found in the source captures and left out.
#[test]
fn every_fixture_is_a_distinct_pattern() {
    let payloads: Vec<Vec<u8>> = ALL.iter().map(|n| a4_pattern(n).payload).collect();
    for i in 0..payloads.len() {
        for j in i + 1..payloads.len() {
            assert_ne!(payloads[i], payloads[j], "{} and {} are the same capture", ALL[i], ALL[j]);
        }
    }
}

// --- Trigs -------------------------------------------------------------------

/// **The regression that catches a wrong trig model**, and the reason it is a
/// count rather than a round trip: every one of the three models round-tripped
/// perfectly, and what exposed the two wrong ones was "the box shows 4 trigs and
/// the tool says 19".
///
/// A01 SYN4 holds four trigs — the roots of the progression SYN1 is playing, on
/// the bar lines. Fifteen further steps carry [`TrigState::Residue`], byte 0 bit
/// 0 set with the state bits clear, and the box displays every one of them as
/// empty. DEVELOPMENT.md lesson 16.
#[test]
fn a01_syn4_holds_the_four_trigs_the_box_shows_not_the_nineteen_two_models_counted() {
    let p = a4_pattern(A01);
    let trigs = read_track_trigs(&p.payload, 3).unwrap();
    assert_eq!(
        trigs.iter().map(|t| t.step).collect::<Vec<_>>(),
        vec![1, 17, 33, 49],
        "SYN4 trig steps"
    );
    assert_eq!(
        trigs.iter().map(|t| note_name(t.note.unwrap())).collect::<Vec<_>>(),
        vec!["A4", "G4", "D4", "F4"],
        "the roots, on the bar lines"
    );

    let states = read_track_states(&p.payload, 3).unwrap();
    let residue: Vec<usize> = states
        .iter()
        .enumerate()
        .filter(|(_, s)| **s == TrigState::Residue)
        .map(|(i, _)| i + 1)
        .collect();
    assert_eq!(
        residue,
        vec![2, 3, 5, 7, 9, 11, 13, 15, 19, 35, 44, 48, 50, 51, 53],
        "fifteen steps of residue the box shows as empty"
    );
    // The arithmetic behind "19": both bad models counted the residue.
    assert_eq!(trigs.len() + residue.len(), 19);
}

/// **A layout that reproduces musical sense it was not fitted to.** A01 SYN1
/// carries 32 trigs on every odd step: an arpeggio with a chord change, which is
/// a musician's pattern and not a plausible-looking byte run. The roots of that
/// progression are what SYN4 plays on the bar lines above.
#[test]
fn a01_syn1_is_an_arpeggio_with_a_chord_change() {
    let p = a4_pattern(A01);
    let trigs = read_track_trigs(&p.payload, 0).unwrap();
    assert_eq!(trigs.len(), 32);
    assert!(trigs.iter().all(|t| t.step % 2 == 1), "every trig on an odd step");
    assert!(trigs.iter().all(|t| t.state == TrigState::Note));
    let names: Vec<String> = trigs.iter().map(|t| note_name(t.note.unwrap())).collect();
    assert_eq!(
        names.join(" "),
        "A4 A5 A6 A4 A5 A6 A4 A5 G6 G4 G5 G6 G4 G5 G6 G4 \
         D5 D6 D4 D5 D7 D4 D5 D7 F4 F5 F6 F4 G5 G6 G4 G5"
    );
}

/// The two-byte diff that refuted the model PLAN.md had built off 51 agreeing
/// trigs in A01. A cleared A16 with one deliberate trigless trig on SYN1 step 1
/// changed byte 1 to `0x02` — and byte 0 **stayed clear**, where the old model
/// required it set.
#[test]
fn one_deliberate_trigless_trig_is_two_bytes_and_byte_zero_is_not_one_of_them() {
    let clear = a4_pattern(CLEAR).payload;
    let trigless = a4_pattern(TRIGLESS).payload;

    let diffs: Vec<usize> = (0..PAYLOAD_LEN).filter(|&i| clear[i] != trigless[i]).collect();
    // Byte 5 is SYN1 step 1's second trig byte; 12,962 is the slot marker, which
    // moves because saving the pattern is what made the second capture possible.
    assert_eq!(diffs, vec![5, 12_962], "two bytes in 12,974");
    assert_eq!((clear[4], clear[5]), (0x00, 0x00));
    assert_eq!((trigless[4], trigless[5]), (0x00, 0x02));

    assert_eq!(read_track_trigs(&clear, 0).unwrap().len(), 0);
    let trigs = read_track_trigs(&trigless, 0).unwrap();
    assert_eq!(trigs.len(), 1);
    assert_eq!(trigs[0].step, 1);
    assert_eq!(trigs[0].state, TrigState::Trigless);
    assert_eq!(trigs[0].note, None, "a trigless trig plays nothing");
}

/// A cleared pattern has nothing on any of the six tracks — including FX and CV,
/// which have trig lanes like the rest.
#[test]
fn a_cleared_pattern_has_no_trigs_anywhere() {
    let p = a4_pattern(CLEAR).payload;
    for track in 0..NUM_TRACKS {
        assert!(read_track_trigs(&p, track).unwrap().is_empty(), "track {track}");
        assert!(
            read_track_states(&p, track).unwrap().iter().all(|s| *s == TrigState::Empty),
            "track {track} has no residue either"
        );
    }
}

/// Byte 0 bit 3 is positional, not state: in a pattern with nothing in it the
/// first trig byte reads `00 08 00 08 …` for all 64 steps of all six tracks. It
/// carries no per-step information, which is why a write ORs rather than
/// assigns.
#[test]
fn the_first_trig_byte_is_positional_in_an_empty_pattern() {
    let p = a4_pattern(CLEAR).payload;
    for track in 0..NUM_TRACKS {
        let base = TRACK_BASE + track * TRACK_STRIDE;
        for step in 0..64 {
            let expected = if step % 2 == 0 { 0x00 } else { 0x08 };
            assert_eq!(
                p[base + step * 2], expected,
                "track {track} step {} byte 0",
                step + 1
            );
        }
    }
}

/// **The factory reset anchors the trig finding to the exact bytes it was made
/// against**, which mattered because the reset happened between the capture and
/// the observation of an unlit LED.
///
/// A01 before and after differs by one byte in 12,974, and the one that drifted
/// is the per-track default note on SYN1 — **it moved alone**, which is
/// independent confirmation that `+448` is per-track and not per-step.
#[test]
fn a_factory_reset_restored_a01_to_within_one_byte() {
    let before = a4_pattern(A01).payload;
    let after = a4_pattern(A01_POSTRESET).payload;

    let diffs: Vec<usize> = (0..PAYLOAD_LEN).filter(|&i| before[i] != after[i]).collect();
    assert_eq!(diffs, vec![TRACK_BASE + 448], "SYN1's default note, and nothing else");
    assert_eq!(track_default_note(&before, 0).unwrap(), 0x45);
    assert_eq!(track_default_note(&after, 0).unwrap(), 0x3c);

    // Every trig byte, note lane and per-step lane is identical, so the trig
    // expectations above hold against both captures.
    for track in 0..NUM_TRACKS {
        assert_eq!(
            read_track_trigs(&before, track).unwrap(),
            read_track_trigs(&after, track).unwrap(),
            "track {track}"
        );
    }
}

/// A cleared pattern's per-track defaults open `30 64 0e 00 00 00 40` — default
/// note C4, velocity 100, length 14, centre 64 — on every track. `0x30` is the
/// byte a written note came back from the box's screen as **C4**, which is where
/// the octave correction came from.
#[test]
fn a_cleared_pattern_defaults_to_c4_on_every_track() {
    let p = a4_pattern(CLEAR).payload;
    for track in 0..NUM_TRACKS {
        let d = track_default_note(&p, track).unwrap();
        assert_eq!(d, 0x30, "track {track}");
        assert_eq!(note_name(d), "C4");
    }
}

/// The one A4 write hardware has confirmed, reproduced from the fixture it was
/// made against: `0x30` onto SYN1 step 1 of a cleared A16, which the box then
/// displayed as C4.
#[test]
fn the_confirmed_write_reproduces_and_reframes() {
    let p = a4_pattern(CLEAR);
    let mut payload = p.payload.clone();
    set_note_trig(&mut payload, 0, 0, Some(0x30)).unwrap();

    assert_eq!((payload[4], payload[5]), (0x01, 0xc1));
    let trigs = read_track_trigs(&payload, 0).unwrap();
    assert_eq!(trigs.len(), 1);
    assert_eq!(trigs[0].state, TrigState::Note);
    assert_eq!(note_name(trigs[0].note.unwrap()), "C4");

    // Three bytes changed and no more: the two trig bytes' worth of state, and
    // the note. A minimal diff is PLAN.md §7 rule 1's second clause.
    let diffs: Vec<usize> = (0..PAYLOAD_LEN).filter(|&i| p.payload[i] != payload[i]).collect();
    assert_eq!(diffs, vec![4, 5, 132]);

    // And the result is a message the box's own checksum would accept.
    let msg = build_pattern(p.slot, &payload).unwrap();
    let reparsed = digi_protocol::a4_pattern::parse_pattern(&msg).unwrap();
    assert_eq!(reparsed.payload, payload);
    assert_eq!(reparsed.slot, p.slot);
}

/// **Residue appears at both parities of the positional bit, and always with an
/// unset note lane.** Measured here rather than reasoned about, because it is
/// what [`build_trig_probe`] authors and a probe that authored the wrong bytes
/// would test nothing.
///
/// A01 SYN4 holds fifteen residue steps against four live ones. Four of the
/// fifteen read `(09,c0)` — byte 0 carrying [`TRIG_BYTE0_POSITIONAL`] *and* the
/// residue bit — so byte 0 bit 0 and bit 3 are independently set in real data,
/// which is the thing a single-parity capture could not have shown.
///
/// Every one of the fifteen has [`NO_NOTE`] in the note lane. So the residue the
/// box leaves behind is a note trig that took the **track default** and then lost
/// its state bits, not one whose own note byte was erased.
#[test]
fn a01_syn4_residue_carries_an_unset_note_lane_at_both_parities() {
    let p = a4_pattern(A01).payload;
    let states = read_track_states(&p, 3).unwrap();

    let residue: Vec<usize> = (0..64).filter(|&i| states[i] == TrigState::Residue).collect();
    assert_eq!(
        residue.iter().map(|i| i + 1).collect::<Vec<_>>(),
        vec![2, 3, 5, 7, 9, 11, 13, 15, 19, 35, 44, 48, 50, 51, 53],
        "fifteen residue steps, and read_track_trigs shows none of them"
    );
    assert_eq!(read_track_trigs(&p, 3).unwrap().len(), 4, "the four the box lights");

    let mut parities = std::collections::BTreeSet::new();
    for &i in &residue {
        let o = TRACK_BASE + 3 * TRACK_STRIDE + i * 2;
        assert_eq!(p[o + 1], 0xc0, "step {} byte 1", i + 1);
        assert_eq!(p[o] & 0x01, 0x01, "step {} residue bit", i + 1);
        assert_eq!(
            p[TRACK_BASE + 3 * TRACK_STRIDE + 128 + i],
            NO_NOTE,
            "step {} note lane: every residue step took the track default",
            i + 1
        );
        parities.insert(p[o]);
    }
    assert_eq!(
        parities.into_iter().collect::<Vec<_>>(),
        vec![0x01, 0x09],
        "both parities of the positional bit occur beside the residue bit"
    );
}

/// **The write experiment, pinned on this side of the cable.** PLAN.md §10 open
/// item 2 is the one question about this format a capture cannot answer, and
/// this is everything about it that can be checked without a box: that the
/// authored bytes are the bytes the box itself writes for each of the four
/// states, that the reader and the hand-written prediction agree on all seven
/// steps, and that the result is a message the A4's own checksum accepts.
///
/// The front-panel half **ran on 2026-08-31 (A4 0195) and every prediction
/// held**: steps 3, 5 and 12 lit, the other 61 dark, and 3 and 12 shown as
/// trigless trigs rather than merely lit. So the box reads these bytes the way
/// it writes them, and this test is now the regression that keeps the authored
/// bytes equal to the ones hardware accepted.
#[test]
fn the_trig_probe_authors_the_four_states_and_predicts_three_lit_leds() {
    let baseline = a4_pattern(TRIGLESS);
    let probe = build_trig_probe(&baseline).unwrap();

    assert_eq!(probe.slot, baseline.slot, "the probe overwrites the slot it came from");
    assert_eq!(probe.track, 0, "SYN1");
    assert_eq!(probe.expected_lit_steps(), vec![3, 5, 12], "the whole prediction");

    // The bytes, against the four states the box's own screen established.
    let expected: [(usize, (u8, u8), u8, TrigState); 7] = [
        (1, (0x00, 0x00), NO_NOTE, TrigState::Empty),
        (3, (0x00, 0x02), NO_NOTE, TrigState::Trigless),
        (5, (0x01, 0xc1), PROBE_NOTE, TrigState::Note),
        (7, (0x00, 0x00), NO_NOTE, TrigState::Empty),
        (9, (0x01, 0xc0), NO_NOTE, TrigState::Residue),
        (10, (0x09, 0xc0), NO_NOTE, TrigState::Residue),
        (12, (0x08, 0x02), NO_NOTE, TrigState::Trigless),
    ];
    for (i, &(step, bytes, note, state)) in expected.iter().enumerate() {
        let s = &probe.steps[i];
        assert_eq!(s.step, step);
        assert_eq!(s.bytes, bytes, "step {step} trig bytes");
        assert_eq!(s.note, note, "step {step} note lane");
        assert_eq!(s.state, state, "step {step} state");
        // The reader and the hand-written prediction are independent, and this
        // is where they have to meet. PROBE_STEPS deliberately does not compute
        // its column from `is_live`.
        assert_eq!(
            s.state.is_live(),
            s.expect_lit,
            "step {step}: the reader and the prediction disagree"
        );
    }

    // The two residue steps put the same state either side of the positional
    // bit, which one parity could not have separated.
    assert_eq!(probe.steps[4].bytes.0 & TRIG_BYTE0_POSITIONAL, 0);
    assert_eq!(probe.steps[5].bytes.0 & TRIG_BYTE0_POSITIONAL, TRIG_BYTE0_POSITIONAL);

    // Ten bytes changed and no more. Nothing outside SYN1's trig and note lanes
    // moves, so a surprise on the screen is about these ten bytes.
    let diffs: Vec<usize> =
        (0..PAYLOAD_LEN).filter(|&i| baseline.payload[i] != probe.payload[i]).collect();
    assert_eq!(diffs, vec![5, 9, 12, 13, 20, 21, 22, 23, 27, 136]);
    assert!(diffs.iter().all(|&i| i < TRACK_BASE + TRACK_STRIDE), "SYN1 only");

    // And it frames as a message the box would accept.
    let msg = probe.build().unwrap();
    let reparsed = parse_pattern(&msg).unwrap();
    assert_eq!(reparsed.payload, probe.payload);
    assert_eq!(reparsed.slot, baseline.slot);
}

/// A baseline with something already on the probe's steps would frame and send
/// perfectly well, and its predictions would silently not hold. So the wrong
/// baseline is an error rather than a caveat — including the cleared A16, which
/// is the obvious file to reach for and lacks the trigless trig step 1 needs.
#[test]
fn the_trig_probe_refuses_a_baseline_it_cannot_predict() {
    let err = build_trig_probe(&a4_pattern(CLEAR)).unwrap_err();
    assert!(err.contains("trigless trig on SYN1 step 1"), "{err}");

    // A01 has the trigless requirement failed too, but check the second guard
    // directly: take the right baseline and occupy one of the bare steps.
    let mut baseline = a4_pattern(TRIGLESS);
    set_note_trig(&mut baseline.payload, 0, 8, Some(0x30)).unwrap();
    let err = build_trig_probe(&baseline).unwrap_err();
    assert!(err.contains("probe step 9 must start bare"), "{err}");
}

// --- The p-lock pool ---------------------------------------------------------

/// In a cleared pattern all 256 `FF` bytes in the 8,448-byte region sit at
/// predicted header positions with the other 8,192 zero. **The geometry is
/// forced by the data rather than fitted to it.**
#[test]
fn a_cleared_pool_is_128_free_lanes_and_nothing_else() {
    let p = a4_pattern(CLEAR).payload;
    assert!(read_all_plocks(&p).unwrap().is_empty(), "neither A16 nor A01 carries a p-lock");
    assert_eq!(free_lane_count(&p).unwrap(), NUM_LANES);
    assert_eq!(orphan_extension_count(&p).unwrap(), 0);
    assert!(is_compacted(&p).unwrap());

    let region = &p[digi_protocol::a4_plocks::POOL_BASE..digi_protocol::a4_plocks::POOL_END];
    assert_eq!(region.len(), 8_448);
    assert_eq!(region.iter().filter(|&&b| b == 0xff).count(), 256);
    assert_eq!(region.iter().filter(|&&b| b == 0x00).count(), 8_192);
}

/// A01 carries no p-lock either — which is what makes one finding in PLAN.md §10
/// hold: the locks seen on the box after the first written pattern **cannot have
/// come from the message**, whose lanes were all `FF FF`. They were pre-existing
/// box or kit state the write did not disturb.
#[test]
fn a01_carries_no_plock_so_the_first_write_cannot_have_set_one() {
    for name in [A01, A01_POSTRESET] {
        assert!(read_all_plocks(&a4_pattern(name).payload).unwrap().is_empty(), "{name}");
    }
}

/// **The pool hypothesis, confirmed to the byte.** It was written down with a
/// sharp test before the capture: lock one knob on one step and exactly one lane
/// should change its header from `FF FF` to a parameter id, with the value at
/// that step's index.
///
/// What actually arrived was that, plus a second lane — see
/// [`the_extension_lane_carries_the_fine_half_of_one_value`].
#[test]
fn locking_one_knob_allocates_one_lane_with_the_gen2_header() {
    let p = a4_pattern(FREQ64).payload;
    let lanes = read_all_plocks(&p).unwrap();
    assert_eq!(lanes.len(), 1, "the 80 80 lane is half a value, not a lane of its own");

    let l = &lanes[0];
    assert_eq!(l.lane, 0);
    assert_eq!(l.param_id, 0x22, "FLTR1 FREQ");
    assert_eq!(l.track, 0, "SYN1");
    assert_eq!(l.values[0], Some(64), "the coarse byte is the displayed value");
    assert!(l.values[1..].iter().all(Option::is_none), "the other 63 steps carry no lock");
    assert_eq!(free_lane_count(&p).unwrap(), NUM_LANES - 2);
}

/// **`22 00` alone cannot separate "the second byte is the track" from "the
/// second byte is always zero"** — SYN1 is track 0. Locking the same parameter
/// on SYN2 settles it, the same way A01's slot 0 could not settle where the
/// checksum starts and A16's slot 15 could.
#[test]
fn the_same_parameter_on_syn2_proves_the_second_header_byte_is_the_track() {
    let p = a4_pattern(PLUS_SYN2).payload;
    let lanes = read_all_plocks(&p).unwrap();
    assert_eq!(
        lanes.iter().map(|l| (l.param_id, l.track)).collect::<Vec<_>>(),
        vec![(0x22, 0), (0x22, 1), (0x23, 0)],
        "param ids are per parameter, not per track: 0x22 appears on both SYN1 and SYN2"
    );

    let syn1 = read_track_plocks(&p, 0).unwrap();
    let syn2 = read_track_plocks(&p, 1).unwrap();
    assert_eq!(syn1.len(), 2, "SYN1 holds FREQ and RESO");
    assert_eq!(syn2.len(), 1, "SYN2 holds FREQ");
    assert_eq!(syn2[0].values[0], Some(64));
}

/// **Gen-1 compacts the pool, and gen-2 does not.** Adding SYN2's lock moved
/// SYN1's existing RESO lane from index 2 to index 4, with the new pair inserted
/// ahead of it, leaving the lanes ordered by `(param_id, track)`.
///
/// [`digi_protocol::plocks`] documents the opposite for the digis, and
/// [`digi_protocol::plocks::apply_track_plocks`]'s scrub-then-write policy is
/// built on it. **A gen-1 lane index does not survive an edit on the box.**
#[test]
fn adding_a_lock_reorders_the_lanes_the_gen2_boxes_would_have_left_alone() {
    let before = a4_pattern(FREQ64_RESO100).payload;
    let after = a4_pattern(PLUS_SYN2).payload;

    let reso = |p: &[u8]| {
        read_all_plocks(p).unwrap().into_iter().find(|l| l.param_id == 0x23).unwrap().lane
    };
    assert_eq!(reso(&before), 2, "RESO before SYN2's lock arrived");
    assert_eq!(reso(&after), 4, "and after: moved, not left in place");

    for (name, p) in [(FREQ64, &a4_pattern(FREQ64).payload), (FREQ64_RESO100, &before), (PLUS_SYN2, &after)] {
        assert!(is_compacted(p).unwrap(), "{name}");
        assert_eq!(orphan_extension_count(p).unwrap(), 0, "{name}");
    }
}

/// **The capture that separated three readings of `80 80` by changing one
/// thing.** FREQ 64 → 100, with RESO left alone as a control, changed two bytes
/// in the whole 12,974-byte payload: the coarse byte and the extension's.
///
/// So the extension is bound to the lane before it and carries a second byte of
/// the same value. An end-of-pool marker or a count could not move when a value
/// moved, and a companion field would not track it this way.
#[test]
fn the_extension_lane_carries_the_fine_half_of_one_value() {
    let base = a4_pattern(BASE).payload;
    let changed = a4_pattern(FREQ100).payload;

    let diffs: Vec<(usize, u8, u8)> = (0..PAYLOAD_LEN)
        .filter(|&i| base[i] != changed[i])
        .map(|i| (i, base[i], changed[i]))
        .collect();
    assert_eq!(
        diffs,
        vec![(4_512, 0x40, 0x64), (4_578, 0x17, 0x60)],
        "lane 0's step-1 coarse byte, and lane 1's — two bytes, nothing else"
    );

    let lane = &read_all_plocks(&changed).unwrap()[0];
    assert_eq!(lane.param_id, 0x22);
    assert_eq!(lane.ext_lane, Some(1));
    assert_eq!(lane.values[0], Some(100), "the coarse byte is the display: 0x64");
    assert_eq!(lane.fine.as_ref().unwrap()[0], Some(0x60));
    assert_eq!(lane.word(0), Some(0x6460), "the same 16-bit quantity gen-2 stores inline");

    // The control: RESO came back byte-identical, which is what it was locked
    // for. A capture that proves the box rewrote nothing but the one lock.
    let reso_of = |p: &[u8]| {
        read_all_plocks(p).unwrap().into_iter().find(|l| l.param_id == 0x23).unwrap()
    };
    assert_eq!(reso_of(&base).values, reso_of(&changed).values);
}

/// **FLTR1 FREQ has allocated an extension in every capture and RESO never
/// has**, now across six captures including the four-value RESO lane that settled
/// why. This was the shape of open item 3: either RESO was integer-valued or the
/// box omitted an all-zero extension, and no capture *we had* could say which.
/// [`four_reso_values_on_one_lane_allocate_no_extension`] is the one that could,
/// and it is integer-valued — so this is now a rule for RESO and still only an
/// observation for every parameter whose id this box has not mapped.
#[test]
fn freq_always_has_an_extension_and_reso_never_does() {
    for name in [FREQ64, FREQ64_RESO100, PLUS_SYN2, BASE, FREQ100, RESO4] {
        for lane in read_all_plocks(&a4_pattern(name).payload).unwrap() {
            match lane.param_id {
                0x22 => assert!(lane.ext_lane.is_some(), "{name}: FREQ lane {}", lane.lane),
                0x23 => assert!(lane.ext_lane.is_none(), "{name}: RESO lane {}", lane.lane),
                other => panic!("{name}: unexpected param id {other:#04x}"),
            }
        }
    }
}

/// **Open item 3, answered on the box 2026-08-31: FLTR1 RESO is integer-valued.**
///
/// Four RESO locks on one SYN1 lane at **0, 50, 90 and 127** — the ends of the
/// range and two points inside it — and the pool allocated **no extension lane at
/// all**. The competing reading, that the box omits an extension whose fine bytes
/// are all zero, would need all four of those to have landed on a fine byte of
/// exactly zero: four independent 1-in-256 accidents.
///
/// 0 and 127 in the same lane are worth their own line. RESO spans the full
/// 0..=127 as integers, which is what a parameter with 128 discrete positions
/// looks like and what a parameter with sub-unit resolution does not.
///
/// **The encoder rule this licenses** — emit an extension iff some fine byte is
/// non-zero — was the same under either answer, so what this closes is confidence
/// rather than the rule. The pool writer is still blocked, on the *other* unknown
/// in [`digi_protocol::a4_plocks`]'s module doc: whether the box requires the
/// compacted order it produces. That one is a write test.
#[test]
fn four_reso_values_on_one_lane_allocate_no_extension() {
    let p = a4_pattern(RESO4).payload;
    let lanes = read_all_plocks(&p).unwrap();

    let reso = lanes.iter().find(|l| l.param_id == 0x23).expect("a RESO lane");
    assert_eq!(reso.track, 0, "SYN1");
    assert_eq!(reso.ext_lane, None, "no extension lane — the whole finding");
    assert!(reso.fine.is_none());

    let locked: Vec<(usize, u8)> =
        (0..64).filter_map(|s| reso.values[s].map(|v| (s + 1, v))).collect();
    assert_eq!(
        locked,
        vec![(1, 0), (5, 50), (9, 90), (13, 127)],
        "four distinct values including both ends of the range"
    );
    // `word` reads an absent extension as fine = 0. That is the inference half of
    // the module doc, and after this capture it is the *right* inference for RESO
    // rather than a convenient one.
    assert_eq!(reso.word(0), Some(0x0000));
    assert_eq!(reso.word(12), Some(0x7f00));
}

/// **An extension lane is indexed per step, measured rather than inferred.**
///
/// Every p-lock captured before this one sat on step 1, so that a 64-byte
/// extension carries a fine byte *per step* followed from the lane geometry and
/// from nothing else. The control half of the item-3 capture put FREQ on four
/// steps: the extension holds its fine byte at exactly steps 1, 5, 9 and 13 and
/// [`NO_VALUE`](digi_protocol::a4_plocks::NO_VALUE) at the other sixty, matching
/// its parent lane position for position.
///
/// **What this does not show** is a fine byte *differing* between steps of one
/// lane: all four read 23, which is one gesture applied to four held trigs rather
/// than four independent turns. So the indexing is measured and the per-step
/// *independence* is still inference — a narrower gap than before and not a
/// closed one, which is why this test asserts the fill pattern rather than a
/// spread of values.
#[test]
fn a_four_step_lane_carries_its_fine_bytes_at_the_same_four_steps() {
    let p = a4_pattern(RESO4).payload;
    let lanes = read_all_plocks(&p).unwrap();

    let freq = lanes.iter().find(|l| l.param_id == 0x22).expect("a FREQ lane");
    assert!(freq.ext_lane.is_some(), "FREQ allocates an extension, as always");

    let coarse: Vec<usize> = (0..64).filter(|&s| freq.values[s].is_some()).collect();
    let fine_lane = freq.fine.as_ref().unwrap();
    let fine: Vec<usize> = (0..64).filter(|&s| fine_lane[s].is_some()).collect();
    assert_eq!(coarse, vec![0, 4, 8, 12]);
    assert_eq!(fine, coarse, "the extension is filled at its parent's steps and nowhere else");

    for &s in &coarse {
        assert_eq!(freq.values[s], Some(64));
        assert_eq!(fine_lane[s], Some(23), "one gesture on four held trigs");
        assert_eq!(freq.word(s), Some(0x4017));
    }
}

/// **The observation above reads like a generalisation and rests on one lock.**
/// Recorded because it is the whole reason open item 3 is still open, and because
/// the test above loops over five captures in a way that looks like five
/// independent RESO samples.
///
/// It is not. Across every fixture there is exactly **one** distinct RESO lock —
/// SYN1, step 1, coarse 100 — captured four times because RESO was the *control*
/// in those diffs and was deliberately not touched. FREQ, meanwhile, has five
/// distinct fine bytes and **none of them is zero**.
///
/// So the two hypotheses are not equally supported by the same amount of
/// evidence: "the box omits an all-zero extension" requires that one RESO sample
/// to have landed on a fine byte of exactly zero, which is a 1-in-256 accident,
/// while "RESO is integer-valued" explains it outright. That asymmetry is worth
/// having written down, and it is also why **one** further RESO lock at a
/// different value settles it.
///
/// The other gap this pins: **every lock in these captures is on step 1.** No
/// multi-step lane had been captured, so that an extension carries a fine byte
/// *per step* was inference from the lane geometry rather than a measurement.
///
/// **Both gaps were closed the same day by one dump** — see
/// [`four_reso_values_on_one_lane_allocate_no_extension`] and
/// [`a_four_step_lane_carries_its_fine_bytes_at_the_same_four_steps`]. This test
/// is kept because it is the *argument* for that dump: it holds the sample counts
/// that made a 1-in-256 accident the alternative to a measurement, and a fixture
/// list that grows past them is how the next thin generalisation gets caught.
#[test]
fn the_reso_observation_rests_on_a_single_lock() {
    let mut reso = std::collections::BTreeSet::new();
    let mut freq_fine = std::collections::BTreeSet::new();
    let mut freq_lanes = 0;
    let mut reso_lanes = 0;

    for name in [FREQ64, FREQ64_RESO100, PLUS_SYN2, BASE, FREQ100] {
        for lane in read_all_plocks(&a4_pattern(name).payload).unwrap() {
            let locked: Vec<usize> =
                (0..64).filter(|&s| lane.values[s].is_some()).collect();
            assert_eq!(locked, vec![0], "{name}: every captured lock is on step 1 alone");

            match lane.param_id {
                0x22 => {
                    freq_lanes += 1;
                    freq_fine.insert(lane.fine.as_ref().unwrap()[0].unwrap());
                }
                0x23 => {
                    reso_lanes += 1;
                    reso.insert((lane.track, lane.values[0].unwrap()));
                }
                other => panic!("{name}: unexpected param id {other:#04x}"),
            }
        }
    }

    assert_eq!(reso_lanes, 4, "four RESO lane-instances");
    assert_eq!(
        reso.into_iter().collect::<Vec<_>>(),
        vec![(0, 100)],
        "...and all four are the SAME lock: SYN1, coarse 100"
    );

    assert_eq!(freq_lanes, 6, "six FREQ lane-instances");
    assert_eq!(
        freq_fine.iter().copied().collect::<Vec<_>>(),
        vec![23, 52, 96, 113, 116],
        "five distinct fine bytes"
    );
    assert!(
        !freq_fine.contains(&0),
        "and never zero — a fractional parameter looks like this, which is what \
         makes RESO's single absent extension a 1-in-256 accident under the \
         omit-when-zero hypothesis"
    );
}

/// Four takes of a displayed "64" produced fine bytes of 23, 52, 113 and 116: a
/// knob landing in different places inside one displayed integer, which is what
/// a fine byte must look like and what a marker or a count could not.
///
/// Three of those four are in this fixture set; the coarse byte is `0x40` in all
/// of them, and that stability beside a moving fine byte is the argument.
#[test]
fn one_displayed_value_carries_different_fine_bytes() {
    let mut fines = Vec::new();
    for name in [FREQ64, FREQ64_RESO100, PLUS_SYN2, BASE] {
        for lane in read_all_plocks(&a4_pattern(name).payload).unwrap() {
            if lane.param_id == 0x22 && lane.values[0] == Some(64) {
                fines.push(lane.fine.as_ref().unwrap()[0].unwrap());
            }
        }
    }
    fines.sort_unstable();
    fines.dedup();
    assert_eq!(fines, vec![23, 52, 113, 116], "one display value, four knob positions");
}
