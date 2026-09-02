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
    build_dump_message, parse_sysex, ParsedSysEx, SysExKind, DUMP_KIT, DUMP_KIT_REQUEST,
    FAMILY_ANALOG_FOUR,
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

// --- Putting a sound on a track ----------------------------------------------
//
// **The A4's answer to `0x6b`/`0x5b`, and it is a different shape.** A digi
// addresses one kit track's sound directly: a read returns that track's bytes
// and a store puts new bytes back, and `midi::preset_load` is five round trips
// on top of that pair. This box has no such message — its `0x6b` is the working
// *pattern* — so the unit that moves is the whole kit, and a load becomes:
// fetch the working kit, replace 350 of its 2,410 bytes, send it back.
//
// That is a **read-modify-write, and the read is not optional**. Sending a kit
// built from anything other than the box's own current bytes would overwrite
// the three sounds nobody mentioned along with the one that was asked for, plus
// the kit's name and the FX and CV tracks' settings. [`splice_sound`] therefore
// takes a payload rather than building one, and touches exactly its slot's
// stride — the same minimal-diff discipline `safe_write` applies to a pattern
// (PLAN.md §7 rules 2 and 3), reached from the other end: here the bytes we
// cannot explain are 2,060 of the 2,410, and every one of them is carried
// through untouched.

use crate::sound::A4_SOUND_MAGIC_FOOT;

/// The struct version a kit slot's sound has to be, and **the box enforces this
/// itself in the worst possible way**.
///
/// All 512 kit-embedded containers of the project stream are version 6. Every
/// +Drive preset file on this box is version **5** — the factory banks predate
/// the OS these kits were written by. Handed a version-5 sound in a kit slot,
/// the A4 does not refuse the kit and does not convert the sound: it stores the
/// kit and replaces that slot with an **init sound named `SOUND n`**, which is
/// silent failure with the user's track gone
/// (`examples/a4_sound_convert_probe`, 2026-09-01: two different presets left
/// the slot byte-identical, which is the reading that separates a refusal from a
/// conversion).
///
/// So a load has to hand the box version 6. [`sound_for_kit`] is how, and
/// [`splice_sound`] refuses anything else rather than sending a kit that would
/// come back hollowed out.
pub const KIT_SOUND_VERSION: u32 = 6;

/// The one byte that differs between a version-5 container and the version-6
/// rendering of the same sound: **184 in every version 5, 0 in every version 6.**
///
/// Unnamed on purpose — this crate does not map the parameter bytes, and what is
/// known about this one is exactly what was measured. What it is *not* is a
/// parameter: 28 different version-5 files all carry 184 here and 28 different
/// version-6 pool sounds all carry 0, and a parameter would vary across 28
/// sounds.
pub const V5_ONLY_BYTE: usize = 235;

/// The version-5 value of [`V5_ONLY_BYTE`].
pub const V5_ONLY_VALUE: u8 = 184;

/// One +Drive sound container as a kit slot's sound: **version 6, and the box's
/// own two-byte conversion applied.**
///
/// # This is measured off the box and not derived
///
/// The A4's project sound pool holds 128 version-6 sounds and its +Drive holds
/// version-5 files, and 28 of them share a name — the same sound in both
/// versions, which is the pair needed to see what the conversion is with nobody
/// guessing (`examples/a4_sound_pool_probe`, 2026-09-01). Across all 28:
///
/// * [`V5_ONLY_BYTE`] is [`V5_ONLY_VALUE`] in the file and 0 in the pool, every
///   time;
/// * the version word is 5 in the file and 6 in the pool, every time;
/// * **and one pair differs in nothing else at all** — `DUAL OSCS`, where the
///   box's own version-6 sound is byte-for-byte the file with those two fields
///   changed.
///
/// That last one is what makes this a conversion rather than a correlation. Any
/// other byte belonging to a universal rule would have to differ in *that* pair
/// too, and none does; the bytes that differ in the other 27 differ because
/// somebody edited the pool copy — byte 126 moves in both directions across the
/// set, which no version rule does.
///
/// # What is refused
///
/// A version this build has not measured. Version 6 passes through unaltered —
/// a sound off a kit or out of the pool needs nothing done to it — version 5 is
/// converted, and anything else is an error rather than a guess sent to
/// hardware. Getting this wrong does not fail loudly: it leaves the user's track
/// holding an init sound (see [`KIT_SOUND_VERSION`]).
pub fn sound_for_kit(sound: &[u8]) -> Result<Vec<u8>, String> {
    if sound.len() != SOUND_SIZE {
        return Err(format!(
            "a kit slot holds {SOUND_SIZE} bytes and this sound is {}",
            sound.len()
        ));
    }
    let version = u32_be(sound, 4);
    let mut out = sound.to_vec();
    match version {
        KIT_SOUND_VERSION => {}
        5 => {
            out[4..8].copy_from_slice(&KIT_SOUND_VERSION.to_be_bytes());
            out[V5_ONLY_BYTE] = 0;
        }
        other => {
            return Err(format!(
                "this sound is struct version {other}, and the only conversion measured on \
                 hardware is version 5 to version {KIT_SOUND_VERSION} — refusing to guess at \
                 one, because a version this box does not want is not refused by it: it \
                 silently replaces the track with an init sound"
            ))
        }
    }
    Ok(out)
}

