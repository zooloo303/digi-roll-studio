// Port enumeration and binding.
//
// A binding records both the OS port id and the port name. The id is the
// better key — midir 0.10.1 added `id()` precisely so a port survives being
// renamed or duplicated — but it is not guaranteed stable across OS versions,
// and a session file written on one machine may be opened on another. So a
// binding resolves by id first and falls back to the name, which is what
// PLAN.md §6 Phase 2 asks for and is strictly weaker on its own: two Digitakt
// IIs on one host share a name but not an id.

use midir::{MidiInput, MidiOutput};

use crate::MidiError;
use digi_protocol::device::slug_from_port_name;

/// What the OS shows as this app's MIDI client.
pub const CLIENT_NAME: &str = "digi-roll-studio";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    /// OS-assigned port id, used as the primary key when re-binding.
    pub id: String,
    pub name: String,
    /// Which box the *name* looks like. A guess, not the handshake's answer —
    /// `None` means "don't know", never "default".
    pub slug: Option<&'static str>,
}

/// A port remembered across runs. Serde lives on the session side; this stays
/// a plain pair so `core` can own the persistence format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortBinding {
    pub id: String,
    pub name: String,
}

impl From<&PortInfo> for PortBinding {
    fn from(p: &PortInfo) -> Self {
        PortBinding { id: p.id.clone(), name: p.name.clone() }
    }
}

fn info(id: String, name: String) -> PortInfo {
    let slug = slug_from_port_name(&name);
    PortInfo { id, name, slug }
}

pub fn list_inputs() -> Result<Vec<PortInfo>, MidiError> {
    let midi_in = MidiInput::new(CLIENT_NAME)?;
    Ok(midi_in
        .ports()
        .iter()
        .map(|p| info(p.id(), midi_in.port_name(p).unwrap_or_default()))
        .collect())
}

pub fn list_outputs() -> Result<Vec<PortInfo>, MidiError> {
    let midi_out = MidiOutput::new(CLIENT_NAME)?;
    Ok(midi_out
        .ports()
        .iter()
        .map(|p| info(p.id(), midi_out.port_name(p).unwrap_or_default()))
        .collect())
}

/// Both directions of one box, as far as the port names can tell. Elektron
/// boxes expose an input and an output with the same name, so pairing by name
/// is right here even though binding prefers the id.
pub fn find_port_pair(name_fragment: &str) -> Result<Option<(PortInfo, PortInfo)>, MidiError> {
    let needle = name_fragment.to_lowercase();
    let inputs = list_inputs()?;
    let outputs = list_outputs()?;
    let input = inputs.into_iter().find(|p| p.name.to_lowercase().contains(&needle));
    let output = outputs.into_iter().find(|p| p.name.to_lowercase().contains(&needle));
    Ok(match (input, output) {
        (Some(i), Some(o)) => Some((i, o)),
        _ => None,
    })
}

/// Open an output connection to the port with this exact name.
///
/// The engine's sink is built from these, one per port: `midir` connections are
/// one per port and a tick is N sends, not N × tracks (PLAN.md §4). Exposed here
/// rather than opened in `engine` so this crate stays the only one that knows
/// what `midir` is — `engine` sends bytes at a port, and nothing more.
pub fn open_output_by_name(name: &str) -> Result<midir::MidiOutputConnection, MidiError> {
    let midi_out = MidiOutput::new(CLIENT_NAME)?;
    let port = midi_out
        .ports()
        .into_iter()
        .find(|p| midi_out.port_name(p).as_deref() == Ok(name))
        .ok_or_else(|| MidiError::PortNotFound(name.to_string()))?;
    midi_out
        .connect(&port, name)
        .map_err(|e| MidiError::Connect(e.to_string()))
}

/// An input port held open, accumulating whole SysEx frames until asked for
/// them.
///
/// [`capture_sysex`] listens for a fixed window and hands back what arrived,
/// which is the right shape for "did the box reply to what I just sent". It is
/// the wrong shape for the Analog Four, whose dumps are started by a person
/// walking over and pressing a button: that wait has no window worth naming in
/// advance, has to stay cancellable, and should report frames as they land
/// rather than in one lump at the end. So the connection is a value that can be
/// held and drained — see [`crate::a4_transfer`], which is the reason this
/// exists.
///
/// `capture_sysex` is written in terms of it, so there is one copy of the
/// `midir` wiring and the reassembler rather than two that can drift.
pub struct SysExInbox {
    // Held only to keep the connection alive: dropping it closes the port.
    _conn: midir::MidiInputConnection<()>,
    frames: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    /// Set while a frame is open, so a caller can tell "nothing is coming" from
    /// "14 KB is halfway here". Without it a quiet-period test would call the
    /// gap between two driver callbacks the end of the dump.
    mid_frame: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl SysExInbox {
    /// Open `name` for input and start collecting.
    pub fn open(name: &str) -> Result<Self, MidiError> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};

