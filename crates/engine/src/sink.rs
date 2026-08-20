//! Where the engine's bytes actually go.
//!
//! One `midir` connection per port, opened once and held for the life of the
//! run. PLAN.md §4 is specific about why the shape matters: sending on N ports
//! raises per-tick syscall cost, so events are batched per port per wake-up and a
//! clock tick costs N sends — not N × tracks.

use digi_midi::{open_output_by_name, MidiError, MidiOutputConnection};

use crate::event::{PortId, PortTable};
use crate::transport::PortSink;

/// A `PortSink` over real MIDI outputs, indexed by [`PortId`].
///
/// A port that could not be opened is `None` and silently drops what is sent to
/// it. That is deliberate: a box unplugged mid-set must not stop the other one
/// playing, and the UI already shows which devices have live ports.
pub struct MidirSink {
    connections: Vec<Option<MidiOutputConnection>>,
}

impl MidirSink {
    /// Open every port in the table, in `PortId` order.
    ///
    /// Returns the sink plus the ports that would not open, so the caller can say
    /// so rather than leaving the user wondering why one box is silent.
    pub fn open(ports: &PortTable) -> (Self, Vec<(PortId, MidiError)>) {
        let mut connections = Vec::with_capacity(ports.len());
        let mut failed = Vec::new();
        for id in ports.ids() {
            let name = ports.name(id).unwrap_or_default();
            match open_output_by_name(name) {
                Ok(conn) => connections.push(Some(conn)),
                Err(e) => {
                    failed.push((id, e));
                    connections.push(None);
                }
            }
        }
        (Self { connections }, failed)
    }

    pub fn open_count(&self) -> usize {
        self.connections.iter().filter(|c| c.is_some()).count()
    }
}

impl PortSink for MidirSink {
    fn send(&mut self, port: PortId, bytes: &[u8]) {
        if let Some(Some(conn)) = self.connections.get_mut(port.0) {
            // A failed send means the device went away mid-set. There is nothing
            // useful to do about it on this thread and it must not stop the
            // other port — the same call `js/midi.js` makes with its bare catch.
            let _ = conn.send(bytes);
        }
    }
}
