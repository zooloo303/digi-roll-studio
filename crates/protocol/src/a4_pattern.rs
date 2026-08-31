//! The Analog Four's **gen-1** SysEx pattern dump: framing, payload layout,
//! trigs and notes.
//!
//! A 2013 box on a protocol elk-herd never documented. Everything here was
//! measured from captures taken off the A4's own front-panel SysEx Dump menu in
//! August 2026 — the nine `analogfour-*.syx` fixtures are the evidence, and
//! PLAN.md §10 is the working. The p-lock pool is [`crate::a4_plocks`].
//!
//! # What gen-1 shares with gen-2, which is more than PLAN.md thought
//!
//! Two of the three differences this port was scoped around **do not exist**,
//! and finding that out was most of the work. Both were written down from
//! reading code rather than running it.
//!
//! * **The seven-bit packing is the same packing.** PLAN.md §10 and
//!   DEVELOPMENT.md lesson 14 say gen-1 runs MSB-first where
//!   [`crate::sevenbit`] "runs bit 0 to byte 0". `sevenbit.rs` does not: its
//!   `head |= 1 << (6 - i)` puts the first data byte's high bit in header bit 6,
//!   which is precisely the gen-1 order. The two are the same function on every
//!   input, including every ragged tail length. Lesson 14's *conclusion* is
//!   right and hardware-verified — the A4 is MSB-first, and `BEEFBABA` reads at
//!   offset 0 of a sound dump only that way — but its *attribution* was not: the
//!   order that produced four wrong rounds on 2026-08-30 was a hand-written
//!   `msb_first=False`, believed to be what `sevenbit.rs` did and never checked
//!   against it. So `sevenbit.rs` takes no bit-order parameter, and
//!   [`sevenbit_is_shared_across_generations`] is the test that will say so
//!   again if anyone re-opens it.
//! * **The message framing is the same framing.**
//!   [`crate::protocol::build_dump_message`] emits an A4 pattern dump
//!   byte-exactly, and [`crate::protocol::parse_sysex`] reads one with its
//!   checksum and count verified. The header PLAN.md §10 recorded as
//!   `mfr(3) product device type 01 01 slot` **is** the gen-2 dump header:
//!   `product` is `family`, and the `01 01` "constant across every capture,
//!   unidentified" is gen-2's `version` field. The checksum starts at the same
//!   place and the count is the same `encoded + 5`.
//!
//! What does differ is the **meaning** of the opcode and the shape of the
//! payload. `0x54` is [`crate::protocol::DUMP_PROJECT_SETTINGS`] on the digis
//! and a *pattern* here, so a `dump_type` is only meaningful alongside its
//! `family` — see [`is_a4_pattern`].
//!
//! # The payload
//!
//! 12,974 bytes, which arrives twice from unrelated directions: it is the
//! decoded length of the dump, and it is the measured stride of an A4 project
//! file's leading 1.67 MB. The dump and the project's pattern record are the
//! same object.
//!
//! ```text
//!   0      4          header
//!   4      6 × 751    tracks — SYN1..SYN4, FX, CV
//!   4510   128 × 66   the p-lock pool (crate::a4_plocks)
//!   12958  16         tail; the slot marker at +4 (byte 12,962)
//! ```
//!
//! Inside a 751-byte track, measured across 18 track-instances in three
//! captures:
//!
//! ```text
//!   +0     2 × 64     trig bytes
//!   +128   64         note lane, FF = no note
//!   +192   64         per-step lane, FF-filled when cleared     unnamed
//!   +256   64         per-step lane, FF-filled when cleared     unnamed
//!   +320   64         per-step lane, ZERO-filled when cleared   unnamed
//!   +384   64         per-step lane, FF-filled when cleared     unnamed
//!   +448   11         per-track defaults: 30 64 0e 00 00 00 40 00 00 00 01
//!   +459   64         FF
//!   +523   9          per-track, unnamed: 00 05 02 00 0e 64 40 40 40
//!   +532   64         per-step lane — populated on exactly SYN1's 32 trig
//!                     steps in A01, so trig-attached                unnamed
//!   +596   64         per-step lane — populated on 25 of those 32    unnamed
//!   +660   64         per-step lane, FF in every capture so far      unnamed
//!   +724   27         per-track tail                                unnamed
//! ```
//!
//! The offsets above sum to 751 exactly, which is the only thing that makes the
//! decomposition more than a list of places something was seen. Six of the
//! regions have no name: they are recorded as *shape* — a 64-byte per-step lane
//! and its fill — because that shape is what a capture can establish and what
//! the next capture will have to contradict.
//!
//! **`FF` in a per-step lane means "unset, take the track default".** That is
//! why a fresh trig inherits its note from `+448`, and it is the opposite of the
//! pool's convention for a *free lane* — see [`crate::a4_plocks`], where the two
//! opposite fills have already caused one reader bug.
//!
//! # What this module is not
//!
//! It is layout, not a write path. Nothing here is wired into
//! [`crate::safe_write`], and it cannot be: PLAN.md §7 rule 1 wants a backup and
//! a read-back, and **the A4 answers no dump request at all**. Its dumps are
//! initiated from its own front panel, so the only backup is a human taking one
//! and the only verification is the box's screen. [`build_pattern`] frames a
//! message; deciding whether to send it is somebody else's problem, and
//! `examples/a4_pattern_send.rs` is where that lives.

