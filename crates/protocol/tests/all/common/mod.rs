//! Fixture loading shared by the protocol integration tests.
//!
//! The fixtures under `tests/fixtures/` are real SysEx captures from a Digitakt
//! II (OS 1.15B, build 0070) and a Digitone II (OS 1.10D, build 0049), taken
//! read-only in August 2026. They are the only hardware-derived truth this
//! repository has; every expected value in these suites was read out of them by
//! the JS original (`~/Projects/digi-roll`) before being written down here, so
//! the tests pin digi-roll's behaviour rather than the port's.
//!
//! **The `analogfour-*.syx` fixtures are different in one way that matters.**
//! They are gen-1 pattern dumps from an Analog Four mk1 (OS 1.55B), taken off
//! the box's own front-panel SysEx Dump menu on 2026-08-30 and 2026-08-31, and
//! there is **no JS original for this format at all** — elk-herd documents only
//! gen-2. Every expectation in `a4.rs` was measured from these nine files and
//! nothing else, so those tests pin the captures rather than a second
//! implementation. Two of the findings they carry could not have come from the
//! files either way: the trig-state model and the octave numbering were settled
//! by looking at the box's screen (DEVELOPMENT.md lesson 16), and what the
//! fixtures do is hold the exact bytes that observation was made against.

#![allow(dead_code)]

use digi_protocol::pattern::Note;
use digi_protocol::protocol::{split_sysex_stream, SysExKind, DUMP_PATTERN_KIT};

pub fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading fixture {}: {e}", path.display()))
}

/// Every pattern-kit dump in a capture, as `(index, payload)`.
///
/// Asserts the whole stream checksums and counts: a capture that does not is
/// not evidence of anything, and a silent decode failure here would make every
/// downstream expectation meaningless.
pub fn pattern_kits(name: &str) -> Vec<(u8, Vec<u8>)> {
    let bytes = fixture_bytes(name);
    let messages = split_sysex_stream(&bytes);
    assert!(!messages.is_empty(), "{name}: no SysEx messages");
    let mut kits = Vec::new();
    for m in &messages {
        assert_eq!(m.kind, SysExKind::Dump, "{name}: non-dump message in capture");
        let d = m.dump.as_ref().unwrap();
        assert!(d.checksum_ok, "{name}: bad checksum on a dump message");
        assert!(d.count_ok, "{name}: bad byte count on a dump message");
        if d.dump_type == DUMP_PATTERN_KIT {
            kits.push((d.index, d.payload.clone()));
        }
    }
    kits
}

/// The one pattern-kit payload in a single-pattern capture.
pub fn payload(name: &str) -> Vec<u8> {
    let kits = pattern_kits(name);
    assert_eq!(kits.len(), 1, "{name}: expected exactly one pattern-kit dump");
    kits.into_iter().next().unwrap().1
}

/// A note in the compact form the expectations are written in:
/// `(step, pitch, velocity, length in steps, micro in 1/24 steps)`.
pub fn note(step: u8, pitch: u8, velocity: u8, len_steps: f64, micro_ticks: i8) -> Note {
    Note {
        step,
        pitch,
        velocity,
        len_steps,
        micro: micro_ticks as f64 / 24.0,
    }
}

pub fn notes_from(rows: &[(u8, u8, u8, f64, i8)]) -> Vec<Note> {
    rows.iter().map(|&(s, p, v, l, m)| note(s, p, v, l, m)).collect()
}

/// Notes sorted by `(step, pitch)`. The encoder groups a step's notes by pitch,
/// so a write-back reorders records the box stored in entry order — harmless
/// now that every value travels with its own note, but comparisons have to sort
/// before matching.
pub fn by_pitch(notes: &[Note]) -> Vec<Note> {
    let mut v = notes.to_vec();
    v.sort_by(|a, b| a.step.cmp(&b.step).then(a.pitch.cmp(&b.pitch)));
    v
}

/// One gen-1 Analog Four pattern dump from a fixture, parsed and verified.
///
/// Asserts the capture is a single well-formed message, for the reason
/// [`pattern_kits`] does: a capture whose checksum or count does not hold is not
/// evidence of anything, and a silent decode failure here would make every
/// expectation downstream meaningless.
pub fn a4_pattern(name: &str) -> digi_protocol::a4_pattern::A4Pattern {
    let bytes = fixture_bytes(name);
    digi_protocol::a4_pattern::parse_pattern(&bytes)
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// One gen-1 Analog Four **working** pattern — the reply to a `0x6a`, the box's
/// edit buffer rather than a stored slot.
///
/// A separate helper rather than a flag on [`a4_pattern`], because the two must
/// not be interchangeable: the stored parser refusing a working dump is the
/// check that keeps a live buffer from being mistaken for a saved slot, and a
/// helper that accepted either would take that check away from every test that
/// uses it.
pub fn a4_working_pattern(name: &str) -> digi_protocol::a4_pattern::A4Pattern {
    let bytes = fixture_bytes(name);
    digi_protocol::a4_pattern::parse_working_pattern(&bytes)
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}
