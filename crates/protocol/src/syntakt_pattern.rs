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
//! # Editing in place, and still no encoder
//!
//! There is deliberately nothing here that *builds* a pattern from scratch, and
//! there never will be: the decode below is honest about a handful of fields and
//! silent about everything else in the 983 bytes, so anything this crate
//! assembled whole would be mostly invention.
//!
//! What it does have is the read-modify-write half — [`set_track_notes`],
//! [`set_step`] and [`locks_for`], which edit a dump **fetched from the box it
//! is going back to** and leave every untouched byte exactly as it arrived.
//! That is the same bargain `a4_pattern` strikes, and it is what makes a write
//! minimal-diff rather than a re-synthesis.
//!
//! **This section said "no write path" and that the box had neither a byte-exact
//! round trip nor a firmware allowlist entry, until 2026-09-16.** All three had
//! been false since 2026-09-11: the round trip is what `syntakt_transfer`'s
//! fixtures assert, the allowlist carries `("syntakt", ["0082"])`, and the three
//! functions above are in this file. DEVELOPMENT.md §17 — and a header is where
//! it hides longest, because no test reads one.

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

/// What a trig-on sets in trig word byte 0. Measured: every lit step in the
/// captured pattern carries these bits and no unlit one does.
pub const TRIG_ON_BYTE0: u8 = 0x03;
/// What a trig-on sets in trig word byte 1.
///
/// **Set, not assigned.** An empty even step already carries
/// [`TRIG_POSITIONAL`], and a trig placed on one reads `91` where the same trig
/// on an odd step reads `81` — so the box ORs, and so does this module.
pub const TRIG_ON_BYTE1: u8 = 0x81;

/// What every lane but micro holds on a step with no trig. Measured across 730
/// such steps with no exception.
pub const EMPTY_LANE: u8 = NO_LOCK;
/// What the micro lane holds on a step with no trig. Zero, not [`NO_LOCK`],
/// because that lane is signed.
pub const EMPTY_MICRO: u8 = 0x00;

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
    /// The condition lane's byte, raw. [`NO_LOCK`] where the step has none.
    /// Kept raw rather than decoded so a note can be written back unchanged
    /// even where [`condition`] would not name it.
    pub condition_byte: u8,
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

/// Whether a step holds nothing at all.
///
/// Measured: an empty step's word reads `00 00`, except that odd-numbered steps
/// carry [`TRIG_POSITIONAL`] in the second byte and read `00 10`. The DN2's
/// published format map calls that bit a structural marker for the step's
/// position, which agrees with every capture here.
///
/// **What the gap between this and [`plays_note`] does not tell you.** This
/// said, until 2026-09-15, that a step which is not empty and plays no note
/// holds a trig with parameter locks and no note. A beta test on hardware
/// showed otherwise: B01's T1 step 11 read `03 80` — trig bits set, note bit
/// clear — with no pool lane for T1 and **nothing lit on the box**. That is
/// residue, and a count built on this gap reported it as a trig.
///
/// So this is only a statement about the word's shape. Evidence that a step
/// carries a parameter-only trig comes from the pool — see [`plock_lanes`] —
/// not from here. What this box writes for a real one is still unmeasured; the
/// DN2 map's `0x78` is not what this residue looks like.
///
/// [`set_step`] still leaves such a step alone, and that rule stands on its own:
/// residue kept is harmless, and a real lock trig kept is the point.
pub fn step_is_empty(payload: &[u8], track: usize, step: usize) -> bool {
    match trig_word(payload, track, step) {
        None => true,
        Some((b0, b1)) => b0 == 0 && b1 & !TRIG_POSITIONAL == 0,
    }
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
                condition_byte: lane(payload, track, CONDITION_LANE, step)?,
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

// --- The p-lock pool ------------------------------------------------------------

/// Where the pool starts: 80 records of [`PLOCK_RECORD_BYTES`], ending at 23183.
///
/// Measured on 2026-09-10 and written up in
/// `dumps/syntakt-2026-09-10/stride-H01/README.md`, including the one way this
/// box differs from the digis — it keeps records sorted by paramId and shifts
/// to make room, where the digis take the lowest free record.
pub const PLOCK_POOL: usize = 12_783;
/// How many records the pool holds.
pub const PLOCK_RECORDS: usize = 80;
/// One record: a two-byte `(paramId, track)` header, then 64 big-endian 16-bit
/// values, one per step.
pub const PLOCK_RECORD_BYTES: usize = 130;
/// A free record's header. Its 128 value bytes are zero, not `FFFF`.
pub const PLOCK_FREE: [u8; 2] = [0xFF, 0xFF];
/// A step with no lock, inside a record that is in use.
pub const PLOCK_NO_VALUE: u16 = 0xFFFF;

/// One record in use: which parameter, on which block, locked on which steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlockLane {
    /// The box's own parameter number. Only `29` (FLTR RESO) is named.
    pub param_id: u8,
    /// Zero-based block, the same numbering as every other block function here.
    pub track: usize,
    /// Zero-based steps carrying a value, in step order.
    pub steps: Vec<usize>,
}

