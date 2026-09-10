//! Reading a Syntakt pattern. **Read-only, and only what has been measured.**
//!
//! Every offset and every rule below was established by controlled capture
//! pairs against a real Syntakt on 2026-09-10 — one edit, one variable, a dump
//! either side — on top of the eight donated pairs already in the browser
//! repo. The evidence and the working are in
//! `dumps/syntakt-2026-09-10/stride-H01/README.md`.
//!
//! # This box is shaped like the Analog Four, not like the digis
//!
//! The lane offsets inside a track block are **identical** to
//! [`crate::a4_pattern`]'s: trig at 0, note at 128, velocity at 192, length at
//! 256, micro at 320, `0xFF` for "no lock". What differs is the track stride
//! (983 here against the A4's 751) and where the per-track defaults sit (960
//! here against 448).
//!
//! That is why this module is modelled on `a4_pattern` rather than on
//! [`crate::pattern`], and it is worth knowing before adding anything: a
//! question about this format is more likely to be answered by the A4's notes
//! than by a digi's.
//!
//! # No encoder, and no write path
//!
//! There is deliberately nothing here that builds a pattern. Writing needs a
//! byte-exact round trip and a firmware allowlist entry, and this box has
//! neither; the decode below is honest about a handful of fields and silent
//! about everything else in the 983 bytes.

/// Blocks in the pattern region. The Syntakt shows twelve tracks and an FX
/// track, which is thirteen — but **the count is what was measured, not what
/// the panel implies.** The earlier notes were right to say the thirteenth
/// block's role is unresolved, and nothing here assumes block 13 is a track
/// anyone would want notes from.
pub const NUM_BLOCKS: usize = 13;
/// Steps a block addresses. Sixty-four two-byte trig words fill the 128 bytes
/// before the first lane.
pub const NUM_STEPS: usize = 64;
/// Where the first block starts. The four bytes before it are the struct
/// version, big-endian.
pub const BLOCK_BASE: usize = 4;
/// Bytes from one block to the next.
pub const BLOCK_STRIDE: usize = 983;
/// How much of a dump the pattern occupies. `0x60` continues into the kit here
/// and `0x61` pads to its own length with zeros, so a decoder needs this many
/// bytes and must not care which request produced them.
pub const PATTERN_BYTES: usize = 23_552;
/// The struct version every capture carries at offset 0, big-endian.
pub const STRUCT_VERSION: u32 = 11;

/// Block-relative offset of the 64 two-byte trig words.
pub const TRIG_LANE: usize = 0;
/// Block-relative offset of the note lane, one raw MIDI byte per step.
pub const NOTE_LANE: usize = 128;
/// Block-relative offset of the velocity lane, one raw byte per step.
pub const VELOCITY_LANE: usize = 192;
/// Block-relative offset of the length lane, on the gen-2 length scale.
pub const LENGTH_LANE: usize = 256;
/// Block-relative offset of the micro-timing lane, one **signed** byte.
pub const MICRO_LANE: usize = 320;

/// Block-relative offset of the track's default note.
pub const DEFAULT_NOTE: usize = 960;
/// Block-relative offset of the track's default velocity.
pub const DEFAULT_VELOCITY: usize = 961;
/// Block-relative offset of the track's default length.
pub const DEFAULT_LENGTH: usize = 962;

/// "No lock — take the track default", in the note, velocity and length lanes.
///
/// **Not in the micro lane.** That byte is signed and has no sentinel, so an
/// `0xFF` there is −1 and a decoder that treats it as "unset" loses a real
/// nudge. The earlier notes carried that warning without a reason; the reason
/// is the sign.
pub const NO_LOCK: u8 = 0xFF;

/// Trig word byte 1, bit 0: this step plays a note.
///
/// Measured across a whole pattern: every step the box lit read `81` or `91` in
/// this byte and every step it did not read `00` or `10`. The same bit the A4
/// calls `TRIG_NOTE`.
pub const TRIG_PLAYS_NOTE: u8 = 0x01;
/// Trig word byte 1, bit 4: **positional, not state**.
///
/// An untouched pattern reads `00 00` on odd steps and `00 10` on even ones,
/// and a trig placed on an even step reads `91` where an odd one reads `81`.
/// It carries no per-step information — which is why the box ORs a trig in
/// rather than assigning it, exactly as the A4 does.
pub const TRIG_POSITIONAL: u8 = 0x10;

/// Micro-timing ticks in one step. Measured: the box displayed ±1/48 of a bar,
/// which is a third of a step, for a lane byte of ∓8.
pub const MICRO_TICKS_PER_STEP: i32 = 24;

/// One trig's worth of note, as the lanes hold it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyntaktNote {
    /// Zero-based step within the block.
    pub step: usize,
    /// MIDI note number, raw. The track default when the lane held [`NO_LOCK`].
    pub note: u8,
    /// Raw, as displayed. The track default when the lane held [`NO_LOCK`].
    pub velocity: u8,
    /// The length lane's byte. Convert with
    /// [`crate::pattern::length_byte_to_steps`] — this box shares that scale,
    /// confirmed at 1/32, 1/16 and 1/8.
    pub length_byte: u8,
    /// Signed micro-timing, in [`MICRO_TICKS_PER_STEP`]ths of a step.
    pub micro_ticks: i8,
    /// Whether each of the three lockable lanes carried a lock, rather than
    /// falling back to the track's default. Kept because "this trig is at the
    /// default" and "this trig is locked to the same value as the default" are
    /// different facts, and only one of them survives an edit to the default.
    pub locked: Locks,
}

