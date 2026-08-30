// An Elektron box on a real pair of MIDI ports: the identity handshake, the dump
// read paths, and — since 2026-08-18 — the one write path. Ported from
// `js/elektron/device.js`.
//
// The JS keeps a pending-request map because the browser hands it callbacks.
// Here the driver thread pushes reassembled frames down a channel and the
// caller blocks on it, so a request is a send followed by a bounded read of
// replies until one matches. Same semantics, no shared mutable state.
//
// **This file used to say "read paths only" and no longer can.** That claim was
// true until `store_pattern_kit` landed, and it is the sentence to read before
// anything else here: this crate can now store a pattern on a box. Every *fetch*
// still goes through `assert_request_opcode`, so no read path can write by
// accident (the one deliberate exception to the guard's range is the whole-project
// request, 0x6f, which mirrors the JS) — but `store_pattern_kit` sends an 0x50,
// which is exactly the message that overwrites a slot.
//
// **What keeps that safe is not this file, it is `safe_write_track`.** The only
// public route to a store is `impl PatternIo for ElektronDevice` at the bottom,
// which exists so `digi_protocol::safe_write` can drive it: that function
// re-fetches, backs up, confirms, encodes minimally, sends and verifies, and
// PLAN.md §7 rule 1 is that a write goes through all five of those or none of
// them. Two honest caveats on how strong the narrowing is:
//
// * A caller that imports `PatternIo` itself can reach the store directly, so the
//   private inherent method is a speed bump rather than a gate. Because of that,
//   `store_pattern_kit` re-checks the firmware allowlist itself — see the gate
//   note on the function. A bypass can therefore skip the backup and the verify,
//   but it cannot reach a box whose OS build the format was never verified on.
// * The verify step reads the slot back through this same pair of ports. On a
//   *loopback* port (an IAC bus, a virtual port) our own 0x50 comes straight back
//   at our input, and it is indistinguishable from a box answering a fetch — so a
//   write aimed at a loopback can read back as verified while nothing stored
//   anything. That is a hazard for testing, not for a real box on a real cable.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::thread::sleep;
use std::time::{Duration, Instant};

use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

use digi_protocol::device::{
    assert_request_opcode, identity_from_responses, parse_device_response,
    parse_version_response, DeviceError, DeviceIdentity,
};
use digi_protocol::drive::{
    assert_read_only_file_op, dir_list_args, list_request_args, parse_dir_list,
    close_request_args, open_request_args, parse_close_reply, parse_list_reply,
    parse_open_reply, parse_read_reply, read_request_args, DirEntry, DriveError, ListReply,
    API_DIR_LIST, API_FILE_CLOSE, API_FILE_LIST, API_FILE_OPEN, API_FILE_READ, READ_CHUNK,
};
use digi_protocol::query::{parse_query_reply, query_args, QueryValue, API_QUERY};
use digi_protocol::protocol::{
    build_api_message, build_dump_message, parse_sysex, API_DEVICE, API_RESPONSE, API_VERSION,
    DUMP_KIT_TRACK_SOUND, DUMP_KIT_TRACK_SOUND_REQUEST, DUMP_PATTERN_KIT,
    DUMP_PATTERN_KIT_REQUEST, DUMP_PROJECT_SETTINGS, DUMP_SOUND_REQUEST,
    DUMP_WHOLE_PROJECT_REQUEST, SysExKind,
};
use digi_protocol::sound::{
    decode_dn1_sound, decode_sound_dump, measure_struct_size, SoundError, SOUND_WRAPPER,
};
use digi_protocol::safe_write::{write_gate, PatternIo};

use crate::ports::{resolve_input, resolve_output, PortBinding, CLIENT_NAME};
use crate::sysex_stream::SysExReassembler;
use crate::MidiError;

/// elk-herd uses 5 s with 2 retries.
const REQUEST_TIMEOUT: Duration = Duration::from_millis(5000);
const REQUEST_RETRIES: u32 = 2;
/// Max silence between messages of a dump stream.
const DUMP_STALL: Duration = Duration::from_millis(5000);

/// A +Drive file read starts at chunk **1**, not 0. `seq = 0` is answered with
/// a zero-length body and a differently-shaped header — it is metadata, and
/// treating it as the first chunk ends the read before it starts.
const FIRST_CHUNK_SEQ: u32 = 1;

/// A backstop on the read loop, which is terminated by the box saying "nothing
/// more" rather than by a length this end knows. At `READ_CHUNK` this is far
/// larger than any +Drive file; it exists so a box that never says stop costs a
/// bounded error instead of an unbounded loop.
const MAX_CHUNKS: u32 = 4096;

/// **A single `midir` send cannot carry a whole pattern-kit dump, and failing at
/// it is silent.** Measured on this Mac against a virtual port, 2026-08-18:
/// a send of 65,535 bytes arrives complete, and a send of 65,536 returns
/// `Ok(())` and delivers **nothing at all** — no error, no panic, no partial
/// write. The cause is structural rather than a driver quirk: CoreMIDI carries a
/// packet's length in a `UInt16`, and `midir`'s CoreMIDI backend puts the whole
/// message in one packet, so anything that cannot be described in 16 bits is
/// dropped on the floor by `MIDIPacketListAdd`.
///
/// A framed pattern-kit store message is **127,577 bytes on a DT2** and 114,118 on
/// a DN2 — not marginally over the ceiling but nearly double it — so the oracle's
/// one-shot `output.send(msg)` would have stored nothing on macOS, on every write,
/// while reporting success. The JS never met this: Web MIDI's `send` takes a SysEx of
/// any length and the browser fragments it out of sight.
///
/// So the message goes out in chunks. Splitting a SysEx across sends is legal and
/// was measured too — 125,000 bytes at chunk sizes from 256 to 65,524 bytes all
/// arrived byte-identical and were reassembled into one frame at the far end,
/// which is the same thing `sysex_stream.rs` does for the receive direction.
///
/// 4 KB is not near the limit and is not meant to be: it is the granularity the
/// pacing below is applied at, ~5 ms of box-digest time per chunk, so the rate
/// limit is spent in small steps across the transfer instead of one long sleep at
/// the end of it.
///
/// # Windows wants the opposite of what macOS wants
///
/// **On WinMM this chunking is not merely unnecessary, it is fatal**, and the two
/// platforms' constraints point in opposite directions. `midir`'s WinMM backend
/// decides sysex-versus-short-message by testing the *first byte of each send*:
///
/// ```text
/// if message[0] == 0xF0 { /* midiOutLongMsg, any length */ }
/// else if nbytes > 3 { return Err(SendError::InvalidData(..)) }
/// ```
///
/// Only the first chunk of a split dump begins `0xF0`. Every chunk after it is
/// raw payload, so it takes the second arm and is refused outright — a 127 KB
/// DT2 store would fail on chunk 2 of 31. CoreMIDI has no such dispatch (it
/// packetises whatever bytes it is handed, which is exactly why it has the
/// 16-bit ceiling above), and ALSA encodes through a *stateful* event encoder
/// that accepts sysex continuations, so chunking is correct on both of those and
/// wrong only here.
///
/// So Windows sends the framed message in one call, which is what
/// `midiOutLongMsg` wants and what the JS oracle did all along. **The pacing
/// survives the change unaltered**: one chunk means one `digest_pace` of the
/// whole message followed by the settle, which is precisely the oracle's
/// "send instantly, then wait `len/800 + 100`". The pauses get coarser, not
/// shorter.
///
/// **Verified against a DN2 on 2026-08-21**, from the installed Windows build: a
/// driver does swallow a whole pattern in one `midiOutLongMsg`, so the unchunked
/// path this constant selects works against real hardware. That was the last
/// assumption in the write path taken from `midir` 0.11's source rather than
/// measured.
///
/// **Not the DT2's larger payload, which no WinMM build has sent** — a DT2 store
/// is 127,577 framed bytes against the DN2's smaller one, so the biggest single
/// transfer this code can attempt has still only ever gone out over CoreMIDI.
/// The failure mode if some driver refuses it is loud rather than silent
/// (`paced_send` propagates with `?`, and rule 1 has already taken the backup):
/// a refused write, not a scrambled slot.
#[cfg(not(target_os = "windows"))]
const SEND_CHUNK: usize = 4096;
#[cfg(target_os = "windows")]
const SEND_CHUNK: usize = usize::MAX;

/// The rate elk-herd assumes a DT-family box swallows a dump at, and the rate the
/// JS's post-send delay is computed from (`Math.ceil(msg.length / 800) + 100`).
///
/// The JS sends instantly and then waits that long for the box to digest. Since
/// the message has to be chunked anyway, the same budget is spent *during* the
/// transfer instead: pace each chunk at this rate, then settle. Total wall clock
/// comes out at or just above the oracle's, which is the safe side to miss on —
/// and a box with a small SysEx input buffer is fed rather than flooded.
const DIGEST_BYTES_PER_MS: usize = 800;