/// Every record in use, in the order the box keeps them.
///
/// **Read, not decoded.** A value is the display number × 256, but with one
/// paramId named out of an unknown total there is nothing to show beside it,
/// so this reports *that* a step is locked and not *to what*. That is enough
/// for what the import needs it for: to say a pattern carries automation the
/// app will not show, and to tell a real parameter-only trig from residue.
pub fn plock_lanes(payload: &[u8]) -> Vec<PlockLane> {
    let mut lanes = Vec::new();
    for record in 0..PLOCK_RECORDS {
        let at = PLOCK_POOL + record * PLOCK_RECORD_BYTES;
        let Some(bytes) = payload.get(at..at + PLOCK_RECORD_BYTES) else {
            break;
        };
        if bytes[..2] == PLOCK_FREE {
            continue;
        }
        let steps = (0..NUM_STEPS)
            .filter(|s| u16::from_be_bytes([bytes[2 + 2 * s], bytes[3 + 2 * s]]) != PLOCK_NO_VALUE)
            .collect();
        lanes.push(PlockLane { param_id: bytes[0], track: usize::from(bytes[1]), steps });
    }
    lanes
}

/// How a measured parameter's 16-bit lock value reads on the box's screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlockScale {
    /// display = raw / 256. The low byte is the fraction the box keeps when a
    /// knob is turned finely; the screen truncates it to two decimals
    /// (`0x45b0` = 69.6875 shows as 69.68).
    Unsigned,
    /// display = raw / 256 − 64: PAN, `L64` = 0, centre = 64, `R47` = 111.
    Centred,
    /// The high byte picks an entry. An empty name is an entry not captured
    /// yet, shown by its number: envelope mode 1 = ADSR was captured, and 0 is
    /// presumably AHD but was not, so it stays unnamed.
    Choice(&'static [&'static str]),
}

/// The p-lock parameter IDs measured on a Syntakt, OS 1.42 (build 0097),
/// 2026-11-22 — `dumps/syntakt-2026-11-22/plocks/README.md`. Each was locked
/// alone on a trig and read back with `0x61`. IDs not here are left unnamed.
///
/// Scope, as measured: FREQ, RESO, ATK and VOL on track 1 (SY BITS, digital)
/// *and* track 9 (SD HARD, analog), the same IDs on both; HOLD, DEC, DELAY,
/// REV and PAN on track 9 only; DEC (in both envelope modes), SUS, REL and the
/// envelope mode on track 1 only.
pub const MEASURED_PLOCKS: &[(u8, &str, PlockScale)] = &[
    (28, "FLTR FREQ", PlockScale::Unsigned),
    (29, "FLTR RESO", PlockScale::Unsigned),
    (39, "AMP ATK", PlockScale::Unsigned),
    (40, "AMP HOLD", PlockScale::Unsigned),
    (41, "AMP DEC", PlockScale::Unsigned),
    (42, "AMP SUS", PlockScale::Unsigned),
    (43, "AMP REL", PlockScale::Unsigned),
    (44, "AMP DELAY SEND", PlockScale::Unsigned),
    (45, "AMP REVERB SEND", PlockScale::Unsigned),
    (46, "AMP PAN", PlockScale::Centred),
    (47, "AMP VOL", PlockScale::Unsigned),
    (49, "AMP ENV MODE", PlockScale::Choice(&["", "ADSR"])),
];

