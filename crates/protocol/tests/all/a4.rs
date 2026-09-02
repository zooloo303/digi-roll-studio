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


use crate::common::{a4_pattern, a4_working_pattern, fixture_bytes};

use digi_protocol::a4_pattern::{
    build_pattern, build_trig_probe, describe_offset, effective_length, effective_velocity, note_name, parse_pattern, read_track_states,
    read_track_trigs, set_note_trig, set_trig_condition, set_trig_length, set_trig_micro_timing,
    set_trig_velocity, track_default, track_default_note, LaneEvidence, TrigState, LANES, NO_NOTE,
    NUM_STEPS, NUM_TRACKS, PAYLOAD_LEN, PROBE_NOTE, TRACK_BASE, TRACK_DEFAULTS,
    TRACK_DEFAULTS_LEN, TRACK_STRIDE, TRIG_BYTE0_POSITIONAL, VELOCITY_LANE, VELOCITY_MAX,
    VELOCITY_MIN,
};
use digi_protocol::a4_plocks::{
    apply_track_plocks, free_lane_count, is_compacted, orphan_extension_count, read_all_plocks,
    read_track_plocks, A4LaneWrite, NUM_LANES,
};

const CLEAR: &str = "analogfour-A16-clear-2026-08-31.syx";
const TRIGLESS: &str = "analogfour-A16-trigless-trk1-step1-2026-08-31.syx";
const FREQ64: &str = "analogfour-A16-plock-fltfreq64-2026-08-31.syx";
const FREQ64_RESO100: &str = "analogfour-A16-plock-freq64-reso100-2026-08-31.syx";
const PLUS_SYN2: &str = "analogfour-A16-plock-plus-syn2-freq64-2026-08-31.syx";
const BASE: &str = "analogfour-A16-plock-freq64-reso100-base-2026-08-31.syx";
const FREQ100: &str = "analogfour-A16-plock-freq100-reso100-2026-08-31.syx";
const RESO4: &str = "analogfour-A16-plock-reso4-freq64-2026-08-31.syx";
/// Two tracks with locks — SYN1 `0x22` and `0x23`, SYN2 `0x24` — which is what
/// the containment test needs and no 2026-08-31 capture has.
const TWO_TRACK: &str = "analogfour-A16-plock-two-track-2026-09-01.syx";
/// What the box wrote back when it was handed an `80 80` extension detached
/// from the lane it extends. See [`the_box_binds_an_extension_to_the_lane_before_it`].
const DETACHED_EXT: &str = "analogfour-A16-plock-detached-ext-readback-2026-09-01.syx";
/// The canonical pool that detached extension was built from, and the backup it
/// was restored from.
const DETACHED_EXT_BASE: &str = "analogfour-A16-plock-detached-ext-sent-base-2026-09-01.syx";
/// **61 lanes on one trig** — the parameter-naming sweep of 2026-09-01, caught in
/// the working buffer. Twenty times richer than any pool captured before it, and
/// the only fixture here that exercises the reader and the writer at scale. A
/// `0x5a` working dump rather than a `0x54` stored one, because the sweep never
/// saved: 61 p-locks were authored, read and thrown away without a slot moving.
const SIXTY_ONE_LANES: &str = "analogfour-A16-plock-61-lanes-2026-09-01.syx";
const A01: &str = "analogfour-A01-2026-08-30.syx";
const A01_POSTRESET: &str = "analogfour-A01-postreset-2026-08-31.syx";

/// The 2026-09-01 knob session: one field per step on A16 SYN4, each value one
/// turn of one knob, read back off the *working* pattern so nothing was saved
/// over. This is the only fixture with a velocity, a length, a micro timing or
/// a condition somebody watched being made.
const LANES_FIXTURE: &str = "analogfour-A16-lanes-2026-09-01.syx";
/// The first frame of the condition walk — `FF -> 0x0b` on step 7 — kept
/// because it is the moment `+384` stopped being an unnamed lane.
const COND_FIRST: &str = "analogfour-A16-cond-first-2026-09-01.syx";
/// The arp session: NO2/NO3/NO4 turned on held trigs of SYN1, SYN3 and SYN4,
/// which is what named `+532`, `+596` and `+660` — and unnamed them as "chord
/// notes", since the A4 is monophonic per track.
const ARP_NOTES: &str = "analogfour-A16-arp-notes-2026-09-01.syx";

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


// --- The nine per-step lanes -------------------------------------------------

