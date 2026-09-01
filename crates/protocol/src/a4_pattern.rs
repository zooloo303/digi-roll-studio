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
//! It is layout, not a write path. This section said the layout *could not* be
//! wired into [`crate::safe_write`] — "the A4 answers no dump request at all" —
//! and that claim fell on 2026-08-31: the box answers `0x60`–`0x6d` in the dump
//! namespace its advertised opcode list never described (PLAN.md §10, "The A4
//! answers dump requests"). So the backup and the read-back rule 1 wants are
//! wire questions with answers, and `safe_write::a4_safe_write_tracks` is the
//! write path built on them. This module stays what it was: the byte layout,
//! shared by that flow, the front-panel listener in `digi_midi::a4_transfer`,
//! and `examples/a4_pattern_send.rs`.

use crate::protocol::{
    build_dump_message, parse_sysex, SysExKind, DUMP_PROJECT_SETTINGS,
    DUMP_PROJECT_SETTINGS_REQUEST, FAMILY_ANALOG_FOUR,
};

/// A decoded gen-1 pattern payload is exactly this long. 1853 × 7 + 3, so the
/// final seven-bit group is ragged — see [`build_pattern`].
pub const PAYLOAD_LEN: usize = 12_974;

/// The A4 pattern opcode. Numerically [`DUMP_PROJECT_SETTINGS`], which is a
/// different message on the digis; the pair `(family, dump_type)` is the key.
pub const DUMP_A4_PATTERN: u8 = DUMP_PROJECT_SETTINGS;

/// The request that fetches one — [`DUMP_A4_PATTERN`] + 0x10, as for every
/// request in the dump namespace. Index is the slot, linear 0–127: 1 is A02,
/// 16 is B01, verified against the box 2026-08-31 (`examples/a4_dump_probe`,
/// PLAN.md §10 "The A4 answers dump requests").
pub const DUMP_A4_PATTERN_REQUEST: u8 = DUMP_PROJECT_SETTINGS_REQUEST;

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
/// **Confirmed from the write side 2026-08-31 on A4 0195**, which is the half
/// that reading dumps could never establish: the box was handed all four states
/// authored by this module and displayed each one as the table below says. So
/// the A4 reads these two bytes the way it writes them, and `byte1 & 0x03`
/// decides the display in both directions. [`build_trig_probe`] is the
/// experiment.
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
    ///
    /// **The box ignores the bit on the way in as well**, which is the sharp
    /// half of [`build_trig_probe`] and was confirmed on 2026-08-31: `(01,c0)`
    /// and `(09,c0)` were authored onto bare steps of an A4 and both stayed
    /// dark. A set bit that must be ignored is the one thing no capture could
    /// have shown.
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
/// **Hardware-confirmed 2026-08-31 on A4 0195**, by [`build_trig_probe`]. The
/// box lit steps 3 and 12 of the probe and **showed them as trigless trigs**,
/// which is more than the LED: it is the box reading byte 1 bit 1 the way this
/// function writes it. Step 12 carries `(08,02)`, so the positional bit does not
/// interfere.
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
///
/// **Hardware-confirmed 2026-08-31 on A4 0195.** The probe cleared a trigless
/// trig the box itself had written, and the box showed step 1 dark — so a clear
/// authored here takes, and the `(01,c0)` this leaves behind on a note trig
/// really does display as an empty step.
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

// --- The trig-model write probe ----------------------------------------------

/// SYN1. One track, one screen page, and the track whose LEDs Neil has already
/// read twice.
pub const PROBE_TRACK: usize = 0;

/// The note the probe's control trig carries. `0x30` is the only note byte a
/// write has ever put on this box, and the box displayed it as C4 — so if the
/// control step misbehaves, the send is at fault rather than the note.
pub const PROBE_NOTE: u8 = 0x30;