use crate::protocol::{
    build_dump_message, parse_sysex, SysExKind, DUMP_PROJECT_SETTINGS, FAMILY_ANALOG_FOUR,
};

/// A decoded gen-1 pattern payload is exactly this long. 1853 × 7 + 3, so the
/// final seven-bit group is ragged — see [`build_pattern`].
pub const PAYLOAD_LEN: usize = 12_974;

/// The A4 pattern opcode. Numerically [`DUMP_PROJECT_SETTINGS`], which is a
/// different message on the digis; the pair `(family, dump_type)` is the key.
pub const DUMP_A4_PATTERN: u8 = DUMP_PROJECT_SETTINGS;

pub const NUM_TRACKS: usize = 6;
pub const NUM_STEPS: usize = 64;
pub const TRACK_BASE: usize = 4;
pub const TRACK_STRIDE: usize = 751;

/// Track names as the box labels them. Four analogue synth voices, then FX and
/// CV, which have trig lanes like the rest.
pub const TRACK_NAMES: [&str; NUM_TRACKS] = ["SYN1", "SYN2", "SYN3", "SYN4", "FX", "CV"];

/// Two bytes per step, from the track base.
pub const TRIG_LANE: usize = 0;
/// One byte per step.
pub const NOTE_LANE: usize = 128;
/// One byte **per track**, not per step: the note a fresh trig inherits.
///
/// Established without a new capture, then confirmed by accident: a factory
/// reset moved A01 by exactly one byte in 12,974, and it was this one on SYN1.
/// A per-step field cannot drift alone.
pub const DEFAULT_NOTE: usize = 448;
/// `FF` in the note lane, or in any per-step lane: unset, use the track default.
pub const NO_NOTE: u8 = 0xFF;

/// Byte 12,962 tracks the slot index — `FF` in a pattern the box has never
/// saved, the zero-based slot in every dump taken after a save.
pub const SLOT_MARKER: usize = 12_962;

/// Byte 1 bit 0: this trig plays a note.
pub const TRIG_NOTE: u8 = 0x01;
/// Byte 1 bit 1: this trig fires without one.
pub const TRIG_TRIGLESS: u8 = 0x02;
/// The whole of the trig state. **Byte 0 contributes nothing** — see
/// [`TrigState`].
pub const TRIG_STATE: u8 = TRIG_NOTE | TRIG_TRIGLESS;

/// The second trig byte the box writes for a note trig. `0xC0` plus
/// [`TRIG_NOTE`]; what the `0xC0` bits mean is unknown, and they are reproduced
/// rather than understood.
pub const TRIG_BYTE1_NOTE: u8 = 0xC1;

/// Byte 0 bit 3 is **positional**, not state: in a pattern with nothing in it
/// the first trig byte reads `00 08 00 08 …` across all 64 steps. It carries no
/// per-step information, so a write must OR rather than assign.
pub const TRIG_BYTE0_POSITIONAL: u8 = 0x08;

