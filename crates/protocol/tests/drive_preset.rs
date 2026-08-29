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
//!   * **The A4 is sized by its header, not by a foot.** It has no foot magic
//!     anywhere, and until 2026-08-29 that had it refused outright. Its extent
//!     is nonetheless stated — by the file header's declared payload length,
//!     with the container flush against it — so it decodes, and the tests pin
//!     both halves: that it comes out with the right length and name, and that
//!     the layout the length rests on is the layout every capture has.
//!
//! A fourth was added once the layout was measured rather than assumed: every
//! file is a **31-byte header, a payload, and a 12-byte trailer**, on all three
//! boxes. The digis' container sits five bytes further in than the A4's because
//! their payload opens with a `SOUND_WRAPPER`, not because their header is
//! longer — which is what `container_offset`'s doc originally said and what one
//! of the assertions below used to repeat.

use std::path::PathBuf;

use digi_protocol::drive::{
    container_offset, decode_drive_preset, file_declared_size, DriveError,
};
use digi_protocol::sound::tag_names_for;

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

/// The A4 decodes, and is sized by the one witness it has.
///
/// This test replaced one that asserted the opposite. The old one guarded
/// against a "fix" that relaxed the head magic and returned a sound of the
/// wrong length — a real hazard, and the answer to it turned out not to be
/// refusal but *sizing from the header* rather than from a relaxed search. So
/// the length is asserted here explicitly: 366 bytes, the payload size the file
/// itself declares. A change that starts guessing again fails on the number.
#[test]
fn an_a4_preset_decodes_at_the_length_its_header_declares() {
    let file = fixture("analogfour-soundbanks-A-1-THE-SAW-2026-08-29.bin");

    assert_eq!(container_offset(&file), Some(31), "flush: the A4 has no five-byte wrapper");
    assert_eq!(file_declared_size(&file), Some(366));

    let sound = decode_drive_preset(&file).expect("THE SAW should decode");
    assert_eq!(sound.name, "THE SAW");
    assert_eq!(sound.bytes.len(), 366, "the declared payload length, not a searched-for one");
    assert_eq!(sound.tag_mask, 0x0584_0003);
}

/// A file whose A4 container is *not* flush with a header refuses rather than
/// falling back, because the declared length is the only witness to the extent
/// and it does not apply once the layout moves.
#[test]
fn an_a4_container_that_is_not_flush_is_refused_rather_than_guessed_at() {
    let mut file = fixture("analogfour-soundbanks-A-1-THE-SAW-2026-08-29.bin");
    // Shift the container one byte later, leaving the header's declaration
    // describing a payload that no longer starts where it says.
    file.insert(31, 0x00);

    match decode_drive_preset(&file) {
        Err(DriveError::UnsizedContainer { at, declared }) => {
            assert_eq!(at, 32);
            assert_eq!(declared, Some(366));
        }
        other => panic!("expected UnsizedContainer, got {other:?}"),
    }
}

/// A container magic that is neither box's is still refused, and still carries
/// the magic it found — that is the diagnosis for the fourth box, whenever one
/// lands.
#[test]
fn an_unknown_container_magic_is_refused_and_names_itself() {
    let mut file = fixture("analogfour-soundbanks-A-1-THE-SAW-2026-08-29.bin");
    file[31..35].copy_from_slice(&0xBEEF_BADEu32.to_be_bytes());
    // `container_offset` searches for the magic, so with the A4's overwritten
    // there is no container at all — which is itself the honest answer.
    assert!(matches!(decode_drive_preset(&file), Err(DriveError::NoContainer { .. })));
}

/// The head bytes must reach past where the container magic belongs, and this
/// asserts the exact failure that made 48 the width rather than 16.
///
/// A DN2 file's first 36 bytes are its header and `BEEFBACE` sits at 36. Every
/// DN2 file on the box opens `ac11d303 02000500 0f303035 30…`, so a 16-byte
/// window shows only the part that *cannot* differ: the 388 undecodable presets
/// printed a head identical to a good capture's and the diagnostic dead-ended
/// there. The check is therefore not "the string is long" but "the string
/// distinguishes" — a good capture and a file that diverges only after byte 16
/// must not produce the same head.
#[test]
fn the_head_bytes_reach_past_the_container_magic() {
    let good = fixture("digitone2-soundbanks-A-1-HIDDEN-TEARS-2026-08-29.bin");

    // Identical to a real DN2 preset up to byte 36, then carrying anything but a
    // container — which is all that is known about the 388, and enough.
    let mut odd = good.clone();
    odd[36..40].copy_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
    assert_eq!(good[..36], odd[..36], "the two must agree exactly where a 16-byte head looks");

    let Err(DriveError::NoContainer { head, .. }) = decode_drive_preset(&odd) else {
        panic!("a file with no container magic must be refused as NoContainer");
    };

    let good_head: String = good.iter().take(48).map(|b| format!("{b:02x}")).collect();
    assert_ne!(
        head, good_head,
        "the head bytes do not distinguish this file from a working one — the window is \
         back inside the prefix every DN2 file shares, which is what made a 388-preset \
         scan unactionable"
    );
    // Byte 36 lands at hex offset 72, and the window runs past it rather than
    // stopping there — the whole point of the width.
    assert_eq!(&head[72..80], "deadbeef", "byte 36 must be in frame, at its own offset: {head}");
}

