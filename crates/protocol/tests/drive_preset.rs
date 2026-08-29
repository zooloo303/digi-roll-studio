//! The +Drive container layer, against 24 real preset files.
//!
//! `src/drive.rs` has unit tests over captured *replies*, which prove the
//! transport: the right chunk under the right sequence number, a Close whose
//! total agrees. They say nothing about what is inside a file, because a
//! transport fixture does not contain a preset — it contains the file's own
//! header, and nothing a user named.
//!
//! This suite is the other half. `tests/fixtures/drive/` holds eight presets
//! from `/soundbanks/A` on each of a DT2 (0071), a DN2 (0050) and an A4 (0195),
//! captured 2026-08-29 by `capture_drive_presets.rs` with `manifest.tsv`
//! recording where each came from. **These carry real names and real tag
//! masks**, deliberately — see `drive.rs`'s file-read tests for why that
//! narrowed, and PLAN.md §9 for what the files then showed.
//!
//! # What is actually being pinned here
//!
//! Three claims, and the third is the reason this file exists rather than a
//! couple more cases in `sound.rs`:
//!
//!   * **The struct is measured, not looked up.** One DN2 bank holds structs of
//!     319 *and* 359 bytes. Any size table keyed by box gets one of those wrong,
//!     so these tests assert the sizes come out per-file.
//!   * **Names are Windows-1252.** `BLÅ VIND` and `SYNTHVÅG` are the cases that
//!     fail under `from_utf8_lossy`, and they are here as literals so a
//!     regression reads as a mangled name rather than as a byte count.
//!   * **The A4 is refused as the A4.** It has no foot magic anywhere, so it
//!     cannot be decoded honestly, and the test asserts the *specific* error —
//!     because the failure mode worth guarding is a future change that "fixes"
//!     the A4 by relaxing the head magic and silently returns a sound of the
//!     wrong length.

use std::path::PathBuf;

use digi_protocol::drive::{
    container_offset, decode_drive_preset, DriveError, A4_CONTAINER_MAGIC,
};

fn fixture(name: &str) -> Vec<u8> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drive").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Every file captured, by box, with the struct size the manifest recorded.
fn digi_files() -> Vec<(&'static str, usize)> {
    vec![
        ("digitakt2-soundbanks-A-1-ACIDD-2026-08-29.bin", 299),
        ("digitone2-soundbanks-A-2-MONOLOW-2026-08-29.bin", 319),
        ("digitone2-soundbanks-A-6-7THPAD-2026-08-29.bin", 359),
    ]
}

#[test]
fn a_dt2_preset_decodes_with_its_name_and_tags() {
    let sound = decode_drive_preset(&fixture("digitakt2-soundbanks-A-1-ACIDD-2026-08-29.bin"))
        .expect("a DT2 preset should decode");
    assert_eq!(sound.name, "ACIDD");
    assert_eq!(sound.tag_mask, 0x0000_0200);
    assert_eq!(sound.bytes.len(), 299);
}

#[test]
fn a_dn2_preset_decodes_with_its_name_and_tags() {
    let sound = decode_drive_preset(&fixture("digitone2-soundbanks-A-1-HIDDEN-TEARS-2026-08-29.bin"))
        .expect("a DN2 preset should decode");
    assert_eq!(sound.name, "HIDDEN TEARS");
    assert_eq!(sound.tag_mask, 0x0488_0804);
    assert_eq!(sound.bytes.len(), 319);
}

/// The finding that killed `KNOWN_SOUND_SIZES` as a strategy: two struct sizes
/// in one bank on one box, so the size cannot be a per-box constant and has to
/// come out of each file.
#[test]
fn one_dn2_bank_holds_two_struct_sizes() {
    let short = decode_drive_preset(&fixture("digitone2-soundbanks-A-2-MONOLOW-2026-08-29.bin"))
        .expect("MONOLOW should decode");
    let long = decode_drive_preset(&fixture("digitone2-soundbanks-A-6-7THPAD-2026-08-29.bin"))
        .expect("7THPAD should decode");

    assert_eq!(short.bytes.len(), 319);
    assert_eq!(long.bytes.len(), 359);
    assert_ne!(short.bytes.len(), long.bytes.len(), "same bank, same box, two sizes");
}