/// Where slot `slot`'s sound starts in a kit payload.
pub fn sound_offset(slot: usize) -> usize {
    SOUNDS_OFFSET + slot * SOUND_SIZE
}

/// One slot's 350 bytes out of a kit payload, unaltered.
///
/// **This is the backup a load takes, and it costs nothing extra**: a load has
/// to fetch the kit before it can splice into it, so the bytes it is about to
/// displace are already in hand. The same argument `midi::preset_load`'s module
/// doc makes for the digis' pre-read, on a box where the pre-read is the whole
/// kit rather than one track.
pub fn sound_slot(payload: &[u8], slot: usize) -> Result<&[u8], String> {
    check_kit(payload)?;
    check_slot(slot)?;
    let at = sound_offset(slot);
    Ok(&payload[at..at + SOUND_SIZE])
}

/// A kit payload with slot `slot`'s sound replaced by `sound`, and every other
/// byte carried through.
///
/// `sound` is a whole [`SOUND_SIZE`]-byte container — head magic, foot magic and
/// the parameters between them — as `drive::a4_preset_sound` cuts one out of a
/// +Drive preset file, or as [`sound_slot`] returns one for a revert.
///
/// # What is checked, and why the head and foot are both
///
/// The destination is checked as a kit (length and struct version) and the slot
/// against [`NUM_SOUNDS`]. The incoming bytes are checked at **both ends**:
/// [`A4_SOUND_MAGIC_HEAD`] says these are an A4 sound container rather than a
/// digi's or an mk1's, and [`A4_SOUND_MAGIC_FOOT`] at `SOUND_SIZE - 4` says the
/// slice is the *right length* — a 366-byte file payload sliced to 350 lands its
/// foot exactly there, and one sliced anywhere else does not. Sending a kit
/// whose slot boundaries have shifted by sixteen bytes is the failure mode this
/// pair rules out, and `DEVELOPMENT.md` lesson 13 is what it would cost: a body
/// this box cannot parse takes its SysEx API down until it is power-cycled.
///
/// **The struct version is not checked, and that is a decision.** A kit's four
/// embedded sounds are version 6 in all 512 captured containers and every
/// +Drive preset file on this box is version 5 — the factory banks predate the
/// OS the kits were written by. Refusing the mismatch would refuse the entire
/// library, and rewriting the field would be inventing a struct nobody has
/// captured; the box's own browser loads these files onto its tracks, so the box
/// reads version 5 in a kit slot. What this crate must not do is *claim* that
/// from arithmetic — see `PLAN.md` §10.7 for what the hardware said when asked.
pub fn splice_sound(payload: &[u8], slot: usize, sound: &[u8]) -> Result<Vec<u8>, String> {
    check_kit(payload)?;
    check_slot(slot)?;
    if sound.len() != SOUND_SIZE {
        return Err(format!(
            "a kit slot holds {SOUND_SIZE} bytes and this sound is {} — refusing to splice a \
             length that would move the slots after it",
            sound.len()
        ));
    }
    let head = u32_be(sound, 0);
    if head != A4_SOUND_MAGIC_HEAD {
        return Err(format!(
            "these bytes open {head:#010x} and an A4 sound container opens \
             {A4_SOUND_MAGIC_HEAD:#010x} — this is not one of this box's sounds"
        ));
    }
    let foot = u32_be(sound, SOUND_SIZE - 4);
    if foot != A4_SOUND_MAGIC_FOOT {
        return Err(format!(
            "these {SOUND_SIZE} bytes end {foot:#010x} and every captured A4 sound ends \
             {A4_SOUND_MAGIC_FOOT:#010x} — the slice is the wrong length, so splicing it \
             would shift the slot boundaries"
        ));
    }
    // **The refusal that costs a track if it is missing.** A version this box
    // does not want is not rejected by it — it stores the kit and puts an init
    // sound in the slot. So the check happens here, where a caller can still
    // fix it with `sound_for_kit`, rather than on the box where it looks like a
    // load that quietly did the wrong thing.
    let version = u32_be(sound, 4);
    if version != KIT_SOUND_VERSION {
        return Err(format!(
            "this sound is struct version {version} and a kit slot takes version \
             {KIT_SOUND_VERSION} — pass it through `sound_for_kit` first. Sent as it is, the \
             box would store the kit and replace the track with an init sound"
        ));
    }
    let mut out = payload.to_vec();
    let at = sound_offset(slot);
    out[at..at + SOUND_SIZE].copy_from_slice(sound);
    Ok(out)
}

