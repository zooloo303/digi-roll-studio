//! The sound (preset) struct: the unit a kit is built out of.
//!
//! A kit holds 16 of these, and the +Drive holds hundreds. Each one is
//! self-delimiting, which is what makes it safe to read without trusting the
//! surrounding struct's arithmetic:
//!
//! ```text
//!   +0   magic head   0xBEEFBACE       u32be
//!   +4   version                       u32be   (DT2: 3, DN2: 2)
//!   +8   tag mask                      u32be   ← the Overbridge filter grid
//!   +12  name         16 bytes, NUL-terminated, Windows-1252
//!   …    machine and parameter bytes — not mapped here
//!   -4   magic foot   0xBACEF00C       u32be
//! ```
//!
//! Field layout ported from elk-herd's `structSound`
//! (`src/Elektron/Digitakt/Dump.elm`) — BSD-2-Clause, © 2017-2025 Mark
//! Lentczner. See `CREDITS.md`. The foot magic and the DN2 version are not in
//! elk-herd, which has no Digitone II: both are read off our own captures.
//!
//! # What this deliberately does not do
//!
//! It does not map the machine or parameter bytes. A kit builder assigns whole
//! sounds — it never edits one — so the bytes between the name and the foot are
//! carried verbatim and never interpreted. That is the same minimal-diff
//! discipline `safe_write` applies to a pattern (PLAN.md §7 rules 2 and 3): the
//! bytes we cannot explain are the bytes we must not rewrite.

use crate::pattern::{chars16, u32_be, KitSpec};

pub const SOUND_MAGIC_HEAD: u32 = 0xBEEF_BACE;
pub const SOUND_MAGIC_FOOT: u32 = 0xBACE_F00C;

/// The A4's head magic, one nibble off the digis' and followed by **no foot at
/// all**. The first three fields still sit where the diagram above puts them —
/// that is what all eight A4 captures decode to. See [`decode_a4_sound`].
pub const A4_SOUND_MAGIC_HEAD: u32 = 0xBEEF_BABA;

/// A Digitone **mk1** sound, as stored on a DN2's +Drive. ASCII `DN1S`, not a
/// `0xBEEF…` value at all — the only container magic on any box that is legible
/// in a hexdump.
///
/// **A DN2's library is two formats, and 388 of 1,189 presets are this one.**
/// They sit flush with the payload at byte 31 like an A4's, having no
/// [`SOUND_WRAPPER`], and they carry a foot like a digi's — so they are neither
/// existing case exactly, and [`decode_dn1_sound`] is `decode_sound` with this
/// head magic rather than a third set of rules.
///
/// **The tag vocabulary is the DN2's own**, which is the part that had to be
/// checked rather than assumed. Overbridge lists these presets in the DN2's
/// browser under the DN2's own 32-cell grid, and all eight of
/// `/soundbanks/C/1..8` decode through [`TAG_NAMES_DIGI`] to exactly the tags it
/// shows. So the box re-maps mk1 tags into its own vocabulary and no third table
/// exists. Suspecting one was reasonable and wrong: `Cowbell` set on four of the
/// first five files looked like a mis-decode and is simply what that library is
/// tagged like.
pub const DN1_SOUND_MAGIC_HEAD: u32 = 0x444E_3153;

/// Offsets within one sound struct. Fixed across every version seen so far —
/// the struct grows at the tail (DT2 kit v3 341 bytes → v4 1109), not the head.
pub const SOUND_VERSION_OFFSET: usize = 4;
pub const SOUND_TAG_MASK_OFFSET: usize = 8;
pub const SOUND_NAME_OFFSET: usize = 12;