/// The measured name and scale of a p-lock parameter ID, if it has one.
pub fn measured_plock(param_id: u8) -> Option<(&'static str, PlockScale)> {
    MEASURED_PLOCKS.iter().find(|(id, _, _)| *id == param_id).map(|(_, n, s)| (*n, *s))
}

/// What a raw lock value reads as on the screen, for a measured parameter.
/// Kept as a number for the continuous scales; the raw value stays the truth.
pub fn plock_display(scale: PlockScale, raw: u16) -> String {
    match scale {
        PlockScale::Unsigned => format!("{:.2}", (f64::from(raw) / 256.0 * 100.0).floor() / 100.0),
        PlockScale::Centred => {
            let v = (f64::from(raw) / 256.0 - 64.0).trunc() as i32;
            match v {
                0 => "C".into(),
                v if v < 0 => format!("L{}", -v),
                v => format!("R{v}"),
            }
        }
        PlockScale::Choice(names) => match names.get(usize::from(raw >> 8)) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => format!("#{}", raw >> 8),
        },
    }
}

/// Every record in use with its raw values: `(param, track, [(step, raw)])`.
pub fn plock_values(payload: &[u8]) -> Vec<(u8, usize, Vec<(usize, u16)>)> {
    let mut out = Vec::new();
    for record in 0..PLOCK_RECORDS {
        let at = PLOCK_POOL + record * PLOCK_RECORD_BYTES;
        let Some(bytes) = payload.get(at..at + PLOCK_RECORD_BYTES) else { break };
        if bytes[..2] == PLOCK_FREE {
            continue;
        }
        let values = (0..NUM_STEPS)
            .filter_map(|s| {
                let v = u16::from_be_bytes([bytes[2 + 2 * s], bytes[3 + 2 * s]]);
                (v != PLOCK_NO_VALUE).then_some((s, v))
            })
            .collect();
        out.push((bytes[0], usize::from(bytes[1]), values));
    }
    out
}

#[cfg(test)]
mod measured_plock_tests {
    use super::*;

    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../dumps/syntakt-2026-11-22/plocks");