/// The JS's trailing `+ 100`: quiet time for the box to commit the slot before
/// anything reads it back. The verify step is the reason this matters — re-reading
/// a slot the box has not finished storing would fail a write that worked.
const SEND_SETTLE: Duration = Duration::from_millis(100);

/// How long to give a box to digest `bytes`, at [`DIGEST_BYTES_PER_MS`].
///
/// `div_ceil` rather than a float divide because the JS is `Math.ceil` and a
/// zero-length wait for a short chunk is not what either of them means.
fn digest_pace(bytes: usize) -> Duration {
    Duration::from_millis(bytes.div_ceil(DIGEST_BYTES_PER_MS) as u64)
}

/// Pace one already-framed message out through `send`, in chunks of `chunk`
/// bytes, then give the box its settling time.
///
/// `send` and `pause` are injected for one reason: this is where the whole write
/// crosses from decision into effect, and a version of it that only ran against a
/// real port would be a loop nothing could check. A recording `send` and a
/// recording `pause` let the chunk sizes, the byte order and the pacing schedule
/// all be asserted in microseconds — including the case that matters most, a send
/// that fails partway through and must not be reported as a write.
///
/// **`chunk` is a parameter rather than a read of [`SEND_CHUNK`] so that both
/// platforms' rules can be tested on either platform.** That constant is now
/// `cfg`-conditional — 4 KB where CoreMIDI's 16-bit packet length binds,
/// unlimited on WinMM where a chunk not starting `0xF0` is refused — and a rule
/// that only compiles on Windows is a rule this repo's Macs can never run. Every
/// caller in the app passes `SEND_CHUNK`; the tests pass both values.
fn paced_send(
    msg: &[u8],
    chunk: usize,
    mut send: impl FnMut(&[u8]) -> Result<(), MidiError>,
    mut pause: impl FnMut(Duration),
) -> Result<(), MidiError> {
    // `?` rather than a collected error: a dump that stopped halfway is not a
    // smaller write, it is a corrupt slot, and the caller has to hear about it
    // before it re-reads and reports.
    for piece in msg.chunks(chunk) {
        send(piece)?;
        pause(digest_pace(piece.len()));
    }
    pause(SEND_SETTLE);
    Ok(())
}

/// Everything a store decides before it touches a port: may we write to this box
/// at all, and what are the exact bytes.
///
/// A free function taking the identity rather than a method on `ElektronDevice`,
/// because an `ElektronDevice` cannot exist without two open MIDI ports and every
/// decision here deserves a test that needs neither. Same split as `engine`'s —
/// what decides is kept out of what waits.
fn plan_store(
    identity: Option<&DeviceIdentity>,
    index: u8,
    payload: &[u8],
) -> Result<Vec<u8>, MidiError> {
    // The firmware allowlist, a second time. `safe_write_track` checks this same
    // gate with this same function before any of this runs, so a legitimate write
    // passes both and nothing changes for it. The copy is here for the route that
    // skips the flow: `PatternIo` is public, so a caller can reach the store
    // without the backup or the verify, and the one rule that has to survive that
    // is the one about OS builds the format was never verified against. A bypass
    // can lose the backup; it must not be able to put unverified-format bytes in a
    // slot, because that is the failure no backup was taken for.
    let gate = write_gate(identity);
    if !gate.ok {
        return Err(MidiError::WriteRefused(gate.reason));
    }
    let family = identity.and_then(|i| i.family).ok_or_else(|| {
        MidiError::Protocol(DeviceError::UnknownFamily(
            identity.map(|i| i.name.clone()).unwrap_or_else(|| "this device".into()),
        ))
    })?;
    // The only 0x5n this crate ever builds. There is no "write request" in the
    // protocol — you send an unsolicited dump *response* and the box stores it in
    // that slot — which is why the read path's `assert_request_opcode` has no
    // mirror worth writing here: the opcode is not a parameter, so there is
    // nothing for a guard to refuse.
    Ok(build_dump_message(family, DUMP_PATTERN_KIT, index, payload))
}

/// The number of tracks a kit addresses, and the bound on a `0x5b` index.
///
/// Sixteen because that is what `0x6b` answers for and nothing more: indices 0
/// through 15 were read back on a DT2 and a DN2 and matched Overbridge's pane
/// in order. What index 16 does is unknown, and an unknown index under a store
/// opcode is the one experiment nobody should run by accident.
pub const KIT_TRACKS: u8 = 16;

/// Everything a per-track sound store decides before it touches a port.
///
/// The sibling of [`plan_store`], and deliberately its shape rather than a
/// generalisation of it: the opcode is a constant here too, so no caller can
/// choose which `0x5n` goes out. Same reasoning as `plan_store`'s — a guard can
/// only refuse a parameter, so the safest store is one whose opcode is not one.
///
/// # The three checks, and why they outlived the hypothesis
///
/// [`DUMP_KIT_TRACK_SOUND`] was derived from the response-is-request-minus-0x10
/// rule and nothing else, so it might not have been a store at all. It is one —
/// confirmed on a DT2 and a DN2, see [`ElektronDevice::store_kit_track_sound`].
///
/// The checks stay exactly as they were written, and the argument for each is
/// the stronger now that this is a *shipping* path rather than a probe: what
/// they guard against is not a wrong opcode but a wrong payload, and a user
/// double-clicking a preset can reach this far more often than an afternoon of
/// probing ever did. Three things are checked before any of it reaches a cable,
/// in the order that a wrong payload would break them:
///
/// 1. **The firmware allowlist**, via the same [`write_gate`] `plan_store`
///    uses. An unverified OS build is the case no backup was taken for, and
///    that argument does not get weaker because the message is a probe.
/// 2. **The track index**, against [`KIT_TRACKS`].
/// 3. **That the payload contains a decodable sound struct**, at offset 0 or
///    behind the 5-byte wrapper `0x6b` returns. The magic foot is what makes
///    this worth doing (see [`decode_sound_dump`]): it does not validate at the
///    wrong size, so "this is a sound" is checked rather than assumed. Sending
///    bytes we cannot decode under a store opcode is how a stray slice turns
///    into a corrupted kit.
///
///    **This check was silently refusing good payloads until 2026-08-29**, and
///    it is worth knowing why, because the failure was invisible: it read as
///    "not a sound struct" and there was nothing to see. `decode_sound_dump`
///    recovered a struct's size from a table, and roughly half a DN2's own
///    presets are 319 bytes — a size no table here ever had. It measures the
///    foot now. A guard that refuses what it cannot parse is only as honest as
///    its parser.
fn plan_track_sound_store(
    identity: Option<&DeviceIdentity>,
    track: u8,
    payload: &[u8],
) -> Result<Vec<u8>, MidiError> {
    let gate = write_gate(identity);
    if !gate.ok {
        return Err(MidiError::WriteRefused(gate.reason));
    }
    if track >= KIT_TRACKS {
        return Err(MidiError::WriteRefused(format!(
            "track {track} is outside a kit's 0-{} tracks",
            KIT_TRACKS - 1
        )));
    }
    // Either shape decodes: the wrapper is what `0x6b` hands back and what the
    // store turned out to want, and the bare struct is kept accepted because
    // the probe tried both and this guard should not be the thing that decides.
    //
    // **A Digitone mk1 struct counts too, and that was a conflation worth
    // untangling.** This check asks one question — *are these bytes a sound* —
    // and until 2026-08-29 it answered it with a decoder that knew one head
    // magic, so it also silently answered a second question it was never asked:
    // *is this a format this box's kit takes*. Those are different, and the
    // second belongs to `drive::preset_load_payload`, which is where a caller
    // can see it and reason about it. An mk1 payload validates exactly as
    // strongly as a native one — head `DN1S` at +0 and [`SOUND_MAGIC_FOOT`] at
    // its measured end, both checked by [`decode_dn1_sound`] — so refusing it
    // *here* was never about the bytes being unsafe.
    //
    // Nothing in the app reaches this with one: the load path refuses mk1
    // presets a layer above, deliberately, until a box says what it does with
    // them. What this unblocks is the probe that asks — `probe_mk1_store`.
    let sound_at = |bytes: &[u8]| -> Result<(), SoundError> {
        decode_sound_dump(bytes).map(|_| ()).or_else(|digi| {
            match measure_struct_size(bytes) {
                Some(size) => decode_dn1_sound(bytes, size).map(|_| ()).map_err(|_| digi),
                None => Err(digi),
            }
        })
    };
    let wrapped = payload.get(SOUND_WRAPPER..).map(sound_at);
    if let Err(bare) = sound_at(payload) {
        if !matches!(wrapped, Some(Ok(_))) {
            return Err(MidiError::WriteRefused(format!(
                "payload is not a sound struct at +0 ({bare}) or +{SOUND_WRAPPER} ({}) \
                 — refusing to send it under a store opcode",
                match wrapped {
                    Some(Err(e)) => e.to_string(),
                    _ => format!("only {} bytes", payload.len()),
                }
            )));
        }
    }
    let family = identity.and_then(|i| i.family).ok_or_else(|| {
        MidiError::Protocol(DeviceError::UnknownFamily(
            identity.map(|i| i.name.clone()).unwrap_or_else(|| "this device".into()),
        ))
    })?;
    Ok(build_dump_message(family, DUMP_KIT_TRACK_SOUND, track, payload))
}

