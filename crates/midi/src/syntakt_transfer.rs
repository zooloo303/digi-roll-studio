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
//! # Pacing, and why the default is not one call
//!
//! A 31 KB frame handed to CoreMIDI in one `send` lands in microseconds, and on
//! an Analog Four that shape **did nothing at all** — no error, no partial
//! write, no trace. PLAN.md §9 measured it on 2026-08-30 and [`Pacing`] is what
//! came out of it: 256-byte packets spaced to arrive at DIN rate.
//!
//! That finding was not applied here until 2026-09-11, when a controlled pair
//! settled it on this box too: the same 36,294-byte frame, the same
//! destination, minutes apart. **One `send` stored nothing. 142 packets of 256
//! bytes at DIN rate stored every byte.** SYXGRID, an independent Digitone II
//! editor, carries the same control as a user-facing "GAP" in milliseconds,
//! "if a large dump loses trigs in transit".
//!
//! [`Pacing`] is reused from [`crate::a4_transfer`] rather than copied. The name
//! is that box's, the measurement is not: it is a fact about how fast an
//! Elektron box reads its own input, and it was first measured over there.
//!
//! # The destination is the index byte
//!
//! Measured on 2026-09-11 with a control, because the first trial was
//! confounded: a dump whose index read 2 landed in A03 while SYSEX RECEIVE was
//! armed on A02, and A02 came back byte-identical to its pre-write read. A
//! third send with nothing armed at all landed in the slot its index named.
//!
//! **This is the opposite of SYXGRID's finding on a Digitone II**, where the
//! index is descriptive and the armed slot decides; that project removed a
//! destination-slot picker for being "a fiction", and on a Syntakt the picker
//! would have been true. Two gen-2 boxes, two answers, so a third gets neither
//! for free.
//!
//! It means [`Consent`] naming a slot is not decoration here: the slot it names
//! is the slot that gets overwritten.
//!
//! # No firmware allowlist entry
//!
//! There is none for this box and this module does not add one. A write here is
//! an experiment run by hand with a backup in front of it, not a route the app
//! offers. `core::device::SYNTAKT` is `PatternRoute::RequestReadOnly`: a write
//! has now been seen to work, which is the bar for promoting it, but promoting
//! it is a UI change with its own consent surface and it is not this module's
//! to make quietly.

use std::time::Duration;

use digi_protocol::protocol::{parse_sysex, SysExKind, FAMILY_SYNTAKT};
use digi_protocol::syntakt_pattern as st;

use crate::a4_transfer::{Pacing, CAN_PACE};
use crate::MidiError;

/// The dump type that carries a pattern with no kit.
///
/// **The box does not store this one.** Every attempt on 2026-09-10 and
/// 2026-09-11 sent it and the Syntakt kept none of it, paced and unpaced, into
/// an armed slot with PATTERN selected and [YES] pressed, with all 128 slots
/// swept afterwards. It is a fine thing to *request* — `0x61` answers with it —
/// and it appears to be nothing the store side accepts.
pub const PATTERN_DUMP: u8 = 0x51;

/// The dump type that carries a pattern **and its kit**, and the one the box
/// stores.
///
/// Measured, not reasoned: on 2026-09-11 the Syntakt's own SETTINGS > SYSEX
/// DUMP > SYSEX SEND with PATTERN selected emitted `0x50`, 36,294 bytes on the
/// wire and 31,744 unpacked. SYSEX SEND and SYSEX RECEIVE are the two halves of
/// one menu, so what one emits is the best evidence of what the other takes.
///
/// **This reaches sounds.** A `0x51` write could not damage a kit because it
/// could not address one; this can. That is why [`Consent`] has to be built for
/// it specifically — see [`Consent::given_for_pattern_and_kit`].
pub const PATTERN_KIT_DUMP: u8 = 0x50;

/// A frame that has been checked and the slot it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaktFrame {
    /// The slot byte the message carries.
    ///
    /// **Not the destination.** A gen-2 box stores into the slot armed in
    /// SETTINGS > SYSEX DUMP > SYSEX RECEIVE and ignores this; it describes
    /// where the dump came from. Measured on a Digitone II by SYXGRID, whose
    /// author removed a destination-slot picker for being "a fiction".
    pub slot: u8,
    /// [`PATTERN_DUMP`] or [`PATTERN_KIT_DUMP`].
    pub dump_type: u8,
    /// How many bytes the unpacked pattern came to.
    pub payload_len: usize,
}