/// The decomposition is only worth anything if it *is* a decomposition: nine
/// 64-byte lanes, no two overlapping, all of them inside one track's 751 bytes
/// and none of them straddling the two per-track blocks. A lane offset typed one
/// digit wrong would still read plausible-looking bytes out of a neighbour, so
/// this is the assertion that a typo meets instead of the box.
#[test]
fn the_nine_lanes_tile_a_track_without_overlapping() {
    let mut claimed = [false; TRACK_STRIDE];
    for lane in &LANES {
        assert!(
            lane.offset + NUM_STEPS <= TRACK_STRIDE,
            "lane +{} runs past the {TRACK_STRIDE}-byte stride",
            lane.offset
        );
        for (byte, taken) in claimed.iter_mut().enumerate().skip(lane.offset).take(NUM_STEPS) {
            assert!(!*taken, "two lanes both claim track byte {byte}");
            *taken = true;
        }
    }
    // The trig lane and the two per-track blocks are the rest of the track, and
    // no lane may sit inside any of them.
    for (byte, taken) in claimed.iter().enumerate().take(2 * NUM_STEPS) {
        assert!(!taken, "a lane overlaps the trig lane at {byte}");
    }
    for (byte, taken) in
        claimed.iter().enumerate().skip(TRACK_DEFAULTS).take(TRACK_DEFAULTS_LEN)
    {
        assert!(!taken, "a lane overlaps the track defaults at {byte}");
    }
}

/// Default 1 of the per-track block is `0x64` — 100, the velocity Elektron
/// ships — in every track of every fixture here.
///
/// **It is not `0x64` on every track in the world**, and saying so was a
/// mistake this file briefly encoded: across the project stream it is `0x64`
/// 784 times out of 792, and the other eight are a default somebody turned. So
/// this asserts the fixtures, which is what it can, and [`resolve`] handles the
/// tail — including the one track whose default is itself `FF`.
#[test]
fn the_default_velocity_in_every_fixture_here_is_one_hundred() {
    for name in ALL {
        let pattern = a4_pattern(name);
        for track in 0..NUM_TRACKS {
            let base = TRACK_BASE + track * TRACK_STRIDE;
            assert_eq!(
                pattern.payload[base + TRACK_DEFAULTS + 1], 0x64,
                "{name} {}: default velocity",
                digi_protocol::a4_pattern::TRACK_NAMES[track]
            );
        }
    }
}

/// A01 SYN1 was played in rather than stepped in, and that is what makes it the
/// only fixture with anything in the velocity and length lanes: 32 trigs at
/// `0x7f` velocity, and a length that *varies between adjacent notes* — 0x1b on
/// most, 0x1a on two. A lane that were a per-track setting could not do that,
/// and a lane that were an index into something would not land on two adjacent
/// values.
#[test]
fn a01_syn1_carries_a_recorded_velocity_and_a_length_that_varies() {
    let pattern = a4_pattern(A01);
    let base = TRACK_BASE;
    let velocity = LANES.iter().find(|l| l.name == "velocity").expect("a velocity lane");
    let length = LANES.iter().find(|l| l.name == "length").expect("a length lane");

    let trigs = read_track_trigs(&pattern.payload, 0).expect("SYN1");
    assert_eq!(trigs.len(), 32, "SYN1");

    let velocities: Vec<u8> =
        trigs.iter().map(|t| pattern.payload[base + velocity.offset + t.step - 1]).collect();
    assert!(
        velocities.iter().all(|&v| v == 0x7f),
        "every recorded trig at full velocity, got {velocities:?}"
    );

    let lengths: Vec<u8> =
        trigs.iter().map(|t| pattern.payload[base + length.offset + t.step - 1]).collect();
    assert_eq!(lengths.iter().filter(|&&l| l == 0x1a).count(), 7, "seven shorter notes");
    assert_eq!(lengths.iter().filter(|&&l| l == 0x1b).count(), 25, "and twenty-five at 0x1b");
    assert!(lengths.iter().all(|&l| l <= 0x7f), "no length above 0x7f in this capture");
}

/// SYN4 is the other half of the same argument, from the default side: its
/// per-track default length is `0x3e`, and every one of its four trigs carries
/// `0x3e` in the length lane. The pairing between lane order and default order
/// is what named these two lanes, and this is it in one fixture.
#[test]
fn a01_syn4s_trigs_carry_its_own_default_length() {
    let pattern = a4_pattern(A01);
    let base = TRACK_BASE + 3 * TRACK_STRIDE;
    let length = LANES.iter().find(|l| l.name == "length").expect("a length lane");
    let default = pattern.payload[base + TRACK_DEFAULTS + 2];
    assert_eq!(default, 0x3e, "SYN4's default length");
    for trig in read_track_trigs(&pattern.payload, 3).expect("SYN4") {
        assert_eq!(
            pattern.payload[base + length.offset + trig.step - 1], default,
            "SYN4 step {} length",
            trig.step
        );
    }
}