/// One received SysEx frame: the bytes exactly as the box sent them.
type Frame = Vec<u8>;

/// The first message id, and the one the counter comes back to.
///
/// elk-herd starts here to stay clear of Transfer's ids, and this is that number
/// rather than a literal in three places — the constructor, the wrap, and the
/// test that pins the wrap.
const FIRST_MSG_ID: u16 = 20000;

/// The id after `current`: one more, or back to [`FIRST_MSG_ID`] at the top of
/// the range.
///
/// **A free function so the rule lives once.** It was two lines of arithmetic
/// inside `take_msg_id` and the same two lines copied into a test that said it
/// "mirrors `take_msg_id`" — which is `DEVELOPMENT.md` lesson 5 exactly: a copy
/// can be right about a rule that has since changed. The test now calls this.
///
/// `checked_add` rather than `>= 0xffff`, which is what it said before and what
/// clippy refuses: on a `u16` that comparison can only mean `== u16::MAX`, so it
/// read as guarding a range when it was guarding an overflow. Same ids, in the
/// same order — the behaviour is elk-herd's and is not up for renegotiation.
fn next_msg_id(current: u16) -> u16 {
    match current.checked_add(1) {
        Some(next) => next,
        None => FIRST_MSG_ID,
    }
}


/// A dump message as received, keeping the original bytes. An unknown box's
/// version bytes and framing are evidence, so captures keep the box's own
/// encoding rather than a re-encoding of the payload.
#[derive(Debug, Clone)]
pub struct DumpResponse {
    pub family: u8,
    pub dump_type: u8,
    pub index: u8,
    pub payload: Vec<u8>,
    pub raw: Vec<u8>,
}

pub struct ElektronDevice {
    conn_out: MidiOutputConnection,
    rx: Receiver<Frame>,
    /// Held so the input connection stays open; dropping it closes the port.
    _conn_in: MidiInputConnection<CallbackState>,
    next_msg_id: u16,
    pub identity: Option<DeviceIdentity>,
    pub port_name: String,
}

struct CallbackState {
    reassembler: SysExReassembler,
    tx: Sender<Frame>,
}

impl ElektronDevice {
    /// Open a box by remembered binding. `input` and `output` are the two
    /// directions of the same device.
    pub fn open(input: &PortBinding, output: &PortBinding) -> Result<Self, MidiError> {
        let mut midi_in = MidiInput::new(CLIENT_NAME)?;
        // Without this the driver filters SysEx out and nothing here works.
        midi_in.ignore(Ignore::None);
        let midi_out = MidiOutput::new(CLIENT_NAME)?;

        let in_port = resolve_input(&midi_in, input)
            .ok_or_else(|| MidiError::PortNotFound(input.name.clone()))?;
        let out_port = resolve_output(&midi_out, output)
            .ok_or_else(|| MidiError::PortNotFound(output.name.clone()))?;

        let port_name = midi_in.port_name(&in_port).unwrap_or_else(|_| input.name.clone());
        let (tx, rx) = channel();

        let conn_in = midi_in
            .connect(
                &in_port,
                CLIENT_NAME,
                |_stamp, bytes, state: &mut CallbackState| {
                    for frame in state.reassembler.push(bytes) {
                        // A closed receiver means the device was dropped; the
                        // callback outliving it is normal, so this is not an error.
                        let _ = state.tx.send(frame);
                    }
                },
                CallbackState { reassembler: SysExReassembler::new(), tx },
            )
            .map_err(|e| MidiError::Connect(e.to_string()))?;

        let conn_out = midi_out
            .connect(&out_port, CLIENT_NAME)
            .map_err(|e| MidiError::Connect(e.to_string()))?;

        Ok(Self {
            conn_out,
            rx,
            _conn_in: conn_in,
            next_msg_id: FIRST_MSG_ID,
            identity: None,
            port_name,
        })
    }

