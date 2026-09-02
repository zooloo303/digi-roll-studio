//! The Analog Four's kit dump, against three captures off a real box.
//!
//! | fixture | what it is |
//! |---|---|
//! | `kit-00` | the reply to a `0x62` at index 0 — the kit named `POLYTRON` |
//! | `kit-01` | kit 1 from the `0x60` project stream, a second, independent kit |
//! | `kit-00-working` | the reply to a `0x68`, the box's edit buffer |
//!
//! **`kit-00` and `kit-00-working` carry the same payload, and that is the
//! point rather than an oversight.** `a4.rs`'s header explains why a capture
//! byte-identical to another one is normally worthless and kept out; here the
//! *identity itself* is the finding — two different request opcodes returned the
//! same object, which is the evidence for what `0x68` means. The two messages
//! differ in their type byte, so each one exercises its own parser against
//! bytes the box actually sent rather than against something this crate built.
//!
//! The offsets these tests pin were checked against **128 kits** from the same
//! project stream before any of them was written down; see
//! `a4_kit`'s module header for that count and what it covers.

use crate::common::fixture_bytes;

use digi_protocol::a4_kit::{
    is_a4_kit, is_a4_working_kit, parse_kit, parse_working_kit, read_kit, A4Kit, KIT_VERSION,
    NUM_SOUNDS, PAYLOAD_LEN, SOUNDS_OFFSET, SOUND_SIZE,
};
use digi_protocol::a4_pattern::NUM_TRACKS;
use digi_protocol::protocol::{
    build_dump_message, parse_sysex, DUMP_KIT, FAMILY_ANALOG_FOUR, FAMILY_DIGITAKT_2,
};
use digi_protocol::sound::A4_SOUND_MAGIC_HEAD;

const KIT00: &str = "analogfour-kit-00-2026-08-31.syx";
const KIT01: &str = "analogfour-kit-01-2026-08-31.syx";
const KIT00_WORKING: &str = "analogfour-kit-00-working-2026-08-31.syx";