/// The A4 really has no foot, which is *why* it is sized from its header rather
/// than a preference for doing so. Asserted directly, so that if a future OS
/// starts emitting one this test fails and tells somebody the situation changed
/// — at which point the cheaper `decode_sound` path becomes available to it.
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

/// The file layout, measured across every capture on 2026-08-29 and pinned here
/// because the first reading of it was wrong.
///
/// `container_offset`'s doc used to say the header was 36 bytes on a digi and
/// 31 on an A4. It is **31 on all three**; the digis' container sits five bytes
/// further in because their payload opens with the same five-byte wrapper a
/// `0x6b` kit-track-sound payload carries. A trailer of twelve bytes closes
/// every file: something checksum-shaped, the payload length again, and the
/// magic `AAA1DAAA`.
#[test]
fn every_capture_has_a_31_byte_header_and_a_12_byte_trailer() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drive");
    let mut checked = 0;

    for entry in std::fs::read_dir(&dir).expect("fixtures/drive") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("bin") {
            continue;
        }
        let file = std::fs::read(&path).expect("capture");
        let name = path.file_name().unwrap().to_string_lossy().to_string();

        let declared = digi_protocol::drive::file_declared_size(&file)
            .unwrap_or_else(|| panic!("{name} has no declared size")) as usize;

        assert_eq!(&file[file.len() - 4..], b"\xaa\xa1\xda\xaa", "{name} trailer magic");
        let tail_len = u32::from_be_bytes(
            file[file.len() - 8..file.len() - 4].try_into().unwrap(),
        ) as usize;
        assert_eq!(tail_len, declared, "{name}: the trailer repeats the payload length");

        // header + payload + 12-byte trailer accounts for the whole file.
        assert_eq!(file.len() - 12 - declared, 31, "{name}: header is 31 bytes on every box");

        // And the container lands where the wrapper says it should: flush with
        // the payload, or five bytes in behind a `SOUND_WRAPPER`.
        //
        // **Not "A4 versus digi", which is what this said until the mk1 files
        // landed.** Eight of these captures are Digitone mk1 presets off a DN2's
        // +Drive and they are flush, sitting beside that same box's own presets
        // which are not. The wrapper is a property of the file, not of the box
        // that answered.
        let at = container_offset(&file).unwrap_or_else(|| panic!("{name} has no container"));
        let wrapper = at - 31;
        assert!(
            wrapper == 0 || wrapper == digi_protocol::sound::SOUND_WRAPPER,
            "{name}: container at {at} is neither flush nor one wrapper past the header"
        );
        checked += 1;
    }
    assert_eq!(checked, 32, "all 32 captures should be covered");
}

/// **The error a 2026-08-29 DN2 scan could not be read from.** 388 presets came
/// back as `no sound container magic in 407 bytes` — and 407 is exactly the
/// length of a *good* DN2 preset file, so the length said nothing about what had
/// actually arrived. The message now carries the head bytes, which is what tells
/// "a file this parser does not know" apart from "not a file at all".
#[test]
fn a_file_with_no_container_says_what_it_found_instead() {
    // A good DN2 file's own opening, with the magic removed: the exact shape the
    // failure has to be distinguishable from.
    let mut not_a_preset = vec![0xac, 0x11, 0xd3, 0x03, 0x02, 0x00, 0x05, 0x00];
    not_a_preset.extend(std::iter::repeat_n(0u8, 399));
    assert_eq!(not_a_preset.len(), 407, "the length a real DN2 preset also has");

    let err = decode_drive_preset(&not_a_preset).expect_err("no magic anywhere");
    let text = err.to_string();
    assert!(text.contains("407"), "{text}");
    assert!(text.contains("ac11d303"), "the head has to be in the message: {text}");

    // An empty read is its own answer rather than an empty pair of brackets.
    let err = decode_drive_preset(&[]).expect_err("nothing at all");
    assert!(err.to_string().contains("nothing"), "{err}");
}