    fn capture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{DIR}/{name}.bin")).unwrap()
    }

    /// The one record a capture adds over the previous one.
    fn lock(name: &str) -> (u8, usize, u16) {
        let lanes = plock_values(&capture(name));
        let (p, t, v) = lanes.last().unwrap().clone();
        assert_eq!(v.len(), 1, "{name}: one locked step");
        assert_eq!(v[0].0, 0, "{name}: on TRIG 1");
        (p, t, v[0].1)
    }

    #[test]
    fn every_single_lock_capture_decodes_to_its_parameter_and_raw_value() {
        // (capture, param, track (0-based), raw, what the screen showed)
        for (name, param, track, raw, shown) in [
            ("01-reso-min", 29, 0, 0x0000, "0.00"),
            ("02-reso-80", 29, 0, 0x5002, "80.00"),
            ("03-reso-127", 29, 0, 0x7f00, "127.00"),
            ("05-freq-0", 28, 0, 0x0000, "0.00"),
            ("06-freq-70", 28, 0, 0x4602, "70.00"),
            ("07-freq-127", 28, 0, 0x7f00, "127.00"),
            ("10-t9-sdhard-reso-53.27", 29, 8, 0x3546, "53.27"),
        ] {
            assert_eq!(lock(name), (param, track, raw), "{name}");
            let (_, scale) = measured_plock(param).unwrap();
            assert_eq!(plock_display(scale, raw), shown, "{name}");
        }
    }

    #[test]
    fn the_amp_page_and_the_envelope_mode() {
        let find = |name: &str, param: u8, track: usize| -> u16 {
            plock_values(&capture(name)).into_iter().find(|r| r.0 == param && r.1 == track).unwrap().2[0].1
        };
        for (name, param, track, raw, shown) in [
            ("11-t9-freq-88.08", 28, 8, 0x5815, "88.08"),
            ("12-t9-amp-atk-31", 39, 8, 0x1f00, "31.00"),
            ("13-t9-amp-hold-40", 40, 8, 0x2800, "40.00"),
            ("14-t9-amp-dec-92", 41, 8, 0x5c00, "92.00"),
            ("15-t9-amp-delay-53.96", 44, 8, 0x35f6, "53.96"),
            ("16-t9-amp-rev-69.68", 45, 8, 0x45b0, "69.68"),
            ("17-t9-amp-pan-R47", 46, 8, 0x6f00, "R47"),
            ("18-t9-amp-vol-80.21", 47, 8, 0x5037, "80.21"),
            ("19-t9-amp-pan-L64", 46, 8, 0x0000, "L64"),
            ("20-t1-sybits-amp-atk-51", 39, 0, 0x3300, "51.00"),
            ("21-t1-sybits-amp-vol-27", 47, 0, 0x1b01, "27.00"),
            ("22-t1-amp2-env-mode-adsr", 49, 0, 0x0100, "ADSR"),
            ("23-t1-amp-rel-90-adsr", 43, 0, 0x5a00, "90.00"),
            ("24-t1-amp-sus-77-adsr", 42, 0, 0x4d00, "77.00"),
            ("25-t1-amp-dec-103-adsr", 41, 0, 0x6700, "103.00"),
        ] {
            assert_eq!(find(name, param, track), raw, "{name}");
            assert_eq!(plock_display(measured_plock(param).unwrap().1, raw), shown, "{name}");
        }
    }

    #[test]
    fn records_stay_sorted_by_parameter_id() {
        let ids: Vec<u8> = plock_values(&capture("25-t1-amp-dec-103-adsr")).iter().map(|r| r.0).collect();
        assert_eq!(ids, vec![39, 41, 42, 43, 47, 49]);
        // FREQ (28) was locked after RESO (29) on track 9 and went in front.
        let ids: Vec<u8> = plock_values(&capture("11-t9-freq-88.08")).iter().map(|r| r.0).collect();
        assert_eq!(ids, vec![28, 29]);
    }

    #[test]
    fn removing_the_lock_returns_the_pattern_byte_for_byte_and_kit_edits_never_reach_it() {
        assert_eq!(capture("04-back"), capture("00-baseline"));
        assert_eq!(capture("08-back"), capture("00-baseline"));
        // A knob turned with no trig held changes the kit, not the pattern.
        assert_eq!(capture("00b-other-track-reso-kit-only"), capture("00-baseline"));
        assert_eq!(capture("08b-t9-reso-kit-only-no-trig"), capture("08-back"));
    }

    #[test]
    fn deleting_a_trig_clears_its_locks() {
        // Track 9's trig was removed alone with its TRIG key between 19 and
        // 20 (nine records went with it); track 1's by clearing the whole
        // sequence at the end (six went).
        assert!(plock_values(&capture("20-t1-sybits-amp-atk-51")).iter().all(|r| r.1 == 0));
        assert!(plock_values(&capture("26-final-cleared")).is_empty());
        // A single-trig delete leaves the step word at 0x0000; a sequence
        // clear only clears bit 0 and leaves 0x0380 — residue, not a trig.
        let t9 = capture("20-t1-sybits-amp-atk-51");
        assert_eq!(&t9[7868..7870], &[0x00, 0x00]);
        assert_eq!(&capture("26-final-cleared")[4..6], &[0x03, 0x80]);
    }

    #[test]
    fn unmeasured_ids_and_entries_have_no_name() {
        assert!(measured_plock(30).is_none());
        assert!(measured_plock(48).is_none());
        // Envelope mode 0 (presumably AHD) was not captured: shown by number.
        let (_, mode) = measured_plock(49).unwrap();
        assert_eq!(plock_display(mode, 0x0000), "#0");
        assert_eq!(plock_display(mode, 0x0100), "ADSR");
    }
}