/// The two lanes with no name must stay nameless in what the probe prints. A
/// diff that labelled `+459` "condition" would be the model deciding the
/// experiment's outcome before the box was asked — which is exactly the shape of
/// the three refuted trig models.
#[test]
fn describe_offset_refuses_to_name_the_unnamed_lanes() {
    for lane in LANES.iter().filter(|l| l.evidence == LaneEvidence::Shape) {
        let described = describe_offset(TRACK_BASE + lane.offset);
        assert!(
            described.contains("UNNAMED") && described.contains(&format!("+{}", lane.offset)),
            "lane +{} described as {described:?}",
            lane.offset
        );
    }
    assert_eq!(describe_offset(TRACK_BASE + 192), "SYN1 step 1 velocity");
    assert_eq!(describe_offset(TRACK_BASE + 256 + 63), "SYN1 step 64 length");
    assert_eq!(describe_offset(TRACK_BASE + TRACK_STRIDE + 128), "SYN2 step 1 note");
    assert_eq!(describe_offset(TRACK_BASE + 5), "SYN1 trig step 3 byte 1");
    assert_eq!(
        describe_offset(TRACK_BASE + TRACK_DEFAULTS + 1),
        "SYN1 track defaults +1 (default velocity)"
    );
}


// --- The knob session, 2026-09-01 --------------------------------------------

/// **The fixture is the experiment.** Eight steps of A16 SYN4, one field turned
/// on each, and the assertions below are the knob positions: velocity at both
/// ends, length at both ends, micro timing at both ends, a condition. If any of
/// these ever fails, a lane moved — because these bytes did not come from a
/// model, they came from a hand on a knob and a diff naming the byte it moved.
#[test]
fn the_knob_session_put_one_field_on_each_step() {
    let pattern = a4_working_pattern(LANES_FIXTURE);
    let trigs = read_track_trigs(&pattern.payload, 3).expect("SYN4");
    let at = |step: usize| trigs.iter().find(|t| t.step == step).expect("a trig on this step");

    assert_eq!(at(1).velocity, Some(VELOCITY_MIN), "step 1: VEL turned to minimum");
    assert_eq!(at(2).velocity, Some(VELOCITY_MAX), "step 2: VEL turned to maximum");
    assert_eq!(at(3).length, Some(0x00), "step 3: LEN at its shortest");
    assert_eq!(at(4).length, Some(0x7f), "step 4: LEN at the top of the menu");
    assert_eq!(at(5).micro_timing, -23, "step 5: micro timing hard left");
    assert_eq!(at(6).micro_timing, 23, "step 6: micro timing hard right");
    assert_eq!(at(7).condition, Some(0x1f), "step 7: a trig condition");

    // And the fields do not bleed: every step carries exactly the one that was
    // turned on it. A lane read one step wide of itself would still produce
    // plausible values, and this is what catches that.
    assert_eq!(at(1).length, None, "step 1 got no length");
    assert_eq!(at(3).velocity, None, "step 3 got no velocity");
    assert_eq!(at(5).condition, None, "step 5 got no condition");
    assert_eq!(at(7).micro_timing, 0, "step 7 was never nudged");
}

/// The A4's velocity floor is 1, not 0 — the knob stops there. Anything
/// mapping a MIDI 0-127 velocity onto this box has to know that, so the setter
/// clamps rather than refusing, and a 0 lands on the floor the box has.
#[test]
fn velocity_clamps_to_the_range_the_knob_stops_at() {
    let mut payload = a4_pattern(CLEAR).payload;
    set_note_trig(&mut payload, 0, 0, Some(PROBE_NOTE)).expect("a trig to carry it");

    set_trig_velocity(&mut payload, 0, 0, Some(0)).expect("zero is clamped, not refused");
    assert_eq!(read_track_trigs(&payload, 0).unwrap()[0].velocity, Some(VELOCITY_MIN));

    set_trig_velocity(&mut payload, 0, 0, Some(200)).expect("and so is 200");
    assert_eq!(read_track_trigs(&payload, 0).unwrap()[0].velocity, Some(VELOCITY_MAX));

    // None is not silence: it is "take the track's", which is 100.
    set_trig_velocity(&mut payload, 0, 0, None).expect("clearing");
    let trig = read_track_trigs(&payload, 0).unwrap()[0].clone();
    assert_eq!(trig.velocity, None, "the lane reads unset");
    assert_eq!(effective_velocity(&payload, 0, &trig).unwrap(), 0x64, "and it sounds at 100");
}

/// Micro timing is the one lane with no "unset": it clears to zero, so a trig
/// nobody nudged and a trig nudged back to centre are the same byte. Both ends
/// clamp to the range the knob stops at.
#[test]
fn micro_timing_is_signed_and_has_no_unset() {
    let mut payload = a4_pattern(CLEAR).payload;
    set_note_trig(&mut payload, 0, 0, Some(PROBE_NOTE)).expect("a trig");
    assert_eq!(read_track_trigs(&payload, 0).unwrap()[0].micro_timing, 0, "cleared is centred");

    set_trig_micro_timing(&mut payload, 0, 0, -100).expect("clamped");
    assert_eq!(read_track_trigs(&payload, 0).unwrap()[0].micro_timing, -23);
    set_trig_micro_timing(&mut payload, 0, 0, 100).expect("clamped");
    assert_eq!(read_track_trigs(&payload, 0).unwrap()[0].micro_timing, 23);
}

