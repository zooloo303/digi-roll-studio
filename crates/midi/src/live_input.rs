// One input port held open for the notes somebody is playing.
//
// MIDI_RECORD_DESIGN.md §4.1. This is the whole of stage 1: a connection, a
// parser, and a channel. It makes no decision about *when* a note happened
// beyond stamping the moment it arrived, and none at all about where it lands —
// that is the engine's job (§4.2) and the take's (§4.3).
//
// # Why this is not `SysExInbox`
//
// The other input this crate opens ([`crate::ports::SysExInbox`]) reassembles
// whole SysEx frames and hands them over in lumps. This one wants the opposite
// of every part of that: single channel-voice messages, delivered the instant
// they arrive, with SysEx filtered out at the driver so a box's 14 KB dump
// cannot walk through a note stream. Two types rather than one flag, because
// the two have no line of code in common.
//
// # The timestamp, and the one that is not used
//
// `midir`'s callback carries a `u64` timestamp, and **it is ignored here**. It
// is microseconds on a per-backend epoch — CoreMIDI host time, the ALSA queue's
// own clock, WinMM's milliseconds since `midiInStart` — and none of the three is
// comparable to the `Instant` the engine thread dates everything off without a
// calibration nobody wants to own. `Instant::now()` in the callback is one
// syscall on the driver thread and is directly comparable to
// `EngineThread::started_at`, which is the only clock that matters downstream.
//
// # Ignore flags, and a correction to the design
//
// MIDI_RECORD_DESIGN.md §3 says midir's *default* filters SysEx, time code and
// active sensing, and that `SysExInbox` sets the opposite. That is backwards:
// every one of midir 0.11's seven backends initialises `ignore_flags` to
// `Ignore::None`, so the default filters **nothing** and `SysExInbox`'s explicit
// `Ignore::None` is a no-op kept for clarity. The filtering this file wants is
// therefore asked for, not inherited — see [`LiveInput::open`].

use std::sync::mpsc::Sender;
use std::time::Instant;

use midir::{Ignore, MidiInput, MidiInputConnection};

use crate::ports::{resolve_input, PortBinding, CLIENT_NAME};
use crate::MidiError;

/// A note-on or note-off as it came off the wire, stamped with the moment it
/// arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveEvent {
    /// Stamped in the driver callback, not midir's `u64` — see the module
    /// header.
    pub at: Instant,
    pub kind: LiveKind,
}

/// The only two things this feature listens for, decision 7.
///
/// **Deliberately not `digi_core::record::PlacedKind`, which is the same two
/// variants.** This crate does not depend on `core` and `core` does not depend
/// on this one; `engine` is the one crate that can see both, and it does the
/// five-line conversion in `engine::record`. Two small enums either side of a
/// layer boundary is the price of the boundary, and it is cheaper than adding
/// an edge to the crate graph for a pitch and a velocity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveKind {
    NoteOn { pitch: u8, velocity: u8 },
    NoteOff { pitch: u8 },
}

impl LiveKind {
    pub fn pitch(&self) -> u8 {
        match self {
            LiveKind::NoteOn { pitch, .. } | LiveKind::NoteOff { pitch } => *pitch,
        }
    }
}

/// One channel-voice message to a [`LiveKind`], or `None` for anything this
/// feature does not listen for.
///
/// Three rules, each of them a thing a keyboard really does:
///
/// * **A note-on with velocity 0 is a note-off** (MIDI 1.0 §4.2, "Running
///   Status"). Most controllers send it that way and never send `0x80` at all,
///   so a parser that only knows `0x80` records notes that are never released.
/// * **The channel nibble is read and discarded** — decision 1. The record
///   input is one port with every channel merged; thru rewrites the channel to
///   the selected track's on the way out, so keeping the incoming one would only
///   give something downstream the chance to use it.
/// * **Everything else returns `None`** — decision 7. CC, program change,
///   aftertouch (both kinds), pitch bend and every system message are ignored on
///   capture *and* on thru until §8 says otherwise.
///
/// Drivers deliver whole channel messages on all three backends this app runs
/// on, so there is no running-status reassembly here: a two-byte buffer is a
/// truncated message, not a continuation, and it is dropped.
pub fn parse_live(bytes: &[u8]) -> Option<LiveKind> {
    let [status, data1, data2, ..] = bytes[..] else {
        return None;
    };
    let pitch = data1 & 0x7f;
    match status & 0xf0 {
        0x90 if data2 & 0x7f > 0 => Some(LiveKind::NoteOn { pitch, velocity: data2 & 0x7f }),
        // Velocity 0 on a note-on, and the real `0x80` note-off, are one thing.
        0x90 | 0x80 => Some(LiveKind::NoteOff { pitch }),
        _ => None,
    }
}

/// An input port held open, forwarding every note down a channel.
///
/// Dropping this closes the port, which is the whole of its lifecycle: the
/// `EngineLink` holds one, and a rebuild drops it and opens another.
pub struct LiveInput {
    // Held only to keep the connection alive, the same as `SysExInbox`'s.
    _conn: MidiInputConnection<()>,
    /// What was opened, so a caller can tell whether the binding it is holding
    /// is the one that is live.
    pub port: PortBinding,
}