/// The 32 tags in the +Drive browser's filter grid, in bit order, on the two
/// digis.
///
/// **Calibrated on a DN2 on 2026-08-26 and on both digis on 2026-08-29.** The
/// first pass rested on one preset — DN2 pool slot 1 `BD BRASSY KICK`, mask
/// `0x04100021`, read against the box's own screen. The second held all eight
/// committed DT2 captures and all eight DN2 ones against Overbridge 2.26.9's
/// filter grid and tag column: sixteen presets, every bit accounted for, no tag
/// shown that the mask does not carry and none carried that is not shown. The
/// DT2 had never actually been checked before that — this array claimed to be
/// "ground truth for a DT2 and a DN2" on the strength of a DN2 alone, which is
/// the same over-generalisation [`TAG_NAMES_A4`] exists to undo.
///
/// **The two digis share this table exactly.** That is measured, not assumed:
/// the DT2's and DN2's filter grids are the same 32 names in the same 32
/// positions. [`tag_names_for`] still routes them separately so a firmware that
/// splits them can be handled there rather than here.
///
/// [`Sound::tag_mask`] remains the value to **store and compare on**, for a
/// reason calibration does not retire: PLAN.md §10.3's index keeps the raw mask
/// because a stored *label* would rot the next time one of these arrays moves,
/// and one has moved once already.
pub const TAG_NAMES_DIGI: [&str; 32] = [
    "Kick", "Snare", "Rimshot", "Clap", "Tom", "Percussion", "Hi-Hat", "Cymbal",
    "Cowbell", "Synth", "Bass", "Lead", "Pad", "Texture", "Chord", "Sound Fx",
    "Electronic", "Metallic", "Acoustic", "Atmosphere", "Noisy", "Glitch", "Hard", "Soft",
    "Dark", "Bright", "Vintage", "Epic", "Fail", "Loop", "Mine", "Favourite",
];

/// The A4's tag vocabulary — the same *names*, in almost none of the same
/// *places*.
///
/// **Calibrated on 2026-08-29 against Overbridge 2.26.9's filter grid**, which
/// lays all 32 tags out in a 4×8 block that reads left-to-right, top-to-bottom
/// as bit 0 through bit 31. All eight committed A4 captures decode through this
/// array to exactly the tag list Overbridge prints beside them.
///
/// The overlap with [`TAG_NAMES_DIGI`] is small enough to be a trap rather than
/// a convenience: bit 0 is Kick on a digi and **Bass** on an A4, bit 22 is Hard
/// against **Dark**, bit 25 Bright against **Acid**. **Exactly two of the
/// thirty-two positions agree** — Mine at 30 and Favourite at 31 — and every
/// other bit means something else. Names recur across the two vocabularies
/// (Noisy, Glitch, Bass, Kick) but never in the same place, which is worse than
/// no overlap: it is what makes a mis-decoded mask read as an ordinary list of
/// tags. So a single global table was not merely imprecise, it named nearly
/// every A4 tag wrongly, and that is why [`tag_names_for`] takes a slug and
/// there is no table-less default.
///
/// **A note on how this was read, worth keeping.** Three photographs of the A4's
/// own screen got three positions wrong — bit 7 read as "STAB" (it is Strings),
/// bit 14 as "AMB" (Atmosphere), bit 25 as "ARP" (Acid) — because the A4
/// truncates its tag row at four entries, so `THE SAW` shows four tags and
/// carries six. The device's own display is the standard PLAN.md §9 sets and it
/// was not sufficient here; a desktop editor rendering the same data settled it
/// in one screenshot. Using both is what made this exact rather than
/// exact-looking.
pub const TAG_NAMES_A4: [&str; 32] = [
    "Bass", "Lead", "Pad", "Texture", "Chord", "Keys", "Brass", "Strings",
    "Transient", "Sound Fx", "Kick", "Snare", "Hi-Hat", "Percussion", "Atmosphere", "Evolving",
    "Noisy", "Glitch", "Hard", "Soft", "Expressive", "Deep", "Dark", "Bright",
    "Vintage", "Acid", "Epic", "Fail", "Tempo Sync", "Input", "Mine", "Favourite",
];