/// `FF` is how the format says "no condition", so it cannot also be a condition
/// somebody selects. The setter says so rather than writing a byte that would
/// read back as nothing.
#[test]
fn a_condition_of_ff_is_refused_because_that_is_the_encoding_for_none() {
    let mut payload = a4_pattern(CLEAR).payload;
    let refused = set_trig_condition(&mut payload, 0, 0, Some(NO_NOTE));
    assert!(refused.is_err(), "FF as a condition value");
    set_trig_condition(&mut payload, 0, 0, None).expect("None is how you clear it");
}

/// Byte 4 of the default block moved during the condition walk with no trig
/// held, which looks like a track-level default — and it is `0x00` on a cleared
/// track, which a *default condition* would not be. So the reading is recorded
/// and not acted on: an unset condition is no condition, and this test pins
/// both halves of why.
#[test]
fn an_unset_condition_is_no_condition_and_the_default_block_is_not_consulted() {
    let pattern = a4_working_pattern(LANES_FIXTURE);
    let trigs = read_track_trigs(&pattern.payload, 3).expect("SYN4");
    let plain = trigs.iter().find(|t| t.step == 8).expect("step 8");
    assert_eq!(plain.condition, None, "no condition of its own, and none at all");
    // Length does have a measured pairing, so the same untouched step resolves
    // to the track's 0x0e — the contrast is the point.
    assert_eq!(plain.length, None, "no length of its own");
    assert_eq!(effective_length(&pattern.payload, 3, plain).unwrap(), 0x0e, "so it takes SYN4's");

    // The byte the session moved, on a track nobody has touched: 0x00, not FF.
    // A default condition would not sit on menu entry zero for every track.
    assert_eq!(track_default(&a4_pattern(CLEAR).payload, 0, 4).unwrap(), 0x00);
    assert_eq!(track_default(&pattern.payload, 3, 4).unwrap(), 0x00);
}

/// The moment `+384` stopped being nameless: one byte, `FF -> 0x0b`, on the
/// step the TRC knob was turned on. Kept as its own fixture because a table
/// entry that says "hardware" should be able to point at the frame.
#[test]
fn the_condition_lane_was_named_by_one_byte_moving() {
    let before = a4_working_pattern(COND_FIRST);
    let after = a4_working_pattern(LANES_FIXTURE);
    let step_seven = |p: &digi_protocol::a4_pattern::A4Pattern| {
        read_track_trigs(&p.payload, 3)
            .expect("SYN4")
            .into_iter()
            .find(|t| t.step == 7)
            .expect("step 7")
    };
    assert_eq!(step_seven(&before).condition, Some(0x0b), "where the walk started");
    assert_eq!(step_seven(&after).condition, Some(0x1f), "and where it stopped");
}

/// The working pattern's reply index is the loaded slot, not a constant zero —
/// which is what a day of captures taken with A01 loaded made it look like.
/// A16 was loaded for this one.
#[test]
fn a_working_pattern_reports_which_slot_the_box_is_sitting_on() {
    let pattern = a4_working_pattern(LANES_FIXTURE);
    assert_eq!(pattern.slot, 15, "A16");
    assert_eq!(pattern.slot_name(), "A16");
    // And it is not a stored dump: the type byte differs, so the stored-slot
    // parser must refuse it rather than quietly accept a live buffer.
    assert!(parse_pattern(&fixture_bytes(LANES_FIXTURE)).is_err());
}

/// Every setter writes exactly one byte, and writing twice changes nothing —
/// the property `safe_write` leans on when it composes an edit onto a freshly
/// re-fetched destination.
#[test]
fn each_setter_moves_one_byte_and_is_idempotent() {
    let original = a4_pattern(CLEAR).payload;
    for (name, apply) in [
        ("velocity", &(|p: &mut Vec<u8>| set_trig_velocity(p, 2, 9, Some(64))) as &dyn Fn(&mut Vec<u8>) -> Result<(), String>),
        ("length", &|p: &mut Vec<u8>| set_trig_length(p, 2, 9, Some(30))),
        ("micro timing", &|p: &mut Vec<u8>| set_trig_micro_timing(p, 2, 9, -4)),
        ("condition", &|p: &mut Vec<u8>| set_trig_condition(p, 2, 9, Some(3))),
    ] {
        let mut payload = original.clone();
        apply(&mut payload).expect("the write");
        let moved: Vec<usize> =
            (0..payload.len()).filter(|&i| payload[i] != original[i]).collect();
        assert_eq!(moved.len(), 1, "{name} moved {} bytes", moved.len());

        let once = payload.clone();
        apply(&mut payload).expect("again");
        assert_eq!(payload, once, "{name} is not idempotent");
    }
}