        let mut midi_in = MidiInput::new(CLIENT_NAME)?;
        // Default settings filter SysEx out, which would report every box as
        // silent.
        midi_in.ignore(midir::Ignore::None);
        let port = midi_in
            .ports()
            .into_iter()
            .find(|p| midi_in.port_name(p).as_deref() == Ok(name))
            .ok_or_else(|| MidiError::PortNotFound(name.to_string()))?;

        let frames: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let mid_frame = Arc::new(AtomicBool::new(false));
        let sink = Arc::clone(&frames);
        let flag = Arc::clone(&mid_frame);
        let mut reasm = crate::sysex_stream::SysExReassembler::new();
        let conn = midi_in
            .connect(
                &port,
                name,
                move |_ts, bytes, _| {
                    let done = reasm.push(bytes);
                    if !done.is_empty() {
                        sink.lock().unwrap().extend(done);
                    }
                    flag.store(reasm.is_mid_frame(), Ordering::Relaxed);
                },
                (),
            )
            .map_err(|e| MidiError::Connect(e.to_string()))?;

        Ok(Self { _conn: conn, frames, mid_frame })
    }

    /// Every frame completed since the last call. Never blocks.
    pub fn drain(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *self.frames.lock().unwrap())
    }

    /// True when a frame is open — bytes are arriving right now.
    pub fn mid_frame(&self) -> bool {
        self.mid_frame.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Listen on one input port and collect whole SysEx frames for a fixed window.
///
/// Exists so an example can hear a box's *reply* without learning what `midir`
/// is — this crate stays the only one that does, which is the rule the rest of
/// this file is built on. Reassembly goes through `SysExReassembler` because a
/// long dump does not arrive in one callback.
///
/// Returns whatever completed inside the window, including nothing. Silence is
/// a real answer here: a box that declines a message often declines it quietly.
pub fn capture_sysex(
    name: &str,
    window: std::time::Duration,
) -> Result<Vec<Vec<u8>>, MidiError> {
    let mut inbox = SysExInbox::open(name)?;
    std::thread::sleep(window);
    Ok(inbox.drain())
}

/// Resolve a remembered binding against the ports present now: id first, then
/// name. Returns `None` when the device is not plugged in.
pub(crate) fn resolve_input(
    midi_in: &MidiInput,
    binding: &PortBinding,
) -> Option<midir::MidiInputPort> {
    if let Some(p) = midi_in.find_port_by_id(&binding.id) {
        return Some(p);
    }
    midi_in
        .ports()
        .into_iter()
        .find(|p| midi_in.port_name(p).as_deref() == Ok(binding.name.as_str()))
}

pub(crate) fn resolve_output(
    midi_out: &MidiOutput,
    binding: &PortBinding,
) -> Option<midir::MidiOutputPort> {
    if let Some(p) = midi_out.find_port_by_id(&binding.id) {
        return Some(p);
    }
    midi_out
        .ports()
        .into_iter()
        .find(|p| midi_out.port_name(p).as_deref() == Ok(binding.name.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Enumeration must work on a machine with no MIDI hardware at all — CI runs
    // there. An empty list is a valid answer; an error is not.
    #[test]
    fn enumerating_ports_never_fails_without_hardware() {
        assert!(list_inputs().is_ok());
        assert!(list_outputs().is_ok());
    }

    #[test]
    fn port_info_carries_the_name_guess() {
        let p = info("id-1".into(), "Elektron Digitone II".into());
        assert_eq!(p.slug, Some("digitone2"));
        let unknown = info("id-2".into(), "Scarlett 2i2".into());
        assert_eq!(unknown.slug, None);
    }

    #[test]
    fn binding_is_built_from_both_keys() {
        let p = info("id-1".into(), "Elektron Digitakt II".into());
        let b = PortBinding::from(&p);
        assert_eq!(b.id, "id-1");
        assert_eq!(b.name, "Elektron Digitakt II");
    }
}
