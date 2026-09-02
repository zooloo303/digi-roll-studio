//! The Analog Four's **kit** dump: four track sounds, by name.
//!
//! The last thing the A4 could not do that both digis could. `ui::sync`'s
//! patch-names read asks a box what it currently calls each of its tracks'
//! sounds, and it did that through [`crate::pattern::decode_pattern_kit`] — a
//! gen-2 `Spec` walk over a combined pattern+kit dump. The A4 has no `Spec` and
//! never will, so the panel said *"Analog Four plays over MIDI but has no patch
//! names to read"* and greyed the control out.
//!
//! **That sentence was wrong in both halves.** The box has not been live-only
//! since 2026-08-31, and its kit carries the names all along — this module is
//! seventy lines of offsets, and every one of them was read out of captures
//! already on disk.
//!
//! # The layout
//!
//! ```text
//!   2,410 bytes, the reply to a 0x62 (stored) or 0x68 (working) request:
//!     +0     u32be    struct version — 11 in every capture
//!     +4     16 × u8  the kit's name, NUL-padded
//!     +20    12 × u8  unidentified; six u16be, plausibly the six track levels
//!     +32             sound 1  ┐  350 bytes each, back to back: the same
//!     +382            sound 2  │  0xBEEFBABA container the 0x53 pool-sound
//!     +732            sound 3  │  dump returns whole, and the same one
//!     +1082           sound 4  ┘  `drive::decode_drive_preset` reads off the
//!                                 +Drive. `decode_a4_sound` reads all three.
//!     +1432   978 × u8 unidentified — the FX and CV tracks' own settings live
//!                      somewhere in here, and neither has a sound to name.
//! ```
//!
//! # How this was measured, and why none of it needed a box
//!
//! **The evidence was already committed on 2026-08-31 and went unmined for a
//! day.** `0x60` returns the whole project as a 417-frame stream, and 128 of
//! those frames are kits — so the offsets below are not read off one capture,
//! they are checked against **128 independent kits from a real box**:
//!
//! * the version field is `11` in all 128;
//! * all four offsets carry the `0xBEEFBABA` head in all 128 — 512 containers,
//!   no exceptions, which is what makes the 350-byte stride a fact rather than
//!   an average;
//! * every sound's own version field is `6`, matching the `0x53` pool sound;
//! * every kit name and every sound name is printable ASCII, NUL-terminated
//!   inside its sixteen bytes.
//!
//! A layout that held on one capture and drifted on another would have shown up
//! here. The two fixtures committed with this module are two of those 128, one
//! of them the same kit the `0x62` and `0x68` requests both returned.
//!
//! # Four sounds, six tracks
//!
//! The A4 sequences six tracks and its kit holds four sounds. FX and CV are the
//! sequencer's tracks, not the kit's: they have no sound, which is a different
//! fact from an audio slot that has never been named, and
//! [`crate::pattern::KitInfo`]'s three-way answer had no room for it. That is
//! why [`A4Kit::sounds`] is four long and the caller maps it onto tracks rather
//! than this padding it out to six with empty strings — the exact
//! `sound_name: String::new()` shortcut packet E found and deleted.

use crate::pattern::{chars16, u32_be};
use crate::protocol::{
    parse_sysex, ParsedSysEx, SysExKind, DUMP_KIT, DUMP_KIT_REQUEST, FAMILY_ANALOG_FOUR,
};
use crate::sound::{decode_a4_sound, Sound, A4_SOUND_MAGIC_HEAD};

/// Every A4 kit dump is exactly this long, in all 130 captures.
pub const PAYLOAD_LEN: usize = 2_410;

/// The struct version at `+0`. `11` in all 128 kits of the project stream, and
/// checked rather than ignored: the gen-2 path keys its whole offset table off
/// the equivalent field, and a box that ever ships a version 12 kit must be
/// refused here rather than read with version 11's offsets.
pub const KIT_VERSION: u32 = 11;

pub const NAME_OFFSET: usize = 4;
pub const SOUNDS_OFFSET: usize = 32;
/// One embedded sound container, the same size as the `0x53` pool-sound dump's
/// whole payload.
pub const SOUND_SIZE: usize = 350;
/// SYN1–SYN4. The FX and CV tracks have no sound — see the module header.
pub const NUM_SOUNDS: usize = 4;

/// A stored kit, the reply to `0x62`. **`0x52` is `DUMP_KIT` on a digi too**,
/// which is why every check here is `(family, dump_type)` and never the type
/// alone — the same trap [`crate::a4_pattern::is_a4_pattern`] documents, where
/// `0x54` is a DT2's project settings and an A4's pattern.
pub const DUMP_A4_KIT: u8 = DUMP_KIT;
pub const DUMP_A4_KIT_REQUEST: u8 = DUMP_KIT_REQUEST;

/// The **working** kit — the box's edit buffer, the `0x68` reply, index ignored
/// and echoed as zero.
///
/// The `-0x10` sibling of [`DUMP_A4_KIT`], on the same rule that makes `0x5A`
/// the working pattern. During the 2026-08-31 sweep `0x68`'s reply was
/// byte-identical to stored kit 0, which is what a box sitting on kit 0 with no
/// unsaved edits should return.
pub const DUMP_A4_KIT_WORKING: u8 = 0x58;
pub const DUMP_A4_KIT_WORKING_REQUEST: u8 = 0x68;