/// **The A4 shares the digis' length scale**, which is the finding that saved
/// this format a table of its own. Three anchors, two of them read off the A4's
/// screen on 2026-09-01 and one of them this format's own default:
///
/// `0x00` shows `.125`, `0x7e` shows `128`, `0x7f` shows `INF` — and `0x0e`,
/// the per-track default length in every A4 capture, is exactly one step.
#[test]
fn an_a4_length_byte_means_what_a_digi_length_byte_means() {
    use digi_protocol::pattern::length_byte_to_steps;

    assert_eq!(length_byte_to_steps(0x00), 0.125, "the box showed .125");
    assert_eq!(length_byte_to_steps(0x7e), 128.0, "the box showed 128");
    assert!(length_byte_to_steps(0x7f).is_infinite(), "the box showed INF");
    assert_eq!(length_byte_to_steps(0x0e), 1.0, "and the A4's own default is one step");

    // The default really is 0x0e, on every track of every capture but the one
    // SYN4 whose own default was raised.
    let cleared = a4_pattern(CLEAR);
    for track in 0..NUM_TRACKS {
        assert_eq!(track_default(&cleared.payload, track, 2).unwrap(), 0x0e);
    }
}

/// `+532`/`+596`/`+660` are the ARP menu's NO2/NO3/NO4, turned on the box.
/// They were "chord notes 2-4" until then — a name derived from a nesting
/// correlation, which was right about the shape and wrong about the field.
#[test]
fn the_arp_note_lanes_were_named_by_the_arp_menu() {
    let pattern = a4_working_pattern(ARP_NOTES);
    let named: Vec<&str> = LANES
        .iter()
        .filter(|l| l.offset >= 532)
        .map(|l| l.name)
        .collect();
    assert_eq!(named, ["arp note 2", "arp note 3", "arp note 4"]);
    for lane in LANES.iter().filter(|l| l.offset >= 532) {
        assert_eq!(lane.evidence, LaneEvidence::Hardware, "+{}", lane.offset);
    }

    // SYN4 step 1 carries the two that were turned on it, and not the third.
    let base = TRACK_BASE + 3 * TRACK_STRIDE;
    assert_eq!(pattern.payload[base + 532], 0x3d, "NO2");
    assert_eq!(pattern.payload[base + 596], 0x44, "NO3");
    assert_eq!(pattern.payload[base + 660], NO_NOTE, "NO4 was never turned here");
}

/// The one lane still without a name, asserted as *not named* — so that giving
/// it one is a deliberate edit to this test rather than a drive-by rename.
#[test]
fn exactly_one_lane_is_still_unnamed() {
    let unnamed: Vec<usize> = LANES
        .iter()
        .filter(|l| l.evidence == LaneEvidence::Shape)
        .map(|l| l.offset)
        .collect();
    assert_eq!(unnamed, [459], "the lane no knob has been found for");
}


/// A track whose *own* default velocity is `FF` — "unset", one of which exists
/// in the 792 track-instances measured. Resolving it naively hands back 255,
/// which is not a velocity; it falls back to the value a cleared pattern
/// carries instead.
#[test]
fn a_track_default_that_is_itself_unset_does_not_resolve_to_two_hundred_and_fifty_five() {
    let mut payload = a4_pattern(A01).payload;
    let base = TRACK_BASE;
    payload[base + VELOCITY_LANE] = NO_NOTE;
    payload[base + TRACK_DEFAULTS + 1] = NO_NOTE;

    let trig = read_track_trigs(&payload, 0).expect("SYN1").remove(0);
    assert_eq!(trig.velocity, None, "the trig's own lane is unset");
    assert_eq!(effective_velocity(&payload, 0, &trig).unwrap(), 0x64, "and so is the track's");
}


// --- the pool writer, against the box's own bytes -----------------------------
//
// The gen-1 pool writer landed 2026-09-01, once three writes to A16 answered
// whether the box requires the compacted `(param_id, track)` order it produces.
// **It does not** — handed a pool with its keys swapped, or with a hole, or with
// an extension detached from its parent, the box parsed every lane, lost no
// lock, and wrote back its own canonical form.
//
// The encoder emits that form anyway, because the *verify* needs it: a write is
// checked by reading the slot back and comparing byte for byte, and a box that
// normalises turns a correct write into 10 or 132 spurious diffs. See
// `a4_plocks::apply_track_plocks`.