// --- Trig conditions ---------------------------------------------------------

/// Block-relative offset of the trig-condition lane, one byte per step.
///
/// The same offset the A4 keeps its conditions at, predicted from that and then
/// measured. The **table is not the A4's**, though — see [`condition`].
pub const CONDITION_LANE: usize = 384;

/// Entries in the probability ladder, `0`..=`21`. Both ends measured: 50% reads
/// 10 and 100% reads 21.
pub const CONDITION_LOGIC_BASE: u8 = 22;
/// Where the ratios begin. Measured: `1:2` reads 32.
pub const CONDITION_RATIO_BASE: u8 = 32;

/// What a trig condition says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaktCond {
    /// Plays this often, as a percentage off Elektron's ladder.
    Probability(u8),
    /// `FILL`, `PRE`, `NEI`, `1ST`, `LST`, and their negations.
    Logic { name: &'static str, negated: bool },
    /// `A:B` — plays on pass A of every B.
    Ratio { a: u8, b: u8 },
}

/// The five logic pairs, in menu order. Each is followed by its negation, which
/// is the order every Elektron box lists them in.
///
/// **Five, where the A4 has four.** That extra pair is what makes the ratios
/// start two later here than there, and `LST` is the one the A4 does not have.
pub const CONDITION_LOGIC: [&str; 5] = ["FILL", "PRE", "NEI", "1ST", "LST"];

/// Read one condition byte. `None` for [`NO_LOCK`] and for anything past the
/// menu.
///
/// # What is measured and what is not
///
/// Four points were read off the box: `50%` → 10, `100%` → 21, `PRE` → 24 and
/// `1:2` → 32. Those fix **the shape** — where each of the three regions starts
/// and ends — and they were chosen to, one per region plus both ends of the
/// ladder.
///
/// What they do not fix is the interior. The percentages between 1 and 100 are
/// [`crate::a4_conditions::PERCENTAGES`], Elektron's ladder, which the two
/// measured points sit on exactly but which is *taken* from the A4 rather than
/// measured here. The order of the logic pairs after `PRE`, and the grouping of
/// the ratios, are the same kind of inference. A caller that needs one of those
/// exactly should measure it before trusting it.
pub fn condition(byte: u8) -> Option<SyntaktCond> {
    use crate::a4_conditions::PERCENTAGES;
    if byte == NO_LOCK {
        return None;
    }
    if byte < CONDITION_LOGIC_BASE {
        return Some(SyntaktCond::Probability(PERCENTAGES[byte as usize]));
    }
    if byte < CONDITION_RATIO_BASE {
        let i = byte - CONDITION_LOGIC_BASE;
        return Some(SyntaktCond::Logic {
            name: CONDITION_LOGIC[(i / 2) as usize],
            negated: i % 2 == 1,
        });
    }
    // Ratios grouped by denominator: 1:2, 2:2, 1:3, 2:3, 3:3, … 8:8.
    let mut n = byte - CONDITION_RATIO_BASE;
    for b in 2..=8u8 {
        if n < b {
            return Some(SyntaktCond::Ratio { a: n + 1, b });
        }
        n -= b;
    }
    None
}

