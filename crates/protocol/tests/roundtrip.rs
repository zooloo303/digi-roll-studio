//! The round trip: decode a track off a box, hand it back to the encoder, and
//! check what lands. Port of `test/roundtrip.test.js`.
//!
//! These are the Phase 1 exit criteria. Nothing above this layer is trustworthy
//! until a real capture decodes, re-encodes and comes home byte-identical, and
//! until the minimal-diff contract — the thing that makes a read-modify-write to
//! hardware safe — is evidence rather than an intention.

mod common;

use common::*;
use digi_protocol::pattern::*;

struct Fixture {
    name: &'static str,
    spec: Spec,
    payload: Vec<u8>,
    /// The track carrying notes in this capture.
    track: usize,
    /// Track stride, written out longhand: if the spec drifts, these notice.
    track_size: usize,
    pool: (usize, usize),
    /// Bytes that move on the first write-back. Nonzero because the box stores
    /// "use the track default" as 0xFF and the encoder always writes explicit
    /// values; all of it is confined to this track's regions.
    first_pass_diffs: usize,
    /// Bytes that move when one note slides one step later.
    move_one_step_diffs: usize,
}

fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            name: "DT2",
            spec: dt2_spec(),
            payload: payload("digitakt2-A01-conditions-2026-08-02.syx"),
            track: 0,
            track_size: 1184,
            pool: (18948, 68100),
            first_pass_diffs: 147,
            move_one_step_diffs: 6,
        },
        Fixture {
            name: "DN2",
            spec: dn2_spec(),
            payload: payload("digitone2-A01-conditions-2026-08-02.syx"),
            track: 0,
            track_size: 1187,
            pool: (18996, 68148),
            first_pass_diffs: 16,
            move_one_step_diffs: 4,
        },
        Fixture {
            name: "DN2 chords",
            spec: dn2_spec(),
            payload: payload("digitone2-pernote-chords-2026-08-04.syx"),
            track: 0,
            track_size: 1187,
            pool: (18996, 68148),
            first_pass_diffs: 4,
            move_one_step_diffs: 6,
        },
    ]
}

impl Fixture {
    fn notes(&self, payload: &[u8]) -> Vec<Note> {
        let p = decode_pattern_kit(&self.spec, payload).expect("decode");
        track_notes(&p, self.track)
    }

    /// One trip through the studio and back: decode → notes → encode. This is
    /// precisely what "import, edit nothing, write back" does.
    fn round_trip(&self, payload: &[u8], edit: impl Fn(Vec<Note>) -> Vec<Note>) -> Vec<u8> {
        let notes = edit(self.notes(payload));
        encode_track_notes(&self.spec, payload, self.track, &notes)
            .expect("encode")
            .0
    }

    /// The write path's contract: nothing outside the track's step words and the
    /// shared trig-record pool may move.
    fn assert_only_track_bytes_changed(&self, before: &[u8], after: &[u8]) {
        let step_lo = 4 + self.track * self.track_size;
        let step_hi = step_lo + 256;
        let (pool_lo, pool_hi) = self.pool;
        for d in diff_payloads(before, after, 100_000) {
            let ok = (d.offset >= step_lo && d.offset < step_hi)
                || (d.offset >= pool_lo && d.offset < pool_hi);
            assert!(
                ok,
                "{}: unexpected byte change at {} ({})",
                self.name,
                d.offset,
                describe_offset(&self.spec, d.offset)
            );
        }
    }

    /// The step byte of every pool record this track owns, in pool order.
    fn pool_steps(&self, payload: &[u8]) -> Vec<u8> {
        let (lo, hi) = self.pool;
        (lo..hi)
            .step_by(6)
            .filter(|&o| payload[o] == self.track as u8)
            .map(|o| payload[o + 1])
            .collect()
    }
}

#[test]
fn brings_every_note_home_unchanged() {
    // Nothing about an unedited round trip may alter a note. Pitch, velocity and
    // micro-timing have to be exact; so does length, which is why `Note` carries
    // fractional steps rather than whole ones — the DN2's 3.25-step trig would
    // otherwise come home rounded and go back to the box too long.
    for f in fixtures() {
        let before = f.notes(&f.payload);
        let out = f.round_trip(&f.payload, |n| n);
        // The encoder groups a step's notes by pitch, so a write-back reorders
        // records the box stored in entry order — step 9 of the chord capture
        // came off the hardware as 68, 64, 61. Harmless now that every value
        // travels with its own note, so compare sorted.
        assert_eq!(by_pitch(&f.notes(&out)), by_pitch(&before), "{}", f.name);
    }
}

