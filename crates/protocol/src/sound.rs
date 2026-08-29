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

/// Offsets within one sound struct. Fixed across every version seen so far —
/// the struct grows at the tail (DT2 kit v3 341 bytes → v4 1109), not the head.
pub const SOUND_VERSION_OFFSET: usize = 4;
pub const SOUND_TAG_MASK_OFFSET: usize = 8;
pub const SOUND_NAME_OFFSET: usize = 12;

/// The 32 tags in the +Drive browser's filter grid, in bit order.
///
/// **Calibrated on 2026-08-26, on a DN2** — this doc said "unverified ordering"
/// until 2026-08-29 and by then it had been wrong for three days. The check was
/// the one this comment used to ask for: DN2 pool slot 1 `BD BRASSY KICK` reads
/// `0x04100021`, which these names decode to Kick, Percussion, Noisy, Vintage,
/// matching the box's own display bit for bit. See PLAN.md §9.
///
/// **Calibrated on the digis, and on nothing else.** The A4's masks differ in
/// character from every digi capture and have never been held against that
/// box's display, which is what `preset_scan::ScanError::BoxNotIndexable`
/// actually rests on — see that module. So these names are ground truth for a
/// DT2 and a DN2 and a guess for anything else, and a caller naming bits for a
/// third box is publishing that guess.
///
/// [`Sound::tag_mask`] remains the value to **store and compare on**, for a
/// reason calibration does not retire: PLAN.md §10.3's index keeps the raw mask
/// because a stored *label* would rot the next time this array moves, and this
/// array has moved once already.
pub const TAG_NAMES: [&str; 32] = [
    "Kick", "Snare", "Rimshot", "Clap", "Tom", "Percussion", "Hi-Hat", "Cymbal",
    "Cowbell", "Synth", "Bass", "Lead", "Pad", "Texture", "Chord", "Sound Fx",
    "Electronic", "Metallic", "Acoustic", "Atmosphere", "Noisy", "Glitch", "Hard", "Soft",
    "Dark", "Bright", "Vintage", "Epic", "Fail", "Loop", "Mine", "Favourite",
];

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

    /// The set bits of [`Sound::tag_mask`], named by [`TAG_NAMES`]. See that
    /// constant: the bit→name mapping is not yet verified against hardware.
    pub fn tags(&self) -> Vec<&'static str> {
        (0..32)
            .filter(|bit| self.tag_mask & (1 << bit) != 0)
            .map(|bit| TAG_NAMES[bit])
            .collect()
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
        assert_eq!(s.tags(), vec!["Kick", "Percussion"]);
    }

    #[test]
    fn an_untagged_unnamed_slot_is_empty_not_an_error() {
        let s = decode_sound(&built(3, 0, "", 341), 341).expect("decode");
        assert!(s.is_empty());
        assert_eq!(s.tags(), Vec::<&str>::new());
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