/// Every pool the box authored on its own: read its lanes, ask for them back,
/// and the payload must not move.
///
/// **The box's own bytes are the oracle here**, which is what makes this the
/// strongest single test of the encoder. Every rule it follows — the sort key,
/// packing from lane zero, `FF FF` over 64 zeros for a free lane, an extension
/// iff some fine byte is non-zero and holding fine bytes at exactly its parent's
/// locked steps — is a rule about bytes the A4 wrote. Getting any one of them
/// wrong moves a byte.
///
/// `TWO_TRACK` is deliberately absent, and
/// [`a_zero_extension_is_normalised_away_and_that_is_the_one_exception`] is why:
/// it is the one pool here the box did not compose by itself.
#[test]
fn a_pool_read_and_written_back_unchanged_moves_no_bytes() {
    for name in [FREQ64, FREQ64_RESO100, PLUS_SYN2, BASE, FREQ100, RESO4] {
        let pattern = a4_pattern(name);
        let mut payload = pattern.payload.clone();
        for track in 0..NUM_TRACKS {
            let lanes: Vec<A4LaneWrite> =
                read_track_plocks(&payload, track).unwrap().iter().map(A4LaneWrite::from).collect();
            apply_track_plocks(&mut payload, track, &lanes).unwrap();
        }
        assert_eq!(payload, pattern.payload, "{name}: a write-back of what was read moved bytes");
    }
}

/// The same round trip at twenty times the size, against the 61-lane sweep.
///
/// **Scale is the point.** Every other pool fixture holds one to three lanes, so
/// the sort, the packing and the extension placement are all exercised over a
/// handful of entries where an off-by-one has nowhere to show. This one has 61
/// lanes across two tracks with ten of them carrying extensions, in the box's own
/// order — so the encoder has to reproduce 4 KB of pool exactly, and any rule it
/// gets subtly wrong moves a byte.
#[test]
fn the_sixty_one_lane_pool_survives_a_round_trip_byte_for_byte() {
    let pattern = a4_working_pattern(SIXTY_ONE_LANES);
    assert_eq!(read_all_plocks(&pattern.payload).unwrap().len(), 61);
    assert!(is_compacted(&pattern.payload).unwrap());

    let mut payload = pattern.payload.clone();
    for track in 0..NUM_TRACKS {
        let lanes: Vec<A4LaneWrite> =
            read_track_plocks(&payload, track).unwrap().iter().map(A4LaneWrite::from).collect();
        apply_track_plocks(&mut payload, track, &lanes).unwrap();
    }
    assert_eq!(payload, pattern.payload, "61 lanes did not survive a write-back");
}

/// **The containment property, against a two-track pool off the box.**
///
/// A gen-1 write rebuilds the pool, so SYN2's lane changes index — the one thing
/// `plocks::apply_track_plocks` exists to avoid on the digis, and impossible to
/// avoid on a box that sorts. What must hold instead is that its *contents*
/// survive: same parameter, same values, same fine bytes.
///
/// This is the property the whole read-modify-write bargain rests on. A07 going
/// back to the box on 2026-09-01 moved 56 of 12,974 bytes with the pool
/// untouched, and the pool writer is the first thing that changes that.
#[test]
fn writing_one_track_s_lanes_leaves_another_track_s_contents_alone() {
    let pattern = a4_pattern(TWO_TRACK);
    let mut payload = pattern.payload.clone();

    let syn2_before = read_track_plocks(&payload, 1).unwrap();
    assert_eq!(syn2_before.len(), 1, "the fixture's SYN2 lane");
    assert_eq!(syn2_before[0].param_id, 0x24, "OVERDRIVE, named on the box 2026-09-01");

    // SYN1 gives up its 0x23 lane and gains a 0x10 — one below SYN2's 0x24 and
    // one above nothing, so the layout genuinely shifts underneath it.
    let lanes = [
        A4LaneWrite::new(0x10, {
            let mut v = vec![None; NUM_STEPS];
            v[2] = Some(0x4000);
            v
        }),
        A4LaneWrite::new(0x22, read_track_plocks(&payload, 0).unwrap()[0].values.iter()
            .enumerate()
            .map(|(step, _)| read_track_plocks(&payload, 0).unwrap()[0].word(step))
            .collect()),
    ];
    apply_track_plocks(&mut payload, 0, &lanes).unwrap();

    let syn2_after = read_track_plocks(&payload, 1).unwrap();
    assert_eq!(syn2_after.len(), 1, "SYN2 still has its lane");
    assert_ne!(syn2_after[0].lane, syn2_before[0].lane, "and it moved, which is forced");
    assert_eq!(syn2_after[0].param_id, syn2_before[0].param_id);
    assert_eq!(syn2_after[0].track, syn2_before[0].track);
    assert_eq!(syn2_after[0].values, syn2_before[0].values, "SYN2's values are untouched");
    assert_eq!(syn2_after[0].fine, syn2_before[0].fine);
    assert!(is_compacted(&payload).unwrap(), "and the result is the box's own form");
}