    fn take_msg_id(&mut self) -> u16 {
        let id = self.next_msg_id;
        self.next_msg_id = next_msg_id(id);
        id
    }

    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        self.conn_out.send(bytes).map_err(|e| MidiError::Send(e.to_string()))
    }

    /// Drop anything already queued, so a reply to an abandoned request cannot
    /// be mistaken for the answer to the next one.
    fn drain(&self) {
        while self.rx.try_recv().is_ok() {}
    }

    fn request_once(&mut self, api_id: u8, args: &[u8]) -> Result<Vec<u8>, MidiError> {
        self.drain();
        let msg_id = self.take_msg_id();
        self.send(&build_api_message(msg_id, api_id, args, 0))?;

        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(MidiError::Timeout);
            }
            let frame = match self.rx.recv_timeout(remaining) {
                Ok(f) => f,
                Err(RecvTimeoutError::Timeout) => return Err(MidiError::Timeout),
                Err(RecvTimeoutError::Disconnected) => return Err(MidiError::Disconnected),
            };
            let msg = parse_sysex(&frame);
            if msg.kind != SysExKind::Api {
                continue;
            }
            if let Some(api) = msg.api {
                // Responses come back as request opcode + 0x80, tagged with the
                // msgId they answer.
                if api.resp_id == msg_id && api.api_id == api_id.wrapping_add(API_RESPONSE) {
                    return Ok(api.args);
                }
            }
        }
    }

    fn request(&mut self, api_id: u8, args: &[u8]) -> Result<Vec<u8>, MidiError> {
        let mut last = MidiError::Timeout;
        for _ in 0..=REQUEST_RETRIES {
            match self.request_once(api_id, args) {
                Ok(v) => return Ok(v),
                Err(e @ MidiError::Disconnected) => return Err(e),
                Err(e) => last = e,
            }
        }
        Err(MidiError::NoReply { api_id, tries: REQUEST_RETRIES + 1, last: Box::new(last) })
    }

    /// Handshake: who are you, what OS? Stores and returns the identity.
    pub fn identify(&mut self) -> Result<DeviceIdentity, MidiError> {
        let dev_args = self.request(API_DEVICE, &[])?;
        let dev = parse_device_response(&dev_args)?;
        let ver_args = self.request(API_VERSION, &[])?;
        let (build, version) = parse_version_response(&ver_args)?;

        let identity = identity_from_responses(&dev, build, version);
        self.identity = Some(identity.clone());
        Ok(identity)
    }

    fn family(&self) -> Result<u8, MidiError> {
        let id = self.identity.as_ref();
        id.and_then(|i| i.family).ok_or_else(|| {
            MidiError::Protocol(DeviceError::UnknownFamily(
                id.map(|i| i.name.clone()).unwrap_or_else(|| "this device".into()),
            ))
        })
    }

    /// Fetch one dump of any type from any family: one 0x6n request → one 0x5n
    /// response (the response opcode is always the request minus 0x10).
    ///
    /// `family` and `request_type` are explicit rather than read off the
    /// identity, because for an unmapped box there is nothing on the identity
    /// to read.
    pub fn fetch_dump(
        &mut self,
        family: u8,
        request_type: u8,
        index: u8,
    ) -> Result<DumpResponse, MidiError> {
        assert_request_opcode(request_type)?;
        let response_type = request_type - 0x10;
        self.drain();
        self.send(&build_dump_message(family, request_type, index, &[]))?;

        let deadline = Instant::now() + DUMP_STALL;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(MidiError::Timeout);
            }
            let frame = match self.rx.recv_timeout(remaining) {
                Ok(f) => f,
                Err(RecvTimeoutError::Timeout) => return Err(MidiError::Timeout),
                Err(RecvTimeoutError::Disconnected) => return Err(MidiError::Disconnected),
            };
            let msg = parse_sysex(&frame);
            let Some(dump) = msg.dump else { continue };
            if dump.family != family || dump.dump_type != response_type || dump.index != index {
                continue;
            }
            if !dump.checksum_ok || !dump.count_ok {
                return Err(MidiError::CorruptDump { dump_type: dump.dump_type, index });
            }
            return Ok(DumpResponse {
                family: dump.family,
                dump_type: dump.dump_type,
                index: dump.index,
                payload: dump.payload,
                raw: frame,
            });
        }
    }

    /// Fetch one dump, like [`Self::fetch_dump`], but with a non-empty request
    /// payload.
    ///
    /// `fetch_dump` always sends an empty payload, which is enough to address the
    /// 128 dump-addressable slots a request's one `index` byte reaches. The
    /// +Drive is bigger than that — banks A–H times 128+ slots each — so if a
    /// dump request is how it is reached at all, a bank/slot argument has to
    /// travel somewhere, and the only somewhere a dump *request* message has
    /// spare room for is the payload before the checksum trailer
    /// (`build_dump_message`'s `payload` argument, always `&[]` until now).
    ///
    /// Goes through the same `assert_request_opcode` guard as `fetch_dump` — the
    /// point of that guard is that no *fetch* can send a store opcode, and that
    /// has to hold regardless of what travels in the payload.
    pub fn fetch_dump_with_args(
        &mut self,
        family: u8,
        request_type: u8,
        index: u8,
        args: &[u8],
    ) -> Result<DumpResponse, MidiError> {
        assert_request_opcode(request_type)?;
        let response_type = request_type - 0x10;
        self.drain();
        self.send(&build_dump_message(family, request_type, index, args))?;

        let deadline = Instant::now() + DUMP_STALL;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(MidiError::Timeout);
            }
            let frame = match self.rx.recv_timeout(remaining) {
                Ok(f) => f,
                Err(RecvTimeoutError::Timeout) => return Err(MidiError::Timeout),
                Err(RecvTimeoutError::Disconnected) => return Err(MidiError::Disconnected),
            };
            let msg = parse_sysex(&frame);
            let Some(dump) = msg.dump else { continue };
            if dump.family != family || dump.dump_type != response_type || dump.index != index {
                continue;
            }
            if !dump.checksum_ok || !dump.count_ok {
                return Err(MidiError::CorruptDump { dump_type: dump.dump_type, index });
            }
            return Ok(DumpResponse {
                family: dump.family,
                dump_type: dump.dump_type,
                index: dump.index,
                payload: dump.payload,
                raw: frame,
            });
        }
    }

    /// One pattern-kit dump (0x60 request → 0x50 response) from the identified box.
    pub fn fetch_pattern_kit(&mut self, index: u8) -> Result<Vec<u8>, MidiError> {
        let family = self.family()?;
        Ok(self.fetch_dump(family, DUMP_PATTERN_KIT_REQUEST, index)?.payload)
    }

    /// List one +Drive directory (`0x10` request → `0x90` response).
    ///
    /// The API path, not the dump path: this is how anything beyond the
    /// project's 128 dump-addressable slots gets reached. Read-only — `0x10` is
    /// the only `0x1n` opcode this crate implements, and the mutating ones
    /// (`0x11` DirCreate, `0x12` DirDelete, `0x20` FileDelete) are absent by
    /// choice, so no code path here can alter a +Drive.
    ///
    /// Ask [`DeviceIdentity::supported_ids`] before calling: a box that does not
    /// list `0x10` will simply not answer, and the retry loop will spend
    /// `REQUEST_TIMEOUT` finding that out.
    pub fn dir_list(&mut self, path: &str) -> Result<Vec<DirEntry>, MidiError> {
        let args = dir_list_args(path).map_err(|e| MidiError::Send(e.to_string()))?;
        let reply = self.request(API_DIR_LIST, &args)?;
        parse_dir_list(&reply).map_err(|e| MidiError::Send(e.to_string()))
    }

    /// Read one Query key (`0x09` request → `0x89` response).
    ///
    /// The API path's other read, alongside [`Self::dir_list`]: a flat
    /// key→value namespace rather than a directory tree. Only the request and
    /// reply are handled here — the key space itself is not documented
    /// anywhere in this crate, so this is a probe primitive, not a lookup for a
    /// known key.
    ///
    /// A reply of [`QueryValue::None`] still means the key was *answered* —
    /// that is a weaker claim than "exists" but a much stronger one than a
    /// timeout, which is what an unrecognised key is expected to produce.
    pub fn query(&mut self, key: &str) -> Result<QueryValue, MidiError> {
        let args = query_args(key).map_err(|e| MidiError::Send(e.to_string()))?;
        let reply = self.request(API_QUERY, &args)?;
        parse_query_reply(&reply).map_err(|e| MidiError::Send(e.to_string()))
    }

    /// List one +Drive directory through the **Digitone** file API (`0x53` List
    /// → `0xD3`), the renumbering a DN1/DN2 answers.
    ///
    /// Not the same call as [`ElektronDevice::dir_list`], which speaks elk-herd's
    /// gen-1 `0x10` numbering that a DT2 answers. A DN2 answers this one and not
    /// that one; the two are different opcode spaces for the same feature.
    ///
    /// `start = 0, count = 0` lists everything. To page, pass the
    /// [`ListReply::next_cursor`] from the previous reply — **not** an index of
    /// your own choosing, which returns zero entries.
    ///
    /// Read-only, and checked: the opcode goes through
    /// [`assert_read_only_file_op`], which admits only List/Open/Read/Close. In
    /// this namespace `0x5C` deletes, so the dump path's "0x5n cannot be sent"
    /// habit is not a safety property here and this guard is what replaces it.
    pub fn drive_list(
        &mut self,
        path: &str,
        start: u32,
        count: u32,
    ) -> Result<ListReply, MidiError> {
        assert_read_only_file_op(API_FILE_LIST).map_err(|e| MidiError::Send(e.to_string()))?;
        let args = list_request_args(path, start, count)
            .map_err(|e| MidiError::Send(e.to_string()))?;
        let reply = self.request(API_FILE_LIST, &args)?;
        parse_list_reply(&reply).map_err(|e| MidiError::Send(e.to_string()))
    }

    /// Send one read-only file-API opcode with caller-supplied argument bytes,
    /// and hand the reply body back **undecoded**.
    ///
    /// This exists because `0x54` Open, `0x55` Read and `0x56` Close are named
    /// by the source document and their argument layouts are not — only List's
    /// body is written down. So the layout has to be derived from a real reply
    /// before a parser can be written, and deriving it needs a way to put bytes
    /// on the wire and see bytes come back. `browse_drive_dn.rs` did exactly
    /// this for the entry layout; `probe_drive_read.rs` is its counterpart here.
    ///
    /// **Read-only, by the same guard and not by a weaker one.** `api_id` goes
    /// through [`assert_read_only_file_op`], which admits List/Open/Read/Close
    /// and nothing else — so this cannot send `0x57`/`0x58`/`0x59` WriteOpen/
    /// Write/WriteClose, `0x5A` Move, `0x5B` Copy or `0x5C` **Delete**, however
    /// it is called. The argument bytes are free; the opcode is not, and the
    /// opcode is what decides whether a +Drive changes.
    ///
    /// Prefer a named method once a layout is known. A raw primitive is the
    /// right tool for one session and the wrong one for a caller in the app.
    pub fn drive_file_request(&mut self, api_id: u8, args: &[u8]) -> Result<Vec<u8>, MidiError> {
        assert_read_only_file_op(api_id).map_err(|e| MidiError::Send(e.to_string()))?;
        self.request(api_id, args)
    }

    /// Read one whole file off the +Drive: `0x54` Open, `0x55` Read until the
    /// end, `0x56` Close. Returns the file's bytes, header and all.
    ///
    /// Read-only, and by the same guard as [`Self::drive_list`] rather than a
    /// weaker one — every opcode goes through [`assert_read_only_file_op`], so
    /// no path through this function can send `0x57`/`0x58`/`0x59`, `0x5A`,
    /// `0x5B` or `0x5C` **Delete**.
    ///
    /// # What this checks, and why each check is here
    ///
    /// A read that half-works is the failure this API makes easy: the box
    /// answers each Read independently, so a dropped chunk yields a shorter
    /// file rather than an error. Three things guard against believing one.
    ///
    /// * **The sequence number is checked on every chunk**, not assumed. A
    ///   chunk under the wrong number would assemble a plausible, wrong file.
    /// * **Close is the completion signal.** It refuses with "Reader did not
    ///   complete" until the read reaches the end, so a Close that succeeds is
    ///   the box agreeing the file was read whole — which is worth more than
    ///   this end's own count.
    /// * **The box's total is compared with what was assembled.** Close reports
    ///   the length it sent; a disagreement is [`DriveError::ShortRead`] and the
    ///   bytes are refused rather than returned.
    ///
    /// A box runs **one transfer job at a time** — a second Open voids the
    /// first — so two of these cannot be interleaved on one device. `&mut self`
    /// is what enforces that, and it is not incidental.
    pub fn drive_read_file(&mut self, path: &str) -> Result<Vec<u8>, MidiError> {
        let drive_err = |e: DriveError| MidiError::Send(e.to_string());

        assert_read_only_file_op(API_FILE_OPEN).map_err(drive_err)?;
        let args = open_request_args(path, Some(READ_CHUNK)).map_err(drive_err)?;
        let open = parse_open_reply(&self.request(API_FILE_OPEN, &args)?).map_err(drive_err)?;

        // Read to the end. The loop ends on an empty chunk rather than on a
        // computed count: the file's length is not known here — the listing has
        // it and this function deliberately does not take it — so the box's own
        // "nothing more" is the terminator.
        assert_read_only_file_op(API_FILE_READ).map_err(drive_err)?;
        let mut file: Vec<u8> = Vec::new();
        let mut seq: u32 = FIRST_CHUNK_SEQ;
        loop {
            let args = read_request_args(open.fd, seq);
            let reply = match parse_read_reply(&self.request(API_FILE_READ, &args)?) {
                Ok(reply) => reply,
                // End of file. The box refuses a read past the last chunk
                // rather than answering an empty one, so this is the normal
                // way a read finishes when the file is an exact multiple of
                // the chunk size.
                Err(DriveError::TransferComplete) => break,
                Err(e) => return Err(drive_err(e)),
            };
            if reply.seq != seq {
                return Err(drive_err(DriveError::SequenceOutOfOrder {
                    expected: seq,
                    got: reply.seq,
                }));
            }
            let short = reply.data.len() < open.chunk as usize;
            file.extend_from_slice(&reply.data);
            // A chunk shorter than the one asked for is the last one. Stopping
            // here rather than reading on is what keeps the common case to a
            // single Read: every preset seen fits inside one 4 KB chunk, and
            // asking for a second is refused, not answered.
            if short {
                break;
            }
            seq += 1;
            if seq > MAX_CHUNKS {
                return Err(MidiError::Send(format!(
                    "{path}: still reading after {MAX_CHUNKS} chunks — refusing to loop"
                )));
            }
        }

        assert_read_only_file_op(API_FILE_CLOSE).map_err(drive_err)?;
        let close = parse_close_reply(&self.request(API_FILE_CLOSE, &close_request_args(open.fd))?)
            .map_err(drive_err)?;
        if close.total_len as usize != file.len() {
            return Err(drive_err(DriveError::ShortRead {
                expected: close.total_len,
                got: file.len(),
            }));
        }
        Ok(file)
    }

    /// As [`Self::drive_file_request`], but collect **every** reply the box
    /// sends rather than the first one.
    ///
    /// [`Self::request`] returns on the first message whose `respId` matches and
    /// then the next call drains the queue, which is correct for every request
    /// this crate had before now — they all answer in one message. A file read
    /// is the first candidate for answering in several, and the difference is
    /// invisible from the caller's side: a header-only reply and the first
    /// message of a longer answer are the same 22 bytes.
    ///
    /// So this returns what arrived, and the *count* is the finding. Same
    /// allowlist, same guarantee — `api_id` is checked before anything is sent.
    ///
    /// `quiet` is how long to wait after the last message before deciding the
    /// box has finished talking.
    pub fn drive_file_request_all(
        &mut self,
        api_id: u8,
        args: &[u8],
        quiet: Duration,
    ) -> Result<Vec<Vec<u8>>, MidiError> {
        assert_read_only_file_op(api_id).map_err(|e| MidiError::Send(e.to_string()))?;
        self.drain();
        let msg_id = self.take_msg_id();
        self.send(&build_api_message(msg_id, api_id, args, 0))?;

        let mut replies = Vec::new();
        let hard_deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            // Before the first reply, allow the full request timeout; after it,
            // only the quiet window — otherwise every read pays the timeout.
            let window = if replies.is_empty() {
                hard_deadline.saturating_duration_since(Instant::now())
            } else {
                quiet
            };
            if window.is_zero() {
                break;
            }
            let frame = match self.rx.recv_timeout(window) {
                Ok(f) => f,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return Err(MidiError::Disconnected),
            };
            let msg = parse_sysex(&frame);
            if msg.kind != SysExKind::Api {
                continue;
            }
            if let Some(api) = msg.api {
                if api.resp_id == msg_id && api.api_id == api_id.wrapping_add(API_RESPONSE) {
                    replies.push(api.args);
                }
            }
        }
        if replies.is_empty() {
            return Err(MidiError::Timeout);
        }
        Ok(replies)
    }

    /// One sound dump (`0x63` request → `0x53` response) from the identified box.
    ///
    /// `index` addresses the **project's sound pool**, which is 128 slots — the
    /// pool Overbridge shows as "PROJECT PRESET POOL". It is not a +Drive
    /// address: the dump request carries a single index byte, so it cannot reach
    /// the +Drive's banks at all. Reading those needs the API path (`0x10`
    /// DirList and friends), which this crate does not implement.
    ///
    /// Read-only, and provably so: `0x63` goes through the same
    /// `assert_request_opcode` as every other fetch, and that guard admits no
    /// opcode that stores.
    pub fn fetch_sound(&mut self, index: u8) -> Result<Vec<u8>, MidiError> {
        let family = self.family()?;
        Ok(self.fetch_dump(family, DUMP_SOUND_REQUEST, index)?.payload)
    }

    /// One track's sound from the box's **active** kit (`0x6b` → `0x5b`), for
    /// `track` in 0–15.
    ///
    /// Not a +Drive address and not a pool slot: this is the kit the box is
    /// playing right now, which is why it is the only read here that changes
    /// its answer when somebody turns a knob. Payload is
    /// [`SOUND_WRAPPER`] bytes then one sound struct — see
    /// [`digi_protocol::sound::decode_sound_dump`], which reads it from
    /// `SOUND_WRAPPER` on.
    ///
    /// Read-only through the same `assert_request_opcode` as every other fetch.
    pub fn fetch_kit_track_sound(&mut self, track: u8) -> Result<Vec<u8>, MidiError> {
        let family = self.family()?;
        Ok(self.fetch_dump(family, DUMP_KIT_TRACK_SOUND_REQUEST, track)?.payload)
    }

    /// Store one sound onto a track of the box's **active** kit — the store
    /// `0x6b`'s existence predicts, **confirmed on hardware and now in use.**
    ///
    /// Read [`plan_track_sound_store`] before calling this; it holds the three
    /// checks and the reasons for them. What that function could not check was
    /// the hypothesis itself: `0x5b` is named by arithmetic on `0x6b`, and
    /// whether a box reads it as a per-track sound store is what
    /// `examples/probe_sound_store.rs` was written to find out.
    ///
    /// **It does.** Positive on a DT2 (0071) on 2026-08-28 and on a DN2 (0050)
    /// on 2026-08-29 — the wrapped payload accepted first try on both, two
    /// agreeing reads and the box's own screen as witnesses, and the original
    /// bytes restoring. So the bar this doc used to set — *nothing in the app
    /// may call this until the probe returns a positive* — is met, and
    /// PLAN.md §10.6 step 6 is what walked through it: `preset_load` is the
    /// caller, and `ui::presets` is the button.
    ///
    /// The A4 has no route here at all and never will: it answers no `0x6x`
    /// request, so there is no `0x6b` for this to mirror. `Product::family` is
    /// `None` for it, and `ui::presets::load_blocker` is where a user is told
    /// so in words.
    ///
    /// # Why this is not `safe_write_tracks`
    ///
    /// It cannot be. Rule 1's ceremony is built on a slot that can be re-fetched
    /// and restored, and the active kit is a working buffer: there is no `0x50`
    /// that puts it back. What makes an active-kit write recoverable is the box
    /// itself — reloading the pattern discards an unsaved kit — and that is a
    /// property of the hardware rather than of a backup this code took. Anything
    /// that ships on top of this has to say so plainly, which is what §10.4's
    /// audition mode is.
    ///
    /// No reply comes back, exactly as for [`Self::store_pattern_kit`], so the
    /// only way to know what happened is to read the track back.
    pub fn store_kit_track_sound(&mut self, track: u8, payload: &[u8]) -> Result<(), MidiError> {
        let msg = plan_track_sound_store(self.identity.as_ref(), track, payload)?;
        let conn = &mut self.conn_out;
        paced_send(
            &msg,
            SEND_CHUNK,
            |chunk| conn.send(chunk).map_err(|e| MidiError::Send(e.to_string())),
            sleep,
        )
    }

    /// Store a pattern-kit in a slot on the box, overwriting whatever is there.
    ///
    /// Port of `js/elektron/device.js`'s `sendPatternKit`. **Private on purpose**:
    /// the public route is `impl PatternIo for ElektronDevice`, which exists for
    /// `digi_protocol::safe_write::safe_write_track` to drive. Call that, not this
    /// — it is the function that holds all five of PLAN.md §7 rule 1's safety
    /// rules, and the module header says how far that narrowing does and does not
    /// go.
    ///
    /// No reply comes back, so there is nothing to wait for except the box: the
    /// message is paced out at [`DIGEST_BYTES_PER_MS`] and then given
    /// [`SEND_SETTLE`] of quiet before the caller re-reads to verify.
    fn store_pattern_kit(&mut self, index: u8, payload: &[u8]) -> Result<(), MidiError> {
        let msg = plan_store(self.identity.as_ref(), index, payload)?;
        let conn = &mut self.conn_out;
        paced_send(
            &msg,
            SEND_CHUNK,
            |chunk| conn.send(chunk).map_err(|e| MidiError::Send(e.to_string())),
            sleep,
        )
    }

    /// Fetch a whole-project dump: one 0x6f request, then the box streams
    /// pattern-kit (0x50) and sound (0x53) responses and finishes with a single
    /// project-settings (0x54) response — the only end-of-stream marker there
    /// is. Returns the raw concatenated SysEx messages: a replayable .syx file.
    pub fn fetch_project_dump(
        &mut self,
        mut on_progress: impl FnMut(usize),
    ) -> Result<Vec<u8>, MidiError> {
        let family = self.family()?;
        self.drain();
        // 0x6f sits outside `assert_request_opcode`'s range by design, so this
        // path names it explicitly rather than widening the guard for everyone.
        self.send(&build_dump_message(family, DUMP_WHOLE_PROJECT_REQUEST, 0, &[]))?;

        let mut out: Vec<u8> = Vec::new();
        let mut count = 0usize;
        let mut deadline = Instant::now() + DUMP_STALL;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(MidiError::DumpStalled { messages: count });
            }
            let frame = match self.rx.recv_timeout(remaining) {
                Ok(f) => f,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(MidiError::DumpStalled { messages: count })
                }
                Err(RecvTimeoutError::Disconnected) => return Err(MidiError::Disconnected),
            };
            let msg = parse_sysex(&frame);
            let Some(dump) = msg.dump else { continue };
            if dump.family != family {
                continue;
            }
            if !dump.checksum_ok || !dump.count_ok {
                return Err(MidiError::CorruptDump { dump_type: dump.dump_type, index: dump.index });
            }
            out.extend_from_slice(&frame);
            count += 1;
            on_progress(count);
            // Each message refreshes the stall window; a full project takes
            // seconds to cross USB-MIDI.
            deadline = Instant::now() + DUMP_STALL;
            if dump.dump_type == DUMP_PROJECT_SETTINGS {
                return Ok(out);
            }
        }
    }
}