/// What the two trig bytes say a step is.
///
/// **This enum is the third model of these two bytes and the first correct
/// one**, and the two before it were wrong in the same direction: they counted
/// [`TrigState::Residue`] as a trig, so both reported A01 SYN4 as 19 trigs where
/// the box shows 4. Neither was refuted by any byte we had. What settled it was
/// Neil looking at an unlit LED, either side of a factory reset.
/// DEVELOPMENT.md lesson 16 is the whole story.
///
/// | bytes | state | the box shows |
/// |---|---|---|
/// | `(00,00)` | [`Empty`](TrigState::Empty) | nothing |
/// | `(00,02)` | [`Trigless`](TrigState::Trigless) | a trig |
/// | `(01,c1)` | [`Note`](TrigState::Note) | a trig |
/// | `(01,c0)` | [`Residue`](TrigState::Residue) | **nothing** |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrigState {
    /// No trig, and no sign there ever was one.
    Empty,
    /// A trig that fires without a note.
    Trigless,
    /// A trig that plays the note in the note lane.
    Note,
    /// **Byte 0 bit 0 set with the state bits clear.** A note trig whose note
    /// was taken off again; the box stopped honouring the bit without erasing
    /// it. Displays as an empty step, and counting it as a trig is the bug that
    /// took two models to find.
    Residue,
}

impl TrigState {
    /// Does the box show a trig on this step?
    pub fn is_live(self) -> bool {
        matches!(self, TrigState::Trigless | TrigState::Note)
    }
}

/// Read the state out of a step's two trig bytes.
///
/// `byte1 & 0x03` decides it, alone. `byte0` is consulted only to tell
/// [`TrigState::Empty`] from [`TrigState::Residue`], which the box displays
/// identically and which differ only in their history.
pub fn trig_state(byte0: u8, byte1: u8) -> TrigState {
    if byte1 & TRIG_NOTE != 0 {
        TrigState::Note
    } else if byte1 & TRIG_TRIGLESS != 0 {
        TrigState::Trigless
    } else if byte0 & 0x01 != 0 {
        TrigState::Residue
    } else {
        TrigState::Empty
    }
}

/// One step the box would show a trig on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trig {
    /// One-based, as the box counts steps.
    pub step: usize,
    pub state: TrigState,
    /// The note lane's byte, `None` where it is [`NO_NOTE`]. A
    /// [`TrigState::Trigless`] trig has no note; a [`TrigState::Note`] trig
    /// whose lane reads `FF` takes the track's [`DEFAULT_NOTE`].
    pub note: Option<u8>,
    /// The two bytes as stored. Kept because the `0xC0` half of byte 1 is
    /// unexplained and a round trip has to reproduce it.
    pub bytes: (u8, u8),
}

/// A parsed gen-1 pattern dump.
#[derive(Debug, Clone)]
pub struct A4Pattern {
    /// Zero-based slot index: 0 is A01, 15 is A16.
    pub slot: u8,
    pub payload: Vec<u8>,
}

impl A4Pattern {
    /// `A01`-style name for the slot.
    pub fn slot_name(&self) -> String {
        slot_name(self.slot)
    }
}

/// `A01` … `H16` for a zero-based slot index.
pub fn slot_name(slot: u8) -> String {
    format!("{}{:02}", (b'A' + (slot >> 4) % 8) as char, slot % 16 + 1)
}

/// Is this parsed dump an A4 pattern?
///
/// **The `family` check is not decoration.** `0x54` is a project-settings dump
/// on a Digitakt II and a pattern here; reading `dump_type` without `family`
/// would hand a DT2 project-settings payload to [`read_track_trigs`], which
/// would find trigs in it.
pub fn is_a4_pattern(parsed: &crate::protocol::ParsedSysEx) -> bool {
    parsed.kind == SysExKind::Dump
        && parsed
            .dump
            .as_ref()
            .is_some_and(|d| d.family == FAMILY_ANALOG_FOUR && d.dump_type == DUMP_A4_PATTERN)
}