/// **The box binds an `80 80` to the lane physically before it — measured from
/// the write side, 2026-09-01.**
///
/// `read_all_plocks` has always read an extension that way, and until this
/// capture it was inference: the box had never produced a pool where a lane and
/// its extension were apart, so nothing tested what "before it" meant.
///
/// A16 was sent a pool holding FREQ with no extension, RESO, and FREQ's orphaned
/// `80 80` immediately after RESO — genuinely ambiguous whose it is. The box kept
/// the layout and rewrote exactly two bytes of the extension: it **adopted the
/// lane as RESO's** and re-aligned its fine bytes to RESO's own locked steps,
/// a fine byte where the parent locks and `NO_VALUE` where it does not.
///
/// Two things follow. The reader's adjacency rule is right. And "an extension is
/// indexed per step" — previously measured only on the four-step FREQ lane of
/// `RESO4` — is confirmed by the box doing that alignment itself.
#[test]
fn the_box_binds_an_extension_to_the_lane_before_it() {
    let sent_base = a4_pattern(DETACHED_EXT_BASE);
    let got = a4_pattern(DETACHED_EXT);

    // The pool it was built from: FREQ with a fine byte, RESO without.
    let before = read_all_plocks(&sent_base.payload).unwrap();
    assert_eq!(before.len(), 2);
    assert_eq!((before[0].param_id, before[1].param_id), (0x22, 0x23));
    assert_eq!(before[0].values[0], Some(50), "FREQ 50 on step 1, named on the box");
    assert_eq!(before[0].fine.as_ref().unwrap()[0], Some(0x3b), "and its fine byte");
    assert!(before[1].fine.is_none(), "RESO is integer-valued and had no extension");

    // What came back: the extension is RESO's, and holds a fine byte at RESO's
    // step and nowhere else.
    let after = read_all_plocks(&got.payload).unwrap();
    assert_eq!(after.len(), 2);
    assert_eq!((after[0].param_id, after[1].param_id), (0x22, 0x23));
    assert!(after[0].fine.is_none(), "FREQ lost the extension we detached from it");
    assert_eq!(after[0].values[0], Some(50), "and kept its coarse value");

    let reso_fine = after[1].fine.as_ref().expect("the box adopted the 80 80 as RESO's");
    assert_eq!(after[1].values[4], Some(100), "RESO 100 on step 5");
    assert_eq!(reso_fine[4], Some(0), "a fine byte at its parent's locked step");
    assert_eq!(reso_fine[0], None, "and NO_VALUE where the parent does not lock");
    assert_eq!(reso_fine.iter().filter(|f| f.is_some()).count(), 1);

    assert!(is_compacted(&got.payload).unwrap(), "the box's answer is always its own form");
}

/// The box will *store* an all-zero extension when handed one, and never
/// allocates one itself.
///
/// Both halves matter to the encoder rule. The second is why "emit an extension
/// iff some fine byte is non-zero" does not waste a lane; the first is why
/// emitting one would not be *rejected* either — so the rule is a choice the
/// box tolerates rather than one it enforces, and it is worth knowing which.
#[test]
fn an_all_zero_extension_is_stored_but_never_allocated() {
    // Stored: the box kept the zero extension it was handed, through a save.
    let got = a4_pattern(DETACHED_EXT);
    let reso = &read_all_plocks(&got.payload).unwrap()[1];
    assert_eq!(reso.fine.as_ref().unwrap()[4], Some(0));

    // Never allocated: RESO across every capture the box composed by itself.
    // `TWO_TRACK` is downstream of the detached-extension write, so its RESO
    // extension is one the box was *handed* — including it here would be reading
    // our own experiment back as evidence about the box.
    for name in [FREQ64_RESO100, BASE, FREQ100, RESO4] {
        for lane in read_all_plocks(&a4_pattern(name).payload).unwrap() {
            if lane.param_id == 0x23 {
                assert!(lane.fine.is_none(), "{name}: the box gave RESO an extension");
            }
        }
    }
}

/// Three A4 parameter ids are measured, all three named by a hand on the box.
///
/// `0x24` OVERDRIVE joined `0x22` FLTR1 FREQ and `0x23` RESO on 2026-09-01 —
/// SYN2 step 9, set to max, and it stored as 127 with no extension. The
/// adjacent-ids-for-adjacent-knobs pattern holds for a third.
///
/// **This is the count, not a table.** None of the three is in `params::A4_PARAMS`
/// with a p-lock slot, which is why `core::a4_transfer` carries a lane by its id
/// rather than by a name — see `a4_lanes_for_write`.
#[test]
fn the_two_track_fixture_names_a_third_parameter_id() {
    let p = a4_pattern(TWO_TRACK);
    let all = read_all_plocks(&p.payload).unwrap();
    assert_eq!(
        all.iter().map(|l| (l.param_id, l.track)).collect::<Vec<_>>(),
        [(0x22, 0), (0x23, 0), (0x24, 1)],
        "sorted by (param_id, track), param primary"
    );
    let overdrive = &all[2];
    assert_eq!(overdrive.values[8], Some(127), "max OVERDRIVE on step 9");
    assert!(overdrive.fine.is_none(), "and integer-valued, like RESO");
}

