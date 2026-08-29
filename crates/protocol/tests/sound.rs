//! The sound struct, against the real captures.
//!
//! `src/sound.rs` has unit tests over a synthetic struct, which prove the
//! arithmetic and nothing about the hardware. This suite is the other half: the
//! same decoder pointed at the DT2 and DN2 captures under `tests/fixtures/`.
//!
//! What it is really pinning is the **tag mask offset**. The name at +12 was
//! already read correctly by `decode_pattern_kit` before this module existed,
//! so a name that decodes proves only that the offsets did not move. The four
//! bytes at +8 are new, and the claim being tested is that they are a tag
//! bitmask rather than the machine/algorithm field they were first mistaken for.
//! Two properties say so, and neither would hold of a machine type:
//!
//!   * every factory `PRESET n` slot in every capture reads zero — untagged, as
//!     a factory init kit should be, where a machine field would name a machine;
//!   * the tagged capture's slots are sparse bitmasks, not small enumerations.
//!
//! Note which fixture carries which. All twelve fixtures that predate this
//! module are factory init kits, so *none* of them has a single tagged sound —
//! they can only show the field is zero. `digitone2-tagged-sounds-2026-08-01.syx`
//! was added for this suite: one pattern-kit message lifted out of a real DN2
//! project capture, with 9 of its 16 slots tagged by hand on the box.

mod common;

use common::*;
use digi_protocol::pattern::{dn2_spec, dt2_spec, Spec};
use digi_protocol::sound::*;

/// The only fixture with tagged sounds: one pattern-kit out of a real DN2
/// project capture, 9 of 16 slots tagged on the box. Every other fixture is a
/// factory init kit and reads all-zero.
const TAGGED: &str = "digitone2-tagged-sounds-2026-08-01.syx";

/// Every sound of the kit in the first pattern-kit of a capture.
fn kit_sounds(name: &str, spec: &Spec) -> Vec<Sound> {
    let payload = payload(name);
    let kit_base = spec.pattern.size;
    let kit_version = digi_protocol::pattern::u32_be(&payload, kit_base + 4);
    let kit_spec = spec
        .kits
        .get(&kit_version)
        .unwrap_or_else(|| panic!("{name}: unmapped {} kit version {kit_version}", spec.device));
    decode_kit_sounds(&payload, kit_base, kit_spec, spec.pattern.num_tracks)
        .into_iter()
        .enumerate()
        .map(|(t, r)| r.unwrap_or_else(|e| panic!("{name}: track {} sound: {e}", t + 1)))
        .collect()
}

/// The load-bearing test: all 16 slots of a real kit decode with **both**
/// magics landing. The foot only lands if `KitSpec::sound_size` is exactly
/// right, so this is simultaneously a check that the DT2 v3/v4 and DN2 v3 sizes
/// in `pattern.rs` are correct — sizes nothing had previously exercised, because
/// `decode_pattern_kit` only ever used them to stride to the next name.
#[test]
fn every_slot_of_a_real_kit_decodes_with_both_magics() {
    for (name, spec) in [
        ("digitakt2-A01-conditions-2026-08-02.syx", dt2_spec()),
        ("digitone2-A01-conditions-2026-08-02.syx", dn2_spec()),
        ("dn2-fresh-A01.syx", dn2_spec()),
    ] {
        let sounds = kit_sounds(name, &spec);
        assert_eq!(sounds.len(), 16, "{name}: expected 16 sounds");
        for (t, s) in sounds.iter().enumerate() {
            assert_eq!(
                digi_protocol::pattern::u32_be(&s.bytes, 0),
                SOUND_MAGIC_HEAD,
                "{name} track {}: head magic",
                t + 1
            );
            let n = s.bytes.len();
            assert_eq!(
                digi_protocol::pattern::u32_be(&s.bytes, n - 4),
                SOUND_MAGIC_FOOT,
                "{name} track {}: foot magic",
                t + 1
            );
        }
    }
}

/// The names this decoder reads must be the names `decode_pattern_kit` already
/// read, or one of the two is wrong about where the struct starts.
#[test]
fn names_agree_with_decode_pattern_kit() {
    for (name, spec) in [
        ("digitakt2-A01-conditions-2026-08-02.syx", dt2_spec()),
        ("digitone2-A01-conditions-2026-08-02.syx", dn2_spec()),
    ] {
        let kit = digi_protocol::pattern::decode_pattern_kit(&spec, &payload(name)).expect("decode");
        let mine: Vec<String> = kit_sounds(name, &spec).into_iter().map(|s| s.name).collect();
        assert_eq!(mine, kit.kit.sound_names, "{name}: sound names disagree");
    }
}

/// A factory init kit is untagged, on both boxes. A machine/algorithm field
/// would not be all-zero across all 16 slots of a kit that plays sound.
#[test]
fn factory_presets_carry_no_tags() {
    for (name, spec) in [
        ("digitakt2-A01-conditions-2026-08-02.syx", dt2_spec()),
        ("digitone2-A01-conditions-2026-08-02.syx", dn2_spec()),
        ("dn2-fresh-A01.syx", dn2_spec()),
    ] {
        for (t, s) in kit_sounds(name, &spec).iter().enumerate() {
            assert_eq!(s.tag_mask, 0, "{name} track {} ({:?}) should be untagged", t + 1, s.name);
            assert!(s.tags("digitone2").is_empty());
        }
    }
}

