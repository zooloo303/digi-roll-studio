//! The active-note table: what is sounding, and when it has to stop.
//!
//! Non-negotiable per PLAN.md §4, and for a reason that is easy to underestimate:
//! note lengths are fractional and tracks have independent lengths, so note-offs
//! routinely fall *after* their track has wrapped. Nothing about "send the off
//! at the end of the step" survives contact with polymeter.
//!
//! Keyed by `(port, channel, pitch)` — the tuple that identifies a sounding voice
//! on the wire. Two tracks on the same port and channel playing the same pitch
//! are one voice as far as MIDI is concerned, which is exactly why re-triggering
//! has to send the previous note-off first.

use crate::event::{MidiMsg, PortId, ScheduledEvent, CC_ALL_NOTES_OFF, CC_ALL_SOUND_OFF};

/// A note the engine believes is currently sounding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveNote {
    pub port: PortId,
    pub channel: u8,
    pub pitch: u8,
    /// Seconds since transport start, when its note-off is due.
    pub off_at: f64,
}

/// Everything currently sounding.
///
/// A `Vec` with a linear scan rather than a map: the table holds tens of entries,
/// not thousands, and a `Vec` reserved once at construction never allocates on
/// the engine thread — which a `HashMap` insert can, and which PLAN.md §4 forbids.
#[derive(Debug, Clone)]
pub struct ActiveNotes {
    notes: Vec<ActiveNote>,
}

impl Default for ActiveNotes {
    fn default() -> Self {
        Self::new()
    }
}