/// Frame a kit payload as a sendable **working-kit** dump — the `0x58` a `0x68`
/// request is answered with.
///
/// The edit buffer, deliberately, and not [`DUMP_A4_KIT`]'s stored slot. A load
/// changes the kit the box is playing and the box's own undo is reloading the
/// pattern, which discards it — the same recovery story
/// `midi::preset_load` documents for a digi's active kit, and the reason
/// neither path can be [`crate::safe_write`]'s ceremony. Nothing in this crate
/// builds a `0x52`: a store into a saved kit slot is a write nobody has asked
/// for and it would have no undo at all.
///
/// **Refuses a payload whose ragged final group is not seven-bit clean**, which
/// is [`crate::a4_pattern::build_pattern`]'s refusal for the same reason: 2,410
/// is 344 × 7 + 2, so the last group carries two bytes, and both candidate
/// short-group bit orders encode a group with no high bit set identically. Every
/// capture is clean there, so nothing is lost — and a payload that is not would
/// be a guess sent to hardware.
///
/// The framing is [`build_dump_message`] unmodified and reproduces the box's own
/// `0x58` message byte for byte; the round-trip test is what says so.
pub fn build_working_kit(payload: &[u8]) -> Result<Vec<u8>, String> {
    check_kit(payload)?;
    let tail = &payload[payload.len() - payload.len() % 7..];
    if tail.iter().any(|b| b & 0x80 != 0) {
        return Err(format!(
            "last seven-bit group {} has a high bit set; short-group bit order is \
             unmeasured, refusing to encode",
            tail.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
        ));
    }
    Ok(build_dump_message(FAMILY_ANALOG_FOUR, DUMP_A4_KIT_WORKING, 0, payload))
}

/// The destination checks every splice makes: this is a kit payload of the
/// version these offsets were read against.
///
/// The same two refusals [`read_kit`] makes, and made here as well rather than
/// by decoding: a splice does not need the four sounds parsed, and a kit holding
/// one container this build cannot read must still be able to have a *different*
/// slot written — the box's bytes are the box's, and refusing to touch a kit
/// because slot 3 is unfamiliar would be this app's opinion getting in the way.
fn check_kit(payload: &[u8]) -> Result<(), String> {
    if payload.len() != PAYLOAD_LEN {
        return Err(format!("payload is {} bytes, an A4 kit is {PAYLOAD_LEN}", payload.len()));
    }
    let version = u32_be(payload, 0);
    if version != KIT_VERSION {
        return Err(format!(
            "unsupported A4 kit struct version {version} — every capture is {KIT_VERSION}, and \
             writing one of those with these offsets would put a sound in the wrong place"
        ));
    }
    Ok(())
}

fn check_slot(slot: usize) -> Result<(), String> {
    if slot >= NUM_SOUNDS {
        return Err(format!(
            "an A4 kit holds {NUM_SOUNDS} sounds — SYN1 to SYN4 — and this is slot {}. The FX \
             and CV tracks sequence, and have no sound to put one on",
            slot + 1
        ));
    }
    Ok(())
}