/// The tag table for a box, keyed by its identity slug, or `None` for a box
/// whose grid nobody has read.
///
/// **`None` is the whole point of this function.** Naming bit 3 "Clap" on a box
/// we have never seen a filter grid for is a guess wearing a label, and the A4
/// is the proof: it spent three days being told its low bits meant Kick and
/// Snare when they mean Bass and Lead. A caller that gets `None` should show the
/// mask, or show nothing, and must not fall back to a digi's names.
///
/// Keyed on the **identity slug** rather than on `DeviceModel::key`, because
/// every other thing in the +Drive path already is: the index files are named
/// by slug, `ui::presets` carries a slug, and `decode_drive_preset` is reached
/// from a device that has just identified itself. `params::device_kind_key` is
/// the same idea in the *audition* path and keys on `key` for the same reason —
/// that is what a p-lock lane carries. Two domains, two spellings, and this is
/// the one place the +Drive's crossing happens.
///
/// `digitakt` (the mk1) is deliberately absent: it is a known box, but its grid
/// has not been photographed, and a known box is not a calibrated one.
pub fn tag_names_for(slug: &str) -> Option<&'static [&'static str; 32]> {
    match slug {
        "digitakt2" | "digitone2" => Some(&TAG_NAMES_DIGI),
        "analogfour" => Some(&TAG_NAMES_A4),
        _ => None,
    }
}

/// The names of the set bits of `mask` on the box `slug` names, or an empty
/// list for a box with no calibrated table.
///
/// Free-standing rather than a [`Sound`] method because the caller that needs it
/// most — PLAN.md §10.3's preset index — stores a `u32` and never keeps a
/// `Sound` at all.
pub fn tag_names(mask: u32, slug: &str) -> Vec<&'static str> {
    let Some(table) = tag_names_for(slug) else {
        return Vec::new();
    };
    (0..32).filter(|bit| mask & (1u32 << bit) != 0).map(|bit| table[bit]).collect()
}
/// One decoded sound, plus the bytes it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    pub version: u32,
    /// The raw tag bitmask. Store and compare on this, not on [`Sound::tags`].
    pub tag_mask: u32,
    pub name: String,
    /// Every byte of the struct, head magic to foot magic inclusive. This is
    /// what gets written back into a kit — unmodified, because nothing here
    /// claims to understand it.
    pub bytes: Vec<u8>,
}

impl Sound {
    /// Whether this slot has ever held a sound. A never-written slot decodes
    /// cleanly — the magics are there — with an empty name and no tags.
    pub fn is_empty(&self) -> bool {
        self.name.is_empty() && self.tag_mask == 0
    }

    /// The set bits of [`Sound::tag_mask`], named by the table `slug`'s box
    /// uses, or empty for a box with no calibrated table.
    ///
    /// **The slug is not optional and cannot be defaulted**, which is the
    /// correction of 2026-08-29: this used to read one global array, and that
    /// array was a digi's. An A4 sound decoded through it came back naming
    /// mostly the wrong tags — confidently, and in the right shape. See
    /// [`tag_names_for`].
    pub fn tags(&self, slug: &str) -> Vec<&'static str> {
        tag_names(self.tag_mask, slug)
    }
}

#[derive(Debug, PartialEq)]
pub enum SoundError {
    /// The slice is shorter than the struct it is supposed to contain.
    Truncated { need: usize, got: usize },
    /// No `0xBEEFBACE` at the start — the offset arithmetic that produced this
    /// slice is wrong, so nothing after it can be trusted.
    BadHead { found: u32 },
    /// No `0xBACEF00C` at the end. The head was right and the tail was not,
    /// which means the *size* is wrong: a struct version we have mis-sized.
    BadFoot { found: u32, size: usize },
}

impl std::fmt::Display for SoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SoundError::Truncated { need, got } => {
                write!(f, "sound struct needs {need} bytes, slice has {got}")
            }
            SoundError::BadHead { found } => write!(
                f,
                "sound magic head {found:#010x} is not {SOUND_MAGIC_HEAD:#010x} — wrong offset"
            ),
            SoundError::BadFoot { found, size } => write!(
                f,
                "sound magic foot {found:#010x} is not {SOUND_MAGIC_FOOT:#010x} at size {size} \
                 — wrong struct size for this version"
            ),
        }
    }
}