/// A real box as the safe-write flow's device.
///
/// This impl is the whole point of the write path: it is what gives
/// `digi_protocol::safe_write::safe_write_track` — which held all five safety
/// rules and had no caller from the day it landed — something to drive. Two
/// methods, both already built above; the value is entirely in which flow can now
/// reach them.
///
/// The trait talks in `String` errors because `protocol` may not depend on this
/// crate (PLAN.md §3), so `MidiError` cannot appear in its signature.
/// `WriteError::Io` carries the text through to the UI, and `MidiError`'s
/// `Display` is what a person reads.
impl PatternIo for ElektronDevice {
    fn identity(&self) -> Option<&DeviceIdentity> {
        self.identity.as_ref()
    }

    fn fetch_pattern_kit(&mut self, index: u8) -> Result<Vec<u8>, String> {
        ElektronDevice::fetch_pattern_kit(self, index).map_err(|e| e.to_string())
    }

    fn send_pattern_kit(&mut self, index: u8, payload: &[u8]) -> Result<(), String> {
        self.store_pattern_kit(index, payload).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_protocol::device::DeviceResponse;
    use digi_protocol::protocol::{FAMILY_DIGITAKT_2, FAMILY_DIGITONE_2};
    use digi_protocol::sound::{SOUND_MAGIC_FOOT, SOUND_MAGIC_HEAD};

    /// A box that answered the handshake. Product 42 is the DT2 and 43 the DN2, so
    /// the slug, name and dump family all come from the real product table rather
    /// than being made up here.
    fn identity(product_id: u8, build: &str) -> DeviceIdentity {
        let dev = DeviceResponse {
            product_id,
            supported_ids: vec![0x60],
            reported_name: String::new(),
        };
        identity_from_responses(&dev, build.into(), "1.0".into())
    }

    /// The payload size of a real DT2 pattern-kit dump — the committed
    /// `digitakt2-A01-conditions-2026-08-02.syx` capture. The framed length is a
    /// function of the payload length alone (7-bit encoding groups 7 bytes into
    /// 8), so zeros stand in for the pattern without weakening anything the tests
    /// below claim.
    const DT2_PAYLOAD_BYTES: usize = 111_616;
    /// What that payload frames up to as an 0x50 store message. Derived by running
    /// the JS's own `buildDumpMessage` under node against that fixture
    /// (`node /tmp/send-derive.mjs`, recipe in the git message).
    const DT2_STORE_BYTES: usize = 127_577;

    /// A sound struct the size a DN2 uses, valid at both magics so that
    /// `decode_sound_dump` accepts it. The bytes between are zeros: nothing in
    /// the store path reads them, which is the point of carrying them verbatim.
    fn sound_struct() -> Vec<u8> {
        let mut bytes = vec![0u8; 359];
        bytes[..4].copy_from_slice(&SOUND_MAGIC_HEAD.to_be_bytes());
        bytes[355..].copy_from_slice(&SOUND_MAGIC_FOOT.to_be_bytes());
        bytes
    }

    /// The same struct behind the 5-byte wrapper a `0x6b` fetch hands back.
    fn wrapped_sound() -> Vec<u8> {
        let mut bytes = vec![0u8; SOUND_WRAPPER];
        bytes.extend(sound_struct());
        bytes
    }

    // The whole reason `0x5b` gets its own planner rather than a `dump_type`
    // argument on `plan_store`: an opcode that is not a parameter cannot be
    // chosen wrongly. This pins that the message that goes out is the store the
    // minus-0x10 rule predicts, addressed by track — and that it is *not* an
    // 0x50, which would put a sound-sized payload into pattern slot `track`.
    #[test]
    fn a_track_sound_store_is_an_0x5b_addressed_by_track() {
        let msg =
            plan_track_sound_store(Some(&identity(43, "0050")), 9, &wrapped_sound()).unwrap();
        // Header: F0, the Elektron id, family, 0x00, then opcode, version, index.
        assert_eq!(msg[4], FAMILY_DIGITONE_2);
        assert_eq!(msg[6], DUMP_KIT_TRACK_SOUND, "the opcode must be 0x5b");
        assert_ne!(msg[6], DUMP_PATTERN_KIT, "and must never be the pattern-kit store");
        assert_eq!(msg[9], 9, "index addresses the track");
        assert_eq!(msg[6], DUMP_KIT_TRACK_SOUND_REQUEST - 0x10, "response-is-request-minus-0x10");
    }

    // Rule 1's firmware allowlist, on the probe path. The argument for it is
    // *stronger* here than for `store_pattern_kit`, not weaker: this opcode has
    // never been answered by any box, so an unverified OS build is two unknowns
    // multiplied rather than one.
    #[test]
    fn a_track_sound_store_refuses_an_unverified_build() {
        let err = plan_track_sound_store(Some(&identity(43, "9999")), 0, &wrapped_sound())
            .unwrap_err();
        assert!(
            matches!(err, MidiError::WriteRefused(ref r) if r.contains("9999")),
            "expected the build in the refusal, got {err:?}"
        );
        // And nothing was planned, so there are no bytes for a caller to send.
        assert!(plan_track_sound_store(None, 0, &wrapped_sound()).is_err());
    }

    // Sixteen tracks, and index 16 is not a smaller mistake than index 200. An
    // out-of-range index under an opcode whose meaning is a hypothesis is the
    // experiment that has no recovery story, so it is refused before the gate
    // has anything to send.
    #[test]
    fn a_track_sound_store_refuses_a_track_that_is_not_in_a_kit() {
        for track in [KIT_TRACKS, 16, 100, 255] {
            let err = plan_track_sound_store(Some(&identity(43, "0050")), track, &wrapped_sound())
                .unwrap_err();
            assert!(
                matches!(err, MidiError::WriteRefused(ref r) if r.contains("outside")),
                "track {track} should be refused, got {err:?}"
            );
        }
        assert!(plan_track_sound_store(Some(&identity(43, "0050")), 15, &wrapped_sound()).is_ok());
    }

    // The check that stops a probe corrupting a kit. `decode_sound_dump` is
    // trusted here for the reason its own doc gives — the foot magic does not
    // validate at the wrong size — so this asserts the planner *consults* it, on
    // both shapes, and refuses what neither accepts.
    #[test]
    fn a_track_sound_store_refuses_bytes_that_are_not_a_sound() {
        let id = identity(43, "0050");
        // Both shapes the probe may legitimately send.
        assert!(plan_track_sound_store(Some(&id), 0, &sound_struct()).is_ok(), "bare struct");
        assert!(plan_track_sound_store(Some(&id), 0, &wrapped_sound()).is_ok(), "wrapped struct");

        // A struct whose foot magic is wrong: the head still reads, a name and a
        // tag mask still decode, and it is still not a sound.
        let mut broken = sound_struct();
        broken[355] ^= 0xff;
        let err = plan_track_sound_store(Some(&id), 0, &broken).unwrap_err();
        assert!(
            matches!(err, MidiError::WriteRefused(ref r) if r.contains("not a sound struct")),
            "a bad foot must be refused, got {err:?}"
        );

        for junk in [vec![], vec![0u8; 8], vec![0xffu8; 359]] {
            assert!(
                plan_track_sound_store(Some(&id), 0, &junk).is_err(),
                "{} bytes of junk must not reach a cable",
                junk.len()
            );
        }
    }

    // The read guard used to make this whole file incapable of writing. It no
    // longer does — `store_pattern_kit` sends an 0x50 — so what this pins now is
    // narrower and still worth pinning: no *fetch* can turn into a store.
    #[test]
    fn fetch_dump_refuses_a_write_opcode_before_touching_a_port() {
        // 0x50 is what *stores* a pattern-kit on the box.
        assert_eq!(
            assert_request_opcode(DUMP_PATTERN_KIT),
            Err(DeviceError::NotARequestOpcode(DUMP_PATTERN_KIT))
        );
        assert!(assert_request_opcode(DUMP_PATTERN_KIT_REQUEST).is_ok());
    }

    // A request carries no payload, so its checksum covers zero bytes and its
    // count is the bare trailer. Pinning the exact bytes means a change to the
    // framing cannot silently turn a request into something else.
    #[test]
    fn a_dump_request_is_an_empty_payload_message() {
        let msg = build_dump_message(FAMILY_DIGITAKT_2, DUMP_PATTERN_KIT_REQUEST, 3, &[]);
        assert_eq!(
            msg,
            vec![0xf0, 0x00, 0x20, 0x3c, 0x14, 0x00, 0x60, 0x01, 0x01, 0x03, 0x00, 0x00, 0x00, 0x05, 0xf7]
        );
    }

    #[test]
    fn msg_ids_advance_and_wrap_clear_of_transfers_range() {
        // `take_msg_id`'s own arithmetic, called rather than copied: this test
        // used to restate it and could therefore have gone on passing about a
        // rule the real counter no longer followed.
        assert_eq!(next_msg_id(FIRST_MSG_ID), FIRST_MSG_ID + 1);
        assert_eq!(next_msg_id(0xfffe), 0xffff);
        // The top of the range comes back to the start, not to zero — an id
        // below `FIRST_MSG_ID` is Transfer's to use.
        assert_eq!(next_msg_id(0xffff), FIRST_MSG_ID);
        assert!(next_msg_id(0xffff) >= FIRST_MSG_ID);
    }

    // --- the write path -------------------------------------------------------

    // A store is an unsolicited dump *response*, and its opcode is the difference
    // between overwriting a slot and asking about one. Pinned byte for byte off
    // the JS's own `buildDumpMessage`, so a change to the framing cannot quietly
    // aim this at something else.
    #[test]
    fn a_store_is_an_unsolicited_0x50_and_its_bytes_are_pinned() {
        let msg = plan_store(Some(&identity(42, "0070")), 3, &[1, 2, 3]).unwrap();
        assert_eq!(
            msg,
            vec![
                0xf0, 0x00, 0x20, 0x3c, 0x14, 0x00, 0x50, 0x01, 0x01, 0x03, 0x00, 0x01, 0x02,
                0x03, 0x00, 0x06, 0x00, 0x09, 0xf7
            ]
        );
        // The opcode, called out on its own: 0x50 stores, 0x60 asks.
        assert_eq!(msg[6], DUMP_PATTERN_KIT);
        assert_ne!(msg[6], DUMP_PATTERN_KIT_REQUEST);
    }

    // The reason `SEND_CHUNK` exists. A real pattern's store message is not near
    // the 65,535-byte ceiling a single CoreMIDI packet can describe — it is nearly
    // twice it — so a port of the oracle's one-shot `output.send(msg)` would have
    // sent nothing, every time, and returned `Ok(())` while doing it.
    #[test]
    fn a_real_patterns_store_message_is_nearly_twice_what_one_send_can_carry() {
        let payload = vec![0u8; DT2_PAYLOAD_BYTES];
        let msg = plan_store(Some(&identity(42, "0070")), 0, &payload).unwrap();
        assert_eq!(msg.len(), DT2_STORE_BYTES);
        // Not "a bit over" — 127,577 against a 65,535 ceiling. There is no chunk
        // size at all that makes a one-shot send of this work.
        assert!(msg.len() > 65_535, "the premise: a store message does not fit in one packet");
        assert!(msg.len() > 65_535 + 65_535 / 2, "and it is not marginally over");
        // Which is only useful if the chunks it goes out in do fit. Measured
        // boundary, 2026-08-18: 65,535 bytes arrive and 65,536 vanish silently.
        //
        // `CHUNKED` rather than `SEND_CHUNK` for the reason spelled out where that
        // literal is declared: `SEND_CHUNK` is `usize::MAX` on Windows, so a test
        // that followed it there would split into exactly one piece and assert
        // that one piece is under 65,535 — both of which are false, and neither of
        // which is what this test is about. The rule being pinned is CoreMIDI's.
        let chunks: Vec<_> = msg.chunks(CHUNKED).collect();
        assert!(chunks.len() > 1, "a message this size has to be split");
        for c in &chunks {
            assert!(c.len() <= 65_535, "a chunk over 65,535 bytes is dropped without an error");
        }
    }

    /// The chunk size CoreMIDI and ALSA get, named so the tests below read as
    /// being about a platform rather than about a number. Deliberately a literal
    /// rather than a read of [`SEND_CHUNK`]: on a Windows host that constant is
    /// `usize::MAX`, and a test that followed it there would silently stop
    /// testing splitting at all.
    const CHUNKED: usize = 4096;

    /// What WinMM gets — the whole framed message in one call. Same reasoning
    /// inverted: this value has to be reachable from a Mac, or the rule it pins
    /// is one nobody in this repo can run.
    const WHOLE: usize = usize::MAX;

    /// Drive `paced_send` over a recording sink. Returns what went out and how
    /// long it waited between pieces — the two things a real port would hide.
    fn record_send(msg: &[u8], chunk: usize) -> (Vec<Vec<u8>>, Vec<Duration>) {
        let mut sent = Vec::new();
        let mut waits = Vec::new();
        paced_send(msg, chunk, |c| {
            sent.push(c.to_vec());
            Ok(())
        }, |d| waits.push(d))
        .unwrap();
        (sent, waits)
    }

    // Splitting a SysEx across sends is only safe if the bytes are unchanged by
    // it: the far end reassembles the stream, so the message it sees is the
    // concatenation and nothing else. This goes through `paced_send` rather than
    // through `chunks` directly, because the claim being made is about what the
    // *write path* does — an earlier draft of these tests asserted `chunks()` in
    // isolation and would have passed with the chunking removed from the sender.
    #[test]
    fn a_store_goes_out_in_pieces_that_rejoin_into_exactly_what_was_framed() {
        let payload: Vec<u8> = (0..DT2_PAYLOAD_BYTES).map(|i| (i % 128) as u8).collect();
        let msg = plan_store(Some(&identity(42, "0070")), 0, &payload).unwrap();
        let (sent, _) = record_send(&msg, CHUNKED);

        assert!(sent.len() > 1, "a message this size has to leave in more than one send");
        for (i, c) in sent.iter().enumerate() {
            assert!(c.len() <= 65_535, "send {i} is {} bytes — over 65,535 vanishes", c.len());
        }
        let rejoined: Vec<u8> = sent.concat();
        assert_eq!(rejoined, msg, "the box must receive the framed message and nothing else");

        // And the rejoined stream still parses as the store it was built as, which
        // is the property a box on the far end actually depends on.
        let dump = parse_sysex(&rejoined).dump.expect("a store message is a dump message");
        assert_eq!(dump.dump_type, DUMP_PATTERN_KIT);
        assert!(dump.checksum_ok && dump.count_ok);
        assert_eq!(dump.payload, payload);
    }

    // The Windows rule, run on whatever host this suite is on. `midir`'s WinMM
    // backend decides sysex-versus-short-message from `message[0] == 0xF0` and
    // refuses any other send over three bytes, so the whole framed dump has to
    // leave in **one** call — the exact opposite of the constraint above, and the
    // reason `SEND_CHUNK` is `cfg`-conditional rather than a single number.
    //
    // Written against `WHOLE` rather than `SEND_CHUNK` on purpose: on a Mac this
    // is the only thing standing between a Windows build and thirty refused
    // sends per write, and a `cfg`-gated version of it would compile away
    // precisely where it is needed.
    #[test]
    fn on_winmm_the_whole_dump_leaves_in_one_send_that_starts_with_f0() {
        let payload: Vec<u8> = (0..DT2_PAYLOAD_BYTES).map(|i| (i % 128) as u8).collect();
        let msg = plan_store(Some(&identity(42, "0070")), 0, &payload).unwrap();
        let (sent, waits) = record_send(&msg, WHOLE);

        assert_eq!(sent.len(), 1, "WinMM refuses every piece after the first");
        assert_eq!(sent[0], msg, "and the one piece is the entire framed message");
        assert_eq!(sent[0][0], 0xf0, "the byte WinMM dispatches on");

        // The pacing budget is unchanged by sending it whole — one digest pause
        // for the message, then the settle, which is the JS oracle's own
        // `Math.ceil(len / 800) + 100` rather than a shortcut around it.
        assert_eq!(waits, vec![digest_pace(msg.len()), SEND_SETTLE]);
        assert!(waits.iter().sum::<Duration>() >= digest_pace(msg.len()) + SEND_SETTLE);
    }

    // And the constant actually wired into the sender is the one this host needs.
    // Without this, both rules above could be perfectly tested while `SEND_CHUNK`
    // itself was wrong on every platform.
    #[test]
    fn the_send_chunk_this_platform_compiles_with_matches_its_backend() {
        if cfg!(target_os = "windows") {
            assert_eq!(SEND_CHUNK, WHOLE, "WinMM cannot take a split sysex");
        } else {
            assert_eq!(SEND_CHUNK, CHUNKED, "CoreMIDI drops anything over 65,535 in one packet");
        }
    }

    // A message that already fits goes out whole — the chunking is a ceiling, not
    // a fragmentation policy, and the handshake-sized traffic on this port must not
    // start arriving in pieces.
    #[test]
    fn a_message_that_fits_in_one_send_is_not_split() {
        let msg = plan_store(Some(&identity(42, "0070")), 3, &[1, 2, 3]).unwrap();
        let (sent, waits) = record_send(&msg, CHUNKED);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], msg);
        // One pace for the chunk, then the settle.
        assert_eq!(waits, vec![digest_pace(msg.len()), SEND_SETTLE]);
    }

    // The pacing is a pause after *every* chunk plus one settle at the end, and
    // the settle is last — a re-read that beat the settle would fail the verify on
    // a write that worked.
    #[test]
    fn the_pacing_pauses_after_every_chunk_and_settles_once_at_the_end() {
        let msg = plan_store(Some(&identity(42, "0070")), 0, &vec![0u8; DT2_PAYLOAD_BYTES]).unwrap();
        let (sent, waits) = record_send(&msg, CHUNKED);
        assert_eq!(waits.len(), sent.len() + 1, "one pause per chunk, then the settle");
        assert_eq!(*waits.last().unwrap(), SEND_SETTLE);
        assert!(
            waits.iter().sum::<Duration>() >= digest_pace(msg.len()) + SEND_SETTLE,
            "the total must not undercut the oracle's single wait"
        );
    }

    // The case that would otherwise be discovered as a scrambled slot: a send
    // that fails partway through must stop, and must not come back as `Ok`. The
    // safe-write flow reads the pattern back afterwards, so an error swallowed
    // here would be reported as a verify failure on a write that was never
    // finished — the right diagnosis for the wrong reason, and a caller told to
    // restore rather than told to retry.
    #[test]
    fn a_send_that_fails_partway_stops_and_is_not_reported_as_a_write() {
        let msg = plan_store(Some(&identity(42, "0070")), 0, &vec![0u8; DT2_PAYLOAD_BYTES]).unwrap();
        let total_chunks = msg.chunks(CHUNKED).count();
        assert!(total_chunks > 3, "the test needs a message that outlives the failure");

        let mut sent = 0usize;
        let err = paced_send(
            &msg,
            CHUNKED,
            |_| {
                sent += 1;
                if sent == 3 {
                    Err(MidiError::Send("cable pulled".into()))
                } else {
                    Ok(())
                }
            },
            |_| {},
        )
        .unwrap_err();

        assert!(matches!(err, MidiError::Send(_)));
        assert_eq!(sent, 3, "nothing may be sent after a failure");
        assert!(sent < total_chunks, "and the message was genuinely left unfinished");
    }

    // `Math.ceil(n / 800)`, the rate the JS computes its post-send wait from.
    // Values derived under node rather than reasoned about, because the case that
    // matters is the off-by-one at the boundary.
    #[test]
    fn digest_pace_is_the_js_ceiling() {
        for (bytes, ms) in [(0, 0), (1, 1), (799, 1), (800, 1), (801, 2), (4096, 6), (65_535, 82)] {
            assert_eq!(digest_pace(bytes), Duration::from_millis(ms), "at {bytes} bytes");
        }
    }

    // The JS sends instantly and waits `ceil(len / 800) + 100` ms for the box to
    // digest. This port spends that budget pacing the chunks instead, so the box
    // is fed rather than flooded — and the total must not come out *quicker* than
    // the oracle's, because being quicker is the only direction that could hand
    // the box less time than digi-roll proved it needs.
    #[test]
    fn the_paced_send_is_never_quicker_than_the_oracles_single_wait() {
        let payload = vec![0u8; DT2_PAYLOAD_BYTES];
        let msg = plan_store(Some(&identity(42, "0070")), 0, &payload).unwrap();
        let paced: Duration = msg.chunks(SEND_CHUNK).map(|c| digest_pace(c.len())).sum();
        let ours = paced + SEND_SETTLE;
        let oracle = digest_pace(msg.len()) + SEND_SETTLE;
        assert_eq!(oracle, Duration::from_millis(260), "the JS's own wait for this message");
        assert!(ours >= oracle, "ours {ours:?} is quicker than the oracle's {oracle:?}");
    }

    // Rule 3 of the five, enforced at the wire rather than only in the flow. The
    // route that skips `safe_write_track` skips the backup and the verify, and
    // this is the one thing it must not also skip — a build whose pattern format
    // was never verified is how a slot gets scrambled in a way nothing was backed
    // up for.
    #[test]
    fn the_allowlist_refuses_a_store_before_a_message_is_even_built() {
        let err = plan_store(Some(&identity(43, "0001")), 0, &[1, 2, 3]).unwrap_err();
        match &err {
            MidiError::WriteRefused(reason) => {
                assert!(reason.contains("isn't write-verified"), "got {reason:?}");
                assert!(reason.contains("0001"), "the build has to be in the message: {reason:?}");
            }
            other => panic!("expected WriteRefused, got {other:?}"),
        }
        // And the wording survives into what a person is shown.
        assert!(err.to_string().starts_with("nothing was sent:"));
    }

    // A box we can talk to and cannot decode stays read-only whatever its build.
    // Product 12 is the gen-1 Digitakt: identifiable here, undecodable, real.
    #[test]
    fn a_box_whose_patterns_we_cannot_decode_cannot_be_written_to() {
        let err = plan_store(Some(&identity(12, "0070")), 0, &[1, 2, 3]).unwrap_err();
        assert!(matches!(err, MidiError::WriteRefused(ref r) if r.contains("read-only")));
    }

    #[test]
    fn a_store_needs_an_identity_because_the_gate_has_nothing_to_check_without_one() {
        let err = plan_store(None, 0, &[1, 2, 3]).unwrap_err();
        assert!(matches!(err, MidiError::WriteRefused(ref r) if r == "no device connected"));
    }

    // Both allowlisted boxes reach the wire, so the gate above is refusing the
    // right things rather than everything.
    #[test]
    fn the_two_write_verified_boxes_frame_a_store_for_their_own_family() {
        let dt2 = plan_store(Some(&identity(42, "0070")), 0, &[0]).unwrap();
        let dn2 = plan_store(Some(&identity(43, "0049")), 0, &[0]).unwrap();
        assert_eq!(dt2[4], FAMILY_DIGITAKT_2);
        assert_eq!(dn2[4], FAMILY_DIGITONE_2);
    }
}
