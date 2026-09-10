//! Putting a pattern **onto** a Syntakt.
//!
//! The second box-specific write path in this crate, and it exists for the
//! reason the first one does: [`a4_transfer::send_pattern`] refuses anything
//! that is not an Analog Four pattern dump, which is correct of it and leaves
//! every other box needing its own. `ElektronDevice::send` is private so nobody
//! bolts a store onto the side of the read path, and nothing here changes that.
//!
//! [`a4_transfer::send_pattern`]: crate::a4_transfer::send_pattern
//!
//! # What this validates, and why it validates it here
//!
//! DEVELOPMENT.md lesson 13: an Analog Four handed a body it could not parse
//! stopped answering SysEx entirely — not just the file layer — and came back
//! only on a power cycle, six times over two days. The lesson taken from it was
//! to check the frame **immediately before it leaves**, not only where it was
//! built, because those are different moments and only the later one is the one
//! that matters.
//!
//! So [`verify_before_send`] re-derives the framing from the bytes in hand and
//! refuses on any of: not a SysEx dump, not the Syntakt's family, not the
//! pattern dump type, a payload that is not a pattern. A caller cannot skip it:
//! [`send_pattern`] calls it, and the raw sink is not reachable from here with
//! anything else.
//!
//! # What it deliberately does not do
//!
//! No backup, no re-fetch, no read-back. Those belong to the caller, which can
//! see the destination and the diff; this is the twelve inches of cable. What it
//! keeps is the two things a cable can: bytes verified as they leave, and a
//! [`Consent`] that names the slot it was given for, so a message aimed at a
//! different one is refused rather than sent.
//!
//! # No firmware allowlist entry
//!
//! There is none for this box and this module does not add one. A write here is
//! an experiment run by hand with a backup in front of it, not a route the app
//! offers; `core::device::SYNTAKT` is `PatternRoute::RequestReadOnly` and stays
//! that way until a write has been seen to work.

use digi_protocol::protocol::{parse_sysex, SysExKind, FAMILY_SYNTAKT};
use digi_protocol::syntakt_pattern as st;

use crate::MidiError;

/// The dump type that carries a pattern with no kit.
///
/// **Not `0x50`.** That one carries the kit as well, and a write that cannot
/// reach a sound cannot break one. Narrowness is the point.
pub const PATTERN_DUMP: u8 = 0x51;

/// A frame that has been checked and the slot it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaktFrame {
    /// The slot the message would land in.
    pub slot: u8,
    /// How many bytes the unpacked pattern came to.
    pub payload_len: usize,
}

/// Permission to overwrite one slot, naming which.
///
/// A value rather than a bool so it cannot be defaulted into existence, and it
/// carries the slot so a consent given for one destination cannot be spent on
/// another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Consent(u8);

impl Consent {
    /// Record that somebody agreed to overwrite `slot`.
    pub fn given_for(slot: u8) -> Self {
        Self(slot)
    }

    pub fn slot(&self) -> u8 {
        self.0
    }
}

/// Why a send did not happen, or did not finish.
#[derive(Debug)]
pub enum SendError {
    /// These bytes are not a well-formed Syntakt pattern dump. Checked
    /// immediately before the send, not only where the frame was built.
    NotSendable(String),
    /// The consent names a different slot from the message.
    ConsentMismatch { consented: u8, message: u8 },
    /// The wire failed. **The box may now hold a partial message** and its
    /// SysEx API is likely wedged until it is power-cycled.
    Wire(MidiError),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSendable(why) => write!(f, "not a Syntakt pattern dump: {why}"),
            Self::ConsentMismatch { consented, message } => write!(
                f,
                "consent was given for slot {consented}, but the message names slot {message}"
            ),
            Self::Wire(e) => write!(f, "the wire failed mid-send: {e}"),
        }
    }
}

/// Check that `wire` is a Syntakt pattern dump, and say which slot it names.
///
/// Every refusal here is a refusal to put bytes on a wire, so each one names
/// what it saw rather than only what it wanted.
pub fn verify_before_send(wire: &[u8]) -> Result<SyntaktFrame, String> {
    let msg = parse_sysex(wire);
    if msg.kind != SysExKind::Dump {
        return Err(format!("this is a {:?}, not a dump", msg.kind));
    }
    let dump = msg.dump.ok_or_else(|| "the dump carries no header".to_string())?;
    // **The two the parser already computed, and the ones lesson 13 is about.**
    // A body whose checksum or length does not agree with its header is exactly
    // the "body it cannot parse" that took an Analog Four's SysEx API down, and
    // it is free to catch here.
    if !dump.checksum_ok {
        return Err("the checksum does not match the body".to_string());
    }
    if !dump.count_ok {
        return Err("the byte count does not match the body".to_string());
    }
    if dump.family != FAMILY_SYNTAKT {
        return Err(format!(
            "family {:#04x}, not the Syntakt's {FAMILY_SYNTAKT:#04x}",
            dump.family
        ));
    }
    if dump.dump_type != PATTERN_DUMP {
        return Err(format!(
            "dump type {:#04x}, not the pattern's {PATTERN_DUMP:#04x} — {}",
            dump.dump_type,
            if dump.dump_type == 0x50 {
                "that one carries the kit, which this path will not write"
            } else {
                "this path writes patterns and nothing else"
            }
        ));
    }
    if !st::looks_like_pattern(&dump.payload) {
        return Err(format!(
            "the payload is {} bytes and does not announce struct version {} — not a pattern",
            dump.payload.len(),
            st::STRUCT_VERSION
        ));
    }
    Ok(SyntaktFrame { slot: dump.index, payload_len: dump.payload.len() })
}