/// The probe's seven steps: `(step, the box should light it, what a
/// disagreement would mean)`.
///
/// **`expect_lit` here is a hand-written prediction, deliberately not derived
/// from [`TrigState::is_live`].** The experiment is a test of the model, and a
/// prediction computed by the model under test would agree with it by
/// construction. [`ProbeStep::state`] carries what the reader thinks; this
/// column carries what the box is claimed to do; a test asserts they match, and
/// the front panel is the third witness that settles it.
const PROBE_STEPS: [(usize, bool, &str); 7] = [
    (1, false, "our clear does not take — the box keeps a trig we removed"),
    (3, true, "the box will not accept a trigless trig authored by us"),
    (5, true, "the send did not land at all; discard the run and retry the cable"),
    (7, false, "a step we never touched lit — the write reached the wrong offsets"),
    (9, false, "the box honours byte 0 bit 0, so `byte1 & 0x03` alone is WRONG"),
    (10, false, "the same, on a step whose positional bit is set"),
    (12, true, "the positional bit suppresses an authored trigless trig"),
];

/// One step of [`build_trig_probe`]: the bytes authored, and what the front
/// panel should show if the trig model holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeStep {
    /// One-based, as the box counts steps.
    pub step: usize,
    /// The two trig bytes as they ended up in the payload.
    pub bytes: (u8, u8),
    /// The note lane byte, [`NO_NOTE`] where unset.
    pub note: u8,
    /// What [`trig_state`] reads [`ProbeStep::bytes`] as — **our** reading.
    pub state: TrigState,
    /// What the **box** is predicted to do — hand-written, and deliberately not
    /// computed from [`ProbeStep::state`]. See [`build_trig_probe`].
    pub expect_lit: bool,
    /// What it means if the box disagrees with `expect_lit` on this step.
    pub falsifies: &'static str,
}

/// A pattern authored to make the trig model falsifiable on a front panel.
#[derive(Debug, Clone)]
pub struct TrigProbe {
    /// The slot the baseline came from, and the slot this would overwrite.
    pub slot: u8,
    pub payload: Vec<u8>,
    pub track: usize,
    pub steps: Vec<ProbeStep>,
}

impl TrigProbe {
    /// The steps whose LEDs should be lit, which is the whole prediction in one
    /// line: **3, 5 and 12, and nothing else on the track.**
    pub fn expected_lit_steps(&self) -> Vec<usize> {
        self.steps.iter().filter(|s| s.expect_lit).map(|s| s.step).collect()
    }

    /// Frame the probe as a sendable message.
    pub fn build(&self) -> Result<Vec<u8>, String> {
        build_pattern(self.slot, &self.payload)
    }
}