/// One condition as the byte that stores it — the inverse of [`condition`].
///
/// `None` where the menu has no such entry: a probability off Elektron's
/// ladder, a logic name this box does not list, or a ratio outside `1:2`–`8:8`.
/// A caller that gets `None` has something the box cannot hold and has to say
/// so rather than round silently.
///
/// **Inherits [`condition`]'s uncertainty exactly**, because it is derived from
/// the same four measured points and the same inferences about the interior.
/// The round trip being exact proves the two agree with each other, which is a
/// weaker claim than either agreeing with the box.
pub fn condition_byte(cond: &SyntaktCond) -> Option<u8> {
    use crate::a4_conditions::PERCENTAGES;
    match cond {
        SyntaktCond::Probability(p) => {
            PERCENTAGES.iter().position(|v| v == p).map(|i| i as u8)
        }
        SyntaktCond::Logic { name, negated } => CONDITION_LOGIC
            .iter()
            .position(|n| n == name)
            .map(|i| CONDITION_LOGIC_BASE + (i as u8) * 2 + u8::from(*negated)),
        SyntaktCond::Ratio { a, b } => {
            if !(2..=8).contains(b) || *a == 0 || a > b {
                return None;
            }
            // The same grouping `condition` walks, counted forwards.
            let before: u8 = (2..*b).sum();
            Some(CONDITION_RATIO_BASE + before + (a - 1))
        }
    }
}

/// The condition on one step, if it carries one.
pub fn step_condition(payload: &[u8], track: usize, step: usize) -> Option<SyntaktCond> {
    if track >= NUM_BLOCKS || step >= NUM_STEPS {
        return None;
    }
    condition(*payload.get(block(track) + CONDITION_LANE + step)?)
}

// --- Writing back ------------------------------------------------------------

/// Write one block's notes into `payload`, leaving every other byte alone.
///
/// # This is not yet a write path, and it is the thing one would be built on
///
/// There is no `safe_write` route for this box, no firmware allowlist entry,
/// and nothing here sends anything anywhere. What this is for is the property
/// that has to hold *before* any of that is worth attempting: decode a captured
/// pattern, write the result straight back, and get the same bytes. A test does
/// exactly that over every capture and every block.
///
/// **The bits it does not understand, it does not touch.** A trig is OR-ed on
/// and masked off by [`TRIG_ON_BYTE0`] and [`TRIG_ON_BYTE1`] alone, so
/// [`TRIG_POSITIONAL`] survives, and so does the `0x10` that one track's trig
/// words carry in byte 0 for reasons nobody here knows. That is deliberate:
/// reproducing a byte is not the same as understanding it, and only one of the
/// two is needed to give it back unchanged.
///
/// A step with no note gets the values an empty step was measured to hold —
/// [`EMPTY_LANE`] in four lanes and [`EMPTY_MICRO`] in micro — so a note
/// removed here leaves the same bytes behind that the box leaves.
///
/// Returns whether anything was written; `false` means the indices or the slice
/// were out of range and nothing was touched.
pub fn set_track_notes(payload: &mut [u8], track: usize, notes: &[SyntaktNote]) -> bool {
    if track >= NUM_BLOCKS || payload.len() < BLOCK_BASE + BLOCK_STRIDE * (track + 1) {
        return false;
    }
    for step in 0..NUM_STEPS {
        set_step(payload, track, step, notes.iter().find(|n| n.step == step));
    }
    true
}