/// **The one place a read-modify-write is not byte-exact, and the argument for
/// letting it not be.**
///
/// The encoder emits an extension iff some fine byte is non-zero. A pool holding
/// an extension whose fine bytes are *all* zero therefore comes back one lane
/// shorter — 66 bytes that moved without being asked to, which is normally
/// exactly what this app refuses to do.
///
/// It is allowed here because the lane carries nothing. [`A4Lane::word`] reads a
/// zero fine byte and an absent extension identically, so the two forms are the
/// same value written two ways, and dropping one hands a lane back to a pool of
/// 128. The box never produces this shape — every RESO lane it composed itself
/// has no extension — so the only way to meet one is to have sent it, which is
/// how `TWO_TRACK` came to have one.
///
/// The property that has to hold instead is that **no lock changes**, and that
/// is what is asserted.
#[test]
fn a_zero_extension_is_normalised_away_and_that_is_the_one_exception() {
    let pattern = a4_pattern(TWO_TRACK);
    let mut payload = pattern.payload.clone();

    let before = read_all_plocks(&payload).unwrap();
    let reso = before.iter().find(|l| l.param_id == 0x23).unwrap();
    assert!(reso.fine.is_some(), "the fixture's RESO carries an all-zero extension");
    assert!(reso.fine.as_ref().unwrap().iter().flatten().all(|&f| f == 0));
    let words_before: Vec<Vec<Option<u16>>> =
        before.iter().map(|l| (0..NUM_STEPS).map(|s| l.word(s)).collect()).collect();

    for track in 0..NUM_TRACKS {
        let lanes: Vec<A4LaneWrite> =
            read_track_plocks(&payload, track).unwrap().iter().map(A4LaneWrite::from).collect();
        apply_track_plocks(&mut payload, track, &lanes).unwrap();
    }

    let after = read_all_plocks(&payload).unwrap();
    assert!(
        after.iter().find(|l| l.param_id == 0x23).unwrap().fine.is_none(),
        "the empty extension was dropped"
    );
    assert_eq!(free_lane_count(&payload).unwrap(), free_lane_count(&pattern.payload).unwrap() + 1);

    // And nothing a person could hear or see has changed.
    assert_eq!(
        after.iter().map(|l| (l.param_id, l.track)).collect::<Vec<_>>(),
        before.iter().map(|l| (l.param_id, l.track)).collect::<Vec<_>>(),
    );
    let words_after: Vec<Vec<Option<u16>>> =
        after.iter().map(|l| (0..NUM_STEPS).map(|s| l.word(s)).collect()).collect();
    assert_eq!(words_after, words_before, "every lock is the same value it was");
    assert!(is_compacted(&payload).unwrap());
}

/// **No fine byte in any capture sets its top bit**, which is what says the fine
/// half of a gen-1 value is 128ths of a display unit rather than 256ths.
///
/// Measured directly on OSC TUNE (2026-09-01): the box carries from `fine 127`
/// to `coarse + 1, fine 0`, and FIN's on-screen −64…+63 maps onto bytes 64…127
/// and 0…63. TUNE is the only parameter whose fine byte the box displays a
/// number for, so it is the only one that could have been read this way.
///
/// This is the corroboration across everything else. Under the old 256ths
/// reading — inference imported from gen-2, and what `a4_plocks` documented
/// until that day — roughly half of these should exceed 127. None does, across
/// parameters as unrelated as filter cutoff, envelope depths and LFO depths.
#[test]
fn no_captured_fine_byte_uses_the_top_bit() {
    let mut examined = 0usize;
    let mut highest = 0u8;
    for name in [
        FREQ64, FREQ64_RESO100, PLUS_SYN2, BASE, FREQ100, RESO4, TWO_TRACK, DETACHED_EXT,
        DETACHED_EXT_BASE,
    ] {
        check_fine_bytes(&a4_pattern(name).payload, name, &mut examined, &mut highest);
    }
    check_fine_bytes(
        &a4_working_pattern(SIXTY_ONE_LANES).payload,
        SIXTY_ONE_LANES,
        &mut examined,
        &mut highest,
    );

    // A guard on the guard: an assertion that examines nothing passes for the
    // wrong reason, and this one reads a *subset* of lanes (only those with an
    // extension) so an encoding change could silently empty it.
    assert!(examined >= 20, "only {examined} fine bytes examined — too few to mean anything");
    assert!(highest <= 127, "highest fine byte was {highest}");
}

fn check_fine_bytes(payload: &[u8], name: &str, examined: &mut usize, highest: &mut u8) {
    for lane in read_all_plocks(payload).unwrap() {
        let Some(fine) = &lane.fine else { continue };
        for (step, f) in fine.iter().enumerate() {
            let Some(f) = f else { continue };
            *examined += 1;
            *highest = (*highest).max(*f);
            assert!(
                *f <= 127,
                "{name}: param {:#04x} step {} has fine byte {f}, which sets the top bit — \
                 the 128ths reading is wrong",
                lane.param_id,
                step + 1,
            );
        }
    }
}