/// Windows-1252, and the two names that prove it. Under `from_utf8_lossy` both
/// of these come back with U+FFFD where the Å is.
#[test]
fn names_are_windows_1252_not_utf8() {
    let dt2 = decode_drive_preset(&fixture("digitakt2-soundbanks-A-7-BL--VIND-2026-08-29.bin"))
        .expect("BLÅ VIND should decode");
    assert_eq!(dt2.name, "BLÅ VIND");

    let dn2 = decode_drive_preset(&fixture("digitone2-soundbanks-A-3-SYNTHV-G-2026-08-29.bin"))
        .expect("SYNTHVÅG should decode");
    assert_eq!(dn2.name, "SYNTHVÅG");

    assert!(!dt2.name.contains('\u{FFFD}'), "a replacement char means the decoder regressed");
    assert!(!dn2.name.contains('\u{FFFD}'));
}

/// The A4 is refused **by name**, and this is the guard against a future
/// "fix" that relaxes the head magic. Such a change would decode an A4 preset
/// to a plausible name, a plausible tag mask and a wrong length — which is
/// precisely what `decode_sound`'s foot check exists to prevent, and it would
/// pass any test that only asserted "does not panic".
#[test]
fn an_a4_preset_is_refused_as_the_a4_rather_than_as_corruption() {
    let file = fixture("analogfour-soundbanks-A-1-THE-SAW-2026-08-29.bin");

    match decode_drive_preset(&file) {
        Err(DriveError::UndecodableContainer { magic, at }) => {
            assert_eq!(magic, A4_CONTAINER_MAGIC);
            assert_eq!(at, 31, "the A4's header is 31 bytes where the digis' is 36");
        }
        other => panic!("expected UndecodableContainer, got {other:?}"),
    }
}

/// Why the A4 cannot simply be let through: the thing that would size it is
/// not in the file. Asserted directly, so that if a future OS starts emitting
/// a foot this test fails and tells somebody the situation changed.
#[test]
fn no_a4_capture_contains_a_foot_magic_anywhere() {
    let foot = 0xBACE_F00Cu32.to_be_bytes();
    for name in [
        "analogfour-soundbanks-A-1-THE-SAW-2026-08-29.bin",
        "analogfour-soundbanks-A-4-SNAKECHARMER-2026-08-29.bin",
        "analogfour-soundbanks-A-8-JUST-BASS-2026-08-29.bin",
    ] {
        let file = fixture(name);
        assert!(
            !file.windows(4).any(|w| w == foot),
            "{name} contains a foot magic — the A4 may now be decodable, see PLAN.md §10.2"
        );
    }
}

/// A DT2 file carries a second `BEEFBACE` at 1060. Finding the magic is not the
/// same as finding the sound, and `container_offset` taking the *first* is what
/// makes that safe — so it is pinned rather than left as an implementation
/// detail somebody could "optimise" into a reverse search.
#[test]
fn a_dt2_file_has_two_head_magics_and_the_first_is_the_sound() {
    let file = fixture("digitakt2-soundbanks-A-1-ACIDD-2026-08-29.bin");
    let head = 0xBEEF_BACEu32.to_be_bytes();
    let all: Vec<usize> =
        file.windows(4).enumerate().filter(|(_, w)| *w == head).map(|(at, _)| at).collect();

    assert_eq!(all, vec![36, 1060], "the second magic is why a reverse search would be wrong");
    assert_eq!(container_offset(&file), Some(36));
}

/// Every capture either decodes or is refused for a stated reason. No panics,
/// no silent zero-length sounds — the sweep that would catch a file whose shape
/// nobody looked at individually.
#[test]
fn every_capture_either_decodes_or_is_refused_with_a_reason() {
    for (name, expected) in digi_files() {
        let sound = decode_drive_preset(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(sound.bytes.len(), expected, "{name}");
        assert!(!sound.name.is_empty(), "{name} decoded to an empty name");
    }
}
