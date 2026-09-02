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
    build_working_kit, is_a4_kit, is_a4_working_kit, parse_kit, parse_working_kit, read_kit,
    sound_for_kit, sound_slot, splice_sound, A4Kit, KIT_SOUND_VERSION, KIT_VERSION, NUM_SOUNDS,
    PAYLOAD_LEN, SOUNDS_OFFSET, SOUND_SIZE, V5_ONLY_BYTE, V5_ONLY_VALUE,
};
use digi_protocol::a4_pattern::NUM_TRACKS;
use digi_protocol::drive::a4_preset_sound;
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

// --- Putting a sound on a track ----------------------------------------------
//
// The load path's protocol half, and the fixtures make an unusually strong pair
// for it: the destination is a real kit off the box and the sound being spliced
// in is a real +Drive preset file off the same box. Nothing in these tests is
// constructed except the splice itself.

/// A +Drive preset file from the same A4, for the sound half of a splice.
fn drive_fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/drive")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading fixture {}: {e}", path.display()))
}

const THE_SAW: &str = "analogfour-soundbanks-A-1-THE-SAW-2026-08-29.bin";

/// The 350 bytes a kit slot takes, cut out of one of those files **and put in
/// the version a kit slot takes** — which is what the load path does, and why
/// nothing here splices a file's bytes as they come off the +Drive.
fn preset_sound(name: &str) -> Vec<u8> {
    sound_for_kit(&preset_sound_raw(name)).expect("a version 5 preset converts")
}

/// The file's own bytes, unconverted: struct version 5, as every +Drive preset
/// on this box is.
fn preset_sound_raw(name: &str) -> Vec<u8> {
    let file = drive_fixture(name);
    a4_preset_sound(&file).expect("an A4 preset").to_vec()
}

fn kit_payload(name: &str) -> Vec<u8> {
    parse_sysex(&fixture_bytes(name)).dump.unwrap().payload
}

/// The whole load, in the layer that has no box in it: SYN3 of a real kit ends
/// up holding a real +Drive preset, and the other three sounds and the kit's own
/// name come through untouched.
#[test]
fn a_splice_puts_a_drive_preset_on_one_synth_track_and_leaves_the_rest() {
    let before = kit_payload(KIT00);
    let sound = preset_sound(THE_SAW);

    let after = splice_sound(&before, 2, &sound).expect("a splice");

    let kit = read_kit(0, &after).expect("a kit");
    assert_eq!(kit.name, "POLYTRON", "the kit's own name is not the load's business");
    assert_eq!(names(&kit), ["ARPME", "WAVE MOD LEAD", "THE SAW", "BRE"]);
}

/// **Exactly 350 bytes move.** The three sounds nobody mentioned are somebody's
/// work, and so are the 978 bytes at the tail the FX and CV tracks live in — a
/// byte-level diff is the only assertion that covers the ones this crate cannot
/// name.
#[test]
fn a_splice_changes_only_its_own_slot_s_bytes() {
    let before = kit_payload(KIT00);
    let sound = preset_sound(THE_SAW);

    let after = splice_sound(&before, 1, &sound).expect("a splice");

    assert_eq!(after.len(), before.len());
    let differing: Vec<usize> =
        (0..before.len()).filter(|&i| before[i] != after[i]).collect();
    let slot = SOUNDS_OFFSET + SOUND_SIZE;
    assert!(
        differing.iter().all(|&i| (slot..slot + SOUND_SIZE).contains(&i)),
        "bytes changed outside SYN2's stride: {:?}",
        differing.iter().filter(|&&i| !(slot..slot + SOUND_SIZE).contains(&i)).collect::<Vec<_>>()
    );
    assert_eq!(&after[slot..slot + SOUND_SIZE], &sound[..], "the slot is the file's bytes verbatim");
}