impl std::error::Error for SoundError {}

/// Decode one sound from the front of `bytes`, which must be exactly `size`
/// bytes of sound struct.
///
/// Both magics are checked. Checking the foot is the point: a name and a tag
/// mask decode plausibly from almost any offset, so the head alone cannot tell
/// a correct read from a lucky one. The foot lands only if `size` is right for
/// this struct version, which is the assumption most likely to be wrong when a
/// new firmware lands.
pub fn decode_sound(bytes: &[u8], size: usize) -> Result<Sound, SoundError> {
    if bytes.len() < size {
        return Err(SoundError::Truncated { need: size, got: bytes.len() });
    }
    if size < SOUND_NAME_OFFSET + 16 + 4 {
        return Err(SoundError::Truncated { need: SOUND_NAME_OFFSET + 20, got: size });
    }
    let head = u32_be(bytes, 0);
    if head != SOUND_MAGIC_HEAD {
        return Err(SoundError::BadHead { found: head });
    }
    let foot = u32_be(bytes, size - 4);
    if foot != SOUND_MAGIC_FOOT {
        return Err(SoundError::BadFoot { found: foot, size });
    }
    Ok(Sound {
        version: u32_be(bytes, SOUND_VERSION_OFFSET),
        tag_mask: u32_be(bytes, SOUND_TAG_MASK_OFFSET),
        name: chars16(bytes, SOUND_NAME_OFFSET),
        bytes: bytes[..size].to_vec(),
    })
}

/// Decode an A4 container of exactly `size` bytes, whose extent the caller
/// already knows from the file header.
///
/// **The foot is not checked, because there is not one** — no A4 capture
/// carries [`SOUND_MAGIC_FOOT`] anywhere. That is a weaker check than
/// [`decode_sound`] performs only if you read the foot as validating the
/// *bytes*; what it actually validates is the *size*, which [`decode_sound`]
/// has to search for and this does not. `drive::decode_drive_preset` takes the
/// size from the file's own declared payload length, so it is stated rather
/// than guessed, and there is no wrong guess here for a foot to catch.
///
/// The head is still checked. It is the one thing that says the offset
/// arithmetic which produced this slice was right, and that failure mode is
/// unchanged.
pub fn decode_a4_sound(bytes: &[u8], size: usize) -> Result<Sound, SoundError> {
    if size < SOUND_NAME_OFFSET + 16 {
        return Err(SoundError::Truncated { need: SOUND_NAME_OFFSET + 16, got: size });
    }
    if bytes.len() < size {
        return Err(SoundError::Truncated { need: size, got: bytes.len() });
    }
    let head = u32_be(bytes, 0);
    if head != A4_SOUND_MAGIC_HEAD {
        return Err(SoundError::BadHead { found: head });
    }
    Ok(Sound {
        version: u32_be(bytes, SOUND_VERSION_OFFSET),
        tag_mask: u32_be(bytes, SOUND_TAG_MASK_OFFSET),
        name: chars16(bytes, SOUND_NAME_OFFSET),
        bytes: bytes[..size].to_vec(),
    })
}

/// Decode a Digitone mk1 container of exactly `size` bytes.
///
/// [`decode_sound`] with a different head magic, and deliberately not a
/// generalisation of it: the head magic is what says which *box's* rules apply,
/// so a decoder that took it as an argument would let a caller decode a digi
/// file as an mk1 one by passing the wrong constant. The foot is still checked
/// — these files have one, at 329 in every capture — so this gives up none of
/// the size validation [`decode_a4_sound`] has to.
pub fn decode_dn1_sound(bytes: &[u8], size: usize) -> Result<Sound, SoundError> {
    if bytes.len() < size {
        return Err(SoundError::Truncated { need: size, got: bytes.len() });
    }
    if size < SOUND_NAME_OFFSET + 16 + 4 {
        return Err(SoundError::Truncated { need: SOUND_NAME_OFFSET + 20, got: size });
    }
    let head = u32_be(bytes, 0);
    if head != DN1_SOUND_MAGIC_HEAD {
        return Err(SoundError::BadHead { found: head });
    }
    let foot = u32_be(bytes, size - 4);
    if foot != SOUND_MAGIC_FOOT {
        return Err(SoundError::BadFoot { found: foot, size });
    }
    Ok(Sound {
        version: u32_be(bytes, SOUND_VERSION_OFFSET),
        tag_mask: u32_be(bytes, SOUND_TAG_MASK_OFFSET),
        name: chars16(bytes, SOUND_NAME_OFFSET),
        bytes: bytes[..size].to_vec(),
    })
}