/// Parse one `F0 … F7` message as a gen-1 pattern dump.
///
/// Rejects anything whose checksum, count or payload length does not hold. A
/// capture that does not verify is not evidence of anything, and every
/// expectation downstream of here assumes all three.
pub fn parse_pattern(message: &[u8]) -> Result<A4Pattern, String> {
    let parsed = parse_sysex(message);
    if !is_a4_pattern(&parsed) {
        return Err(match parsed.dump.as_ref() {
            Some(d) => format!(
                "not an A4 pattern dump: family {:#04x}, type {:#04x}",
                d.family, d.dump_type
            ),
            None => format!("not an Elektron dump message ({:?})", parsed.kind),
        });
    }
    let d = parsed.dump.unwrap();
    if !d.checksum_ok {
        return Err("checksum does not verify".into());
    }
    if !d.count_ok {
        return Err("byte count does not verify".into());
    }
    if d.payload.len() != PAYLOAD_LEN {
        return Err(format!(
            "payload is {} bytes, an A4 pattern is {PAYLOAD_LEN}",
            d.payload.len()
        ));
    }
    Ok(A4Pattern { slot: d.index, payload: d.payload })
}

/// Frame a payload as a sendable `F0 … F7` pattern dump.
///
/// **Refuses a payload whose ragged final group is not seven-bit clean.**
/// 12,974 is 1853 × 7 + 3, so the last group carries three bytes, and no
/// capture has ever had a high bit set in them — which means both candidate
/// short-group bit orders encode that group identically and **no capture can
/// tell them apart**. Rather than pick, this refuses, exactly as
/// `local/a4_pattern.py` does. A payload that ends high-bit-set would settle it
/// in one dump; until then, emitting one would be a guess sent to hardware.
///
/// The framing itself is [`build_dump_message`] unmodified, and it reproduces
/// the box's own messages byte for byte — see the round-trip tests.
pub fn build_pattern(slot: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
    if payload.len() != PAYLOAD_LEN {
        return Err(format!("payload is {} bytes, an A4 pattern is {PAYLOAD_LEN}", payload.len()));
    }
    let tail = &payload[payload.len() - payload.len() % 7..];
    if tail.iter().any(|b| b & 0x80 != 0) {
        return Err(format!(
            "last seven-bit group {} has a high bit set; short-group bit order is \
             unmeasured, refusing to encode",
            tail.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
        ));
    }
    Ok(build_dump_message(FAMILY_ANALOG_FOUR, DUMP_A4_PATTERN, slot, payload))
}

// --- Layout ------------------------------------------------------------------

fn check_track(track: usize) -> Result<(), String> {
    if track >= NUM_TRACKS {
        return Err(format!("no track {track}; an A4 pattern has {NUM_TRACKS}"));
    }
    Ok(())
}

fn check_payload(payload: &[u8]) -> Result<(), String> {
    if payload.len() != PAYLOAD_LEN {
        return Err(format!("payload is {} bytes, an A4 pattern is {PAYLOAD_LEN}", payload.len()));
    }
    Ok(())
}

/// Where track `track`'s 751 bytes start. Zero-based.
pub fn track_base(track: usize) -> usize {
    TRACK_BASE + track * TRACK_STRIDE
}

/// Offset of step `step`'s first trig byte. Both indices zero-based.
pub fn trig_offset(track: usize, step: usize) -> usize {
    track_base(track) + TRIG_LANE + step * 2
}

/// Offset of step `step`'s note byte. Both indices zero-based.
pub fn note_offset(track: usize, step: usize) -> usize {
    track_base(track) + NOTE_LANE + step
}

/// The note a fresh trig on this track inherits.
pub fn track_default_note(payload: &[u8], track: usize) -> Result<u8, String> {
    check_payload(payload)?;
    check_track(track)?;
    Ok(payload[track_base(track) + DEFAULT_NOTE])
}