/// A slot's own bytes spliced back in are a byte-for-byte no-op — which is what
/// makes [`sound_slot`] a usable backup and REVERT a real undo rather than an
/// approximation.
#[test]
fn a_slot_s_own_bytes_splice_back_to_the_kit_it_came_from() {
    let before = kit_payload(KIT01);
    let sound = preset_sound(THE_SAW);

    for slot in 0..NUM_SOUNDS {
        let backup = sound_slot(&before, slot).expect("a slot").to_vec();
        let auditioned = splice_sound(&before, slot, &sound).expect("a splice");
        assert_ne!(auditioned, before, "slot {slot} did not change");

        let reverted = splice_sound(&auditioned, slot, &backup).expect("a revert");
        assert_eq!(reverted, before, "slot {slot} did not come back");
    }
}

/// The FX and CV tracks sequence and have no sound. A selection pointing at one
/// is refused by name rather than wrapping onto SYN1.
#[test]
fn there_is_no_fifth_or_sixth_sound_in_a_kit() {
    let payload = kit_payload(KIT00);
    let sound = preset_sound(THE_SAW);

    for slot in NUM_SOUNDS..NUM_TRACKS {
        let err = splice_sound(&payload, slot, &sound).unwrap_err();
        assert!(err.contains("FX"), "slot {slot}: {err}");
        assert!(sound_slot(&payload, slot).is_err(), "slot {slot} should not read either");
    }
}

/// The two refusals that keep a splice from moving the slot boundaries: bytes
/// that are not an A4 sound container, and a slice of the right magic and the
/// wrong length.
#[test]
fn a_splice_refuses_anything_that_is_not_a_350_byte_a4_sound() {
    let payload = kit_payload(KIT00);
    let sound = preset_sound(THE_SAW);

    // The declared payload of the same file — 366 bytes, the cut this path used
    // to look like it wanted. Sixteen bytes too many.
    let file = drive_fixture(THE_SAW);
    let declared = &file[31..31 + 366];
    let err = splice_sound(&payload, 0, declared).unwrap_err();
    assert!(err.contains("366"), "{err}");

    // A digi's sound, off a DN2's +Drive: right length category, wrong box.
    let digi = drive_fixture("digitone2-soundbanks-A-2-MONOLOW-2026-08-29.bin");
    let err = splice_sound(&payload, 0, &digi[36..36 + SOUND_SIZE]).unwrap_err();
    assert!(err.contains("not one of this box's sounds"), "{err}");

    // The A4's head and 350 bytes, and the foot moved: a slice taken at the
    // right length from the wrong offset.
    let mut shifted = sound.clone();
    shifted[SOUND_SIZE - 1] = 0;
    let err = splice_sound(&payload, 0, &shifted).unwrap_err();
    assert!(err.contains("wrong length"), "{err}");

    // And a destination that is not a kit is refused before any of that.
    assert!(splice_sound(&[], 0, &sound).is_err());
    assert!(splice_sound(&vec![0u8; PAYLOAD_LEN], 0, &sound).is_err(), "version 0");
}

/// **The framing is the box's own.** `build_working_kit` on the payload the box
/// answered a `0x68` with reproduces that message byte for byte — the same
/// round-trip claim `a4.rs` makes for a pattern, and the only thing that says
/// the bytes a load sends are shaped like the bytes a load read.
#[test]
fn a_built_working_kit_is_the_box_s_own_0x58_message() {
    let captured = fixture_bytes(KIT00_WORKING);
    let payload = parse_sysex(&captured).dump.unwrap().payload;

    let built = build_working_kit(&payload).expect("a sendable kit");

    assert_eq!(built, captured, "the built frame is not the captured one");
    // And it parses as what it claims to be, through the reader the app uses.
    assert_eq!(parse_working_kit(&built).unwrap().name, "POLYTRON");
}