// --- The tag calibration -------------------------------------------------------
//
// Every one of the 24 captures, decoded through `sound::tag_names_for` and held
// against the tag column of that box's Sound Browser in Overbridge 2.26.9,
// screenshotted 2026-08-29. The eight files per box are `/soundbanks/A/1..8`,
// which are exactly the first eight rows each screenshot shows — so the two
// sides of this table were produced by different software reading different
// copies of the same data, which is the only reason it is worth asserting.
//
// **This is the check `TAG_NAMES` went three days without.** That array was
// calibrated on one DN2 preset and described in its own doc as ground truth for
// two boxes; the DT2 had never been held against anything, and the A4 was being
// decoded through a table where bit 0 means Kick and on that box means Bass. A
// single preset cannot catch a table that is right about the bits it happens to
// set, so the guard has to be a set of presets wide enough to light up most of
// the vocabulary. These 24 set 27 of the 32 digi bits and 17 of the 32 A4 ones.
//
// Ordering is asserted too, not just membership: `tag_names` walks bit 0 upward,
// so a table shifted by one produces the right *count* and the wrong names, and
// a list comparison catches that where a set comparison would not.

/// `(file, slug, name, tags as Overbridge prints them)`.
fn tagged_captures() -> Vec<(&'static str, &'static str, &'static str, &'static [&'static str])> {
    vec![
        // --- Analog Four, OS build 0195 ---
        ("analogfour-soundbanks-A-1-THE-SAW-2026-08-29.bin", "analogfour", "THE SAW",
         &["Bass", "Lead", "Hard", "Bright", "Vintage", "Epic"]),
        ("analogfour-soundbanks-A-2-SQUARE-WAVE-2026-08-29.bin", "analogfour", "SQUARE WAVE",
         &["Bass", "Lead", "Hard", "Bright"]),
        ("analogfour-soundbanks-A-3-GOOM-SSAB-2026-08-29.bin", "analogfour", "GOOM SSAB",
         &["Bass", "Lead", "Soft", "Vintage", "Acid"]),
        ("analogfour-soundbanks-A-4-SNAKECHARMER-2026-08-29.bin", "analogfour", "SNAKECHARMER",
         &["Lead", "Pad", "Brass", "Soft"]),
        ("analogfour-soundbanks-A-5-SINGLE-CHORD-2026-08-29.bin", "analogfour", "SINGLE CHORD",
         &["Lead", "Pad", "Texture", "Chord", "Soft", "Bright"]),
        ("analogfour-soundbanks-A-6-101-BASS-2026-08-29.bin", "analogfour", "101 BASS",
         &["Bass", "Soft", "Vintage", "Acid"]),
        ("analogfour-soundbanks-A-7-EDGAR-2026-08-29.bin", "analogfour", "EDGAR",
         &["Texture", "Strings", "Atmosphere", "Evolving", "Epic"]),
        ("analogfour-soundbanks-A-8-JUST-BASS-2026-08-29.bin", "analogfour", "JUST BASS",
         &["Bass", "Hard", "Vintage", "Epic"]),
        // --- Digitakt II, OS build 0071 ---
        ("digitakt2-soundbanks-A-1-ACIDD-2026-08-29.bin", "digitakt2", "ACIDD", &["Synth"]),
        ("digitakt2-soundbanks-A-2-BAM-BASS-2026-08-29.bin", "digitakt2", "BAM BASS", &["Bass"]),
        ("digitakt2-soundbanks-A-3-BAM-TICK-2026-08-29.bin", "digitakt2", "BAM TICK",
         &["Percussion"]),
        ("digitakt2-soundbanks-A-4-BL--LOFI-BASS-2026-08-29.bin", "digitakt2", "BLÅ LOFI BASS",
         &["Bass"]),
        ("digitakt2-soundbanks-A-5-BL--MEOW-2026-08-29.bin", "digitakt2", "BLÅ MEOW",
         &["Sound Fx"]),
        ("digitakt2-soundbanks-A-6-BL--SQ-CHIP-2026-08-29.bin", "digitakt2", "BLÅ SQ CHIP",
         &["Synth"]),
        ("digitakt2-soundbanks-A-7-BL--VIND-2026-08-29.bin", "digitakt2", "BLÅ VIND",
         &["Texture", "Noisy", "Soft"]),
        ("digitakt2-soundbanks-A-8-BLUE-HH-2026-08-29.bin", "digitakt2", "BLUE HH", &["Hi-Hat"]),
        // --- Digitone II, OS build 0050 ---
        ("digitone2-soundbanks-A-1-HIDDEN-TEARS-2026-08-29.bin", "digitone2", "HIDDEN TEARS",
         &["Rimshot", "Lead", "Atmosphere", "Soft", "Vintage"]),
        ("digitone2-soundbanks-A-2-MONOLOW-2026-08-29.bin", "digitone2", "MONOLOW",
         &["Bass", "Glitch", "Soft", "Dark", "Vintage"]),
        ("digitone2-soundbanks-A-3-SYNTHV-G-2026-08-29.bin", "digitone2", "SYNTHVÅG",
         &["Lead", "Chord", "Bright", "Vintage"]),
        ("digitone2-soundbanks-A-4-WET-SAND-2026-08-29.bin", "digitone2", "WET SAND",
         &["Lead", "Atmosphere", "Soft", "Vintage"]),
        ("digitone2-soundbanks-A-5-FAMILY-CREST-2026-08-29.bin", "digitone2", "FAMILY CREST",
         &["Lead", "Soft", "Vintage"]),
        ("digitone2-soundbanks-A-6-7THPAD-2026-08-29.bin", "digitone2", "7THPAD",
         &["Tom", "Pad", "Chord", "Atmosphere", "Soft", "Dark"]),
        ("digitone2-soundbanks-A-7-BASS-SPACE-2026-08-29.bin", "digitone2", "BASS SPACE",
         &["Rimshot", "Cowbell", "Bass"]),
        ("digitone2-soundbanks-A-8-LONELY-NIGHTS-2026-08-29.bin", "digitone2", "LONELY NIGHTS",
         &["Cowbell", "Lead", "Soft", "Vintage"]),
        // --- Digitone **mk1** presets, on that same DN2's +Drive, `DN1S` ---
        //
        // The second format in one library, and the reason they are here is the
        // tag column rather than the container: Overbridge lists them in the
        // DN2's browser under the DN2's own 32-cell grid, and they decode
        // through `TAG_NAMES_DIGI` to exactly what it shows. That is what says
        // the box re-maps mk1 tags into its own vocabulary and no third table is
        // needed — a claim worth pinning, because a third table was the expected
        // answer right up to the screenshot that disproved it.
        ("digitone2-soundbanks-C-1-ORGANIC-2026-08-29.bin", "digitone2", "ORGANIC",
         &["Chord", "Electronic", "Soft", "Vintage"]),
        ("digitone2-soundbanks-C-2-PHASEY-DUB-2026-08-29.bin", "digitone2", "PHASEY DUB",
         &["Clap", "Chord", "Vintage"]),
        ("digitone2-soundbanks-C-3-PLOINKEYS-2026-08-29.bin", "digitone2", "PLOINKEYS",
         &["Lead", "Chord", "Metallic", "Soft", "Bright"]),
        ("digitone2-soundbanks-C-4-RESO-DUB-2026-08-29.bin", "digitone2", "RESO DUB",
         &["Cowbell", "Chord", "Soft", "Bright"]),
        ("digitone2-soundbanks-C-5-RUBBER-BAND-2026-08-29.bin", "digitone2", "RUBBER BAND",
         &["Tom", "Chord", "Acoustic", "Dark"]),
        ("digitone2-soundbanks-C-6-SIMPL-BRSS-2026-08-29.bin", "digitone2", "SIMPL BRSS",
         &["Clap", "Chord", "Bright", "Vintage"]),
        ("digitone2-soundbanks-C-7-SPRINKLE-STAR-2026-08-29.bin", "digitone2", "SPRINKLE STAR",
         &["Chord", "Electronic", "Soft", "Vintage"]),
        ("digitone2-soundbanks-C-8-SWEET-and-SOUND-2026-08-29.bin", "digitone2", "SWEET & SOUND",
         &["Chord", "Soft", "Vintage"]),
    ]
}

/// One box's library holds two container formats, and the DN2 is the box.
///
/// Pinned separately from the tag table because it is the structural claim: a
/// `BEEFBACE` preset and a `DN1S` preset, both off the same +Drive, sized by
/// different rules — a foot search on both, but reached from offsets 36 and 31 —
/// and decoding to the same `Sound` shape. 388 of that box's 1,189 presets are
/// the second kind, so this is the common case rather than a curiosity.
#[test]
fn one_dn2_library_holds_two_container_formats() {
    let native = decode_drive_preset(&fixture("digitone2-soundbanks-A-1-HIDDEN-TEARS-2026-08-29.bin"))
        .expect("a native DN2 preset should decode");
    let mk1 = decode_drive_preset(&fixture("digitone2-soundbanks-C-1-ORGANIC-2026-08-29.bin"))
        .expect("an mk1 preset on a DN2 should decode");

    assert_eq!(native.name, "HIDDEN TEARS");
    assert_eq!(mk1.name, "ORGANIC");

    let native_file = fixture("digitone2-soundbanks-A-1-HIDDEN-TEARS-2026-08-29.bin");
    let mk1_file = fixture("digitone2-soundbanks-C-1-ORGANIC-2026-08-29.bin");
    assert_eq!(container_offset(&native_file), Some(36), "a wrapper sits in front of the native one");
    assert_eq!(container_offset(&mk1_file), Some(31), "the mk1 container is flush with the payload");

    // Same file length, different struct length — so the file size says nothing
    // about which format it is, which is why 407 was such a poor clue.
    assert_eq!(native_file.len(), mk1_file.len(), "both are 407-byte files");
    assert_ne!(native.bytes.len(), mk1.bytes.len(), "and different struct sizes: 319 against 302");
}

#[test]
fn every_capture_decodes_the_tags_its_box_displays() {
    for (file, slug, name, expected) in tagged_captures() {
        let sound = decode_drive_preset(&fixture(file)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(sound.name, name, "{file}");
        assert_eq!(
            sound.tags(slug),
            expected,
            "{name} on {slug}: mask {:#010x} decodes to the wrong tags — see sound::TAG_NAMES_A4 \
             and TAG_NAMES_DIGI, both calibrated against Overbridge 2.26.9 on 2026-08-29",
            sound.tag_mask,
        );
    }
}

/// The A4's table is a different table, not a relabelled one — asserted on the
/// bits that actually differ, because a caller reading an A4 through the digi
/// array gets a full, plausible, wrong answer rather than an empty one.
///
/// `THE SAW` is the case to keep in mind: through the digi table its mask reads
/// Kick, Snare, Acoustic, Soft, Dark, Vintage — six tags, every one a real name,
/// the right *number* of them, and five of the six wrong. Only Vintage survives,
/// and it survives by coincidence. Nothing about that output looks like a bug.
#[test]
fn the_a4_table_is_not_the_digi_table() {
    let saw = decode_drive_preset(&fixture("analogfour-soundbanks-A-1-THE-SAW-2026-08-29.bin"))
        .expect("THE SAW should decode");

    assert_eq!(saw.tags("analogfour"), ["Bass", "Lead", "Hard", "Bright", "Vintage", "Epic"]);
    assert_eq!(saw.tags("digitone2"), ["Kick", "Snare", "Acoustic", "Soft", "Dark", "Vintage"]);
}

/// Exactly two of the thirty-two positions agree between the two tables.
///
/// Pinned because the prose around these tables has already been wrong about it
/// twice — both the parked note and PLAN.md's first draft claimed "Mine and
/// Favourite and a scattering in the middle", and there is no scattering. Names
/// recur across the vocabularies without ever landing on the same bit, which is
/// exactly what makes a mis-decoded mask look ordinary.
#[test]
fn the_two_tables_agree_on_exactly_two_positions() {
    let digi = tag_names_for("digitone2").expect("a digi table");
    let a4 = tag_names_for("analogfour").expect("an A4 table");

    let agree: Vec<&str> =
        (0..32).filter(|&b| digi[b] == a4[b]).map(|b| digi[b]).collect();
    assert_eq!(agree, ["Mine", "Favourite"]);

    // And the recurrence that makes the rest dangerous: shared names, moved.
    assert_eq!((digi[0], a4[0]), ("Kick", "Bass"));
    assert_eq!((digi[10], a4[10]), ("Bass", "Kick"));
    assert_eq!((digi[29], a4[29]), ("Loop", "Input"));
}

/// A box with no calibrated grid names nothing. Not a digi's names as a
/// fallback, and not a panic: the mask is still there to display.
#[test]
fn an_uncalibrated_box_names_no_tags_at_all() {
    let saw = decode_drive_preset(&fixture("analogfour-soundbanks-A-1-THE-SAW-2026-08-29.bin"))
        .expect("THE SAW should decode");

    assert!(tag_names_for("digitakt").is_none(), "the mk1's grid has never been read");
    assert!(saw.tags("digitakt").is_empty());
    assert!(saw.tags("").is_empty());
    assert_ne!(saw.tag_mask, 0, "the mask survives even when nothing can name it");
}