#[test]
fn an_unedited_second_write_is_a_no_op() {
    // Once the box's 0xFF defaults are materialised, import → write back →
    // import → write back must converge: no drift, no pool churn, ever.
    for f in fixtures() {
        let first = f.round_trip(&f.payload, |n| n);
        let second = f.round_trip(&first, |n| n);
        let diffs = diff_payloads(&first, &second, 100_000);
        assert!(
            diffs.is_empty(),
            "{}: second pass moved {} bytes, first at {:?}",
            f.name,
            diffs.len(),
            diffs.first()
        );
    }
}

#[test]
fn writes_back_a_payload_that_differs_from_the_box_only_in_that_track() {
    for f in fixtures() {
        let out = f.round_trip(&f.payload, |n| n);
        assert_eq!(
            diff_payloads(&f.payload, &out, 100_000).len(),
            f.first_pass_diffs,
            "{}",
            f.name
        );
        f.assert_only_track_bytes_changed(&f.payload, &out);
    }
}

#[test]
fn leaves_every_other_track_byte_identical() {
    for f in fixtures() {
        let out = f.round_trip(&f.payload, |n| n);
        let before = decode_pattern_kit(&f.spec, &f.payload).unwrap();
        let after = decode_pattern_kit(&f.spec, &out).unwrap();
        for other in 0..16 {
            if other == f.track {
                continue;
            }
            assert_eq!(
                track_notes(&after, other),
                track_notes(&before, other),
                "{}: track {} changed",
                f.name,
                other + 1
            );
        }
        assert_eq!(after.name, before.name);
        assert_eq!(after.tempo_bpm, before.tempo_bpm);
        assert_eq!(after.kit.sound_names, before.kit.sound_names);
        assert_eq!(after.kit_index, before.kit_index);
    }
}

#[test]
fn produces_a_minimal_diff_when_one_note_moves_one_step() {
    for f in fixtures() {
        let base = f.round_trip(&f.payload, |n| n);
        let last_step = f.notes(&base).iter().map(|n| n.step).max().unwrap();
        let moved = f.round_trip(&f.payload, |notes| {
            notes
                .into_iter()
                .map(|n| {
                    if n.step == last_step {
                        Note { step: n.step + 1, ..n }
                    } else {
                        n
                    }
                })
                .collect()
        });

        // A handful of bytes — two step words plus the moved trig's records —
        // not a rewritten pattern.
        assert_eq!(
            diff_payloads(&base, &moved, 100_000).len(),
            f.move_one_step_diffs,
            "{}",
            f.name
        );
        f.assert_only_track_bytes_changed(&base, &moved);

        let expected: Vec<Note> = f
            .notes(&base)
            .into_iter()
            .map(|n| {
                if n.step == last_step {
                    Note { step: n.step + 1, ..n }
                } else {
                    n
                }
            })
            .collect();
        assert_eq!(by_pitch(&f.notes(&moved)), by_pitch(&expected), "{}", f.name);
    }
}

#[test]
fn clears_the_track_when_emptied_touching_nothing_else() {
    for f in fixtures() {
        let (out, dropped) = encode_track_notes(&f.spec, &f.payload, f.track, &[]).unwrap();
        assert_eq!(dropped, 0);
        assert_eq!(f.notes(&out), Vec::new(), "{}", f.name);
        f.assert_only_track_bytes_changed(&f.payload, &out);
    }
}

// --- Determinism --------------------------------------------------------------

#[test]
fn encoding_the_same_notes_twice_yields_identical_bytes() {
    // The bug this test exists for: `encode_track_notes` grouped notes into a
    // `HashMap` and iterated it to claim pool records. Rust seeds each `HashMap`
    // separately, so the same pattern encoded to a different byte layout every
    // time — which quietly destroys both the minimal-diff contract and the
    // read-back verify, since neither means anything if the "same" write
    // produces different bytes.
    for f in fixtures() {
        let notes = f.notes(&f.payload);
        let a = encode_track_notes(&f.spec, &f.payload, f.track, &notes).unwrap().0;
        let b = encode_track_notes(&f.spec, &f.payload, f.track, &notes).unwrap().0;
        assert!(
            diff_payloads(&a, &b, 100_000).is_empty(),
            "{}: two encodes of the same notes disagreed",
            f.name
        );
    }
}

#[test]
fn claims_pool_records_in_ascending_step_order() {
    // The property that makes the above deterministic rather than merely
    // repeatable within one process: groups are claimed in ascending step order,
    // as the JS `Map` gives after its (step, pitch) sort. A single reordered
    // claim would still pass the encode-twice test on a lucky seed.
    for f in fixtures() {
        let out = f.round_trip(&f.payload, |n| n);
        let steps = f.pool_steps(&out);
        assert!(!steps.is_empty(), "{}: no pool records claimed", f.name);
        assert!(
            steps.windows(2).all(|w| w[0] <= w[1]),
            "{}: pool records out of step order: {:?}",
            f.name,
            steps
        );
    }
}