/// A spliced kit frames and parses too, which is the assertion the probe on
/// hardware was allowed to run on the strength of: what goes on the wire is a
/// well-formed working-kit dump whose SYN3 is the preset.
#[test]
fn a_spliced_kit_frames_as_a_working_kit_dump() {
    let payload = kit_payload(KIT00);
    let sound = preset_sound(THE_SAW);
    let spliced = splice_sound(&payload, 2, &sound).expect("a splice");

    let wire = build_working_kit(&spliced).expect("a sendable kit");

    assert_eq!(wire[0], 0xf0);
    assert_eq!(*wire.last().unwrap(), 0xf7);
    assert!(wire[1..wire.len() - 1].iter().all(|b| b & 0x80 == 0), "a high bit inside the frame");
    let sent = parse_working_kit(&wire).expect("a working kit");
    assert_eq!(sent.sound_name(2), Some("THE SAW"));
}

/// A payload that is not a kit never reaches a wire, and neither does one whose
/// version these offsets were not read against.
#[test]
fn only_a_kit_of_the_measured_version_is_framed() {
    assert!(build_working_kit(&[]).is_err());
    assert!(build_working_kit(&vec![0u8; PAYLOAD_LEN]).is_err(), "version 0");
}

// --- The version a kit slot takes --------------------------------------------

/// **The conversion is two bytes, and this is the pair that says so.** A +Drive
/// file is struct version 5 and a kit slot takes version 6; the box does not
/// refuse the mismatch, it replaces the track with an init sound. So the
/// conversion is part of a load, and what it does is checked here against the
/// numbers 28 pairs of the same sound in both versions produced on 2026-09-01.
#[test]
fn a_version_5_preset_becomes_a_version_6_kit_sound_in_two_bytes() {
    let raw = preset_sound_raw(THE_SAW);
    assert_eq!(u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]), 5, "the file's version");
    assert_eq!(raw[V5_ONLY_BYTE], V5_ONLY_VALUE);

    let for_kit = sound_for_kit(&raw).expect("a version 5 preset converts");

    assert_eq!(
        u32::from_be_bytes([for_kit[4], for_kit[5], for_kit[6], for_kit[7]]),
        KIT_SOUND_VERSION
    );
    assert_eq!(for_kit[V5_ONLY_BYTE], 0);
    // And nothing else moved: the parameters, the name, the tag mask and both
    // magics are the file's own.
    let differing: Vec<usize> = (0..SOUND_SIZE).filter(|&i| raw[i] != for_kit[i]).collect();
    assert_eq!(differing, vec![7, V5_ONLY_BYTE], "only the version word and the one byte");
}

/// A sound already in the kit's version passes through untouched — a revert
/// sends a slot's own bytes back, and a conversion that "tidied" them would make
/// an undo into an edit.
#[test]
fn a_version_6_sound_passes_through_unaltered() {
    let payload = kit_payload(KIT00);
    for slot in 0..NUM_SOUNDS {
        let own = sound_slot(&payload, slot).expect("a slot");
        assert_eq!(sound_for_kit(own).expect("version 6 needs nothing"), own);
    }
}

/// A version nobody has measured is an error rather than a guess. The cost of
/// being wrong here is not a refused load — it is a load that reports success
/// with an init sound on the track.
#[test]
fn an_unmeasured_struct_version_is_refused_rather_than_converted() {
    let mut odd = preset_sound_raw(THE_SAW);
    odd[7] = 7;

    let err = sound_for_kit(&odd).unwrap_err();
    assert!(err.contains("version 7"), "{err}");
    assert!(err.contains("init sound"), "the reason must say what going ahead costs: {err}");

    // And the splice refuses it too, so no path reaches a wire around this.
    let err = splice_sound(&kit_payload(KIT00), 0, &odd).unwrap_err();
    assert!(err.contains("sound_for_kit"), "{err}");
}

/// The unconverted file is refused by the splice, which is the guard that
/// matters: this is the mistake the box punishes silently.
#[test]
fn a_version_5_sound_never_reaches_a_kit() {
    let raw = preset_sound_raw(THE_SAW);
    let err = splice_sound(&kit_payload(KIT00), 0, &raw).unwrap_err();
    assert!(err.contains("version 5"), "{err}");
    assert!(err.contains("init sound"), "{err}");
}