/// The tagged capture, pinned byte for byte. This is the hardware truth that
/// says +8 is a tag mask, so the exact values are asserted rather than a
/// property: if a future decode change shifts the field by even one byte, every
/// line of this table moves at once.
///
/// The mask **values** are ground truth, and this test asserts on `tag_mask`
/// only, never on a name — which outlives the calibration that has since
/// happened (`TAG_NAMES_DIGI`, and `tests/drive_preset.rs` for the check). A
/// name assertion here would fail for two different reasons, a shifted field
/// and a corrected table, and this test is meant to detect only the first.
#[test]
fn tagged_dn2_sounds_have_the_masks_the_box_wrote() {
    let sounds = kit_sounds(TAGGED, &dn2_spec());
    let got: Vec<(&str, u32)> = sounds
        .iter()
        .filter(|s| s.tag_mask != 0)
        .map(|s| (s.name.as_str(), s.tag_mask))
        .collect();

    // Read off the capture on 2026-08-26. Nine of sixteen slots were tagged on
    // the box; the other seven are untagged and are checked below by count.
    let expected: Vec<(&str, u32)> = vec![
        ("BLADERNR", 0x0300_3700),
        ("WAH FUNK", 0x0200_0800),
        ("A_303_INNIT", 0xc500_0400),
        ("GREYISH WIND", 0x0000_0c00),
        ("SUBFOCUS", 0x0000_0404),
        ("DIR INDICATOR", 0x1020_2000),
        ("CREAPING CRAWLER", 0x0108_2100),
        ("GOOD MORNING", 0x1228_2000),
        ("CELL NUCLEUS", 0x0000_0021),
    ];
    assert_eq!(got, expected);
    assert_eq!(sounds.len(), 16, "the other 7 slots exist and are untagged");

    // The shape that tells a tag set from an enumeration: a handful of bits,
    // spread across more than one byte of the u32.
    for (name, mask) in &got {
        let bits = mask.count_ones();
        assert!(
            (2..=12).contains(&bits),
            "{name}: {bits} bits set in {mask:#010x} — a tag set, not an enum, was expected"
        );
    }
}

/// The part of the bit→tag mapping that real patch names corroborate.
///
/// This is not a proof, and it is deliberately narrow: it asserts only the bits
/// where the sound's own name makes the tag near-certain, and says nothing about
/// the other 23. `A_303_INNIT` — a 303 acid line — carries bits 10, 26, 30 and
/// 31, which `TAG_NAMES` reads as Bass, Vintage, Mine and Favourite; `WAH FUNK`
/// carries 11 and 25, read as Lead and Bright.
///
/// **The calibration it was waiting for has since landed and agreed with it** —
/// see `tests/drive_preset.rs`, which holds 24 captures against all three boxes'
/// Overbridge filter grids. This stays because it is evidence of a different
/// kind: those tests check bytes against a screenshot, and this checks the
/// decoded meaning against what the patch is plainly called.
#[test]
fn the_calibrated_tag_bits_match_the_patch_names() {
    let sounds = kit_sounds(TAGGED, &dn2_spec());
    let by_name = |n: &str| -> Vec<&'static str> {
        sounds.iter().find(|s| s.name == n).unwrap_or_else(|| panic!("{n} in {TAGGED}")).tags("digitone2")
    };
    for tag in ["Bass", "Vintage", "Mine", "Favourite"] {
        assert!(by_name("A_303_INNIT").contains(&tag), "a 303 bass should be {tag}");
    }
    for tag in ["Lead", "Bright"] {
        assert!(by_name("WAH FUNK").contains(&tag), "a wah funk lead should be {tag}");
    }
    // The converse, on the same two sounds: an acid bass is not a Kick, and the
    // low bits are exactly the ones a machine-type field would have used.
    assert!(!by_name("A_303_INNIT").contains(&"Kick"));
    assert!(!by_name("WAH FUNK").contains(&"Kick"));
}

/// Reading a v4 DT2 sound at the v3 size finds a plausible name and is still
/// rejected. This is the foot magic earning its place on real bytes rather than
/// on the synthetic struct in the unit tests.
#[test]
fn a_real_sound_read_at_the_wrong_size_is_rejected() {
    let spec = dt2_spec();
    let payload = payload("digitakt2-A01-conditions-2026-08-02.syx");
    let kit_base = spec.pattern.size;
    let version = digi_protocol::pattern::u32_be(&payload, kit_base + 4);
    let right = spec.kits.get(&version).expect("mapped kit version").sound_size;
    let wrong = spec
        .kits
        .iter()
        .find(|(v, _)| **v != version)
        .map(|(_, k)| k.sound_size)
        .expect("dt2 has two kit versions to confuse");

    let off = kit_base + spec.kits.get(&version).unwrap().sounds_offset;
    assert!(decode_sound(&payload[off..], right).is_ok(), "the right size must decode");
    assert!(
        matches!(decode_sound(&payload[off..], wrong), Err(SoundError::BadFoot { .. })),
        "size {wrong} is not this kit's sound size and must be refused"
    );
}