impl LiveInput {
    /// Open `binding` — by id first and then by name, the rule
    /// [`resolve_input`] owns — and forward every parsed message down `tx`.
    ///
    /// **A closed receiver is not an error.** The engine that owned the other
    /// end has been rebuilt and this connection is about to be dropped with it;
    /// treating a `SendError` as a failure would put a line in the console every
    /// time a box was plugged in. The callback drops the event and carries on.
    pub fn open(binding: &PortBinding, tx: Sender<LiveEvent>) -> Result<Self, MidiError> {
        let mut midi_in = MidiInput::new(CLIENT_NAME)?;
        // Asked for rather than inherited — see the module header. `All` is
        // SysEx, timing (clock and MTC) and active sensing: three streams an
        // Elektron box on the same cable emits constantly and none of which
        // `parse_live` would return anything for. Filtering them at the driver
        // is what keeps this callback from running 300 times a second to say no.
        midi_in.ignore(Ignore::All);
        let port = resolve_input(&midi_in, binding)
            .ok_or_else(|| MidiError::PortNotFound(binding.name.clone()))?;

        let conn = midi_in
            .connect(
                &port,
                &binding.name,
                move |_ts, bytes, _| {
                    // Everything this callback does, in the order it does it.
                    // Nothing here allocates, locks or waits.
                    if let Some(kind) = parse_live(bytes) {
                        let _ = tx.send(LiveEvent { at: Instant::now(), kind });
                    }
                },
                (),
            )
            .map_err(|e| MidiError::Connect(e.to_string()))?;

        Ok(Self { _conn: conn, port: binding.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_on_carries_its_pitch_and_velocity() {
        assert_eq!(
            parse_live(&[0x90, 60, 100]),
            Some(LiveKind::NoteOn { pitch: 60, velocity: 100 })
        );
        assert_eq!(parse_live(&[0x80, 60, 64]), Some(LiveKind::NoteOff { pitch: 60 }));
    }

    /// MIDI 1.0 §4.2, and the rule most controllers actually use: a keyboard
    /// that only ever sends `0x90` releases a key by sending it again with
    /// velocity 0. Without this every note recorded from such a keyboard would
    /// be held open for the whole take.
    #[test]
    fn a_note_on_at_velocity_zero_is_a_note_off() {
        assert_eq!(parse_live(&[0x90, 60, 0]), Some(LiveKind::NoteOff { pitch: 60 }));
    }

    /// Decision 1: one port, every channel merged. The nibble is read — it has
    /// to be, to find the message type — and then thrown away, because thru
    /// rewrites it to the selected track's.
    #[test]
    fn the_channel_nibble_is_discarded() {
        for channel in 0..16u8 {
            assert_eq!(
                parse_live(&[0x90 | channel, 64, 80]),
                Some(LiveKind::NoteOn { pitch: 64, velocity: 80 }),
                "channel {channel}"
            );
        }
    }

    /// Decision 7: notes only, on capture and on thru. Each of these is a
    /// message a keyboard on the same cable really sends, and each one silently
    /// becoming a note would be worse than being ignored.
    #[test]
    fn nothing_but_a_note_is_listened_for() {
        let ignored: [&[u8]; 6] = [
            &[0xb0, 64, 127],       // sustain pedal — §8, not yet
            &[0xa0, 60, 40],        // polyphonic aftertouch
            &[0xd0, 40],            // channel aftertouch
            &[0xe0, 0x00, 0x40],    // pitch bend
            &[0xc0, 7],             // program change
            &[0xf0, 0x7e, 0xf7],    // SysEx, which the driver filters anyway
        ];
        for bytes in ignored {
            assert_eq!(parse_live(bytes), None, "{bytes:02x?}");
        }
    }

    /// A short buffer is a truncated message, not a running-status continuation:
    /// every backend this app runs on delivers whole channel messages. Dropping
    /// it is the only safe answer, since guessing the missing byte would invent
    /// a pitch.
    #[test]
    fn a_truncated_message_is_dropped_rather_than_guessed() {
        assert_eq!(parse_live(&[0x90, 60]), None);
        assert_eq!(parse_live(&[0x90]), None);
        assert_eq!(parse_live(&[]), None);
    }

    /// Realtime bytes arrive one at a time and share the `0xf0` high nibble with
    /// SysEx. They are filtered at the driver, but the parser must not fall over
    /// on one that slips past a backend that does not honour the flag.
    #[test]
    fn a_single_realtime_byte_is_not_a_note() {
        for byte in [0xf8u8, 0xfa, 0xfc, 0xfe] {
            assert_eq!(parse_live(&[byte]), None, "{byte:02x}");
        }
    }

    #[test]
    fn a_live_kind_knows_its_own_pitch() {
        assert_eq!(LiveKind::NoteOn { pitch: 42, velocity: 1 }.pitch(), 42);
        assert_eq!(LiveKind::NoteOff { pitch: 42 }.pitch(), 42);
    }
}