/// Permission to overwrite one slot, naming which.
///
/// A value rather than a bool so it cannot be defaulted into existence, and it
/// carries the slot so a consent given for one destination cannot be spent on
/// another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Consent {
    slot: u8,
    kit: bool,
}

impl Consent {
    /// Somebody agreed to overwrite the pattern in `slot`, and nothing else.
    ///
    /// This will not carry a [`PATTERN_KIT_DUMP`]. Reaching a sound is a
    /// separate decision and it has a separate constructor.
    pub fn given_for(slot: u8) -> Self {
        Self { slot, kit: false }
    }

    /// Somebody agreed to overwrite the pattern in `slot` **and its kit**.
    ///
    /// The longer name is the interlock. There is no flag to flip and no
    /// default to inherit: a caller that reaches a sound had to type this.
    pub fn given_for_pattern_and_kit(slot: u8) -> Self {
        Self { slot, kit: true }
    }

    pub fn slot(&self) -> u8 {
        self.slot
    }

    /// Whether this consent covers the kit as well as the pattern.
    pub fn covers_kit(&self) -> bool {
        self.kit
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
    /// The message carries a kit and the consent was for a pattern alone.
    KitNotConsented,
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
            Self::KitNotConsented => write!(
                f,
                "this is a {PATTERN_KIT_DUMP:#04x} and carries a kit; the consent was for a pattern alone"
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
    if dump.dump_type != PATTERN_DUMP && dump.dump_type != PATTERN_KIT_DUMP {
        return Err(format!(
            "dump type {:#04x}, neither {PATTERN_DUMP:#04x} nor {PATTERN_KIT_DUMP:#04x} — this path writes patterns and nothing else",
            dump.dump_type
        ));
    }
    if !st::looks_like_pattern(&dump.payload) {
        return Err(format!(
            "the payload is {} bytes and does not announce struct version {} — not a pattern",
            dump.payload.len(),
            st::STRUCT_VERSION
        ));
    }
    Ok(SyntaktFrame {
        slot: dump.index,
        dump_type: dump.dump_type,
        payload_len: dump.payload.len(),
    })
}

/// Somewhere bytes can be put. The one method a send needs, so a test can be a
/// `Vec` and nothing here has to know about MIDI ports.
pub trait SyntaktSink {
    fn send_chunk(&mut self, bytes: &[u8]) -> Result<(), MidiError>;

    /// Hold off between packets, and report how long that actually took.
    ///
    /// Returned rather than assumed, because a sleep is a request and the number
    /// that matters is the one the scheduler granted.
    fn pace(&mut self, gap: Duration) -> Duration {
        let t = std::time::Instant::now();
        std::thread::sleep(gap);
        t.elapsed()
    }
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

    /// A test must be able to exercise the paced path without waiting ten
    /// seconds for it, so the one sink with no wire does not sleep.
    fn pace(&mut self, _gap: Duration) -> Duration {
        Duration::ZERO
    }
}

/// Overwrite one pattern slot on a Syntakt.
///
/// Verifies the frame, checks the consent names the slot the message does, and
/// delivers it at `pacing`.
///
/// **`pacing` is not politeness.** [`Pacing::single`] is the shape that did
/// nothing on an Analog Four, silently, and it is the shape every Syntakt
/// attempt on 2026-09-10 used. See the module doc.
///
/// There is no backup and no read-back here; see the module doc.
pub fn send_pattern(
    sink: &mut impl SyntaktSink,
    wire: &[u8],
    consent: Consent,
    pacing: Pacing,
) -> Result<SyntaktFrame, SendError> {
    let frame = verify_before_send(wire).map_err(SendError::NotSendable)?;
    if consent.slot() != frame.slot {
        return Err(SendError::ConsentMismatch {
            consented: consent.slot(),
            message: frame.slot,
        });
    }
    if frame.dump_type == PATTERN_KIT_DUMP && !consent.covers_kit() {
        return Err(SendError::KitNotConsented);
    }
    let pacing = pacing.resolve(CAN_PACE);
    if pacing.chunk == 0 {
        sink.send_chunk(wire).map_err(SendError::Wire)?;
        return Ok(frame);
    }
    for (n, piece) in wire.chunks(pacing.chunk).enumerate() {
        if n > 0 {
            sink.pace(pacing.gap);
        }
        sink.send_chunk(piece).map_err(SendError::Wire)?;
    }
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
        let frame = send_pattern(&mut sink, &wire, Consent::given_for(7), Pacing::single()).expect("should send");
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
        let err = send_pattern(&mut sink, &wire, Consent::given_for(0), Pacing::din()).unwrap_err();
        assert!(matches!(err, SendError::NotSendable(_)), "{err}");
        assert!(sink.is_empty(), "nothing may reach the wire after a refusal");
    }

    /// `0x50` carries the kit, and an ordinary consent does not. A caller that
    /// reaches for it with [`Consent::given_for`] is reaching for someone's
    /// sounds without having said so.
    #[test]
    fn a_kit_dump_needs_a_consent_that_says_kit() {
        let wire = build_dump_message(FAMILY_SYNTAKT, PATTERN_KIT_DUMP, 0, &payload());
        let mut sink: Vec<u8> = Vec::new();
        let err = send_pattern(&mut sink, &wire, Consent::given_for(0), Pacing::din()).unwrap_err();
        assert!(matches!(err, SendError::KitNotConsented), "{err}");
        assert!(sink.is_empty(), "nothing may reach the wire after a refusal");

        let frame =
            send_pattern(&mut sink, &wire, Consent::given_for_pattern_and_kit(0), Pacing::din())
                .expect("the kit consent should carry it");
        assert_eq!(frame.dump_type, PATTERN_KIT_DUMP);
        assert_eq!(sink, wire);
    }

    /// The kit consent is a superset, not a different path: it still carries an
    /// ordinary pattern, and it still will not cross slots.
    #[test]
    fn the_kit_consent_is_still_bound_to_one_slot() {
        let wire = build_dump_message(FAMILY_SYNTAKT, PATTERN_KIT_DUMP, 4, &payload());
        let mut sink: Vec<u8> = Vec::new();
        let err = send_pattern(
            &mut sink,
            &wire,
            Consent::given_for_pattern_and_kit(5),
            Pacing::din(),
        )
        .unwrap_err();
        assert!(
            matches!(err, SendError::ConsentMismatch { consented: 5, message: 4 }),
            "{err}"
        );
        assert!(sink.is_empty());
    }

    #[test]
    fn a_payload_that_is_not_a_pattern_is_refused() {
        let wire = build_dump_message(FAMILY_SYNTAKT, PATTERN_DUMP, 0, &[0u8; 64]);
        let mut sink: Vec<u8> = Vec::new();
        let err = send_pattern(&mut sink, &wire, Consent::given_for(0), Pacing::din()).unwrap_err();
        assert!(err.to_string().contains("not a pattern"), "{err}");
        assert!(sink.is_empty());
    }

    /// Consent is for one slot. Aiming the message somewhere else is exactly the
    /// mistake that overwrites the wrong pattern.
    #[test]
    fn consent_for_one_slot_cannot_send_to_another() {
        let wire = build_dump_message(FAMILY_SYNTAKT, PATTERN_DUMP, 3, &payload());
        let mut sink: Vec<u8> = Vec::new();
        let err = send_pattern(&mut sink, &wire, Consent::given_for(1), Pacing::din()).unwrap_err();
        assert!(
            matches!(err, SendError::ConsentMismatch { consented: 1, message: 3 }),
            "{err}"
        );
        assert!(sink.is_empty());
    }

    #[test]
    fn bytes_that_are_not_a_message_at_all_are_refused() {
        let mut sink: Vec<u8> = Vec::new();
        assert!(send_pattern(&mut sink, &[0xf0, 0x00, 0xf7], Consent::given_for(0), Pacing::din()).is_err());
        assert!(send_pattern(&mut sink, &[], Consent::given_for(0), Pacing::din()).is_err());
        assert!(sink.is_empty());
    }

    /// The paced path must deliver the same bytes in the same order as the
    /// unpaced one. A frame reassembled wrong is a body the box cannot parse,
    /// and DEVELOPMENT.md lesson 13 is what that costs.
    #[test]
    fn pacing_changes_the_packets_and_not_the_bytes() {
        let wire = build_dump_message(FAMILY_SYNTAKT, PATTERN_DUMP, 2, &payload());
        let mut burst: Vec<u8> = Vec::new();
        let mut paced: Vec<u8> = Vec::new();
        send_pattern(&mut burst, &wire, Consent::given_for(2), Pacing::single()).expect("burst");
        send_pattern(&mut paced, &wire, Consent::given_for(2), Pacing::din()).expect("paced");
        assert_eq!(burst, wire);
        assert_eq!(paced, wire, "chunking must not reorder or drop a byte");
        assert!(Pacing::din().packets(wire.len()) > 1, "this frame is worth pacing");
    }
}