/// Every sound-struct size we have mapped, for the cases where the size is not
/// known in advance. A standalone `0x53` sound dump carries one struct and no
/// `KitSpec` to size it by, so the size has to be recovered from the bytes.
pub const KNOWN_SOUND_SIZES: [usize; 3] = [341, 359, 1109];

/// The bytes a `0x6b` kit-track-sound payload carries *before* the struct.
///
/// Measured on a DT2 and a DN2 on 2026-08-26: the payload is this wrapper and
/// then one whole sound struct, head magic onward. Not interpreted — like the
/// machine bytes above, it is carried verbatim, and it is named here only so
/// that the offset appears once rather than as a `5` in every caller.
pub const SOUND_WRAPPER: usize = 5;

/// Decode a standalone sound dump (`0x53`) payload, whose struct size is not
/// known up front.
///
/// Tries the payload's own length first — the box is expected to send exactly
/// one struct — and then each of [`KNOWN_SOUND_SIZES`], in case the dump is
/// padded or carries a trailer. The foot magic is what makes this safe to guess
/// at: a wrong size does not validate, so a size that lands is the size.
///
/// Returns the [`SoundError::BadFoot`] for the payload's own length when no
/// candidate validates, since that is the size the box implied.
pub fn decode_sound_dump(payload: &[u8]) -> Result<Sound, SoundError> {
    let head = if payload.len() >= 4 { u32_be(payload, 0) } else { 0 };
    if payload.len() < SOUND_NAME_OFFSET + 20 {
        return Err(SoundError::Truncated { need: SOUND_NAME_OFFSET + 20, got: payload.len() });
    }
    if head != SOUND_MAGIC_HEAD {
        return Err(SoundError::BadHead { found: head });
    }
    std::iter::once(payload.len())
        .chain(KNOWN_SOUND_SIZES)
        .filter(|&size| size >= SOUND_NAME_OFFSET + 20 && size <= payload.len())
        .find_map(|size| decode_sound(payload, size).ok())
        .ok_or(SoundError::BadFoot {
            found: u32_be(payload, payload.len() - 4),
            size: payload.len(),
        })
}