/// Somewhere bytes can be put. The one method a send needs, so a test can be a
/// `Vec` and nothing here has to know about MIDI ports.
pub trait SyntaktSink {
    fn send_chunk(&mut self, bytes: &[u8]) -> Result<(), MidiError>;
}

impl SyntaktSink for midir::MidiOutputConnection {
    fn send_chunk(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        self.send(bytes).map_err(|e| MidiError::Send(e.to_string()))
    }
}

impl SyntaktSink for Vec<u8> {
    fn send_chunk(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        self.extend_from_slice(bytes);
        Ok(())
    }
}

/// Overwrite one pattern slot on a Syntakt.
///
/// Verifies the frame, checks the consent names the slot the message does, and
/// sends it as one piece — this box is on USB, so there is no DIN pacing to
/// arrange and no partial-frame window to leave open.
///
/// There is no backup and no read-back here; see the module doc.
pub fn send_pattern(
    sink: &mut impl SyntaktSink,
    wire: &[u8],
    consent: Consent,
) -> Result<SyntaktFrame, SendError> {
    let frame = verify_before_send(wire).map_err(SendError::NotSendable)?;
    if consent.slot() != frame.slot {
        return Err(SendError::ConsentMismatch {
            consented: consent.slot(),
            message: frame.slot,
        });
    }
    sink.send_chunk(wire).map_err(SendError::Wire)?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_protocol::protocol::build_dump_message;

    /// A pattern payload that passes `looks_like_pattern`: the version word and
    /// enough bytes after it.
    fn payload() -> Vec<u8> {
        let mut p = vec![0u8; st::PATTERN_BYTES];
        p[..4].copy_from_slice(&st::STRUCT_VERSION.to_be_bytes());
        p
    }

    #[test]
    fn a_pattern_dump_for_the_named_slot_goes_out_whole() {
        let wire = build_dump_message(FAMILY_SYNTAKT, PATTERN_DUMP, 7, &payload());
        let mut sink: Vec<u8> = Vec::new();
        let frame = send_pattern(&mut sink, &wire, Consent::given_for(7)).expect("should send");
        assert_eq!(frame.slot, 7);
        assert_eq!(frame.payload_len, st::PATTERN_BYTES);
        assert_eq!(sink, wire, "the bytes on the wire are the bytes it was given");
    }

    /// The guard that would have caught the A4's own lesson: a frame for another
    /// box must not reach the wire.
    #[test]
    fn another_boxs_frame_is_refused_and_nothing_is_sent() {
        use digi_protocol::protocol::FAMILY_DIGITONE_2;
        let wire = build_dump_message(FAMILY_DIGITONE_2, PATTERN_DUMP, 0, &payload());
        let mut sink: Vec<u8> = Vec::new();
        let err = send_pattern(&mut sink, &wire, Consent::given_for(0)).unwrap_err();
        assert!(matches!(err, SendError::NotSendable(_)), "{err}");
        assert!(sink.is_empty(), "nothing may reach the wire after a refusal");
    }

    /// `0x50` carries the kit. This path writes patterns, so it says no and says
    /// why — a caller reaching for it is reaching for someone's sounds.
    #[test]
    fn the_combined_pattern_and_kit_dump_is_refused() {
        let wire = build_dump_message(FAMILY_SYNTAKT, 0x50, 0, &payload());
        let mut sink: Vec<u8> = Vec::new();
        let err = send_pattern(&mut sink, &wire, Consent::given_for(0)).unwrap_err();
        assert!(err.to_string().contains("carries the kit"), "{err}");
        assert!(sink.is_empty());
    }

    #[test]
    fn a_payload_that_is_not_a_pattern_is_refused() {
        let wire = build_dump_message(FAMILY_SYNTAKT, PATTERN_DUMP, 0, &[0u8; 64]);
        let mut sink: Vec<u8> = Vec::new();
        let err = send_pattern(&mut sink, &wire, Consent::given_for(0)).unwrap_err();
        assert!(err.to_string().contains("not a pattern"), "{err}");
        assert!(sink.is_empty());
    }

    /// Consent is for one slot. Aiming the message somewhere else is exactly the
    /// mistake that overwrites the wrong pattern.
    #[test]
    fn consent_for_one_slot_cannot_send_to_another() {
        let wire = build_dump_message(FAMILY_SYNTAKT, PATTERN_DUMP, 3, &payload());
        let mut sink: Vec<u8> = Vec::new();
        let err = send_pattern(&mut sink, &wire, Consent::given_for(1)).unwrap_err();
        assert!(
            matches!(err, SendError::ConsentMismatch { consented: 1, message: 3 }),
            "{err}"
        );
        assert!(sink.is_empty());
    }

    #[test]
    fn bytes_that_are_not_a_message_at_all_are_refused() {
        let mut sink: Vec<u8> = Vec::new();
        assert!(send_pattern(&mut sink, &[0xf0, 0x00, 0xf7], Consent::given_for(0)).is_err());
        assert!(send_pattern(&mut sink, &[], Consent::given_for(0)).is_err());
        assert!(sink.is_empty());
    }
}