/// Every step of one track the box would show a trig on, in step order.
///
/// [`TrigState::Residue`] steps are **not** included, which is the whole point:
/// under this reader A01 SYN4 holds 4 trigs — the roots on the bar lines — where
/// the two earlier models counted 19.
pub fn read_track_trigs(payload: &[u8], track: usize) -> Result<Vec<Trig>, String> {
    check_payload(payload)?;
    check_track(track)?;
    Ok((0..NUM_STEPS)
        .filter_map(|step| {
            let o = trig_offset(track, step);
            let (b0, b1) = (payload[o], payload[o + 1]);
            let state = trig_state(b0, b1);
            state.is_live().then(|| {
                let note = payload[note_offset(track, step)];
                Trig {
                    step: step + 1,
                    state,
                    note: (note != NO_NOTE).then_some(note),
                    bytes: (b0, b1),
                }
            })
        })
        .collect())
}

/// The note this trig actually sounds, `None` if it sounds none.
///
/// [`Trig::note`] is the raw lane byte and stops at [`NO_NOTE`]. **In a per-step
/// lane `NO_NOTE` means "unset, use the track default"**, not "no note", so a
/// note trig whose lane reads `FF` plays whatever sits at [`DEFAULT_NOTE`] — that
/// is the mechanism by which a fresh trig inherits its pitch, established across
/// five captures.
///
/// **No fixture contains one**, which is exactly why this exists: every A4 note
/// trig captured so far carries its own note, so a reader that returned `None`
/// here would be right about all nine files and wrong on the box. That is
/// lesson 16's shape — a state invisible to every check on our side of the
/// cable — and the cheap fix is to not have the case.
pub fn effective_note(payload: &[u8], track: usize, trig: &Trig) -> Result<Option<u8>, String> {
    match trig.state {
        TrigState::Note => Ok(Some(match trig.note {
            Some(n) => n,
            None => track_default_note(payload, track)?,
        })),
        _ => Ok(None),
    }
}

/// Every step's state, including the ones that display as nothing — the
/// diagnostic view, where [`read_track_trigs`] is the musical one.
pub fn read_track_states(payload: &[u8], track: usize) -> Result<Vec<TrigState>, String> {
    check_payload(payload)?;
    check_track(track)?;
    Ok((0..NUM_STEPS)
        .map(|step| {
            let o = trig_offset(track, step);
            trig_state(payload[o], payload[o + 1])
        })
        .collect())
}

// --- Writing -----------------------------------------------------------------

/// Author a note trig on one step, in place.
///
/// **This is the one write on this box that hardware has confirmed.** `0x30` was
/// written to SYN1 step 1 of A16 and the A4 displayed C4 on that step — which is
/// also where the octave correction below came from. It reproduces exactly what
/// the box itself writes: byte 0 bit 0 set, byte 1 [`TRIG_BYTE1_NOTE`], and the
/// note in the note lane.
///
/// **Byte 0 is OR-ed, never assigned**, because bit 3 of it is positional and
/// belongs to the step rather than to the trig.
///
/// Pass `None` for `note` to leave the note lane at [`NO_NOTE`], so the trig
/// takes the track's [`DEFAULT_NOTE`].
pub fn set_note_trig(
    payload: &mut [u8],
    track: usize,
    step: usize,
    note: Option<u8>,
) -> Result<(), String> {
    check_payload(payload)?;
    check_track(track)?;
    check_step(step)?;
    let o = trig_offset(track, step);
    payload[o] |= TRIG_NOTE;
    payload[o + 1] = TRIG_BYTE1_NOTE;
    payload[note_offset(track, step)] = note.unwrap_or(NO_NOTE);
    Ok(())
}

/// Author a trigless trig on one step, in place.
///
/// **Predicted, not verified.** The model says the box reads `byte1 & 0x03`
/// alone, so `(00,02)` on a bare step should light a trigless trig — and that is
/// what the box wrote when Neil made one by hand, which is why the bytes are not
/// in doubt. What is untested is the box *accepting* them from us: no A4 write
/// has yet carried anything but a note trig. This is open item 2 in PLAN.md §10,
/// and the sharper half of that experiment is the reverse — authoring `(01,c0)`
/// and confirming the box shows nothing, since it asks the box to ignore a bit
/// that is set.
///
/// Byte 0 is left alone entirely: the box's own trigless trig on a cleared A16
/// changed byte 1 and nothing else.
pub fn set_trigless_trig(payload: &mut [u8], track: usize, step: usize) -> Result<(), String> {
    check_payload(payload)?;
    check_track(track)?;
    check_step(step)?;
    let o = trig_offset(track, step);
    payload[o + 1] = TRIG_TRIGLESS;
    Ok(())
}