/// Decode all 16 sounds of a kit, given the kit's base offset in a pattern-kit
/// payload and the [`KitSpec`] for its version.
///
/// A slot that fails to decode comes back as `Err` in place rather than sinking
/// the whole kit: one unreadable sound should not cost the browser the other
/// fifteen.
pub fn decode_kit_sounds(
    payload: &[u8],
    kit_base: usize,
    kit_spec: &KitSpec,
    num_tracks: usize,
) -> Vec<Result<Sound, SoundError>> {
    (0..num_tracks)
        .map(|t| {
            let off = kit_base + kit_spec.sounds_offset + t * kit_spec.sound_size;
            if off >= payload.len() {
                return Err(SoundError::Truncated { need: off + kit_spec.sound_size, got: payload.len() });
            }
            decode_sound(&payload[off..], kit_spec.sound_size)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic struct, so the offsets are checked independently of any
    /// capture: if this and the fixture tests disagree, the fixture is telling
    /// us the layout is wrong rather than the arithmetic.
    fn built(version: u32, tag_mask: u32, name: &str, size: usize) -> Vec<u8> {
        let mut b = vec![0u8; size];
        b[0..4].copy_from_slice(&SOUND_MAGIC_HEAD.to_be_bytes());
        b[4..8].copy_from_slice(&version.to_be_bytes());
        b[8..12].copy_from_slice(&tag_mask.to_be_bytes());
        b[12..12 + name.len()].copy_from_slice(name.as_bytes());
        let n = b.len();
        b[n - 4..].copy_from_slice(&SOUND_MAGIC_FOOT.to_be_bytes());
        b
    }

    #[test]
    fn decodes_version_tags_and_name() {
        let s = decode_sound(&built(3, 0b101, "BD BRASSY KICK", 341), 341).expect("decode");
        assert_eq!(s.version, 3);
        assert_eq!(s.tag_mask, 0b101);
        assert_eq!(s.name, "BD BRASSY KICK");
        assert_eq!(s.bytes.len(), 341);
        assert!(!s.is_empty());
    }

    #[test]
    fn tags_name_the_set_bits() {
        let s = decode_sound(&built(3, 0b1 | 0b100000, "K", 341), 341).expect("decode");
        assert_eq!(s.tags("digitone2"), vec!["Kick", "Percussion"]);
    }

    #[test]
    fn an_untagged_unnamed_slot_is_empty_not_an_error() {
        let s = decode_sound(&built(3, 0, "", 341), 341).expect("decode");
        assert!(s.is_empty());
        assert_eq!(s.tags("digitone2"), Vec::<&str>::new());
    }

    #[test]
    fn a_wrong_offset_is_caught_by_the_head() {
        let mut b = built(3, 0, "X", 341);
        b[0] = 0x00;
        assert!(matches!(decode_sound(&b, 341), Err(SoundError::BadHead { .. })));
    }

    /// The case the foot magic exists for: right offset, wrong size. Reading a
    /// 1109-byte v4 struct as though it were 341 finds the head and a valid
    /// name, and only the foot says it is wrong.
    #[test]
    fn a_wrong_size_is_caught_by_the_foot() {
        let b = built(4, 0, "LD OMEK BRIDGE", 1109);
        let err = decode_sound(&b, 341).expect_err("341 is the wrong size for a v4 sound");
        assert!(matches!(err, SoundError::BadFoot { size: 341, .. }));
        // …and the correct size decodes the same bytes cleanly.
        assert_eq!(decode_sound(&b, 1109).expect("decode").name, "LD OMEK BRIDGE");
    }

    #[test]
    fn a_sound_dump_recovers_its_own_size() {
        for size in KNOWN_SOUND_SIZES {
            let b = built(3, 0b1000, "SIZED", size);
            let s = decode_sound_dump(&b).unwrap_or_else(|e| panic!("size {size}: {e}"));
            assert_eq!(s.name, "SIZED");
            assert_eq!(s.bytes.len(), size);
        }
    }

    /// A padded dump: the payload is longer than the struct, so the payload's
    /// own length fails the foot check and a known size has to rescue it.
    #[test]
    fn a_padded_sound_dump_falls_back_to_a_known_size() {
        let mut b = built(3, 0, "PADDED", 341);
        b.extend_from_slice(&[0u8; 7]);
        let s = decode_sound_dump(&b).expect("a known size should validate");
        assert_eq!(s.bytes.len(), 341);
        assert_eq!(s.name, "PADDED");
    }

    #[test]
    fn a_sound_dump_of_an_unmapped_size_is_refused_not_guessed() {
        // Right head, right-looking name, foot in a place no known size predicts.
        let mut b = built(9, 0, "FUTURE", 700);
        b[696..700].copy_from_slice(&[0, 0, 0, 0]);
        b[700 - 4..].copy_from_slice(&[0, 0, 0, 0]);
        assert!(matches!(decode_sound_dump(&b), Err(SoundError::BadFoot { .. })));
    }

    #[test]
    fn a_short_slice_is_truncated_not_a_panic() {
        let b = built(3, 0, "X", 341);
        assert_eq!(
            decode_sound(&b[..100], 341),
            Err(SoundError::Truncated { need: 341, got: 100 })
        );
    }
}