/// Author the four trig states onto one track, so that **one look at the front
/// panel** either confirms the trig model or refutes it.
///
/// This was PLAN.md §10's item 2 — **closed 2026-08-31**, and the last question
/// about this format that a capture could not answer. The model says the box reads
/// `byte1 & 0x03` alone and that byte 0 bit 0 is residue it ignores. Every byte
/// behind that claim came from dumps the box *sent*; nothing establishes that
/// the box reads its own bytes the way it writes them, and the sharp half — a
/// bit that is set and must be ignored — cannot be checked from this side of the
/// cable at all.
///
/// The layout, on [`PROBE_TRACK`], with the predicted LEDs:
///
/// | step | authored | bytes | box should show |
/// |---|---|---|---|
/// | 1 | the baseline's own trigless trig, cleared | `(00,00)` | nothing |
/// | 3 | [`set_trigless_trig`] | `(00,02)` | **a trig** |
/// | 5 | [`set_note_trig`] with [`PROBE_NOTE`] | `(01,c1)` | **a trig** |
/// | 7 | nothing — left as the baseline has it | `(00,00)` | nothing |
/// | 9 | a note trig, then [`clear_trig`] | `(01,c0)` | nothing |
/// | 10 | the same, on an odd step | `(09,c0)` | nothing |
/// | 12 | [`set_trigless_trig`] on an odd step | `(08,02)` | **a trig** |
///
/// **Run on hardware 2026-08-31, A4 0195: every one of the seven predictions
/// held.** Steps 3, 5 and 12 lit and the other 61 steps were dark, and 3 and 12
/// showed as *trigless* trigs rather than merely lit — so the box read byte 1
/// bit 1 as this module writes it, ignored byte 1 bit 0 where it was clear, and
/// ignored byte 0 bit 0 at both parities of the positional bit. PLAN.md §10 open
/// item 2 closed on that run.
///
/// The function is kept rather than deleted, for the reason
/// `probe_drive_read.rs` is kept: it is the experiment the finding rests on, and
/// a claim whose experiment has been thrown away is a claim on trust.
///
/// **Steps 9 and 10 are the experiment**; the rest are controls that say which
/// way to read a surprise. Step 5 is the shape hardware has already accepted, so
/// a dark step 5 means the send failed rather than the model did. Steps 9 and 10
/// carry the same state at both parities of the positional bit, because
/// [`TRIG_BYTE0_POSITIONAL`] shares byte 0 with the residue bit and a single
/// parity cannot separate them — the same trap as A01's slot 0 and the checksum
/// start.
///
/// **Residue is authored by composition, not by a primitive**, because that is
/// what it is: a note trig with the note taken off. [`set_note_trig`] with
/// `None` then [`clear_trig`] leaves `(b0|01, c0)` and the note lane at
/// [`NO_NOTE`] — which is byte-for-byte what all **fifteen** residue steps of
/// A01 SYN4 hold, both parities included. A `set_residue` helper would have been
/// one call and would have hidden that.
///
/// # The baseline
///
/// Takes a parsed dump rather than building from nothing, because a pattern this
/// code invented would differ from a real one in 12,974 places and a surprise
/// could be any of them. The intended baseline is
/// `analogfour-A16-trigless-trk1-step1-2026-08-31.syx`: a real A16 dump, one
/// change away from the message hardware has already accepted, and it carries
/// the box's own trigless trig on step 1 — so clearing it is a free eighth
/// question at no extra risk.
///
/// The preconditions are checked rather than assumed. A baseline with something
/// already on the probe's steps would still frame and send, and its predictions
/// would silently not hold.
pub fn build_trig_probe(baseline: &A4Pattern) -> Result<TrigProbe, String> {
    let mut payload = baseline.payload.clone();
    check_payload(&payload)?;

    let states = read_track_states(&payload, PROBE_TRACK)?;
    if states[0] != TrigState::Trigless {
        return Err(format!(
            "the probe baseline must carry the box's own trigless trig on {} step 1, and this \
             one reads {:?} there — the intended baseline is the A16-trigless capture",
            TRACK_NAMES[PROBE_TRACK], states[0]
        ));
    }
    for &(step, _, _) in &PROBE_STEPS[1..] {
        if states[step - 1] != TrigState::Empty {
            return Err(format!(
                "probe step {step} must start bare and this baseline reads {:?} there",
                states[step - 1]
            ));
        }
    }

    clear_trig(&mut payload, PROBE_TRACK, 0)?;
    set_trigless_trig(&mut payload, PROBE_TRACK, 2)?;
    set_note_trig(&mut payload, PROBE_TRACK, 4, Some(PROBE_NOTE))?;
    // Step 7 is untouched on purpose, so leave index 6 alone.
    for idx in [8, 9] {
        set_note_trig(&mut payload, PROBE_TRACK, idx, None)?;
        clear_trig(&mut payload, PROBE_TRACK, idx)?;
    }
    set_trigless_trig(&mut payload, PROBE_TRACK, 11)?;

    // Read the bytes back out rather than recording what was asked for: the
    // table a person checks against a screen should describe the payload that
    // is actually going to be sent.
    let steps = PROBE_STEPS
        .iter()
        .map(|&(step, expect_lit, falsifies)| {
            let o = trig_offset(PROBE_TRACK, step - 1);
            let bytes = (payload[o], payload[o + 1]);
            ProbeStep {
                step,
                bytes,
                note: payload[note_offset(PROBE_TRACK, step - 1)],
                state: trig_state(bytes.0, bytes.1),
                expect_lit,
                falsifies,
            }
        })
        .collect();

    Ok(TrigProbe { slot: baseline.slot, payload, track: PROBE_TRACK, steps })
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
