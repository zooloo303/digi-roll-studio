//! What goes on the wire, and when.
//!
//! PLAN.md §4: *every scheduled event is `(deadline, PortId, MidiMsg)`, and the
//! queue is sorted by deadline across all devices*. Two boxes is not two
//! sequencers side by side; it is one queue whose entries carry a port. That is
//! the whole reason a DT2 trig and a DN2 trig on the same step go out back to
//! back with no per-device drift.

/// A port, as the engine refers to one: an index into the transport's connection
/// table, interned once by [`PortTable`].
///
/// Not a `String`. The queue is walked on the engine thread, which must not
/// allocate or hash its way through a name per event (PLAN.md §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PortId(pub usize);

/// Interns port names to [`PortId`]s. Built and extended on the UI thread while
/// preparing a snapshot; the engine thread only ever reads through it.
///
/// Equality is the whole table in order, not just the set of names: two tables
/// holding the same names in a different order number the ports differently, and
/// a sink opened against one would send to the wrong box under the other. The UI
/// compares tables to decide whether a session's routing still matches the
/// connections it opened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortTable {
    names: Vec<String>,
}

impl PortTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// The id for this port name, creating one if it is new.
    pub fn intern(&mut self, name: &str) -> PortId {
        match self.names.iter().position(|n| n == name) {
            Some(i) => PortId(i),
            None => {
                self.names.push(name.to_string());
                PortId(self.names.len() - 1)
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<PortId> {
        self.names.iter().position(|n| n == name).map(PortId)
    }

    pub fn name(&self, id: PortId) -> Option<&str> {
        self.names.get(id.0).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn ids(&self) -> impl Iterator<Item = PortId> {
        (0..self.names.len()).map(PortId)
    }
}

/// All Notes Off. Sent per channel by a panic.
pub const CC_ALL_NOTES_OFF: u8 = 123;
/// All Sound Off — stops even notes already released into a long release.
pub const CC_ALL_SOUND_OFF: u8 = 120;

/// One MIDI message, in the form the engine reasons about rather than as bytes.
///
/// `Nrpn` is one variant rather than four `ControlChange`s because the four have
/// to stay together and in order on one port; splitting them at schedule time
/// would let the queue interleave another event's CC between the parameter
/// select and its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiMsg {
    NoteOn { channel: u8, pitch: u8, velocity: u8 },
    NoteOff { channel: u8, pitch: u8 },
    ControlChange { channel: u8, controller: u8, value: u8 },
    /// The p-lock audition path. NRPN rather than plain CC for the reasons
    /// `js/midi.js` gives from the boxes' own MIDI appendices: it reaches
    /// parameters that have no CC at all, it carries the full 14 bits, and its
    /// numbering is largely shared between DT2 and DN2 where CC numbering very
    /// much is not — pan is CC 90 on a DT2 and CC 89 on a DN2, where 89 is
    /// Volume, a mix-up that would ride the wrong fader.
    Nrpn { channel: u8, msb: u8, lsb: u8, value14: u16 },
    Clock,
    Start,
    Stop,
    Continue,
}

impl MidiMsg {
    /// Wire bytes. `Nrpn` is the MIDI standard's four-message form: select the
    /// parameter with CC 99 (MSB) and CC 98 (LSB), then the value as CC 6 (MSB)
    /// and CC 38 (LSB).
    pub fn write_bytes(&self, out: &mut Vec<u8>) {
        match *self {
            MidiMsg::NoteOn { channel, pitch, velocity } => {
                out.extend_from_slice(&[0x90 | (channel & 0x0f), pitch & 0x7f, velocity.clamp(1, 127)]);
            }
            MidiMsg::NoteOff { channel, pitch } => {
                out.extend_from_slice(&[0x80 | (channel & 0x0f), pitch & 0x7f, 0]);
            }
            MidiMsg::ControlChange { channel, controller, value } => {
                out.extend_from_slice(&[0xb0 | (channel & 0x0f), controller & 0x7f, value.min(127)]);
            }
            MidiMsg::Nrpn { channel, msb, lsb, value14 } => {
                let ch = 0xb0 | (channel & 0x0f);
                let v = value14.min(0x3fff);
                out.extend_from_slice(&[ch, 99, msb & 0x7f]);
                out.extend_from_slice(&[ch, 98, lsb & 0x7f]);
                out.extend_from_slice(&[ch, 6, ((v >> 7) & 0x7f) as u8]);
                out.extend_from_slice(&[ch, 38, (v & 0x7f) as u8]);
            }
            MidiMsg::Clock => out.push(0xf8),
            MidiMsg::Start => out.push(0xfa),
            MidiMsg::Continue => out.push(0xfb),
            MidiMsg::Stop => out.push(0xfc),
        }
    }

    pub fn to_bytes(self) -> Vec<u8> {
        let mut v = Vec::with_capacity(8);
        self.write_bytes(&mut v);
        v
    }

    /// Tie-break rank for events sharing a deadline. Lower goes first.
    ///
    /// The order is not cosmetic:
    ///
    /// - transport messages before anything they gate;
    /// - clock before notes, so a box that just started is already counting;
    /// - **note-off before note-on**, which is what makes re-triggering a
    ///   sounding pitch work rather than leaving it stuck;
    /// - parameter changes before the note they belong to, so a p-lock has
    ///   landed by the time the trig sounds — what a real p-lock does.
    pub fn rank(&self) -> u8 {
        match self {
            MidiMsg::Start | MidiMsg::Stop | MidiMsg::Continue => 0,
            MidiMsg::Clock => 1,
            MidiMsg::NoteOff { .. } => 2,
            MidiMsg::ControlChange { .. } | MidiMsg::Nrpn { .. } => 3,
            MidiMsg::NoteOn { .. } => 4,
        }
    }

    /// The last tie-break: channel, then the first data byte.
    ///
    /// Without it the sort is not a total order, and two messages of the same
    /// rank on the same port at the same deadline come out in whatever order they
    /// happened to be pushed — which depends on the internal layout of the active
    /// note table and on how the caller chopped the timeline into windows. Two
    /// note-offs swapping places is inaudible, but "the same session produces the
    /// same bytes in the same order" is a property worth being able to rely on
    /// when a run has to be reproduced from a seed.
    pub fn tie_break(&self) -> (u8, u8) {
        match *self {
            MidiMsg::NoteOn { channel, pitch, .. } | MidiMsg::NoteOff { channel, pitch } => {
                (channel, pitch)
            }
            MidiMsg::ControlChange { channel, controller, .. } => (channel, controller),
            MidiMsg::Nrpn { channel, msb, .. } => (channel, msb),
            MidiMsg::Clock | MidiMsg::Start | MidiMsg::Stop | MidiMsg::Continue => (0, 0),
        }
    }
}

/// One entry in the queue: when, where, what.
///
/// `at` is seconds since the transport started, not an `Instant`. The conversion
/// to a wall-clock deadline happens once, in the transport thread — which keeps
/// everything that decides *what goes out when* a pure function of the session
/// and the tempo, and therefore testable without a clock or a box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduledEvent {
    pub at: f64,
    pub port: PortId,
    pub msg: MidiMsg,
}

impl ScheduledEvent {
    pub fn new(at: f64, port: PortId, msg: MidiMsg) -> Self {
        Self { at, port, msg }
    }

    /// Sort key. Deadline first — across every device, which is the point — then
    /// [`MidiMsg::rank`], then the port, then [`MidiMsg::tie_break`], which
    /// together make the order total.
    pub fn sort_key(&self) -> (f64, u8, usize, (u8, u8)) {
        (self.at, self.msg.rank(), self.port.0, self.msg.tie_break())
    }
}

/// Sort a window of events into send order.
pub fn sort_events(events: &mut [ScheduledEvent]) {
    events.sort_by(|a, b| {
        let (at_a, rank_a, port_a, tie_a) = a.sort_key();
        let (at_b, rank_b, port_b, tie_b) = b.sort_key();
        at_a.partial_cmp(&at_b)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(rank_a.cmp(&rank_b))
            .then(port_a.cmp(&port_b))
            .then(tie_a.cmp(&tie_b))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_table_interns_by_name_and_reads_back() {
        let mut t = PortTable::new();
        let a = t.intern("Digitakt II");
        let b = t.intern("Digitone II");
        assert_eq!(t.intern("Digitakt II"), a, "interning is idempotent");
        assert_ne!(a, b);
        assert_eq!(t.name(a), Some("Digitakt II"));
        assert_eq!(t.get("nothing here"), None);
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn nrpn_expands_to_the_standard_four_messages() {
        let bytes = MidiMsg::Nrpn { channel: 2, msb: 1, lsb: 40, value14: 0x2000 }.to_bytes();
        assert_eq!(
            bytes,
            vec![0xb2, 99, 1, 0xb2, 98, 40, 0xb2, 6, 0x40, 0xb2, 38, 0x00]
        );
    }

    #[test]
    fn a_note_on_never_goes_out_at_velocity_zero() {
        // 0x90 with velocity 0 *is* a note-off on the wire, so a note the user
        // set to 0 would silently become one.
        let bytes = MidiMsg::NoteOn { channel: 0, pitch: 60, velocity: 0 }.to_bytes();
        assert_eq!(bytes, vec![0x90, 60, 1]);
    }

    #[test]
    fn transport_bytes_are_the_realtime_ones() {
        assert_eq!(MidiMsg::Clock.to_bytes(), vec![0xf8]);
        assert_eq!(MidiMsg::Start.to_bytes(), vec![0xfa]);
        assert_eq!(MidiMsg::Continue.to_bytes(), vec![0xfb]);
        assert_eq!(MidiMsg::Stop.to_bytes(), vec![0xfc]);
    }

    #[test]
    fn a_note_off_sorts_before_a_note_on_at_the_same_deadline() {
        let port = PortId(0);
        let mut events = vec![
            ScheduledEvent::new(1.0, port, MidiMsg::NoteOn { channel: 0, pitch: 60, velocity: 100 }),
            ScheduledEvent::new(1.0, port, MidiMsg::Nrpn { channel: 0, msb: 1, lsb: 1, value14: 0 }),
            ScheduledEvent::new(1.0, port, MidiMsg::NoteOff { channel: 0, pitch: 60 }),
            ScheduledEvent::new(1.0, port, MidiMsg::Clock),
        ];
        sort_events(&mut events);
        let ranks: Vec<u8> = events.iter().map(|e| e.msg.rank()).collect();
        assert_eq!(ranks, vec![1, 2, 3, 4], "clock, off, p-lock, on");
    }

    #[test]
    fn events_on_two_ports_interleave_by_deadline_not_by_port() {
        let (dt2, dn2) = (PortId(0), PortId(1));
        let mut events = vec![
            ScheduledEvent::new(0.50, dt2, MidiMsg::Clock),
            ScheduledEvent::new(0.25, dn2, MidiMsg::Clock),
            ScheduledEvent::new(0.75, dn2, MidiMsg::Clock),
            ScheduledEvent::new(0.00, dt2, MidiMsg::Clock),
        ];
        sort_events(&mut events);
        let order: Vec<(f64, usize)> = events.iter().map(|e| (e.at, e.port.0)).collect();
        assert_eq!(order, vec![(0.0, 0), (0.25, 1), (0.5, 0), (0.75, 1)]);
    }
}