impl ActiveNotes {
    /// Sized for a full session — 32 tracks with a few voices each — so the
    /// backing store is allocated once, here, and not on the engine thread.
    pub fn new() -> Self {
        Self {
            notes: Vec::with_capacity(256),
        }
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &ActiveNote> {
        self.notes.iter()
    }

    fn position(&self, port: PortId, channel: u8, pitch: u8) -> Option<usize> {
        self.notes
            .iter()
            .position(|n| n.port == port && n.channel == channel && n.pitch == pitch)
    }

    /// Register a note that is about to sound, and say whether it displaced one.
    ///
    /// `Some(event)` means this pitch was already sounding on that port and
    /// channel: the returned note-off must be sent **before** the note-on, or the
    /// box is left holding a voice nothing will ever release. [`MidiMsg::rank`]
    /// puts an off before an on at the same deadline, so the two may share one.
    ///
    /// The displaced note keeps **the earlier of its own deadline and the new
    /// note's start**. That is not a nicety. The scheduler may be asked for a
    /// window of any length — a window is however long the last wake-up took — and
    /// a long window would otherwise drag every already-expired note-off forward
    /// to whenever its pitch happened to be retriggered, so the same pattern would
    /// play differently depending on how the timeline was chopped up. Taking the
    /// earlier deadline makes the output identical either way: a note that had
    /// already finished is released when it finished, and only a genuine early
    /// retrigger is cut short.
    pub fn note_on(
        &mut self,
        port: PortId,
        channel: u8,
        pitch: u8,
        at: f64,
        off_at: f64,
    ) -> Option<ScheduledEvent> {
        let displaced = self.position(port, channel, pitch).map(|i| {
            let previous = self.notes.remove(i);
            ScheduledEvent::new(
                previous.off_at.min(at),
                port,
                MidiMsg::NoteOff { channel, pitch },
            )
        });
        self.notes.push(ActiveNote { port, channel, pitch, off_at });
        displaced
    }

    /// Emit and forget every note-off due at or before `now`.
    ///
    /// Appends rather than returning a `Vec`, so the caller's window buffer is
    /// the only allocation in the loop.
    pub fn drain_due(&mut self, now: f64, out: &mut Vec<ScheduledEvent>) {
        let mut i = 0;
        while i < self.notes.len() {
            if self.notes[i].off_at <= now {
                let n = self.notes.swap_remove(i);
                out.push(ScheduledEvent::new(
                    n.off_at,
                    n.port,
                    MidiMsg::NoteOff { channel: n.channel, pitch: n.pitch },
                ));
            } else {
                i += 1;
            }
        }
    }

    /// The earliest note-off still pending, so the transport knows how long it
    /// may sleep.
    pub fn next_due(&self) -> Option<f64> {
        self.notes
            .iter()
            .map(|n| n.off_at)
            .fold(None, |acc: Option<f64>, t| Some(acc.map_or(t, |a| a.min(t))))
    }

    /// Every pending note-off, dated `at`, and empty the table.
    ///
    /// Stop does this. A note left sounding after the transport stops is the one
    /// failure mode of a sequencer that a user cannot work around from the UI.
    pub fn flush(&mut self, at: f64, out: &mut Vec<ScheduledEvent>) {
        for n in self.notes.drain(..) {
            out.push(ScheduledEvent::new(
                at,
                n.port,
                MidiMsg::NoteOff { channel: n.channel, pitch: n.pitch },
            ));
        }
    }

    /// Panic: every pending note-off, then All Notes Off (CC 123) and All Sound
    /// Off (CC 120) on every `(port, channel)` that has been used.
    ///
    /// The explicit note-offs come first because they are the ones that are
    /// certainly correct; the two CCs are the belt-and-braces for a voice the
    /// table does not know about — one left over from a crash, or from another
    /// sequencer on the same port.
    ///
    /// `channels_in_use` is passed in rather than derived from the table: by the
    /// time a panic is worth pressing, the table may already be empty and the box
    /// may still be droning.
    pub fn panic(
        &mut self,
        at: f64,
        channels_in_use: &[(PortId, u8)],
        out: &mut Vec<ScheduledEvent>,
    ) {
        self.flush(at, out);
        for &(port, channel) in channels_in_use {
            out.push(ScheduledEvent::new(
                at,
                port,
                MidiMsg::ControlChange { channel, controller: CC_ALL_NOTES_OFF, value: 0 },
            ));
            out.push(ScheduledEvent::new(
                at,
                port,
                MidiMsg::ControlChange { channel, controller: CC_ALL_SOUND_OFF, value: 0 },
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: PortId = PortId(0);

    #[test]
    fn a_note_on_registers_and_displaces_nothing_when_the_pitch_is_free() {
        let mut a = ActiveNotes::new();
        assert!(a.note_on(P, 0, 60, 0.0, 1.0).is_none());
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn retriggering_a_sounding_pitch_sends_its_note_off_first() {
        let mut a = ActiveNotes::new();
        a.note_on(P, 0, 60, 0.0, 10.0);
        let displaced = a.note_on(P, 0, 60, 1.0, 11.0).expect("the first note must be released");
        assert_eq!(displaced.msg, MidiMsg::NoteOff { channel: 0, pitch: 60 });
        assert_eq!(displaced.at, 1.0, "dated at the new note, not the old off");
        assert!(displaced.msg.rank() < MidiMsg::NoteOn { channel: 0, pitch: 60, velocity: 1 }.rank());
        assert_eq!(a.len(), 1, "one voice, not two");
    }

    #[test]
    fn the_same_pitch_on_another_channel_or_port_is_a_different_voice() {
        let mut a = ActiveNotes::new();
        a.note_on(P, 0, 60, 0.0, 10.0);
        assert!(a.note_on(P, 1, 60, 0.0, 10.0).is_none(), "other channel");
        assert!(a.note_on(PortId(1), 0, 60, 0.0, 10.0).is_none(), "other port");
        assert_eq!(a.len(), 3);
    }

    #[test]
    fn note_offs_come_due_at_their_deadline_and_only_then() {
        let mut a = ActiveNotes::new();
        a.note_on(P, 0, 60, 0.0, 1.0);
        a.note_on(P, 0, 64, 0.0, 2.0);
        let mut out = Vec::new();
        a.drain_due(0.5, &mut out);
        assert!(out.is_empty(), "nothing due yet");
        a.drain_due(1.0, &mut out);
        assert_eq!(out.len(), 1, "the deadline is inclusive");
        assert_eq!(a.len(), 1);
        a.drain_due(99.0, &mut out);
        assert_eq!(out.len(), 2);
        assert!(a.is_empty());
    }

    /// The polymetric case the table exists for: a note that outlives the wrap of
    /// the track that started it.
    #[test]
    fn a_note_off_survives_its_tracks_wrap() {
        let mut a = ActiveNotes::new();
        // A 4-step track at 120 bpm wraps every 0.5 s; this note runs to 0.9 s.
        a.note_on(P, 0, 60, 0.4, 0.9);
        let mut out = Vec::new();
        a.drain_due(0.5, &mut out);
        assert!(out.is_empty(), "the wrap must not release it");
        a.drain_due(0.9, &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn next_due_is_the_earliest_pending_off() {
        let mut a = ActiveNotes::new();
        assert_eq!(a.next_due(), None);
        a.note_on(P, 0, 60, 0.0, 3.0);
        a.note_on(P, 0, 64, 0.0, 1.5);
        a.note_on(P, 0, 67, 0.0, 2.0);
        assert_eq!(a.next_due(), Some(1.5));
    }

    #[test]
    fn stop_flushes_every_pending_note_off() {
        let mut a = ActiveNotes::new();
        a.note_on(P, 0, 60, 0.0, 100.0);
        a.note_on(P, 3, 64, 0.0, 200.0);
        let mut out = Vec::new();
        a.flush(5.0, &mut out);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|e| e.at == 5.0), "all dated at the stop");
        assert!(a.is_empty());
    }

    #[test]
    fn panic_sends_the_offs_then_all_notes_off_and_all_sound_off() {
        let mut a = ActiveNotes::new();
        a.note_on(P, 0, 60, 0.0, 100.0);
        let mut out = Vec::new();
        a.panic(5.0, &[(P, 0), (PortId(1), 9)], &mut out);
        assert_eq!(out[0].msg, MidiMsg::NoteOff { channel: 0, pitch: 60 });
        assert_eq!(
            out[1].msg,
            MidiMsg::ControlChange { channel: 0, controller: CC_ALL_NOTES_OFF, value: 0 }
        );
        assert_eq!(
            out[2].msg,
            MidiMsg::ControlChange { channel: 0, controller: CC_ALL_SOUND_OFF, value: 0 }
        );
        assert_eq!(out[3].port, PortId(1));
        assert_eq!(out.len(), 5);
        assert!(a.is_empty());
    }

    #[test]
    fn panic_still_speaks_when_the_table_is_empty() {
        // The case it is pressed in: the box is droning and the engine has no
        // record of why.
        let mut a = ActiveNotes::new();
        let mut out = Vec::new();
        a.panic(0.0, &[(P, 4)], &mut out);
        assert_eq!(out.len(), 2);
    }
}