/// Which lanes held a value of their own on a trig.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Locks {
    pub note: bool,
    pub velocity: bool,
    pub length: bool,
}

/// The struct version at offset 0, or `None` if the slice is too short.
pub fn struct_version(payload: &[u8]) -> Option<u32> {
    payload
        .get(0..4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Whether a payload is long enough and announces the version every capture so
/// far has carried. **A guard, not a parser**: nothing here can tell a Syntakt
/// pattern from anything else that happens to start with an 11.
pub fn looks_like_pattern(payload: &[u8]) -> bool {
    payload.len() >= PATTERN_BYTES && struct_version(payload) == Some(STRUCT_VERSION)
}

fn block(track: usize) -> usize {
    BLOCK_BASE + BLOCK_STRIDE * track
}

/// The two trig-word bytes for a step, `None` when the indices are off the end.
pub fn trig_word(payload: &[u8], track: usize, step: usize) -> Option<(u8, u8)> {
    if track >= NUM_BLOCKS || step >= NUM_STEPS {
        return None;
    }
    let at = block(track) + TRIG_LANE + step * 2;
    Some((*payload.get(at)?, *payload.get(at + 1)?))
}

/// Whether a step plays a note. See [`TRIG_PLAYS_NOTE`].
pub fn plays_note(payload: &[u8], track: usize, step: usize) -> bool {
    trig_word(payload, track, step).is_some_and(|(_, b1)| b1 & TRIG_PLAYS_NOTE != 0)
}

fn lane(payload: &[u8], track: usize, offset: usize, step: usize) -> Option<u8> {
    payload.get(block(track) + offset + step).copied()
}

/// The track's default note, velocity and length bytes.
pub fn defaults(payload: &[u8], track: usize) -> Option<(u8, u8, u8)> {
    if track >= NUM_BLOCKS {
        return None;
    }
    let b = block(track);
    Some((
        *payload.get(b + DEFAULT_NOTE)?,
        *payload.get(b + DEFAULT_VELOCITY)?,
        *payload.get(b + DEFAULT_LENGTH)?,
    ))
}

/// Every note on one block, in step order.
///
/// A lane holding [`NO_LOCK`] resolves to the track's default, which is what
/// the box itself displays for such a step. Micro is taken as-is: it is signed
/// and has no unset state.
pub fn track_notes(payload: &[u8], track: usize) -> Vec<SyntaktNote> {
    let Some((default_note, default_velocity, default_length)) = defaults(payload, track) else {
        return Vec::new();
    };
    (0..NUM_STEPS)
        .filter(|&step| plays_note(payload, track, step))
        .filter_map(|step| {
            let note_byte = lane(payload, track, NOTE_LANE, step)?;
            let velocity_byte = lane(payload, track, VELOCITY_LANE, step)?;
            let length_byte = lane(payload, track, LENGTH_LANE, step)?;
            let micro_byte = lane(payload, track, MICRO_LANE, step)?;
            Some(SyntaktNote {
                step,
                note: if note_byte == NO_LOCK { default_note } else { note_byte },
                velocity: if velocity_byte == NO_LOCK { default_velocity } else { velocity_byte },
                length_byte: if length_byte == NO_LOCK { default_length } else { length_byte },
                micro_ticks: micro_byte as i8,
                locked: Locks {
                    note: note_byte != NO_LOCK,
                    velocity: velocity_byte != NO_LOCK,
                    length: length_byte != NO_LOCK,
                },
            })
        })
        .collect()
}

/// How many steps on a block play a note.
pub fn trig_count(payload: &[u8], track: usize) -> usize {
    (0..NUM_STEPS).filter(|&s| plays_note(payload, track, s)).count()
}

// --- Pattern-level fields ----------------------------------------------------

/// Where the tempo sits, as a big-endian 32-bit value.
///
/// Only the low half has ever moved — 65535/120 is 546 BPM, so the top half has
/// nothing to say — but this is the offset the field was first measured at and
/// a 16-bit read at 23201 would agree with every capture so far.
pub const TEMPO: usize = 23_199;
/// Units of the tempo field to one BPM. Measured at two points: the box showed
/// 130.0 for 15600 and 100.0 for 12000.
///
/// A twelfth of the 0.1 the screen shows, which is what makes the display's
/// decimal place representable.
pub const TEMPO_UNITS_PER_BPM: u32 = 120;

/// Where the pattern's length sits, as a raw step count.
pub const PATTERN_LENGTH: usize = 23_204;

/// Where swing sits: **the offset from straight, not the percentage.**
pub const SWING: usize = 23_207;
/// What a swing byte of zero means. The digis store swing the same way.
pub const SWING_STRAIGHT_PERCENT: u8 = 50;

/// The pattern's tempo in BPM.
pub fn tempo_bpm(payload: &[u8]) -> Option<f64> {
    let b = payload.get(TEMPO..TEMPO + 4)?;
    let raw = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    Some(f64::from(raw) / f64::from(TEMPO_UNITS_PER_BPM))
}

/// The pattern's swing as the box shows it, in percent.
pub fn swing_percent(payload: &[u8]) -> Option<u8> {
    Some(SWING_STRAIGHT_PERCENT + payload.get(SWING)?)
}

/// How many steps the pattern runs before it wraps.
pub fn pattern_length_steps(payload: &[u8]) -> Option<u8> {
    payload.get(PATTERN_LENGTH).copied()
}
