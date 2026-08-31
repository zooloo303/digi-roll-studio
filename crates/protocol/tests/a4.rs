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
    build_pattern, note_name, read_track_states, read_track_trigs, set_note_trig, track_default_note,
    TrigState, NUM_TRACKS, PAYLOAD_LEN, TRACK_BASE, TRACK_STRIDE,
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
const A01: &str = "analogfour-A01-2026-08-30.syx";
const A01_POSTRESET: &str = "analogfour-A01-postreset-2026-08-31.syx";

const ALL: [&str; 9] =
    [CLEAR, TRIGLESS, FREQ64, FREQ64_RESO100, PLUS_SYN2, BASE, FREQ100, A01, A01_POSTRESET];

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
/// has.** Either RESO is integer-valued or the box omits an extension whose fine
/// bytes are all zero, and no capture can say which — so this test pins the
/// observation rather than a rule, and PLAN.md §10 open item 3 is why there is
/// no gen-1 pool writer.
#[test]
fn freq_always_has_an_extension_and_reso_never_does() {
    for name in [FREQ64, FREQ64_RESO100, PLUS_SYN2, BASE, FREQ100] {
        for lane in read_all_plocks(&a4_pattern(name).payload).unwrap() {
            match lane.param_id {
                0x22 => assert!(lane.ext_lane.is_some(), "{name}: FREQ lane {}", lane.lane),
                0x23 => assert!(lane.ext_lane.is_none(), "{name}: RESO lane {}", lane.lane),
                other => panic!("{name}: unexpected param id {other:#04x}"),
            }
        }
    }
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