fn kit(name: &str) -> A4Kit {
    parse_kit(&fixture_bytes(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn names(k: &A4Kit) -> Vec<&str> {
    (0..NUM_SOUNDS).map(|t| k.sound_name(t).unwrap()).collect()
}

/// The whole feature, in one assertion: the box's own name for each of the four
/// synth tracks' sounds, off a dump it answered.
#[test]
fn a_kit_names_its_four_synth_sounds() {
    let k = kit(KIT00);
    assert_eq!(k.index, 0);
    assert_eq!(k.version, KIT_VERSION);
    assert_eq!(k.name, "POLYTRON");
    assert_eq!(names(&k), ["ARPME", "WAVE MOD LEAD", "ALONE", "BRE"]);
}

/// A second kit through the same offsets, which is what stops the first from
/// being a lucky alignment. Different name, different sounds, one of them long
/// enough to matter — `CHORD SHARP` is eleven characters, so a stride that was
/// wrong by a few bytes would truncate or run on here rather than land clean.
#[test]
fn a_second_kit_reads_through_the_same_offsets() {
    let k = kit(KIT01);
    assert_eq!(k.index, 1);
    assert_eq!(k.name, "STEPPA");
    assert_eq!(names(&k), ["CHORDY", "CHORD SHARP", "BASS UNO", "LOFFE"]);
}

/// `0x68` returns the same object as `0x62` under a different type byte — the
/// box's edit buffer, with the index ignored. Each message parses only through
/// its own entry point, because a caller that asked for the working kit and got
/// a stored one (or the reverse) is holding something other than what it asked
/// for, and the two cannot be told apart afterwards.
#[test]
fn the_working_kit_is_the_same_object_under_a_different_type() {
    let stored = kit(KIT00);
    let working = parse_working_kit(&fixture_bytes(KIT00_WORKING)).unwrap();

    assert_eq!(working.name, stored.name);
    assert_eq!(names(&working), names(&stored));
    assert_eq!(working.index, 0, "the box echoes zero whatever was asked for");

    assert!(is_a4_kit(&parse_sysex(&fixture_bytes(KIT00))));
    assert!(is_a4_working_kit(&parse_sysex(&fixture_bytes(KIT00_WORKING))));
    assert!(!is_a4_working_kit(&parse_sysex(&fixture_bytes(KIT00))));

    assert!(parse_kit(&fixture_bytes(KIT00_WORKING)).is_err());
    assert!(parse_working_kit(&fixture_bytes(KIT00)).is_err());
}

/// **Four sounds, six tracks.** FX and CV are the sequencer's tracks and not the
/// kit's, so there is nothing to name — and `sound_name` says so with `None`
/// rather than an empty string, which is the distinction packet E deleted from
/// the gen-2 path for the same reason.
#[test]
fn a_kit_has_no_sound_for_the_fx_and_cv_tracks() {
    let k = kit(KIT00);
    assert_eq!(k.sounds.len(), NUM_SOUNDS);
    assert_eq!(NUM_TRACKS, 6, "SYN1-4, FX, CV");
    for t in NUM_SOUNDS..NUM_TRACKS {
        assert_eq!(k.sound_name(t), None, "track {t} has no sound in the kit");
    }
}

/// Every embedded container is the `0xBEEFBABA` pool-sound container, whole —
/// which is what makes the 350-byte stride a structure rather than a spacing.
/// Checked here at the byte level as well as through the decoder, because the
/// decoder finding a head is the *only* thing that says the arithmetic was
/// right, and a test that only asked the decoder would be circular.
#[test]
fn every_embedded_container_is_a_whole_pool_sound() {
    for name in [KIT00, KIT01] {
        let raw = fixture_bytes(name);
        let payload = parse_sysex(&raw).dump.unwrap().payload;
        assert_eq!(payload.len(), PAYLOAD_LEN, "{name}");

        let k = kit(name);
        for n in 0..NUM_SOUNDS {
            let at = SOUNDS_OFFSET + n * SOUND_SIZE;
            let head = u32::from_be_bytes(payload[at..at + 4].try_into().unwrap());
            assert_eq!(head, A4_SOUND_MAGIC_HEAD, "{name}: sound {n} head at +{at}");
            assert_eq!(k.sounds[n].bytes.len(), SOUND_SIZE, "{name}: sound {n}");
            assert_eq!(
                k.sounds[n].version, 6,
                "{name}: sound {n} — the same struct version the 0x53 pool dump carries",
            );
        }
    }
}

/// **A digi's kit dump is not read as an A4's**, and `0x52` is `DUMP_KIT` on
/// both — so the family byte is the only thing standing between a DT2 kit and
/// four confidently wrong sound names. The same trap `is_a4_pattern` documents
/// for `0x54`.
#[test]
fn a_digi_kit_dump_is_not_read_as_an_a4_one() {
    let payload = vec![0u8; PAYLOAD_LEN];
    let digi = build_dump_message(FAMILY_DIGITAKT_2, DUMP_KIT, 0, &payload);
    assert!(!is_a4_kit(&parse_sysex(&digi)));
    let err = parse_kit(&digi).unwrap_err();
    assert!(err.contains("not an A4 kit dump"), "{err}");

    // And the same bytes under the A4's family get past the type check and are
    // refused on their contents instead, which is what says the family byte is
    // doing the work above rather than something else about the message.
    let a4 = build_dump_message(FAMILY_ANALOG_FOUR, DUMP_KIT, 0, &payload);
    assert!(is_a4_kit(&parse_sysex(&a4)));
    assert!(parse_kit(&a4).unwrap_err().contains("version"));
}

/// An unsupported struct version is refused rather than read with version 11's
/// offsets. The gen-2 path keys its whole offset table off the equivalent
/// field; this one has a single layout and would otherwise silently apply it to
/// a struct that had moved.
#[test]
fn an_unsupported_struct_version_is_refused_rather_than_read() {
    let mut payload = parse_sysex(&fixture_bytes(KIT00)).dump.unwrap().payload;
    assert!(read_kit(0, &payload).is_ok());
    payload[3] = KIT_VERSION as u8 + 1;
    let err = read_kit(0, &payload).unwrap_err();
    assert!(err.contains("version 12"), "{err}");
}

/// A payload of the wrong length, and one whose sound containers are not where
/// the stride says — both are errors rather than a panic or a shrug.
#[test]
fn a_payload_that_is_not_a_kit_is_an_error_not_a_panic() {
    assert!(read_kit(0, &[]).is_err());
    assert!(read_kit(0, &vec![0u8; PAYLOAD_LEN - 1]).is_err());

    let mut payload = parse_sysex(&fixture_bytes(KIT00)).dump.unwrap().payload;
    // Break the last sound's head only: the first three still decode, so this
    // tests that a partial read is refused wholesale rather than returning the
    // sounds it managed.
    let at = SOUNDS_OFFSET + 3 * SOUND_SIZE;
    payload[at] = 0;
    let err = read_kit(0, &payload).unwrap_err();
    assert!(err.contains("sound 4"), "{err}");
}