/// Take the trig off one step, in place, the way the box does.
///
/// Clears the two state bits of byte 1 and **leaves byte 0 as it is**, which
/// reproduces the box's own behaviour: a note trig whose note is removed becomes
/// `(01,c0)` — [`TrigState::Residue`] — and displays as empty. Erasing byte 0
/// too would be tidier and is not what the hardware does, and the difference is
/// invisible on the screen, so the box's version is the one that survives a
/// round trip.
///
/// The note lane is not touched either. It is a per-step lane, so [`NO_NOTE`]
/// there means "use the track default" rather than "no note", and rewriting it
/// would be a second, unasked-for edit.
pub fn clear_trig(payload: &mut [u8], track: usize, step: usize) -> Result<(), String> {
    check_payload(payload)?;
    check_track(track)?;
    check_step(step)?;
    let o = trig_offset(track, step);
    payload[o + 1] &= !TRIG_STATE;
    Ok(())
}

fn check_step(step: usize) -> Result<(), String> {
    if step >= NUM_STEPS {
        return Err(format!("no step {step}; an A4 track has {NUM_STEPS}"));
    }
    Ok(())
}

/// A note byte in the A4's own octave numbering: it displays `0x30` as **C4**.
///
/// **Measured on the box, not chosen.** A written `0x30` came back on the A4's
/// screen as C4, where this project had been printing C3 under the `60 = C4`
/// convention that is not this box's. Every note name in PLAN.md §10 written
/// before 2026-08-30 is one octave low. Intervals are unaffected, so the
/// argument the layout was validated by still holds; only the labels moved.
pub fn note_name(v: u8) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    if v == NO_NOTE {
        return "--".to_string();
    }
    format!("{}{}", N[usize::from(v) % 12], v / 12)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sevenbit::{decode7, encode7};

    /// The claim this port was scoped around, as a falsifiable test.
    ///
    /// `sevenbit.rs` is the elk-herd port, pinned against twelve gen-2 hardware
    /// captures. If it and the A4's gen-1 order were different functions, this
    /// would fail — and PLAN.md §10 plus DEVELOPMENT.md lesson 14 both say they
    /// are. They are not. Every ragged tail length is covered because that is
    /// the only place a group-packing order has room to disagree.
    #[test]
    fn sevenbit_is_shared_across_generations() {
        // The gen-1 decoder as `local/a4_pattern.py` and
        // `examples/a4_pattern_send.rs` spell it: header bit `6 - n` carries
        // data byte `n`.
        fn decode7_gen1(wire: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            for group in wire.chunks(8) {
                let msbs = group[0];
                for (n, &b) in group[1..].iter().enumerate() {
                    out.push(b | if msbs >> (6 - n) & 1 == 1 { 0x80 } else { 0 });
                }
            }
            out
        }

        let mut data = Vec::new();
        let mut x: u32 = 0x1234_5678;
        for _ in 0..2048 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            data.push((x >> 16) as u8);
        }
        for len in [0usize, 1, 2, 3, 6, 7, 8, 9, 14, 15, 16, 55, 2048] {
            let wire = encode7(&data[..len]);
            assert_eq!(
                decode7(&wire),
                decode7_gen1(&wire),
                "the two generations' seven-bit orders differ at length {len}"
            );
        }
    }

    #[test]
    fn trig_states_cover_the_four_observed_byte_pairs() {
        assert_eq!(trig_state(0x00, 0x00), TrigState::Empty);
        assert_eq!(trig_state(0x00, 0x02), TrigState::Trigless);
        assert_eq!(trig_state(0x01, 0xc1), TrigState::Note);
        assert_eq!(trig_state(0x01, 0xc0), TrigState::Residue);
        // Byte 0 bit 3 is positional and must not reach the state.
        assert_eq!(trig_state(0x08, 0x00), TrigState::Empty);
        assert_eq!(trig_state(0x09, 0xc0), TrigState::Residue);
        assert!(!TrigState::Residue.is_live());
        assert!(!TrigState::Empty.is_live());
        assert!(TrigState::Note.is_live());
        assert!(TrigState::Trigless.is_live());
    }

    #[test]
    fn a_write_preserves_the_positional_bit() {
        let mut payload = vec![0u8; PAYLOAD_LEN];
        // An even step in a cleared pattern reads `08` in byte 0.
        let o = trig_offset(0, 1);
        payload[o] = TRIG_BYTE0_POSITIONAL;
        set_note_trig(&mut payload, 0, 1, Some(0x30)).unwrap();
        assert_eq!(payload[o], TRIG_BYTE0_POSITIONAL | TRIG_NOTE);
        assert_eq!(payload[o + 1], TRIG_BYTE1_NOTE);
    }

    /// The case no capture has: a note trig whose lane is unset takes the
    /// track's default, and a trigless trig takes nothing however the lane
    /// reads.
    #[test]
    fn an_unset_note_lane_resolves_to_the_track_default() {
        let mut payload = vec![0u8; PAYLOAD_LEN];
        payload[track_base(2) + DEFAULT_NOTE] = 0x3c;
        set_note_trig(&mut payload, 2, 0, None).unwrap();
        set_trigless_trig(&mut payload, 2, 1).unwrap();

        let trigs = read_track_trigs(&payload, 2).unwrap();
        assert_eq!(trigs[0].note, None, "the raw lane byte is still NO_NOTE");
        assert_eq!(effective_note(&payload, 2, &trigs[0]).unwrap(), Some(0x3c));
        assert_eq!(
            effective_note(&payload, 2, &trigs[1]).unwrap(),
            None,
            "a trigless trig sounds nothing whatever the lane says"
        );
    }

    #[test]
    fn clearing_a_note_trig_leaves_the_residue_the_box_leaves() {
        let mut payload = vec![0u8; PAYLOAD_LEN];
        set_note_trig(&mut payload, 0, 0, Some(0x30)).unwrap();
        clear_trig(&mut payload, 0, 0).unwrap();
        let o = trig_offset(0, 0);
        assert_eq!((payload[o], payload[o + 1]), (0x01, 0xc0));
        assert_eq!(trig_state(payload[o], payload[o + 1]), TrigState::Residue);
        assert!(read_track_trigs(&payload, 0).unwrap().is_empty());
    }

    #[test]
    fn build_refuses_a_ragged_tail_with_a_high_bit() {
        let mut payload = vec![0u8; PAYLOAD_LEN];
        payload[PAYLOAD_LEN - 1] = 0x80;
        let err = build_pattern(0, &payload).unwrap_err();
        assert!(err.contains("refusing to encode"), "{err}");
        // The same payload with a clean tail frames fine.
        payload[PAYLOAD_LEN - 1] = 0x7f;
        assert!(build_pattern(0, &payload).is_ok());
    }

    #[test]
    fn build_refuses_a_payload_of_the_wrong_length() {
        assert!(build_pattern(0, &[0u8; 16]).is_err());
    }

    #[test]
    fn slot_names_match_the_front_panel() {
        assert_eq!(slot_name(0), "A01");
        assert_eq!(slot_name(15), "A16");
        assert_eq!(slot_name(16), "B01");
        assert_eq!(slot_name(127), "H16");
    }

    /// **The `% 8` earns itself only above 127**, which is why the test above
    /// cannot see it: the A4 has 128 patterns, so a higher slot byte is a
    /// malformed message. It wraps to bank A rather than counting on past `H`
    /// into whatever ASCII lies there — the same thing `pattern::bank_name` does
    /// for the digis, and worth pinning because a naming function that emits
    /// `I01` for a bad byte reads like a real slot.
    #[test]
    fn a_slot_byte_past_the_last_bank_wraps_rather_than_inventing_a_letter() {
        assert_eq!(slot_name(128), "A01");
        assert_eq!(slot_name(255), "H16");
    }

    /// The correction that came off the box's own screen.
    #[test]
    fn note_names_use_the_a4s_octave_numbering() {
        assert_eq!(note_name(0x30), "C4");
        assert_eq!(note_name(NO_NOTE), "--");
    }
}