/// Write one step of a block.
///
/// `Some` authors a note trig. `None` clears one — **but only if the step plays
/// a note now.**
///
/// That exception is the whole reason this is a function rather than a branch
/// inside the loop above. A Syntakt step can hold a trig that sounds no note and
/// carries parameter locks alone; the DN2's published format map calls that
/// state `0x78` in the trig word's first byte, and nothing in this model can
/// represent it. A step like that was never on screen, so **nobody can have
/// meant to delete it**, and a write-back that cleared the word would destroy it
/// silently on every press.
///
/// This is the rule `a4_pattern`'s `TrigState::Trigless` arrived at on
/// 2026-09-01, for the same reason and after the same mistake. Here it costs a
/// [`plays_note`] check.
///
/// Returns whether anything was written; `false` means the indices or the slice
/// were out of range and nothing was touched.
pub fn set_step(
    payload: &mut [u8],
    track: usize,
    step: usize,
    note: Option<&SyntaktNote>,
) -> bool {
    if track >= NUM_BLOCKS
        || step >= NUM_STEPS
        || payload.len() < BLOCK_BASE + BLOCK_STRIDE * (track + 1)
    {
        return false;
    }
    let base = block(track);
    let word = base + TRIG_LANE + step * 2;
    match note {
        Some(n) => {
            payload[word] |= TRIG_ON_BYTE0;
            payload[word + 1] |= TRIG_ON_BYTE1;
            payload[base + NOTE_LANE + step] = if n.locked.note { n.note } else { NO_LOCK };
            payload[base + VELOCITY_LANE + step] =
                if n.locked.velocity { n.velocity } else { NO_LOCK };
            payload[base + LENGTH_LANE + step] =
                if n.locked.length { n.length_byte } else { NO_LOCK };
            payload[base + MICRO_LANE + step] = n.micro_ticks as u8;
            payload[base + CONDITION_LANE + step] = n.condition_byte;
        }
        // Not `else { clear }`: a step holding something unrepresentable keeps
        // every one of its bytes, the condition lane included.
        None if plays_note(payload, track, step) => {
            payload[word] &= !TRIG_ON_BYTE0;
            payload[word + 1] &= !TRIG_ON_BYTE1;
            payload[base + NOTE_LANE + step] = EMPTY_LANE;
            payload[base + VELOCITY_LANE + step] = EMPTY_LANE;
            payload[base + LENGTH_LANE + step] = EMPTY_LANE;
            payload[base + MICRO_LANE + step] = EMPTY_MICRO;
            payload[base + CONDITION_LANE + step] = EMPTY_LANE;
        }
        None => {}
    }
    true
}

/// Which of a note's three lockable lanes need a lock to say what it says, when
/// it is written over `step` of the destination `payload`.
///
/// A lane stays [`NO_LOCK`] when it **held** [`NO_LOCK`] and the value going in
/// is still that track's default. Each lane is decided on its own, so moving a
/// note's pitch locks the pitch and leaves a default velocity following the
/// default. A lane that already held a lock keeps one, even when its value
/// equals the default — "locked to the same value" is a fact about the box that
/// a later change to the default would show.
///
/// Why this exists: a write that locked every lane turned every untouched trig
/// following its track's default into one locked to that value. On 2026-09-15,
/// writing one moved pitch back to A03's T2 rewrote 23 lane bytes besides the
/// one that was edited. Nothing sounded different, but a later change to the
/// track's NOTE, VEL or LEN would no longer have reached those trigs.
///
/// A step with no trig reads [`EMPTY_LANE`], which is [`NO_LOCK`], so a note
/// authored there at the default follows the default — what the box does with
/// a trig placed by hand.
///
/// Out-of-range indices lock everything, the answer that cannot lose a value.
pub fn locks_for(
    payload: &[u8],
    track: usize,
    step: usize,
    note: u8,
    velocity: u8,
    length_byte: u8,
) -> Locks {
    let all = Locks { note: true, velocity: true, length: true };
    if step >= NUM_STEPS {
        return all;
    }
    let Some((default_note, default_velocity, default_length)) = defaults(payload, track) else {
        return all;
    };
    let keeps = |offset: usize, value: u8, default: u8| {
        !(lane(payload, track, offset, step) == Some(NO_LOCK) && value == default)
    };
    Locks {
        note: keeps(NOTE_LANE, note, default_note),
        velocity: keeps(VELOCITY_LANE, velocity, default_velocity),
        length: keeps(LENGTH_LANE, length_byte, default_length),
    }
}