/// A parsed gen-1 kit dump.
#[derive(Debug, Clone)]
pub struct A4Kit {
    /// The reply's index byte: the kit slot for a `0x62`, and zero for a `0x68`
    /// whatever was asked for.
    pub index: u8,
    pub version: u32,
    /// The kit's own name — `POLYTRON`, `STEPPA`. Often set, unlike a pattern's.
    pub name: String,
    /// SYN1–SYN4's sounds, in track order. Always [`NUM_SOUNDS`] long.
    pub sounds: Vec<Sound>,
}

impl A4Kit {
    /// What SYN1–SYN4 call their sounds, trimmed. `None` for a track index the
    /// kit has no sound for — FX and CV — which is the whole reason this is not
    /// a `Vec<String>` a caller indexes into.
    pub fn sound_name(&self, track_index: usize) -> Option<&str> {
        self.sounds.get(track_index).map(|s| s.name.trim())
    }
}

/// Is this parsed dump an A4 kit?
pub fn is_a4_kit(parsed: &ParsedSysEx) -> bool {
    is_a4_kit_of(parsed, DUMP_A4_KIT)
}

/// Is this parsed dump the A4's *working* kit — its edit buffer?
pub fn is_a4_working_kit(parsed: &ParsedSysEx) -> bool {
    is_a4_kit_of(parsed, DUMP_A4_KIT_WORKING)
}

fn is_a4_kit_of(parsed: &ParsedSysEx, dump_type: u8) -> bool {
    parsed.kind == SysExKind::Dump
        && parsed
            .dump
            .as_ref()
            .is_some_and(|d| d.family == FAMILY_ANALOG_FOUR && d.dump_type == dump_type)
}

/// Parse one `F0 … F7` message as a stored gen-1 kit dump.
///
/// Rejects anything whose checksum, count, payload length or struct version
/// does not hold — the same bar [`crate::a4_pattern::parse_pattern`] sets, for
/// the same reason: a capture that does not verify is not evidence of anything.
pub fn parse_kit(message: &[u8]) -> Result<A4Kit, String> {
    parse_kit_of(message, DUMP_A4_KIT)
}

/// Parse one message as the A4's **working** kit — [`DUMP_A4_KIT_WORKING`], the
/// reply to a `0x68` request. Identical but for the type byte; the payload is
/// the same 2,410 bytes.
pub fn parse_working_kit(message: &[u8]) -> Result<A4Kit, String> {
    parse_kit_of(message, DUMP_A4_KIT_WORKING)
}

fn parse_kit_of(message: &[u8], dump_type: u8) -> Result<A4Kit, String> {
    let parsed = parse_sysex(message);
    if !is_a4_kit_of(&parsed, dump_type) {
        return Err(match parsed.dump.as_ref() {
            Some(d) => format!(
                "not an A4 kit dump: family {:#04x}, type {:#04x}",
                d.family, d.dump_type
            ),
            None => format!("not an Elektron dump message ({:?})", parsed.kind),
        });
    }
    let d = parsed.dump.expect("checked by is_a4_kit_of");
    if !d.checksum_ok {
        return Err("checksum does not verify".into());
    }
    if !d.count_ok {
        return Err("byte count does not verify".into());
    }
    read_kit(d.index, &d.payload)
}

/// Read a decoded kit payload — the four sounds and the kit's name.
///
/// Separate from [`parse_kit`] so a payload that arrived some other way (a
/// project stream frame, a fixture) reads through exactly the same offsets as
/// one off the wire.
pub fn read_kit(index: u8, payload: &[u8]) -> Result<A4Kit, String> {
    if payload.len() != PAYLOAD_LEN {
        return Err(format!("payload is {} bytes, an A4 kit is {PAYLOAD_LEN}", payload.len()));
    }
    let version = u32_be(payload, 0);
    if version != KIT_VERSION {
        return Err(format!(
            "unsupported A4 kit struct version {version} — every capture is {KIT_VERSION}, and \
             reading a later one with these offsets would name the wrong bytes"
        ));
    }
    let mut sounds = Vec::with_capacity(NUM_SOUNDS);
    for n in 0..NUM_SOUNDS {
        let at = SOUNDS_OFFSET + n * SOUND_SIZE;
        let bytes = &payload[at..at + SOUND_SIZE];
        // The head magic is what says the offset arithmetic above was right,
        // which is exactly the job `decode_a4_sound`'s own head check does — so
        // the error is re-worded to name the *slot*, since at this point a bad
        // head means the stride is wrong and not that the sound is corrupt.
        let sound = decode_a4_sound(bytes, SOUND_SIZE).map_err(|e| {
            format!(
                "sound {} at +{at} does not decode ({e}) — expected the {A4_SOUND_MAGIC_HEAD:#010x} \
                 head every one of 512 captured containers carries",
                n + 1
            )
        })?;
        sounds.push(sound);
    }
    Ok(A4Kit { index, version, name: chars16(payload, NAME_OFFSET), sounds })
}
